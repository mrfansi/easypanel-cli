use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use anyhow::Result;
use serde_json::{json, Value};

use crate::client::EasypanelClient;
use crate::cloudflare::{
    object_basename, record_body, upload_key, AnalyticsSummary, CloudflareClient, CloudflareTunnel,
    R2Bucket, R2Object, Record, RecordFilter, TunnelConfiguration, TunnelIngressRule,
    TunnelRouteChange, WebAnalyticsSite, WorkerDeployment, WorkerScript, WorkerSettingsBundle,
    WorkerUploadMode, Zone, DNS_BATCH_LIMIT, MAX_REST_OBJECT_BYTES,
};
use crate::output::field;

// ---------- Worker (networking on a separate thread so the UI doesn't freeze) ----------

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum View {
    Logs,
    Env,
    Ports,
    Mounts,
    Redirects,
    Backups,
    Source,
    ConfigFile,
    /// The env shared by every service in a PROJECT. Not a service view — it is
    /// fetched with the service name empty.
    ProjectEnv,
}

impl View {
    /// Does this view show ROWS you act on one at a time, rather than free text?
    ///
    /// A collection gets a selected row and `x` to delete it. Logs, env and an
    /// action's output are prose: selecting a line there means nothing.
    pub(crate) fn is_collection(self) -> bool {
        matches!(self, View::Ports | View::Mounts | View::Redirects)
    }

    pub(crate) fn title(self) -> &'static str {
        match self {
            View::Logs => "Logs",
            View::Env => "Env",
            View::Ports => "Ports",
            View::Mounts => "Mounts",
            View::Redirects => "Redirects",
            View::Backups => "Database backups",
            View::Source => "Source & build",
            View::ConfigFile => "Config file",
            View::ProjectEnv => "Project env",
        }
    }
}

pub(crate) enum Req {
    Stats,
    Nodes,
    Projects,
    Actions,
    /// Detail of one action (getAction): metadata + `log` (deploy/action output).
    ActionDetail(String),
    MonitorData,
    /// Swarm replicas per service (actual/desired) — the ground truth for "running
    /// as targeted?". One call covers every service; `actual < desired` = down.
    TaskStats,
    Storage,
    Domains,
    Fetch {
        view: View,
        project: String,
        service: String,
        stype: String,
    },
    Action {
        project: String,
        service: String,
        stype: String,
        action: String,
        /// Deploy only: rebuild from scratch instead of reusing the layer cache.
        /// Ignored by every other action — the endpoint takes no such field.
        force: bool,
    },
    /// The same lifecycle action across many services at once.
    ///
    /// EasyPanel has NO batch endpoint — every candidate (`projects/deployProject`,
    /// `services/deployMany`, …) answers with the bare 404 an unknown route gives,
    /// not the 400 a real op gives for a bad argument. So bulk is a client-side
    /// fan-out over the per-service calls, and each one can fail on its own.
    Bulk {
        targets: Vec<(String, String, String)>,
        action: String,
        force: bool,
    },
    /// Compare two services field by field. Each is (project, service, type) —
    /// the type picks the `services/{type}` endpoint group for inspectService.
    DiffServices {
        a: (String, String, String),
        b: (String, String, String),
    },
    /// Compare a local service against the same one on ANOTHER host. The target
    /// host's url+token arrive here because only the event loop can read them.
    DiffAcrossHosts {
        local: (String, String, String),
        target: EasypanelClient,
        target_name: String,
    },
    /// Compare a whole project against the same project on another host. Only two
    /// calls: inspectProject carries every service's full config.
    DiffProjectAcross {
        project: String,
        target: EasypanelClient,
        target_name: String,
    },
    /// Open the deploy form: replicas, start command, zero-downtime.
    DeployForm {
        project: String,
        service: String,
    },
    /// Save the deploy block (updateDeploy).
    DeploySave {
        project: String,
        service: String,
        deploy: Value,
    },
    /// Open the edit form for one mount: fetch what it currently IS, so the box
    /// is prefilled rather than asking the user to retype a path from memory.
    MountForm {
        project: String,
        service: String,
        index: usize,
    },
    /// Save an edited mount (updateMount). `index` is its position as listed.
    MountUpdate {
        project: String,
        service: String,
        index: usize,
        values: Value,
    },
    /// Where backups are written. Listed once at start-up: EasyPanel offers no
    /// way to create one over the API, so this only ever reads what the
    /// dashboard already has.
    StorageProviders,
    /// Which databases this service actually holds.
    ///
    /// EasyPanel records only the one it created, but a server holds many and the
    /// backup endpoint accepts any of them. No API lists them, so the engine is
    /// asked directly through the container shell.
    DatabasesIn {
        project: String,
        service: String,
        stype: String,
    },
    /// One step of the database browser: list the databases, list a database's
    /// tables, preview a table's rows, or run what the user typed.
    ///
    /// EasyPanel has no query endpoint, so every step runs the engine's own
    /// client in batch mode inside the container (`crate::dbms` builds the
    /// command, `container::run_capture` runs it). One request variant rather
    /// than four: they differ only in which command is built, and splitting them
    /// would spread one feature over four identical handlers.
    Dbms {
        project: String,
        service: String,
        stype: String,
        level: crate::dbms::Level,
        /// Empty at the `Databases` level.
        database: String,
        /// Empty above the `Rows` level.
        table: String,
        /// Only used by the `Query` level.
        query: String,
    },
    /// Back this database up ONCE, right now.
    ///
    /// There is no such endpoint: `runDatabaseBackup` only runs a SCHEDULE. So a
    /// disabled schedule is created, run, and deleted again — verified live, a
    /// disabled schedule runs fine — which leaves no clutter behind in the
    /// panel's backup list.
    BackupNow {
        project: String,
        service: String,
        database: String,
        provider: String,
        path: String,
    },
    /// The backups that exist for this service, read out of the action history
    /// (nothing else lists the files).
    BackupHistory {
        project: String,
        service: String,
    },
    /// The same, but read from ANOTHER host — so a backup taken there can be
    /// restored here. Only backups on a REMOTE provider are usable: a local-disk
    /// one physically lives on that host and this one cannot read it.
    BackupHistoryFrom {
        src: EasypanelClient,
        src_name: String,
        project: String,
        service: String,
    },
    /// Restore one backup file INTO this service.
    RestoreBackup {
        project: String,
        service: String,
        database: String,
        provider: String,
        path: String,
    },
    /// Non-locking dump of the chosen databases straight to object storage — the
    /// same path as the CLI `db dump`, so the TUI is not stuck on the old locking
    /// backup. Databases already chosen in the picker.
    DumpR2 {
        project: String,
        service: String,
        databases: Vec<String>,
    },
    /// List object-storage dumps to restore INTO `project`/`service`.
    ///
    /// `all_services` widens the listing from that service's own dumps to every
    /// dump on the host: the destination is still `project`/`service`, so the
    /// scope decides only what is OFFERED, never what a restore is aimed at.
    R2Dumps {
        project: String,
        service: String,
        all_services: bool,
    },
    /// Download one of those dumps (by object key) to the current directory.
    DownloadR2 {
        project: String,
        service: String,
        path: String,
    },
    /// Restore one of those dumps (by object key) back INTO the service.
    RestoreR2 {
        project: String,
        service: String,
        path: String,
    },
    /// Copy databases OUT of `project`/`service` and INTO another service, which
    /// may live on another host — the same two-leg composition as the CLI
    /// `db copy` (dump to shared storage, then load from it).
    ///
    /// The target's url+token are resolved in event_loop, the only place holding
    /// the ServerConfig; the worker is bound to one host. For a SAME-host copy
    /// they are simply this host's own, which needs no special case.
    ///
    /// Preview resolves the facts; Confirmed carries the operator-reviewed facts
    /// unchanged into execution, where operations revalidates them before writes.
    CopyDb {
        target: EasypanelClient,
        /// The target's configured name — for the status line, not the call.
        target_name: String,
        target_project: String,
        target_service: String,
        project: String,
        service: String,
        stage: CopyDbStage,
    },
    /// All services across projects in a single call.
    AllServices,
    /// Load a project's services for a form dropdown (not the Projects panel).
    ServicesFor(String),
    /// Open the source/build form: needs inspectService (the current values) and —
    /// for source — the list of GitHub repos for its dropdown.
    ConfigForm {
        project: String,
        service: String,
        build: bool,
    },
    /// Open the resource limit form: inspectService (group services/{stype}) for
    /// the current `resources` values.
    ResourceForm {
        project: String,
        service: String,
        stype: String,
    },
    /// Save the resource limit (updateResources). The group follows `stype` since
    /// resources exist on every service type, not just app.
    ResourceSave {
        project: String,
        service: String,
        stype: String,
        resources: Value,
    },
    /// Set the same resource limit on every marked service. Each is updated under
    /// its own `services/{stype}` group; a type with no limits is reported, not
    /// silently skipped.
    BulkResource {
        targets: Vec<(String, String, String)>,
        resources: Value,
    },
    /// Open the basic auth form: inspectService for the current `basicAuth`.
    BasicAuthForm {
        project: String,
        service: String,
        stype: String,
    },
    /// Save basic auth (updateBasicAuth). Only web services (app/box/compose/
    /// wordpress) have this endpoint.
    BasicAuthSave {
        project: String,
        service: String,
        stype: String,
        basic_auth: Value,
    },
    /// Server info for the Maintenance tab (Docker version, IP, update availability).
    MaintInfo,
    /// Docker cleanup: systemPrune / cleanupDockerImages / cleanupDockerBuilder.
    MaintAction(&'static str),
    /// Search for `query` in the logs of ALL services at once (the killer
    /// feature). Parallel fan-out; results grouped per service.
    LogSearch {
        query: String,
    },
    /// The next log-tail round. `since` = the newest timestamp already seen; None =
    /// the first batch.
    LogTail {
        project: String,
        service: String,
        since: Option<String>,
    },
    /// The list of GitHub repos for the "Repo" dropdown in the "New service" form.
    ///
    /// The source form uses ConfigForm, but that needs a service that ALREADY
    /// exists — the create form doesn't have one yet.
    Repos,
    /// A repo's branches for the "Branch" dropdown (triggered once a repo is chosen).
    Branches {
        owner: String,
        repo: String,
    },
    /// `op` picks the endpoint: updateSourceGithub/Git/Image, or updateBuild.
    ///
    /// `auto_deploy` follows via enable/disableGithubDeploy: updateSourceGithub
    /// always resets autoDeploy to false (verified on the server), so its value has
    /// to be reapplied after the update — otherwise changing the branch would
    /// silently disable auto-deploy.
    ConfigSave {
        project: String,
        service: String,
        op: &'static str,
        body: Value,
        auto_deploy: Option<bool>,
    },
    /// Turn auto deploy on/off without touching the source.
    ///
    /// Separate from ConfigSave: going through there would mean resending
    /// updateSourceGithub, which resets autoDeploy to false and then reapplies it —
    /// two calls and one window where the value is wrong, just to flip a bool.
    AutoDeploy {
        project: String,
        service: String,
        on: bool,
    },
    ProjectCreate(String),
    ProjectDestroy(String),
    ServiceCreate {
        project: String,
        service: String,
        stype: String,
        /// Fields safe to send inline to createService: db (databaseName, user, …),
        /// build, env, dotEnvPath, domains. These are all fast and do NOT trigger a
        /// deploy. Only fields the user filled are included: empty = the server
        /// creates them.
        extra: Value,
        /// The source is applied SEPARATELY after createService (updateSource*),
        /// because inline it triggers a 100-second deploy. (op, body, auto_deploy).
        source: Option<crate::form::SourceCall>,
    },
    /// Continue configuration of an already-created service; never creates it again.
    ServiceSetup {
        project: String,
        service: String,
        stype: String,
        source: crate::form::SourceCall,
        phase: ServiceSetupPhase,
    },
    /// Clone a service — config only, WITHOUT data — into `new_name` in the same
    /// project. A killer feature: EasyPanel has no clone endpoint; this is a
    /// composition of inspectService → createService (minus source) →
    /// updateSource*/updateAdvanced.
    CloneService {
        project: String,
        service: String,
        stype: String,
        /// The clone's target project (may differ from the source). Must be a
        /// project that ALREADY exists — otherwise its Docker network isn't ready
        /// yet (race).
        target: String,
        new_name: String,
    },
    /// Move service CONFIG (never data) to another EasyPanel host.
    ///
    /// The destination's url+token are resolved in event_loop, which is the only
    /// place holding the ServerConfig; the worker is bound to one host.
    Migrate {
        target: EasypanelClient,
        /// The destination's configured name — for the status line, not the call.
        target_name: String,
        target_project: String,
        /// What to move, as (project, service, type). One entry migrates a single
        /// service; a whole project is the same operation over its service list.
        services: Vec<(String, String, String)>,
    },
    /// Apply an already-planned rewrite to many domains: (id, what it read
    /// before, the new body). The plan is made and shown before this is sent, so
    /// the worker only has to deliver it.
    DomainBulkEdit {
        changes: Vec<(String, String, Value)>,
    },
    /// Delete many domains by id, reporting per-domain failures.
    DomainBulkDelete {
        domains: Vec<(String, String)>,
    },
    /// Ask every watched domain whether it answers, and how fast. Not an
    /// EasyPanel call at all — it goes to the domains themselves, which is the
    /// whole point: the panel can only tell you what it INTENDED to serve.
    RunChecks(Vec<crate::uptime::Check>),
    /// Replace a project's shared env (`updateProjectEnv`).
    ProjectEnvSave {
        project: String,
        env: String,
    },
    DomainSave {
        id: Option<String>,
        body: Value,
    },
    /// Add a port (createPort) to a service.
    PortSave {
        project: String,
        service: String,
        values: Value,
    },
    /// Delete a port by its index in listPorts (deletePort), then reload the port
    /// list into the viewer so the deleted one disappears immediately.
    PortDelete {
        project: String,
        service: String,
        index: usize,
    },
    /// Add a mount (createMount) to a service.
    MountSave {
        project: String,
        service: String,
        values: Value,
    },
    /// Delete a mount by index (deleteMount), then reload the mount list.
    MountDelete {
        project: String,
        service: String,
        index: usize,
    },
    /// Add one redirect. There's no per-item endpoint: read the current redirects,
    /// append, then updateRedirects the whole array (read-modify-write).
    RedirectAdd {
        project: String,
        service: String,
        stype: String,
        redirect: Value,
    },
    /// Delete a redirect by index: read, drop that index, updateRedirects.
    RedirectDelete {
        project: String,
        service: String,
        stype: String,
        index: usize,
    },
    DomainDelete(String),
    DomainSetPrimary(String),
    EnvSave {
        project: String,
        service: String,
        stype: String,
        env: String,
    },
    /// Turn writing env as a `.env` file (`dotEnvPath`) on/off. Read the current
    /// state then flip it: if already on → turn off, if off → turn on at the path
    /// `.env`.
    EnvFileToggle {
        project: String,
        service: String,
        stype: String,
    },
    /// Save the Config File (Advanced db) via updateAdvanced. `config` = new contents.
    ConfigFileSave {
        project: String,
        service: String,
        stype: String,
        config: String,
    },
    /// A Cloudflare request. Cloudflare is a bounded context outside EasyPanel, so
    /// its work rides the SAME worker lanes but is dispatched apart: the worker
    /// builds a `CloudflareClient` from the token carried IN the request (resolved
    /// on the main thread from the active account), never from EasyPanel config.
    Cf(CfReq),
}

/// A Cloudflare worker request. The token travels in every variant — the worker is
/// stateless about CF config and builds a fresh client per request. Never logged.
pub(crate) enum CfReq {
    Analytics {
        token: String,
        account_id: String,
        days: u16,
    },
    Zones {
        token: String,
        account_id: Option<String>,
    },
    Records {
        token: String,
        zone_id: String,
        filter: RecordFilter,
    },
    WebAnalyticsSites {
        token: String,
        account_id: String,
    },
    CreateZone {
        token: String,
        name: String,
        account_id: String,
    },
    DeleteZone {
        token: String,
        zone_id: String,
    },
    CreateRecord {
        token: String,
        zone_id: String,
        body: Value,
    },
    PatchRecord {
        token: String,
        zone_id: String,
        id: String,
        body: Value,
    },
    DeleteRecord {
        token: String,
        zone_id: String,
        id: String,
    },
    /// Apply the same patch to many records through Cloudflare's DNS batch endpoint.
    /// Chunked at `DNS_BATCH_LIMIT`, so one failed chunk does not abort later chunks.
    BulkPatch {
        token: String,
        zone_id: String,
        ids: Vec<String>,
        body: Value,
    },
    /// Delete many records through Cloudflare's DNS batch endpoint, chunked at the
    /// lowest official plan limit.
    BulkDelete {
        token: String,
        zone_id: String,
        ids: Vec<String>,
    },
    /// R2 buckets are account-scoped, so the account_id travels alongside the token
    /// (the main thread resolves both from the active account).
    R2Buckets {
        token: String,
        account_id: String,
    },
    Tunnels {
        token: String,
        account_id: String,
    },
    TunnelCreate {
        token: String,
        account_id: String,
        name: String,
    },
    TunnelDelete {
        token: String,
        account_id: String,
        tunnel_id: String,
        name: String,
    },
    TunnelToken {
        token: String,
        account_id: String,
        tunnel_id: String,
        name: String,
    },
    TunnelConfig {
        token: String,
        account_id: String,
        tunnel_id: String,
    },
    TunnelRouteAdd {
        token: String,
        account_id: String,
        tunnel_id: String,
        hostname: String,
        service: String,
        path: String,
        origin_request: Option<Value>,
        dns: bool,
        zone: Option<String>,
        proxied: bool,
    },
    TunnelRouteEdit {
        token: String,
        account_id: String,
        tunnel_id: String,
        hostname: String,
        path: String,
        service: String,
        origin_request: Option<Option<Value>>,
        dns: bool,
        zone: Option<String>,
        proxied: bool,
    },
    TunnelRouteDelete {
        token: String,
        account_id: String,
        tunnel_id: String,
        hostname: String,
        path: String,
        delete_dns: bool,
        zone: Option<String>,
    },
    Workers {
        token: String,
        account_id: String,
    },
    WorkerDeployments {
        token: String,
        account_id: String,
        name: String,
    },
    WorkerSettings {
        token: String,
        account_id: String,
        name: String,
    },
    WorkerDeploy {
        token: String,
        account_id: String,
        name: String,
        path: String,
        mode: WorkerUploadMode,
    },
    WorkerDelete {
        token: String,
        account_id: String,
        name: String,
        force: bool,
    },
    CreateR2Bucket {
        token: String,
        account_id: String,
        name: String,
    },
    DeleteR2Bucket {
        token: String,
        account_id: String,
        name: String,
    },
    /// List ONE folder level of a bucket via the REST API (delimiter=/) — the SAME Bearer
    /// token as buckets, no S3 credentials. Account-scoped, so the account_id travels
    /// alongside the token. `prefix` is "" at the bucket root, or e.g. `assets/css/` deeper.
    R2Objects {
        token: String,
        account_id: String,
        bucket: String,
        prefix: String,
    },
    /// Upload a LOCAL file into `bucket` at `prefix`. The worker reads the file (so the
    /// UI never blocks), enforces the 300 MB REST cap, and computes the key with
    /// `upload_key(prefix, path)`. Account-scoped, so the account_id travels along.
    R2Put {
        token: String,
        account_id: String,
        bucket: String,
        prefix: String,
        path: String,
    },
    /// Download ONE object to a local `dest`, refusing to overwrite an existing file.
    R2Get {
        token: String,
        account_id: String,
        bucket: String,
        key: String,
        dest: String,
    },
    /// Download MANY marked objects into `dir`, each under its basename (skips a name
    /// already present rather than clobbering). Reports per-key pass/fail.
    R2GetMany {
        token: String,
        account_id: String,
        bucket: String,
        keys: Vec<String>,
        dir: String,
    },
    /// Delete one or many object keys (single = a one-element vec), one call per key —
    /// a mid-list failure never aborts the rest. Reports per-key pass/fail; the app
    /// re-lists the level afterwards.
    R2Delete {
        token: String,
        account_id: String,
        bucket: String,
        keys: Vec<String>,
    },
}

pub(crate) enum CopyDbStage {
    Preview { databases: Vec<String>, all: bool },
    Confirmed(Box<crate::operations::CopyPlan>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ServiceSetupPhase {
    Source,
    AutoDeploy,
}

pub(crate) enum Resp {
    /// The resource exists even when subsequent configuration needs repair.
    ServiceCreated {
        project: String,
        service: String,
        stype: String,
        notes: Vec<String>,
        pending: Option<ServiceSetupPhase>,
    },
    Fetched {
        view: View,
        project: String,
        service: String,
        /// Global port/mount APIs have no service-type scope.
        stype: Option<String>,
        data: Value,
    },
    Stats(Value),
    /// The system-stats fetch FAILED. Kept apart from `Resp::Err` so the Dashboard
    /// can say "couldn't load" instead of drawing 0.0% gauges — numbers that read
    /// as real and are both alarming and falsely reassuring.
    StatsErr(String),
    Nodes(Vec<Value>),
    Projects(Vec<String>),
    Actions(Vec<Value>),
    MonitorData(Vec<Value>),
    /// (actual, desired) swarm replicas, keyed by the name "{project}_{service}".
    TaskStats(HashMap<String, (i64, i64)>),
    Storage(Vec<Value>),
    Domains(Vec<Value>),
    /// The domain list FAILED to load — kept separate from `Resp::Err` so the
    /// screen can tell "the host has no domains" from "the fetch died", instead of
    /// drawing the empty-state ("No domains yet") over a host with hundreds.
    DomainsErr(String),
    /// All services across projects + the project names for a form dropdown.
    AllServices {
        projects: Vec<String>,
        services: Vec<Value>,
    },
    /// The service list FAILED to load — kept apart from `Resp::Err` so an empty
    /// Services table reads as "couldn't load", not "this host has nothing".
    AllServicesErr(String),
    /// An operation that SUCCEEDED but left the user something to act on.
    ///
    /// The notes used to be appended to the status line — which is one line, so
    /// the sentence that mattered ("config file NOT applied because …") was cut
    /// off mid-word by the terminal width. Anything the user must act on gets the
    /// viewer, where it can be read in full.
    Notes {
        msg: String,
        notes: Vec<String>,
        refresh: Refresh,
    },
    /// The storage provider ids/names configured on this panel.
    /// The storage providers configured on this panel: (id, name, type).
    ///
    /// The TYPE matters: a `local` provider stores on THIS host's disk, so a
    /// backup written there can never be restored onto another host — verified,
    /// and the reason cross-server restore refuses one.
    StorageProviders(Vec<(String, String, String)>),
    /// The deploy block, to prefill its form.
    DeployForm {
        project: String,
        service: String,
        deploy: Value,
    },
    /// One mount's current values, to prefill its edit form.
    MountForm {
        project: String,
        service: String,
        index: usize,
        values: Value,
    },
    /// The databases a service holds, for choosing what to back up.
    DatabasesIn {
        project: String,
        service: String,
        names: Vec<String>,
    },
    /// One step of the database browser came back: the grid, where it belongs,
    /// and what is imperfect about it.
    Dbms {
        project: String,
        service: String,
        level: crate::dbms::Level,
        database: String,
        table: String,
        columns: Vec<String>,
        rows: Vec<Vec<String>>,
        /// The preview hit its LIMIT: there are more rows than these.
        capped: bool,
        /// The captured output was cut at the capture budget.
        truncated: bool,
    },
    /// A browser step FAILED, carrying the engine's own message.
    ///
    /// Kept apart from `Resp::Err` (which only writes the status line): a SQL
    /// error must be visible on the screen that asked for it, and an empty grid
    /// must never be the only sign that something went wrong.
    DbmsFailed {
        message: String,
    },
    /// The object-storage dumps to pick one of, and what `project`/`service` a
    /// pick would be restored INTO.
    ///
    /// Object keys only: the rows are built from them (`backup_ui::r2_rows`), so
    /// how a dump reads and which key a row aims at have one definition, and the
    /// scope travels along because it decides both the wording and the columns.
    R2Dumps {
        project: String,
        service: String,
        all_services: bool,
        keys: Vec<String>,
    },
    /// A copy that has been PLANNED but not run: what `plan_copy` resolved, ready
    /// to be shown and confirmed.
    ///
    /// Nonsecret typed facts stay intact through confirmation. Frontends format
    /// their own review; the App never receives storage credentials or tokens.
    CopyDbPlan {
        target_name: String,
        target_project: String,
        target_service: String,
        project: String,
        service: String,
        plan: Box<crate::operations::CopyPlan>,
    },
    /// The restorable backups found on ANOTHER host. `hidden` counts the ones
    /// left out because they sit on that host's local disk and cannot be read
    /// from here — said out loud, so a short list is never mistaken for "there
    /// are no backups".
    BackupHistoryFrom {
        src_name: String,
        project: String,
        service: String,
        hidden: usize,
        rows: Vec<String>,
        files: Vec<(String, String, String)>,
    },
    /// The restorable backups for a service: display rows plus, in the same
    /// order, what each one needs to actually be restored. Kept side by side so
    /// the restore reads its arguments from the LIST rather than parsing them
    /// back out of the text on screen.
    BackupHistory {
        project: String,
        service: String,
        rows: Vec<String>,
        files: Vec<(String, String, String)>,
    },
    /// The outcome of a bulk action, per service.
    ///
    /// Failures are carried individually rather than counted: "9 of 12 done" tells
    /// you something broke but not WHAT, and a bulk action that half-worked is
    /// exactly when the names matter. `ok` holds "project/service", `failed` holds
    /// it with the reason.
    BulkDone {
        action: String,
        ok: Vec<String>,
        failed: Vec<(String, String)>,
    },
    /// Every watched domain's answer, in the order they were asked.
    Checked(Vec<crate::uptime::Probe>),
    /// A project's shared env was saved. The services keep running the OLD values
    /// until they are deployed again, so the app turns this into an offer rather
    /// than a "saved" that quietly leaves nothing changed.
    ProjectEnvSaved(String),
    /// A bulk domain rewrite that has run. `failed` holds what each unchanged
    /// domain read, with the reason — a routing change that half-landed is
    /// exactly when the names matter.
    DomainsEdited {
        ok: usize,
        failed: Vec<(String, String)>,
    },
    DomainsDeleted {
        ok: usize,
        failed: Vec<(String, String)>,
    },
    ServicesFor(String, Vec<String>),
    /// Data to open the resource limit form: the inspectService result.
    ResourceForm {
        project: String,
        service: String,
        stype: String,
        data: Value,
    },
    /// Data to open the basic auth form: the inspectService result.
    BasicAuthForm {
        project: String,
        service: String,
        stype: String,
        data: Value,
    },
    /// Data to open the source/build form: the inspectService result + repo list.
    ConfigForm {
        project: String,
        service: String,
        build: bool,
        data: Value,
        repos: Vec<String>,
    },
    /// Log lines newer than `since`, plus a cursor for the next round.
    LogTail {
        lines: Vec<String>,
        cursor: Option<String>,
    },
    /// An empty list = GitHub not connected; "Repo" stays a text input.
    Repos(Vec<String>),
    /// Err = the branch list couldn't load (e.g. the GitHub token in EasyPanel is dead).
    Branches(std::result::Result<Vec<String>, String>),
    /// Per-row result. Typed rather than a stringly-typed "error: …" value, so
    /// the renderer can COLOUR a failure instead of drawing it in the same ink as
    /// a real reading — on the screen that carries three irreversible host-wide
    /// actions.
    MaintInfo(Vec<(String, Result<String, String>)>),
    /// The result for one host on the Hosts screen; each host arrives on its own so
    /// a slow/dead host doesn't hold up the others.
    HostStat {
        name: String,
        data: std::result::Result<Value, String>,
    },
    /// The server form's Ctrl-T: what opening that tunnel said.
    SshTested(std::result::Result<String, String>),
    Viewer(String, Vec<String>),
    /// Output bytes from a container terminal session (fed to the vt100 parser).
    TermOutput(Vec<u8>),
    /// The terminal session ended (shell exited / socket closed).
    TermClosed,
    /// A mutation succeeded: a status message + which data needs reloading.
    /// A long job saying where it has got to. Only the status line moves; the
    /// spinner keeps running because `busy` counts requests, not replies, so a
    /// dump can report for twenty minutes without ever looking finished.
    Progress(String),
    Done(String, Refresh),
    Err(String),
    /// A Cloudflare reply. Kept apart from the EasyPanel variants so the two
    /// contexts never share state — the app routes it to `app.cf`.
    Cf(CfResp),
}

/// A Cloudflare worker reply.
pub(crate) enum CfResp {
    Analytics(AnalyticsSummary),
    Zones(Vec<Zone>),
    /// The records for `zone_id` — the id is echoed back so a stale reply for a
    /// zone the user already left is discarded rather than drawn.
    Records {
        zone_id: String,
        records: Vec<Record>,
    },
    WebAnalyticsSites(Vec<WebAnalyticsSite>),
    /// Web Analytics metadata is an optional enrichment for the Domains table. A
    /// token without Account Settings Read must not make the zones list look failed.
    WebAnalyticsErr(String),
    /// The R2 buckets for the active account.
    R2Buckets(Vec<R2Bucket>),
    /// The Cloudflare Tunnels for the active account.
    Tunnels(Vec<CloudflareTunnel>),
    /// One tunnel's ingress/configuration. Tunnel id is echoed so stale replies can
    /// be ignored when the user opens another tunnel before the first request returns.
    TunnelConfig {
        tunnel_id: String,
        config: Box<TunnelConfiguration>,
    },
    Viewer {
        title: String,
        lines: Vec<String>,
    },
    /// The Worker scripts for the active account.
    Workers(Vec<WorkerScript>),
    /// The deployments/version history for one Worker. The worker name is echoed so
    /// stale replies can be ignored when the user opens another Worker.
    WorkerDeployments {
        worker: String,
        deployments: Vec<WorkerDeployment>,
    },
    WorkerSettings {
        worker: String,
        settings: Box<WorkerSettingsBundle>,
    },
    /// One folder level of `bucket` at `prefix` — the subfolders (`folders`, full key
    /// prefixes ending in `/`) and the files directly here. Both bucket AND prefix are
    /// echoed back so a stale reply for a level the user already left (a different bucket
    /// OR a different prefix) is discarded rather than drawn over the current one.
    R2Objects {
        bucket: String,
        prefix: String,
        folders: Vec<String>,
        objects: Vec<R2Object>,
        /// This level held more than one page; the app tells the user to narrow.
        truncated: bool,
    },
    /// A create/edit/delete succeeded; the app re-lists the affected screen.
    Done(String),
    /// An operation that set a status line but did NOT change the current level, so
    /// the app must NOT reload (a download writes a local file — the object list is
    /// unchanged, and a reload would flash the loading state for nothing).
    Status(String),
    /// A bulk fan-out: how many succeeded and, per failed id, the reason.
    BulkDone {
        ok: usize,
        failed: Vec<(String, String)>,
    },
    Err(String),
}

/// The data that needs refreshing after a mutation.
pub(crate) enum Refresh {
    Projects,
    Domains,
    None,
}

/// One worker lane: processes requests in order and sends the results to `resp_tx`.
/// Spawn a lane. `busy`, when given, counts what this lane is working on RIGHT
/// NOW — the honest answer to "is the user waiting for something?".
///
/// It is the worker that counts, not the sender, because only the worker knows
/// when the work is actually over. The alternative — inferring it from the status
/// text ending in "..." — was what the UI used to do, and it could not tell a
/// finished request from an abandoned message.
pub(crate) fn spawn_worker(
    client: EasypanelClient,
    resp_tx: Sender<Resp>,
    busy: Option<Arc<AtomicUsize>>,
) -> Sender<Req> {
    let (req_tx, req_rx) = mpsc::channel::<Req>();

    let progress_tx = resp_tx.clone();
    let emit: Emit = Arc::new(move |resp| {
        let _ = progress_tx.send(resp);
    });
    thread::spawn(move || {
        while let Ok(req) = req_rx.recv() {
            if let Some(b) = &busy {
                b.fetch_add(1, Ordering::Relaxed);
            }
            let resp = handle_req(&client, req, &emit);
            // Cleared BEFORE the reply is posted, so by the time the UI handles the
            // response the lane already reads idle — otherwise the spinner would
            // survive one extra frame past the answer.
            if let Some(b) = &busy {
                b.fetch_sub(1, Ordering::Relaxed);
            }
            if resp_tx.send(resp).is_err() {
                break;
            }
        }
    });

    req_tx
}

/// Two lanes: `user` for user actions, `poll` for periodic metrics.
///
/// getSystemStats/getMonitorTableData can each take ~2.5 seconds. With a single
/// lane, metric polling would block a user action (e.g. opening a tab) for that long.
pub(crate) struct Workers {
    pub(crate) user: Sender<Req>,
    pub(crate) poll: Sender<Req>,
    pub(crate) resp: Receiver<Resp>,
    /// For the Hosts screen fan-out: each host gets its own thread, so its result
    /// doesn't go through the user/poll lane bound to a single client.
    pub(crate) resp_tx: Sender<Resp>,
    /// How many user-initiated requests are in flight. Shared with the App, which
    /// uses it to decide whether anything is actually happening.
    pub(crate) busy: Arc<AtomicUsize>,
}

pub(crate) fn spawn_workers(client: EasypanelClient) -> Workers {
    let (resp_tx, resp) = mpsc::channel::<Resp>();
    let busy = Arc::new(AtomicUsize::new(0));
    // Only the user lane is counted. The poll lane refetches metrics every two
    // seconds; counting it would leave a spinner running permanently and tell the
    // user nothing about the action they actually asked for.
    let user = spawn_worker(client.clone(), resp_tx.clone(), Some(busy.clone()));
    let poll = spawn_worker(client, resp_tx.clone(), None);
    Workers {
        user,
        poll,
        resp,
        resp_tx,
        busy,
    }
}

pub(crate) type Emit = Arc<dyn Fn(Resp) + Send + Sync>;

pub(crate) fn handle_req(client: &EasypanelClient, req: Req, emit: &Emit) -> Resp {
    match req {
        Req::Stats => match client.call("metrics", "getSystemStats", json!({})) {
            Ok(v) => Resp::Stats(v),
            Err(e) => Resp::StatsErr(e.to_string()),
        },
        Req::Nodes => match client.call("cluster", "listNodes", Value::Null) {
            Ok(v) => Resp::Nodes(v.as_array().cloned().unwrap_or_default()),
            Err(e) => Resp::Err(e.to_string()),
        },
        Req::Projects => match client.call("projects", "listProjects", Value::Null) {
            Ok(v) => Resp::Projects(
                v.as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|p| p.get("name").and_then(Value::as_str).map(String::from))
                            .collect()
                    })
                    .unwrap_or_default(),
            ),
            Err(e) => Resp::Err(e.to_string()),
        },
        Req::Actions => match client.call("actions", "listActions", json!({ "limit": 50 })) {
            Ok(v) => Resp::Actions(v.as_array().cloned().unwrap_or_default()),
            Err(e) => Resp::Err(e.to_string()),
        },
        Req::ActionDetail(id) => match client.call("actions", "getAction", json!({ "id": id })) {
            Ok(v) => {
                let mut lines = vec![
                    format!(
                        "{} · {} · {}",
                        field(&v, "/type"),
                        field(&v, "/status"),
                        field(&v, "/createdAt")
                    ),
                    format!(
                        "{}/{}",
                        field(&v, "/projectName"),
                        field(&v, "/serviceName")
                    ),
                    String::new(),
                ];
                match v.get("log").and_then(Value::as_str) {
                    Some(log) if !log.trim().is_empty() => {
                        lines.extend(log.lines().map(String::from))
                    }
                    _ => lines.push("(no log for this action)".into()),
                }
                Resp::Viewer(format!("Action · {}", field(&v, "/description")), lines)
            }
            Err(e) => Resp::Err(e.to_string()),
        },
        Req::MonitorData => match client.call("metrics", "getAllServicesStats", json!({})) {
            Ok(v) => Resp::MonitorData(v.as_array().cloned().unwrap_or_default()),
            Err(e) => Resp::Err(e.to_string()),
        },
        Req::TaskStats => match client.call("monitorOld", "getDockerTaskStats", Value::Null) {
            Ok(v) => Resp::TaskStats(parse_task_stats(&v)),
            Err(e) => Resp::Err(e.to_string()),
        },
        Req::Storage => match client.call("monitorOld", "getStorageStats", Value::Null) {
            Ok(v) => Resp::Storage(v.as_array().cloned().unwrap_or_default()),
            Err(e) => Resp::Err(e.to_string()),
        },
        Req::Domains => match client.call("domains", "listDomains", json!({})) {
            Ok(v) => Resp::Domains(v.as_array().cloned().unwrap_or_default()),
            Err(e) => Resp::DomainsErr(e.to_string()),
        },
        Req::AllServices => match client.call("projects", "listProjectsAndServices", Value::Null) {
            Ok(v) => Resp::AllServices {
                projects: v
                    .get("projects")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().map(|p| field(p, "/name")).collect())
                    .unwrap_or_default(),
                services: v
                    .get("services")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default(),
            },
            Err(e) => Resp::AllServicesErr(e.to_string()),
        },
        Req::ServicesFor(project) => {
            match client.call(
                "projects",
                "inspectProject",
                json!({ "projectName": project }),
            ) {
                // Only web services: this list fills the DOMAIN form's
                // destination, and pointing a domain at a database is refused by
                // the server ("Wrong service type") — an option that can only
                // fail does not belong in a dropdown.
                Ok(v) => Resp::ServicesFor(
                    project,
                    parse_services(&v)
                        .into_iter()
                        .filter(|(_, t)| crate::lifecycle::has_domains(t))
                        .map(|(n, _)| n)
                        .collect(),
                ),
                Err(e) => Resp::Err(e.to_string()),
            }
        }
        Req::LogTail {
            project,
            service,
            since,
        } => {
            let mut input = json!({
                "projectName": project, "serviceName": service, "limit": 200
            });
            if let Some(ts) = &since {
                input["start"] = json!(crate::logs::after(ts));
            }
            match client.call("logs", "queryServiceLogs", input) {
                Ok(v) => Resp::LogTail {
                    lines: crate::logs::format(&v),
                    cursor: crate::logs::newest_ts(&v),
                },
                Err(e) => Resp::Err(e.to_string()),
            }
        }
        Req::Bulk {
            targets,
            action,
            force,
        } => bulk_action(client, emit, targets, &action, force),
        Req::BulkResource { targets, resources } => bulk_resource(client, targets, resources),
        Req::DiffServices { a, b } => diff_services(client, a, b),
        Req::DiffAcrossHosts {
            local,
            target,
            target_name,
        } => {
            let other = target;
            let (p, sv, ty) = local.clone();
            // The same project/service on the other host — the whole point is a
            // like-for-like "staging vs production".
            let here = fetch_inspect(client, &local);
            let there = fetch_inspect(&other, &(p.clone(), sv.clone(), ty));
            match (here, there) {
                (Ok(va), Ok(vb)) => {
                    let la = format!("{}/{} @ this host", p, sv);
                    let lb = format!("{p}/{sv} @ {target_name}");
                    let diffs = crate::services::diff(&va, &vb);
                    Resp::Viewer(
                        format!(" Diff across hosts: {p}/{sv} "),
                        crate::services::diff_lines(&diffs, &la, &lb),
                    )
                }
                // Name which side failed: "not found" is the likely case (the
                // service does not exist on the other host), and the operator
                // needs to know WHICH host that was.
                (Err(e), _) => Resp::Err(format!("this host: {e}")),
                (_, Err(e)) => Resp::Err(format!("{target_name}: {e}")),
            }
        }
        Req::DiffProjectAcross {
            project,
            target,
            target_name,
        } => {
            let other = target;
            let inspect = |c: &EasypanelClient| {
                c.call(
                    "projects",
                    "inspectProject",
                    json!({ "projectName": project }),
                )
            };
            match (inspect(client), inspect(&other)) {
                (Ok(va), Ok(vb)) => {
                    let services = |v: &Value| {
                        v.get("services")
                            .and_then(Value::as_array)
                            .cloned()
                            .unwrap_or_default()
                    };
                    let d = crate::services::project_diff(&services(&va), &services(&vb));
                    let (la, lb) = ("this host".to_string(), target_name.clone());
                    Resp::Viewer(
                        format!(" Diff project across hosts: {project} "),
                        crate::services::project_diff_lines(&d, &la, &lb),
                    )
                }
                (Err(e), _) => Resp::Err(format!("this host: {e}")),
                (_, Err(e)) => Resp::Err(format!("{target_name}: {e}")),
            }
        }
        Req::DeployForm { project, service } => {
            let ps = json!({ "projectName": project, "serviceName": service });
            match client.call("services/app", "inspectService", ps) {
                Ok(v) => Resp::DeployForm {
                    project,
                    service,
                    deploy: v.get("deploy").cloned().unwrap_or(Value::Null),
                },
                Err(e) => Resp::Err(e.to_string()),
            }
        }
        Req::DeploySave {
            project,
            service,
            deploy,
        } => {
            // NESTED under "deploy". A flat {replicas: 3} is answered 200 and
            // changes nothing — verified live — so a wrong shape here would look
            // exactly like success.
            let body = json!({
                "projectName": project, "serviceName": service, "deploy": deploy,
            });
            match client.call("services/app", "updateDeploy", body) {
                Ok(_) => Resp::Done(
                    format!("Deploy settings saved for {project}/{service} — deploy to apply"),
                    Refresh::Projects,
                ),
                Err(e) => Resp::Err(e.to_string()),
            }
        }
        Req::MountForm {
            project,
            service,
            index,
        } => {
            let ps = json!({ "projectName": project, "serviceName": service });
            match client.call("mounts", "listMounts", ps) {
                // Read at the moment of editing, not remembered from when the
                // list was drawn: someone else may have changed it since.
                Ok(v) => match v.as_array().and_then(|a| a.get(index)).cloned() {
                    Some(values) => Resp::MountForm {
                        project,
                        service,
                        index,
                        values,
                    },
                    None => Resp::Err(format!("Mount [{index}] is no longer there")),
                },
                Err(e) => Resp::Err(e.to_string()),
            }
        }
        Req::MountUpdate {
            project,
            service,
            index,
            values,
        } => {
            let body = json!({
                "projectName": project, "serviceName": service,
                "index": index, "values": values,
            });
            match client.call("mounts", "updateMount", body) {
                // Reload the list, the same way a delete does. A change you
                // cannot see is indistinguishable from one that did not happen —
                // the viewer sat there showing the old path while the server had
                // the new one.
                Ok(_) => match fetch_view(client, View::Mounts, &project, &service, "") {
                    Ok(data) => Resp::Fetched {
                        view: View::Mounts,
                        project,
                        service,
                        stype: None,
                        data,
                    },
                    Err(e) => Resp::Err(e.to_string()),
                },
                Err(e) => Resp::Err(e.to_string()),
            }
        }
        Req::StorageProviders => {
            match client.call("storageProviders/common", "list", Value::Null) {
                Ok(v) => Resp::StorageProviders(
                    v.as_array()
                        .map(|a| {
                            a.iter()
                                .map(|p| (field(p, "/id"), field(p, "/name"), field(p, "/type")))
                                .collect()
                        })
                        .unwrap_or_default(),
                ),
                // Not fatal: without a provider the backup action explains itself
                // rather than the whole TUI failing to start.
                Err(_) => Resp::StorageProviders(Vec::new()),
            }
        }
        Req::DatabasesIn {
            project,
            service,
            stype,
        } => {
            let ps = json!({ "projectName": project, "serviceName": service });
            let cur = match client.call(&format!("services/{stype}"), "inspectService", ps) {
                Ok(v) => v,
                Err(e) => return Resp::Err(e.to_string()),
            };
            let configured = field(&cur, "/databaseName");
            let cmd = crate::backup::list_databases_command(
                &stype,
                &field(&cur, "/user"),
                &field(&cur, "/rootPassword"),
            );
            // No listing for this engine, or the shell could not answer: fall
            // back to the ONE name the panel knows. Better a working backup of
            // the obvious database than a dead end.
            // The fallback is only a fallback when there is something to fall
            // back TO: `field()` yields "-" for a service with no databaseName,
            // and offering that produced a row the endpoint always rejects.
            let fallback = |c: &String| {
                if crate::backup::is_named(c) {
                    vec![c.clone()]
                } else {
                    Vec::new()
                }
            };
            let names = match cmd {
                Some(c) => match crate::container::run_once(client, &project, &service, &c) {
                    Ok(out) => {
                        let n = crate::backup::parse_databases(&out);
                        if n.is_empty() {
                            fallback(&configured)
                        } else {
                            n
                        }
                    }
                    Err(_) => fallback(&configured),
                },
                None => fallback(&configured),
            };
            Resp::DatabasesIn {
                project,
                service,
                names,
            }
        }
        Req::Dbms {
            project,
            service,
            stype,
            level,
            database,
            table,
            query,
        } => dbms_step(
            client, project, service, stype, level, database, table, query,
        ),
        Req::BackupNow {
            project,
            service,
            database,
            provider,
            path,
        } => backup_now(client, &project, &service, &database, &provider, &path),
        Req::BackupHistoryFrom {
            src,
            src_name,
            project,
            service,
        } => {
            let remote: Vec<String> = match src.call("storageProviders/common", "list", Value::Null)
            {
                Ok(v) => v
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter(|p| crate::backup::is_remote(&field(p, "/type")))
                            .map(|p| field(p, "/id"))
                            .collect()
                    })
                    .unwrap_or_default(),
                Err(e) => return Resp::Err(format!("{src_name}: {e}")),
            };
            match src.call("actions", "listActions", json!({ "limit": 200 })) {
                Ok(v) => {
                    let acts = v.as_array().cloned().unwrap_or_default();
                    let all = crate::backup::history_all(&acts);
                    let total = all.len();
                    let usable: Vec<_> = all
                        .into_iter()
                        .filter(|(_, f)| remote.contains(&f.storage_provider_id))
                        .collect();
                    Resp::BackupHistoryFrom {
                        src_name,
                        project,
                        service,
                        hidden: total - usable.len(),
                        rows: usable
                            .iter()
                            // The database (schema) name is what tells you WHAT a
                            // backup restores — a service can hold several, and the
                            // file path names only the service, not the schema. It
                            // was carried for the restore but never shown; now it is.
                            .map(|(origin, f)| {
                                format!("{:<21}{:<24}{:<20}{}", f.when, origin, f.database, f.path)
                            })
                            .collect(),
                        files: usable
                            .into_iter()
                            .map(|(_, f)| (f.database, f.storage_provider_id, f.path))
                            .collect(),
                    }
                }
                Err(e) => Resp::Err(format!("{src_name}: {e}")),
            }
        }
        Req::BackupHistory { project, service } => {
            match client.call("actions", "listActions", json!({ "limit": 200 })) {
                Ok(v) => {
                    let acts = v.as_array().cloned().unwrap_or_default();
                    let files = crate::backup::history(&acts, &project, &service);
                    Resp::BackupHistory {
                        project,
                        service,
                        rows: files.iter().map(|f| f.row()).collect(),
                        files: files
                            .into_iter()
                            .map(|f| (f.database, f.storage_provider_id, f.path))
                            .collect(),
                    }
                }
                Err(e) => Resp::Err(e.to_string()),
            }
        }
        Req::RestoreBackup {
            project,
            service,
            database,
            provider,
            path,
        } => {
            // Pre-flight: EasyPanel restores INTO an existing database, so on a
            // host where `database` was never created the restore dies with a
            // cryptic `[400] … docker exec … exit code 1`. Ask what the service
            // actually holds and say so plainly instead. A failure of OUR check
            // (network, unexpected shape) must not block a restore that might
            // work — only a definitive "not in the list" does.
            let names = client.call(
                "databaseBackups",
                "getServiceDatabases",
                json!({ "projectName": project, "serviceName": service }),
            );
            if let Ok(v) = &names {
                if crate::backup::service_lists_database(v, &database) == Some(false) {
                    return Resp::Err(crate::backup::missing_database_message(&service, &database));
                }
            }
            let body = match crate::backup::restore_body(
                &project, &service, &database, &provider, &path,
            ) {
                Ok(body) => body,
                Err(message) => return Resp::Err(message),
            };
            match client.call("databaseBackups", "restoreDatabaseBackup", body) {
                // The restore recycles the container, so the service comes back a
                // few seconds later — refreshing keeps the table honest about it.
                Ok(_) => Resp::Done(
                    format!("Restored {database} into {project}/{service} from {path}"),
                    Refresh::Projects,
                ),
                Err(e) => Resp::Err(format!("Restore failed: {e}")),
            }
        }
        Req::DumpR2 {
            project,
            service,
            databases,
        } => match crate::operations::dump_to_r2(
            client,
            &project,
            &service,
            &databases,
            false,
            None,
            |p| {
                // The phase word comes from `dump_phase` ("dumping — 319.7 MB
                // written"), so the prefix must not say it again.
                emit(Resp::Progress(format!("{project}/{service}: {p}")));
            },
        ) {
            Ok(d) => Resp::Done(
                format!(
                    "Dumped {} database(s) → {}/{} ({}), non-locking",
                    d.databases.len(),
                    d.bucket,
                    d.key,
                    d.provider
                ),
                Refresh::None,
            ),
            Err(e) => Resp::Err(e.to_string()),
        },
        Req::R2Dumps {
            project,
            service,
            all_services,
        } => {
            // Two scopes, one destination. The wide listing is what makes a dump
            // of ANOTHER service reachable from here at all; the narrow one stays
            // this service's own truthful history.
            let keys = if all_services {
                crate::operations::list_all_r2_dumps(client, None)
                    .map(|d| d.into_iter().map(|d| d.key).collect())
            } else {
                crate::operations::list_r2_dumps(client, &project, &service, None)
            };
            match keys {
                Ok(keys) => Resp::R2Dumps {
                    project,
                    service,
                    all_services,
                    keys,
                },
                Err(e) => Resp::Err(e.to_string()),
            }
        }
        Req::DownloadR2 {
            project,
            service,
            path,
        } => match crate::operations::download_r2_dump(
            client,
            &project,
            &service,
            Some(&path),
            None,
            None,
            |b, t| {
                emit(Resp::Progress(format!(
                    "Downloading {path} — {}",
                    crate::operations::download_progress(b, t)
                )));
            },
        ) {
            Ok((_, dest, bytes)) => Resp::Done(
                format!(
                    "Downloaded {path} → {} ({:.1} MB)",
                    dest.display(),
                    bytes as f64 / 1_048_576.0
                ),
                Refresh::None,
            ),
            Err(e) => Resp::Err(e.to_string()),
        },
        Req::RestoreR2 {
            project,
            service,
            path,
        } => {
            match crate::operations::restore_from_r2(client, &project, &service, &path, None, |p| {
                // The phase word comes from `restore_phase` ("importing — 2.9 GB
                // to read"), so the prefix must not say it again.
                emit(Resp::Progress(format!("{project}/{service}: {p}")));
            }) {
                // Refresh::None, like DumpR2: a restore imports rows INTO a database, it
                // doesn't change the service table (names, status, metrics), so reloading
                // every service on completion is a wasted round-trip — needless churn on a
                // big host.
                Ok(()) => Resp::Done(
                    format!("Restored {path} into {project}/{service}"),
                    Refresh::None,
                ),
                Err(e) => Resp::Err(e.to_string()),
            }
        }
        Req::CopyDb {
            target,
            target_name,
            target_project,
            target_service,
            project,
            service,
            stage,
        } => {
            let dst_client = target;
            let src = crate::operations::CopyTarget {
                client,
                project: &project,
                service: &service,
            };
            let dst = crate::operations::CopyTarget {
                client: &dst_client,
                project: &target_project,
                service: &target_service,
            };
            let confirmed = match stage {
                CopyDbStage::Preview { databases, all } => {
                    let plan = match crate::operations::plan_copy(&src, &dst, &databases, all, None)
                    {
                        Ok(plan) => plan,
                        Err(error) => return Resp::Err(format!("{error:#}")),
                    };
                    return Resp::CopyDbPlan {
                        plan: Box::new(plan),
                        target_name,
                        target_project,
                        target_service,
                        project,
                        service,
                    };
                }
                CopyDbStage::Confirmed(plan) => plan,
            };
            // Same closure convention as DumpR2: the phase words the two legs emit
            // are already distinct ("uploading" vs "downloading"), so nothing is
            // prefixed onto them.
            match crate::operations::copy_database(&src, &dst, &confirmed, None, |p| {
                emit(Resp::Progress(format!("{project}/{service}: {p}")));
            }) {
                // Refresh::None, like RestoreR2: rows land INSIDE a database and
                // change nothing the service table shows.
                Ok(d) => Resp::Done(
                    format!(
                        "Copied {} database(s) into {target_name}/{target_project}/{target_service}: {} · dump kept at {}/{}",
                        d.databases.len(),
                        d.databases.join(", "),
                        d.bucket,
                        d.key
                    ),
                    Refresh::None,
                ),
                Err(e) => Resp::Err(format!("{e:#}")),
            }
        }
        Req::LogSearch { query } => log_search(client, &query),
        Req::Repos => Resp::Repos(github_repos(client)),
        Req::ConfigForm {
            project,
            service,
            build,
        } => {
            let ps = json!({ "projectName": project, "serviceName": service });
            match client.call("services/app", "inspectService", ps) {
                // Repos are only needed for the source form. A searchRepos failure
                // doesn't fail the form: the "Repo" field falls back to a plain
                // text input.
                Ok(data) => {
                    let repos = if build {
                        Vec::new()
                    } else {
                        github_repos(client)
                    };
                    Resp::ConfigForm {
                        project,
                        service,
                        build,
                        data,
                        repos,
                    }
                }
                Err(e) => Resp::Err(e.to_string()),
            }
        }
        Req::ResourceForm {
            project,
            service,
            stype,
        } => {
            let ps = json!({ "projectName": project, "serviceName": service });
            match client.call(&format!("services/{stype}"), "inspectService", ps) {
                Ok(data) => Resp::ResourceForm {
                    project,
                    service,
                    stype,
                    data,
                },
                Err(e) => Resp::Err(e.to_string()),
            }
        }
        Req::ResourceSave {
            project,
            service,
            stype,
            resources,
        } => {
            let mut input = resources;
            input["projectName"] = json!(project);
            input["serviceName"] = json!(service);
            match client.call(&format!("services/{stype}"), "updateResources", input) {
                // Refresh::None: limits don't show in the table; just store the
                // config, deploy applies it (same as ports).
                Ok(_) => Resp::Done(
                    format!("Resource {project}/{service} saved — deploy to apply"),
                    Refresh::None,
                ),
                Err(e) => Resp::Err(e.to_string()),
            }
        }
        Req::BasicAuthForm {
            project,
            service,
            stype,
        } => {
            let ps = json!({ "projectName": project, "serviceName": service });
            match client.call(&format!("services/{stype}"), "inspectService", ps) {
                Ok(data) => Resp::BasicAuthForm {
                    project,
                    service,
                    stype,
                    data,
                },
                Err(e) => Resp::Err(e.to_string()),
            }
        }
        Req::BasicAuthSave {
            project,
            service,
            stype,
            basic_auth,
        } => match client.call(
            &format!("services/{stype}"),
            "updateBasicAuth",
            json!({ "projectName": project, "serviceName": service, "basicAuth": basic_auth }),
        ) {
            Ok(_) => Resp::Done(
                format!("Basic auth {project}/{service} saved — deploy to apply"),
                Refresh::None,
            ),
            Err(e) => Resp::Err(e.to_string()),
        },
        Req::MaintInfo => {
            // Each row stands on its own: one endpoint failing must not empty the
            // whole tab.
            let one = |op: &str| match client.call("settings", op, Value::Null) {
                Ok(v) => Ok(field(&v, "")),
                Err(e) => Err(e.to_string()),
            };
            Resp::MaintInfo(vec![
                ("Docker".into(), one("getDockerVersion")),
                ("Server IP".into(), one("getServerIp")),
                ("Update available".into(), one("checkForUpdates")),
                ("Daily cleanup".into(), one("getDailyDockerCleanup")),
            ])
        }
        Req::MaintAction(op) => match client.call("settings", op, Value::Null) {
            Ok(_) => Resp::Done(format!("{op} done"), Refresh::None),
            Err(e) => Resp::Err(e.to_string()),
        },
        Req::Branches { owner, repo } => {
            match client.call(
                "github",
                "searchBranches",
                json!({ "owner": owner, "repo": repo, "search": "" }),
            ) {
                // searchBranches returns a flat array of strings (not {items:[...]}).
                Ok(v) => Resp::Branches(Ok(v
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .map(String::from)
                            .collect()
                    })
                    .unwrap_or_default())),
                Err(e) => Resp::Branches(Err(e.to_string())),
            }
        }
        Req::ConfigSave {
            project,
            service,
            op,
            body,
            auto_deploy,
        } => {
            let ps = json!({ "projectName": project, "serviceName": service });
            let mut input = body;
            input["projectName"] = json!(project);
            input["serviceName"] = json!(service);
            match client.call("services/app", op, input) {
                Ok(_) => match auto_deploy {
                    Some(on) => {
                        let ep = if on {
                            "enableGithubDeploy"
                        } else {
                            "disableGithubDeploy"
                        };
                        match client.call("services/app", ep, ps) {
                            // Refresh::Projects, not None: without it the Source
                            // column in the table keeps showing the old
                            // branch/source until the user presses `r`. Exactly the
                            // same class of bug as a deleted service that doesn't
                            // leave the table.
                            Ok(_) => Resp::Done("Saved".into(), Refresh::Projects),
                            Err(e) => {
                                Resp::Err(format!("Source saved, but auto deploy failed: {e}"))
                            }
                        }
                    }
                    None => Resp::Done("Saved".into(), Refresh::Projects),
                },
                Err(e) => Resp::Err(e.to_string()),
            }
        }
        Req::AutoDeploy {
            project,
            service,
            on,
        } => {
            let ep = if on {
                "enableGithubDeploy"
            } else {
                "disableGithubDeploy"
            };
            let ps = json!({ "projectName": project, "serviceName": service });
            match client.call("services/app", ep, ps) {
                Ok(_) => Resp::Done(
                    format!(
                        "Auto deploy {} for {service}",
                        if on { "on" } else { "off" }
                    ),
                    Refresh::Projects,
                ),
                Err(e) => Resp::Err(auto_deploy_error(&service, &e.to_string())),
            }
        }
        Req::Fetch {
            view,
            project,
            service,
            stype,
        } => match fetch_view(client, view, &project, &service, &stype) {
            Ok(data) => Resp::Fetched {
                view,
                project,
                service,
                stype: Some(stype),
                data,
            },
            Err(e) => Resp::Err(e.to_string()),
        },
        Req::ProjectCreate(name) => {
            match client.call("projects", "createProject", json!({ "name": name })) {
                Ok(_) => Resp::Done(format!("Project '{name}' created"), Refresh::Projects),
                Err(e) => Resp::Err(e.to_string()),
            }
        }
        Req::ProjectDestroy(name) => {
            match client.call("projects", "destroyProject", json!({ "name": name })) {
                Ok(_) => Resp::Done(format!("Project '{name}' deleted"), Refresh::Projects),
                Err(e) => Resp::Err(e.to_string()),
            }
        }
        Req::ServiceCreate {
            project,
            service,
            stype,
            extra,
            source,
        } => {
            let grp = format!("services/{stype}");
            let mut input = extra;
            input["projectName"] = json!(project);
            input["serviceName"] = json!(service);
            // Inline source triggers a deploy. Creation and configuration remain separate.
            match client.call(&grp, "createService", input) {
                Ok(_) => match source {
                    Some(source) => service_setup(
                        client,
                        project,
                        service,
                        stype,
                        source,
                        ServiceSetupPhase::Source,
                    ),
                    None => Resp::ServiceCreated {
                        project,
                        service,
                        stype,
                        notes: Vec::new(),
                        pending: None,
                    },
                },
                Err(error) => Resp::Err(error.to_string()),
            }
        }
        Req::ServiceSetup {
            project,
            service,
            stype,
            source,
            phase,
        } => service_setup(client, project, service, stype, source, phase),
        Req::CloneService {
            project,
            service,
            stype,
            target,
            new_name,
        } => clone_service(client, &project, &service, &stype, &target, &new_name),
        Req::Migrate {
            target,
            target_name,
            target_project,
            services,
        } => {
            let dst_client = target;
            // The destination project usually doesn't exist yet — that IS the
            // normal case when moving to a fresh host.
            if let Err(e) = crate::migrate::ensure_project(&dst_client, &target_project) {
                return Resp::Err(format!("Migration stopped: {e:#}"));
            }
            let total = services.len();
            let (mut ok, mut notes, mut failed) = (0usize, Vec::new(), Vec::new());
            for (project, service, stype) in services {
                let dst = crate::migrate::Target {
                    client: &dst_client,
                    project: &target_project,
                    service: &service,
                };
                // One service failing must not abandon the rest: a partial
                // migration the user can see is more useful than an opaque stop.
                match crate::migrate::migrate_service(
                    client, &project, &service, &stype, &dst, true,
                ) {
                    Ok(mut n) => {
                        ok += 1;
                        notes.append(&mut n);
                    }
                    Err(e) => failed.push(format!("{service}: {e:#}")),
                }
            }
            let mut msg = format!(
                "Migrated {ok}/{total} to {target_name}/{target_project} — config only, NO data"
            );
            if !failed.is_empty() {
                msg.push_str(&format!(" · failed: {}", failed.join("; ")));
            }
            if ok == 0 && total > 0 {
                Resp::Err(msg)
            } else if !notes.is_empty() {
                // Same reason as the clone path: a note the terminal width cuts in
                // half is a note that was never delivered.
                Resp::Notes {
                    msg,
                    notes,
                    refresh: Refresh::Projects,
                }
            } else {
                // The services landed on ANOTHER host, so this host's view is
                // unchanged — refresh only so a same-host migration shows up.
                Resp::Done(msg, Refresh::Projects)
            }
        }
        Req::RunChecks(checks) => Resp::Checked(crate::uptime::send_all(&checks)),
        Req::ProjectEnvSave { project, env } => {
            match client.call(
                "projects",
                "updateProjectEnv",
                json!({ "projectName": project, "env": env }),
            ) {
                Ok(_) => Resp::ProjectEnvSaved(project),
                Err(e) => Resp::Err(e.to_string()),
            }
        }
        Req::DomainBulkEdit { changes } => {
            // Sequential, not fanned out: these are edits to a routing table, and
            // a failure has to name the domain it belongs to. One at a time also
            // keeps a fleet-wide rename from arriving as a burst the panel has to
            // reconcile all at once.
            let (mut ok, mut failed) = (0usize, Vec::new());
            for (id, before, mut input) in changes {
                input["id"] = json!(id);
                match client.call("domains", "updateDomain", input) {
                    Ok(_) => ok += 1,
                    Err(e) => failed.push((before, e.to_string())),
                }
            }
            Resp::DomainsEdited { ok, failed }
        }
        Req::DomainBulkDelete { domains } => {
            let (mut ok, mut failed) = (0usize, Vec::new());
            for (id, host) in domains {
                match client.call("domains", "deleteDomain", json!({ "id": id })) {
                    Ok(_) => ok += 1,
                    Err(e) => failed.push((host, e.to_string())),
                }
            }
            Resp::DomainsDeleted { ok, failed }
        }
        Req::DomainSave { id, body } => {
            // createDomain requires `id` but the server ignores it and makes its own
            // cuid, so a placeholder is enough for a new domain.
            let op = if id.is_some() {
                "updateDomain"
            } else {
                "createDomain"
            };
            let mut input = body;
            input["id"] = json!(id.clone().unwrap_or_else(|| "new".to_string()));
            match client.call("domains", op, input) {
                Ok(_) => Resp::Done(
                    if id.is_some() {
                        "Domain updated".into()
                    } else {
                        "Domain created".into()
                    },
                    Refresh::Domains,
                ),
                Err(e) => Resp::Err(e.to_string()),
            }
        }
        Req::PortSave {
            project,
            service,
            values,
        } => {
            // Ports don't show in the Services table, so no refresh needed; the user
            // reopens it with `p` to check.
            match client.call(
                "ports",
                "createPort",
                json!({ "projectName": project, "serviceName": service, "values": values }),
            ) {
                Ok(_) => Resp::Done(format!("Port added to {project}/{service}"), Refresh::None),
                Err(e) => Resp::Err(e.to_string()),
            }
        }
        Req::PortDelete {
            project,
            service,
            index,
        } => {
            match client.call(
                "ports",
                "deletePort",
                json!({ "projectName": project, "serviceName": service, "index": index }),
            ) {
                // Reload the port list into the viewer: the deleted one must
                // disappear immediately, not wait for the user to reopen it (the
                // "deleted row still showing" class of bug that has recurred in this
                // project).
                Ok(_) => match fetch_view(client, View::Ports, &project, &service, "") {
                    Ok(data) => Resp::Fetched {
                        view: View::Ports,
                        project,
                        service,
                        stype: None,
                        data,
                    },
                    Err(e) => Resp::Err(e.to_string()),
                },
                Err(e) => Resp::Err(e.to_string()),
            }
        }
        Req::MountSave {
            project,
            service,
            values,
        } => match client.call(
            "mounts",
            "createMount",
            json!({ "projectName": project, "serviceName": service, "values": values }),
        ) {
            Ok(_) => Resp::Done(
                format!("Mount added to {project}/{service} — deploy when ready"),
                Refresh::None,
            ),
            Err(e) => Resp::Err(e.to_string()),
        },
        Req::MountDelete {
            project,
            service,
            index,
        } => match client.call(
            "mounts",
            "deleteMount",
            json!({ "projectName": project, "serviceName": service, "index": index }),
        ) {
            // Reload the mount list into the viewer (same pattern as port delete).
            Ok(_) => match fetch_view(client, View::Mounts, &project, &service, "") {
                Ok(data) => Resp::Fetched {
                    view: View::Mounts,
                    project,
                    service,
                    stype: None,
                    data,
                },
                Err(e) => Resp::Err(e.to_string()),
            },
            Err(e) => Resp::Err(e.to_string()),
        },
        Req::RedirectAdd {
            project,
            service,
            stype,
            redirect,
        } => match save_redirects(client, &stype, &project, &service, |mut list| {
            list.push(redirect.clone());
            list
        }) {
            Ok(_) => Resp::Done(
                format!("Redirect added to {project}/{service} — deploy to apply"),
                Refresh::None,
            ),
            Err(e) => Resp::Err(e.to_string()),
        },
        Req::RedirectDelete {
            project,
            service,
            stype,
            index,
        } => {
            match save_redirects(client, &stype, &project, &service, |mut list| {
                if index < list.len() {
                    list.remove(index);
                }
                list
            }) {
                // Reload the viewer (same pattern as port/mount delete).
                Ok(_) => match fetch_view(client, View::Redirects, &project, &service, &stype) {
                    Ok(data) => Resp::Fetched {
                        view: View::Redirects,
                        project,
                        service,
                        stype: Some(stype),
                        data,
                    },
                    Err(e) => Resp::Err(e.to_string()),
                },
                Err(e) => Resp::Err(e.to_string()),
            }
        }
        Req::DomainDelete(id) => {
            match client.call("domains", "deleteDomain", json!({ "id": id })) {
                Ok(_) => Resp::Done("Domain deleted".into(), Refresh::Domains),
                Err(e) => Resp::Err(e.to_string()),
            }
        }
        Req::DomainSetPrimary(id) => {
            match client.call("domains", "setPrimaryDomain", json!({ "id": id })) {
                Ok(_) => Resp::Done("Domain set as primary".into(), Refresh::Domains),
                Err(e) => Resp::Err(e.to_string()),
            }
        }
        Req::EnvSave {
            project,
            service,
            stype,
            env,
        } => match crate::operations::save_env(client, &project, &service, &stype, &env) {
            Ok(()) => Resp::Done(format!("Env {project}/{service} saved"), Refresh::None),
            Err(e) => Resp::Err(e.to_string()),
        },
        Req::EnvFileToggle {
            project,
            service,
            stype,
        } => {
            let grp = format!("services/{stype}");
            let ps = json!({ "projectName": project, "serviceName": service });
            match client.call(&grp, "inspectService", ps) {
                Ok(cur) => {
                    let env = cur.get("env").and_then(Value::as_str).unwrap_or("");
                    let active = cur.get("dotEnvPath").and_then(Value::as_str).is_some();
                    // Flip: on → turn off (dot None), off → turn on at ".env".
                    let dot = if active { None } else { Some(".env") };
                    match client.call(&grp, "updateEnv", env_body(&project, &service, env, dot)) {
                        Ok(_) => {
                            let msg = if active {
                                format!(".env file turned off for {project}/{service}")
                            } else {
                                format!(".env file on (.env) for {project}/{service}")
                            };
                            Resp::Done(msg, Refresh::None)
                        }
                        Err(e) => Resp::Err(e.to_string()),
                    }
                }
                Err(e) => Resp::Err(e.to_string()),
            }
        }
        Req::ConfigFileSave {
            project,
            service,
            stype,
            config,
        } => {
            let grp = format!("services/{stype}");
            let ps = json!({ "projectName": project, "serviceName": service });
            // updateAdvanced replaces the whole Advanced block; image & command MUST
            // be strings (verified: null/omit is rejected). Read what's there, keep
            // it, replace only configFile. env is included if present so it isn't lost.
            match client.call(&grp, "inspectService", ps) {
                Ok(cur) => {
                    let mut body = json!({
                        "projectName": project,
                        "serviceName": service,
                        "image": cur.get("image").and_then(Value::as_str).unwrap_or(""),
                        "command": cur.get("command").and_then(Value::as_str).unwrap_or(""),
                        "configFile": config,
                    });
                    if let Some(env) = cur.get("env").and_then(Value::as_str) {
                        body["env"] = json!(env);
                    }
                    match client.call(&grp, "updateAdvanced", body) {
                        Ok(_) => Resp::Done(
                            format!("Config file {project}/{service} saved"),
                            Refresh::None,
                        ),
                        Err(e) => Resp::Err(e.to_string()),
                    }
                }
                Err(e) => Resp::Err(e.to_string()),
            }
        }
        Req::Action {
            project,
            service,
            stype,
            action,
            force,
        } => {
            // Refused BEFORE the deploy branch below: a type with no build has no
            // deployService route, so dispatching one only produced a "started"
            // message followed seconds later by a 404.
            if crate::lifecycle::ops(&stype, &action).is_none() {
                return Resp::Err(crate::lifecycle::unavailable(&stype, &action));
            }
            // Deploy is DISPATCHED, not awaited. Its build takes an unpredictable
            // time — possibly minutes, depending on the repo — and exceeds any
            // proxy limit (measured: 125 seconds then a 524 from Cloudflare).
            // Awaiting it = "error sending request" even though the deploy keeps
            // running. So fire it on a separate thread and report started right
            // away; the server finishes the build on its own (dropping the
            // connection doesn't cancel it — proven with createService).
            if action == "deploy" {
                let c = client.clone();
                let (grp, input) = (
                    format!("services/{stype}"),
                    json!({ "projectName": project, "serviceName": service, "forceRebuild": force }),
                );
                // An immediate rejection (bad config, 400, service can't deploy) used
                // to be swallowed by `let _ =` — the UI said "started" while the
                // server rejected it instantly. Now the thread reports the failure
                // via the shared sink so either frontend sees it. A build that actually
                // runs is still not awaited (can be minutes, exceeds proxy limits).
                let tx = Arc::clone(emit);
                let (p, s) = (project.clone(), service.clone());
                std::thread::spawn(move || {
                    if let Err(e) = c.call(&grp, "deployService", input) {
                        // Only a REFUSAL is a failure. The build outliving our
                        // connection is the normal case for anything real, and
                        // reporting that as "deploy failed" contradicted the table
                        // — which showed the very same service as `deploying` —
                        // and invited the user to deploy a second time.
                        //
                        // Nothing is said in that case: the status line already
                        // reported the deploy as started, the Status column tracks
                        // it live, and the Actions tab carries the outcome.
                        if !crate::client::gave_up_waiting(&e) {
                            tx(Resp::Err(format!("Deploy {p}/{s} failed: {e}")));
                        }
                    }
                });
                return Resp::Done(
                    format!("Deploy {project}/{service} started — watch it in Logs"),
                    Refresh::None,
                );
            }
            // Which endpoints this action actually IS for this service type. A
            // database has no restartService route — it cycles `enabled` — and
            // sending the old guess simply 404'd, which is why the Lifecycle menu
            // never worked on a single database in the panel.
            let Some(ops) = crate::lifecycle::ops(&stype, &action) else {
                return Resp::Err(crate::lifecycle::unavailable(&stype, &action));
            };
            let mut last = Ok(Value::Null);
            for op in ops {
                let input = json!({ "projectName": project, "serviceName": service });
                last = client.call(&format!("services/{stype}"), op, input);
                if last.is_err() {
                    break;
                }
            }
            match last {
                // Refresh, not just a message: destroy/start/stop are already done on
                // the server when this call returns (destroyService measured
                // 0.2-5 seconds), but the table was never reloaded — a deleted
                // service stayed on screen until the user pressed `r`. Exactly the
                // "new service doesn't show up right away" class of bug that was
                // fixed for create and missed for this one.
                Ok(_) => Resp::Done(
                    format!("{action} triggered for {project}/{service}"),
                    Refresh::Projects,
                ),
                Err(e) => Resp::Err(e.to_string()),
            }
        }
        Req::Cf(cf) => handle_cf(cf, &CloudflareClient::new),
    }
}

/// Dispatch a Cloudflare request. Builds a fresh client from the token in the
/// request (never from EasyPanel state), so the worker stays stateless about CF
/// config. The token is used only to authenticate the call and is never logged.
pub(crate) fn handle_cf(req: CfReq, make_client: &dyn Fn(&str) -> CloudflareClient) -> Resp {
    Resp::Cf(cf_response(req, make_client))
}

fn cf_response(req: CfReq, make_client: &dyn Fn(&str) -> CloudflareClient) -> CfResp {
    match req {
        CfReq::Analytics {
            token,
            account_id,
            days,
        } => match make_client(&token).account_analytics(&account_id, days) {
            Ok(summary) => CfResp::Analytics(summary),
            Err(e) => CfResp::Err(e.to_string()),
        },
        CfReq::Zones { token, account_id } => {
            match make_client(&token).list_zones(account_id.as_deref()) {
                Ok(zones) => CfResp::Zones(zones),
                Err(e) => CfResp::Err(e.to_string()),
            }
        }
        CfReq::Records {
            token,
            zone_id,
            filter,
        } => match make_client(&token).list_records(&zone_id, &filter) {
            Ok(records) => CfResp::Records { zone_id, records },
            Err(e) => CfResp::Err(e.to_string()),
        },
        CfReq::WebAnalyticsSites { token, account_id } => {
            match make_client(&token).list_web_analytics_sites(&account_id) {
                Ok(sites) => CfResp::WebAnalyticsSites(sites),
                Err(e) => CfResp::WebAnalyticsErr(e.to_string()),
            }
        }
        CfReq::CreateZone {
            token,
            name,
            account_id,
        } => match make_client(&token).create_zone(&name, &account_id) {
            Ok(z) => CfResp::Done(format!("Zone '{}' created", z.name)),
            Err(e) => CfResp::Err(e.to_string()),
        },
        CfReq::DeleteZone { token, zone_id } => {
            match make_client(&token).delete_zone(&zone_id) {
                Ok(()) => CfResp::Done("Zone deleted".into()),
                Err(e) => CfResp::Err(e.to_string()),
            }
        }
        CfReq::CreateRecord {
            token,
            zone_id,
            body,
        } => match make_client(&token).create_record(&zone_id, &body) {
            Ok(r) => CfResp::Done(format!("Record '{}' created", r.name)),
            Err(e) => CfResp::Err(e.to_string()),
        },
        CfReq::PatchRecord {
            token,
            zone_id,
            id,
            body,
        } => match make_client(&token).patch_record(&zone_id, &id, &body) {
            Ok(r) => CfResp::Done(format!("Record '{}' updated", r.name)),
            Err(e) => CfResp::Err(e.to_string()),
        },
        CfReq::DeleteRecord { token, zone_id, id } => {
            match make_client(&token).delete_record(&zone_id, &id) {
                Ok(()) => CfResp::Done("Record deleted".into()),
                Err(e) => CfResp::Err(e.to_string()),
            }
        }
        CfReq::BulkPatch {
            token,
            zone_id,
            ids,
            body,
        } => {
            let client = make_client(&token);
            let (mut ok, mut failed) = (0usize, Vec::new());
            for chunk in ids.chunks(DNS_BATCH_LIMIT) {
                match client.batch_patch_records(&zone_id, chunk, &body) {
                    Ok(()) => ok += chunk.len(),
                    Err(e) => {
                        let msg = e.to_string();
                        failed.extend(chunk.iter().cloned().map(|id| (id, msg.clone())));
                    }
                }
            }
            CfResp::BulkDone { ok, failed }
        }
        CfReq::BulkDelete {
            token,
            zone_id,
            ids,
        } => {
            let client = make_client(&token);
            let (mut ok, mut failed) = (0usize, Vec::new());
            for chunk in ids.chunks(DNS_BATCH_LIMIT) {
                match client.batch_delete_records(&zone_id, chunk) {
                    Ok(()) => ok += chunk.len(),
                    Err(e) => {
                        let msg = e.to_string();
                        failed.extend(chunk.iter().cloned().map(|id| (id, msg.clone())));
                    }
                }
            }
            CfResp::BulkDone { ok, failed }
        }
        CfReq::R2Buckets { token, account_id } => {
            match make_client(&token).list_r2_buckets(&account_id) {
                Ok(buckets) => CfResp::R2Buckets(buckets),
                Err(e) => CfResp::Err(e.to_string()),
            }
        }
        CfReq::Tunnels { token, account_id } => {
            match make_client(&token).list_tunnels(&account_id) {
                Ok(tunnels) => CfResp::Tunnels(tunnels),
                Err(e) => CfResp::Err(e.to_string()),
            }
        }
        CfReq::TunnelCreate {
            token,
            account_id,
            name,
        } => match make_client(&token).create_tunnel(&account_id, &name) {
            Ok(tunnel) => CfResp::Done(format!("Tunnel '{}' created", tunnel.name)),
            Err(e) => CfResp::Err(e.to_string()),
        },
        CfReq::TunnelDelete {
            token,
            account_id,
            tunnel_id,
            name,
        } => match make_client(&token).delete_tunnel(&account_id, &tunnel_id) {
            Ok(()) => CfResp::Done(format!("Tunnel '{name}' deleted")),
            Err(e) => CfResp::Err(e.to_string()),
        },
        CfReq::TunnelToken {
            token,
            account_id,
            tunnel_id,
            name,
        } => match make_client(&token).get_tunnel_token(&account_id, &tunnel_id) {
            Ok(tunnel_token) => CfResp::Viewer {
                title: format!("Tunnel install · {name}"),
                lines: vec![
                    "Install cloudflared on the origin machine, then run one of:".into(),
                    String::new(),
                    "Linux service:".into(),
                    format!("sudo cloudflared service install {tunnel_token}"),
                    String::new(),
                    "Docker:".into(),
                    format!(
                        "docker run -d --name cloudflared --restart unless-stopped cloudflare/cloudflared:latest tunnel --no-autoupdate run --token {tunnel_token}"
                    ),
                    String::new(),
                    "Token:".into(),
                    tunnel_token,
                ],
            },
            Err(e) => CfResp::Err(e.to_string()),
        },
        CfReq::TunnelConfig {
            token,
            account_id,
            tunnel_id,
        } => match make_client(&token).get_tunnel_config(&account_id, &tunnel_id) {
            Ok(config) => CfResp::TunnelConfig {
                tunnel_id,
                config: Box::new(config),
            },
            Err(e) => CfResp::Err(e.to_string()),
        },
        CfReq::TunnelRouteAdd {
            token,
            account_id,
            tunnel_id,
            hostname,
            service,
            path,
            origin_request,
            dns,
            zone,
            proxied,
        } => {
            let rule = TunnelIngressRule::route(&hostname, &service, &path, origin_request);
            let client = make_client(&token);
            match client.add_tunnel_route(&account_id, &tunnel_id, rule) {
                Ok(_) => {
                    let mut msg = format!("Tunnel route '{}' added", hostname);
                    if dns {
                        match upsert_tunnel_dns(
                            &client,
                            &account_id,
                            &tunnel_id,
                            &hostname,
                            zone.as_deref(),
                            proxied,
                        ) {
                            Ok(record) => {
                                msg.push_str(&format!(
                                    "; DNS CNAME '{}' -> {} updated",
                                    record.name,
                                    tunnel_target(&tunnel_id)
                                ));
                            }
                            Err(e) => return CfResp::Err(e.to_string()),
                        }
                    }
                    CfResp::Done(msg)
                }
                Err(e) => CfResp::Err(e.to_string()),
            }
        }
        CfReq::TunnelRouteEdit {
            token,
            account_id,
            tunnel_id,
            hostname,
            path,
            service,
            origin_request,
            dns,
            zone,
            proxied,
        } => {
            let client = make_client(&token);
            match client.edit_tunnel_route(
                &account_id,
                &tunnel_id,
                TunnelRouteChange {
                    hostname: hostname.clone(),
                    service: (!service.trim().is_empty()).then_some(service),
                    path: (!path.trim().is_empty()).then_some(path),
                    origin_request,
                },
            ) {
                Ok(_) => {
                    let mut msg = format!("Tunnel route '{}' updated", hostname);
                    if dns {
                        match upsert_tunnel_dns(
                            &client,
                            &account_id,
                            &tunnel_id,
                            &hostname,
                            zone.as_deref(),
                            proxied,
                        ) {
                            Ok(record) => {
                                msg.push_str(&format!(
                                    "; DNS CNAME '{}' -> {} updated",
                                    record.name,
                                    tunnel_target(&tunnel_id)
                                ));
                            }
                            Err(e) => return CfResp::Err(e.to_string()),
                        }
                    }
                    CfResp::Done(msg)
                }
                Err(e) => CfResp::Err(e.to_string()),
            }
        }
        CfReq::TunnelRouteDelete {
            token,
            account_id,
            tunnel_id,
            hostname,
            path,
            delete_dns,
            zone,
        } => {
            let client = make_client(&token);
            match client.delete_tunnel_route(
                &account_id,
                &tunnel_id,
                &hostname,
                (!path.trim().is_empty()).then_some(path.as_str()),
            ) {
                Ok(_) => {
                    let mut msg = format!("Tunnel route '{}' deleted", hostname);
                    if delete_dns {
                        match delete_tunnel_dns(
                            &client,
                            &account_id,
                            &tunnel_id,
                            &hostname,
                            zone.as_deref(),
                        ) {
                            Ok(deleted) => {
                                msg.push_str(&format!(
                                    "; deleted {deleted} matching DNS CNAME record(s)"
                                ));
                            }
                            Err(e) => return CfResp::Err(e.to_string()),
                        }
                    }
                    CfResp::Done(msg)
                }
                Err(e) => CfResp::Err(e.to_string()),
            }
        }
        CfReq::Workers { token, account_id } => {
            match make_client(&token).list_worker_scripts(&account_id) {
                Ok(workers) => CfResp::Workers(workers),
                Err(e) => CfResp::Err(e.to_string()),
            }
        }
        CfReq::WorkerDeployments {
            token,
            account_id,
            name,
        } => match make_client(&token).list_worker_deployments(&account_id, &name) {
            Ok(deployments) => CfResp::WorkerDeployments {
                worker: name,
                deployments,
            },
            Err(e) => CfResp::Err(e.to_string()),
        },
        CfReq::WorkerSettings {
            token,
            account_id,
            name,
        } => match make_client(&token).get_worker_settings_bundle(&account_id, &name) {
            Ok(settings) => CfResp::WorkerSettings {
                worker: name,
                settings: Box::new(settings),
            },
            Err(e) => CfResp::Err(e.to_string()),
        },
        CfReq::WorkerDeploy {
            token,
            account_id,
            name,
            path,
            mode,
        } => match std::fs::read(&path) {
            Ok(bytes) => {
                let filename = std::path::Path::new(&path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("worker.js")
                    .to_string();
                match make_client(&token).put_worker_script_content(
                    &account_id,
                    &name,
                    &filename,
                    bytes,
                    mode,
                ) {
                    Ok(script) => CfResp::Done(format!(
                        "Worker '{}' deployed",
                        if script.id.is_empty() {
                            name
                        } else {
                            script.id
                        }
                    )),
                    Err(e) => CfResp::Err(e.to_string()),
                }
            }
            Err(e) => CfResp::Err(format!("Can't read {path}: {e}")),
        },
        CfReq::WorkerDelete {
            token,
            account_id,
            name,
            force,
        } => match make_client(&token).delete_worker_script(&account_id, &name, force) {
            Ok(()) => CfResp::Done(format!("Worker '{name}' deleted")),
            Err(e) => CfResp::Err(e.to_string()),
        },
        CfReq::CreateR2Bucket {
            token,
            account_id,
            name,
        } => match make_client(&token).create_r2_bucket(&account_id, &name) {
            Ok(b) => CfResp::Done(format!("Bucket '{}' created", b.name)),
            Err(e) => CfResp::Err(e.to_string()),
        },
        CfReq::DeleteR2Bucket {
            token,
            account_id,
            name,
        } => match make_client(&token).delete_r2_bucket(&account_id, &name) {
            Ok(()) => CfResp::Done(format!("Bucket '{name}' deleted")),
            Err(e) => CfResp::Err(e.to_string()),
        },
        CfReq::R2Objects {
            token,
            account_id,
            bucket,
            prefix,
        } => match make_client(&token).list_r2_level(&account_id, &bucket, &prefix) {
            Ok(level) => CfResp::R2Objects {
                bucket,
                prefix,
                folders: level.folders,
                objects: level.files,
                truncated: level.truncated,
            },
            Err(e) => CfResp::Err(e.to_string()),
        },
        // Read the file HERE (not on the UI thread), enforce the 300 MB REST cap BEFORE
        // reading its bytes into memory, then PUT. The key is the browsed prefix + the
        // local basename.
        CfReq::R2Put {
            token,
            account_id,
            bucket,
            prefix,
            path,
        } => match std::fs::metadata(&path) {
            Ok(m) if m.len() > MAX_REST_OBJECT_BYTES => CfResp::Err(format!(
                "{path} is {} — over the 300 MB REST upload limit; larger objects need the S3 API",
                crate::output::format_bytes(m.len() as f64)
            )),
            Ok(_) => match std::fs::read(&path) {
                Ok(bytes) => {
                    let key = upload_key(&prefix, &path);
                    match make_client(&token).put_object(
                        &account_id,
                        &bucket,
                        &key,
                        bytes,
                        None,
                    ) {
                        Ok(()) => {
                            let where_ = if prefix.is_empty() {
                                bucket.clone()
                            } else {
                                format!("{bucket}/{prefix}")
                            };
                            CfResp::Done(format!("Uploaded {} → {where_}", object_basename(&key)))
                        }
                        Err(e) => CfResp::Err(e.to_string()),
                    }
                }
                Err(e) => CfResp::Err(format!("Can't read {path}: {e}")),
            },
            Err(e) => CfResp::Err(format!("Can't read {path}: {e}")),
        },
        // Refuse to overwrite an existing dest; stream the body into the file. A failed
        // download removes the partial file it created rather than leaving a stub.
        CfReq::R2Get {
            token,
            account_id,
            bucket,
            key,
            dest,
        } => {
            if std::path::Path::new(&dest).exists() {
                CfResp::Err(format!("{dest} already exists — refusing to overwrite"))
            } else {
                match std::fs::File::create(&dest) {
                    Ok(mut f) => {
                        match make_client(&token).download_object(
                            &account_id,
                            &bucket,
                            &key,
                            &mut f,
                        ) {
                            Ok(n) => CfResp::Status(format!(
                                "Downloaded {bucket}/{key} → {dest} ({})",
                                crate::output::format_bytes(n as f64)
                            )),
                            Err(e) => {
                                let _ = std::fs::remove_file(&dest);
                                CfResp::Err(e.to_string())
                            }
                        }
                    }
                    Err(e) => CfResp::Err(format!("Can't create {dest}: {e}")),
                }
            }
        }
        // Download each marked key into `dir` under its basename, one at a time. A name
        // already present is a per-key failure (never a clobber); a failed download drops
        // its partial file. Reports how many landed.
        CfReq::R2GetMany {
            token,
            account_id,
            bucket,
            keys,
            dir,
        } => {
            let client = make_client(&token);
            let (mut ok, mut failed) = (0usize, Vec::new());
            for key in keys {
                let dest = std::path::Path::new(&dir).join(object_basename(&key));
                if dest.exists() {
                    failed.push((key, "a file of that name already exists".to_string()));
                    continue;
                }
                match std::fs::File::create(&dest) {
                    Ok(mut f) => match client.download_object(&account_id, &bucket, &key, &mut f) {
                        Ok(_) => ok += 1,
                        Err(e) => {
                            let _ = std::fs::remove_file(&dest);
                            failed.push((key, e.to_string()));
                        }
                    },
                    Err(e) => failed.push((key, e.to_string())),
                }
            }
            let msg = if failed.is_empty() {
                format!("Downloaded {ok} object(s) to the current directory")
            } else {
                let detail = failed
                    .iter()
                    .map(|(k, e)| format!("{k}: {e}"))
                    .collect::<Vec<_>>()
                    .join("; ");
                format!("{ok} downloaded · {} failed — {detail}", failed.len())
            };
            CfResp::Status(msg)
        }
        // Delete each key, one call per key — a mid-list failure never aborts the rest.
        // Reports pass/fail; Done re-lists the level.
        CfReq::R2Delete {
            token,
            account_id,
            bucket,
            keys,
        } => {
            let client = make_client(&token);
            let (mut ok, mut failed) = (0usize, Vec::new());
            for key in keys {
                match client.delete_object(&account_id, &bucket, &key) {
                    Ok(()) => ok += 1,
                    Err(e) => failed.push((key, e.to_string())),
                }
            }
            let msg = if failed.is_empty() {
                format!("Deleted {ok} object(s)")
            } else {
                let detail = failed
                    .iter()
                    .map(|(k, e)| format!("{k}: {e}"))
                    .collect::<Vec<_>>()
                    .join("; ");
                format!("{ok} deleted · {} failed — {detail}", failed.len())
            };
            CfResp::Done(msg)
        }
    }
}

fn tunnel_target(tunnel_id: &str) -> String {
    format!("{}.cfargotunnel.com", tunnel_id.trim())
}

fn cf_zone_for_hostname(
    client: &CloudflareClient,
    account_id: &str,
    hostname: &str,
    zone: Option<&str>,
) -> Result<Zone> {
    if let Some(zone) = zone.filter(|z| !z.trim().is_empty()) {
        return client
            .list_zones(Some(account_id))?
            .into_iter()
            .find(|z| z.id == zone || z.name == zone)
            .ok_or_else(|| anyhow::anyhow!("No zone named or identified by '{zone}'"));
    }
    client
        .list_zones(Some(account_id))?
        .into_iter()
        .filter(|z| hostname == z.name || hostname.ends_with(&format!(".{}", z.name)))
        .max_by_key(|z| z.name.len())
        .ok_or_else(|| {
            anyhow::anyhow!("Cannot infer a zone for '{hostname}'. Fill DNS Zone explicitly.")
        })
}

fn upsert_tunnel_dns(
    client: &CloudflareClient,
    account_id: &str,
    tunnel_id: &str,
    hostname: &str,
    zone: Option<&str>,
    proxied: bool,
) -> Result<Record> {
    let hostname = hostname.trim();
    let zone = cf_zone_for_hostname(client, account_id, hostname, zone)?;
    let target = tunnel_target(tunnel_id);
    let body = record_body("CNAME", hostname, &target, 1, proxied, None);
    let matches = client.list_records(
        &zone.id,
        &RecordFilter {
            kind: Some("CNAME".into()),
            name: Some(hostname.to_string()),
            content: None,
        },
    )?;
    if let Some(existing) = matches
        .into_iter()
        .find(|r| r.name.eq_ignore_ascii_case(hostname))
    {
        client.patch_record(&zone.id, &existing.id, &body)
    } else {
        client.create_record(&zone.id, &body)
    }
}

fn delete_tunnel_dns(
    client: &CloudflareClient,
    account_id: &str,
    tunnel_id: &str,
    hostname: &str,
    zone: Option<&str>,
) -> Result<usize> {
    let hostname = hostname.trim();
    let zone = cf_zone_for_hostname(client, account_id, hostname, zone)?;
    let target = tunnel_target(tunnel_id);
    let records = client.list_records(
        &zone.id,
        &RecordFilter {
            kind: Some("CNAME".into()),
            name: Some(hostname.to_string()),
            content: Some(target.clone()),
        },
    )?;
    let mut deleted = 0;
    for record in records {
        if record.name.eq_ignore_ascii_case(hostname) && record.content == target {
            client.delete_record(&zone.id, &record.id)?;
            deleted += 1;
        }
    }
    Ok(deleted)
}

/// Back a database up once, through a schedule that exists only for this run.
///
/// `runDatabaseBackup` needs a schedule id and there is no one-off endpoint, so:
/// create disabled → run → delete. The delete runs even when the backup itself
/// failed, otherwise a failed "backup now" would leave a stray disabled schedule
/// in the panel for the user to find and wonder about.
fn backup_now(
    client: &EasypanelClient,
    project: &str,
    service: &str,
    database: &str,
    provider: &str,
    path: &str,
) -> Resp {
    // Desktop documents can request backups concurrently; keep temporary schedule ownership unambiguous.
    static BACKUP_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = match BACKUP_LOCK.lock() {
        Ok(guard) => guard,
        Err(_) => return Resp::Err("Backup preparation lock failed".into()),
    };
    let ps = json!({ "projectName": project, "serviceName": service });
    let before = match client.call("databaseBackups", "listDatabaseBackups", ps.clone()) {
        Ok(value) => value,
        Err(e) => return Resp::Err(format!("Backup could not be prepared: {e}")),
    };
    if !before.as_array().is_some_and(|items| {
        items.iter().all(|item| {
            item.get("id")
                .and_then(Value::as_str)
                .is_some_and(crate::backup::is_named)
        })
    }) {
        return Resp::Err("Backup schedule list has invalid IDs; nothing created".into());
    }
    let body = crate::backup::schedule_body(
        project,
        service,
        database,
        "0 3 * * *",
        false,
        provider,
        path,
    );
    if let Err(e) = client.call("databaseBackups", "createDatabaseBackup", body) {
        return Resp::Err(format!("Backup schedule creation failed or stopped reporting: {e}; inspect Database backups before retrying"));
    }
    let after = match client.call("databaseBackups", "listDatabaseBackups", ps) {
        Ok(value) => value,
        Err(e) => return Resp::Err(format!("Backup schedule was created but could not be identified: {e}; inspect Database backups before retrying")),
    };
    let id = match created_backup_id(&before, &after, database) {
        Ok(id) => id,
        Err(error) => return Resp::Err(error),
    };

    let ran = client.call("databaseBackups", "runDatabaseBackup", json!({ "id": id }));
    let cleaned = client.call(
        "databaseBackups",
        "deleteDatabaseBackup",
        json!({ "id": id }),
    );
    if let Err(error) = cleaned {
        return Resp::Err(match ran {
            Ok(_) => format!("Backup of {database} started, but temporary schedule {id} could not be deleted: {error}"),
            Err(run) => format!("Backup of {database} refused: {run}; temporary schedule {id} could not be deleted: {error}"),
        });
    }
    match ran {
        // The panel writes the file asynchronously, so the outcome lands in the
        // Actions tab rather than here — saying "done" would be a guess.
        Ok(_) => Resp::Done(
            format!("Backup of {database} started — the result appears in Actions"),
            Refresh::None,
        ),
        // A database that isn't running answers "Invariant failed", which says
        // nothing; the likely cause is worth naming.
        Err(e) => Resp::Err(format!(
            "Backup of {database} refused: {e} (is the database running?)"
        )),
    }
}

/// One lifecycle action across many services.
///
/// Two different shapes hide behind one call, because the endpoints behave
/// differently (both measured against a live panel):
///
/// - **deploy** BLOCKS until the build finishes — 51 seconds for a trivial
///   Dockerfile, minutes for anything real, past every proxy limit. Awaiting a
///   dozen of those would freeze the UI for as long as the slowest build. They are
///   DISPATCHED like a single deploy: fired on their own threads, reported as
///   started, with only outright refusals (4xx) coming back later. The Status
///   column and the Actions tab carry the real outcome.
/// - **start/stop/restart** return when the server is done (0.2-5 s), so these ARE
///   awaited and reported per service — that is the whole point of a bulk run.
///
/// `CHUNK` bounds how many run at once. Without it, marking a whole panel would
/// open one connection per service simultaneously.
/// ponytail: fixed chunks, not a real pool — the tail of each chunk waits for its
/// slowest member. Swap in a pool if bulk over hundreds of services gets slow.
/// Fetch two services and hand the pair to the diff.
///
/// `inspectService` is grouped by type, and the two services may be different
/// types (comparing an app with a compose is a legitimate "why are these set up
/// differently"), so each is fetched with its own group.
/// Fetch one service's full config. Grouped by type, since two services being
/// compared may be different types.
fn fetch_inspect(
    client: &EasypanelClient,
    (project, service, stype): &(String, String, String),
) -> anyhow::Result<Value> {
    client.call(
        &format!("services/{stype}"),
        "inspectService",
        json!({ "projectName": project, "serviceName": service }),
    )
}

/// The pre-flight rows the copy confirmation shows, indented exactly as the CLI's
/// `db copy` prints the same plan.
///
/// The indent is what tells `render_confirm` this is an ALIGNED BLOCK and not
/// prose, so the rows keep their column instead of each being centred on its own.
///
/// Storage takes three rows, not one. It used to be a single line carrying the
/// endpoint, the bucket and both provider names — around 100 characters, which the
/// 60-column dialog hard-broke mid-token through the middle of an opaque R2
/// hostname. Split, every row fits and nothing is dropped: the bucket comes FIRST
/// (so an elision at the width eats the generic `.r2.cloudflarestorage.com` tail,
/// never the account that identifies it), and each panel's own name for that
/// storage gets a row, mirroring the `source image` / `target image` pair above.
/// Both ends provably point at the same bucket — `plan_copy` refuses the copy
/// otherwise (`dump::store_refusal`) — so ONE storage row is the honest shape.
pub(crate) fn copy_plan_lines(plan: &crate::operations::CopyPlan) -> Vec<String> {
    // The scheme is the same on every S3 endpoint and costs 8 of the columns the
    // account id needs.
    let host = plan
        .endpoint
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    vec![
        format!("  engine        {}", plan.stype),
        format!("  databases     {}", plan.databases.join(", ")),
        // Shown, never compared: no tag says whether a load fits.
        format!("  source image  {}", plan.src_image),
        format!("  target image  {}", plan.dst_image),
        format!("  via storage   {} @ {host}", plan.bucket),
        format!("  source store  '{}'", plan.src_provider),
        format!("  target store  '{}'", plan.dst_provider),
    ]
}

/// One step of the database browser, start to finish: what the engine needs,
/// what it was asked, and what it answered.
///
/// Every failure on the way — an unsupported engine, an unreachable panel, a
/// broken query — comes back as `Resp::DbmsFailed` so the screen can SAY it. The
/// one thing this must never do is return an empty grid for a step that failed.
#[allow(clippy::too_many_arguments)]
fn dbms_step(
    client: &EasypanelClient,
    project: String,
    service: String,
    stype: String,
    level: crate::dbms::Level,
    database: String,
    table: String,
    query: String,
) -> Resp {
    use crate::dbms;
    use crate::dbms::Level;

    let Some(engine) = dbms::Engine::from_service_type(&stype) else {
        return Resp::DbmsFailed {
            message: dbms::unsupported_reason(&stype)
                .unwrap_or_else(|| format!("{stype} cannot be browsed")),
        };
    };
    let inspect = match client.call(
        &format!("services/{stype}"),
        "inspectService",
        json!({ "projectName": project, "serviceName": service }),
    ) {
        Ok(v) => v,
        Err(e) => {
            return Resp::DbmsFailed {
                message: e.to_string(),
            }
        }
    };
    let creds = dbms::creds(engine, &inspect);

    let command = match level {
        Level::Databases => dbms::list_databases_cmd(engine, &creds),
        Level::Tables => dbms::list_tables_cmd(engine, &creds, &database),
        Level::Rows => dbms::preview_cmd(engine, &creds, &database, &table, dbms::PREVIEW_LIMIT),
        Level::Query => dbms::query_cmd(engine, &creds, &database, &query),
    };
    // A listing is a fixed, cheap statement; what the user typed is not, and the
    // cap is the only thing standing between a cartesian join and a frozen lane.
    let cap = match level {
        Level::Query | Level::Rows => Duration::from_secs(60),
        _ => Duration::from_secs(20),
    };
    let capture = match crate::container::run_capture(client, &project, &service, &command, cap) {
        Ok(c) => c,
        Err(e) => {
            return Resp::DbmsFailed {
                message: e.to_string(),
            }
        }
    };
    if let Some(message) = dbms::failure(&capture) {
        return Resp::DbmsFailed { message };
    }

    let grid = match level {
        Level::Databases => dbms::Grid::from_names("Database", dbms::parse_names(&capture.output)),
        Level::Tables => {
            dbms::Grid::from_names(engine.table_word(), dbms::parse_names(&capture.output))
        }
        Level::Rows | Level::Query => dbms::parse_grid(engine, &capture.output),
    };
    Resp::Dbms {
        project,
        service,
        level,
        database,
        table,
        capped: level == Level::Rows && grid.rows.len() >= dbms::PREVIEW_LIMIT,
        truncated: capture.truncated,
        columns: grid.columns,
        rows: grid.rows,
    }
}

fn diff_services(
    client: &EasypanelClient,
    a: (String, String, String),
    b: (String, String, String),
) -> Resp {
    let (va, vb) = match (fetch_inspect(client, &a), fetch_inspect(client, &b)) {
        (Ok(va), Ok(vb)) => (va, vb),
        (Err(e), _) | (_, Err(e)) => return Resp::Err(e.to_string()),
    };
    let (la, lb) = (format!("{}/{}", a.0, a.1), format!("{}/{}", b.0, b.1));
    let diffs = crate::services::diff(&va, &vb);
    Resp::Viewer(
        format!(" Diff: {la} vs {lb} "),
        crate::services::diff_lines(&diffs, &la, &lb),
    )
}

// Both initial creation and retry use the same phase transitions. The request
// retains the original payload; a completed phase is never replayed on retry.
fn service_setup(
    client: &EasypanelClient,
    project: String,
    service: String,
    stype: String,
    (op, mut body, auto_deploy): crate::form::SourceCall,
    phase: ServiceSetupPhase,
) -> Resp {
    let group = format!("services/{stype}");
    if phase == ServiceSetupPhase::Source {
        body["projectName"] = json!(project);
        body["serviceName"] = json!(service);
        if let Err(error) = client.call(&group, op, body) {
            return Resp::ServiceCreated {
                project, service, stype, pending: Some(ServiceSetupPhase::Source),
                notes: vec![format!("Source setup failed: {error}. Repair the existing service's Source settings; do not create it again.")],
            };
        }
    }
    if let Some(enabled) = auto_deploy {
        let operation = if enabled {
            "enableGithubDeploy"
        } else {
            "disableGithubDeploy"
        };
        if let Err(error) = client.call(
            &group,
            operation,
            json!({"projectName":project,"serviceName":service}),
        ) {
            return Resp::ServiceCreated {
                notes: vec![auto_deploy_error(&service, &error.to_string())],
                project,
                service,
                stype,
                pending: Some(ServiceSetupPhase::AutoDeploy),
            };
        }
    } else if phase == ServiceSetupPhase::AutoDeploy {
        return Resp::ServiceCreated {
            project,
            service,
            stype,
            pending: Some(ServiceSetupPhase::AutoDeploy),
            notes: vec!["No GitHub auto-deploy setting was provided for this retry".into()],
        };
    }
    Resp::ServiceCreated {
        project,
        service,
        stype,
        notes: Vec::new(),
        pending: None,
    }
}

fn bulk_action(
    client: &EasypanelClient,
    emit: &Emit,
    targets: Vec<(String, String, String)>,
    action: &str,
    force: bool,
) -> Resp {
    const CHUNK: usize = 6;

    if targets.is_empty() {
        return Resp::Err("Nothing marked".into());
    }

    if action == "deploy" {
        let (mut ok, mut failed) = (Vec::new(), Vec::new());
        for (project, service, stype) in targets {
            let name = format!("{project}/{service}");
            if crate::lifecycle::ops(&stype, action).is_none() {
                failed.push((name, crate::lifecycle::unavailable(&stype, action)));
                continue;
            }
            let client = client.clone();
            let emit = Arc::clone(emit);
            let group = format!("services/{stype}");
            let input =
                json!({ "projectName": project, "serviceName": service, "forceRebuild": force });
            let error_name = name.clone();
            thread::spawn(move || {
                if let Err(error) = client.call(&group, "deployService", input) {
                    if !crate::client::gave_up_waiting(&error) {
                        emit(Resp::Err(format!("Deploy {error_name} failed: {error}")));
                    }
                }
            });
            ok.push(name);
        }
        return Resp::BulkDone {
            action: action.to_string(),
            ok,
            failed,
        };
    }

    let mut ok = Vec::new();
    let mut failed = Vec::new();
    for group in targets.chunks(CHUNK) {
        let handles: Vec<_> = group
            .iter()
            .map(|(project, service, stype)| {
                let c = client.clone();
                let grp = format!("services/{stype}");
                // The SAME rule as a single action: a database restarts by
                // cycling `enabled`, and has no deploy at all. Spelling the
                // endpoint out here again is how a bulk run would keep 404ing
                // on databases after the single path was fixed.
                let ops = crate::lifecycle::ops(stype, action);
                let why = crate::lifecycle::unavailable(stype, action);
                let (p, s) = (project.clone(), service.clone());
                thread::spawn(move || {
                    let name = format!("{p}/{s}");
                    let Some(ops) = ops else {
                        return Err((name, why));
                    };
                    for op in ops {
                        let input = json!({ "projectName": p, "serviceName": s });
                        if let Err(e) = c.call(&grp, op, input) {
                            return Err((name, e.to_string()));
                        }
                    }
                    Ok(name)
                })
            })
            .collect();
        for h in handles {
            // A panicked thread is a failure too, not a silent omission: the
            // summary must account for every service that was marked.
            match h.join() {
                Ok(Ok(name)) => ok.push(name),
                Ok(Err(f)) => failed.push(f),
                Err(_) => failed.push(("(unknown)".into(), "worker thread panicked".into())),
            }
        }
    }
    Resp::BulkDone {
        action: action.to_string(),
        ok,
        failed,
    }
}

/// Apply the same resource limit to every marked service, one PUT each under its
/// own `services/{stype}` group. `updateResources` only stores config (it takes
/// effect on the next deploy), so this is a low-risk write done sequentially with
/// a per-service pass/fail — the same honest accounting as `bulk_action`.
///
/// A type that has no resource route (compose sets its limits in its file) is
/// reported as failed with the reason, never silently skipped: the summary must
/// account for every service that was marked.
fn bulk_resource(
    client: &EasypanelClient,
    targets: Vec<(String, String, String)>,
    resources: Value,
) -> Resp {
    if targets.is_empty() {
        return Resp::Err("Nothing marked".into());
    }
    let mut ok = Vec::new();
    let mut failed = Vec::new();
    for (project, service, stype) in &targets {
        let name = format!("{project}/{service}");
        if !crate::lifecycle::has_resource_limits(stype) {
            failed.push((
                name,
                format!("a {stype} service sets limits in its compose file"),
            ));
            continue;
        }
        let mut input = resources.clone();
        input["projectName"] = json!(project);
        input["serviceName"] = json!(service);
        match client.call(&format!("services/{stype}"), "updateResources", input) {
            Ok(_) => ok.push(name),
            Err(e) => failed.push((name, e.to_string())),
        }
    }
    Resp::BulkDone {
        action: "resource limits".into(),
        ok,
        failed,
    }
}

/// Search for `query` in the logs of every service at once — the killer feature.
///
/// EasyPanel has no "search across services" endpoint; we fan out
/// `queryServiceLogs` (which accepts `search`, verified on the server) to each
/// service in PARALLEL. One thread per service with a cloned client; logs are
/// backed by Loki, so the search runs server-side, fast. Results are merged,
/// grouped per service, only the ones with a match.
fn log_search(client: &EasypanelClient, query: &str) -> Resp {
    if query.trim().is_empty() {
        return Resp::Err("Search keyword is empty".into());
    }
    let all = match client.call("projects", "listProjectsAndServices", Value::Null) {
        Ok(v) => v,
        Err(e) => return Resp::Err(e.to_string()),
    };
    let services: Vec<(String, String)> = all
        .get("services")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|s| (field(s, "/projectName"), field(s, "/name")))
                .collect()
        })
        .unwrap_or_default();

    // Parallel fan-out: one thread per service. reqwest blocking shares a pool, but
    // Loki answers fast, so dozens of services finish in ~1-2 seconds.
    let handles: Vec<_> = services
        .into_iter()
        .map(|(project, service)| {
            let c = client.clone();
            let q = query.to_string();
            thread::spawn(move || {
                let v = c
                    .call(
                        "logs",
                        "queryServiceLogs",
                        json!({
                            "projectName": project, "serviceName": service,
                            "search": q, "limit": 40
                        }),
                    )
                    .ok()?;
                let lines = crate::logs::format(&v);
                if lines.is_empty() {
                    None
                } else {
                    Some((project, service, lines))
                }
            })
        })
        .collect();

    let mut hits: Vec<(String, String, Vec<String>)> =
        handles.into_iter().filter_map(|h| h.join().ok()?).collect();
    hits.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));

    let total: usize = hits.iter().map(|(_, _, l)| l.len()).sum();
    let mut out = Vec::new();
    for (project, service, lines) in &hits {
        out.push(format!("── {project}/{service} ({}) ──", lines.len()));
        out.extend(lines.iter().cloned());
        out.push(String::new());
    }
    if out.is_empty() {
        out.push(format!("No match for '{query}' in any service."));
    }
    Resp::Viewer(
        format!(
            "Search '{query}' — {total} lines in {} services",
            hits.len()
        ),
        out,
    )
}

pub(crate) fn fetch_view(
    client: &EasypanelClient,
    view: View,
    project: &str,
    service: &str,
    stype: &str,
) -> Result<Value> {
    let ps = json!({ "projectName": project, "serviceName": service });
    match view {
        View::Logs => client.call(
            "logs",
            "queryServiceLogs",
            json!({ "projectName": project, "serviceName": service, "limit": 200 }),
        ),
        View::ProjectEnv => client.call(
            "projects",
            "inspectProject",
            json!({ "projectName": project }),
        ),
        View::Ports => client.call("ports", "listPorts", ps),
        View::Mounts => client.call("mounts", "listMounts", ps),
        View::Backups => client.call("databaseBackups", "listDatabaseBackups", ps),
        View::Env | View::ConfigFile | View::Redirects | View::Source => {
            client.call(&format!("services/{stype}"), "inspectService", ps)
        }
    }
}

/// The list of GitHub repos as "owner/repo" for a dropdown.
///
/// GitHub may not be connected on a given host, and that's no reason to fail the
/// form: an empty list makes "Repo" a plain text input.
pub(crate) fn github_repos(client: &EasypanelClient) -> Vec<String> {
    let Ok(v) = client.call("github", "searchRepos", Value::Null) else {
        return Vec::new();
    };
    v.get("items")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|r| {
                    Some(format!(
                        "{}/{}",
                        r.get("owner")?.as_str()?,
                        r.get("repo")?.as_str()?
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// updateEnv body. `dot` = the `.env` file path (`dotEnvPath`): Some to write env
/// as a file, None for no file. The server rejects a null/empty dotEnvPath (min 1
/// char), so "no file" means the field is omitted entirely.
pub(crate) fn env_body(project: &str, service: &str, env: &str, dot: Option<&str>) -> Value {
    let mut body = json!({ "projectName": project, "serviceName": service, "env": env });
    if let Some(path) = dot {
        body["dotEnvPath"] = json!(path);
    }
    body
}

/// Read the current redirects, transform via `transform`, then updateRedirects the
/// whole array. updateRedirects replaces everything (there's no per-item endpoint),
/// so adding/removing one MUST go through read-modify-write or the rest are lost.
fn save_redirects(
    client: &EasypanelClient,
    stype: &str,
    project: &str,
    service: &str,
    transform: impl FnOnce(Vec<Value>) -> Vec<Value>,
) -> Result<()> {
    let grp = format!("services/{stype}");
    let ps = json!({ "projectName": project, "serviceName": service });
    let cur = client.call(&grp, "inspectService", ps)?;
    let list = cur
        .get("redirects")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let next = transform(list);
    client.call(
        &grp,
        "updateRedirects",
        json!({ "projectName": project, "serviceName": service, "redirects": next }),
    )?;
    Ok(())
}

/// Clone a service's CONFIG into `new_name` (not its data — volumes aren't
/// copied). Domains are NOT copied: the same host can't serve two identical
/// hostnames, so a clone would only collide.
fn clone_service(
    client: &EasypanelClient,
    project: &str,
    service: &str,
    stype: &str,
    target: &str,
    new_name: &str,
) -> Resp {
    let dst = crate::migrate::Target {
        client,
        project: target,
        service: new_name,
    };
    match crate::migrate::migrate_service(client, project, service, stype, &dst, false) {
        // The notes used to be dropped here (`Ok(_)`) while the migrate path
        // reported them: a clone that skipped the config file or failed to
        // re-enable auto-deploy said only "cloned", and the one thing the user
        // had to act on was the one thing thrown away.
        Ok(notes) => {
            let msg = format!(
                "'{service}' cloned into '{target}/{new_name}' — deploy when ready (data NOT included)"
            );
            if notes.is_empty() {
                Resp::Done(msg, Refresh::Projects)
            } else {
                Resp::Notes {
                    msg,
                    notes,
                    refresh: Refresh::Projects,
                }
            }
        }
        Err(e) => Resp::Err(format!("clone: {e:#}")),
    }
}

/// Turn the map `{ "{project}_{service}": {actual, desired} }` from
/// getDockerTaskStats into `swarm_name -> (actual, desired)`. Entries missing
/// either number are ignored.
pub(crate) fn parse_task_stats(v: &Value) -> HashMap<String, (i64, i64)> {
    v.as_object()
        .map(|o| {
            o.iter()
                .filter_map(|(k, t)| {
                    Some((
                        k.clone(),
                        (
                            t.get("actual").and_then(Value::as_i64)?,
                            t.get("desired").and_then(Value::as_i64)?,
                        ),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Service name+type from inspectProject, for the Service dropdown in the domain form.
pub(crate) fn parse_services(v: &Value) -> Vec<(String, String)> {
    v.get("services")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|s| {
                    (
                        field(s, "/name"),
                        s.get("type")
                            .and_then(Value::as_str)
                            .unwrap_or("app")
                            .to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Condense an API error into an actionable cause.
///
/// EasyPanel wraps upstream errors, so a dead GitHub token shows up as "[400]
/// Request failed with status code 403 Forbidden" — two status codes and zero
/// hints. What the user needs to know is that their credential was rejected.
/// A failed-auto-deploy message that names the cause, not a stack of status codes.
///
/// enable/disableGithubDeploy creates a GitHub webhook, so it fails for a repo we
/// don't control. EasyPanel forwards it as a 400 that contains a 404 from
/// `GET /repos/{owner}/{repo}/hooks` — observed directly on the server for a
/// service sourced from a third-party repo.
///
/// Anything unrecognized is returned as-is: a long server message is still more
/// useful than "failed", and dropping it is a bug this project has had before.
pub(crate) fn auto_deploy_error(service: &str, raw: &str) -> String {
    if raw.contains("404") && raw.contains("/hooks") {
        format!("Auto deploy {service}: no webhook access to that repo — usually because it's a third-party repo")
    } else {
        format!("Auto deploy {service} failed: {raw}")
    }
}

fn created_backup_id(before: &Value, after: &Value, database: &str) -> Result<String, String> {
    let prior = before.as_array().ok_or("Backup list is not an array")?;
    let current = after.as_array().ok_or("Backup list is not an array")?;
    let mut candidates = current.iter().filter(|item| {
        item.get("databaseName").and_then(Value::as_str) == Some(database)
            && item.get("id").and_then(Value::as_str).is_some_and(|id| {
                !id.is_empty()
                    && !prior
                        .iter()
                        .any(|old| old.get("id").and_then(Value::as_str) == Some(id))
            })
    });
    let item = candidates.next().filter(|_| candidates.next().is_none()).ok_or(
        "Backup schedule was created but cannot be uniquely identified; inspect Database backups before retrying",
    )?;
    Ok(item["id"].as_str().unwrap().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;

    #[test]
    fn fetch_and_delete_refresh_preserve_raw_collection_and_target() {
        let server = MockServer::start();
        let ports =
            json!([{ "published": 8443, "target": 443, "protocol": "tcp", "futureOption": true }]);
        let list = server.mock(|when, then| {
            when.method(POST)
                .path("/api/rpc/ports/listPorts")
                .json_body(json!({"json":{"projectName":"project","serviceName":"web"}}));
            then.status(200).json_body(json!({"json":ports}));
        });
        let delete = server.mock(|when, then| {
            when.method(POST)
                .path("/api/rpc/ports/deletePort")
                .json_body(json!({"json":{"projectName":"project","serviceName":"web","index":0}}));
            then.status(200).json_body(json!({"json":null}));
        });
        let client = EasypanelClient::new(&server.base_url(), "test-only");
        let emit: Emit = Arc::new(|_| {});
        for request in [
            Req::Fetch {
                view: View::Ports,
                project: "project".into(),
                service: "web".into(),
                stype: "app".into(),
            },
            Req::PortDelete {
                project: "project".into(),
                service: "web".into(),
                index: 0,
            },
        ] {
            match handle_req(&client, request, &emit) {
                Resp::Fetched {
                    view: View::Ports,
                    project,
                    service,
                    data,
                    ..
                } => {
                    assert_eq!((project.as_str(), service.as_str()), ("project", "web"));
                    assert_eq!(data, ports);
                }
                _ => panic!("collection must remain a structured fetch"),
            }
        }
        list.assert_hits(2);
        delete.assert_hits(1);
    }

    #[test]
    fn confirmed_creation_keeps_its_target_when_source_or_autodeploy_setup_fails() {
        for (source_status, auto_status) in [(400, 200), (200, 400), (200, 200)] {
            let server = MockServer::start();
            let create = server.mock(|when, then| {
                when.method(POST)
                    .path("/api/rpc/services/app/createService")
                    .json_body(json!({"json":{"projectName":"project","serviceName":"web"}}));
                then.status(200).json_body(json!({"json":null}));
            });
            let source = server.mock(|when, then| {
                when.method(POST)
                    .path("/api/rpc/services/app/updateSourceGithub");
                then.status(source_status)
                    .json_body(json!({"json":{"message":"Source refused"}}));
            });
            let auto = server.mock(|when, then| {
                when.method(POST)
                    .path("/api/rpc/services/app/enableGithubDeploy");
                then.status(auto_status)
                    .json_body(json!({"json":{"message":"Webhook refused"}}));
            });
            let deploy = server.mock(|when, then| {
                when.method(POST)
                    .path("/api/rpc/services/app/deployService");
                then.status(200).json_body(json!({"json":null}));
            });
            let emit: Emit = Arc::new(|_| {});
            match handle_req(
                &EasypanelClient::new(&server.base_url(), "test-only"),
                Req::ServiceCreate {
                    project: "project".into(),
                    service: "web".into(),
                    stype: "app".into(),
                    extra: json!({}),
                    source: Some((
                        "updateSourceGithub",
                        json!({"owner":"owner","repo":"repository","ref":"main"}),
                        Some(true),
                    )),
                },
                &emit,
            ) {
                Resp::ServiceCreated {
                    project,
                    service,
                    stype,
                    notes,
                    pending,
                } => {
                    assert_eq!(
                        (project.as_str(), service.as_str(), stype.as_str()),
                        ("project", "web", "app")
                    );
                    assert_eq!(notes.is_empty(), source_status == 200 && auto_status == 200);
                    assert_eq!(
                        pending,
                        if source_status != 200 {
                            Some(ServiceSetupPhase::Source)
                        } else if auto_status != 200 {
                            Some(ServiceSetupPhase::AutoDeploy)
                        } else {
                            None
                        }
                    );
                    if source_status != 200 {
                        assert!(notes[0].contains("Source refused"));
                    } else if auto_status != 200 {
                        assert!(notes[0].contains("Webhook refused"));
                    }
                }
                _ => panic!("a confirmed resource must never look like a failed create"),
            }
            create.assert_hits(1);
            source.assert_hits(1);
            auto.assert_hits(usize::from(source_status == 200));
            deploy.assert_hits(0);
        }
    }

    #[test]
    fn service_setup_retries_only_the_unfinished_phase_and_never_creates() {
        for phase in [ServiceSetupPhase::Source, ServiceSetupPhase::AutoDeploy] {
            let server = MockServer::start();
            let create = server.mock(|when, then| {
                when.method(POST)
                    .path("/api/rpc/services/app/createService");
                then.status(500)
                    .json_body(json!({"json":{"message":"must not create again"}}));
            });
            let source = server.mock(|when, then| {
                when.method(POST)
                    .path("/api/rpc/services/app/updateSourceGithub");
                then.status(200).json_body(json!({"json":null}));
            });
            let auto = server.mock(|when, then| {
                when.method(POST)
                    .path("/api/rpc/services/app/disableGithubDeploy");
                then.status(if phase == ServiceSetupPhase::Source {
                    400
                } else {
                    200
                })
                .json_body(json!({"json":{"message":"Webhook unavailable"}}));
            });
            let emit: Emit = Arc::new(|_| {});
            let result = handle_req(
                &EasypanelClient::new(&server.base_url(), "test-only"),
                Req::ServiceSetup {
                    project: "project".into(),
                    service: "web".into(),
                    stype: "app".into(),
                    phase,
                    source: (
                        "updateSourceGithub",
                        json!({"owner":"owner","repo":"repository","ref":"main"}),
                        Some(false),
                    ),
                },
                &emit,
            );
            match result {
                Resp::ServiceCreated { pending, notes, .. } => {
                    if phase == ServiceSetupPhase::Source {
                        assert_eq!(pending, Some(ServiceSetupPhase::AutoDeploy));
                        assert!(notes[0].contains("Webhook unavailable"));
                    } else {
                        assert_eq!(pending, None);
                        assert!(notes.is_empty());
                    }
                }
                _ => panic!("setup retry must retain its existing-service outcome"),
            }
            create.assert_hits(0);
            source.assert_hits(usize::from(phase == ServiceSetupPhase::Source));
            auto.assert_hits(1);
        }
    }

    #[test]
    fn bulk_deploy_dispatches_buildable_targets_and_reports_every_unsupported_target() {
        let server = MockServer::start();
        let app = server.mock(|when, then| {
            when.method(POST)
                .path("/api/rpc/services/app/deployService");
            then.status(400)
                .json_body(json!({"json":{"message":"app refusal"}}));
        });
        let compose = server.mock(|when, then| {
            when.method(POST)
                .path("/api/rpc/services/compose/deployService");
            then.status(400)
                .json_body(json!({"json":{"message":"compose refusal"}}));
        });
        let (tx, rx) = std::sync::mpsc::channel();
        let emit: Emit = Arc::new(move |response| {
            let _ = tx.send(response);
        });
        let targets = [
            ("web", "app"),
            ("stack", "compose"),
            ("db", "mysql"),
            ("site", "wordpress"),
            ("shell", "box"),
        ]
        .into_iter()
        .map(|(service, kind)| ("project".into(), service.into(), kind.into()))
        .collect();
        let result = bulk_action(
            &EasypanelClient::new(&server.base_url(), "test-only"),
            &emit,
            targets,
            "deploy",
            true,
        );
        match result {
            Resp::BulkDone { action, ok, failed } => {
                assert_eq!(action, "deploy");
                assert_eq!(ok, ["project/web", "project/stack"]);
                assert_eq!(failed.len(), 3);
                for (name, reason) in failed {
                    assert!(name.starts_with("project/") && !reason.is_empty());
                }
            }
            _ => panic!("bulk dispatch must account for supported and unsupported targets"),
        }
        for _ in 0..2 {
            assert!(matches!(
                rx.recv_timeout(Duration::from_secs(3)).unwrap(),
                Resp::Err(_)
            ));
        }
        assert!(rx.try_recv().is_err());
        app.assert_hits(1);
        compose.assert_hits(1);
    }

    #[test]
    fn dispatched_deploy_keeps_the_sink_for_late_refusals() {
        let server = MockServer::start();
        let deploy = server.mock(|when, then| {
            when.method(POST)
                .path("/api/rpc/services/app/deployService");
            then.status(400)
                .json_body(json!({"json":{"message":"Invalid build configuration"}}));
        });
        let (tx, rx) = std::sync::mpsc::channel();
        let emit: Emit = Arc::new(move |response| {
            let _ = tx.send(response);
        });
        let response = handle_req(
            &EasypanelClient::new(&server.base_url(), "test-only"),
            Req::Action {
                project: "project".into(),
                service: "web".into(),
                stype: "app".into(),
                action: "deploy".into(),
                force: false,
            },
            &emit,
        );
        assert!(matches!(response, Resp::Done(_, Refresh::None)));
        drop(emit);
        assert!(
            matches!(rx.recv_timeout(Duration::from_secs(3)).unwrap(),Resp::Err(error)
            if error.contains("Invalid build configuration") && error.contains("project/web"))
        );
        deploy.assert_hits(1);
    }

    #[test]
    fn backup_now_attempts_cleanup_after_refusal_and_reports_both_failures() {
        use std::io::{BufRead, Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let responses = [
                ("listDatabaseBackups", 200, json!([])),
                ("createDatabaseBackup", 200, Value::Null),
                (
                    "listDatabaseBackups",
                    200,
                    json!([{"id":"owned-schedule","databaseName":"shop"}]),
                ),
                (
                    "runDatabaseBackup",
                    400,
                    json!({"message":"Engine refused backup"}),
                ),
                (
                    "deleteDatabaseBackup",
                    500,
                    json!({"message":"Cleanup unavailable"}),
                ),
            ];
            for (operation, status, response) in responses {
                let deadline = std::time::Instant::now() + Duration::from_secs(5);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error)
                            if error.kind() == std::io::ErrorKind::WouldBlock
                                && std::time::Instant::now() < deadline =>
                        {
                            thread::sleep(Duration::from_millis(5))
                        }
                        Err(error) => panic!("waiting for {operation}: {error}"),
                    }
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut reader = std::io::BufReader::new(&mut stream);
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                assert!(line.starts_with(&format!("POST /api/rpc/databaseBackups/{operation} ")));
                let mut length = 0;
                loop {
                    line.clear();
                    assert!(
                        reader.read_line(&mut line).unwrap() > 0,
                        "request headers ended early"
                    );
                    if line == "\r\n" {
                        break;
                    }
                    if let Some((name, value)) = line.split_once(':') {
                        if name.eq_ignore_ascii_case("content-length") {
                            length = value.trim().parse().unwrap();
                        }
                    }
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                drop(reader);
                if operation == "runDatabaseBackup" || operation == "deleteDatabaseBackup" {
                    assert_eq!(
                        serde_json::from_slice::<Value>(&body).unwrap(),
                        json!({"json":{"id":"owned-schedule"}})
                    );
                }
                let body = json!({"json":response}).to_string();
                write!(stream,"HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
            }
        });
        let result = backup_now(
            &EasypanelClient::new(&url, "test-only"),
            "project",
            "db",
            "shop",
            "provider",
            "backups",
        );
        assert!(
            matches!(result,Resp::Err(message) if message.contains("Engine refused backup")
            && message.contains("Cleanup unavailable") && message.contains("owned-schedule"))
        );
        server.join().unwrap();
    }

    #[test]
    fn temporary_backup_identity_never_selects_a_preexisting_or_ambiguous_schedule() {
        let before = json!([{"id":"existing","databaseName":"app"}]);
        assert!(created_backup_id(&before, &before, "app").is_err());
        let after = json!([
            {"id":"created","databaseName":"app"},
            {"id":"existing","databaseName":"app"},
            {"id":"other","databaseName":"other"}
        ]);
        assert_eq!(
            created_backup_id(&before, &after, "app").unwrap(),
            "created"
        );
        let ambiguous = json!([
            {"id":"created","databaseName":"app"},
            {"id":"external","databaseName":"app"}
        ]);
        assert!(created_backup_id(&before, &ambiguous, "app").is_err());
    }
}
