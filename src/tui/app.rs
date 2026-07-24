use std::collections::{HashMap, HashSet};
use std::sync::mpsc::Sender;
use std::time::Instant;

use ratatui::layout::Rect;
use ratatui::widgets::{ListState, TableState};
use serde_json::{json, Value};

use crate::cloudflare::{
    apply_patch, filter_buckets, filter_records, filter_tunnel_config_rows, filter_tunnels,
    filter_worker_deployments, filter_worker_settings_rows, filter_workers, filter_zones,
    parse_tunnel_origin_request, proxyable, record_body, valid_record_type, AnalyticsSummary,
    CloudflareAccount, CloudflareTunnel, R2Bucket, R2Object, Record, RecordFilter, RecordPatch,
    TunnelConfigRow, TunnelConfiguration, TunnelIngressRule, WebAnalyticsSite, WorkerDeployment,
    WorkerScript, WorkerSettingsBundle, WorkerSettingsRow, WorkerUploadMode, Zone,
};
use crate::commands;
use crate::output::field;

use super::actions::{Menu, MenuItem, Palette};
use super::backup_ui::BackupUi;
use super::form::*;
use super::render::cap;
use super::table::*;
use super::worker::{CfReq, CfResp, Refresh, Req, Resp, View};
use super::LOG_BUFFER;

// ---------- State ----------

#[derive(PartialEq, Clone, Copy)]
pub(super) enum Screen {
    Dashboard,
    /// Every host at once — the one screen a web panel can't replace.
    Hosts,
    /// Docker info & cleanup on the active server.
    Maintenance,
    Actions,
    Monitor,
    Domains,
    Projects,
    /// The domains the operator enrolled for uptime checks, and their last
    /// answers. Deliberately a tab of its own: it is a short, curated list, not
    /// a view of the 700-row domain table.
    Uptime,
    Viewer,
    /// An embedded container terminal; opened from a service.
    Terminal,
    /// A database service's connection identity (user, password, host, URL),
    /// opened from a service. Read-only, with reveal + copy.
    Credentials,
}

/// Viewer is deliberately NOT here: it's the result of opening something on a
/// service, not a destination of its own. As a tab it would just be an empty box
/// until the user arrives from Projects.
pub(super) const TABS: [&str; 8] = [
    "Dashboard",
    "Hosts",
    "Maintenance",
    "Actions",
    "Monitor",
    "Domains",
    // This screen lists SERVICES across projects, not projects. It's still called
    // Screen::Projects in the code (a leftover from the old panel), but the label
    // must be honest.
    "Services",
    // Appended rather than slotted in next to Domains, where it belongs
    // logically: every other tab would shift a number, and the digit keys are
    // muscle memory that a new feature has no right to move.
    "Uptime",
];

/// The same tabs, shortened, for a terminal too narrow for the full words.
///
/// Only the two longest are cut, and each stays a word rather than becoming an
/// initial: "Maint" is still recognisable, "M" is a guess.
pub(super) const SHORT_TABS: [&str; 8] = [
    "Dash", "Hosts", "Maint", "Actions", "Monitor", "Domains", "Services", "Uptime",
];

/// The labels that FIT in `width`.
///
/// The eighth tab (Uptime, v0.65.0) pushed the full set past 80 columns, and the
/// bar was clipped at the frame — so the newest tab silently disappeared from the
/// one strip whose entire job is saying where you are and where you can go. A
/// shortened word beats a missing one.
pub(super) fn tabs_for(width: u16) -> &'static [&'static str; 8] {
    // Each label is padded by one space either side, with a single-column
    // separator between neighbours, and the bar sits inside a border.
    let needed = |tabs: &[&str]| -> u16 {
        (tabs.iter().map(|t| t.chars().count() + 2).sum::<usize>() + tabs.len() - 1) as u16
    };
    if needed(&TABS) + 2 <= width {
        &TABS
    } else {
        &SHORT_TABS
    }
}

/// Tab (by label order) → Screen, the inverse of Screen::index. For clicking a tab.
pub(super) const TAB_SCREENS: [Screen; 8] = [
    Screen::Dashboard,
    Screen::Hosts,
    Screen::Maintenance,
    Screen::Actions,
    Screen::Monitor,
    Screen::Domains,
    Screen::Projects,
    Screen::Uptime,
];

impl Screen {
    pub(super) fn index(self) -> usize {
        match self {
            Screen::Dashboard => 0,
            Screen::Hosts => 1,
            Screen::Maintenance => 2,
            Screen::Actions => 3,
            Screen::Monitor => 4,
            Screen::Domains => 5,
            Screen::Projects => 6,
            Screen::Uptime => 7,
            // Viewer & Terminal are always opened from Projects, so that tab stays
            // highlighted — neither has its own tab.
            Screen::Viewer | Screen::Terminal | Screen::Credentials => 6,
        }
    }
    pub(super) fn next(self) -> Self {
        match self {
            Screen::Dashboard => Screen::Hosts,
            Screen::Hosts => Screen::Maintenance,
            Screen::Maintenance => Screen::Actions,
            Screen::Actions => Screen::Monitor,
            Screen::Monitor => Screen::Domains,
            Screen::Domains => Screen::Projects,
            Screen::Projects => Screen::Uptime,
            Screen::Uptime => Screen::Dashboard,
            Screen::Viewer | Screen::Terminal | Screen::Credentials => Screen::Dashboard,
        }
    }
    /// The previous tab (for ←). Wraps through TAB_SCREENS; Viewer/Terminal count
    /// as being on the Projects tab (index 6).
    pub(super) fn prev(self) -> Self {
        let i = self.index();
        TAB_SCREENS[(i + TAB_SCREENS.len() - 1) % TAB_SCREENS.len()]
    }
}

/// The Credentials screen's state: a database service's connection identity, the
/// selected row, and whether the secret fields are currently revealed.
pub(super) struct CredsUi {
    pub(super) title: String,
    pub(super) items: Vec<crate::credentials::Cred>,
    pub(super) row: TableState,
    /// Secrets are masked until the operator asks — a password on screen is a
    /// deliberate act, not the default, in a tool that redacts everywhere else.
    pub(super) revealed: bool,
}

impl Default for CredsUi {
    fn default() -> Self {
        Self {
            title: "Credentials".into(),
            items: Vec::new(),
            row: TableState::default(),
            revealed: false,
        }
    }
}

/// The top-level workspace: which product this session is managing. ORTHOGONAL to
/// `Screen` (the EasyPanel tabs) — the Cloudflare workspace is a fully isolated
/// view, not another tab. Switched with `W`.
#[derive(PartialEq, Clone, Copy, Default)]
pub(super) enum Workspace {
    #[default]
    Easypanel,
    Cloudflare,
}

/// Which Cloudflare screen is showing. The workspace opens on the Zones home of
/// the active account; Records is a drill-in from a selected zone. Accounts are
/// switched through a picker overlay (like the EasyPanel server picker), not a
/// screen. Orthogonal to the EasyPanel `Screen`.
#[derive(PartialEq, Debug, Clone, Copy, Default)]
pub(super) enum CfScreen {
    #[default]
    Zones,
    Records,
    /// The R2 objects drill-in from a selected bucket — the mirror of Records for
    /// DNS. R2's buckets home is any non-`Objects` state (R2 dispatches on this).
    Objects,
    /// The Workers deployments/version-history drill-in from a selected Worker.
    WorkerDeployments,
    /// The Workers settings/configuration drill-in from a selected Worker.
    WorkerSettings,
    /// The Cloudflare Tunnel ingress/configuration drill-in from a selected Tunnel.
    TunnelConfig,
}

/// A Cloudflare product section, shown as a tab in the CF workspace. DNS (zones +
/// records) and R2 (buckets) today; D1/KV/Workers/Connectors slot in later. The
/// enum carries only the variants that exist — a speculative one would be dead
/// code — so growing it is: add a variant here plus one row to `CF_PRODUCTS`.
#[derive(PartialEq, Debug, Clone, Copy, Default)]
pub(super) enum CfProduct {
    Analytics,
    #[default]
    Dns,
    Tunnels,
    R2,
    Workers,
}

/// The product tab bar, in label order. The single list the tab bar renders and
/// the switch keys index into — adding a product is one row here.
pub(super) const CF_PRODUCTS: &[(&str, CfProduct)] = &[
    ("Analytics", CfProduct::Analytics),
    ("Domains", CfProduct::Dns),
    ("Tunnels", CfProduct::Tunnels),
    ("R2", CfProduct::R2),
    ("Workers", CfProduct::Workers),
];

impl CfProduct {
    /// This product's position in `CF_PRODUCTS` — i.e. the active tab index.
    pub(super) fn index(self) -> usize {
        CF_PRODUCTS
            .iter()
            .position(|&(_, p)| p == self)
            .unwrap_or(0)
    }
    /// The next product, wrapping — for `Tab` / `→`.
    pub(super) fn next(self) -> Self {
        CF_PRODUCTS[(self.index() + 1) % CF_PRODUCTS.len()].1
    }
    /// The previous product, wrapping — for `←`.
    pub(super) fn prev(self) -> Self {
        CF_PRODUCTS[(self.index() + CF_PRODUCTS.len() - 1) % CF_PRODUCTS.len()].1
    }
}

/// The Cloudflare workspace's state. Zones and Records reach the network through
/// the worker, carrying the active account's token IN the request. Filter/marks
/// are CF-local (never the EasyPanel ones), so the two workspaces stay isolated.
#[derive(Default)]
pub(super) struct CfUi {
    pub(super) screen: CfScreen,
    /// The active product tab. Analytics is tab 1, before DNS, matching Cloudflare's
    /// account-level dashboard hierarchy.
    pub(super) product: CfProduct,
    pub(super) accounts: Vec<CloudflareAccount>,
    /// The active account — the token for every zone/record request.
    pub(super) active: Option<CloudflareAccount>,
    pub(super) zones: Vec<Zone>,
    pub(super) zones_row: TableState,
    pub(super) records: Vec<Record>,
    pub(super) records_row: TableState,
    pub(super) current_zone: Option<Zone>,
    pub(super) web_analytics_sites: Vec<WebAnalyticsSite>,
    /// R2 product state — the account's buckets and the selected row. Account-scoped,
    /// loaded when the R2 tab is selected; the shared `filter`/`error` cover it too.
    pub(super) r2_buckets: Vec<R2Bucket>,
    pub(super) r2_row: TableState,
    /// R2 objects drill-in state (the mirror of `records`/`current_zone`): the browse
    /// level of `current_bucket` at `current_prefix`. `/`-delimited keys browse as a
    /// folder tree — `r2_folders` are the subfolders here (full key prefixes ending in
    /// `/`), `r2_objects` the files directly at this level. Rendered folders-first into a
    /// single table over `r2_objects_row`. Loaded via the REST objects API (delimiter=/).
    pub(super) r2_folders: Vec<String>,
    pub(super) r2_objects: Vec<R2Object>,
    pub(super) r2_objects_row: TableState,
    /// The path INSIDE the bucket currently browsed: "" at the root, or e.g.
    /// `assets/admin-front-end/css/` deeper. Enter on a folder appends its segment; Esc
    /// strips the last one. Drives the request prefix and the breadcrumb.
    pub(super) current_prefix: String,
    /// This level had more than one page; only the first is loaded, so the screen says
    /// "narrow with a filter" rather than pretending it's the whole level.
    pub(super) r2_truncated: bool,
    /// Account-level analytics summary (GraphQL). This is account-scoped and needs an
    /// account_id plus the Account Analytics:Read permission on the token.
    pub(super) analytics: Option<AnalyticsSummary>,
    pub(super) analytics_days: u16,
    /// Cloudflare Tunnel product state — account-scoped tunnels plus a drill-in
    /// showing the ingress/configuration rows for the selected tunnel.
    pub(super) tunnels: Vec<CloudflareTunnel>,
    pub(super) tunnels_row: TableState,
    pub(super) current_tunnel: Option<CloudflareTunnel>,
    pub(super) tunnel_config: Option<TunnelConfiguration>,
    pub(super) tunnel_config_row: TableState,
    /// Workers scripts are account-scoped like R2. They are rendered as their own product
    /// tab and mutate through explicit deploy/delete actions.
    pub(super) workers: Vec<WorkerScript>,
    pub(super) workers_row: TableState,
    pub(super) current_worker: Option<String>,
    pub(super) worker_deployments: Vec<WorkerDeployment>,
    pub(super) worker_deployments_row: TableState,
    pub(super) worker_settings: Option<WorkerSettingsBundle>,
    pub(super) worker_settings_row: TableState,
    pub(super) current_bucket: Option<String>,
    /// A CF-local text filter, narrowing the loaded list client-side.
    pub(super) filter: String,
    pub(super) filter_input: bool,
    /// The last fetch error, kept apart from an empty result so the screen can
    /// tell "no records" from "the fetch failed".
    pub(super) error: Option<String>,
    /// Record ids marked for a bulk action.
    pub(super) marked: HashSet<String>,
}

/// Loading / empty / error / ready — the state a CF list is in, chosen from the
/// (busy, error, is_empty) triple so the render never draws "No records" over a
/// fetch that actually failed, or an empty list while it is still loading.
#[derive(PartialEq, Debug)]
pub(super) enum CfListState {
    Loading,
    Error,
    Empty,
    Ready,
}

/// A list is Loading while a request is in flight and nothing has arrived yet;
/// Error once a fetch has failed; Empty only on a successful empty result.
pub(super) fn cf_list_state(busy: bool, error: bool, is_empty: bool) -> CfListState {
    if !is_empty {
        CfListState::Ready
    } else if busy {
        CfListState::Loading
    } else if error {
        CfListState::Error
    } else {
        CfListState::Empty
    }
}

/// The R2 objects whose key contains `needle` (case-insensitive). An empty needle
/// keeps everything — narrows the already-loaded page client-side, like `filter_records`.
pub(super) fn filter_objects<'a>(objects: &'a [R2Object], needle: &str) -> Vec<&'a R2Object> {
    let n = needle.to_ascii_lowercase();
    objects
        .iter()
        .filter(|o| n.is_empty() || o.key.to_ascii_lowercase().contains(&n))
        .collect()
}

/// The R2 subfolders whose next-segment name (the prefix stripped of `current`) contains
/// `needle` (case-insensitive). An empty needle keeps everything — the same client-side
/// narrowing as `filter_objects`, matching what the user sees (the segment, not the full
/// key prefix).
pub(super) fn filter_folders<'a>(
    folders: &'a [String],
    current: &str,
    needle: &str,
) -> Vec<&'a String> {
    let n = needle.to_ascii_lowercase();
    folders
        .iter()
        .filter(|f| {
            n.is_empty()
                || f.strip_prefix(current)
                    .unwrap_or(f)
                    .to_ascii_lowercase()
                    .contains(&n)
        })
        .collect()
}

/// The parent of an R2 browse prefix: `assets/css/` → `assets/`, `assets/` → "" (root).
/// Strips the trailing `/`, then everything after the previous `/`. Drives Esc "go up".
pub(super) fn parent_prefix(prefix: &str) -> String {
    let trimmed = prefix.strip_suffix('/').unwrap_or(prefix);
    match trimmed.rfind('/') {
        Some(i) => trimmed[..=i].to_string(),
        None => String::new(),
    }
}

/// Build the patch an edit form describes. `content` is always sent (the field is
/// prefilled from the record); `proxied` only for proxyable types, `priority` only
/// for MX — the same gating `record_body` applies on create.
pub(super) fn cf_record_patch(
    kind: &str,
    content: &str,
    ttl: &str,
    proxied: bool,
    priority: &str,
) -> RecordPatch {
    RecordPatch {
        content: (!content.is_empty()).then(|| content.to_string()),
        ttl: ttl.trim().parse().ok(),
        proxied: proxyable(kind).then_some(proxied),
        priority: if kind.eq_ignore_ascii_case("MX") {
            priority.trim().parse().ok()
        } else {
            None
        },
    }
}

fn tunnel_origin_advanced_json(origin: Option<&Value>) -> String {
    let Some(Value::Object(object)) = origin else {
        return String::new();
    };
    let mut advanced = object.clone();
    advanced.remove("noTLSVerify");
    if advanced.is_empty() {
        String::new()
    } else {
        Value::Object(advanced).to_string()
    }
}

fn tunnel_origin_request_from_form(form: &Form) -> anyhow::Result<Option<Value>> {
    let no_tls_verify = form.by_label("No TLS verify") == "yes";
    let mut value = parse_tunnel_origin_request(&form.by_label("Advanced origin JSON"))?;
    let mut object = match value.take() {
        Some(Value::Object(map)) => map,
        Some(_) => unreachable!("parse_tunnel_origin_request only returns JSON objects"),
        None => serde_json::Map::new(),
    };
    if no_tls_verify {
        object.insert("noTLSVerify".into(), Value::Bool(true));
    } else {
        object.remove("noTLSVerify");
    }
    if object.is_empty() {
        Ok(None)
    } else {
        Ok(Some(Value::Object(object)))
    }
}

/// A Cloudflare account-list change: executed in event_loop, which holds the
/// CloudflareConfig. Same shape as ServerAction — the App never writes the file.
pub(super) enum CfAction {
    Add(crate::cloudflare::CloudflareAccount),
    Save {
        rename_from: Option<String>,
        account: crate::cloudflare::CloudflareAccount,
    },
    SetDefault(String),
    Remove(String),
}

/// One row on the Hosts screen. A dead host must show as an error row, not fail
/// the whole table.
pub(super) struct HostRow {
    pub(super) name: String,
    pub(super) url: String,
    pub(super) state: HostState,
}

pub(super) enum HostState {
    Loading,
    Ok(Box<Value>),
    Err(String),
}

/// A sub-tab on the Monitor screen (following the panel).
#[derive(PartialEq, Clone, Copy)]
pub(super) enum MonitorView {
    Services,
    Storage,
}

pub(super) struct Confirm {
    pub(super) action: String,
    pub(super) project: String,
    pub(super) service: String,
    pub(super) stype: String,
    pub(super) label: String,
}

/// A cross-host compare waiting for its target token. Same reason as MigrateReq:
/// the App knows a server's name and url, never its token.
pub(super) struct DiffProjectAcrossReq {
    pub(super) project: String,
    pub(super) target_server: String,
}

pub(super) struct DiffAcrossReq {
    /// The service on THIS host: (project, service, type).
    pub(super) local: (String, String, String),
    /// The other server to fetch the same project/service from.
    pub(super) target_server: String,
}

/// A migration waiting for its destination token, which only event_loop can look
/// up (the App knows each server's name and url, never its token).
pub(super) struct MigrateReq {
    pub(super) target_server: String,
    pub(super) target_project: String,
    /// (project, service, type) — one entry for a service, many for a project.
    pub(super) services: Vec<(String, String, String)>,
}

/// Does this status line report a failure?
///
/// ONE definition, because two consumers must agree: `render` colours it, and the
/// event loop refuses to fade it. They used to each carry their own copy of the
/// rule, so a message could be painted as an error and then quietly erased as if
/// it were a routine notice.
pub(super) fn status_is_error(status: &str) -> bool {
    // ⚠ counts too. A clone can SUCCEED and still leave the user something they
    // must act on — a config file held back so the database can initialise. That
    // is not an error, but a message which fades after a few seconds is no way to
    // deliver it.
    status.starts_with("Error") || status.contains("failed") || status.contains('⚠')
}

/// A server-list change: executed in event_loop, which holds the ServerConfig.
/// A change to the uptime watchlist, applied by the event loop.
pub(super) enum WatchAction {
    Put(crate::uptime::Check),
    Remove(String),
}

pub(super) enum ServerAction {
    Save {
        /// The name it is stored under today. `Some` only when the edit form
        /// changed it, which makes the save a rename as well.
        rename_from: Option<String>,
        name: String,
        url: String,
        /// None = keep the stored token (an edit form left blank).
        token: Option<String>,
    },
    Remove(String),
}

pub(super) struct App {
    pub(super) server_name: String,
    /// (name, url) for each server. The URL is stored too so the edit form can be
    /// prefilled with the current value, not left blank like the add form.
    pub(super) all_servers: Vec<(String, String)>,
    pub(super) switch_to: Option<String>,
    pub(super) picker: Option<ListState>,
    pub(super) form: Option<Form>,
    pub(super) chooser: Option<Chooser>,
    pub(super) server_action: Option<ServerAction>,
    /// Set by the migrate form; event_loop resolves the destination token and
    /// hands the work to the worker.
    pub(super) migrate_req: Option<MigrateReq>,
    /// A cross-host compare waiting for the event loop to resolve the target
    /// host's token (which only the ServerConfig holds).
    pub(super) diff_across_req: Option<DiffAcrossReq>,
    pub(super) diff_project_across_req: Option<DiffProjectAcrossReq>,
    /// A pending "read the backups on another server": (server name, project,
    /// service). Resolved to a url+token by event_loop, which alone holds them.
    pub(super) restore_from_req: Option<(String, String, String)>,
    /// Shared with the worker's user lane: non-zero while a request the user asked
    /// for is still running. Owned as an Arc so the worker can clear it from its
    /// own thread the instant the work ends.
    pub(super) busy: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    /// (project, service, stype, replace) — awaiting an env edit in $EDITOR.
    /// `replace` = true opens an EMPTY editor (quick-replace: paste new env without
    /// waiting for a fetch or deleting the old one); false loads the current env.
    pub(super) edit_env: Option<(String, String, String)>,
    /// The project whose shared env is about to be opened in $EDITOR.
    pub(super) edit_project_env: Option<String>,
    /// (project, service, stype) — awaiting a Config File (Advanced db) edit in
    /// $EDITOR; its contents come from inspectService and are saved via updateAdvanced.
    pub(super) edit_config: Option<(String, String, String)>,
    /// The index of the form field awaiting an $EDITOR open; event_loop does it —
    /// only it holds the terminal.
    pub(super) edit_field: Option<usize>,
    /// (project, service) awaiting a container terminal; event_loop connects it (it
    /// holds the ServerConfig).
    /// (project, service, db) — a request to open a container terminal. `db` =
    /// Some(stype) for a database shell (mysql/mariadb, auto root login), None for
    /// a plain shell (sh). event_loop connects it.
    pub(super) terminal_req: Option<(String, String, Option<String>)>,
    /// (project, service, stype) awaiting a DB credentials view; event_loop
    /// inspects the service (it holds the ServerConfig) and fills `creds`.
    pub(super) credentials_req: Option<(String, String, String)>,
    /// The database credentials currently on the Credentials screen.
    pub(super) creds: CredsUi,
    /// Text the event_loop should put on the system clipboard (via OSC 52) on its
    /// next pass, then clear. Set by the copy action; the loop owns the terminal.
    pub(super) clipboard: Option<String>,
    /// The active container-terminal session (emulator + input channel + title).
    pub(super) term: super::terminal::TermUi,

    pub(super) screen: Screen,
    /// The active workspace (EasyPanel tabs vs the isolated Cloudflare view).
    /// Orthogonal to `screen`: switching it hides every EasyPanel key and pane.
    pub(super) workspace: Workspace,
    /// The Cloudflare account screen's state (accounts + selection).
    pub(super) cf: CfUi,
    /// The account picker overlay (mirrors the server `s` picker). Some = open.
    pub(super) cf_picker: Option<ListState>,
    /// A pending Cloudflare account-list change, resolved by event_loop (which alone
    /// holds the CloudflareConfig file), then re-seeded into `cf.accounts`.
    pub(super) cf_action: Option<CfAction>,
    pub(super) should_quit: bool,
    pub(super) refresh_inflight: bool,
    pub(super) status: String,

    pub(super) stats: Option<Value>,
    /// Set when the system-stats fetch failed, so the Dashboard says "couldn't
    /// load" instead of drawing 0.0% gauges. Cleared by the next successful load.
    pub(super) stats_error: Option<String>,
    pub(super) nodes: Vec<Value>,

    pub(super) actions: Vec<Value>,
    pub(super) actions_state: TableState,
    /// On the Actions tab: show only the rows that did NOT finish cleanly.
    /// Finding a failure by typing into the text filter also matched commit
    /// messages, so searching "error" returned successful deploys whose message
    /// contained the word — the opposite of the point.
    pub(super) actions_failures_only: bool,
    pub(super) monitor: Vec<Value>,
    pub(super) monitor_state: TableState,
    /// Swarm replicas per service (actual/desired), keyed by "{project}_{service}".
    /// The source of the "down" status in the Services table. Empty = not loaded yet.
    pub(super) task_stats: HashMap<String, (i64, i64)>,
    pub(super) storage: Vec<Value>,
    pub(super) monitor_view: MonitorView,
    pub(super) domains: Vec<Value>,
    pub(super) domains_state: TableState,
    /// Set when the last domain fetch FAILED, so the empty screen can say "couldn't
    /// load" instead of "no domains yet". Cleared by the next successful load.
    pub(super) domains_error: Option<String>,
    /// The (project, service) origin when entering the Domains tab via `o` from a
    /// service — used to prefill the "New domain" form to that service. None = the
    /// Domains tab was opened normally.
    pub(super) domain_scope: Option<(String, String)>,
    /// A previewed bulk rewrite, waiting for the user to accept it. Empty means
    /// there is nothing armed — so Enter in the viewer cannot fire a rewrite the
    /// user has already walked away from.
    pub(super) domain_edits: Vec<crate::domains::Change>,
    /// The domains enrolled for uptime checks on THIS server, as loaded from
    /// checks.json. Only what the operator chose — never the whole domain list.
    pub(super) watch: Vec<crate::uptime::Check>,
    /// The last answers, one per URL that has been checked this session.
    pub(super) probes: Vec<crate::uptime::Probe>,
    pub(super) uptime_state: TableState,
    /// A pending change to the watchlist FILE. The event loop owns every path on
    /// disk (as it does for servers.json), so the App never writes one itself —
    /// which also keeps its tests from touching the user's real config.
    pub(super) watch_action: Option<WatchAction>,
    /// A check run is in flight; the screen says so rather than looking idle
    /// while twenty requests are out.
    pub(super) checking: bool,

    pub(super) projects: Vec<String>,
    /// All services across projects. A flat list replaces the project -> service
    /// hierarchy: drill-down can't be searched and collapses under hundreds of
    /// services.
    pub(super) all_services: Vec<Value>,
    /// Set when the service list failed to load, so an empty Services table reads
    /// as "couldn't load" rather than "this host has nothing". Cleared on success.
    pub(super) services_error: Option<String>,
    pub(super) services_table: TableState,

    /// The full-screen viewer's state — text, scroll, what it was opened from and
    /// about, the live-log cursor. Ten loose fields, folded into one struct next
    /// to their formatting (see `super::viewer::ViewerUi`).
    pub(super) viewer: super::viewer::ViewerUi,

    /// Everything the backup and restore screens hold — the pickers, the ticks,
    /// and the panel's storage providers. Ten fields for one feature had spread
    /// through this struct among the tabs, the filter and the terminal.
    pub(super) backups: BackupUi,

    /// Services marked for a bulk action, as (project, service).
    ///
    /// The service TYPE is deliberately not stored: it is looked up at dispatch
    /// time, so a mark can never carry a stale group into the API call. A mark
    /// for a service that has since disappeared simply finds nothing and is
    /// dropped — see `bulk_targets`.
    pub(super) marked: HashSet<(String, String)>,

    /// The filter text for the active screen's table ("" = no filter).
    pub(super) filter: String,
    /// Currently typing a filter (keys go to the filter, not to the screen).
    pub(super) filter_input: bool,
    /// The help overlay is open.
    pub(super) help: bool,
    /// Scroll offset of the help overlay. The help is longer than a short terminal,
    /// and silently hiding half of it is worse than no help at all.
    pub(super) help_scroll: u16,
    /// The Maintenance tab info rows: (label, value).
    pub(super) maint: Vec<(String, Result<String, String>)>,
    pub(super) hosts: Vec<HostRow>,
    pub(super) hosts_state: TableState,
    /// Set when the Hosts screen needs data; its fan-out is run by event_loop.
    pub(super) load_hosts: bool,

    pub(super) confirm: Option<Confirm>,

    // ---- Animation & mouse ----
    /// The global animation clock; the spinner/pulse phase is computed from its elapsed.
    pub(super) anim: Instant,
    /// When the Services table selection last moved (selection flash).
    pub(super) nav_at: Instant,
    /// When the tab last changed (tab flash).
    pub(super) tab_at: Instant,
    /// When the CF product tab last changed (its own tab flash).
    pub(super) cf_product_at: Instant,
    /// Comparators to detect a tab/selection change without hooking every handler.
    pub(super) last_screen: Screen,
    pub(super) last_cf_product: CfProduct,
    pub(super) last_sel: Option<usize>,
    /// Per-tab click hitboxes (start,end column), filled in during render_tabs. Plus its row.
    pub(super) tab_spans: Vec<(u16, u16)>,
    pub(super) tab_row: u16,
    /// Cloudflare product-tab click hitboxes, filled in during cf_header.
    pub(super) cf_product_spans: Vec<(u16, u16)>,
    pub(super) cf_product_row: u16,
    /// The active screen's table area, filled in during render — maps a click to a
    /// row. Only one screen renders per frame, so one field covers every table.
    pub(super) table_area: Rect,
    /// The context menu (right click). Each item = (label, action).
    pub(super) menu: Option<Menu>,
    /// The command palette (global search) — quick navigation to a service/tab.
    pub(super) palette: Option<Palette>,
}

impl App {
    pub(super) fn new(server_name: String, all_servers: Vec<(String, String)>) -> Self {
        Self {
            server_name,
            all_servers,
            switch_to: None,
            picker: None,
            form: None,
            chooser: None,
            server_action: None,
            migrate_req: None,
            diff_across_req: None,
            diff_project_across_req: None,
            restore_from_req: None,
            busy: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            edit_env: None,
            edit_project_env: None,
            edit_config: None,
            edit_field: None,
            terminal_req: None,
            credentials_req: None,
            creds: CredsUi::default(),
            clipboard: None,
            term: super::terminal::TermUi::default(),
            screen: Screen::Dashboard,
            workspace: Workspace::default(),
            cf: CfUi::default(),
            cf_picker: None,
            cf_action: None,
            should_quit: false,
            refresh_inflight: false,
            status: "Ready".into(),
            stats: None,
            stats_error: None,
            nodes: Vec::new(),
            actions: Vec::new(),
            actions_state: TableState::default(),
            actions_failures_only: false,
            monitor: Vec::new(),
            task_stats: HashMap::new(),
            monitor_state: TableState::default(),
            storage: Vec::new(),
            monitor_view: MonitorView::Services,
            domains: Vec::new(),
            domains_state: TableState::default(),
            domains_error: None,
            domain_scope: None,
            domain_edits: Vec::new(),
            watch: Vec::new(),
            probes: Vec::new(),
            uptime_state: TableState::default(),
            watch_action: None,
            checking: false,
            projects: Vec::new(),
            all_services: Vec::new(),
            services_error: None,
            services_table: TableState::default(),
            viewer: super::viewer::ViewerUi::default(),
            backups: BackupUi::default(),
            marked: HashSet::new(),
            filter: String::new(),
            filter_input: false,
            help: false,
            help_scroll: 0,
            maint: Vec::new(),
            hosts: Vec::new(),
            hosts_state: TableState::default(),
            load_hosts: false,
            confirm: None,
            anim: Instant::now(),
            nav_at: Instant::now(),
            tab_at: Instant::now(),
            cf_product_at: Instant::now(),
            last_screen: Screen::Dashboard,
            last_cf_product: CfProduct::Dns,
            last_sel: None,
            tab_spans: Vec::new(),
            tab_row: 0,
            cf_product_spans: Vec::new(),
            cf_product_row: 0,
            table_area: Rect::default(),
            menu: None,
            palette: None,
        }
    }

    /// The number of rows CURRENTLY rendered in the active screen's table (after
    /// filtering). Used by clicks: the clicked index must be within the range
    /// actually on screen.
    pub(super) fn visible_table_len(&self) -> usize {
        // In the Cloudflare workspace the active table is the CF list, not the
        // (hidden) EasyPanel screen behind it — so the mouse layer measures the
        // filtered CF list under the cursor.
        if self.workspace == Workspace::Cloudflare {
            return match self.cf.product {
                CfProduct::Analytics => 0,
                CfProduct::Workers => match self.cf.screen {
                    CfScreen::WorkerDeployments => self.cf_worker_deployments_shown().len(),
                    CfScreen::WorkerSettings => self.cf_worker_settings_shown().len(),
                    _ => self.cf_workers_shown().len(),
                },
                CfProduct::Tunnels => match self.cf.screen {
                    CfScreen::TunnelConfig => self.cf_tunnel_config_rows_shown().len(),
                    _ => self.cf_tunnels_shown().len(),
                },
                CfProduct::R2 => match self.cf.screen {
                    CfScreen::Objects => self.cf_level_len(),
                    _ => self.cf_buckets_shown().len(),
                },
                CfProduct::Dns => match self.cf.screen {
                    CfScreen::Zones => self.cf_zones_shown().len(),
                    CfScreen::Records
                    | CfScreen::Objects
                    | CfScreen::WorkerDeployments
                    | CfScreen::WorkerSettings
                    | CfScreen::TunnelConfig => self.cf_records_shown().len(),
                },
            };
        }
        match self.screen {
            Screen::Projects => self.visible_rows().len(),
            Screen::Actions => self.visible_actions().len(),
            Screen::Domains => self.visible_domains().len(),
            Screen::Hosts => self.hosts.len(),
            Screen::Monitor => self.monitor_rows_shown(),
            _ => 0,
        }
    }

    /// The active screen's table TableState (to select a row from a click). None =
    /// a screen with no selectable table.
    pub(super) fn active_table(&mut self) -> Option<&mut TableState> {
        // In the Cloudflare workspace the mouse drives the CF list (zones/records)
        // rather than the hidden EasyPanel screen, so scroll/hover/click land on the
        // CF row state.
        if self.workspace == Workspace::Cloudflare {
            return match self.cf.product {
                CfProduct::Analytics => None,
                CfProduct::Workers => match self.cf.screen {
                    CfScreen::WorkerDeployments => Some(&mut self.cf.worker_deployments_row),
                    CfScreen::WorkerSettings => Some(&mut self.cf.worker_settings_row),
                    _ => Some(&mut self.cf.workers_row),
                },
                CfProduct::Tunnels => match self.cf.screen {
                    CfScreen::TunnelConfig => Some(&mut self.cf.tunnel_config_row),
                    _ => Some(&mut self.cf.tunnels_row),
                },
                CfProduct::R2 => match self.cf.screen {
                    CfScreen::Objects => Some(&mut self.cf.r2_objects_row),
                    _ => Some(&mut self.cf.r2_row),
                },
                CfProduct::Dns => match self.cf.screen {
                    CfScreen::Zones => Some(&mut self.cf.zones_row),
                    CfScreen::Records
                    | CfScreen::Objects
                    | CfScreen::WorkerDeployments
                    | CfScreen::WorkerSettings
                    | CfScreen::TunnelConfig => Some(&mut self.cf.records_row),
                },
            };
        }
        match self.screen {
            Screen::Projects => Some(&mut self.services_table),
            Screen::Actions => Some(&mut self.actions_state),
            Screen::Domains => Some(&mut self.domains_state),
            Screen::Hosts => Some(&mut self.hosts_state),
            Screen::Monitor => Some(&mut self.monitor_state),
            _ => None,
        }
    }

    /// Edit the selected database service's Config File (Advanced) in $EDITOR.
    /// event_loop fetches its contents, opens the editor, then saves.
    pub(super) fn start_config_edit(&mut self) {
        match self.selected_row() {
            Some((p, s, t))
                if matches!(
                    t.as_str(),
                    "mysql" | "mariadb" | "postgres" | "mongo" | "redis"
                ) =>
            {
                self.edit_config = Some((p, s, t));
            }
            Some((_, _, t)) => {
                self.status = format!("Config file is only for database services (this is {t})");
            }
            None => self.status = "Select a service first".into(),
        }
    }

    /// Open a shell terminal into the selected service's container (event_loop takes
    /// over the terminal). None = a plain shell.
    pub(super) fn start_terminal(&mut self) {
        match self.selected_row() {
            Some((project, service, _)) => self.terminal_req = Some((project, service, None)),
            None => self.status = "Select a service first".into(),
        }
    }

    /// A database shell with auto login (mysql/mariadb/postgres/mongo/redis).
    pub(super) fn start_db_shell(&mut self) {
        match self.selected_row() {
            Some((project, service, stype))
                if matches!(
                    stype.as_str(),
                    "mysql" | "mariadb" | "postgres" | "mongo" | "redis"
                ) =>
            {
                self.terminal_req = Some((project, service, Some(stype)));
            }
            Some((_, _, stype)) => {
                self.status = format!("DB shell is only for database services (this is {stype})");
            }
            None => self.status = "Select a service first".into(),
        }
    }

    /// Show a database service's stored credentials (user, password, host, port,
    /// connection URL). event_loop inspects the service and fills `creds`.
    pub(super) fn start_credentials(&mut self) {
        match self.selected_row() {
            Some((project, service, stype))
                if matches!(
                    stype.as_str(),
                    "mysql" | "mariadb" | "postgres" | "mongo" | "redis"
                ) =>
            {
                self.credentials_req = Some((project, service, stype));
                self.status = "Reading credentials...".into();
            }
            Some((_, _, stype)) => {
                self.status =
                    format!("Credentials are only for database services (this is {stype})");
            }
            None => self.status = "Select a service first".into(),
        }
    }

    /// The id of the highlighted action (from the shown list, honoring the filter).
    /// None = nothing selected.
    pub(super) fn selected_action_id(&self) -> Option<String> {
        self.actions_state
            .selected()
            .and_then(|i| self.visible_actions().get(i).map(|a| field(a, "/id")))
    }

    /// Detect a tab/selection change (called each frame before draw) to trigger the
    /// transition flash — so there's no need to stamp a timestamp in every nav handler.
    pub(super) fn tick_anim(&mut self) {
        if self.screen != self.last_screen {
            self.last_screen = self.screen;
            self.tab_at = Instant::now();
        }
        if self.cf.product != self.last_cf_product {
            self.last_cf_product = self.cf.product;
            self.cf_product_at = Instant::now();
        }
        let sel = self.services_table.selected();
        if sel != self.last_sel {
            self.last_sel = sel;
            self.nav_at = Instant::now();
        }
    }

    /// The spinner frame while an operation is running (status ends with "..."),
    /// else None.
    pub(super) fn status_is_error(&self) -> bool {
        status_is_error(&self.status)
    }

    /// How many user-initiated requests are still in flight.
    pub(super) fn busy(&self) -> usize {
        self.busy.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// The words to put on the status line.
    ///
    /// "Ready" is the resting message, so it must not sit next to a running
    /// spinner claiming the tool is idle while it waits on the server — which is
    /// exactly what the first paint does while the initial load is in flight.
    pub(super) fn status_line(&self) -> &str {
        if self.busy() > 0 && self.status == "Ready" {
            "Loading…"
        } else {
            &self.status
        }
    }

    pub(super) fn spinner(&self) -> Option<char> {
        const F: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
        // Driven by real in-flight work, not by the message ending in "...". The
        // text was only ever a guess: it kept spinning after a reply had come back
        // and stopped the moment an unrelated message replaced it.
        (self.busy() > 0).then(|| F[((self.anim.elapsed().as_millis() / 90) % 10) as usize])
    }

    /// Is any animation active? Used by the event loop to tighten redraws (smoother)
    /// only when needed, so idle stays cheap.
    pub(super) fn animating(&self) -> bool {
        self.spinner().is_some()
            || self.down_count() > 0
            || self.nav_at.elapsed().as_millis() < 260
            || self.tab_at.elapsed().as_millis() < 320
    }

    pub(super) fn reset_for_server(&mut self, name: String) {
        self.server_name = name;
        self.status = "Switching server".into();
        // Keep the active screen — switching servers must not throw the user back to
        // the Dashboard. Derived screens (Viewer/Terminal) hold the old server's
        // content, so fall back to Services.
        if matches!(
            self.screen,
            Screen::Viewer | Screen::Terminal | Screen::Credentials
        ) {
            self.screen = Screen::Projects;
        }
        self.term.input = None;
        self.term.parser = None;
        self.stats = None;
        self.stats_error = None;
        self.nodes.clear();
        self.actions.clear();
        self.actions_state = TableState::default();
        self.monitor.clear();
        self.monitor_state = TableState::default();
        self.storage.clear();
        self.domains.clear();
        self.domains_state = TableState::default();
        self.domains_error = None;
        self.projects.clear();
        self.all_services.clear();
        self.services_error = None;
        self.services_table = TableState::default();
        self.viewer.lines.clear();
        self.viewer.ctx = None;
    }

    /// A full-screen sub-view opened from a list (Viewer, Credentials) whose Esc
    /// means "back to that list". These handle Esc themselves, so the global
    /// filter/marks Esc guards must step aside and let the keypress reach them.
    /// Terminal is absent on purpose: its keystrokes go straight to the shell and
    /// never reach this dispatch.
    pub(super) fn screen_owns_esc(&self) -> bool {
        matches!(self.screen, Screen::Viewer | Screen::Credentials)
    }

    /// Switch the top-level workspace. Entering Cloudflare lands on the Zones home
    /// of the active account (defaulting to the config default); leaving it returns
    /// to the EasyPanel tabs. The network load happens in `enter_cloudflare`, which
    /// has the request channel — this only sets the state.
    pub(super) fn set_workspace(&mut self, ws: Workspace) {
        self.workspace = ws;
        match ws {
            Workspace::Cloudflare => {
                self.cf.screen = CfScreen::Zones;
                if self.cf.active.is_none() {
                    self.cf.active = self.cf_default_account();
                }
                self.status = "Cloudflare workspace".into();
            }
            Workspace::Easypanel => self.status = "EasyPanel workspace".into(),
        }
    }

    /// No Cloudflare account is configured — the empty state.
    pub(super) fn cf_empty(&self) -> bool {
        self.cf.accounts.is_empty()
    }

    /// The add-account form. All local: on submit the account is written to
    /// cloudflare.json by the event loop, never over the network.
    pub(super) fn open_cf_account_form(&mut self) {
        let fields = vec![
            Field::text("Name", ""),
            Field::secret("API token"),
            Field::text("Account ID", ""),
        ];
        self.form = Some(
            Form::new(FormKind::CfAccountAdd, " New Cloudflare account ", fields)
                .with_note("Stored locally in cloudflare.json (0600). No network call."),
        );
    }

    /// Edit a stored Cloudflare account from the account picker. This mirrors the server
    /// picker: fixing a token or adding an account-id should not require delete + add.
    pub(super) fn open_cf_account_edit_form(&mut self) {
        let Some(acc) = self.cf_picker_selected().or_else(|| self.cf.active.clone()) else {
            self.status = "No Cloudflare account selected".into();
            return;
        };
        let fields = vec![
            Field::text("Name", &acc.name),
            Field::secret_val("API token", &acc.api_token),
            Field::text("Account ID", acc.account_id.as_deref().unwrap_or("")),
        ];
        self.form = Some(
            Form::new(
                FormKind::CfAccountEdit {
                    name: acc.name.clone(),
                },
                format!(" Edit Cloudflare account {} ", acc.name),
                fields,
            )
            .with_note("Updates local cloudflare.json. The token stays masked on screen."),
        );
    }

    // ---------- Cloudflare zones & records ----------

    /// The active account's token, or None before an account was opened.
    pub(super) fn cf_token(&self) -> Option<String> {
        self.cf.active.as_ref().map(|a| a.api_token.clone())
    }

    /// The zones shown right now (after the CF-local filter).
    pub(super) fn cf_zones_shown(&self) -> Vec<&Zone> {
        filter_zones(&self.cf.zones, &self.cf.filter)
    }

    /// The records shown right now (after the CF-local filter).
    pub(super) fn cf_records_shown(&self) -> Vec<&Record> {
        filter_records(&self.cf.records, &self.cf.filter)
    }

    /// The highlighted zone, indexing into the FILTERED list (what's on screen).
    pub(super) fn selected_cf_zone(&self) -> Option<Zone> {
        let shown = self.cf_zones_shown();
        self.cf
            .zones_row
            .selected()
            .and_then(|i| shown.get(i))
            .map(|z| (*z).clone())
    }

    /// The highlighted record, indexing into the FILTERED list.
    pub(super) fn selected_cf_record(&self) -> Option<Record> {
        let shown = self.cf_records_shown();
        self.cf
            .records_row
            .selected()
            .and_then(|i| shown.get(i))
            .map(|r| (*r).clone())
    }

    /// The Web Analytics site associated with a zone, matching Cloudflare's ruleset
    /// zone_name first and falling back to the rule host. Used by the Domains table.
    pub(super) fn cf_web_analytics_for_zone(&self, zone: &Zone) -> Option<&WebAnalyticsSite> {
        self.cf
            .web_analytics_sites
            .iter()
            .find(|s| s.zone_name == zone.name || s.host == zone.name)
    }

    /// The Cloudflare Tunnels shown right now (after the CF-local filter).
    pub(super) fn cf_tunnels_shown(&self) -> Vec<&CloudflareTunnel> {
        filter_tunnels(&self.cf.tunnels, &self.cf.filter)
    }

    /// The highlighted tunnel, indexing into the FILTERED list.
    pub(super) fn selected_cf_tunnel(&self) -> Option<CloudflareTunnel> {
        let shown = self.cf_tunnels_shown();
        self.cf
            .tunnels_row
            .selected()
            .and_then(|i| shown.get(i))
            .map(|t| (*t).clone())
    }

    /// The selected tunnel's ingress/configuration rows shown right now.
    pub(super) fn cf_tunnel_config_rows_shown(&self) -> Vec<TunnelConfigRow> {
        let Some(config) = &self.cf.tunnel_config else {
            return Vec::new();
        };
        filter_tunnel_config_rows(&config.rows(), &self.cf.filter)
    }

    /// The highlighted real ingress rule, mapped back from the filtered display row.
    pub(super) fn selected_cf_tunnel_route(&self) -> Option<TunnelIngressRule> {
        let shown = self.cf_tunnel_config_rows_shown();
        let row = self
            .cf
            .tunnel_config_row
            .selected()
            .and_then(|i| shown.get(i))?;
        self.cf
            .tunnel_config
            .as_ref()?
            .config
            .ingress
            .iter()
            .find(|rule| {
                let service = if rule.service.trim().is_empty() {
                    "-"
                } else {
                    rule.service.as_str()
                };
                rule.hostname_label() == row.hostname && service == row.service
            })
            .cloned()
    }

    /// The R2 buckets shown right now (after the CF-local filter).
    pub(super) fn cf_buckets_shown(&self) -> Vec<&R2Bucket> {
        filter_buckets(&self.cf.r2_buckets, &self.cf.filter)
    }

    /// The highlighted bucket, indexing into the FILTERED list (what's on screen).
    pub(super) fn selected_cf_bucket(&self) -> Option<R2Bucket> {
        let shown = self.cf_buckets_shown();
        self.cf
            .r2_row
            .selected()
            .and_then(|i| shown.get(i))
            .map(|b| (*b).clone())
    }

    /// The Worker scripts shown right now (after the CF-local filter).
    pub(super) fn cf_workers_shown(&self) -> Vec<&WorkerScript> {
        filter_workers(&self.cf.workers, &self.cf.filter)
    }

    /// The highlighted Worker script, indexing into the FILTERED list.
    pub(super) fn selected_cf_worker(&self) -> Option<WorkerScript> {
        let shown = self.cf_workers_shown();
        self.cf
            .workers_row
            .selected()
            .and_then(|i| shown.get(i))
            .map(|w| (*w).clone())
    }

    /// The Worker deployments shown right now (after the CF-local filter).
    pub(super) fn cf_worker_deployments_shown(&self) -> Vec<&WorkerDeployment> {
        filter_worker_deployments(&self.cf.worker_deployments, &self.cf.filter)
    }

    /// The Worker settings rows shown right now (after the CF-local filter).
    pub(super) fn cf_worker_settings_shown(&self) -> Vec<WorkerSettingsRow> {
        let Some(settings) = &self.cf.worker_settings else {
            return Vec::new();
        };
        let worker = self
            .cf
            .current_worker
            .as_ref()
            .and_then(|name| self.cf.workers.iter().find(|w| &w.id == name))
            .cloned()
            .unwrap_or_else(|| WorkerScript {
                id: self.cf.current_worker.clone().unwrap_or_default(),
                ..Default::default()
            });
        filter_worker_settings_rows(&settings.rows(&worker), &self.cf.filter)
    }

    /// The active account's account-id, which every R2 call needs (R2 is
    /// account-scoped — unlike DNS, which can list zones without one).
    pub(super) fn cf_account_id(&self) -> Option<String> {
        self.cf.active.as_ref().and_then(|a| a.account_id.clone())
    }

    /// The subfolders shown right now (after the CF-local filter). Rendered ABOVE the
    /// files, so a row index below this count is a folder.
    pub(super) fn cf_folders_shown(&self) -> Vec<&String> {
        filter_folders(
            &self.cf.r2_folders,
            &self.cf.current_prefix,
            &self.cf.filter,
        )
    }

    /// The files shown right now (after the CF-local filter).
    pub(super) fn cf_objects_shown(&self) -> Vec<&R2Object> {
        filter_objects(&self.cf.r2_objects, &self.cf.filter)
    }

    /// The FILE under the cursor, or None when the selected row is a FOLDER (or nothing
    /// is selected). Folders render first, so a row index below `cf_folders_shown().len()`
    /// is a folder — folders have no per-object actions (download/delete/mark all skip
    /// them). The clone keeps the caller free of the immutable borrow while it mutates.
    pub(super) fn cf_selected_object(&self) -> Option<R2Object> {
        let n_folders = self.cf_folders_shown().len();
        let i = self.cf.r2_objects_row.selected()?;
        if i < n_folders {
            return None;
        }
        self.cf_objects_shown()
            .get(i - n_folders)
            .map(|o| (*o).clone())
    }

    /// The total rows in the objects table: subfolders + files, after the filter. The
    /// table is one list — folders first, then files — so this is its length.
    pub(super) fn cf_level_len(&self) -> usize {
        self.cf_folders_shown().len() + self.cf_objects_shown().len()
    }

    /// The account to activate on first entering the workspace: the config default,
    /// else the first stored account, else none (the empty state).
    fn cf_default_account(&self) -> Option<CloudflareAccount> {
        self.cf
            .accounts
            .iter()
            .find(|a| a.default)
            .or_else(|| self.cf.accounts.first())
            .cloned()
    }

    /// Enter the Cloudflare workspace: land on the Zones home of the active account
    /// (defaulting to the config default) and load its zones.
    pub(super) fn enter_cloudflare(&mut self, req: &Sender<Req>) {
        self.workspace = Workspace::Cloudflare;
        if self.cf.active.is_none() {
            self.cf.active = self.cf_default_account();
        }
        self.cf_goto_home(req);
    }

    /// Show the Zones home for the active account, clearing the old list so the
    /// loading state shows, then (re)load its zones. With no account configured
    /// there is nothing to load — the empty state invites adding one.
    pub(super) fn cf_goto_zones(&mut self, req: &Sender<Req>) {
        self.cf.screen = CfScreen::Zones;
        let Some(name) = self.cf.active.as_ref().map(|a| a.name.clone()) else {
            self.status = "No Cloudflare account yet — press a to add one".into();
            return;
        };
        self.cf_enter_list();
        if let Some(token) = self.cf_token() {
            let account_id = self.cf.active.as_ref().and_then(|a| a.account_id.clone());
            let _ = req.send(Req::Cf(CfReq::Zones {
                token: token.clone(),
                account_id: account_id.clone(),
            }));
            if let Some(account_id) = account_id {
                let _ = req.send(Req::Cf(CfReq::WebAnalyticsSites { token, account_id }));
            }
            self.status = format!("Loading zones for {name}…");
        }
    }

    /// Show the R2 buckets home for the active account, clearing the old list so the
    /// loading state shows, then load. R2 is account-scoped, so an account with no
    /// account-id cannot list buckets — say so rather than fire a call that 404s.
    pub(super) fn cf_goto_buckets(&mut self, req: &Sender<Req>) {
        let Some(name) = self.cf.active.as_ref().map(|a| a.name.clone()) else {
            self.status = "No Cloudflare account yet — press a to add one".into();
            return;
        };
        self.cf.filter.clear();
        self.cf.filter_input = false;
        self.cf.error = None;
        self.cf.r2_buckets.clear();
        self.cf.r2_row.select(None);
        let Some(account_id) = self.cf_account_id() else {
            self.status =
                "This account has no account-id — R2 is account-scoped; re-add it with one".into();
            return;
        };
        if let Some(token) = self.cf_token() {
            let _ = req.send(Req::Cf(CfReq::R2Buckets { token, account_id }));
            self.status = format!("Loading R2 buckets for {name}…");
        }
    }

    /// Show the account-level Analytics dashboard. This is tab 1, before DNS, and is
    /// account-scoped: Cloudflare GraphQL requires an accountTag, and the token needs
    /// Account Analytics:Read.
    pub(super) fn cf_goto_analytics(&mut self, req: &Sender<Req>) {
        let Some(name) = self.cf.active.as_ref().map(|a| a.name.clone()) else {
            self.status = "No Cloudflare account yet — press a to add one".into();
            return;
        };
        self.cf.screen = CfScreen::Zones;
        self.cf.filter.clear();
        self.cf.filter_input = false;
        self.cf.marked.clear();
        self.cf.error = None;
        self.cf.analytics = None;
        let Some(account_id) = self.cf_account_id() else {
            self.status =
                "This account has no account-id — Analytics is account-scoped; edit it with a"
                    .into();
            return;
        };
        if let Some(token) = self.cf_token() {
            let days = self.cf.analytics_days.max(7);
            let _ = req.send(Req::Cf(CfReq::Analytics {
                token,
                account_id,
                days,
            }));
            self.status = format!("Loading account analytics for {name}…");
        }
    }

    /// Show the account-level Cloudflare Tunnels list. Tunnels are account-scoped
    /// and are especially tied to domain routing, so this product lives between
    /// Domains and R2.
    pub(super) fn cf_goto_tunnels(&mut self, req: &Sender<Req>) {
        let Some(name) = self.cf.active.as_ref().map(|a| a.name.clone()) else {
            self.status = "No Cloudflare account yet — press a to add one".into();
            return;
        };
        self.cf.screen = CfScreen::Zones;
        self.cf.filter.clear();
        self.cf.filter_input = false;
        self.cf.marked.clear();
        self.cf.error = None;
        self.cf.tunnels.clear();
        self.cf.tunnels_row.select(None);
        self.cf.current_tunnel = None;
        self.cf.tunnel_config = None;
        self.cf.tunnel_config_row.select(None);
        let Some(account_id) = self.cf_account_id() else {
            self.status =
                "This account has no account-id — Tunnels are account-scoped; edit it with a"
                    .into();
            return;
        };
        if let Some(token) = self.cf_token() {
            let _ = req.send(Req::Cf(CfReq::Tunnels { token, account_id }));
            self.status = format!("Loading Tunnels for {name}…");
        }
    }

    /// Show the account-level Workers scripts list. Workers is account-scoped, so it
    /// requires the account-id just like R2.
    pub(super) fn cf_goto_workers(&mut self, req: &Sender<Req>) {
        let Some(name) = self.cf.active.as_ref().map(|a| a.name.clone()) else {
            self.status = "No Cloudflare account yet — press a to add one".into();
            return;
        };
        self.cf.screen = CfScreen::Zones;
        self.cf.filter.clear();
        self.cf.filter_input = false;
        self.cf.marked.clear();
        self.cf.error = None;
        self.cf.workers.clear();
        self.cf.workers_row.select(None);
        self.cf.current_worker = None;
        self.cf.worker_deployments.clear();
        self.cf.worker_deployments_row.select(None);
        let Some(account_id) = self.cf_account_id() else {
            self.status =
                "This account has no account-id — Workers is account-scoped; edit it with a".into();
            return;
        };
        if let Some(token) = self.cf_token() {
            let _ = req.send(Req::Cf(CfReq::Workers { token, account_id }));
            self.status = format!("Loading Workers for {name}…");
        }
    }

    /// The home for the active product: Zones for DNS, Buckets for R2. Used after an
    /// account switch so the picker lands on the right product's list.
    pub(super) fn cf_goto_home(&mut self, req: &Sender<Req>) {
        match self.cf.product {
            CfProduct::Analytics => self.cf_goto_analytics(req),
            CfProduct::Dns => self.cf_goto_zones(req),
            CfProduct::Tunnels => self.cf_goto_tunnels(req),
            CfProduct::R2 => self.cf_goto_buckets(req),
            CfProduct::Workers => self.cf_goto_workers(req),
        }
    }

    /// Switch the active product tab, loading its list on entry. DNS zones are
    /// already loaded on entering the workspace; R2 buckets load the first time the
    /// tab is selected (and on every `r`).
    pub(super) fn cf_set_product(&mut self, product: CfProduct, req: &Sender<Req>) {
        if self.cf.product == product {
            return;
        }
        // Marks belong to the screen they were made on; carrying them across a
        // product switch would show a stale "[Esc] to clear" message on a screen
        // whose Esc does something else entirely.
        self.cf.marked.clear();
        self.cf.product = product;
        self.cf_goto_home(req);
    }

    // ---------- Account picker (mirrors the server `s` picker) ----------

    /// Open the account picker overlay. Opens even with no accounts so `n` (add) is
    /// reachable; highlights the active account when there is one.
    pub(super) fn open_cf_picker(&mut self) {
        let mut state = ListState::default();
        let idx = self
            .cf
            .active
            .as_ref()
            .and_then(|act| self.cf.accounts.iter().position(|a| a.name == act.name))
            .unwrap_or(0);
        state.select((!self.cf.accounts.is_empty()).then_some(idx));
        self.cf_picker = Some(state);
    }

    /// The account highlighted in the picker.
    pub(super) fn cf_picker_selected(&self) -> Option<CloudflareAccount> {
        self.cf_picker
            .as_ref()
            .and_then(|s| s.selected())
            .and_then(|i| self.cf.accounts.get(i).cloned())
    }

    /// Enter on a zone: open its DNS records.
    pub(super) fn cf_open_records(&mut self, req: &Sender<Req>) {
        let Some(zone) = self.selected_cf_zone() else {
            self.status = "No zone selected".into();
            return;
        };
        self.cf_open_records_for_id(&zone.id, req);
    }

    /// Open a specific zone's DNS records by id — shared by Enter on a zone and the
    /// command palette's zone jump. Switches to the DNS product first (the palette can
    /// fire this from the R2 tab; Enter is already on DNS, so there it's a no-op). The
    /// zone is looked up in the loaded list, so a stale id yields an honest "not found"
    /// status rather than opening the wrong zone.
    pub(super) fn cf_open_records_for_id(&mut self, zone_id: &str, req: &Sender<Req>) {
        let Some(zone) = self.cf.zones.iter().find(|z| z.id == zone_id).cloned() else {
            self.status = "Zone not found".into();
            return;
        };
        self.cf.product = CfProduct::Dns;
        let (id, name) = (zone.id.clone(), zone.name.clone());
        self.cf.current_zone = Some(zone);
        self.cf.screen = CfScreen::Records;
        self.cf_enter_list();
        if let Some(token) = self.cf_token() {
            let _ = req.send(Req::Cf(CfReq::Records {
                token,
                zone_id: id,
                filter: RecordFilter::default(),
            }));
            self.status = format!("Loading records for {name}…");
        }
    }

    /// Enter on a bucket: drill into its objects (the R2 mirror of zone → records).
    /// Objects come from the REST API with the SAME Bearer token as buckets — no
    /// separate credentials. A token missing the R2 permission fails the fetch and
    /// lands in the normal error state (with the "Workers R2 Storage" hint).
    pub(super) fn cf_open_objects(&mut self, req: &Sender<Req>) {
        let Some(bucket) = self.selected_cf_bucket() else {
            self.status = "No bucket selected".into();
            return;
        };
        self.cf_open_objects_for(bucket.name.clone(), req);
    }

    /// Open a bucket's objects by name — shared by Enter on a bucket and the command
    /// palette's bucket jump. Switches to R2 first (the palette can fire this from the
    /// DNS tab; Enter is already on R2, so there it's a no-op).
    pub(super) fn cf_open_objects_for(&mut self, name: String, req: &Sender<Req>) {
        self.cf.product = CfProduct::R2;
        self.cf.current_bucket = Some(name);
        self.cf.screen = CfScreen::Objects;
        self.cf.marked.clear();
        // Land at the bucket root; deeper levels come from Enter on a folder.
        self.cf_request_level(String::new(), req);
    }

    /// Enter on a tunnel: open its Cloudflare-managed ingress/configuration rows,
    /// i.e. the TUI equivalent of the dashboard's published applications/config view.
    pub(super) fn cf_open_tunnel_config(&mut self, req: &Sender<Req>) {
        let Some(tunnel) = self.selected_cf_tunnel() else {
            self.status = "No tunnel selected".into();
            return;
        };
        self.cf_open_tunnel_config_for(tunnel.id, req);
    }

    /// Open a tunnel's configuration by id — shared by Enter and palette.
    pub(super) fn cf_open_tunnel_config_for(&mut self, tunnel_id: String, req: &Sender<Req>) {
        let Some(tunnel) = self
            .cf
            .tunnels
            .iter()
            .find(|t| t.id == tunnel_id || t.name == tunnel_id)
            .cloned()
        else {
            self.status = "Tunnel not found".into();
            return;
        };
        self.cf.product = CfProduct::Tunnels;
        self.cf.screen = CfScreen::TunnelConfig;
        self.cf.current_tunnel = Some(tunnel.clone());
        self.cf_enter_list();
        if let (Some(token), Some(account_id)) = (self.cf_token(), self.cf_account_id()) {
            let _ = req.send(Req::Cf(CfReq::TunnelConfig {
                token,
                account_id,
                tunnel_id: tunnel.id.clone(),
            }));
            self.status = format!("Loading Tunnel config for {}…", tunnel.name);
        }
    }

    /// Enter on a Worker: open its deployments/version history. This mirrors the
    /// Cloudflare dashboard's Worker detail page without adding another product tab.
    pub(super) fn cf_open_worker_deployments(&mut self, req: &Sender<Req>) {
        let Some(worker) = self.selected_cf_worker() else {
            self.status = "No Worker selected".into();
            return;
        };
        self.cf_open_worker_deployments_for(worker.id, req);
    }

    /// Open a specific Worker's deployments by name — shared by Enter and palette.
    pub(super) fn cf_open_worker_deployments_for(&mut self, name: String, req: &Sender<Req>) {
        self.cf.product = CfProduct::Workers;
        self.cf.screen = CfScreen::WorkerDeployments;
        self.cf.current_worker = Some(name.clone());
        self.cf_enter_list();
        if let (Some(token), Some(account_id)) = (self.cf_token(), self.cf_account_id()) {
            let _ = req.send(Req::Cf(CfReq::WorkerDeployments {
                token,
                account_id,
                name: name.clone(),
            }));
            self.status = format!("Loading deployments for {name}…");
        }
    }

    /// Open the highlighted Worker's settings/configuration.
    pub(super) fn cf_open_worker_settings(&mut self, req: &Sender<Req>) {
        let Some(worker) = self.selected_cf_worker() else {
            self.status = "No Worker selected".into();
            return;
        };
        self.cf_open_worker_settings_for(worker.id, req);
    }

    /// Open a specific Worker's settings by name — shared by `s` and palette.
    pub(super) fn cf_open_worker_settings_for(&mut self, name: String, req: &Sender<Req>) {
        self.cf.product = CfProduct::Workers;
        self.cf.screen = CfScreen::WorkerSettings;
        self.cf.current_worker = Some(name.clone());
        self.cf_enter_list();
        if let (Some(token), Some(account_id)) = (self.cf_token(), self.cf_account_id()) {
            let _ = req.send(Req::Cf(CfReq::WorkerSettings {
                token,
                account_id,
                name: name.clone(),
            }));
            self.status = format!("Loading settings for {name}…");
        }
    }

    /// Make an account the active one — shared by Enter in the `a` picker and the
    /// palette's account jump. Records the SetDefault side-effect (persisted by the
    /// event loop) and returns to that account's home, exactly as the picker did inline.
    pub(super) fn cf_activate_account(&mut self, acc: CloudflareAccount, req: &Sender<Req>) {
        self.cf_action = Some(CfAction::SetDefault(acc.name.clone()));
        self.cf.active = Some(acc);
        self.cf_goto_home(req);
    }

    /// Load ONE folder level of `current_bucket` at `prefix` (delimiter=/): reset the
    /// per-level view (folders, files, filter, selection) so the loading state shows,
    /// then fetch. Shared by Enter-on-bucket ("" root), Enter-on-folder (descend) and
    /// Esc (ascend). `current_prefix` is set NOW so the reply's prefix echo can be matched.
    pub(super) fn cf_request_level(&mut self, prefix: String, req: &Sender<Req>) {
        let Some(bucket) = self.cf.current_bucket.clone() else {
            return;
        };
        self.cf.current_prefix = prefix.clone();
        self.cf.r2_folders.clear();
        self.cf.r2_objects.clear();
        self.cf.r2_objects_row.select(None);
        self.cf.filter.clear();
        self.cf.filter_input = false;
        self.cf.error = None;
        if let (Some(token), Some(account_id)) = (self.cf_token(), self.cf_account_id()) {
            let _ = req.send(Req::Cf(CfReq::R2Objects {
                token,
                account_id,
                bucket: bucket.clone(),
                prefix,
            }));
            let where_ = if self.cf.current_prefix.is_empty() {
                bucket
            } else {
                format!("{bucket}/{}", self.cf.current_prefix)
            };
            self.status = format!("Loading {where_}…");
        }
    }

    /// Enter on the selected objects row: descend into a folder, or (on a file) a no-op
    /// with a status — object actions are a later slice. Row indices below the folder
    /// count are folders; the rest are files.
    pub(super) fn cf_object_enter(&mut self, req: &Sender<Req>) {
        let n_folders = self.cf_folders_shown().len();
        match self.cf.r2_objects_row.selected() {
            Some(i) if i < n_folders => {
                let folder = self.cf_folders_shown()[i].clone();
                self.cf_request_level(folder, req);
            }
            // A FILE row: Enter downloads it (the folders above still descend). The form
            // reads the selected object itself, so no argument travels here.
            Some(_) => self.open_cf_object_download(),
            None => {}
        }
    }

    /// Reset the per-list transient state when entering Zones, Records or Objects:
    /// clear the old list so the loading state shows, drop the filter, marks and error.
    fn cf_enter_list(&mut self) {
        self.cf.filter.clear();
        self.cf.filter_input = false;
        self.cf.marked.clear();
        self.cf.error = None;
        match self.cf.screen {
            CfScreen::Zones => {
                self.cf.zones.clear();
                self.cf.web_analytics_sites.clear();
                self.cf.zones_row.select(None);
            }
            CfScreen::Records => {
                self.cf.records.clear();
                self.cf.records_row.select(None);
            }
            CfScreen::Objects => {
                self.cf.r2_objects.clear();
                self.cf.r2_objects_row.select(None);
            }
            CfScreen::WorkerDeployments => {
                self.cf.worker_deployments.clear();
                self.cf.worker_deployments_row.select(None);
            }
            CfScreen::WorkerSettings => {
                self.cf.worker_settings = None;
                self.cf.worker_settings_row.select(None);
            }
            CfScreen::TunnelConfig => {
                self.cf.tunnel_config = None;
                self.cf.tunnel_config_row.select(None);
            }
        }
    }

    /// Reload the current CF list (after a mutation, or on `r`).
    pub(super) fn cf_reload(&self, req: &Sender<Req>) {
        let Some(token) = self.cf_token() else {
            return;
        };
        if self.cf.product == CfProduct::Analytics {
            if let Some(account_id) = self.cf_account_id() {
                let _ = req.send(Req::Cf(CfReq::Analytics {
                    token,
                    account_id,
                    days: self.cf.analytics_days.max(7),
                }));
            }
            return;
        }
        if self.cf.product == CfProduct::Workers {
            if let Some(account_id) = self.cf_account_id() {
                match self.cf.screen {
                    CfScreen::WorkerDeployments => {
                        if let Some(name) = self.cf.current_worker.clone() {
                            let _ = req.send(Req::Cf(CfReq::WorkerDeployments {
                                token,
                                account_id,
                                name,
                            }));
                        }
                    }
                    CfScreen::WorkerSettings => {
                        if let Some(name) = self.cf.current_worker.clone() {
                            let _ = req.send(Req::Cf(CfReq::WorkerSettings {
                                token,
                                account_id,
                                name,
                            }));
                        }
                    }
                    _ => {
                        let _ = req.send(Req::Cf(CfReq::Workers { token, account_id }));
                    }
                }
            }
            return;
        }
        if self.cf.product == CfProduct::Tunnels {
            if let Some(account_id) = self.cf_account_id() {
                match self.cf.screen {
                    CfScreen::TunnelConfig => {
                        if let Some(tunnel) = self.cf.current_tunnel.clone() {
                            let _ = req.send(Req::Cf(CfReq::TunnelConfig {
                                token,
                                account_id,
                                tunnel_id: tunnel.id,
                            }));
                        }
                    }
                    _ => {
                        let _ = req.send(Req::Cf(CfReq::Tunnels { token, account_id }));
                    }
                }
            }
            return;
        }
        if self.cf.product == CfProduct::R2 {
            // In the objects drill-in, `r` re-lists that bucket; on the buckets home it
            // re-lists buckets. Both go through the same Bearer token as the rest of CF.
            if self.cf.screen == CfScreen::Objects {
                if let (Some(account_id), Some(bucket)) =
                    (self.cf_account_id(), self.cf.current_bucket.clone())
                {
                    // Refresh the SAME level in place (no clear → no loading flash).
                    let _ = req.send(Req::Cf(CfReq::R2Objects {
                        token,
                        account_id,
                        bucket,
                        prefix: self.cf.current_prefix.clone(),
                    }));
                }
                return;
            }
            if let Some(account_id) = self.cf_account_id() {
                let _ = req.send(Req::Cf(CfReq::R2Buckets { token, account_id }));
            }
            return;
        }
        match self.cf.screen {
            CfScreen::Zones => {
                let account_id = self.cf.active.as_ref().and_then(|a| a.account_id.clone());
                let _ = req.send(Req::Cf(CfReq::Zones {
                    token: token.clone(),
                    account_id: account_id.clone(),
                }));
                if let Some(account_id) = account_id {
                    let _ = req.send(Req::Cf(CfReq::WebAnalyticsSites { token, account_id }));
                }
            }
            CfScreen::Records => {
                if let Some(zone) = &self.cf.current_zone {
                    let _ = req.send(Req::Cf(CfReq::Records {
                        token,
                        zone_id: zone.id.clone(),
                        filter: RecordFilter::default(),
                    }));
                }
            }
            // Objects and product detail screens are handled by their product branches above.
            CfScreen::Objects
            | CfScreen::WorkerDeployments
            | CfScreen::WorkerSettings
            | CfScreen::TunnelConfig => {}
        }
    }

    /// Keep the CF selection in range after the filter narrows the list.
    pub(super) fn cf_clamp_filtered(&mut self) {
        if self.cf.product == CfProduct::R2 {
            if self.cf.screen == CfScreen::Objects {
                let len = self.cf_level_len();
                *self.cf.r2_objects_row.offset_mut() = 0;
                self.cf.r2_objects_row.select((len > 0).then_some(0));
                return;
            }
            let len = self.cf_buckets_shown().len();
            *self.cf.r2_row.offset_mut() = 0;
            self.cf.r2_row.select((len > 0).then_some(0));
            return;
        }
        if self.cf.product == CfProduct::Workers {
            match self.cf.screen {
                CfScreen::WorkerDeployments => {
                    let len = self.cf_worker_deployments_shown().len();
                    *self.cf.worker_deployments_row.offset_mut() = 0;
                    self.cf
                        .worker_deployments_row
                        .select((len > 0).then_some(0));
                }
                CfScreen::WorkerSettings => {
                    let len = self.cf_worker_settings_shown().len();
                    *self.cf.worker_settings_row.offset_mut() = 0;
                    self.cf.worker_settings_row.select((len > 0).then_some(0));
                }
                _ => {
                    let len = self.cf_workers_shown().len();
                    *self.cf.workers_row.offset_mut() = 0;
                    self.cf.workers_row.select((len > 0).then_some(0));
                }
            }
            return;
        }
        if self.cf.product == CfProduct::Tunnels {
            match self.cf.screen {
                CfScreen::TunnelConfig => {
                    let len = self.cf_tunnel_config_rows_shown().len();
                    *self.cf.tunnel_config_row.offset_mut() = 0;
                    self.cf.tunnel_config_row.select((len > 0).then_some(0));
                }
                _ => {
                    let len = self.cf_tunnels_shown().len();
                    *self.cf.tunnels_row.offset_mut() = 0;
                    self.cf.tunnels_row.select((len > 0).then_some(0));
                }
            }
            return;
        }
        match self.cf.screen {
            CfScreen::Zones => {
                let len = self.cf_zones_shown().len();
                *self.cf.zones_row.offset_mut() = 0;
                self.cf.zones_row.select((len > 0).then_some(0));
            }
            CfScreen::Records => {
                let len = self.cf_records_shown().len();
                *self.cf.records_row.offset_mut() = 0;
                self.cf.records_row.select((len > 0).then_some(0));
            }
            // Objects and product detail screens are handled by their product branches above.
            CfScreen::Objects
            | CfScreen::WorkerDeployments
            | CfScreen::WorkerSettings
            | CfScreen::TunnelConfig => {}
        }
    }

    /// Toggle the mark on the highlighted record.
    pub(super) fn cf_toggle_mark(&mut self) {
        if let Some(r) = self.selected_cf_record() {
            if !self.cf.marked.remove(&r.id) {
                self.cf.marked.insert(r.id);
            }
        }
    }

    /// Mark every record currently shown; if all are already marked, clear them.
    pub(super) fn cf_mark_all_shown(&mut self) {
        let ids: Vec<String> = self
            .cf_records_shown()
            .iter()
            .map(|r| r.id.clone())
            .collect();
        if !ids.is_empty() && ids.iter().all(|id| self.cf.marked.contains(id)) {
            for id in &ids {
                self.cf.marked.remove(id);
            }
        } else {
            self.cf.marked.extend(ids);
        }
    }

    /// The status-bar line for CF marks, or None when nothing is marked. The
    /// EasyPanel wording comes from the domain (`cloudflare::marks_status`);
    /// this only picks the noun the current screen marks (records vs files).
    pub(super) fn cf_marks_status(&self) -> Option<String> {
        if self.cf.marked.is_empty() {
            return None;
        }
        let noun = if self.cf.screen == CfScreen::Objects {
            "file"
        } else {
            "record"
        };
        Some(crate::cloudflare::marks_status(noun, self.cf.marked.len()))
    }

    /// The add-record form. Type is a fixed choice (only v1 types are supported).
    pub(super) fn open_cf_record_form(&mut self) {
        let fields = vec![
            Field::choice("Type", &["A", "AAAA", "CNAME", "TXT", "NS", "MX"], "A"),
            Field::text("Name", ""),
            Field::text("Content", ""),
            Field::text("TTL", "1"),
            Field::boolean("Proxied", false),
            Field::text("Priority", ""),
        ];
        self.form = Some(
            Form::new(FormKind::CfRecordCreate, " New DNS record ", fields).with_note(
                "TTL 1 = automatic · Proxied only for A/AAAA/CNAME · Priority only for MX",
            ),
        );
    }

    /// The edit-record form, prefilled from the selected record. Type and name are
    /// fixed (a PATCH changes content/ttl/proxied/priority); which fields appear
    /// follows the record's type.
    pub(super) fn open_cf_record_edit(&mut self) {
        let Some(rec) = self.selected_cf_record() else {
            self.status = "No record selected".into();
            return;
        };
        let mut fields = vec![
            Field::text("Content", &rec.content),
            Field::text("TTL", &rec.ttl.to_string()),
        ];
        if proxyable(&rec.kind) {
            fields.push(Field::boolean("Proxied", rec.proxied));
        }
        if rec.kind.eq_ignore_ascii_case("MX") {
            fields.push(Field::text(
                "Priority",
                &rec.priority.map(|p| p.to_string()).unwrap_or_default(),
            ));
        }
        self.form = Some(
            Form::new(
                FormKind::CfRecordEdit {
                    id: rec.id.clone(),
                    kind: rec.kind.clone(),
                },
                format!(" Edit {} {} ", rec.kind, rec.name),
                fields,
            )
            .with_note("TTL 1 = automatic. Only the shown fields change."),
        );
    }

    /// Ask before deleting the selected record.
    pub(super) fn ask_cf_record_delete(&mut self) {
        let Some(rec) = self.selected_cf_record() else {
            self.status = "No record selected".into();
            return;
        };
        self.confirm = Some(Confirm {
            action: "cf-record-delete".into(),
            project: rec.id,
            service: String::new(),
            stype: String::new(),
            label: format!("Delete {} record '{}'?", rec.kind, rec.name),
        });
    }

    /// The add-zone form. Refuses to open without an account id — Cloudflare needs
    /// one to create a zone, and a request without it can only fail.
    pub(super) fn open_cf_zone_form(&mut self) {
        let has_id = self
            .cf
            .active
            .as_ref()
            .and_then(|a| a.account_id.as_ref())
            .is_some();
        if !has_id {
            self.status =
                "This account has no account-id — re-add it with one to create zones".into();
            return;
        }
        self.form = Some(Form::new(
            FormKind::CfZoneCreate,
            " New zone ",
            vec![Field::text("Name", "")],
        ));
    }

    /// The typed-name delete form for the selected zone. Deleting a zone destroys
    /// every DNS record in it, so the operator must type the zone name — the same
    /// safeguard the CLI `cf zone delete` uses.
    pub(super) fn open_cf_zone_delete_form(&mut self) {
        let Some(zone) = self.selected_cf_zone() else {
            self.status = "No zone selected".into();
            return;
        };
        self.form = Some(
            Form::new(
                FormKind::CfZoneDelete {
                    zone_id: zone.id.clone(),
                    name: zone.name.clone(),
                },
                " Delete zone ",
                vec![Field::text("Type the zone name to confirm", "")],
            )
            .with_note(format!(
                "Deletes '{}' and ALL its DNS records — this cannot be undone",
                zone.name
            )),
        );
    }

    /// The row action menu for the selected zone (Space / right-click) — the CF
    /// mirror of EasyPanel's per-row menu, built with the same `open_menu` /
    /// `MenuItem` machinery. Presentation only: its items route to the SAME flows
    /// the keys already use — open the zone's DNS records (as Enter does) and the
    /// typed-name delete form.
    pub(super) fn open_cf_zone_menu(&mut self) {
        if self.selected_cf_zone().is_none() {
            self.status = "No zone selected".into();
            return;
        }
        self.open_menu(vec![
            MenuItem::new("Open DNS records", |a, r| a.cf_open_records(r)),
            MenuItem::new("Delete zone…", |a, _| a.open_cf_zone_delete_form()),
        ]);
    }

    /// The per-record action menu — the CF mirror of EasyPanel's right-click row menu on
    /// the Records screen. `Space` there is the bulk menu (marked rows), so a single
    /// record's actions (edit / delete) get their own menu on right-click.
    pub(super) fn open_cf_record_menu(&mut self) {
        if self.selected_cf_record().is_none() {
            self.status = "No record selected".into();
            return;
        }
        self.open_menu(vec![
            MenuItem::new("Edit record", |a, _| a.open_cf_record_edit()),
            MenuItem::new("Delete record…", |a, _| a.ask_cf_record_delete()),
        ]);
    }

    /// The add-bucket form. Requires an account-id (R2 is account-scoped); on submit
    /// it sends a CreateR2Bucket request.
    pub(super) fn open_cf_bucket_form(&mut self) {
        if self.cf_account_id().is_none() {
            self.status =
                "This account has no account-id — R2 is account-scoped; re-add it with one".into();
            return;
        }
        self.form = Some(
            Form::new(
                FormKind::CfBucketCreate,
                " New R2 bucket ",
                vec![Field::text("Bucket name", "")],
            )
            .with_note("Lowercase letters, digits and hyphens; unique within the account."),
        );
    }

    /// The typed-name delete form for the selected bucket. Deleting a bucket is
    /// destructive (and Cloudflare requires it be empty), so the operator must type
    /// the bucket name — the same safeguard the zone delete uses.
    pub(super) fn open_cf_bucket_delete_form(&mut self) {
        let Some(bucket) = self.selected_cf_bucket() else {
            self.status = "No bucket selected".into();
            return;
        };
        self.form = Some(
            Form::new(
                FormKind::CfBucketDelete {
                    name: bucket.name.clone(),
                },
                " Delete R2 bucket ",
                vec![Field::text("Type the bucket name to confirm", "")],
            )
            .with_note(format!(
                "Deletes '{}' — the bucket must be EMPTY, and this cannot be undone",
                bucket.name
            )),
        );
    }

    /// The row action menu for the selected bucket (Space / right-click) — the R2
    /// mirror of the Zones row menu. Presentation only: routes to the typed-name
    /// delete form the `x` key already opens.
    pub(super) fn open_cf_bucket_menu(&mut self) {
        if self.selected_cf_bucket().is_none() {
            self.status = "No bucket selected".into();
            return;
        }
        self.open_menu(vec![
            MenuItem::new("Browse objects", |a, r| a.cf_open_objects(r)),
            MenuItem::new("Delete bucket…", |a, _| a.open_cf_bucket_delete_form()),
        ]);
    }

    /// The row action menu for the selected tunnel.
    pub(super) fn open_cf_tunnel_menu(&mut self) {
        if self.selected_cf_tunnel().is_none() {
            self.status = "No tunnel selected".into();
            return;
        }
        self.open_menu(vec![MenuItem::new("View routes/config", |a, r| {
            a.cf_open_tunnel_config(r)
        })]);
    }

    pub(super) fn open_cf_tunnel_config_menu(&mut self) {
        self.open_menu(vec![
            MenuItem::new("Add route…", |a, _| a.open_cf_tunnel_route_form()),
            MenuItem::new("Edit route…", |a, _| a.open_cf_tunnel_route_edit_form()),
            MenuItem::new("Delete route…", |a, _| {
                a.open_cf_tunnel_route_delete_form()
            }),
        ]);
    }

    pub(super) fn open_cf_tunnel_route_form(&mut self) {
        let Some(tunnel) = self.cf.current_tunnel.clone() else {
            self.status = "No tunnel open".into();
            return;
        };
        self.form = Some(
            Form::new(
                FormKind::CfTunnelRouteCreate {
                    tunnel_id: tunnel.id.clone(),
                },
                " Add tunnel route ",
                vec![
                    Field::text("Hostname", ""),
                    Field::text("Service", "http://localhost:3000"),
                    Field::text("Path", ""),
                    Field::boolean("No TLS verify", false),
                    Field::text("Advanced origin JSON", ""),
                ],
            )
            .with_note(
                "Service examples: http://localhost:3000, ssh://localhost:22, http_status:404. Advanced JSON is optional.",
            ),
        );
    }

    pub(super) fn open_cf_tunnel_route_edit_form(&mut self) {
        let Some(tunnel) = self.cf.current_tunnel.clone() else {
            self.status = "No tunnel open".into();
            return;
        };
        let Some(rule) = self.selected_cf_tunnel_route() else {
            self.status = "No route selected".into();
            return;
        };
        if rule.is_catch_all() {
            self.status = "Catch-all route is managed automatically".into();
            return;
        }
        let no_tls_verify = rule
            .origin_request
            .as_ref()
            .and_then(|v| v.get("noTLSVerify"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let advanced = tunnel_origin_advanced_json(rule.origin_request.as_ref());
        self.form = Some(
            Form::new(
                FormKind::CfTunnelRouteEdit {
                    tunnel_id: tunnel.id.clone(),
                    hostname: rule.hostname.clone(),
                    path: rule.path.clone(),
                },
                format!(" Edit route {} ", rule.hostname_label()),
                vec![
                    Field::text("Service", &rule.service),
                    Field::boolean("No TLS verify", no_tls_verify),
                    Field::text("Advanced origin JSON", &advanced),
                    Field::boolean("Clear origin request", false),
                ],
            )
            .with_note(
                "No TLS verify maps to Cloudflare noTLSVerify. Advanced JSON preserves other originRequest keys.",
            ),
        );
    }

    pub(super) fn open_cf_tunnel_route_delete_form(&mut self) {
        let Some(tunnel) = self.cf.current_tunnel.clone() else {
            self.status = "No tunnel open".into();
            return;
        };
        let Some(rule) = self.selected_cf_tunnel_route() else {
            self.status = "No route selected".into();
            return;
        };
        if rule.is_catch_all() {
            self.status = "Catch-all route is kept automatically".into();
            return;
        }
        self.form = Some(
            Form::new(
                FormKind::CfTunnelRouteDelete {
                    tunnel_id: tunnel.id.clone(),
                    hostname: rule.hostname.clone(),
                    path: rule.path.clone(),
                },
                " Delete tunnel route ",
                vec![Field::text("Type the hostname to confirm", "")],
            )
            .with_note(format!(
                "Removes '{}' from '{}' ingress config",
                rule.hostname_label(),
                tunnel.name
            )),
        );
    }

    pub(super) fn open_cf_worker_deploy_form(&mut self) {
        if self.cf_account_id().is_none() {
            self.status =
                "This account has no account-id — Workers is account-scoped; re-add it with one"
                    .into();
            return;
        }
        self.form = Some(
            Form::new(
                FormKind::CfWorkerDeploy,
                " Deploy Worker ",
                vec![
                    Field::text("Worker name", ""),
                    Field::text("Local file", ""),
                    Field::choice("Mode", &["module", "service-worker"], "module"),
                ],
            )
            .with_note("Uploads one JavaScript file to Workers Scripts. Existing script content is replaced."),
        );
    }

    pub(super) fn open_cf_worker_delete_form(&mut self) {
        let Some(worker) = self.selected_cf_worker() else {
            self.status = "No Worker selected".into();
            return;
        };
        self.form = Some(
            Form::new(
                FormKind::CfWorkerDelete {
                    name: worker.id.clone(),
                },
                " Delete Worker ",
                vec![Field::text("Type the Worker name to confirm", "")],
            )
            .with_note(format!(
                "Deletes '{}' and cannot be undone. Use CLI --force if Cloudflare reports attached bindings.",
                worker.id
            )),
        );
    }

    pub(super) fn open_cf_worker_menu(&mut self) {
        if self.selected_cf_worker().is_none() {
            self.status = "No Worker selected".into();
            return;
        }
        self.open_menu(vec![
            MenuItem::new("View settings", |a, r| a.cf_open_worker_settings(r)),
            MenuItem::new("View deployments", |a, r| a.cf_open_worker_deployments(r)),
            MenuItem::new("Deploy/replace Worker…", |a, _| {
                a.open_cf_worker_deploy_form()
            }),
            MenuItem::new("Delete Worker…", |a, _| a.open_cf_worker_delete_form()),
        ]);
    }

    /// The upload form for the currently-browsed level. One text field (a local path);
    /// the worker reads the file and computes the destination key from the prefix + the
    /// local basename, so the form carries nothing but the path.
    pub(super) fn open_cf_upload_form(&mut self) {
        let Some(bucket) = self.cf.current_bucket.clone() else {
            self.status = "No bucket open".into();
            return;
        };
        let dest = if self.cf.current_prefix.is_empty() {
            bucket
        } else {
            format!("{bucket}/{}", self.cf.current_prefix)
        };
        self.form = Some(
            Form::new(
                FormKind::R2Upload,
                format!(" Upload to {dest} "),
                vec![Field::text("Local file path", "")],
            )
            .with_note("Max 300 MB (the REST API limit); larger objects need the S3 API."),
        );
    }

    /// The download form for the selected FILE. `Save to` defaults to the object's
    /// basename (saved in the CWD); the worker refuses to overwrite an existing file. A
    /// no-op on a folder row (folders have no actions).
    pub(super) fn open_cf_object_download(&mut self) {
        let Some(o) = self.cf_selected_object() else {
            self.status = "Select a file — folders have no actions".into();
            return;
        };
        let base = crate::cloudflare::object_basename(&o.key).to_string();
        self.form = Some(
            Form::new(
                FormKind::R2Download { key: o.key },
                format!(" Download {base} "),
                vec![Field::text("Save to", &base)],
            )
            .with_note("Saved in the current directory unless you give a path; won't overwrite."),
        );
    }

    /// Ask before deleting the selected FILE (its key stashed in `project`, like a record
    /// delete). A no-op on a folder row.
    pub(super) fn ask_cf_object_delete(&mut self) {
        let Some(o) = self.cf_selected_object() else {
            self.status = "Select a file — folders have no actions".into();
            return;
        };
        self.confirm = Some(Confirm {
            action: "cf-object-delete".into(),
            project: o.key.clone(),
            service: String::new(),
            stype: String::new(),
            label: format!("Delete object '{}'?", o.key),
        });
    }

    /// Toggle the mark on the selected FILE (by object key). Folders are not markable —
    /// a folder row is skipped.
    pub(super) fn cf_toggle_object_mark(&mut self) {
        if let Some(o) = self.cf_selected_object() {
            if !self.cf.marked.remove(&o.key) {
                self.cf.marked.insert(o.key);
            }
        }
    }

    /// Mark every FILE shown at this level (folders excluded); if all are already marked,
    /// clear them. Mirrors `cf_mark_all_shown` for records.
    pub(super) fn cf_mark_all_objects(&mut self) {
        let keys: Vec<String> = self
            .cf_objects_shown()
            .iter()
            .map(|o| o.key.clone())
            .collect();
        if !keys.is_empty() && keys.iter().all(|k| self.cf.marked.contains(k)) {
            for k in &keys {
                self.cf.marked.remove(k);
            }
        } else {
            self.cf.marked.extend(keys);
        }
    }

    /// The per-object action menu (right-click a FILE, or Space with nothing marked):
    /// Download / Delete…. Both items read the selected object themselves, so a plain
    /// fn-pointer item works. A no-op on a folder row (folders have no actions).
    pub(super) fn open_cf_object_menu(&mut self) {
        if self.cf_selected_object().is_none() {
            self.status = "Select a file — folders have no actions".into();
            return;
        }
        self.open_menu(vec![
            MenuItem::new("Download", |a, _| a.open_cf_object_download()),
            MenuItem::new("Delete…", |a, _| a.ask_cf_object_delete()),
        ]);
    }

    /// The bulk action menu for the marked objects: Download / Delete. Opened by Space
    /// when ≥1 is marked.
    pub(super) fn open_cf_object_bulk_menu(&mut self) {
        let n = self.cf.marked.len();
        if n == 0 {
            self.status = "No objects marked — v marks one, V marks all shown".into();
            return;
        }
        self.open_menu(vec![
            MenuItem::new(format!("Download {n} marked"), |a, r| a.cf_bulk_download(r)),
            MenuItem::new(format!("Delete {n} marked"), |a, _| {
                a.ask_cf_object_bulk_delete()
            }),
        ]);
    }

    /// Ask before deleting the marked objects.
    pub(super) fn ask_cf_object_bulk_delete(&mut self) {
        let n = self.cf.marked.len();
        self.confirm = Some(Confirm {
            action: "cf-object-bulk-delete".into(),
            project: String::new(),
            service: String::new(),
            stype: String::new(),
            label: format!("Delete {n} marked object(s)?"),
        });
    }

    /// Download every marked object into the CWD under its basename (one worker job).
    /// Marks are cleared once dispatched — they have served their purpose.
    pub(super) fn cf_bulk_download(&mut self, req: &Sender<Req>) {
        let keys: Vec<String> = self.cf.marked.iter().cloned().collect();
        if keys.is_empty() {
            self.status = "No objects marked".into();
            return;
        }
        let (Some(token), Some(account_id), Some(bucket)) = (
            self.cf_token(),
            self.cf_account_id(),
            self.cf.current_bucket.clone(),
        ) else {
            return;
        };
        let _ = req.send(Req::Cf(CfReq::R2GetMany {
            token,
            account_id,
            bucket,
            keys,
            dir: ".".into(),
        }));
        self.cf.marked.clear();
        self.status = "Downloading marked objects…".into();
    }

    /// The bulk action menu for the marked records.
    pub(super) fn open_cf_bulk_menu(&mut self) {
        let n = self.cf.marked.len();
        if n == 0 {
            self.status = "No records marked — v marks one, V marks all shown".into();
            return;
        }
        let items = vec![
            MenuItem::new(format!("Set content on {n}"), |app, _| {
                app.open_cf_bulk_form(CfBulkAttr::Content)
            }),
            MenuItem::new(format!("Set proxied on {n}"), |app, _| {
                app.open_cf_bulk_form(CfBulkAttr::Proxied)
            }),
            MenuItem::new(format!("Set TTL on {n}"), |app, _| {
                app.open_cf_bulk_form(CfBulkAttr::Ttl)
            }),
            MenuItem::new(format!("Delete {n} marked"), |app, _| {
                app.ask_cf_bulk_delete()
            }),
        ];
        self.open_menu(items);
    }

    /// The form for a bulk attribute set (content / proxied / ttl).
    pub(super) fn open_cf_bulk_form(&mut self, attr: CfBulkAttr) {
        let n = self.cf.marked.len();
        let field = match attr {
            CfBulkAttr::Content => Field::text("Content", ""),
            CfBulkAttr::Proxied => Field::boolean("Proxied", false),
            CfBulkAttr::Ttl => Field::text("TTL", "1"),
        };
        self.form = Some(Form::new(
            FormKind::CfBulkSet(attr),
            format!(" Set on {n} marked records "),
            vec![field],
        ));
    }

    /// Ask before deleting the marked records.
    pub(super) fn ask_cf_bulk_delete(&mut self) {
        let n = self.cf.marked.len();
        self.confirm = Some(Confirm {
            action: "cf-bulk-delete".into(),
            project: String::new(),
            service: String::new(),
            stype: String::new(),
            label: format!("Delete {n} marked DNS record(s)?"),
        });
    }

    pub(super) fn handle(&mut self, resp: Resp, req: &Sender<Req>) {
        match resp {
            Resp::Stats(v) => {
                self.refresh_inflight = false;
                self.stats_error = None;
                self.stats = Some(v);
            }
            Resp::StatsErr(e) => {
                // Keep last-good stats on a refresh failure; only note the error so
                // a Dashboard with NO stats yet says so instead of drawing 0.0%.
                self.refresh_inflight = false;
                self.stats_error = Some(e.clone());
                self.status = format!("Error: {e}");
            }
            Resp::Nodes(n) => self.nodes = n,
            Resp::Actions(a) => {
                self.actions = a;
                select_first(&mut self.actions_state, self.actions.len());
            }
            Resp::MonitorData(m) => self.monitor = m,
            Resp::TaskStats(t) => self.task_stats = t,
            Resp::Storage(s) => self.storage = s,
            Resp::Domains(d) => {
                self.domains_error = None;
                self.domains = d;
                select_first(&mut self.domains_state, self.domains.len());
            }
            Resp::DomainsErr(e) => {
                // Keep any previously loaded domains on screen; only remember that
                // the refresh failed so an EMPTY list reads as "couldn't load".
                self.domains_error = Some(e.clone());
                self.status = format!("Error: {e}");
            }
            Resp::Projects(p) => self.projects = p,
            Resp::AllServicesErr(e) => {
                // Keep any services already on screen; only mark the failure so an
                // EMPTY list reads as "couldn't load", not "this host has nothing".
                self.services_error = Some(e.clone());
                self.status = format!("Error: {e}");
            }
            Resp::AllServices { projects, services } => {
                self.services_error = None;
                self.projects = projects;
                self.all_services = services;
                self.all_services
                    .sort_by_key(|s| (field(s, "/projectName"), field(s, "/name")));
                // Land on the first SERVICE, not row 0 — row 0 is a project header,
                // and every service action is a no-op while a header is highlighted,
                // which made the whole action menu look broken on first contact.
                if self.services_table.selected().is_none() {
                    self.services_table.select(self.first_service_row());
                }
            }
            Resp::ServicesFor(project, names) => {
                if let Some(form) = self.form.as_mut() {
                    if form.by_label("Project") == project {
                        if let Some(f) = form.fields.iter_mut().find(|f| f.label == "Service") {
                            f.set_options(names);
                        }
                    }
                }
            }
            Resp::ResourceForm {
                project,
                service,
                stype,
                data,
            } => {
                let title = format!("Resource · {project}/{service}");
                self.form = Some(
                    Form::new(
                        FormKind::ResourceEdit {
                            project,
                            service,
                            stype,
                        },
                        title,
                        resource_fields(data.get("resources")),
                    )
                    .with_note("0 = unlimited"),
                );
            }
            Resp::BasicAuthForm {
                project,
                service,
                stype,
                data,
            } => {
                let title = format!("Basic auth · {project}/{service}");
                self.form = Some(
                    Form::new(
                        FormKind::BasicAuthEdit {
                            project,
                            service,
                            stype,
                        },
                        title,
                        basic_auth_fields(Some(&data)),
                    )
                    .with_note("clear both fields = turn protection off"),
                );
            }
            Resp::ConfigForm {
                project,
                service,
                build,
                data,
                repos,
            } => {
                let title = format!(
                    "{} · {project}/{service}",
                    if build { "Build" } else { "Source" }
                );
                let form = if build {
                    Form::new(
                        FormKind::BuildEdit { project, service },
                        title,
                        build_fields(data.get("build")),
                    )
                    .with_original(data.get("build").cloned().unwrap_or(Value::Null))
                } else {
                    Form::new(
                        FormKind::SourceEdit { project, service },
                        title,
                        source_fields(data.get("source"), repos),
                    )
                };
                self.form = Some(form);
                self.load_form_branches(req);
            }
            Resp::HostStat { name, data } => {
                if let Some(h) = self.hosts.iter_mut().find(|h| h.name == name) {
                    h.state = match data {
                        Ok(v) => HostState::Ok(Box::new(v)),
                        Err(e) => HostState::Err(e),
                    };
                }
                select_first(&mut self.hosts_state, self.hosts.len());
            }
            Resp::MaintInfo(rows) => self.maint = rows,
            Resp::LogTail { lines, cursor } => {
                // The first batch arrives into an empty viewer.lines, so appending
                // = replacing; later rounds append. No need to know which: `since`
                // decides what the server sends.
                if !lines.is_empty() {
                    self.viewer.lines.extend(lines);
                    // An hours-long tail must not pile up without bound.
                    let extra = self.viewer.lines.len().saturating_sub(LOG_BUFFER);
                    self.viewer.lines.drain(..extra);
                }
                if cursor.is_some() {
                    self.viewer.log_cursor = cursor;
                }
            }
            Resp::Repos(repos) => {
                if let Some(f) = self
                    .form
                    .as_mut()
                    .and_then(|form| form.fields.iter_mut().find(|f| f.label == "Repo"))
                {
                    let mut opts = repos;
                    // An empty choice is required while nothing is selected:
                    // set_options() jumps to the first option if the current value
                    // isn't in the list, so without this a new form would silently
                    // point the source at a random repo.
                    if f.value.is_empty() {
                        opts.insert(0, String::new());
                    }
                    f.set_options(opts);
                }
            }
            Resp::Branches(result) => {
                let Some(form) = self.form.as_mut() else {
                    return;
                };
                let Some(f) = form.fields.iter_mut().find(|f| f.label == "Branch") else {
                    return;
                };
                match result {
                    Ok(names) => f.set_options(names),
                    // Without a branch list, the dropdown only holds the current
                    // value — the user is locked to that branch and can't change it
                    // at all. Fall back to a text input: the server still rejects a
                    // nonexistent branch ("Branch not found"), so nothing is lost
                    // but the convenience of picking one.
                    Err(e) => {
                        f.kind = FieldKind::Text;
                        self.status = format!(
                            "Branch list couldn't load ({}) — type the branch name manually. \
                             Fix the GitHub token in EasyPanel > Settings.",
                            short_reason(&e)
                        );
                    }
                }
            }
            // The restore picker. An empty history is a real answer, not an
            // error: this database has never been backed up, and saying so beats
            // an empty box.
            Resp::DeployForm {
                project,
                service,
                deploy,
            } => {
                self.form = Some(
                    Form::new(
                        FormKind::DeployEdit {
                            project: project.clone(),
                            service: service.clone(),
                        },
                        format!(" Deploy · {project}/{service} "),
                        deploy_fields(deploy.as_object().map(|_| &deploy)),
                    )
                    .with_note("takes effect on the next deploy".to_string()),
                );
                self.status = "Ready".into();
            }
            Resp::MountForm {
                project,
                service,
                index,
                values,
            } => {
                self.form = Some(
                    Form::new(
                        FormKind::MountEdit {
                            project,
                            service,
                            index,
                        },
                        format!(" Edit mount [{index}] "),
                        mount_fields(Some(&values)),
                    )
                    .with_note(
                        "the mount path only takes effect after the service restarts".to_string(),
                    ),
                );
                self.status = "Ready".into();
            }
            Resp::StorageProviders(list) => self.backups.providers = list,
            Resp::DatabasesIn {
                project,
                service,
                names,
            } => self.open_backup_picker(project, service, names),
            Resp::R2Dumps {
                project,
                service,
                keys,
            } => {
                let title = format!("Restore {project}/{service} from an object-storage dump");
                let lines = if keys.is_empty() {
                    vec![
                        "No dumps found for this service.".into(),
                        String::new(),
                        "Make one first: Storage ▸ Dump now (non-locking).".into(),
                    ]
                } else {
                    keys.iter()
                        .enumerate()
                        .map(|(i, k)| format!("{} {k}", row_marker(i)))
                        .collect()
                };
                self.backups.r2_dumps = keys;
                self.backups.r2_restore_into = Some((project, service));
                self.show_picker(title, lines);
                self.status = if self.backups.r2_dumps.is_empty() {
                    "No dumps yet".into()
                } else {
                    "[Enter] restore the selected dump · [Esc] back".into()
                };
            }
            Resp::BackupHistoryFrom {
                src_name,
                project,
                service,
                hidden,
                rows,
                files,
            } => {
                // The provider id from the SOURCE panel is meaningless here: ids
                // are per-panel (verified — the same R2 bucket has a different id
                // on each host). Every file is re-pointed at THIS host's remote
                // provider, which is what actually reads the bucket.
                let local = self
                    .backups
                    .providers
                    .iter()
                    .find(|(_, _, t)| crate::backup::is_remote(t))
                    .map(|(id, ..)| id.clone());
                let Some(local) = local else {
                    self.status = "This host has no remote storage to read that backup from".into();
                    return;
                };
                let mut lines = vec![format!(
                    "From {src_name} · restoring into {project}/{service}"
                )];
                if hidden > 0 {
                    lines.push(format!(
                        "({hidden} more exist there on local disk, unreadable from here)"
                    ));
                }
                lines.push(String::new());
                if rows.is_empty() {
                    lines.push("No backups on shared remote storage.".into());
                } else {
                    lines.push(format!(
                        "    {:<21}{:<24}{:<20}{}",
                        "When", "From", "Database", "File"
                    ));
                    lines.extend(
                        rows.iter()
                            .enumerate()
                            .map(|(i, r)| format!("{} {r}", row_marker(i))),
                    );
                }
                self.backups.files = files
                    .into_iter()
                    .map(|(db, _, path)| (db, local.clone(), path))
                    .collect();
                self.backups.restore_into = Some((project, service));
                self.show_picker(format!("Restore from {src_name}"), lines);
                self.status = if self.backups.files.is_empty() {
                    "Nothing there that this host can read".into()
                } else {
                    "[Enter] restore the selected backup · [Esc] back".into()
                };
            }
            Resp::BackupHistory {
                project,
                service,
                rows,
                files,
            } => {
                let title = format!("Restore into {project}/{service}");
                let lines = if rows.is_empty() {
                    vec![
                        "No backups found for this service.".into(),
                        String::new(),
                        "Take one first: Storage ▸ Backup now.".into(),
                    ]
                } else {
                    // Four spaces stand in for the "[n] " each row carries, so
                    // the labels sit over their own columns.
                    let mut v = vec![format!("    {:<21}{:<18}{}", "When", "Database", "File")];
                    v.extend(
                        rows.iter()
                            .enumerate()
                            .map(|(i, r)| format!("{} {r}", row_marker(i))),
                    );
                    v
                };
                self.backups.files = files;
                self.backups.restore_into = Some((project, service));
                self.show_picker(title, lines);
                self.status = if self.backups.files.is_empty() {
                    "No backups yet".into()
                } else {
                    "[Enter] restore the selected backup · [Esc] back".into()
                };
            }
            // Succeeded, with something the user must act on. The viewer, because
            // the status line is one line and these sentences are longer than any
            // terminal: the clone note explaining WHY a config file was held back
            // was being cut off exactly where the reason began.
            Resp::Notes {
                msg,
                notes,
                refresh,
            } => {
                // Wrapped HERE, because the viewer scrolls long lines sideways
                // rather than folding them: unwrapped, the note ran off the right
                // edge and the reason was once again unreadable. 76 keeps it whole
                // on an 80-column terminal, the narrowest this TUI targets.
                const WRAP: usize = 76;
                let mut lines = super::render::wrap_words(&msg, WRAP);
                for n in &notes {
                    lines.push(String::new());
                    lines.extend(super::render::wrap_words(n, WRAP));
                }
                self.viewer.from = self.screen;
                self.show_viewer("Done — please read".into(), lines);
                self.viewer.ctx = None;
                self.status = format!("⚠ {msg}");
                self.apply_refresh(refresh, req);
            }
            // A bulk run that fully succeeded is a status line. One with ANY
            // failure opens the list instead: a message that fades cannot carry
            // three service names, and "9 of 12" without the missing three is the
            // half-truth this project keeps having to fix.
            Resp::BulkDone { action, ok, failed } => {
                self.marked.clear();
                let _ = req.send(Req::AllServices);
                if failed.is_empty() {
                    self.status = format!("{} done on {} services", cap(&action), ok.len());
                    return;
                }
                let mut lines = vec![
                    format!(
                        "{}: {} succeeded, {} FAILED",
                        cap(&action),
                        ok.len(),
                        failed.len()
                    ),
                    String::new(),
                ];
                lines.extend(failed.iter().map(|(name, why)| format!("✗ {name} — {why}")));
                if !ok.is_empty() {
                    lines.push(String::new());
                    lines.extend(ok.iter().map(|name| format!("✓ {name}")));
                }
                self.show_viewer(format!("Bulk {action} — {} failed", failed.len()), lines);
                self.viewer.ctx = None;
                self.status = format!("{} of {} failed", failed.len(), failed.len() + ok.len());
            }
            // Same rule as a bulk lifecycle run: all-clear is a status line, any
            // failure opens the list, because "9 of 12" without the missing three
            // leaves the user to hunt for them by eye.
            Resp::DomainsEdited { ok, failed } => {
                self.apply_refresh(Refresh::Domains, req);
                if failed.is_empty() {
                    self.status = format!("{ok} domain(s) rewritten");
                    return;
                }
                let mut lines = vec![
                    format!("{ok} rewritten, {} FAILED", failed.len()),
                    String::new(),
                ];
                lines.extend(failed.iter().map(|(name, why)| format!("✗ {name} — {why}")));
                self.show_viewer(
                    format!(" Bulk domain edit — {} failed ", failed.len()),
                    lines,
                );
                self.viewer.ctx = None;
                self.status = format!("⚠ {} of {} failed", failed.len(), failed.len() + ok);
            }
            // Saving is only half of it: the running containers keep the OLD
            // values until they are deployed again — proven live, the container
            // still reported the previous value after the project env changed,
            // and the new one only after a deploy. Reporting "saved" and stopping
            // there would be a change the user believes has taken effect.
            Resp::ProjectEnvSaved(project) => {
                let stale = self.deployable_in(&project);
                if stale.is_empty() {
                    self.status = format!("Project env saved for {project}");
                    return;
                }
                self.confirm = Some(Confirm {
                    action: "project-env-deploy".into(),
                    project,
                    service: String::new(),
                    stype: String::new(),
                    label: format!(
                        "Env saved. {} service(s) still run the old values until deployed. Deploy them now?",
                        stale.len()
                    ),
                });
            }
            Resp::Checked(probes) => {
                self.checking = false;
                self.probes = probes;
                select_first(&mut self.uptime_state, self.watch.len());
                let rows = crate::uptime::ranked(&self.watch, &self.probes);
                let bad = rows
                    .iter()
                    .filter(|(c, p)| {
                        p.is_some_and(|p| p.verdict(c) != crate::uptime::Verdict::Working)
                    })
                    .count();
                let median = crate::uptime::median_head(&self.probes);
                self.status = match (bad, median) {
                    // The failure count leads: it is the reason anyone opened
                    // this screen, and a median means nothing next to a domain
                    // that is not answering at all.
                    (0, Some(m)) => format!(
                        "All {} answering — median {}",
                        self.watch.len(),
                        crate::uptime::human(m)
                    ),
                    (0, None) => "Nothing answered".into(),
                    (n, Some(m)) => format!(
                        "⚠ {n} of {} not healthy — median {}",
                        self.watch.len(),
                        crate::uptime::human(m)
                    ),
                    (n, None) => format!("⚠ {n} of {} not healthy", self.watch.len()),
                };
            }
            Resp::Viewer(title, lines) => {
                self.show_viewer(title, lines);
                self.status = "Ready".into();
            }
            Resp::TermOutput(bytes) => {
                if let Some(p) = self.term.parser.as_mut() {
                    p.process(&bytes);
                }
            }
            Resp::TermClosed => {
                // Shell exited / socket closed: back to Services.
                self.term.parser = None;
                self.term.input = None;
                if self.screen == Screen::Terminal {
                    self.screen = Screen::Projects;
                    self.status = format!("Terminal {} closed", self.term.title);
                }
            }
            Resp::Done(msg, what) => {
                self.status = msg;
                self.apply_refresh(what, req);
            }
            Resp::Err(e) => self.status = format!("Error: {e}"),
            Resp::Cf(cf) => self.handle_cf_resp(cf, req),
        }
    }

    /// Route a Cloudflare reply into `app.cf`. A successful list clears the last
    /// error and re-seeds the selection; a Done/BulkDone re-lists the screen so a
    /// change is visible at once (the "deleted row still showing" class of bug).
    fn handle_cf_resp(&mut self, resp: CfResp, req: &Sender<Req>) {
        match resp {
            CfResp::Analytics(summary) => {
                self.cf.error = None;
                self.cf.analytics_days = summary.days;
                self.cf.analytics = Some(summary);
            }
            CfResp::Zones(zones) => {
                self.cf.error = None;
                self.cf.zones = zones;
                let len = self.cf_zones_shown().len();
                select_first(&mut self.cf.zones_row, len);
            }
            CfResp::Records { zone_id, records } => {
                // Discard a stale reply for a zone the user has already left.
                if self.cf.current_zone.as_ref().map(|z| z.id.as_str()) == Some(zone_id.as_str()) {
                    self.cf.error = None;
                    self.cf.records = records;
                    let len = self.cf_records_shown().len();
                    select_first(&mut self.cf.records_row, len);
                }
            }
            CfResp::WebAnalyticsSites(sites) => {
                self.cf.error = None;
                self.cf.web_analytics_sites = sites;
            }
            CfResp::WebAnalyticsErr(e) => {
                self.cf.web_analytics_sites.clear();
                self.status = format!(
                    "Web Analytics unavailable: {e} — add Account Settings Read to show metadata"
                );
            }
            CfResp::R2Buckets(buckets) => {
                self.cf.error = None;
                self.cf.r2_buckets = buckets;
                let len = self.cf_buckets_shown().len();
                select_first(&mut self.cf.r2_row, len);
            }
            CfResp::Tunnels(tunnels) => {
                self.cf.error = None;
                self.cf.tunnels = tunnels;
                let len = self.cf_tunnels_shown().len();
                select_first(&mut self.cf.tunnels_row, len);
            }
            CfResp::TunnelConfig { tunnel_id, config } => {
                // Discard a stale config reply if the user has already opened another tunnel.
                if self.cf.current_tunnel.as_ref().map(|t| t.id.as_str())
                    == Some(tunnel_id.as_str())
                {
                    self.cf.error = None;
                    self.cf.tunnel_config = Some(*config);
                    let len = self.cf_tunnel_config_rows_shown().len();
                    select_first(&mut self.cf.tunnel_config_row, len);
                }
            }
            CfResp::Workers(workers) => {
                self.cf.error = None;
                self.cf.workers = workers;
                let len = self.cf_workers_shown().len();
                select_first(&mut self.cf.workers_row, len);
            }
            CfResp::WorkerDeployments {
                worker,
                deployments,
            } => {
                // Discard a stale deployments reply if the user has already opened
                // another Worker.
                if self.cf.current_worker.as_deref() == Some(worker.as_str()) {
                    self.cf.error = None;
                    self.cf.worker_deployments = deployments;
                    let len = self.cf_worker_deployments_shown().len();
                    select_first(&mut self.cf.worker_deployments_row, len);
                }
            }
            CfResp::WorkerSettings { worker, settings } => {
                // Discard a stale settings reply if the user has already opened another Worker.
                if self.cf.current_worker.as_deref() == Some(worker.as_str()) {
                    self.cf.error = None;
                    self.cf.worker_settings = Some(*settings);
                    let len = self.cf_worker_settings_shown().len();
                    select_first(&mut self.cf.worker_settings_row, len);
                }
            }
            CfResp::R2Objects {
                bucket,
                prefix,
                folders,
                objects,
                truncated,
            } => {
                // Discard a stale reply for a level the user has already left — a different
                // bucket OR a different prefix (an old level must not overwrite a newer one).
                if self.cf.current_bucket.as_deref() == Some(bucket.as_str())
                    && self.cf.current_prefix == prefix
                {
                    self.cf.error = None;
                    self.cf.r2_folders = folders;
                    self.cf.r2_objects = objects;
                    self.cf.r2_truncated = truncated;
                    let len = self.cf_level_len();
                    select_first(&mut self.cf.r2_objects_row, len);
                }
            }
            CfResp::Done(msg) => {
                self.status = msg;
                self.cf_reload(req);
            }
            // A download wrote a local file — the current level is unchanged, so set the
            // status but do NOT reload (a reload would flash the loading state for nothing).
            CfResp::Status(msg) => {
                self.status = msg;
            }
            CfResp::BulkDone { ok, failed } => {
                self.cf.marked.clear();
                self.cf_reload(req);
                if failed.is_empty() {
                    self.status = format!("{ok} record(s) done");
                    return;
                }
                let mut lines = vec![
                    format!("{ok} succeeded, {} FAILED", failed.len()),
                    String::new(),
                ];
                lines.extend(failed.iter().map(|(id, why)| format!("✗ {id} — {why}")));
                self.viewer.from = self.screen;
                self.show_viewer(format!("Cloudflare bulk — {} failed", failed.len()), lines);
                self.viewer.ctx = None;
                self.status = format!("⚠ {} of {} failed", failed.len(), failed.len() + ok);
            }
            CfResp::Err(e) => {
                self.cf.error = Some(e.clone());
                self.status = format!("Error: {e}");
            }
        }
    }

    /// Put `lines` on screen in the viewer.
    ///
    /// Seven places used to assemble the same five fields by hand, and they had
    /// already drifted: two forgot `viewer.ctx`, one forgot `viewer.from` — so
    /// Esc left it going back to whichever screen the last viewer happened to
    /// record. One definition, and what each caller does DIFFERENTLY is now a
    /// visible line next to it instead of buried in a nine-line block.
    fn show_viewer(&mut self, title: String, lines: Vec<String>) {
        self.viewer.title = title;
        self.viewer.lines = lines;
        self.viewer.scroll = 0;
        self.viewer.hscroll = 0;
        // The SELECTED row resets too. It used to survive, so opening a
        // collection inherited whatever index the last one was left on — a
        // different service, a different resource, a row the user never chose,
        // sitting armed under `x delete`.
        self.viewer.row = TableState::default();
        self.screen = Screen::Viewer;
    }

    /// A viewer whose rows are CHOSEN from: the selection starts on the first
    /// `[n]` row rather than on the heading above it, and Esc returns to the
    /// screen the picker was opened from.
    fn show_picker(&mut self, title: String, lines: Vec<String>) {
        // Captured before `show_viewer` makes the current screen the Viewer.
        self.viewer.from = self.screen;
        self.show_viewer(title, lines);
        let first = self.viewer.lines.iter().position(|l| is_row(l));
        self.viewer.row = TableState::default().with_selected(first);
        self.viewer.ctx = None;
    }

    /// Reload whatever an operation invalidated. One definition, because two
    /// results now carry a Refresh (`Done` and `Notes`) and a second inline copy
    /// is how a screen quietly stops updating after one of them.
    fn apply_refresh(&mut self, what: Refresh, req: &Sender<Req>) {
        let _ = match what {
            Refresh::Projects => req.send(Req::AllServices),
            Refresh::Domains => req.send(Req::Domains),
            Refresh::None => return,
        };
    }

    pub(super) fn filterable(&self) -> bool {
        matches!(
            self.screen,
            Screen::Domains | Screen::Actions | Screen::Monitor | Screen::Projects
        )
    }

    pub(super) fn clear_filter(&mut self) {
        self.filter.clear();
        self.filter_input = false;
        self.clamp_filtered();
    }

    /// The filter changed: start the view at the first match.
    ///
    /// Clamping the selected index is not enough, and keeping it is not even
    /// meaningful — row 451 of the filtered list is a different row from row 451
    /// of the unfiltered one. Worse, ratatui keeps the scroll offset separately
    /// and only moves it when the selection sits ABOVE it, so a list scrolled to
    /// the bottom and then narrowed rendered from an offset past most of the
    /// matches: a real host with 713 domains, filtered to 452, showed ONE row
    /// under a title that said 452. The screen contradicted its own heading.
    pub(super) fn clamp_filtered(&mut self) {
        let len = match self.screen {
            Screen::Domains => self.visible_domains().len(),
            Screen::Actions => self.visible_actions().len(),
            Screen::Monitor => self.monitor_rows_shown(),
            Screen::Projects => self.visible_rows().len(),
            _ => return,
        };
        let state = match self.screen {
            Screen::Domains => &mut self.domains_state,
            Screen::Actions => &mut self.actions_state,
            Screen::Monitor => &mut self.monitor_state,
            Screen::Projects => &mut self.services_table,
            _ => return,
        };
        // The offset is reset explicitly rather than trusting the selection to
        // drag it: ratatui pulls the offset down to reveal a selection above it,
        // but nothing guarantees that for a list that changed length underneath.
        *state.offset_mut() = 0;
        match len {
            0 => state.select(None),
            _ => state.select(Some(0)),
        }
    }

    pub(super) fn visible_actions(&self) -> Vec<&Value> {
        self.actions
            .iter()
            .filter(|a| {
                keep(
                    &commands::action_row(a, commands::ACTION_DESC_TUI),
                    &self.filter,
                )
            })
            // "Failures only" keeps everything that is not a clean, finished
            // success: `killed`, `error`, and anything still running. A `done`
            // is the one state that never needs attention, so it is the one this
            // hides.
            .filter(|a| !self.actions_failures_only || field(a, "/status") != "done")
            .collect()
    }

    /// monitor_rows() groups the whole list at once, so its filter is applied to
    /// the resulting rows, not the raw items.
    /// The storage rows currently drawn — filtered, like every other table.
    ///
    /// `/` on this view used to do nothing at all: the rows were built straight
    /// from the unfiltered list and the title never showed a count, so the filter
    /// was both inert and invisible.
    pub(super) fn visible_storage_rows(&self) -> Vec<Vec<String>> {
        crate::monitor::storage_rows(&self.storage)
            .into_iter()
            .filter(|r| keep(r, &self.filter))
            .collect()
    }

    /// How many rows the Monitor screen is DRAWING, whichever view is showing.
    ///
    /// Three call sites used to work this out independently and disagree.
    /// Navigation counted raw metric entries — which excludes the project header
    /// rows the table inserts — so with 60 metrics in 11 projects the table drew
    /// 71 rows and the cursor stopped at 60: the last eleven could not be reached
    /// at all, filter or no filter.
    pub(super) fn monitor_rows_shown(&self) -> usize {
        match self.monitor_view {
            MonitorView::Services => self.visible_monitor_rows().len(),
            MonitorView::Storage => self.visible_storage_rows().len(),
        }
    }

    pub(super) fn visible_monitor_rows(&self) -> Vec<Vec<String>> {
        self.monitor_table().0
    }

    /// The Monitor's Services rows AS DRAWN, plus how many exist unfiltered.
    ///
    /// One function because there must be one rule: a perf change once gave the
    /// renderer its own inline copy of the filtering (to avoid building the rows
    /// twice), and the two promptly disagreed — the copy that decided what you
    /// SEE kept filtering flat, so fixing the other one changed nothing on screen.
    /// Built once here, so both the rows and the count come from the same pass.
    pub(super) fn monitor_table(&self) -> (Vec<Vec<String>>, usize) {
        let all = crate::monitor::monitor_rows(&self.monitor);
        let total = all.len();
        if self.filter.is_empty() {
            return (all, total);
        }
        // Filtered PER PROJECT, not over a flat list. Filtering the rows
        // independently dropped the project headers — they rarely contain what
        // you typed — leaving orphaned service rows with no way to tell which
        // project each belonged to. Two services called "webapp" in different
        // projects became two identical lines.
        //
        // Same rule the Services table already follows: a matching project keeps
        // all its services, and a matching service keeps its project's header.
        let mut out = Vec::new();
        let mut i = 0;
        while i < all.len() {
            let project_matches = keep(&all[i], &self.filter);
            let mut kept = Vec::new();
            let mut j = i + 1;
            while j < all.len() && all[j].first().is_some_and(|c| c.starts_with("  ")) {
                if project_matches || keep(&all[j], &self.filter) {
                    kept.push(all[j].clone());
                }
                j += 1;
            }
            if project_matches || !kept.is_empty() {
                out.push(all[i].clone());
                out.append(&mut kept);
            }
            i = j;
        }
        (out, total)
    }

    /// Switch screens and load its data if it isn't there yet.
    pub(super) fn goto(&mut self, screen: Screen, req: &Sender<Req>) {
        // The filter belongs to the screen it was typed on. Carrying it to another
        // screen would hide rows for no visible reason.
        self.filter.clear();
        self.filter_input = false;
        // The domain scope only applies to an `o` visit from a service; ordinary
        // navigation clears it (open_service_domains sets it again after goto).
        self.domain_scope = None;
        self.screen = screen;
        match screen {
            Screen::Projects => {
                if self.all_services.is_empty() {
                    let _ = req.send(Req::AllServices);
                }
                // Per-service metrics are joined into the table; without this its
                // columns are "-".
                if self.monitor.is_empty() {
                    let _ = req.send(Req::MonitorData);
                }
                // Swarm replicas → the Status column ("down" for crashed/down ones).
                if self.task_stats.is_empty() {
                    let _ = req.send(Req::TaskStats);
                }
            }
            Screen::Actions => {
                if self.actions.is_empty() {
                    let _ = req.send(Req::Actions);
                }
            }
            Screen::Domains => {
                if self.domains.is_empty() {
                    let _ = req.send(Req::Domains);
                }
            }
            Screen::Monitor => {
                if self.monitor.is_empty() {
                    let _ = req.send(Req::MonitorData);
                }
                if self.storage.is_empty() {
                    let _ = req.send(Req::Storage);
                }
            }
            Screen::Hosts if self.hosts.is_empty() => self.load_hosts = true,
            Screen::Maintenance if self.maint.is_empty() => {
                let _ = req.send(Req::MaintInfo);
            }
            _ => {}
        }
    }

    /// The (name, url) of the server highlighted in the picker.
    pub(super) fn picker_selected(&self) -> Option<(String, String)> {
        self.picker
            .as_ref()
            .and_then(|s| s.selected())
            .and_then(|i| self.all_servers.get(i).cloned())
    }

    /// Open the selected row's PROJECT env in $EDITOR.
    pub(super) fn start_project_env_edit(&mut self) {
        self.edit_project_env = self.selected_project();
    }

    pub(super) fn start_env_edit(&mut self) {
        if let Some((p, s, t)) = self.selected_row() {
            self.edit_env = Some((p, s, t));
        }
    }

    /// The rows shown: a project header followed by its services, filtered.
    ///
    /// Render AND actions must both go through here. If render is filtered while
    /// actions use full-list indices, `x` would delete the wrong service.
    pub(super) fn visible_rows(&self) -> Vec<Line2<'_>> {
        let f = self.filter.to_lowercase();
        let mut names: Vec<&String> = self.projects.iter().collect();
        names.sort();

        // Grouped in ONE pass. This used to rescan every service for every
        // project — O(projects × services) on a path that runs on every frame,
        // which measured 90 ms per frame at 500 services (~11 fps, with keypresses
        // queued behind the redraw). One pass makes it O(services).
        let mut by_project: HashMap<&str, Vec<&Value>> = HashMap::new();
        for s in &self.all_services {
            if let Some(p) = s.get("projectName").and_then(Value::as_str) {
                by_project.entry(p).or_default().push(s);
            }
        }

        let mut out = Vec::new();
        for p in names {
            // A matching project name holds all its contents: searching for
            // "harisenin-net" must show its services, not an empty header.
            let project_matches = f.is_empty() || p.to_lowercase().contains(&f);
            let mut kept: Vec<&Value> = by_project
                .get(p.as_str())
                .map(|v| v.as_slice())
                .unwrap_or_default()
                .iter()
                .copied()
                .filter(|s| project_matches || keep(&service_row(s, None, None), &self.filter))
                .collect();
            kept.sort_by_key(|s| field(s, "/name"));

            if kept.is_empty() && !project_matches {
                continue;
            }
            out.push(Line2::Project {
                name: p,
                services: kept.clone(),
            });
            out.extend(kept.into_iter().map(Line2::Service));
        }
        out
    }

    /// Index of the first SERVICE row in `visible_rows()`. Row 0 is a project
    /// header, which carries no service actions, so that is where a fresh selection
    /// belongs. None when nothing is loaded or everything is filtered out.
    pub(super) fn first_service_row(&self) -> Option<usize> {
        self.visible_rows()
            .iter()
            .position(|r| matches!(r, Line2::Service(_)))
    }

    /// The services that pass the filter, as a flat list.
    ///
    /// Test-only: the screen and every action go through `visible_rows()` (which
    /// also carries the project headers). This flat view exists so the cross-project
    /// filter can be asserted directly, without reconstructing the grouped rows.
    #[cfg(test)]
    pub(super) fn visible_services(&self) -> Vec<&Value> {
        self.all_services
            .iter()
            .filter(|s| keep(&service_row(s, None, None), &self.filter))
            .collect()
    }

    /// The metrics for a service, joined by (projectName, serviceName).
    ///
    /// getAllServicesStats carries more entries than the service list (system
    /// services, compose sub-services), so ones that don't match are ignored.
    /// (actual, desired) swarm replicas for a service, from getDockerTaskStats.
    /// None = not loaded yet or the service has no swarm task.
    pub(super) fn replicas(&self, project: &str, service: &str) -> Option<(i64, i64)> {
        self.task_stats
            .get(&format!("{project}_{service}"))
            .copied()
    }

    /// The number of services currently down (desired>0 but actual<desired).
    pub(super) fn down_count(&self) -> usize {
        self.all_services
            .iter()
            .filter(|s| {
                matches!(
                    self.replicas(&field(s, "/projectName"), &field(s, "/name")),
                    Some((a, d)) if d > 0 && a < d
                )
            })
            .count()
    }

    /// Whether this service has a deployment CURRENTLY running (pending/running),
    /// from listActions. The Status column uses it to show "deploying" — without
    /// it, the old container keeps running so the row reads "active" and the user
    /// presses deploy again without knowing the previous one hasn't finished.
    /// A live-verified status: pending → running → done/error.
    pub(super) fn is_deploying(&self, project: &str, service: &str) -> bool {
        self.actions.iter().any(|a| {
            field(a, "/type") == "deployment"
                && matches!(field(a, "/status").as_str(), "pending" | "running")
                && field(a, "/projectName") == project
                && field(a, "/serviceName") == service
        })
    }

    /// The number of services with a running deployment (for the table title).
    pub(super) fn deploying_count(&self) -> usize {
        self.all_services
            .iter()
            .filter(|s| self.is_deploying(&field(s, "/projectName"), &field(s, "/name")))
            .count()
    }

    /// Metrics keyed by (project, service), built ONCE for a frame.
    ///
    /// Looking each row's metrics up by scanning the whole list — which the
    /// Services table did two or three times per row — is O(services²) on a path
    /// that runs every frame. At 500 services that measured 90 ms per frame: the
    /// table redrew about eleven times a second, with keypresses queued behind it.
    pub(super) fn metric_index(&self) -> HashMap<(&str, &str), &Value> {
        self.monitor
            .iter()
            .filter_map(|m| {
                Some((
                    (
                        m.get("projectName").and_then(Value::as_str)?,
                        m.get("serviceName").and_then(Value::as_str)?,
                    ),
                    m,
                ))
            })
            .collect()
    }

    /// The (project, service) pairs with a deployment in flight, built once for a
    /// frame. `is_deploying` scans every action for every row it is asked about.
    pub(super) fn deploying_index(&self) -> std::collections::HashSet<(&str, &str)> {
        self.actions
            .iter()
            .filter(|a| {
                a.get("type").and_then(Value::as_str) == Some("deployment")
                    && matches!(
                        a.get("status").and_then(Value::as_str),
                        Some("pending") | Some("running")
                    )
            })
            .filter_map(|a| {
                Some((
                    a.get("projectName").and_then(Value::as_str)?,
                    a.get("serviceName").and_then(Value::as_str)?,
                ))
            })
            .collect()
    }

    /// (project, service, type) — only when the highlighted row is a SERVICE. A
    /// project header returns None, so service actions (logs/deploy/delete) are
    /// never run on a nonexistent service.
    pub(super) fn selected_row(&self) -> Option<(String, String, String)> {
        match self.visible_rows().get(self.services_table.selected()?)? {
            Line2::Service(s) => Some((
                field(s, "/projectName"),
                field(s, "/name"),
                field(s, "/type"),
            )),
            Line2::Project { .. } => None,
        }
    }

    /// The selected service, whole — selected_row() only gives its identity, and
    /// some actions need its contents (e.g. the current autoDeploy).
    pub(super) fn selected_service(&self) -> Option<&Value> {
        match self.visible_rows().get(self.services_table.selected()?)? {
            Line2::Service(s) => Some(*s),
            Line2::Project { .. } => None,
        }
    }

    /// Flip the selected service's auto deploy.
    /// Close the terminal session (Ctrl-Q). Dropping the input channel → the WS
    /// thread closes the socket; back to Services immediately.
    /// Move through the terminal's scrollback. Positive = back into history.
    ///
    /// Clamped to what actually exists, so holding the key stops at the oldest
    /// line rather than scrolling into blank space.
    pub(super) fn term_scroll(&mut self, delta: isize) {
        let Some(p) = self.term.parser.as_mut() else {
            return;
        };
        // vt100 clamps the far end to the history it actually holds, so only the
        // near end needs guarding — holding the key stops at the newest line
        // instead of wrapping past it.
        let at = p.screen().scrollback() as isize;
        p.set_scrollback((at + delta).max(0) as usize);
    }

    pub(super) fn close_terminal(&mut self) {
        self.term.input = None;
        self.term.parser = None;
        self.screen = Screen::Projects;
        self.status = format!("Terminal {} closed", self.term.title);
    }

    pub(super) fn toggle_auto_deploy(&mut self, req: &Sender<Req>) {
        let picked = self.selected_service().map(|s| {
            (
                field(s, "/projectName"),
                field(s, "/name"),
                // None = no auto deploy at all (database, image source), not "off".
                // Offering a toggle there would only draw an error from the server.
                match field(s, "/source/type").as_str() {
                    "github" => s.pointer("/source/autoDeploy").and_then(Value::as_bool),
                    _ => None,
                },
            )
        });
        match picked {
            None => self.status = "Select a service first".into(),
            Some((_, _, None)) => {
                self.status = "Auto deploy only exists on services with a GitHub source".into()
            }
            Some((project, service, Some(on))) => {
                self.status = format!(
                    "{} auto deploy for {service}...",
                    if on { "Turning off" } else { "Turning on" }
                );
                let _ = req.send(Req::AutoDeploy {
                    project,
                    service,
                    on: !on,
                });
            }
        }
    }

    /// The project name of the highlighted row, whether header or service. Used by
    /// actions that work on a PROJECT: create a service, delete a project.
    pub(super) fn selected_project(&self) -> Option<String> {
        match self.visible_rows().get(self.services_table.selected()?)? {
            Line2::Project { name, .. } => Some((*name).to_string()),
            Line2::Service(s) => Some(field(s, "/projectName")),
        }
    }

    /// The services of `project` that a deploy would pick the new env up on.
    ///
    /// A database or a box has no build step, so it cannot be deployed at all —
    /// offering to deploy them would send a request that can only 404. The same
    /// list is what the confirmation counts, so the number the user approves is
    /// the number that actually gets deployed.
    pub(super) fn deployable_in(&self, project: &str) -> Vec<(String, String, String)> {
        self.all_services
            .iter()
            .filter(|s| field(s, "/projectName") == project)
            .map(|s| {
                (
                    field(s, "/projectName"),
                    field(s, "/name"),
                    field(s, "/type"),
                )
            })
            .filter(|(_, _, t)| crate::lifecycle::ops(t, "deploy").is_some())
            .collect()
    }

    /// The check on display row `i`, in the order the screen ranks them.
    pub(super) fn watched_row(&self, i: usize) -> Option<&crate::uptime::Check> {
        crate::uptime::ranked(&self.watch, &self.probes)
            .get(i)
            .map(|(c, _)| *c)
    }

    /// Open the form that decides what this URL is checked WITH.
    ///
    /// The one door to a check, whether it exists yet or not: from Domains it
    /// enrols, from Uptime it edits. Enrolling used to happen instantly with a
    /// silent GET, leaving the method and body to be set on another screen
    /// afterwards — two doors into one room, and a deliberate act made without
    /// the user deciding anything.
    pub(super) fn open_check_form(&mut self, url: &str) {
        let existing = self.watch.iter().find(|c| c.url == url).cloned();
        let title = match &existing {
            Some(_) => format!(" Check: {url} "),
            None => format!(" Watch {url} "),
        };
        let check = existing.unwrap_or_else(|| crate::uptime::Check::get(url));
        self.form = Some(Form::new(
            FormKind::CheckEdit {
                url: url.to_string(),
            },
            title,
            check_fields(&check),
        ));
    }

    /// Ask every watched domain at once.
    pub(super) fn run_checks(&mut self, req: &Sender<Req>) {
        if self.watch.is_empty() {
            self.status = "Nothing is being watched — press w on a domain to add one".into();
            return;
        }
        self.checking = true;
        self.status = format!("Checking {} domain(s)...", self.watch.len());
        let _ = req.send(Req::RunChecks(self.watch.clone()));
    }

    /// Every (project, service) on this host, or `None` while the list is still
    /// loading.
    ///
    /// `None` is the important case: judging a domain against an empty list would
    /// mark all 713 of them dead at once. An empty list is treated as "not
    /// loaded" rather than "no services", because a panel with domains and no
    /// services does not happen, and the safe direction is to say nothing.
    pub(super) fn live_services(&self) -> Option<std::collections::HashSet<(String, String)>> {
        if self.all_services.is_empty() {
            return None;
        }
        Some(
            self.all_services
                .iter()
                .map(|s| (field(s, "/projectName"), field(s, "/name")))
                .collect(),
        )
    }

    /// The domains that pass the filter.
    ///
    /// Render AND actions (e/x/P) must both go through here. If render is filtered
    /// while actions use full-list indices, `x` would delete the wrong domain.
    pub(super) fn visible_domains(&self) -> Vec<&Value> {
        self.domains
            .iter()
            .filter(|d| keep(&crate::domains::domain_row(d), &self.filter))
            .collect()
    }

    /// Show what a bulk rewrite WOULD do, before anything is sent.
    ///
    /// The rewrite hits the domains currently on screen, which is what the filter
    /// is for: `/api` then a rewrite acts on those and nothing else. So the
    /// preview names them one per line — a count alone ("rewrite 12 domains?")
    /// asks the user to approve a list they cannot see.
    pub(super) fn preview_domain_edits(&mut self, target: &str, find: &str, replace: &str) {
        let plan = match crate::domains::plan(&self.visible_domains(), target, find, replace) {
            // The form STAYS open on a rejected rewrite: the fix is one character
            // in a box the user has already filled in, not a form to retype.
            Err(msg) => {
                self.status = format!("⚠ {msg}");
                return;
            }
            Ok(plan) => plan,
        };
        self.form = None;
        if plan.is_empty() {
            self.status = format!("No domain on screen has '{find}' in its {target}");
            return;
        }
        let mut lines = vec![
            format!("Rewriting the {target} of {} domain(s):", plan.len()),
            String::new(),
        ];
        // Rewriting a destination gives every domain the SAME before → after, so
        // the host is what tells the lines apart. When the host IS the rewrite it
        // is already on the line, and repeating it would only be noise.
        lines.extend(plan.iter().map(|c| match target {
            "host" => format!("{}  →  {}", c.before, c.after),
            _ => format!("{}:  {}  →  {}", c.host, c.before, c.after),
        }));
        self.domain_edits = plan;
        self.viewer.from = Screen::Domains;
        self.show_viewer(" Bulk domain edit — preview ".into(), lines);
        self.viewer.ctx = None;
        self.status = format!(
            "Nothing sent yet — [Enter] rewrites these {} domain(s) · [Esc] cancels",
            self.domain_edits.len()
        );
    }

    /// Send the previewed rewrite.
    pub(super) fn apply_domain_edits(&mut self, req: &Sender<Req>) {
        let changes = std::mem::take(&mut self.domain_edits)
            .into_iter()
            // The host is what names a domain in a failure report — `before` is
            // the same string for every domain when a destination is rewritten.
            .map(|c| (c.id, c.host, c.body))
            .collect();
        let _ = req.send(Req::DomainBulkEdit { changes });
        self.screen = Screen::Domains;
        self.status = "Sending...".into();
    }

    /// Load the list of services for the project currently selected in the form, so
    /// the Service field becomes a real choice rather than free text.
    pub(super) fn load_form_services(&mut self, req: &Sender<Req>) {
        if let Some(form) = self.form.as_ref() {
            let project = form.by_label("Project");
            if !project.is_empty() {
                let _ = req.send(Req::ServicesFor(project));
            }
        }
    }

    /// Request the data for the source/build form of the selected service.
    ///
    /// The form only opens once inspectService arrives (see Resp::ConfigForm),
    /// because the current values must be its initial contents.
    pub(super) fn open_config_form(&mut self, build: bool, req: &Sender<Req>) {
        let Some((project, service, stype)) = self.selected_row() else {
            return;
        };
        // Source/build only exists on app-type services; other types have no such concept.
        if stype != "app" {
            self.status = format!("Source & build is only for app services (this is {stype})");
            return;
        }
        let _ = req.send(Req::ConfigForm {
            project,
            service,
            build,
        });
        self.status = "Loading...".into();
    }

    /// Open the add-mount form for the highlighted service.
    pub(super) fn open_mount_form(&mut self) {
        let Some((project, service, stype)) = self.selected_row() else {
            self.status = "Select a service first".into();
            return;
        };
        if !self.allows_mounts_and_ports(&stype) {
            return;
        }
        self.form = Some(
            Form::new(
                FormKind::MountCreate { project, service },
                " New mount ",
                mount_fields(None),
            )
            .with_note("to delete one instead: 'm', then its digit"),
        );
    }

    /// Manage a service's domains: open the Domains tab filtered to that service.
    /// Reuses the full domain CRUD (n new · e edit · x delete · P primary) instead
    /// of a read-only viewer. The filter matches the destination
    /// "protocol://{project}_{service}:…".
    pub(super) fn open_service_domains(&mut self, req: &Sender<Req>) {
        let Some((project, service, _)) = self.selected_row() else {
            self.status = "Select a service first".into();
            return;
        };
        // goto clears the filter & scope first, so set them AFTER it.
        self.goto(Screen::Domains, req);
        self.filter = format!("{project}_{service}");
        self.domain_scope = Some((project.clone(), service.clone()));
        self.status = format!("Domain {project}/{service} · n new · e edit · x delete · P primary");
    }

    /// Open the clone form for the highlighted service. The new name is suggested
    /// as "{svc}-copy".
    pub(super) fn open_clone_form(&mut self) {
        let Some((project, service, stype)) = self.selected_row() else {
            self.status = "Select a service first".into();
            return;
        };
        let suggested = format!("{service}-copy");
        // Target project: a dropdown of EXISTING projects (default: the source
        // project). Existing only — a brand-new project's network isn't ready at
        // createService time.
        let mut projects = self.projects.clone();
        projects.sort();
        let fields = vec![
            Field::choice_owned("Project", projects, &project),
            Field::text("New name", &suggested),
        ];
        self.form = Some(
            Form::new(
                FormKind::CloneService {
                    project,
                    service,
                    stype,
                },
                " Clone service ",
                fields,
            )
            .with_note("copies the config, NOT the data"),
        );
    }

    /// Show everything known about the selected host — above all, the WHOLE reason
    /// an unreachable one is unreachable.
    ///
    /// The Status cell truncates that reason to a few words, and Hosts is the
    /// screen you are on precisely when something is broken: seeing "DOWN — error
    /// sen" with no way to read the rest is a dead end at the worst moment.
    pub(super) fn open_host_detail(&mut self) {
        let Some(h) = self.hosts_state.selected().and_then(|i| self.hosts.get(i)) else {
            self.status = "Select a host first".into();
            return;
        };
        let mut lines = vec![
            format!("Server    {}", h.name),
            format!("URL       {}", h.url),
            String::new(),
        ];
        match &h.state {
            HostState::Loading => lines.push("Still loading…".into()),
            HostState::Err(e) => {
                lines.push("UNREACHABLE".into());
                lines.push(String::new());
                // Wrapped to the pane: the viewer neither wraps nor scrolls
                // sideways, so an unwrapped error would be cut at the edge — the
                // very thing this screen exists to undo.
                // Floored, because table_area is zero until the first paint and a
                // width of 0 would wrap every word onto its own line.
                let w = (self.table_area.width as usize).saturating_sub(2).max(40);
                for line in e.lines() {
                    lines.extend(super::render::wrap_words(line, w));
                }
            }
            HostState::Ok(v) => {
                let pair = |used: &str, total: &str| {
                    format!(
                        "{} / {}",
                        crate::output::format_bytes(crate::output::num(v, used)),
                        crate::output::format_bytes(crate::output::num(v, total))
                    )
                };
                lines.push("Reachable".into());
                lines.push(String::new());
                // The full figures, not the halves the narrow table has room for.
                lines.push(format!(
                    "CPU       {:.1}%",
                    crate::output::series_last(v, "cpu")
                ));
                lines.push(format!(
                    "Memory    {}",
                    pair("/memoryUsedBytes", "/memoryTotalBytes")
                ));
                lines.push(format!(
                    "Disk      {}",
                    pair("/diskUsedBytes", "/diskTotalBytes")
                ));
                lines.push(format!("Load      {}", crate::monitor::load_avg(v)));
            }
        }
        self.viewer.title = format!("Host · {}", h.name);
        self.viewer.lines = lines;
        self.viewer.scroll = 0;
        self.viewer.hscroll = 0;
        self.viewer.from = Screen::Hosts;
        self.screen = Screen::Viewer;
    }

    /// Open the migrate form — one service, or every service in the highlighted
    /// project when `whole_project`.
    ///
    /// Migration needs somewhere to migrate TO, so a single-host setup gets told
    /// how to add one instead of an empty dropdown it can't act on.
    pub(super) fn open_migrate_form(&mut self, whole_project: bool) {
        let others: Vec<String> = self
            .all_servers
            .iter()
            .map(|(n, _)| n.clone())
            .filter(|n| *n != self.server_name)
            .collect();
        if others.is_empty() {
            self.status =
                "No other server configured — add one on the Hosts screen (h) first".into();
            return;
        }

        let (title, project, service, stype, count) = if whole_project {
            let Some(project) = self.selected_project() else {
                self.status = "Select a project or a service first".into();
                return;
            };
            let n = self.project_services(&project).len();
            if n == 0 {
                self.status = format!("'{project}' has no services to migrate");
                return;
            }
            (
                " Migrate project ".to_string(),
                project,
                String::new(),
                String::new(),
                n,
            )
        } else {
            let Some((project, service, stype)) = self.selected_row() else {
                self.status = "Select a service first".into();
                return;
            };
            (" Migrate service ".to_string(), project, service, stype, 1)
        };

        let fields = vec![
            Field::choice_owned("To server", others, ""),
            // Free text, not a dropdown: the destination's projects live on
            // another host that hasn't been contacted yet, and it's created there
            // if it doesn't exist.
            Field::text("Target project", &project),
        ];
        let what = if count == 1 {
            "1 service".to_string()
        } else {
            format!("{count} services")
        };
        self.form = Some(
            Form::new(
                FormKind::Migrate {
                    project,
                    service,
                    stype,
                },
                &title,
                fields,
            )
            // The count and the data warning must survive the whole edit: this is
            // the last screen before services are created on another host.
            .with_note(format!("{what} · config only, NO data")),
        );
    }

    /// Open the "compare with another host" form: pick which OTHER configured
    /// server to fetch the same project/service from, and diff the two.
    ///
    /// The engine is the same `crate::services::diff` the marked-pair compare
    /// uses; only the second service comes from a different host's client. Needs
    /// a second server, so a single-host setup is told how to add one rather than
    /// shown an empty dropdown.
    pub(super) fn open_diff_across_form(&mut self) {
        let others: Vec<String> = self
            .all_servers
            .iter()
            .map(|(n, _)| n.clone())
            .filter(|n| *n != self.server_name)
            .collect();
        if others.is_empty() {
            self.status =
                "No other server configured — add one on the Hosts screen (s) first".into();
            return;
        }
        let Some((project, service, stype)) = self.selected_row() else {
            self.status = "Select a service first".into();
            return;
        };
        let fields = vec![Field::choice_owned("On server", others, "")];
        self.form = Some(
            Form::new(
                FormKind::DiffAcross {
                    project: project.clone(),
                    service: service.clone(),
                    stype,
                },
                format!(" Compare {project}/{service} with another host "),
                fields,
            )
            .with_note("Compares the SAME project/service on the chosen host".to_string()),
        );
    }

    /// Open the "compare whole project with another host" form. The project is
    /// the selected one, or the project of the selected service.
    pub(super) fn open_diff_project_across_form(&mut self) {
        let others: Vec<String> = self
            .all_servers
            .iter()
            .map(|(n, _)| n.clone())
            .filter(|n| *n != self.server_name)
            .collect();
        if others.is_empty() {
            self.status =
                "No other server configured — add one on the Hosts screen (s) first".into();
            return;
        }
        let project = self
            .selected_project()
            .or_else(|| self.selected_row().map(|(p, ..)| p));
        let Some(project) = project else {
            self.status = "Select a project or a service first".into();
            return;
        };
        let fields = vec![Field::choice_owned("On server", others, "")];
        self.form = Some(
            Form::new(
                FormKind::DiffProjectAcross {
                    project: project.clone(),
                },
                format!(" Compare project {project} with another host "),
                fields,
            )
            .with_note("Compares every service in the project, both ways".to_string()),
        );
    }

    /// Every service belonging to `project`, as (project, service, type).
    pub(super) fn project_services(&self, project: &str) -> Vec<(String, String, String)> {
        self.all_services
            .iter()
            .filter(|s| field(s, "/projectName") == project)
            .map(|s| {
                (
                    field(s, "/projectName"),
                    field(s, "/name"),
                    field(s, "/type"),
                )
            })
            .collect()
    }

    /// Compare the two marked services. The menu only offers this with exactly
    /// two marked, so a wrong count means the marks changed under the menu — say
    /// so rather than diffing the wrong pair.
    pub(super) fn diff_marked(&mut self, req: &Sender<Req>) {
        let t = self.bulk_targets();
        let [a, b] = <[(String, String, String); 2]>::try_from(t)
            .ok()
            .unwrap_or_else(|| {
                [
                    (String::new(), String::new(), String::new()),
                    (String::new(), String::new(), String::new()),
                ]
            });
        if a.1.is_empty() || b.1.is_empty() {
            self.status = "Mark exactly two services to compare".into();
            return;
        }
        let _ = req.send(Req::DiffServices { a, b });
        // The marks have done their job. Left set, the global "Esc clears marks"
        // handler shadows the viewer's own Esc, so leaving the diff took two
        // presses and read as a dead end. Bulk actions clear their marks for the
        // same reason.
        self.marked.clear();
        self.status = "Comparing...".into();
    }

    /// Ask before running `action` on every marked service.
    ///
    /// The confirmation NAMES them (up to a few) instead of only counting: marks
    /// are made over time and scroll off screen, so "Restart 12 services?" asks
    /// the user to approve a set they can no longer see.
    pub(super) fn open_bulk_confirm(&mut self, action: &str, force: bool) {
        let targets = self.bulk_targets();
        if targets.is_empty() {
            self.status = "Mark some services first — [v] marks the row".into();
            return;
        }
        const NAMED: usize = 5;
        let mut names: Vec<String> = targets
            .iter()
            .take(NAMED)
            .map(|(p, s, _)| format!("{p}/{s}"))
            .collect();
        if targets.len() > NAMED {
            names.push(format!("and {} more", targets.len() - NAMED));
        }
        let verb = if force {
            "Force rebuild".to_string()
        } else {
            cap(action)
        };
        self.confirm = Some(Confirm {
            action: format!("bulk-{action}"),
            project: String::new(),
            service: String::new(),
            // Reusing `stype` as the force flag, the same way port/mount delete
            // stash an index in it.
            stype: if force { "force".into() } else { String::new() },
            label: format!("{verb} {} services? {}", targets.len(), names.join(", ")),
        });
    }

    /// Does the viewer show selectable ROWS rather than prose?
    ///
    /// ONE definition: `keys` uses it twice (wheel, movement keys) and `render`
    /// once to pick a Table over a Paragraph. They each carried their own copy
    /// derived from `viewer.ctx`, so the restore picker — which has no
    /// viewer.ctx — would have rendered as a selectable list in one place and
    /// scrolled like prose in another.
    pub(super) fn viewer_is_collection(&self) -> bool {
        self.backups.restore_into.is_some()
            || self.backups.r2_restore_into.is_some()
            || self.backups.backup_from.is_some()
            || self
                .viewer
                .ctx
                .as_ref()
                .is_some_and(|(v, ..)| v.is_collection())
    }

    /// Back the selected database up once, into the panel's storage provider.
    /// Non-locking dump of the chosen databases straight to object storage — the
    /// same path as the CLI `db dump`, offered in the TUI so the backup here is not
    /// stuck on EasyPanel's locking native backup. Reuses the database picker; the
    /// remote provider is resolved by the worker (a dump must be remote to be
    /// useful), so unlike `backup_now` it does not pre-pick one.
    pub(super) fn dump_r2_now(&mut self, req: &Sender<Req>) {
        let Some((project, service, stype)) = self.selected_row() else {
            self.status = "Select a database first".into();
            return;
        };
        self.backups.r2_mode = true;
        self.backups.provider = None;
        let _ = req.send(Req::DatabasesIn {
            project,
            service,
            stype,
        });
        self.status = "Reading the databases in this service...".into();
    }

    pub(super) fn backup_now(&mut self, req: &Sender<Req>) {
        self.backups.r2_mode = false;
        let Some((project, service, stype)) = self.selected_row() else {
            self.status = "Select a database first".into();
            return;
        };
        let Some((id, name, ptype)) = crate::backup::preferred_provider(&self.backups.providers)
        else {
            self.status =
                "No storage provider configured — add one in the EasyPanel dashboard first".into();
            return;
        };
        // Which provider a backup lands on decides whether it can EVER be
        // restored onto another host, so it is named before anything runs.
        self.backups.provider = Some((
            id.clone(),
            if crate::backup::is_remote(ptype) {
                format!("{name} — restorable on any host sharing it")
            } else {
                format!("{name} — stays on THIS host, cannot be restored elsewhere")
            },
        ));
        // A service holds many databases; the panel records only the one it
        // created. Ask the engine what is actually in there rather than assuming.
        let _ = req.send(Req::DatabasesIn {
            project,
            service,
            stype,
        });
        self.status = "Reading the databases in this service...".into();
    }

    /// Ask which database to back up, once the engine has said what it holds.
    ///
    /// "All databases" leads, because backing up everything is the common intent
    /// and doing it by hand meant repeating the whole flow per schema.
    fn open_backup_picker(&mut self, project: String, service: String, names: Vec<String>) {
        // A native backup names its provider up front; a non-locking dump resolves
        // the remote provider later (in the worker), so it has none picked here.
        let (header_verb, where_to) = if self.backups.r2_mode {
            ("Dump", "object storage (non-locking, one file)".to_string())
        } else {
            match self.backups.provider.clone() {
                Some((_, w)) => ("Back up", w),
                None => return,
            }
        };
        // Nothing to choose from is an answer, not an empty box: it means the
        // engine could not be asked and the panel recorded no database either.
        if names.is_empty() {
            self.status = format!("No database found in {project}/{service} — nothing to back up");
            return;
        }
        self.backups.names = names;
        self.backups.marked.clear();
        self.backups.header = format!("{header_verb} from {project}/{service} to {where_to}");
        self.backups.backup_from = Some((project, service));
        let lines = self.backups.picker_lines();
        self.show_picker("Which database?".into(), lines);
        self.status = self.backups.hint();
    }

    /// Tick or untick the database under the cursor.
    /// The `[n]` of the picker row under the cursor.
    ///
    /// Read from the PRINTED marker, never from the cursor's position: both
    /// pickers carry heading lines above their rows, so position 0 is a label.
    /// The same contract the collections use, and the reason a delete once
    /// offered `[13]` of 12.
    pub(super) fn picker_row(&self) -> Option<usize> {
        self.viewer
            .row
            .selected()
            .and_then(|r| self.viewer.lines.get(r))
            .and_then(|l| row_index(l))
    }

    pub(super) fn toggle_backup_mark(&mut self) {
        // Row 0 is "All", which is not a database to tick — `toggle` says so.
        if !self.picker_row().is_some_and(|i| self.backups.toggle(i)) {
            self.status = "Move to a database row first".into();
            return;
        }
        // Rebuild in place: the selection must not jump because a tick appeared.
        let keep = self.viewer.row.selected();
        self.viewer.lines = self.backups.picker_lines();
        self.viewer.row.select(keep);
        self.status = self.backups.hint();
    }

    /// Confirm the backup/dump of whatever the picker has selected.
    pub(super) fn ask_backup(&mut self) {
        let Some((project, service)) = self.backups.backup_from.clone() else {
            return;
        };
        let Some(i) = self.picker_row() else {
            self.status = "Select a database row first".into();
            return;
        };
        let chosen = self.backups.chosen(i);
        if chosen.is_empty() {
            return;
        }
        let what = if chosen.len() == 1 {
            format!("'{}'", chosen[0])
        } else {
            format!("{} databases ({})", chosen.len(), chosen.join(", "))
        };
        self.backups.pending = chosen;
        self.confirm = Some(if self.backups.r2_mode {
            // A non-locking dump: the remote provider is resolved by the worker, so
            // no provider id rides along in `stype`.
            Confirm {
                action: "r2dump".into(),
                project,
                service,
                stype: String::new(),
                label: format!("Dump {what} to object storage — non-locking, one file?"),
            }
        } else {
            let Some((provider, where_to)) = self.backups.provider.clone() else {
                return;
            };
            Confirm {
                action: "backup".into(),
                project,
                service,
                stype: provider,
                label: format!("Back {what} up to {where_to}?"),
            }
        });
    }

    /// Choose another server whose backups can be restored into this service.
    ///
    /// Only worth offering when this panel has a REMOTE provider: a backup taken
    /// elsewhere has to be readable from here, and a local-disk provider by
    /// definition is not.
    pub(super) fn open_restore_from(&mut self) {
        let Some((project, service, _)) = self.selected_row() else {
            self.status = "Select a database first".into();
            return;
        };
        let others: Vec<String> = self
            .all_servers
            .iter()
            .map(|(n, _)| n.clone())
            .filter(|n| *n != self.server_name)
            .collect();
        if others.is_empty() {
            self.status = "No other server configured — add one on the Hosts screen first".into();
            return;
        }
        if !self
            .backups
            .providers
            .iter()
            .any(|(_, _, t)| crate::backup::is_remote(t))
        {
            self.status =
                "This host has only local-disk storage, so it cannot read another host's backups"
                    .into();
            return;
        }
        let fields = vec![Field::choice_owned("Server", others.clone(), &others[0])];
        self.form = Some(
            Form::new(
                FormKind::RestoreFrom { project, service },
                " Restore from another server ",
                fields,
            )
            .with_note(
                "lists that server's backups; only ones on shared remote storage".to_string(),
            ),
        );
    }

    /// Open the list of backups that can be restored INTO the selected service.
    pub(super) fn open_restore(&mut self, req: &Sender<Req>) {
        let Some((project, service, _)) = self.selected_row() else {
            self.status = "Select a database first".into();
            return;
        };
        let _ = req.send(Req::BackupHistory { project, service });
        self.status = "Reading backup history...".into();
    }

    /// Ask before restoring the backup under the cursor.
    ///
    /// A restore OVERWRITES the target database, so the confirmation names the
    /// file, the database and the service it is going into — all three, because
    /// restoring the right file into the wrong service is the mistake worth
    /// preventing.
    pub(super) fn ask_restore(&mut self) {
        let Some((project, service)) = self.backups.restore_into.clone() else {
            return;
        };
        let Some(i) = self.picker_row() else {
            self.status = "Select a backup row first".into();
            return;
        };
        let Some((database, provider, path)) = self.backups.files.get(i).cloned() else {
            return;
        };
        self.backups.pending_restore = Some((database.clone(), provider, path.clone()));
        self.confirm = Some(Confirm {
            action: "restore".into(),
            project,
            service,
            stype: String::new(),
            label: format!(
                "Restore '{database}' from {path}? This REPLACES the data currently in it."
            ),
        });
    }

    /// Open the list of THIS tool's own object-storage dumps for the service, to
    /// restore one — the other half of the non-locking `db dump`, so the TUI is not
    /// stuck restoring only through the CLI.
    pub(super) fn open_r2_restore(&mut self, req: &Sender<Req>) {
        let Some((project, service, _)) = self.selected_row() else {
            self.status = "Select a database first".into();
            return;
        };
        let _ = req.send(Req::R2Dumps { project, service });
        self.status = "Looking for dumps in object storage...".into();
    }

    /// Ask before restoring the object-storage dump under the cursor. It recreates
    /// and OVERWRITES the databases the dump holds, so the confirmation says so.
    pub(super) fn ask_r2_restore(&mut self) {
        let Some((project, service)) = self.backups.r2_restore_into.clone() else {
            return;
        };
        let Some(i) = self.picker_row() else {
            self.status = "Select a dump row first".into();
            return;
        };
        let Some(key) = self.backups.r2_dumps.get(i).cloned() else {
            return;
        };
        self.backups.pending_r2_restore = Some(key.clone());
        self.confirm = Some(Confirm {
            action: "r2restore".into(),
            project,
            service,
            stype: String::new(),
            label: format!(
                "Restore dump '{key}'? It recreates and OVERWRITES the databases in it."
            ),
        });
    }

    /// Is this service marked for a bulk action?
    pub(super) fn is_marked(&self, project: &str, service: &str) -> bool {
        self.marked
            .contains(&(project.to_string(), service.to_string()))
    }

    /// Toggle the mark under the cursor.
    ///
    /// On a project header this covers the whole project, which is the common
    /// case ("restart everything here") and needs no per-row work. It marks all
    /// of them unless they are ALREADY all marked, in which case it clears them
    /// — otherwise a second press on a fully marked project would do nothing and
    /// read as a dead key.
    pub(super) fn toggle_mark(&mut self) {
        if let Some((project, service, _)) = self.selected_row() {
            if !self.marked.remove(&(project.clone(), service.clone())) {
                self.marked.insert((project, service));
            }
        } else if let Some(project) = self.selected_project() {
            let kids: Vec<(String, String)> = self
                .project_services(&project)
                .into_iter()
                .map(|(p, s, _)| (p, s))
                .collect();
            if kids.is_empty() {
                self.status = format!("{project} has no services to mark");
                return;
            }
            if kids.iter().all(|k| self.marked.contains(k)) {
                self.marked.retain(|k| !kids.contains(k));
            } else {
                self.marked.extend(kids);
            }
        } else {
            self.status = "Select a row first".into();
            return;
        }
        self.report_marks();
    }

    /// Mark every service the filter currently shows — the third way of choosing
    /// a set: narrow the table, then take what is left. Marks everything unless
    /// it is all marked already, so the same key clears it again.
    pub(super) fn mark_all_visible(&mut self) {
        let shown: Vec<(String, String)> = self
            .visible_rows()
            .iter()
            .filter_map(|l| match l {
                Line2::Service(s) => Some((field(s, "/projectName"), field(s, "/name"))),
                Line2::Project { .. } => None,
            })
            .collect();
        if shown.is_empty() {
            self.status = "Nothing to mark".into();
            return;
        }
        if shown.iter().all(|k| self.marked.contains(k)) {
            self.marked.retain(|k| !shown.contains(k));
        } else {
            self.marked.extend(shown);
        }
        self.report_marks();
    }

    /// Say what is marked. A mark is a small ✓ far from the cursor, so a count in
    /// the status line is the only feedback that survives a long table.
    fn report_marks(&mut self) {
        self.status = match self.marked.len() {
            0 => "No services marked".into(),
            n => format!("{n} service(s) marked — [Space] to act on them, [Esc] to clear"),
        };
    }

    /// The services a bulk action would hit, with their CURRENT type.
    ///
    /// Marks are looked up against the live service list rather than trusted, so
    /// a service destroyed elsewhere (or by an earlier bulk run) silently drops
    /// out instead of sending a call for something that no longer exists.
    /// Sorted, so the confirmation lists them in the order the table shows.
    pub(super) fn bulk_targets(&self) -> Vec<(String, String, String)> {
        let mut out: Vec<(String, String, String)> = self
            .all_services
            .iter()
            .map(|s| {
                (
                    field(s, "/projectName"),
                    field(s, "/name"),
                    field(s, "/type"),
                )
            })
            .filter(|(p, s, _)| self.is_marked(p, s))
            .collect();
        out.sort();
        out
    }

    /// Refuse a mounts/ports action on a type that has neither.
    ///
    /// The menu hides these, but the leaf keys (`p`, `M`) stay live — the exact
    /// gap that left the Lifecycle menu wrong for databases until v0.52.0.
    /// Returns true when the action may proceed.
    pub(super) fn allows_mounts_and_ports(&mut self, stype: &str) -> bool {
        if crate::lifecycle::has_mounts_and_ports(stype) {
            return true;
        }
        self.status =
            format!("A {stype} service has no mounts or ports — EasyPanel manages its storage");
        false
    }

    /// Open the deploy form (replicas, start command, zero-downtime).
    ///
    /// `updateDeploy` exists only for `app` — every other type answers the bare
    /// 404 of a missing route — which is the same rule that already gates source
    /// and build.
    pub(super) fn open_deploy_form(&mut self, req: &Sender<Req>) {
        let Some((project, service, stype)) = self.selected_row() else {
            self.status = "Select a service first".into();
            return;
        };
        if stype != "app" {
            self.status = format!("Replicas are an app setting (this is {stype})");
            return;
        }
        let _ = req.send(Req::DeployForm { project, service });
        self.status = "Loading...".into();
    }

    /// Open the add-redirect form for the highlighted web service.
    pub(super) fn open_redirect_form(&mut self) {
        let Some((project, service, stype)) = self.selected_row() else {
            self.status = "Select a service first".into();
            return;
        };
        if !matches!(stype.as_str(), "app" | "box" | "compose" | "wordpress") {
            self.status = format!("Redirect is only for web services (this is {stype})");
            return;
        }
        self.form = Some(
            Form::new(
                FormKind::RedirectCreate {
                    project,
                    service,
                    stype,
                },
                " New redirect ",
                redirect_fields(),
            )
            .with_note("to delete one instead: 'f', then its digit"),
        );
    }

    /// Open the basic auth form for the highlighted service. Only web services
    /// (app/box/compose/wordpress) have this endpoint; DBs aren't relevant.
    pub(super) fn open_basic_auth_form(&mut self, req: &Sender<Req>) {
        let Some((project, service, stype)) = self.selected_row() else {
            self.status = "Select a service first".into();
            return;
        };
        if !matches!(stype.as_str(), "app" | "box" | "compose" | "wordpress") {
            self.status = format!("Basic auth is only for web services (this is {stype})");
            return;
        }
        let _ = req.send(Req::BasicAuthForm {
            project,
            service,
            stype,
        });
        self.status = "Loading...".into();
    }

    /// Open the resource limit form for the highlighted service (every type has one).
    pub(super) fn open_resource_form(&mut self, req: &Sender<Req>) {
        let Some((project, service, stype)) = self.selected_row() else {
            self.status = "Select a service first".into();
            return;
        };
        // The menu hides this for a compose service; `L` still reaches here, and
        // without the same guard the leaf key would 404 where the menu no longer
        // can — the gap that made the whole Lifecycle menu wrong for databases.
        if !crate::lifecycle::has_resource_limits(&stype) {
            self.status =
                format!("A {stype} service sets its limits in the compose file, not here");
            return;
        }
        let _ = req.send(Req::ResourceForm {
            project,
            service,
            stype,
        });
        self.status = "Loading...".into();
    }

    /// Set the SAME resource limit on every marked service. Opens the ordinary
    /// resource form (values default to 0 = unlimited, NOT prefilled from any one
    /// service since the marked set differs), and submit applies it to all.
    pub(super) fn open_bulk_resource_form(&mut self) {
        let n = self.marked.len();
        if n == 0 {
            self.status = "Mark some services first — [v] marks the row".into();
            return;
        }
        self.form = Some(
            Form::new(
                FormKind::BulkResourceEdit,
                format!(" Resource limits · {n} marked services "),
                resource_fields(None),
            )
            .with_note(format!(
                "0 = unlimited · applied to all {n} marked (deploy to take effect)"
            )),
        );
    }

    /// Load the currently selected repo's branches into the "Branch" dropdown.
    pub(super) fn load_form_branches(&mut self, req: &Sender<Req>) {
        let Some(form) = self.form.as_ref() else {
            return;
        };
        let repo = form.by_label("Repo");
        if let Some((owner, repo)) = repo.split_once('/') {
            let _ = req.send(Req::Branches {
                owner: owner.into(),
                repo: repo.into(),
            });
        }
    }

    /// Open the dropdown for the currently focused Choice field.
    pub(super) fn open_chooser(&mut self) {
        let Some(form) = self.form.as_ref() else {
            return;
        };
        let f = &form.fields[form.focus];
        if let FieldKind::Choice(opts) = &f.kind {
            if opts.is_empty() {
                self.status = format!("{} has no options yet", f.label);
                return;
            }
            self.chooser = Some(Chooser::new(form.focus, f.label, opts.clone(), &f.value));
        }
    }

    pub(super) fn submit_form(&mut self, req: &Sender<Req>) {
        let Some(form) = self.form.as_ref() else {
            return;
        };

        // Minimal validation here; the server rejects the rest.
        match &form.kind {
            FormKind::ServerAdd | FormKind::ServerEdit { .. } => {
                // Add: token required. Edit: an empty token = keep the old one, so
                // changing just the URL doesn't force retyping the token.
                let (name, url, token) = match &form.kind {
                    FormKind::ServerAdd => (form.val(0), form.val(1), Some(form.val(2))),
                    FormKind::ServerEdit { .. } => (
                        form.val(0),
                        form.val(1),
                        match form.val(2) {
                            t if t.is_empty() => None,
                            t => Some(t),
                        },
                    ),
                    _ => unreachable!(),
                };
                if name.is_empty() || url.is_empty() {
                    self.status = "Name and URL are required".into();
                    return;
                }
                if token.as_deref() == Some("") {
                    self.status = "Token is required".into();
                    return;
                }
                if !commands::valid_name(&name) {
                    self.status = "Server name may only contain a-z, 0-9, - and _".into();
                    return;
                }
                self.server_action = Some(ServerAction::Save {
                    rename_from: match &form.kind {
                        FormKind::ServerEdit { name: old } if *old != name => Some(old.clone()),
                        _ => None,
                    },
                    name,
                    url: url.trim_end_matches('/').to_string(),
                    token,
                });
            }
            FormKind::ProjectCreate => {
                let name = form.val(0);
                if !commands::valid_name(&name) {
                    self.status = "Project name may only contain a-z, 0-9, - and _".into();
                    return;
                }
                let _ = req.send(Req::ProjectCreate(name));
            }
            FormKind::ServiceCreate => {
                let (project, service, stype) = (form.val(0), form.val(1), form.val(2));
                if !commands::valid_name(&service) || project.is_empty() {
                    self.status = "Service names may only contain a-z, 0-9, - and _".into();
                    return;
                }
                // The source is applied separately (see create_source): inline it
                // triggers a deploy. build/env/domains are safe inline — fast, no deploy.
                let source = match create_source(form) {
                    Ok(s) => s,
                    Err(msg) => {
                        self.status = msg;
                        return;
                    }
                };
                let mut extra = service_extra(form);
                if let Some(build) = create_build(form) {
                    extra["build"] = build;
                }
                if let Some(env) = create_env(form) {
                    extra["env"] = json!(env);
                    // "Create .env file" -> write env as a file at this path.
                    if form.is_on_label("Create .env file") {
                        let path = form.by_label(".env file path");
                        extra["dotEnvPath"] =
                            json!(if path.is_empty() { ".env".into() } else { path });
                    }
                }
                if let Some(domains) = create_domains(form) {
                    extra["domains"] = domains;
                }
                self.status = format!("Creating '{service}'...");
                let _ = req.send(Req::ServiceCreate {
                    project,
                    service,
                    stype,
                    extra,
                    source,
                });
                self.form = None;
                return;
            }
            FormKind::SourceEdit { project, service } => match source_body(form) {
                Ok((op, body, auto_deploy)) => {
                    let _ = req.send(Req::ConfigSave {
                        project: project.clone(),
                        service: service.clone(),
                        op,
                        body,
                        auto_deploy,
                    });
                }
                Err(msg) => {
                    self.status = msg;
                    return;
                }
            },
            FormKind::BuildEdit { project, service } => match build_body(form) {
                Ok(body) => {
                    let _ = req.send(Req::ConfigSave {
                        project: project.clone(),
                        service: service.clone(),
                        op: "updateBuild",
                        body,
                        auto_deploy: None,
                    });
                }
                Err(msg) => {
                    self.status = msg;
                    return;
                }
            },
            FormKind::ResourceEdit {
                project,
                service,
                stype,
            } => match resource_body(form) {
                Ok(resources) => {
                    let _ = req.send(Req::ResourceSave {
                        project: project.clone(),
                        service: service.clone(),
                        stype: stype.clone(),
                        resources,
                    });
                }
                Err(msg) => {
                    self.status = msg;
                    return;
                }
            },
            FormKind::BulkResourceEdit => match resource_body(form) {
                Ok(resources) => {
                    let targets = self.bulk_targets();
                    if targets.is_empty() {
                        self.status = "Nothing marked any more — cancelled".into();
                        self.form = None;
                        return;
                    }
                    let _ = req.send(Req::BulkResource { targets, resources });
                    self.status = "Setting resource limits...".into();
                }
                Err(msg) => {
                    self.status = msg;
                    return;
                }
            },
            FormKind::CloneService {
                project,
                service,
                stype,
            } => {
                let new_name = form.by_label("New name");
                let new_name = new_name.trim();
                let target = form.by_label("Project");
                let target = if target.is_empty() {
                    project.clone()
                } else {
                    target
                };
                if new_name.is_empty() {
                    self.status = "Enter the new service name first".into();
                    return;
                }
                // The name may match as long as the project differs; identical
                // (project+name) = a collision.
                if target == *project && new_name == service {
                    self.status =
                        "Use a different project, or a different name — they can't be identical"
                            .into();
                    return;
                }
                let _ = req.send(Req::CloneService {
                    project: project.clone(),
                    service: service.clone(),
                    stype: stype.clone(),
                    target,
                    new_name: new_name.to_string(),
                });
            }
            FormKind::DiffAcross {
                project,
                service,
                stype,
            } => {
                let target_server = form.by_label("On server");
                if target_server.is_empty() {
                    self.status = "Choose a server to compare against first".into();
                    return;
                }
                self.diff_across_req = Some(DiffAcrossReq {
                    local: (project.clone(), service.clone(), stype.clone()),
                    target_server,
                });
                self.status = "Comparing across hosts...".into();
            }
            FormKind::DiffProjectAcross { project } => {
                let target_server = form.by_label("On server");
                if target_server.is_empty() {
                    self.status = "Choose a server to compare against first".into();
                    return;
                }
                self.diff_project_across_req = Some(DiffProjectAcrossReq {
                    project: project.clone(),
                    target_server,
                });
                self.status = "Comparing project across hosts...".into();
            }
            FormKind::Migrate {
                project,
                service,
                stype,
            } => {
                let target_server = form.by_label("To server");
                let target_project = form.by_label("Target project");
                let target_project = target_project.trim();
                if target_server.is_empty() {
                    self.status = "Choose the destination server first".into();
                    return;
                }
                if target_project.is_empty() {
                    self.status = "Enter the target project name first".into();
                    return;
                }
                // Empty service = the whole project, which is the same operation
                // over every service it holds.
                let services = if service.is_empty() {
                    self.project_services(project)
                } else {
                    vec![(project.clone(), service.clone(), stype.clone())]
                };
                self.migrate_req = Some(MigrateReq {
                    target_server,
                    target_project: target_project.to_string(),
                    services,
                });
            }
            FormKind::DeployEdit { project, service } => match deploy_body(form) {
                Ok(deploy) => {
                    let _ = req.send(Req::DeploySave {
                        project: project.clone(),
                        service: service.clone(),
                        deploy,
                    });
                    self.status = "Saving...".into();
                }
                Err(msg) => {
                    self.status = msg;
                    return;
                }
            },
            FormKind::MountEdit {
                project,
                service,
                index,
            } => match mount_body(form) {
                Ok(values) => {
                    let _ = req.send(Req::MountUpdate {
                        project: project.clone(),
                        service: service.clone(),
                        index: *index,
                        values,
                    });
                    self.status = "Saving...".into();
                }
                Err(msg) => {
                    self.status = msg;
                    return;
                }
            },
            FormKind::MountCreate { project, service } => match mount_body(form) {
                Ok(values) => {
                    let _ = req.send(Req::MountSave {
                        project: project.clone(),
                        service: service.clone(),
                        values,
                    });
                }
                Err(msg) => {
                    self.status = msg;
                    return;
                }
            },
            FormKind::BasicAuthEdit {
                project,
                service,
                stype,
            } => match basic_auth_body(form) {
                Ok(basic_auth) => {
                    let _ = req.send(Req::BasicAuthSave {
                        project: project.clone(),
                        service: service.clone(),
                        stype: stype.clone(),
                        basic_auth,
                    });
                }
                Err(msg) => {
                    self.status = msg;
                    return;
                }
            },
            FormKind::RedirectCreate {
                project,
                service,
                stype,
            } => match redirect_body(form) {
                Ok(redirect) => {
                    let _ = req.send(Req::RedirectAdd {
                        project: project.clone(),
                        service: service.clone(),
                        stype: stype.clone(),
                        redirect,
                    });
                }
                Err(msg) => {
                    self.status = msg;
                    return;
                }
            },
            FormKind::RestoreFrom { project, service } => {
                // The server is known by NAME here; only event_loop holds its
                // token, so the request is parked for it to resolve — the same
                // route a migration takes.
                self.restore_from_req =
                    Some((form.by_label("Server"), project.clone(), service.clone()));
                self.status = "Reading that server's backups...".into();
                self.form = None;
                return;
            }
            FormKind::LogSearch => {
                let query = form.by_label("Keyword");
                if query.is_empty() {
                    self.status = "Enter a keyword first".into();
                    return;
                }
                // Open an empty Viewer; results follow once the fan-out finishes.
                self.viewer.lines = vec!["Searching across all services...".into()];
                self.viewer.scroll = 0;
                self.viewer.hscroll = 0;
                self.viewer.follow = false;
                self.viewer.log_cursor = None;
                self.viewer.title = format!("Search '{query}'");
                self.viewer.ctx = None;
                self.viewer.from = Screen::Projects;
                self.screen = Screen::Viewer;
                self.status = format!("Searching '{query}' across all services...");
                let _ = req.send(Req::LogSearch { query });
                self.form = None;
                return;
            }
            FormKind::PortCreate { project, service } => match port_body(form) {
                Ok(values) => {
                    let _ = req.send(Req::PortSave {
                        project: project.clone(),
                        service: service.clone(),
                        values,
                    });
                }
                Err(msg) => {
                    self.status = msg;
                    return;
                }
            },
            FormKind::CheckEdit { url } => {
                let url = url.clone();
                match check_body(&url, form) {
                    Ok(check) => {
                        // Memory first so the screen is honest immediately; the
                        // event loop writes the file.
                        let known = self.watch.iter_mut().find(|c| c.url == check.url);
                        let enrolled = match known {
                            Some(slot) => {
                                *slot = check.clone();
                                false
                            }
                            None => {
                                self.watch.push(check.clone());
                                true
                            }
                        };
                        // The old answer described a different request, so it is
                        // dropped rather than left on screen next to a check it
                        // no longer describes.
                        self.probes.retain(|p| p.url != check.url);
                        self.watch_action = Some(WatchAction::Put(check));
                        self.form = None;
                        self.status = if enrolled {
                            format!("Watching {} — [8] Uptime to check it", url)
                        } else {
                            "Check saved — [r] to run it".into()
                        };
                    }
                    Err(msg) => self.status = format!("⚠ {msg}"),
                }
                return;
            }
            FormKind::DomainBulkEdit => {
                let (target, find, replace) = (
                    form.by_label("Replace in"),
                    form.by_label("Find"),
                    form.by_label("Replace with"),
                );
                self.preview_domain_edits(&target, &find, &replace);
                return;
            }
            FormKind::CfAccountAdd | FormKind::CfAccountEdit { .. } => {
                let (name, token, account_id) = (
                    form.val(0).trim().to_string(),
                    form.val(1).trim().to_string(),
                    form.val(2).trim().to_string(),
                );
                if name.is_empty() || token.is_empty() {
                    self.status = "Name and API token are required".into();
                    return;
                }
                if !commands::valid_name(&name) {
                    self.status =
                        "Cloudflare account names may only contain a-z, 0-9, - and _".into();
                    return;
                }
                // Resolved by event_loop, which alone holds the CloudflareConfig —
                // same rule as a server-list change (see ServerAction).
                let account = crate::cloudflare::CloudflareAccount {
                    name,
                    api_token: token,
                    account_id: (!account_id.is_empty()).then_some(account_id),
                    default: false,
                };
                self.cf_action = Some(match &form.kind {
                    FormKind::CfAccountEdit { name } => CfAction::Save {
                        rename_from: Some(name.clone()),
                        account,
                    },
                    _ => CfAction::Add(account),
                });
            }
            FormKind::CfRecordCreate => {
                let kind = form.val(0).trim().to_string();
                if !valid_record_type(&kind) {
                    self.status = format!("Unsupported record type '{kind}'");
                    return;
                }
                let (name, content) = (
                    form.val(1).trim().to_string(),
                    form.val(2).trim().to_string(),
                );
                if name.is_empty() || content.is_empty() {
                    self.status = "Name and content are required".into();
                    return;
                }
                let ttl_text = form.val(3);
                let ttl = match ttl_text.trim().parse() {
                    Ok(ttl) => ttl,
                    Err(_) => {
                        self.status = "TTL must be a number (use 1 for automatic)".into();
                        return;
                    }
                };
                let proxied = form.val(4) == "yes";
                let priority_text = form.val(5);
                let priority = if priority_text.trim().is_empty() {
                    None
                } else {
                    match priority_text.trim().parse() {
                        Ok(p) => Some(p),
                        Err(_) => {
                            self.status = "Priority must be a number".into();
                            return;
                        }
                    }
                };
                let body = record_body(&kind, &name, &content, ttl, proxied, priority);
                let (Some(token), Some(zone)) = (self.cf_token(), self.cf.current_zone.clone())
                else {
                    self.status = "No active zone".into();
                    return;
                };
                let _ = req.send(Req::Cf(CfReq::CreateRecord {
                    token,
                    zone_id: zone.id,
                    body,
                }));
            }
            FormKind::CfRecordEdit { id, kind } => {
                let (id, kind) = (id.clone(), kind.clone());
                if form.by_label("Content").trim().is_empty() {
                    self.status = "Content is required".into();
                    return;
                }
                if form.by_label("TTL").trim().parse::<u32>().is_err() {
                    self.status = "TTL must be a number (use 1 for automatic)".into();
                    return;
                }
                let priority_text = form.by_label("Priority");
                if !priority_text.trim().is_empty() && priority_text.trim().parse::<u16>().is_err()
                {
                    self.status = "Priority must be a number".into();
                    return;
                }
                let patch = cf_record_patch(
                    &kind,
                    &form.by_label("Content"),
                    &form.by_label("TTL"),
                    form.by_label("Proxied") == "yes",
                    &form.by_label("Priority"),
                );
                if patch.is_empty() {
                    self.status = "Nothing to change".into();
                    return;
                }
                let (Some(token), Some(zone)) = (self.cf_token(), self.cf.current_zone.clone())
                else {
                    self.status = "No active zone".into();
                    return;
                };
                let _ = req.send(Req::Cf(CfReq::PatchRecord {
                    token,
                    zone_id: zone.id,
                    id,
                    body: apply_patch(&patch),
                }));
            }
            FormKind::CfZoneCreate => {
                let name = form.val(0).trim().to_string();
                if name.is_empty() {
                    self.status = "Zone name is required".into();
                    return;
                }
                let (Some(token), Some(account_id)) = (
                    self.cf_token(),
                    self.cf.active.as_ref().and_then(|a| a.account_id.clone()),
                ) else {
                    self.status = "This account has no account-id — cannot create a zone".into();
                    return;
                };
                let _ = req.send(Req::Cf(CfReq::CreateZone {
                    token,
                    name,
                    account_id,
                }));
            }
            FormKind::CfZoneDelete { zone_id, name } => {
                let (zone_id, name) = (zone_id.clone(), name.clone());
                if form.val(0) != name {
                    self.status = "Name did not match — nothing deleted".into();
                    return;
                }
                let Some(token) = self.cf_token() else {
                    self.status = "No active account".into();
                    return;
                };
                let _ = req.send(Req::Cf(CfReq::DeleteZone { token, zone_id }));
            }
            FormKind::CfBulkSet(attr) => {
                let body = match attr {
                    CfBulkAttr::Content => {
                        let content = form.by_label("Content").trim().to_string();
                        if content.is_empty() {
                            self.status = "Content is required".into();
                            return;
                        }
                        json!({ "content": content })
                    }
                    CfBulkAttr::Proxied => {
                        json!({ "proxied": form.by_label("Proxied") == "yes" })
                    }
                    CfBulkAttr::Ttl => match form.by_label("TTL").trim().parse::<u32>() {
                        Ok(ttl) => json!({ "ttl": ttl }),
                        Err(_) => {
                            self.status = "TTL must be a number (use 1 for automatic)".into();
                            return;
                        }
                    },
                };
                let ids: Vec<String> = self.cf.marked.iter().cloned().collect();
                if ids.is_empty() {
                    self.status = "Nothing marked any more — cancelled".into();
                    return;
                }
                let (Some(token), Some(zone)) = (self.cf_token(), self.cf.current_zone.clone())
                else {
                    self.status = "No active zone".into();
                    return;
                };
                let _ = req.send(Req::Cf(CfReq::BulkPatch {
                    token,
                    zone_id: zone.id,
                    ids,
                    body,
                }));
            }
            FormKind::CfBucketCreate => {
                let name = form.val(0).trim().to_string();
                if name.is_empty() {
                    self.status = "Bucket name is required".into();
                    return;
                }
                let (Some(token), Some(account_id)) = (self.cf_token(), self.cf_account_id())
                else {
                    self.status = "This account has no account-id — R2 is account-scoped".into();
                    return;
                };
                let _ = req.send(Req::Cf(CfReq::CreateR2Bucket {
                    token,
                    account_id,
                    name,
                }));
            }
            FormKind::CfBucketDelete { name } => {
                let name = name.clone();
                if form.val(0) != name {
                    self.status = "Name did not match — nothing deleted".into();
                    return;
                }
                let (Some(token), Some(account_id)) = (self.cf_token(), self.cf_account_id())
                else {
                    self.status = "No active account".into();
                    return;
                };
                let _ = req.send(Req::Cf(CfReq::DeleteR2Bucket {
                    token,
                    account_id,
                    name,
                }));
            }
            FormKind::CfWorkerDeploy => {
                let name = form.by_label("Worker name").trim().to_string();
                let path = crate::output::expand_tilde(
                    form.by_label("Local file").trim(),
                    std::env::var("HOME").ok().as_deref(),
                );
                if name.is_empty() || path.is_empty() {
                    self.status = "Worker name and local file are required".into();
                    return;
                }
                let mode = match form.by_label("Mode").as_str() {
                    "service-worker" => WorkerUploadMode::ServiceWorker,
                    _ => WorkerUploadMode::Module,
                };
                let (Some(token), Some(account_id)) = (self.cf_token(), self.cf_account_id())
                else {
                    self.status = "No active account".into();
                    return;
                };
                let _ = req.send(Req::Cf(CfReq::WorkerDeploy {
                    token,
                    account_id,
                    name,
                    path,
                    mode,
                }));
            }
            FormKind::CfWorkerDelete { name } => {
                let name = name.clone();
                if form.val(0) != name {
                    self.status = "Name did not match — nothing deleted".into();
                    return;
                }
                let (Some(token), Some(account_id)) = (self.cf_token(), self.cf_account_id())
                else {
                    self.status = "No active account".into();
                    return;
                };
                let _ = req.send(Req::Cf(CfReq::WorkerDelete {
                    token,
                    account_id,
                    name,
                    force: false,
                }));
            }
            FormKind::CfTunnelRouteCreate { tunnel_id } => {
                let (hostname, service, path) = (
                    form.by_label("Hostname").trim().to_string(),
                    form.by_label("Service").trim().to_string(),
                    form.by_label("Path").trim().to_string(),
                );
                if hostname.is_empty() || service.is_empty() {
                    self.status = "Hostname and service are required".into();
                    return;
                }
                let origin_request = match tunnel_origin_request_from_form(form) {
                    Ok(v) => v,
                    Err(e) => {
                        self.status = e.to_string();
                        return;
                    }
                };
                let (Some(token), Some(account_id)) = (self.cf_token(), self.cf_account_id())
                else {
                    self.status = "No active account".into();
                    return;
                };
                let _ = req.send(Req::Cf(CfReq::TunnelRouteAdd {
                    token,
                    account_id,
                    tunnel_id: tunnel_id.clone(),
                    hostname,
                    service,
                    path,
                    origin_request,
                }));
            }
            FormKind::CfTunnelRouteEdit {
                tunnel_id,
                hostname,
                path,
            } => {
                let service = form.by_label("Service").trim().to_string();
                if service.is_empty() {
                    self.status = "Service is required".into();
                    return;
                }
                let clear = form.by_label("Clear origin request") == "yes";
                let origin_request = if clear {
                    Some(None)
                } else {
                    match tunnel_origin_request_from_form(form) {
                        Ok(v) => Some(v),
                        Err(e) => {
                            self.status = e.to_string();
                            return;
                        }
                    }
                };
                let (Some(token), Some(account_id)) = (self.cf_token(), self.cf_account_id())
                else {
                    self.status = "No active account".into();
                    return;
                };
                let _ = req.send(Req::Cf(CfReq::TunnelRouteEdit {
                    token,
                    account_id,
                    tunnel_id: tunnel_id.clone(),
                    hostname: hostname.clone(),
                    path: path.clone(),
                    service,
                    origin_request,
                }));
            }
            FormKind::CfTunnelRouteDelete {
                tunnel_id,
                hostname,
                path,
            } => {
                if form.val(0) != hostname.as_str() {
                    self.status = "Hostname did not match — nothing deleted".into();
                    return;
                }
                let (Some(token), Some(account_id)) = (self.cf_token(), self.cf_account_id())
                else {
                    self.status = "No active account".into();
                    return;
                };
                let _ = req.send(Req::Cf(CfReq::TunnelRouteDelete {
                    token,
                    account_id,
                    tunnel_id: tunnel_id.clone(),
                    hostname: hostname.clone(),
                    path: path.clone(),
                }));
            }
            FormKind::R2Upload => {
                // A path typed into the form is raw text — expand a leading `~` the way a
                // shell would, so `~/dump.gz` finds the home directory, not a `~` folder.
                let path = crate::output::expand_tilde(
                    form.val(0).trim(),
                    std::env::var("HOME").ok().as_deref(),
                );
                if path.is_empty() {
                    self.status = "Give a local file path".into();
                    return;
                }
                let (Some(token), Some(account_id), Some(bucket)) = (
                    self.cf_token(),
                    self.cf_account_id(),
                    self.cf.current_bucket.clone(),
                ) else {
                    self.status = "No bucket open".into();
                    return;
                };
                let _ = req.send(Req::Cf(CfReq::R2Put {
                    token,
                    account_id,
                    bucket,
                    prefix: self.cf.current_prefix.clone(),
                    path,
                }));
            }
            FormKind::R2Download { key } => {
                let key = key.clone();
                // Same `~` expansion as the upload path — a raw form string, not a shell arg.
                let dest = crate::output::expand_tilde(
                    form.val(0).trim(),
                    std::env::var("HOME").ok().as_deref(),
                );
                if dest.is_empty() {
                    self.status = "Give a path to save to".into();
                    return;
                }
                let (Some(token), Some(account_id), Some(bucket)) = (
                    self.cf_token(),
                    self.cf_account_id(),
                    self.cf.current_bucket.clone(),
                ) else {
                    self.status = "No bucket open".into();
                    return;
                };
                let _ = req.send(Req::Cf(CfReq::R2Get {
                    token,
                    account_id,
                    bucket,
                    key,
                    dest,
                }));
            }
            FormKind::DomainCreate | FormKind::DomainEdit { .. } => match domain_body(form) {
                Ok(body) => {
                    let id = match &form.kind {
                        FormKind::DomainEdit { id } => Some(id.clone()),
                        _ => None,
                    };
                    let _ = req.send(Req::DomainSave { id, body });
                }
                Err(msg) => {
                    self.status = msg;
                    return;
                }
            },
        }
        self.form = None;
        self.status = "Sending...".into();
    }

    /// Open the server list (select / add / edit / delete).
    ///
    /// Must not refuse when there's only one server: this picker is the only way to
    /// add a server from the TUI, so refusing it would make a second server
    /// impossible to create without dropping to the CLI.
    pub(super) fn open_picker(&mut self) {
        let cur = self
            .all_servers
            .iter()
            .position(|(n, _)| n == &self.server_name)
            .unwrap_or(0);
        let mut st = ListState::default();
        st.select(Some(cur));
        self.picker = Some(st);
    }

    /// The new-service form. The project is chosen from a dropdown: a flat list has
    /// no "currently open project", so it must be named explicitly.
    ///
    /// The source is included here, not deferred to an edit form: createService
    /// accepts an inline `source` and only requires projectName + serviceName, so
    /// create-then-edit was all along a limit of this form — not a limit of the API.
    pub(super) fn new_service_form(&mut self, req: &Sender<Req>) {
        if self.projects.is_empty() {
            self.status = "Project list not loaded yet".into();
            return;
        }
        let project = self
            .selected_project()
            .unwrap_or_else(|| self.projects[0].clone());
        // The database fields follow Kind, like the panel dialog. All optional:
        // empty means the server creates them (a random password, a database named
        // after the project, the latest official image) — exactly like the panel.
        let mut fields = vec![
            Field::choice_owned("Project", self.projects.clone(), &project),
            Field::text("Name", ""),
            Field::choice("Kind", SERVICE_TYPES, "app"),
            Field::text("Database", "").when("Kind", "mysql,mariadb,postgres"),
            Field::text("User", "").when("Kind", "mysql,mariadb,postgres,mongo"),
            Field::secret("Password").when("Kind", "mysql,mariadb,postgres,mongo,redis"),
            Field::secret("Root password").when("Kind", "mysql,mariadb"),
            Field::text("Image", "").when("Kind", "mysql,mariadb,postgres,mongo,redis"),
        ];
        // The source fields carry their own condition (Source=github/git/image);
        // .when() adds a condition rather than replacing it, so both apply: shown
        // only when service type = app AND the source type matches.
        //
        // The repo list follows via Resp::Repos: waiting for it here would freeze
        // the TUI until searchRepos finishes.
        // The wizard follows the EasyPanel dashboard flow: Basics → Source → Build.
        // The source & build fields are app-only (`.when("Kind","app")`), so a
        // database service stays a single step. `.step()` puts them on their own
        // pages; submit values are still read across steps.
        fields.extend(
            source_fields(None, Vec::new())
                .into_iter()
                .map(|f| f.when("Kind", "app").step(1)),
        );
        fields.extend(build_fields(None).into_iter().map(|f| {
            f.when("Kind", "app")
                .when("Source", "github,git,dockerfile")
                .step(2)
        }));
        // Continuing the dashboard flow: Environment then Domains. Both are accepted
        // inline by createService (`env` string, `domains` array; only `host`
        // required). The domain labels are prefixed with "Domain " so "Path" doesn't
        // collide with the source's "Path" — by_label() uses find().
        fields.push(Field::editor("Environment", "").when("Kind", "app").step(3));
        // "Create env file" in the dashboard: write env as a .env file at that path
        // (API: dotEnvPath). The path only shows when its toggle is on.
        fields.push(
            Field::boolean("Create .env file", false)
                .when("Kind", "app")
                .step(3),
        );
        fields.push(
            Field::text(".env file path", ".env")
                .when("Kind", "app")
                .when("Create .env file", "yes")
                .step(3),
        );
        fields.extend(
            [
                Field::text("Domain host", ""),
                Field::text("Domain port", "3000"),
                Field::boolean("Domain HTTPS", true),
                Field::text("Domain path", "/"),
            ]
            .map(|f| f.when("Kind", "app").step(4)),
        );
        self.form = Some(Form::new(FormKind::ServiceCreate, " New service ", fields));
        let _ = req.send(Req::Repos);
    }

    pub(super) fn open_view(&mut self, view: View, req: &Sender<Req>) {
        // On a project header there is no service to look at. Saying so beats the
        // key doing nothing: the menu path already says it, so `p`/`b`/`f` going
        // silent was the same action answering differently depending on how you
        // reached it.
        let Some((p, s, t)) = self.selected_row() else {
            self.status = "Select a service first".into();
            return;
        };
        // Leaving an action detail for a service view: stop `r` re-fetching it.
        self.viewer.action_detail = None;
        {
            self.viewer.from = Screen::Projects;
            self.viewer.ctx = Some((view, p.clone(), s.clone(), t.clone()));
            self.status = format!("Loading {}...", view.title());
            // A log is a stream, not a document: start empty, stick to the last
            // line, and let the poll lane keep it going. Other views are snapshots
            // and start at the top.
            if view == View::Logs {
                self.viewer.lines.clear();
                self.viewer.scroll = 0;
                self.viewer.hscroll = 0;
                self.viewer.log_cursor = None;
                self.viewer.follow = true;
                // Other views switch screens via Resp::Viewer; logs don't go through
                // there, so the switch has to happen here. Without it, Enter would
                // seem to do nothing.
                self.viewer.title = format!("Logs · {p}/{s}");
                self.screen = Screen::Viewer;
                let _ = req.send(Req::LogTail {
                    project: p,
                    service: s,
                    since: None,
                });
                return;
            }
            self.viewer.follow = false;
            let _ = req.send(Req::Fetch {
                view,
                project: p,
                service: s,
                stype: t,
            });
        }
    }

    pub(super) fn ask_action(&mut self, action: &str) {
        if let Some((p, s, t)) = self.selected_row() {
            // Debounce deploy: if a deployment is still pending/running, say so in
            // the confirmation dialog so the user doesn't trigger a second build
            // unknowingly.
            // "deploy-force" is the same endpoint with the layer cache off, so it
            // needs its own wording — cap() would render it "Deploy-force".
            let mut label = if action == "deploy-force" {
                format!("Rebuild '{s}' from scratch, ignoring the build cache?")
            } else {
                format!("{} service '{}'?", cap(action), s)
            };
            if action.starts_with("deploy") && self.is_deploying(&p, &s) {
                label.push_str(" ⚠ previous deploy still running");
            }
            self.confirm = Some(Confirm {
                action: action.to_string(),
                project: p,
                service: s.clone(),
                stype: t,
                label,
            });
        }
    }

    pub(super) fn refresh(&mut self, req: &Sender<Req>) {
        let _ = req.send(Req::Stats);
        let _ = req.send(Req::Nodes);
        match self.screen {
            // On this screen `r` means "ask them all again" — the whole point of
            // the screen is the answer being current.
            Screen::Uptime => self.run_checks(req),
            Screen::Projects => {
                let _ = req.send(Req::AllServices);
                let _ = req.send(Req::MonitorData);
            }
            Screen::Viewer => {
                if let Some((view, p, s, t)) = self.viewer.ctx.clone() {
                    let _ = req.send(Req::Fetch {
                        view,
                        project: p,
                        service: s,
                        stype: t,
                    });
                } else if let Some(id) = self.viewer.action_detail.clone() {
                    // An action detail is a one-shot snapshot; this is the key
                    // that makes it current again.
                    let _ = req.send(Req::ActionDetail(id));
                }
            }
            Screen::Actions => {
                let _ = req.send(Req::Actions);
            }
            Screen::Domains => {
                let _ = req.send(Req::Domains);
            }
            Screen::Monitor => {
                let _ = req.send(Req::MonitorData);
                let _ = req.send(Req::Storage);
            }
            Screen::Hosts => self.load_hosts = true,
            // A credentials snapshot doesn't poll; reopening it re-reads.
            Screen::Terminal | Screen::Credentials => {}
            Screen::Maintenance => {
                let _ = req.send(Req::MaintInfo);
            }
            Screen::Dashboard => {}
        }
        self.status = "Refreshing...".into();
    }
}
