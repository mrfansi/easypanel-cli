use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;

use anyhow::Result;
use serde_json::{json, Value};

use crate::client::EasypanelClient;
use crate::output::field;

use super::table::row_marker;

// ---------- Worker (networking on a separate thread so the UI doesn't freeze) ----------

#[derive(Clone, Copy, PartialEq)]
pub(super) enum View {
    Logs,
    Env,
    Ports,
    Mounts,
    Redirects,
    Backups,
    Source,
    ConfigFile,
}

impl View {
    /// Does this view show ROWS you act on one at a time, rather than free text?
    ///
    /// A collection gets a selected row and `x` to delete it. Logs, env and an
    /// action's output are prose: selecting a line there means nothing.
    pub(super) fn is_collection(self) -> bool {
        matches!(self, View::Ports | View::Mounts | View::Redirects)
    }

    pub(super) fn title(self) -> &'static str {
        match self {
            View::Logs => "Logs",
            View::Env => "Env",
            View::Ports => "Ports",
            View::Mounts => "Mounts",
            View::Redirects => "Redirects",
            View::Backups => "Database backups",
            View::Source => "Source & build",
            View::ConfigFile => "Config file",
        }
    }
}

pub(super) enum Req {
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
        source: Option<super::form::SourceCall>,
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
        target_url: String,
        target_token: String,
        /// The destination's configured name — for the status line, not the call.
        target_name: String,
        target_project: String,
        /// What to move, as (project, service, type). One entry migrates a single
        /// service; a whole project is the same operation over its service list.
        services: Vec<(String, String, String)>,
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
}

pub(super) enum Resp {
    Stats(Value),
    Nodes(Vec<Value>),
    Projects(Vec<String>),
    Actions(Vec<Value>),
    MonitorData(Vec<Value>),
    /// (actual, desired) swarm replicas, keyed by the name "{project}_{service}".
    TaskStats(HashMap<String, (i64, i64)>),
    Storage(Vec<Value>),
    Domains(Vec<Value>),
    /// All services across projects + the project names for a form dropdown.
    AllServices {
        projects: Vec<String>,
        services: Vec<Value>,
    },
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
    Viewer(String, Vec<String>),
    /// Output bytes from a container terminal session (fed to the vt100 parser).
    TermOutput(Vec<u8>),
    /// The terminal session ended (shell exited / socket closed).
    TermClosed,
    /// A mutation succeeded: a status message + which data needs reloading.
    Done(String, Refresh),
    Err(String),
}

/// The data that needs refreshing after a mutation.
pub(super) enum Refresh {
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
pub(super) fn spawn_worker(
    client: EasypanelClient,
    resp_tx: Sender<Resp>,
    busy: Option<Arc<AtomicUsize>>,
) -> Sender<Req> {
    let (req_tx, req_rx) = mpsc::channel::<Req>();

    thread::spawn(move || {
        while let Ok(req) = req_rx.recv() {
            if let Some(b) = &busy {
                b.fetch_add(1, Ordering::Relaxed);
            }
            let resp = handle_req(&client, req, &resp_tx);
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
pub(super) struct Workers {
    pub(super) user: Sender<Req>,
    pub(super) poll: Sender<Req>,
    pub(super) resp: Receiver<Resp>,
    /// For the Hosts screen fan-out: each host gets its own thread, so its result
    /// doesn't go through the user/poll lane bound to a single client.
    pub(super) resp_tx: Sender<Resp>,
    /// How many user-initiated requests are in flight. Shared with the App, which
    /// uses it to decide whether anything is actually happening.
    pub(super) busy: Arc<AtomicUsize>,
}

pub(super) fn spawn_workers(client: EasypanelClient) -> Workers {
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

pub(super) fn handle_req(client: &EasypanelClient, req: Req, resp_tx: &Sender<Resp>) -> Resp {
    match req {
        Req::Stats => match client.call("metrics", "getSystemStats", json!({})) {
            Ok(v) => Resp::Stats(v),
            Err(e) => Resp::Err(e.to_string()),
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
            Err(e) => Resp::Err(e.to_string()),
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
            Err(e) => Resp::Err(e.to_string()),
        },
        Req::ServicesFor(project) => {
            match client.call(
                "projects",
                "inspectProject",
                json!({ "projectName": project }),
            ) {
                Ok(v) => Resp::ServicesFor(
                    project,
                    parse_services(&v).into_iter().map(|(n, _)| n).collect(),
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
        } => bulk_action(client, resp_tx, targets, &action, force),
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
                    format!("Resource {project}/{service} saved — deploy (d) to apply"),
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
                format!("Basic auth {project}/{service} saved — deploy (d) to apply"),
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
        } => {
            let title = format!("{} · {}/{}", view.title(), project, service);
            match fetch_view(client, view, &project, &service, &stype) {
                Ok(lines) => Resp::Viewer(title, lines),
                Err(e) => Resp::Err(e.to_string()),
            }
        }
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
            let ps = json!({ "projectName": project, "serviceName": service });
            let mut input = extra;
            input["projectName"] = json!(project);
            input["serviceName"] = json!(service);
            // 1) Create the service. Without an inline source this is fast (~0.2
            //    seconds) and triggers no deploy, so the service shows up in the
            //    table right away.
            match client.call(&grp, "createService", input) {
                Ok(_) => {
                    // 2) Apply the source separately (updateSource* + autoDeploy).
                    //    This only stores the config, without deploying.
                    if let Some((op, mut body, auto)) = source {
                        body["projectName"] = json!(project);
                        body["serviceName"] = json!(service);
                        if let Err(e) = client.call(&grp, op, body) {
                            return Resp::Err(format!(
                                "Service '{service}' created, but its source failed: {e}"
                            ));
                        }
                        if let Some(on) = auto {
                            let ep = if on {
                                "enableGithubDeploy"
                            } else {
                                "disableGithubDeploy"
                            };
                            let _ = client.call(&grp, ep, ps.clone());
                        }
                    }
                    // Deliberately NOT deploying: let it appear in the table first,
                    // then the user presses `d`. Deploying on create is what used
                    // to cause the error.
                    Resp::Done(
                        format!("Service '{service}' created — press d to deploy"),
                        Refresh::Projects,
                    )
                }
                Err(e) => Resp::Err(e.to_string()),
            }
        }
        Req::CloneService {
            project,
            service,
            stype,
            target,
            new_name,
        } => clone_service(client, &project, &service, &stype, &target, &new_name),
        Req::Migrate {
            target_url,
            target_token,
            target_name,
            target_project,
            services,
        } => {
            let dst_client = EasypanelClient::new(&target_url, &target_token);
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
                    Ok(lines) => Resp::Viewer(format!("Ports · {project}/{service}"), lines),
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
                format!("Mount added to {project}/{service} — press d to deploy"),
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
                Ok(lines) => Resp::Viewer(format!("Mounts · {project}/{service}"), lines),
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
                format!("Redirect added to {project}/{service} — deploy (d) to apply"),
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
                    Ok(lines) => Resp::Viewer(format!("Redirects · {project}/{service}"), lines),
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
        } => match crate::commands::save_env(client, &project, &service, &stype, &env) {
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
                // via resp_tx so it reaches the status line. A build that actually
                // runs is still not awaited (can be minutes, exceeds proxy limits).
                let tx = resp_tx.clone();
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
                            let _ = tx.send(Resp::Err(format!("Deploy {p}/{s} failed: {e}")));
                        }
                    }
                });
                return Resp::Done(
                    format!("Deploy {project}/{service} started — watch it in Logs (Enter)"),
                    Refresh::None,
                );
            }
            let input = json!({ "projectName": project, "serviceName": service });
            match client.call(
                &format!("services/{stype}"),
                &format!("{action}Service"),
                input,
            ) {
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
fn bulk_action(
    client: &EasypanelClient,
    resp_tx: &Sender<Resp>,
    targets: Vec<(String, String, String)>,
    action: &str,
    force: bool,
) -> Resp {
    const CHUNK: usize = 6;

    if targets.is_empty() {
        return Resp::Err("Nothing marked".into());
    }

    if action == "deploy" {
        for (project, service, stype) in &targets {
            let c = client.clone();
            let tx = resp_tx.clone();
            let (grp, p, s) = (
                format!("services/{stype}"),
                project.clone(),
                service.clone(),
            );
            let input = json!({ "projectName": p, "serviceName": s, "forceRebuild": force });
            let (pe, se) = (p.clone(), s.clone());
            thread::spawn(move || {
                if let Err(e) = c.call(&grp, "deployService", input) {
                    if !crate::client::gave_up_waiting(&e) {
                        let _ = tx.send(Resp::Err(format!("Deploy {pe}/{se} failed: {e}")));
                    }
                }
            });
        }
        return Resp::Done(
            format!(
                "Deploy started for {} services — watch the Status column",
                targets.len()
            ),
            Refresh::None,
        );
    }

    let mut ok = Vec::new();
    let mut failed = Vec::new();
    for group in targets.chunks(CHUNK) {
        let handles: Vec<_> = group
            .iter()
            .map(|(project, service, stype)| {
                let c = client.clone();
                let (grp, op) = (format!("services/{stype}"), format!("{action}Service"));
                let (p, s) = (project.clone(), service.clone());
                thread::spawn(move || {
                    let input = json!({ "projectName": p, "serviceName": s });
                    let name = format!("{p}/{s}");
                    match c.call(&grp, &op, input) {
                        Ok(_) => Ok(name),
                        Err(e) => Err((name, e.to_string())),
                    }
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

pub(super) fn fetch_view(
    client: &EasypanelClient,
    view: View,
    project: &str,
    service: &str,
    stype: &str,
) -> Result<Vec<String>> {
    let ps = json!({ "projectName": project, "serviceName": service });
    let lines = match view {
        View::Logs => {
            let v = client.call(
                "logs",
                "queryServiceLogs",
                json!({ "projectName": project, "serviceName": service, "limit": 200 }),
            )?;
            crate::logs::format(&v)
        }
        View::Env => {
            let v = client.call(&format!("services/{stype}"), "inspectService", ps)?;
            let env = v.get("env").and_then(Value::as_str).unwrap_or("");
            env.lines().map(String::from).collect()
        }
        View::Ports => {
            let v = client.call("ports", "listPorts", ps)?;
            let lines = list_or_empty(&v, "No ports yet — press n to add one", |i, p| {
                format!(
                    "{} {} {}->{}",
                    row_marker(i),
                    field(p, "/protocol"),
                    field(p, "/published"),
                    field(p, "/target")
                )
            });
            // Show the delete hint only when there's a real port (first line starts
            // with "[0]").
            lines
        }
        View::Mounts => {
            let v = client.call("mounts", "listMounts", ps)?;
            let lines = list_or_empty(&v, "No mounts yet — press n to add one", |i, m| {
                let detail = match field(m, "/type").as_str() {
                    "bind" => format!("{} -> {}", field(m, "/hostPath"), field(m, "/mountPath")),
                    "volume" => format!("{} -> {}", field(m, "/name"), field(m, "/mountPath")),
                    _ => field(m, "/mountPath"),
                };
                format!("{} {}  {detail}", row_marker(i), field(m, "/type"))
            });
            lines
        }
        View::Redirects => {
            // Redirects live in inspectService (not a separate list endpoint).
            let v = client.call(&format!("services/{stype}"), "inspectService", ps)?;
            let arr = v
                .get("redirects")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if arr.is_empty() {
                return Ok(vec!["No redirects".into()]);
            }
            let lines: Vec<String> = arr
                .iter()
                .enumerate()
                .map(|(i, r)| {
                    let kind = if r.get("permanent").and_then(Value::as_bool).unwrap_or(false) {
                        "301"
                    } else {
                        "302"
                    };
                    let on = if r.get("enabled").and_then(Value::as_bool).unwrap_or(true) {
                        "on"
                    } else {
                        "off"
                    };
                    format!(
                        "{} {} -> {}  ({kind}, {on})",
                        row_marker(i),
                        field(r, "/regex"),
                        field(r, "/replacement")
                    )
                })
                .collect();
            lines
        }
        View::Source => {
            let v = client.call(&format!("services/{stype}"), "inspectService", ps)?;
            let mut out = Vec::new();
            // Deliberately not showing `token` (deploy token) and `env`: both are
            // credentials, and env already has its own view.
            for (title, key) in [
                ("Source", "source"),
                ("Build", "build"),
                ("Deploy", "deploy"),
                ("Resources", "resources"),
            ] {
                out.push(format!("── {title}"));
                match v.get(key) {
                    // pointer "" = the value itself, so a string shows without quotes.
                    Some(Value::Object(o)) if !o.is_empty() => out.extend(
                        o.iter()
                            .map(|(k, val)| format!("  {k}: {}", field(val, ""))),
                    ),
                    _ => out.push("  (not set)".into()),
                }
                out.push(String::new());
            }
            out
        }
        View::ConfigFile => {
            // configFile (Advanced) lives in inspectService. Returned as-is for
            // editing in $EDITOR (see edit_config_in_editor).
            let v = client.call(&format!("services/{stype}"), "inspectService", ps)?;
            let cf = v.get("configFile").and_then(Value::as_str).unwrap_or("");
            cf.lines().map(String::from).collect()
        }
        View::Backups => {
            let v = client.call("databaseBackups", "listDatabaseBackups", ps)?;
            // The id led every row and nothing here could use it: this view has no
            // run and no delete, and it is not a collection, so there is no
            // selection either. Twenty-five characters of cuid pushed the only
            // thing that tells two rows apart — the database name — off to the
            // right. The CLI prints the same data under labelled columns; this now
            // says what each field is, too.
            let rows = list_or_empty(&v, "No database backups", |_, b| {
                let state = if b.get("enabled").and_then(Value::as_bool).unwrap_or(false) {
                    "on"
                } else {
                    "off"
                };
                format!(
                    "{:<18}{:<16}{state}",
                    field(b, "/databaseName"),
                    field(b, "/schedule")
                )
            });
            // The header only earns its line when there is something under it.
            if v.as_array().is_some_and(|a| !a.is_empty()) {
                let mut out = vec!["Database          Schedule        Enabled".to_string()];
                out.extend(rows);
                out
            } else {
                rows
            }
        }
    };
    Ok(if lines.is_empty() {
        vec!["(empty)".to_string()]
    } else {
        lines
    })
}

/// The list of GitHub repos as "owner/repo" for a dropdown.
///
/// GitHub may not be connected on a given host, and that's no reason to fail the
/// form: an empty list makes "Repo" a plain text input.
pub(super) fn github_repos(client: &EasypanelClient) -> Vec<String> {
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
pub(super) fn env_body(project: &str, service: &str, env: &str, dot: Option<&str>) -> Value {
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
                "'{service}' cloned into '{target}/{new_name}' — press d to deploy (data NOT included)"
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
pub(super) fn parse_task_stats(v: &Value) -> HashMap<String, (i64, i64)> {
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
pub(super) fn parse_services(v: &Value) -> Vec<(String, String)> {
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

pub(super) fn list_or_empty(
    v: &Value,
    empty: &str,
    f: impl Fn(usize, &Value) -> String,
) -> Vec<String> {
    let arr = v.as_array().cloned().unwrap_or_default();
    if arr.is_empty() {
        return vec![empty.to_string()];
    }
    arr.iter().enumerate().map(|(i, x)| f(i, x)).collect()
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
pub(super) fn auto_deploy_error(service: &str, raw: &str) -> String {
    if raw.contains("404") && raw.contains("/hooks") {
        format!("Auto deploy {service}: no webhook access to that repo — usually because it's a third-party repo")
    } else {
        format!("Auto deploy {service} failed: {raw}")
    }
}
