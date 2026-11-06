use super::connections;
use super::{
    model::{OpenPage, PageContext, PageKind, Payload, Target},
    widgets,
};
use crate::{
    config::{Server, ServerConfig},
    form::{self, Field, FieldKind, Form, FormKind},
    worker::{Req, Resp, ServiceSetupPhase},
};
use eframe::egui;
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap};
use std::time::{Duration, Instant};

#[derive(Default)]
pub(super) struct State {
    initialized: bool,
    projects: Vec<String>,
    services: Vec<Value>,
    replicas: HashMap<String, (i64, i64)>,
    detail: Option<Value>,
    ports: Vec<Value>,
    mounts: Vec<Value>,
    domains: Vec<Value>,
    filter: String,
    kind: String,
    sort: usize,
    descending: bool,
    marked: BTreeSet<(String, String, String)>,
    section: String,
    loaded: BTreeSet<String>,
    editor: Option<Editor>,
    pending_editor: Option<Intent>,
    confirmation: Option<Confirmation>,
    confirmation_entry: String,
    error: Option<String>,
    outcome: Vec<String>,
    selected_server: Option<String>,
    branches_for: String,
    domain_project_for: String,
    import_generation: Option<ImportSnapshot>,
    pending_picker: Option<PickerIntent>,
    generation: u64,
    saving_generation: Option<u64>,
    invalidated: BTreeSet<String>,
    actions: Vec<Value>,
    last_poll: Option<Instant>,
    runtime_updated: Option<Instant>,
    pending_mount: Option<(usize, Value)>,
    metrics: Vec<Value>,
    inventory_loaded: bool,
    seen_deletions: usize,
    confirmation_invalidated: bool,
    editor_invalidated: bool,
    deleted: bool,
    recovering: bool,
}
type ImportSnapshot = (u64, &'static str, u64, String);
enum PickerIntent {
    Import(ImportSnapshot),
    Export { server: Server, project: String },
}
struct Editor {
    form: Form,
    intent: Intent,
    baseline: Vec<String>,
    generation: u64,
    revision: u64,
    discard: bool,
}
#[derive(Clone)]
enum Intent {
    Connection(Option<Server>),
    ProjectCreate,
    ServiceCreate,
    ServiceSetup {
        project: String,
        service: String,
        stype: String,
        phase: ServiceSetupPhase,
    },
    Environment,
    Configuration,
    Source,
    Build,
    Resources,
    BulkResources(Vec<(String, String, String)>),
    Deploy,
    Port,
    Mount(Option<usize>),
    Redirect,
    Auth,
    Domain(Option<String>),
}
struct Confirmation {
    title: String,
    target: String,
    typed: bool,
    action: ConfirmAction,
}
enum ConfirmAction {
    Request(Req),
    Indexed {
        req: Req,
        collection: &'static str,
        index: usize,
        original: Value,
    },
    Export {
        path: std::path::PathBuf,
        server: Server,
        project: String,
    },
    RemoveConnection(String),
}
impl State {
    pub(super) fn deleted(&self) -> bool {
        self.deleted
    }
    pub(super) fn sync_deletions(&mut self, target: &Target, notices: &[Target]) {
        for deleted in &notices[self.seen_deletions..] {
            if !target.same_server(deleted) {
                continue;
            }
            let removed = |id: &(String, String, String)| {
                id.0 == deleted.project
                    && (deleted.service.is_empty()
                        || (id.1 == deleted.service && id.2 == deleted.service_type))
            };
            self.marked.retain(|id| !removed(id));
            self.services.retain(|v| {
                v.get("projectName").and_then(Value::as_str) != Some(deleted.project.as_str())
                    || (!deleted.service.is_empty()
                        && (v.get("name").and_then(Value::as_str)
                            != Some(deleted.service.as_str())
                            || v.get("type").and_then(Value::as_str)
                                != Some(deleted.service_type.as_str())))
            });
            if deleted.service.is_empty() {
                self.projects.retain(|p| *p != deleted.project);
            }
            if let Some(confirm) = &self.confirmation {
                self.confirmation_invalidated |= match &confirm.action {
                    ConfirmAction::Request(Req::Action {
                        project,
                        service,
                        stype,
                        ..
                    }) => removed(&(project.clone(), service.clone(), stype.clone())),
                    ConfirmAction::Request(Req::Bulk { targets, .. }) => {
                        targets.iter().any(removed)
                    }
                    ConfirmAction::Request(Req::ProjectDestroy(project)) => {
                        deleted.service.is_empty() && *project == deleted.project
                    }
                    _ => false,
                };
            }
            if let Some(Editor {
                intent: Intent::BulkResources(targets),
                ..
            }) = &self.editor
            {
                self.editor_invalidated |= targets.iter().any(removed);
            }
            if target.deleted_by(deleted) {
                self.deleted = true;
                self.recovering = false;
                self.detail = None;
                self.pending_editor = None;
                self.error = None;
                self.outcome = vec![format!("Deletion completed for {}.", deleted.label())];
                for key in ["detail", "ports", "mounts", "domains"] {
                    self.invalidated.remove(key);
                }
            }
            self.invalidated.insert("inventory".into());
        }
        self.seen_deletions = notices.len();
    }
    pub(super) fn verified_resource(&self) -> bool {
        !self.deleted && self.detail.is_some()
    }
    fn target_missing(&self, target: &(String, String, String)) -> bool {
        self.inventory_loaded && !self.services.iter().any(|v| service_matches(v, target))
    }
    pub(super) fn is_dirty(&self) -> bool {
        self.editor.as_ref().is_some_and(Editor::dirty)
    }
    fn lifecycle_blocked(&self, action: &str) -> bool {
        self.is_dirty() && matches!(action, "deploy" | "start" | "restart")
    }
    pub(super) fn section_label(&self, target: &Target) -> &'static str {
        let section = match self.editor.as_ref().map(|edit| &edit.intent) {
            Some(Intent::Environment) => "environment",
            Some(Intent::Configuration) => "configuration",
            Some(Intent::Source) => "source",
            Some(Intent::Build) => "build",
            Some(Intent::Resources | Intent::BulkResources(_)) => "resources",
            Some(Intent::Deploy) => "deploy",
            Some(Intent::Port) => "ports",
            Some(Intent::Mount(_)) => "mounts",
            Some(Intent::Redirect) => "redirects",
            Some(Intent::Auth) => "auth",
            Some(Intent::Domain(_)) => "domains",
            _ if self.initialized => self.section.as_str(),
            _ => target.section.as_str(),
        };
        match section {
            "environment" => "Environment",
            "configuration" => "Config file",
            "source" => "Source",
            "build" => "Build",
            "resources" => "Resources",
            "deploy" => "Deployment",
            "ports" => "Ports",
            "mounts" => "Mounts",
            "redirects" => "Redirects",
            "auth" => "Basic authentication",
            "domains" => "Domains",
            "backups" => "Backups",
            _ => "Overview",
        }
    }
    fn edit(&mut self, form: Form, intent: Intent) {
        self.editor_invalidated = false;
        self.generation += 1;
        let baseline = form.fields.iter().map(|f| f.value.clone()).collect();
        self.editor = Some(Editor {
            form,
            intent,
            baseline,
            generation: self.generation,
            revision: 0,
            discard: false,
        });
        self.branches_for.clear();
        self.domain_project_for.clear();
    }
    fn confirm(
        &mut self,
        title: impl Into<String>,
        target: impl Into<String>,
        typed: bool,
        action: ConfirmAction,
    ) {
        self.confirmation_invalidated = false;
        self.confirmation_entry.clear();
        self.confirmation = Some(Confirmation {
            title: title.into(),
            target: target.into(),
            typed,
            action,
        });
    }
    fn chosen_path(
        &mut self,
        path: Option<std::path::PathBuf>,
    ) -> Option<(ImportSnapshot, std::path::PathBuf)> {
        let intent = self.pending_picker.take()?;
        let path = path?;
        match intent {
            PickerIntent::Import(snapshot) => {
                let (generation, label, revision, before) = &snapshot;
                if !self.editor.as_ref().is_some_and(|edit| {
                    edit.generation == *generation
                        && edit.revision == *revision
                        && edit
                            .form
                            .fields
                            .iter()
                            .any(|field| field.label == *label && field.value == *before)
                }) {
                    self.error = Some("Import was not applied because its editor changed while choosing the file. Your newer input is preserved.".into());
                    return None;
                }
                Some((snapshot, path))
            }
            PickerIntent::Export { server, project } => {
                self.confirm(
                    "Export configuration — replaces the destination file if it exists",
                    format!("{} · {project} → {}", server.name, path.display()),
                    false,
                    ConfirmAction::Export {
                        path,
                        server,
                        project,
                    },
                );
                None
            }
        }
    }
}
impl Editor {
    fn dirty(&self) -> bool {
        matches!(self.intent, Intent::ServiceSetup { .. })
            || self
                .form
                .fields
                .iter()
                .map(|f| &f.value)
                .ne(self.baseline.iter())
    }
    fn apply_import(
        &mut self,
        generation: u64,
        label: &str,
        revision: u64,
        before: &str,
        content: String,
    ) -> bool {
        if self.generation != generation || self.revision != revision {
            return false;
        }
        let Some(field) = self.form.fields.iter_mut().find(|f| f.label == label) else {
            return false;
        };
        if field.value != before {
            return false;
        }
        field.value = content;
        self.revision += 1;
        true
    }
    fn retain_setup(
        &mut self,
        project: String,
        service: String,
        stype: String,
        phase: ServiceSetupPhase,
    ) {
        self.form.title = format!("Finish setup · {project}/{service}");
        self.form.note=Some("This service already exists. Retry applies only its pending configuration; it never creates another service.".into());
        self.form.error = None;
        self.form.step = match phase {
            ServiceSetupPhase::Source => 1,
            ServiceSetupPhase::AutoDeploy => {
                if let Some(field) = self
                    .form
                    .fields
                    .iter_mut()
                    .find(|f| f.label == "Auto deploy")
                {
                    field.step = 5;
                }
                5
            }
        };
        self.intent = Intent::ServiceSetup {
            project,
            service,
            stype,
            phase,
        };
    }
}
fn text(v: &Value, key: &str) -> String {
    v.pointer(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| {
            v.pointer(key)
                .filter(|v| v.is_number() || v.is_boolean())
                .map(Value::to_string)
        })
        .unwrap_or_default()
}
fn identity(v: &Value) -> (String, String, String) {
    (text(v, "/projectName"), text(v, "/name"), text(v, "/type"))
}
fn service_matches(v: &Value, target: &(String, String, String)) -> bool {
    [
        ("projectName", &target.0),
        ("name", &target.1),
        ("type", &target.2),
    ]
    .into_iter()
    .all(|(key, expected)| v.get(key).and_then(Value::as_str) == Some(expected.as_str()))
}
struct ServiceRow {
    target: (String, String, String),
    runtime: String,
    cpu: Option<f64>,
    memory: Option<f64>,
    cpu_label: String,
    memory_label: String,
    source: String,
    auto_deploy: &'static str,
}
impl ServiceRow {
    fn matches(&self, matcher: &crate::filter::FilterMatcher<'_>) -> bool {
        matcher.matches_any([
            self.target.0.as_str(),
            self.target.1.as_str(),
            self.target.2.as_str(),
            self.runtime.as_str(),
            self.cpu_label.as_str(),
            self.memory_label.as_str(),
            self.source.as_str(),
            self.auto_deploy,
        ])
    }
}
fn action_index(actions: &[Value]) -> HashMap<(&str, &str), &str> {
    let mut index = HashMap::new();
    for action in actions {
        if action.get("type").and_then(Value::as_str) != Some("deployment") {
            continue;
        }
        let (Some(project), Some(service), Some(status)) = (
            action.get("projectName").and_then(Value::as_str),
            action.get("serviceName").and_then(Value::as_str),
            action.get("status").and_then(Value::as_str),
        ) else {
            continue;
        };
        let current = index.entry((project, service)).or_insert(status);
        if matches!(status, "pending" | "running") {
            *current = status;
        }
    }
    index
}
fn source_summary(service: &Value) -> String {
    match text(service, "/source/type").as_str() {
        "github" => format!(
            "{}/{}#{}",
            text(service, "/source/owner"),
            text(service, "/source/repo"),
            text(service, "/source/ref")
        ),
        "git" => {
            let raw = text(service, "/source/repo");
            let repo = match reqwest::Url::parse(&raw) {
                Ok(mut url) => {
                    let _ = url.set_username("");
                    let _ = url.set_password(None);
                    url.set_query(None);
                    url.to_string()
                }
                Err(_) => raw,
            };
            format!("{repo}#{}", text(service, "/source/ref"))
        }
        "image" => text(service, "/source/image"),
        "dockerfile" => "Dockerfile".into(),
        "" => "Not configured".into(),
        kind => kind.to_owned(),
    }
}
fn auto_deploy_summary(service: &Value) -> &'static str {
    match (
        service.pointer("/source/type").and_then(Value::as_str),
        service
            .pointer("/source/autoDeploy")
            .and_then(Value::as_bool),
    ) {
        (Some("github"), Some(true)) => "Enabled",
        (Some("github"), Some(false)) => "Disabled",
        (Some("github"), None) => "Unknown",
        _ => "Not applicable",
    }
}
fn service_rows(state: &State) -> Vec<ServiceRow> {
    let metrics: HashMap<_, _> = state
        .metrics
        .iter()
        .filter_map(|metric| {
            Some((
                (
                    metric.get("projectName")?.as_str()?,
                    metric.get("serviceName")?.as_str()?,
                ),
                metric,
            ))
        })
        .collect();
    let actions = action_index(&state.actions);
    state
        .services
        .iter()
        .map(|service| {
            let target = identity(service);
            let metric = metrics.get(&(target.0.as_str(), target.1.as_str()));
            let cpu = metric
                .and_then(|m| m.get("cpu"))
                .and_then(Value::as_f64)
                .filter(|n| n.is_finite());
            let memory = metric
                .and_then(|m| m.get("memory"))
                .and_then(Value::as_f64)
                .filter(|n| n.is_finite());
            let runtime = runtime_from(
                state
                    .replicas
                    .get(&format!("{}_{}", target.0, target.1))
                    .copied(),
                actions
                    .get(&(target.0.as_str(), target.1.as_str()))
                    .copied(),
            );
            ServiceRow {
                target,
                runtime,
                cpu,
                memory,
                cpu_label: cpu
                    .map(|n| format!("{n:.1}%"))
                    .unwrap_or_else(|| "Unknown".into()),
                memory_label: memory
                    .map(crate::output::format_bytes)
                    .unwrap_or_else(|| "Unknown".into()),
                source: source_summary(service),
                auto_deploy: auto_deploy_summary(service),
            }
        })
        .collect()
}
#[derive(Default)]
struct ProjectUsage {
    services: usize,
    sampled: usize,
    cpu: f64,
    memory: f64,
}
fn project_usage(rows: &[ServiceRow]) -> HashMap<&str, ProjectUsage> {
    let mut usage = HashMap::<&str, ProjectUsage>::new();
    for row in rows {
        let project = usage.entry(row.target.0.as_str()).or_default();
        project.services += 1;
        if let (Some(cpu), Some(memory)) = (row.cpu, row.memory) {
            project.sampled += 1;
            project.cpu += cpu;
            project.memory += memory;
        }
    }
    usage
}
fn usage_label(usage: &ProjectUsage) -> String {
    if usage.sampled == 0 {
        format!("{} services · metrics unavailable", usage.services)
    } else {
        format!(
            "{} services · {:.1}% CPU · {} · {}/{} measured",
            usage.services,
            usage.cpu,
            crate::output::format_bytes(usage.memory),
            usage.sampled,
            usage.services
        )
    }
}
fn input(cx: &PageContext<'_>) -> Value {
    json!({"projectName":cx.target.project,"serviceName":cx.target.service})
}
fn open(
    cx: &mut PageContext<'_>,
    kind: PageKind,
    section: &str,
    target: Option<(String, String, String)>,
) {
    let mut t = cx.target.clone();
    t.section = section.into();
    t.resource = Value::Null;
    if let Some((p, s, k)) = target {
        t.project = p;
        t.service = s;
        t.service_type = k;
    }
    let title = if t.service.is_empty() {
        format!("{} · {section}", t.project)
    } else {
        format!("{} · {section}", t.service)
    };
    cx.open(OpenPage {
        kind,
        target: t,
        title,
    });
}
fn refresh(state: &mut State, cx: &mut PageContext<'_>) {
    state
        .invalidated
        .extend(["inventory", "replicas"].map(str::to_owned));
    if !cx.target.project.is_empty() && !state.deleted {
        load_detail(state, cx);
    }
}
fn load_detail(state: &mut State, _cx: &mut PageContext<'_>) {
    state.invalidated.insert("detail".into());
    for key in ["ports", "mounts", "domains"] {
        state.loaded.remove(key);
        state.invalidated.insert(key.into());
    }
}
fn flush_reads(state: &mut State, cx: &mut PageContext<'_>) {
    for key in ["inventory", "replicas", "detail"] {
        if key == "detail" && state.deleted && !state.recovering {
            state.invalidated.remove(key);
            continue;
        }
        if !state.invalidated.contains(key) || cx.busy(key) {
            continue;
        }
        state.invalidated.remove(key);
        match key {
            "inventory" => cx.request(key, Req::AllServices),
            "replicas" => cx.request(key, Req::TaskStats),
            _ if cx.target.service.is_empty() => cx.rpc(
                key,
                "projects",
                "inspectProject",
                json!({"projectName":cx.target.project}),
            ),
            _ => cx.rpc(
                key,
                &format!("services/{}", cx.target.service_type),
                "inspectService",
                input(cx),
            ),
        }
    }
}
fn poll_runtime(ui: &egui::Ui, state: &mut State, cx: &mut PageContext<'_>) {
    let due = state
        .last_poll
        .is_none_or(|at| at.elapsed() >= Duration::from_secs(3));
    if due {
        if !cx.busy("replicas") && !state.invalidated.contains("replicas") {
            cx.poll_request("replicas", Req::TaskStats);
        }
        if !cx.busy("actions") {
            cx.poll_request("actions", Req::Actions);
        }
        if !cx.busy("metrics") {
            cx.poll_request("metrics", Req::MonitorData);
        }
        state.last_poll = Some(Instant::now());
    }
    ui.ctx().request_repaint_after(Duration::from_secs(1));
}
fn runtime_from(replicas: Option<(i64, i64)>, action: Option<&str>) -> String {
    if matches!(action, Some("pending" | "running")) {
        return format!("Deploying · {}", action.unwrap_or_default());
    }
    let health = match replicas {
        Some((actual, desired)) if desired > 0 && actual < desired => {
            format!("Down / restarting · {actual}/{desired}")
        }
        Some((actual, 0)) => format!("Stopped · {actual}/0"),
        Some((actual, desired)) => format!("Running · {actual}/{desired}"),
        None => "Runtime unknown".into(),
    };
    if action == Some("error") {
        format!("Last deployment failed · {health}")
    } else {
        health
    }
}
fn runtime_label(state: &State, project: &str, service: &str) -> String {
    runtime_from(
        state.replicas.get(&format!("{project}_{service}")).copied(),
        action_index(&state.actions)
            .get(&(project, service))
            .copied(),
    )
}
fn data_available(section: &str, kind: &str) -> bool {
    match section {
        "database" => crate::dbms::Engine::from_service_type(kind).is_some(),
        "dumps" | "restore" | "copy" => crate::dump::can_dump(kind),
        "restore-from" => crate::backup::can_back_up(kind),
        "db" | "credentials" => crate::lifecycle::is_database(kind),
        _ => true,
    }
}
fn validate_index(rows: &Value, index: usize, original: &Value) -> anyhow::Result<()> {
    if rows.as_array().and_then(|rows| rows.get(index)) != Some(original) {
        anyhow::bail!("The selected entry changed or moved. Refresh and review it again; nothing was changed.");
    }
    Ok(())
}
fn guarded_operation(
    client: &crate::client::EasypanelClient,
    req: Req,
    collection: &str,
    index: usize,
    original: &Value,
    emit: &crate::worker::Emit,
) -> anyhow::Result<Payload> {
    let (project, service, stype) = match &req {
        Req::MountUpdate {
            project, service, ..
        }
        | Req::MountDelete {
            project, service, ..
        }
        | Req::PortDelete {
            project, service, ..
        } => (project, service, ""),
        Req::RedirectDelete {
            project,
            service,
            stype,
            ..
        } => (project, service, stype.as_str()),
        _ => anyhow::bail!("This operation has no indexed review"),
    };
    let input = json!({"projectName":project,"serviceName":service});
    let rows = match collection {
        "mounts" => client.call("mounts", "listMounts", input)?,
        "ports" => client.call("ports", "listPorts", input)?,
        "redirects" => client
            .call(&format!("services/{stype}"), "inspectService", input)?
            .get("redirects")
            .cloned()
            .unwrap_or_else(|| json!([])),
        _ => anyhow::bail!("Unknown indexed collection"),
    };
    validate_index(&rows, index, original)?;
    Ok(Payload::Worker(crate::worker::handle_req(
        client, req, emit,
    )))
}
fn guarded_request(
    cx: &mut PageContext<'_>,
    req: Req,
    collection: &'static str,
    index: usize,
    original: Value,
) {
    let Some(server) = cx.target.server.clone() else {
        return;
    };
    cx.background_progress(
        "mutation",
        "Validate reviewed entry and apply change",
        move |progress| {
            let emit: crate::worker::Emit = std::sync::Arc::new(move |response| {
                if let Resp::Progress(message) = response {
                    progress(message);
                }
            });
            guarded_operation(
                &crate::client::EasypanelClient::for_server(&server),
                req,
                collection,
                index,
                &original,
                &emit,
            )
        },
    );
}
fn result_message(state: &mut State, response: Resp) -> bool {
    match response {
        Resp::Err(e) | Resp::AllServicesErr(e) | Resp::DomainsErr(e) => {
            state.error = Some(e);
            false
        }
        Resp::Done(msg, _) => {
            state.outcome = vec![msg];
            true
        }
        Resp::Notes { msg, notes, .. } => {
            state.outcome = vec![msg];
            state.outcome.extend(notes);
            true
        }
        Resp::BulkDone { action, ok, failed } => {
            let deploying = action == "deploy";
            state.outcome = vec![format!(
                "{action}: {} {}, {} failed",
                ok.len(),
                if deploying { "dispatched" } else { "succeeded" },
                failed.len()
            )];
            state.outcome.extend(ok.into_iter().map(|n| {
                format!(
                    "{}: {n}",
                    if deploying { "Dispatched" } else { "Completed" }
                )
            }));
            if deploying {
                state.outcome.push("Watch Logs and action history for completion; dispatch does not mean deployment is ready.".into());
            }
            state
                .outcome
                .extend(failed.into_iter().map(|(n, e)| format!("Failed: {n}: {e}")));
            true
        }
        Resp::ProjectEnvSaved(p) => {
            state.outcome = vec![format!(
                "Environment saved for {p}. Deploy its services to apply the new values."
            )];
            true
        }
        Resp::Viewer(title, _) => {
            state.outcome = vec![title];
            true
        }
        Resp::Fetched { .. } => {
            state.outcome = vec!["Configuration updated. Refreshing the resource.".into()];
            true
        }
        _ => {
            state.error =
                Some("Unexpected operation response. Refresh the resource before retrying.".into());
            false
        }
    }
}
fn pump(state: &mut State, cx: &mut PageContext<'_>) {
    state.sync_deletions(cx.target, &cx.backend.deletions);
    if let Some(result) = cx.take("native-picker") {
        match result {
            Ok(Payload::ChosenPath(path)) => {
                if let Some((snapshot, path)) = state.chosen_path(path) {
                    state.import_generation = Some(snapshot);
                    cx.background("import", "Read selected text file", move || {
                        Ok(Payload::Text(std::fs::read_to_string(path)?))
                    });
                }
            }
            Err(error) => {
                state.pending_picker = None;
                state.error = Some(error);
            }
            _ => {
                state.pending_picker = None;
            }
        }
    }
    if let Some(r) = cx.take("inventory") {
        if !state.invalidated.contains("inventory") {
            match r {
                Ok(Payload::Worker(Resp::AllServices { projects, services })) => {
                    state.projects = projects;
                    state.services = services;
                    state.inventory_loaded = true;
                    state
                        .marked
                        .retain(|target| state.services.iter().any(|v| service_matches(v, target)));
                }
                Ok(Payload::Worker(r)) => {
                    result_message(state, r);
                }
                Err(e) => state.error = Some(e),
                _ => {}
            }
        }
    }
    if let Some(r) = cx.take("replicas") {
        if !state.invalidated.contains("replicas") {
            match r {
                Ok(Payload::Worker(Resp::TaskStats(v))) => {
                    state.replicas = v;
                    state.runtime_updated = Some(Instant::now());
                }
                Err(e) | Ok(Payload::Worker(Resp::Err(e))) => state.error = Some(e),
                _ => {}
            }
        }
    }
    if let Some(r) = cx.take("actions") {
        match r {
            Ok(Payload::Worker(Resp::Actions(v))) => state.actions = v,
            Err(e) | Ok(Payload::Worker(Resp::Err(e))) => state.error = Some(e),
            _ => {}
        }
    }
    if let Some(r) = cx.take("metrics") {
        match r {
            Ok(Payload::Worker(Resp::MonitorData(rows))) => state.metrics = rows,
            Err(e) | Ok(Payload::Worker(Resp::Err(e))) => state.error = Some(e),
            _ => {}
        }
    }
    if let Some(r) = cx.take("detail") {
        if !state.invalidated.contains("detail") && (!state.deleted || state.recovering) {
            match r {
                Ok(Payload::Json(v)) => {
                    state.detail = Some(v);
                    if state.recovering {
                        state.error = None;
                    }
                    state.deleted = false;
                    state.recovering = false;
                    if let Some(intent) = state.pending_editor.take() {
                        begin_edit(state, cx, intent);
                    }
                }
                Err(e) => {
                    state.recovering = false;
                    state.error = Some(e);
                    state.pending_editor = None;
                }
                _ => {}
            }
        }
    }
    for key in ["ports", "mounts"] {
        if let Some(r) = cx.take(key) {
            if !state.deleted && !state.invalidated.contains(key) {
                match r {
                    Ok(Payload::Json(v)) => {
                        let rows = v.as_array().cloned().unwrap_or_default();
                        if key == "ports" {
                            state.ports = rows;
                        } else {
                            state.mounts = rows;
                        }
                        state.loaded.insert(key.into());
                    }
                    Err(e) => {
                        state.error = Some(e);
                        state.loaded.insert(key.into());
                    }
                    _ => {}
                }
            }
        }
    }
    if let Some(r) = cx.take("domains") {
        if !state.deleted && !state.invalidated.contains("domains") {
            match r {
                Ok(Payload::Worker(Resp::Domains(v))) => {
                    state.domains = v;
                    state.loaded.insert("domains".into());
                }
                Ok(Payload::Worker(r)) => {
                    result_message(state, r);
                    state.loaded.insert("domains".into());
                }
                Err(e) => {
                    state.error = Some(e);
                    state.loaded.insert("domains".into());
                }
                _ => {}
            }
        }
    }
    if let Some(r) = cx.take("mount-edit") {
        match r {
            Ok(Payload::Worker(Resp::MountForm { index, values, .. })) => {
                let expected = state.pending_mount.take();
                if state.editor.is_none()
                    && expected
                        .as_ref()
                        .is_some_and(|(i, original)| *i == index && *original == values)
                {
                    let f=Form::new(FormKind::MountEdit{project:cx.target.project.clone(),service:cx.target.service.clone(),index},"Edit mount",form::mount_fields(Some(&values))).with_original(values)
                        .with_note("Saving empty contents on an existing file mount truncates the file. Discard leaves its original contents unchanged.");
                    state.edit(f, Intent::Mount(Some(index)));
                } else {
                    state.error = Some(
                        "Mount changed while opening the editor. Refresh and review it again."
                            .into(),
                    );
                }
            }
            Ok(Payload::Worker(r)) => {
                result_message(state, r);
            }
            Err(e) => state.error = Some(e),
            _ => {}
        }
    }
    if let Some(r) = cx.take("repos") {
        match r {
            Ok(Payload::Worker(Resp::Repos(mut names))) => {
                if let Some(e) = state.editor.as_mut() {
                    if let Some(f) = e.form.fields.iter_mut().find(|f| f.label == "Repo") {
                        if names.is_empty() {
                            f.kind = FieldKind::Text;
                        } else {
                            if f.value.is_empty() {
                                names.insert(0, String::new());
                            }
                            f.set_options(names);
                        }
                    }
                }
            }
            Err(e) => state.error = Some(e),
            _ => {}
        }
    }
    if let Some(r) = cx.take("branches") {
        if let Some(e) = state.editor.as_mut() {
            if e.form.by_label("Repo") == state.branches_for {
                if let Some(f) = e.form.fields.iter_mut().find(|f| f.label == "Branch") {
                    match r {
                        Ok(Payload::Worker(Resp::Branches(Ok(names)))) => f.set_options(names),
                        Ok(Payload::Worker(Resp::Branches(Err(why)))) | Err(why) => {
                            f.kind = FieldKind::Text;
                            state.error = Some(format!(
                                "Branches could not load: {why}. Enter a branch manually."
                            ));
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    if let Some(r) = cx.take("import") {
        let import = state.import_generation.take();
        match r {
            Ok(Payload::Text(content)) => {
                if let Some((generation, label, revision, before)) = import {
                    if let Some(e) = state.editor.as_mut() {
                        if !e.apply_import(generation, label, revision, &before, content) {
                            state.error=Some("Import was not applied because its editor changed while the file was loading. Your newer input is preserved.".into());
                        }
                    }
                }
            }
            Err(e) => state.error = Some(e),
            _ => {}
        }
    }
    if let Some(r) = cx.take("domain-services") {
        match r {
            Ok(Payload::Worker(Resp::ServicesFor(project, mut names))) => {
                if let Some(e) = state
                    .editor
                    .as_mut()
                    .filter(|e| e.form.by_label("Project") == project)
                {
                    if let Some(f) = e.form.fields.iter_mut().find(|f| f.label == "Service") {
                        if f.value.is_empty() {
                            names.insert(0, String::new());
                        }
                        f.set_options(names);
                    }
                }
            }
            Err(e) | Ok(Payload::Worker(Resp::Err(e))) => state.error = Some(e),
            _ => {}
        }
    }
    if let Some(r) = cx.take("mutation") {
        let saving = state.saving_generation.take();
        let mut retain_editor = false;
        let success = match r {
            Ok(Payload::Worker(Resp::ServiceCreated {
                project,
                service,
                stype,
                notes,
                pending,
            })) => {
                state.outcome = vec![format!(
                    "Service {project}/{service} exists. {}",
                    if pending.is_some() {
                        "Some configuration still needs attention."
                    } else {
                        "Deploy when ready."
                    }
                )];
                state.outcome.extend(notes);
                refresh(state, cx);
                if let Some(phase) = pending {
                    if let Some(edit) = state
                        .editor
                        .as_mut()
                        .filter(|e| Some(e.generation) == saving)
                    {
                        edit.retain_setup(project, service, stype, phase);
                        retain_editor = true;
                    }
                } else {
                    open(
                        cx,
                        PageKind::Service,
                        "overview",
                        Some((project, service, stype)),
                    );
                }
                true
            }
            Ok(Payload::Worker(r)) => {
                let success = result_message(state, r);
                refresh(state, cx);
                success
            }
            Ok(Payload::Text(msg)) => {
                state.outcome = vec![msg];
                cx.reload_connections();
                true
            }
            Err(e) => {
                state.error = Some(e);
                if cx.target.server.is_some() {
                    refresh(state, cx);
                }
                false
            }
            _ => false,
        };
        if success {
            if !retain_editor
                && !state.deleted
                && state
                    .editor
                    .as_ref()
                    .is_some_and(|e| Some(e.generation) == saving)
            {
                state.editor = None;
            }
            state.error = None;
        }
    }
    for key in ["connection-test", "export"] {
        if let Some(r) = cx.take(key) {
            match r {
                Ok(Payload::Text(msg)) => state.outcome = vec![msg],
                Err(e) => state.error = Some(e),
                _ => {}
            }
        }
    }
}

pub(super) fn show(ui: &mut egui::Ui, state: &mut State, cx: &mut PageContext<'_>) {
    pump(state, cx);
    if !state.initialized {
        state.initialized = true;
        state.section = if cx.target.section.is_empty() {
            "overview".into()
        } else {
            cx.target.section.clone()
        };
        refresh(state, cx);
    }
    flush_reads(state, cx);
    poll_runtime(ui, state, cx);
    if state.deleted {
        feedback(ui, state, cx);
        ui.heading("Resource deleted");
        ui.add(egui::Label::new(cx.target.label()).wrap());
        if let Some(server) = &cx.target.server {
            ui.add(egui::Label::new(&server.url).wrap());
        }
        ui.label(format!(
            "Type: {}",
            if cx.target.service.is_empty() {
                "project"
            } else {
                &cx.target.service_type
            }
        ));
        ui.label("The confirmed deletion succeeded. This document keeps its original target.");
        if state.editor.is_some() {
            ui.label("Your unsaved draft is retained. It is not applied to the deleted resource.");
        }
        ui.horizontal(|ui| {
            if ui.button("Open inventory").clicked() {
                let mut target = cx.target.clone();
                target.project.clear();
                target.service.clear();
                target.service_type.clear();
                target.section = "services".into();
                target.resource = Value::Null;
                cx.open(OpenPage {
                    kind: PageKind::Project,
                    target,
                    title: "Projects".into(),
                });
            }
            if ui
                .add_enabled(!state.recovering, egui::Button::new("Refresh"))
                .clicked()
            {
                state.recovering = true;
                refresh(state, cx);
                load_detail(state, cx);
            }
        });
        ui.label("Refresh checks whether this exact target has been recreated.");
        flush_reads(state, cx);
        return;
    }
    if state.confirmation.is_some() {
        feedback(ui, state, cx);
        confirmation(ui, state, cx);
        return;
    }
    if cx.target.service.is_empty() && state.editor.is_none() {
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    !cx.busy("inventory") && state.editor.is_none(),
                    egui::Button::new("Refresh"),
                )
                .clicked()
            {
                refresh(state, cx);
            }
            if ui
                .add_enabled(
                    state.editor.is_none() && !cx.busy("mutation"),
                    egui::Button::new("New project"),
                )
                .clicked()
            {
                state.edit(
                    Form::new(
                        FormKind::ProjectCreate,
                        "New project",
                        vec![Field::text("Name", "")],
                    ),
                    Intent::ProjectCreate,
                );
            }
            if ui
                .add_enabled(
                    !state.projects.is_empty() && state.editor.is_none() && !cx.busy("mutation"),
                    egui::Button::new("New service"),
                )
                .clicked()
            {
                state.edit(
                    form::service_create_form(&state.projects, &cx.target.project),
                    Intent::ServiceCreate,
                );
                cx.request("repos", Req::Repos);
            }
            if ui.button("Host tools").clicked() {
                open(
                    cx,
                    PageKind::Host,
                    "overview",
                    Some((String::new(), String::new(), String::new())),
                );
            }
        });
        ui.separator();
    }
    feedback(ui, state, cx);
    if cx.target.service.is_empty() && state.editor.is_none() {
        let rows = service_rows(state);
        if cx.target.project.is_empty() {
            project_list(ui, state, cx, &rows);
            inventory(ui, state, cx, &rows);
        } else {
            project(ui, state, cx, &rows);
        }
    } else if !cx.target.service.is_empty() {
        service(ui, state, cx);
    }
    editor(ui, state, cx);
    if state.confirmation.is_some() {
        ui.ctx().request_repaint();
    }
    flush_reads(state, cx);
}
fn feedback(ui: &mut egui::Ui, state: &mut State, cx: &PageContext<'_>) {
    if state.error.is_none()
        && state.outcome.is_empty()
        && ![
            "mutation",
            "detail",
            "inventory",
            "export",
            "connection-test",
        ]
        .iter()
        .any(|key| cx.busy(key))
    {
        return;
    }
    ui.horizontal(|ui| {
        if let Some(error) = &state.error {
            ui.menu_button(
                egui::RichText::new("Operation error").color(ui.visuals().error_fg_color),
                |ui| {
                    ui.set_max_width(400.0);
                    widgets::error(ui, error);
                },
            );
        }
        if !state.outcome.is_empty() {
            ui.menu_button("Operation result", |ui| {
                ui.set_max_width(400.0);
                egui::ScrollArea::vertical()
                    .max_height(200.0)
                    .show(ui, |ui| {
                        for line in &state.outcome {
                            ui.add(egui::Label::new(line).wrap());
                        }
                    });
            });
        }
        for key in [
            "mutation",
            "detail",
            "inventory",
            "export",
            "connection-test",
        ] {
            if cx.busy(key) {
                ui.spinner();
                let progress = cx.progress(key).unwrap_or(key);
                ui.add(egui::Label::new(progress).truncate())
                    .on_hover_text(progress);
                break;
            }
        }
    });
}
fn project_list(
    ui: &mut egui::Ui,
    state: &mut State,
    cx: &mut PageContext<'_>,
    rows: &[ServiceRow],
) {
    let matcher = crate::filter::FilterMatcher::new(&state.filter);
    let usage = project_usage(rows);
    let visible: BTreeSet<_> = rows
        .iter()
        .filter(|row| row.matches(&matcher))
        .map(|row| row.target.0.as_str())
        .collect();
    ui.menu_button(format!("{} projects", state.projects.len()), |ui| {
        egui::ScrollArea::vertical()
            .id_salt("project-summary")
            .max_height(260.0)
            .show(ui, |ui| {
                for project in &state.projects {
                    if !matcher.matches(project) && !visible.contains(project.as_str()) {
                        continue;
                    }
                    ui.horizontal(|ui| {
                        if ui.button(project).clicked() {
                            open(
                                cx,
                                PageKind::Project,
                                "overview",
                                Some((project.clone(), String::new(), String::new())),
                            );
                            ui.close();
                        }
                        if let Some(usage) = usage.get(project.as_str()) {
                            ui.label(usage_label(usage));
                        } else {
                            ui.label("0 services");
                        }
                    });
                }
            });
    });
}
fn project(ui: &mut egui::Ui, state: &mut State, cx: &mut PageContext<'_>, rows: &[ServiceRow]) {
    ui.horizontal_wrapped(|ui| {
        ui.strong(&cx.target.project);
        if ui.button("Shared environment").clicked() && state.editor.is_none() {
            state.pending_editor = Some(Intent::Environment);
            load_detail(state, cx);
        }
        if ui.button("Export project…").clicked() {
            export_project(state, cx);
        }
        for (label, section) in [
            ("Migrate config", "migrate"),
            ("Compare hosts", "project-diff"),
        ] {
            if ui.button(label).clicked() {
                open(cx, PageKind::Data, section, None);
            }
        }
        if ui
            .add_enabled(state.editor.is_none(), egui::Button::new("Delete project"))
            .clicked()
        {
            state.confirm(
                "Delete project and all its services",
                cx.target.project.clone(),
                true,
                ConfirmAction::Request(Req::ProjectDestroy(cx.target.project.clone())),
            );
        }
    });
    if let Some(detail) = &state.detail {
        if let Some(p) = detail.get("project") {
            ui.label(format!("Created {}", text(p, "/createdAt")));
        }
    }
    if let Some(usage) = project_usage(rows).get(cx.target.project.as_str()) {
        ui.label(usage_label(usage));
    }
    inventory(ui, state, cx, rows);
    if state.section == "environment" && state.editor.is_none() && state.detail.is_some() {
        begin_edit(state, cx, Intent::Environment);
        state.section = "overview".into();
    }
    if state.section == "export" && ui.button("Choose export destination…").clicked() {
        export_project(state, cx);
    }
}
fn inventory(
    ui: &mut egui::Ui,
    state: &mut State,
    cx: &mut PageContext<'_>,
    all_rows: &[ServiceRow],
) {
    ui.horizontal(|ui| {
        let label = ui.label("Filter");
        let width = (ui.available_width() - 240.0).max(100.0);
        ui.add(
            egui::TextEdit::singleline(&mut state.filter)
                .hint_text("Substring or regex · all columns")
                .desired_width(width),
        )
        .labelled_by(label.id);
        egui::ComboBox::from_id_salt("service-kind")
            .selected_text(if state.kind.is_empty() {
                "All types"
            } else {
                &state.kind
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut state.kind, String::new(), "All types");
                for kind in form::SERVICE_TYPES {
                    ui.selectable_value(&mut state.kind, kind.to_string(), *kind);
                }
            });
        ui.menu_button("Sort", |ui| {
            for (i, label) in [
                "Project",
                "Service",
                "Type",
                "Runtime",
                "CPU",
                "Memory",
                "Source",
                "Auto deploy",
            ]
            .iter()
            .enumerate()
            {
                ui.selectable_value(&mut state.sort, i, *label);
            }
            ui.separator();
            ui.checkbox(&mut state.descending, "Descending");
        });
    });
    let matcher = crate::filter::FilterMatcher::new(&state.filter);
    let mut rows: Vec<_> = all_rows
        .iter()
        .filter(|row| {
            (cx.target.project.is_empty() || row.target.0 == cx.target.project)
                && (state.kind.is_empty() || row.target.2 == state.kind)
                && row.matches(&matcher)
        })
        .collect();
    rows.sort_by(|a, b| match state.sort {
        1 => a.target.1.cmp(&b.target.1),
        2 => a.target.2.cmp(&b.target.2),
        3 => a.runtime.cmp(&b.runtime),
        4 => a.cpu.unwrap_or(-1.0).total_cmp(&b.cpu.unwrap_or(-1.0)),
        5 => a
            .memory
            .unwrap_or(-1.0)
            .total_cmp(&b.memory.unwrap_or(-1.0)),
        6 => a.source.cmp(&b.source),
        7 => a.auto_deploy.cmp(b.auto_deploy),
        _ => a.target.cmp(&b.target),
    });
    if state.descending {
        rows.reverse();
    }
    ui.horizontal_wrapped(|ui| {
        if ui.button("Select shown").clicked() {
            state
                .marked
                .extend(rows.iter().map(|row| row.target.clone()));
        }
        if ui.button("Clear selection").clicked() {
            state.marked.clear();
        }
        ui.label(format!(
            "{} services · {} selected",
            rows.len(),
            state.marked.len()
        ));
        if state.marked.len() == 2 {
            if let Some(server) = cx.target.server.clone() {
                if ui.button("Compare 2 marked").clicked() {
                    let mut targets = state.marked.iter().cloned();
                    if let (Some(a), Some(b)) = (targets.next(), targets.next()) {
                        cx.open(super::data::compare_services_page(&server, a, b));
                    }
                }
            }
        }
        if !state.marked.is_empty() {
            ui.menu_button("Selection actions", |ui| {
                for (label, action, force) in [
                    ("Deploy", "deploy", false),
                    ("Rebuild", "deploy", true),
                    ("Start", "start", false),
                    ("Stop", "stop", false),
                    ("Restart", "restart", false),
                    ("Destroy", "destroy", false),
                ] {
                    let targets: Vec<_> = state
                        .marked
                        .iter()
                        .filter(|(_, _, kind)| crate::lifecycle::ops(kind, action).is_some())
                        .cloned()
                        .collect();
                    let skipped = state.marked.len() - targets.len();
                    if ui
                        .add_enabled(
                            !targets.is_empty()
                                && !cx.busy("mutation")
                                && !state.lifecycle_blocked(action),
                            egui::Button::new(label),
                        )
                        .on_hover_text(format!(
                            "{} eligible, {skipped} unsupported selected services",
                            targets.len()
                        ))
                        .clicked()
                    {
                        let names = targets
                            .iter()
                            .map(|(p, s, _)| format!("{p}/{s}"))
                            .collect::<Vec<_>>()
                            .join(", ");
                        state.confirm(
                            format!(
                                "{label} {} eligible services ({skipped} unsupported excluded)",
                                targets.len()
                            ),
                            names,
                            action == "destroy",
                            ConfirmAction::Request(Req::Bulk {
                                targets,
                                action: action.into(),
                                force,
                            }),
                        );
                        ui.close();
                    }
                }
                if ui
                    .add_enabled(
                        state.editor.is_none() && !cx.busy("mutation"),
                        egui::Button::new("Set resources"),
                    )
                    .clicked()
                {
                    let targets = state.marked.iter().cloned().collect();
                    state.edit(
                        Form::new(
                            FormKind::BulkResourceEdit,
                            "Resources for selected services",
                            form::resource_fields(None),
                        )
                        .with_note(
                            "0 = unlimited. Per-service failures are reported individually.",
                        ),
                        Intent::BulkResources(targets),
                    );
                    ui.close();
                }
            });
        }
    });
    if rows.is_empty() && !cx.busy("inventory") {
        ui.label("No services match. Clear the filter or create a service.");
    }
    if state.editor.is_some() {
        return;
    }
    let height = ui.available_height().max(28.0);
    egui::ScrollArea::horizontal()
        .id_salt("service-inventory")
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
        .max_height(height)
        .show(ui, |ui| {
            let widths = [32.0, 204.0, 164.0, 112.0, 64.0, 56.0, 80.0, 160.0, 84.0];
            let width: f32 = widths.iter().sum();
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                for (label, width) in [
                    "Select",
                    "Service",
                    "Runtime",
                    "Project",
                    "Type",
                    "CPU",
                    "Memory",
                    "Source",
                    "Auto deploy",
                ]
                .into_iter()
                .zip(widths)
                {
                    ui.add_sized(
                        [width, 28.0],
                        egui::Label::new(egui::RichText::new(label).strong()),
                    );
                }
            });
            egui::ScrollArea::vertical()
                .id_salt("service-rows")
                .show_rows(ui, 28.0, rows.len(), |ui, range| {
                    for index in range {
                        let row = rows[index];
                        let id = &row.target;
                        let (rect, response) =
                            ui.allocate_exact_size(egui::vec2(width, 28.0), egui::Sense::click());
                        let marked = state.marked.contains(id);
                        let fill = if marked {
                            ui.visuals().selection.bg_fill
                        } else if response.hovered() {
                            ui.visuals().widgets.hovered.bg_fill
                        } else if index % 2 == 1 {
                            ui.visuals().faint_bg_color
                        } else {
                            egui::Color32::TRANSPARENT
                        };
                        ui.painter().rect_filled(rect, 0.0, fill);
                        response.widget_info(|| {
                            egui::WidgetInfo::selected(
                                egui::Role::Button,
                                ui.is_enabled(),
                                marked,
                                format!(
                                    "{}/{} · {} · {} · Source {} · Auto deploy {} · {} CPU · {}",
                                    id.0,
                                    id.1,
                                    id.2,
                                    row.runtime,
                                    row.source,
                                    row.auto_deploy,
                                    row.cpu_label,
                                    row.memory_label
                                ),
                            )
                        });
                        if response.has_focus() {
                            ui.painter().rect_stroke(
                                rect.shrink(1.0),
                                2.0,
                                ui.visuals().selection.stroke,
                                egui::StrokeKind::Inside,
                            );
                        }
                        let mut changed = false;
                        ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 0.0;
                                let mut checked = marked;
                                let selection = ui
                                    .add_sized(
                                        [widths[0], 28.0],
                                        egui::Checkbox::without_text(&mut checked),
                                    )
                                    .on_hover_text(format!(
                                        "Select {}/{} for bulk actions",
                                        id.0, id.1
                                    ));
                                selection.widget_info(|| {
                                    egui::WidgetInfo::selected(
                                        egui::Role::CheckBox,
                                        ui.is_enabled(),
                                        checked,
                                        format!("Select {}/{} for bulk actions", id.0, id.1),
                                    )
                                });
                                changed = selection.changed();
                                if changed {
                                    if checked {
                                        state.marked.insert(id.clone());
                                    } else {
                                        state.marked.remove(id);
                                    }
                                }
                                for (value, width) in [
                                    id.1.as_str(),
                                    row.runtime.as_str(),
                                    id.0.as_str(),
                                    id.2.as_str(),
                                    row.cpu_label.as_str(),
                                    row.memory_label.as_str(),
                                    row.source.as_str(),
                                    row.auto_deploy,
                                ]
                                .into_iter()
                                .zip(widths[1..].iter())
                                {
                                    ui.add_sized(
                                        [*width, 28.0],
                                        egui::Label::new(value).truncate(),
                                    )
                                    .on_hover_text(value);
                                }
                            });
                        });
                        if !changed
                            && (response.clicked()
                                || (response.has_focus()
                                    && ui.input(|i| i.key_pressed(egui::Key::Enter))))
                        {
                            open(cx, PageKind::Service, "overview", Some(id.clone()));
                        }
                        response.context_menu(|ui| {
                            for (label, section) in [
                                ("Open service", "overview"),
                                ("Environment", "environment"),
                                ("Source", "source"),
                                ("Build", "build"),
                                ("Deployment", "deploy"),
                                ("Resources", "resources"),
                                ("Ports", "ports"),
                                ("Mounts", "mounts"),
                                ("Domains", "domains"),
                                ("Redirects", "redirects"),
                                ("Basic authentication", "auth"),
                                ("Config file", "configuration"),
                                ("Backups", "backups"),
                            ] {
                                let applicable = match section {
                                    "source" | "build" | "deploy" => id.2 == "app",
                                    "resources" => crate::lifecycle::has_resource_limits(&id.2),
                                    "ports" | "mounts" => {
                                        crate::lifecycle::has_mounts_and_ports(&id.2)
                                    }
                                    "domains" | "redirects" | "auth" => {
                                        crate::lifecycle::is_web(&id.2)
                                    }
                                    "configuration" => crate::lifecycle::has_config_file(&id.2),
                                    _ => true,
                                };
                                if applicable && ui.button(label).clicked() {
                                    open(cx, PageKind::Service, section, Some(id.clone()));
                                    ui.close();
                                }
                            }
                            ui.separator();
                            for (label, section, kind) in [
                                ("Logs", "logs", PageKind::Terminal),
                                ("Container terminal", "container", PageKind::Terminal),
                                ("Database terminal", "db", PageKind::Terminal),
                                ("Database workspace", "database", PageKind::Data),
                                ("Database credentials", "credentials", PageKind::Data),
                                ("Copy databases", "copy", PageKind::Data),
                                ("Dump databases", "dumps", PageKind::Data),
                                ("Restore databases", "restore", PageKind::Data),
                                ("Clone configuration", "clone", PageKind::Data),
                                ("Migrate configuration", "migrate", PageKind::Data),
                                ("Compare services", "diff", PageKind::Data),
                            ] {
                                if !data_available(section, &id.2) {
                                    continue;
                                }
                                if ui.button(label).clicked() {
                                    open(cx, kind, section, Some(id.clone()));
                                    ui.close();
                                }
                            }
                            ui.separator();
                            for (label, action, force) in [
                                ("Deploy", "deploy", false),
                                ("Rebuild", "deploy", true),
                                ("Start", "start", false),
                                ("Stop", "stop", false),
                                ("Restart", "restart", false),
                                ("Destroy", "destroy", false),
                            ] {
                                if crate::lifecycle::ops(&id.2, action).is_some()
                                    && ui
                                        .add_enabled(!cx.busy("mutation"), egui::Button::new(label))
                                        .clicked()
                                {
                                    lifecycle(state, id.clone(), label, action, force);
                                    ui.close();
                                }
                            }
                        });
                    }
                });
        });
}
#[derive(Clone, Copy)]
pub(super) enum QuickAction {
    Deploy,
    Rebuild,
    Start,
    Stop,
    Restart,
    Destroy,
}
pub(super) fn quick_action(state: &mut State, cx: &mut PageContext<'_>, action: QuickAction) {
    if state.deleted {
        state.error =
            Some("This target was deleted. Open inventory or explicitly refresh it first.".into());
        return;
    }
    if cx.busy("mutation") {
        state.error = Some("Wait for the current service operation to finish.".into());
        return;
    }
    if state.confirmation.is_some() {
        state.error = Some("Finish or cancel the current action review first.".into());
        return;
    }
    let (label, verb, force) = match action {
        QuickAction::Deploy => ("Deploy", "deploy", false),
        QuickAction::Rebuild => ("Rebuild", "deploy", true),
        QuickAction::Start => ("Start", "start", false),
        QuickAction::Stop => ("Stop", "stop", false),
        QuickAction::Restart => ("Restart", "restart", false),
        QuickAction::Destroy => ("Destroy", "destroy", false),
    };
    if cx.target.project.is_empty() || cx.target.service.is_empty() {
        state.error = Some("Choose a service first.".into());
        return;
    }
    if crate::lifecycle::ops(&cx.target.service_type, verb).is_none() {
        state.error = Some(crate::lifecycle::unavailable(&cx.target.service_type, verb));
        return;
    }
    lifecycle(
        state,
        (
            cx.target.project.clone(),
            cx.target.service.clone(),
            cx.target.service_type.clone(),
        ),
        label,
        verb,
        force,
    );
}
fn lifecycle(
    state: &mut State,
    (project, service, stype): (String, String, String),
    label: &str,
    action: &str,
    force: bool,
) {
    if state.deleted {
        return;
    }
    if state.lifecycle_blocked(action) {
        state.error = Some(format!(
            "Save or discard this editor's changes before {label}. Your draft has not been applied."
        ));
        return;
    }
    let target = format!("{project}/{service}");
    let warning = match action {
        "destroy" => " — may permanently remove stored data",
        "restart" | "stop" => " — interrupts the running service",
        "deploy" => " — changes the running workload",
        _ => "",
    };
    state.confirm(
        format!("{label} service{warning}"),
        target,
        action == "destroy",
        ConfirmAction::Request(Req::Action {
            project,
            service,
            stype,
            action: action.into(),
            force,
        }),
    );
}
fn service_section(ui: &mut egui::Ui, state: &mut State, cx: &mut PageContext<'_>, width: f32) {
    let kind = cx.target.service_type.as_str();
    let mut sections = vec![("overview", "Overview"), ("environment", "Environment")];
    if kind == "app" {
        sections.extend([
            ("source", "Source"),
            ("build", "Build"),
            ("deploy", "Deployment"),
        ]);
    }
    if crate::lifecycle::has_resource_limits(kind) {
        sections.push(("resources", "Resources"));
    }
    if crate::lifecycle::has_config_file(kind) {
        sections.push(("configuration", "Config file"));
    }
    if crate::lifecycle::has_mounts_and_ports(kind) {
        sections.extend([("ports", "Ports"), ("mounts", "Mounts")]);
    }
    if crate::lifecycle::is_web(kind) {
        sections.extend([
            ("domains", "Domains"),
            ("redirects", "Redirects"),
            ("auth", "Basic auth"),
        ]);
    }
    sections.push(("backups", "Backups"));
    let label = ui.label("Section");
    ui.allocate_ui_with_layout(
        egui::vec2(width, ui.spacing().interact_size.y),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            egui::ComboBox::from_id_salt("service-section")
                .selected_text(state.section_label(cx.target))
                .width(width)
                .truncate()
                .show_ui(ui, |ui| {
                    for (key, label) in &sections {
                        if ui.selectable_label(state.section == *key, *label).clicked()
                            && state.section != *key
                        {
                            open(cx, PageKind::Service, key, None);
                            ui.close();
                        }
                    }
                })
                .response
                .labelled_by(label.id)
                .on_hover_text(state.section_label(cx.target));
        },
    );
    if state.editor.is_none()
        && ui
            .add_enabled(!cx.busy("detail"), egui::Button::new("Refresh"))
            .clicked()
    {
        refresh(state, cx);
    }
}
fn service(ui: &mut egui::Ui, state: &mut State, cx: &mut PageContext<'_>) {
    let kind = cx.target.service_type.clone();
    let id = (
        cx.target.project.clone(),
        cx.target.service.clone(),
        kind.clone(),
    );
    let dirty = state.is_dirty();
    let mut section_drawn = false;
    ui.horizontal(|ui| {
        if crate::lifecycle::ops(&kind, "deploy").is_some() {
            let mut button = egui::Button::new("Deploy");
            if !dirty {
                button = button.fill(ui.visuals().selection.bg_fill);
            }
            if ui
                .add_enabled(!cx.busy("mutation") && !dirty, button)
                .on_hover_text(if dirty {
                    "Save or discard changes before deploying. Your draft is not deployed."
                } else {
                    "Deploy saved configuration. Unsaved edits in other documents are not applied."
                })
                .clicked()
            {
                lifecycle(state, id.clone(), "Deploy", "deploy", false);
            }
        }
        if ui.button("Logs").clicked() {
            open(cx, PageKind::Terminal, "logs", None);
        }
        if ui.button("Terminal").clicked() {
            open(cx, PageKind::Terminal, "container", None);
        }
        ui.menu_button("More", |ui| {
            ui.menu_button("Lifecycle", |ui| {
                for (label, action, force) in [
                    ("Rebuild", "deploy", true),
                    ("Start", "start", false),
                    ("Stop", "stop", false),
                    ("Restart", "restart", false),
                ] {
                    if crate::lifecycle::ops(&kind, action).is_some()
                        && ui
                            .add_enabled(
                                !cx.busy("mutation") && !state.lifecycle_blocked(action),
                                egui::Button::new(label),
                            )
                            .clicked()
                    {
                        lifecycle(state, id.clone(), label, action, force);
                        ui.close();
                    }
                }
            });
            ui.menu_button("Transfers and comparison", |ui| {
                for (label, section) in [
                    ("Clone configuration", "clone"),
                    ("Migrate configuration", "migrate"),
                    ("Compare services", "diff"),
                    ("Database workspace", "database"),
                    ("Database credentials", "credentials"),
                    ("Copy databases", "copy"),
                    ("Dump and download", "dumps"),
                    ("Restore data", "restore"),
                    ("Restore from another host", "restore-from"),
                ] {
                    if data_available(section, &kind) && ui.button(label).clicked() {
                        open(cx, PageKind::Data, section, None);
                        ui.close();
                    }
                }
            });
            if crate::lifecycle::is_database(&kind) && ui.button("Database terminal").clicked() {
                open(cx, PageKind::Terminal, "db", None);
                ui.close();
            }
            if ui.button("Deployment and action history").clicked() {
                open(cx, PageKind::Host, "actions", None);
                ui.close();
            }
            if crate::lifecycle::ops(&kind, "destroy").is_some() {
                ui.separator();
                ui.menu_button("Destructive actions", |ui| {
                    if ui
                        .add_enabled(!cx.busy("mutation"), egui::Button::new("Destroy service"))
                        .clicked()
                    {
                        lifecycle(state, id.clone(), "Destroy", "destroy", false);
                        ui.close();
                    }
                });
            }
        });
        let runtime = runtime_label(state, &id.0, &id.1);
        ui.add_sized(
            [128.0, ui.spacing().interact_size.y],
            egui::Label::new(&runtime).truncate(),
        )
        .on_hover_text(state.runtime_updated.map_or_else(
            || format!("{runtime}\nRuntime status has not been refreshed yet"),
            |updated| {
                format!(
                    "{runtime}\nRuntime updated {} s ago",
                    updated.elapsed().as_secs()
                )
            },
        ));
        let font = egui::TextStyle::Button.resolve(ui.style());
        let label_width = ui
            .painter()
            .layout_no_wrap("Section".into(), font.clone(), ui.visuals().text_color())
            .size()
            .x;
        let refresh_width = if state.editor.is_none() {
            ui.painter()
                .layout_no_wrap("Refresh".into(), font, ui.visuals().text_color())
                .size()
                .x
                + 2.0 * ui.spacing().button_padding.x
                + ui.spacing().item_spacing.x
        } else {
            0.0
        };
        let fixed = label_width + refresh_width + 2.0 * ui.spacing().item_spacing.x;
        let available = ui.available_width() - fixed;
        if available >= 132.0 {
            service_section(
                ui,
                state,
                cx,
                if available >= 180.0 { 180.0 } else { 132.0 },
            );
            section_drawn = true;
        }
    });
    if !section_drawn {
        ui.horizontal(|ui| service_section(ui, state, cx, 132.0));
    }
    ui.separator();
    if state.editor.is_some() {
        return;
    }
    egui::ScrollArea::vertical()
        .id_salt("service-section-body")
        .max_height(ui.available_height().max(48.0))
        .show(ui, |ui| {
            let section = state.section.clone();
            match section.as_str() {
                "overview" => {
                    ui.horizontal_wrapped(|ui| {
                        ui.strong(runtime_label(state, &id.0, &id.1));
                        if let Some(metric) = state.metrics.iter().find(|m| {
                            text(m, "/projectName") == id.0 && text(m, "/serviceName") == id.1
                        }) {
                            if let Some(cpu) = metric.get("cpu").and_then(Value::as_f64) {
                                ui.label(format!("CPU {cpu:.1}%"));
                            }
                            if let Some(memory) = metric.get("memory").and_then(Value::as_f64) {
                                ui.label(format!("Memory {}", crate::output::format_bytes(memory)));
                            }
                            if let Some(rate) = metric.get("networkIn").and_then(Value::as_f64) {
                                ui.label(format!("Inbound {}", crate::output::format_rate(rate)));
                            }
                            if let Some(rate) = metric.get("networkOut").and_then(Value::as_f64) {
                                ui.label(format!("Outbound {}", crate::output::format_rate(rate)));
                            }
                        }
                    });
                    if let Some(v) = &state.detail {
                        egui::Grid::new("service-overview")
                            .num_columns(2)
                            .spacing([20.0, 8.0])
                            .show(ui, |ui| {
                                for (label, key) in [
                                    ("Project", "/projectName"),
                                    ("Service", "/name"),
                                    ("Type", "/type"),
                                    ("Enabled", "/enabled"),
                                    ("Created", "/createdAt"),
                                    ("Source type", "/source/type"),
                                    ("Image", "/image"),
                                ] {
                                    ui.label(label);
                                    ui.label(text(v, key));
                                    ui.end_row();
                                }
                            });
                    } else {
                        ui.label("Loading service configuration…");
                    }
                }
                "ports" | "mounts" => collection(ui, state, cx),
                "domains" => domains(ui, state, cx),
                "redirects" => redirects(ui, state, cx),
                "backups" => {
                    if ui.button("Manage backup schedules and history").clicked() {
                        open(cx, PageKind::Host, "backups", None);
                    }
                    for (label, section) in [
                        ("Dump and download", "dumps"),
                        ("Restore data", "restore"),
                        ("Copy databases", "copy"),
                        ("Restore from another host", "restore-from"),
                    ] {
                        if data_available(section, &kind) && ui.button(label).clicked() {
                            open(cx, PageKind::Data, section, None);
                        }
                    }
                }
                section => {
                    let intent = match section {
                        "environment" => Some(Intent::Environment),
                        "configuration" => Some(Intent::Configuration),
                        "source" => Some(Intent::Source),
                        "build" => Some(Intent::Build),
                        "deploy" => Some(Intent::Deploy),
                        "resources" => Some(Intent::Resources),
                        "auth" => Some(Intent::Auth),
                        _ => None,
                    };
                    if let Some(intent) = intent {
                        if state.editor.is_none()
                            && !cx.busy("detail")
                            && ui.button("Edit configuration").clicked()
                        {
                            state.pending_editor = Some(intent);
                            load_detail(state, cx);
                        }
                        if section == "environment" && kind == "app" {
                            let enabled = state
                                .detail
                                .as_ref()
                                .and_then(|v| v.get("dotEnvPath"))
                                .and_then(Value::as_str);
                            ui.label(match enabled {
                                Some(path) => format!("Environment file: {path}"),
                                None => "Environment file: off".into(),
                            });
                            if ui
                                .add_enabled(
                                    !cx.busy("mutation") && !state.is_dirty(),
                                    egui::Button::new(if enabled.is_some() {
                                        "Disable .env file"
                                    } else {
                                        "Enable .env file"
                                    }),
                                )
                                .clicked()
                            {
                                state.confirm(
                                    "Change environment file",
                                    format!("{}/{}", id.0, id.1),
                                    false,
                                    ConfirmAction::Request(Req::EnvFileToggle {
                                        project: id.0.clone(),
                                        service: id.1.clone(),
                                        stype: id.2.clone(),
                                    }),
                                );
                            }
                        }
                        if section == "source"
                            && state
                                .detail
                                .as_ref()
                                .is_some_and(|d| text(d, "/source/type") == "github")
                        {
                            let on = state
                                .detail
                                .as_ref()
                                .and_then(|d| d.pointer("/source/autoDeploy"))
                                .and_then(Value::as_bool)
                                .unwrap_or(false);
                            if ui
                                .add_enabled(
                                    !cx.busy("mutation"),
                                    egui::Button::new(if on {
                                        "Disable auto deploy"
                                    } else {
                                        "Enable auto deploy"
                                    }),
                                )
                                .clicked()
                            {
                                cx.request(
                                    "mutation",
                                    Req::AutoDeploy {
                                        project: id.0.clone(),
                                        service: id.1.clone(),
                                        on: !on,
                                    },
                                );
                            }
                        }
                    }
                }
            }
        });
}
fn collection(ui: &mut egui::Ui, state: &mut State, cx: &mut PageContext<'_>) {
    let mount = state.section == "mounts";
    let key = if mount { "mounts" } else { "ports" };
    if state.editor.is_none() && !state.loaded.contains(key) && !cx.busy(key) {
        state.invalidated.remove(key);
        cx.rpc(
            key,
            key,
            if mount { "listMounts" } else { "listPorts" },
            input(cx),
        );
    }
    let available = state.editor.is_none() && !cx.busy("mutation") && !cx.busy("mount-edit");
    if ui
        .add_enabled(
            available,
            egui::Button::new(if mount { "Add mount" } else { "Add port" }),
        )
        .clicked()
    {
        let intent = if mount {
            Intent::Mount(None)
        } else {
            Intent::Port
        };
        begin_edit(state, cx, intent);
    }
    let rows = if mount {
        state.mounts.clone()
    } else {
        state.ports.clone()
    };
    egui::ScrollArea::horizontal()
        .id_salt(("collection-columns", key))
        .show(ui, |ui| {
            egui::Grid::new(key).striped(true).show(ui, |ui| {
                for (index, row) in rows.iter().enumerate() {
                    ui.label(index.to_string());
                    if mount {
                        ui.label(text(row, "/type"));
                        ui.label(text(row, "/mountPath"));
                        ui.label(if text(row, "/type") == "file" {
                            "File contents (editable)".into()
                        } else {
                            format!("{}{}", text(row, "/name"), text(row, "/hostPath"))
                        });
                        if ui
                            .add_enabled(available, egui::Button::new("Edit"))
                            .clicked()
                        {
                            state.pending_mount = Some((index, row.clone()));
                            cx.request(
                                "mount-edit",
                                Req::MountForm {
                                    project: cx.target.project.clone(),
                                    service: cx.target.service.clone(),
                                    index,
                                },
                            );
                        }
                    } else {
                        ui.label(text(row, "/published"));
                        ui.label(text(row, "/target"));
                        ui.label(text(row, "/protocol"));
                    }
                    if ui
                        .add_enabled(available, egui::Button::new("Delete"))
                        .clicked()
                    {
                        let req = if mount {
                            Req::MountDelete {
                                project: cx.target.project.clone(),
                                service: cx.target.service.clone(),
                                index,
                            }
                        } else {
                            Req::PortDelete {
                                project: cx.target.project.clone(),
                                service: cx.target.service.clone(),
                                index,
                            }
                        };
                        state.confirm(
                            format!("Delete {key} entry"),
                            format!("{}/{} #{}", cx.target.project, cx.target.service, index),
                            false,
                            ConfirmAction::Indexed {
                                req,
                                collection: key,
                                index,
                                original: row.clone(),
                            },
                        );
                    }
                    ui.end_row();
                }
            });
        });
    if rows.is_empty() && !cx.busy(key) {
        ui.label(format!("No {key} configured."));
    }
}
fn domains(ui: &mut egui::Ui, state: &mut State, cx: &mut PageContext<'_>) {
    if state.editor.is_none() && !state.loaded.contains("domains") && !cx.busy("domains") {
        state.invalidated.remove("domains");
        cx.request("domains", Req::Domains);
    }
    if ui
        .add_enabled(state.editor.is_none(), egui::Button::new("Add domain"))
        .clicked()
    {
        begin_edit(state, cx, Intent::Domain(None));
    }
    let rows: Vec<_> = state
        .domains
        .iter()
        .filter(|d| {
            text(d, "/serviceDestination/projectName") == cx.target.project
                && text(d, "/serviceDestination/serviceName") == cx.target.service
        })
        .cloned()
        .collect();
    for d in rows {
        ui.horizontal_wrapped(|ui| {
            ui.strong(text(&d, "/host"));
            ui.label(text(&d, "/path"));
            ui.label(format!(
                "{}:{}",
                text(&d, "/serviceDestination/protocol"),
                text(&d, "/serviceDestination/port")
            ));
            let id = text(&d, "/id");
            if ui
                .add_enabled(state.editor.is_none(), egui::Button::new("Edit"))
                .clicked()
            {
                let f = Form::new(
                    FormKind::DomainEdit { id: id.clone() },
                    "Edit service domain",
                    form::domain_fields(Some(&d), &state.projects),
                )
                .with_original(d.clone());
                state.edit(f, Intent::Domain(Some(id.clone())));
            }
            if ui.button("Set primary").clicked() {
                cx.request("mutation", Req::DomainSetPrimary(id.clone()));
            }
            if ui.button("Delete").clicked() {
                state.confirm(
                    "Delete domain",
                    text(&d, "/host"),
                    true,
                    ConfirmAction::Request(Req::DomainDelete(id)),
                );
            }
        });
    }
}
fn redirects(ui: &mut egui::Ui, state: &mut State, cx: &mut PageContext<'_>) {
    let available = state.editor.is_none() && !cx.busy("mutation");
    if ui
        .add_enabled(available, egui::Button::new("Add redirect"))
        .clicked()
    {
        begin_edit(state, cx, Intent::Redirect);
    }
    let rows = state
        .detail
        .as_ref()
        .and_then(|v| v.get("redirects"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for (index, row) in rows.iter().enumerate() {
        ui.horizontal_wrapped(|ui| {
            ui.label(text(row, "/regex"));
            ui.label(text(row, "/replacement"));
            ui.label(
                if row
                    .get("permanent")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    "301"
                } else {
                    "302"
                },
            );
            ui.label(
                if row.get("enabled").and_then(Value::as_bool).unwrap_or(false) {
                    "Enabled"
                } else {
                    "Disabled"
                },
            );
            if ui
                .add_enabled(available, egui::Button::new("Delete"))
                .clicked()
            {
                let req = Req::RedirectDelete {
                    project: cx.target.project.clone(),
                    service: cx.target.service.clone(),
                    stype: cx.target.service_type.clone(),
                    index,
                };
                state.confirm(
                    "Delete redirect",
                    text(row, "/regex"),
                    false,
                    ConfirmAction::Indexed {
                        req,
                        collection: "redirects",
                        index,
                        original: row.clone(),
                    },
                );
            }
        });
    }
}
fn begin_edit(state: &mut State, cx: &PageContext<'_>, intent: Intent) {
    if state.editor.is_some() {
        return;
    }
    let d = state.detail.as_ref().cloned().unwrap_or(Value::Null);
    let p = cx.target.project.clone();
    let s = cx.target.service.clone();
    let k = cx.target.service_type.clone();
    let f = match &intent {
        Intent::Environment => {
            let resource = if s.is_empty() { &d["project"] } else { &d };
            let content = match crate::operations::env_content(resource) {
                Ok(content) => content,
                Err(error) => {
                    state.error = Some(error.to_string());
                    return;
                }
            };
            Form::new(FormKind::ProjectCreate,"Environment",vec![Field::editor("Environment",content)]).with_note("Values may contain secrets. Save replaces the complete environment; deploy or restart to apply.")
        }
        Intent::Configuration => Form::new(
            FormKind::ProjectCreate,
            "Database / advanced configuration",
            vec![Field::editor(
                "Config file",
                d.get("configFile").and_then(Value::as_str).unwrap_or(""),
            )],
        )
        .with_note("Restart after saving to apply. Image, command and environment are preserved."),
        Intent::Source => Form::new(
            FormKind::SourceEdit {
                project: p,
                service: s,
            },
            "Source",
            form::source_fields(d.get("source"), Vec::new()),
        ),
        Intent::Build => Form::new(
            FormKind::BuildEdit {
                project: p,
                service: s,
            },
            "Build",
            form::build_fields(d.get("build")),
        )
        .with_original(d.get("build").cloned().unwrap_or(Value::Null)),
        Intent::Resources => Form::new(
            FormKind::ResourceEdit {
                project: p,
                service: s,
                stype: k,
            },
            "Resources",
            form::resource_fields(d.get("resources")),
        )
        .with_note("CPU in cores, memory in MB. 0 = unlimited."),
        Intent::Deploy => Form::new(
            FormKind::DeployEdit {
                project: p,
                service: s,
            },
            "Deployment",
            form::deploy_fields(d.get("deploy")),
        )
        .with_original(d.get("deploy").cloned().unwrap_or_else(|| json!({}))),
        Intent::Port => Form::new(
            FormKind::PortCreate {
                project: p,
                service: s,
            },
            "Add port",
            form::port_fields(),
        ),
        Intent::Mount(None) => Form::new(
            FormKind::MountCreate {
                project: p,
                service: s,
            },
            "Add mount",
            form::mount_fields(None),
        ),
        Intent::Redirect => Form::new(
            FormKind::RedirectCreate {
                project: p,
                service: s,
                stype: k,
            },
            "Add redirect",
            form::redirect_fields(),
        ),
        Intent::Auth => Form::new(
            FormKind::BasicAuthEdit {
                project: p,
                service: s,
                stype: k,
            },
            "Basic authentication",
            form::basic_auth_fields(Some(&d)),
        )
        .with_original(d.get("basicAuth").cloned().unwrap_or_else(|| json!([])))
        .with_note(
            "Clear both fields to remove the first credential. Other credentials are preserved.",
        ),
        Intent::Domain(None) => {
            let mut f = Form::new(
                FormKind::DomainCreate,
                "Add service domain",
                form::domain_fields(None, &state.projects),
            );
            for (label, value) in [("Project", p), ("Service", s)] {
                if let Some(field) = f.fields.iter_mut().find(|f| f.label == label) {
                    field.value = value;
                }
            }
            f
        }
        _ => return,
    };
    state.edit(f, intent);
}

fn editor(ui: &mut egui::Ui, state: &mut State, cx: &mut PageContext<'_>) {
    let missing = state.editor_invalidated
        || state
            .editor
            .as_ref()
            .is_some_and(|edit| match &edit.intent {
                Intent::BulkResources(targets) => {
                    targets.iter().any(|target| state.target_missing(target))
                }
                _ => false,
            });
    let Some(edit) = state.editor.as_mut() else {
        return;
    };
    let single_editor = matches!(edit.intent, Intent::Environment | Intent::Configuration);
    if !single_editor {
        ui.add(egui::Label::new(egui::RichText::new(&edit.form.title).strong()).truncate())
            .on_hover_text(&edit.form.title);
    }
    if let Intent::BulkResources(targets) = &edit.intent {
        let names = targets
            .iter()
            .map(|(p, s, _)| format!("{p}/{s}"))
            .collect::<Vec<_>>()
            .join(", ");
        ui.add(egui::Label::new(format!("Apply to {} services", targets.len())).truncate())
            .on_hover_text(names);
    }
    if let Intent::ServiceSetup {
        project,
        service,
        stype,
        ..
    } = &edit.intent
    {
        if ui
            .button("Open created service (keep this setup editor)")
            .clicked()
        {
            open(
                cx,
                PageKind::Service,
                "overview",
                Some((project.clone(), service.clone(), stype.clone())),
            );
        }
    }
    let busy = cx.busy("mutation") || cx.busy("import");
    let mut save = false;
    let mut discard = false;
    let mut test = false;
    let mut import = false;
    let action_label = match edit.intent {
        Intent::Connection(None) => "Add connection",
        Intent::ProjectCreate => "Create project",
        Intent::ServiceCreate => "Create service",
        Intent::Port => "Add port",
        Intent::Mount(None) => "Add mount",
        Intent::Redirect => "Add redirect",
        Intent::Domain(None) => "Add domain",
        Intent::ServiceSetup {
            phase: ServiceSetupPhase::Source,
            ..
        } => "Retry source setup",
        Intent::ServiceSetup {
            phase: ServiceSetupPhase::AutoDeploy,
            ..
        } => "Retry auto deploy",
        _ => "Save changes",
    };
    ui.add_enabled_ui(!busy,|ui|{
        ui.horizontal_wrapped(|ui|{
            if matches!(edit.intent,Intent::ServiceCreate)&&edit.form.is_wizard(){
                if ui.button("Back").clicked(){if let Some(step)=edit.form.prev_present_step(){edit.form.step=step;}}
                if edit.form.next_present_step().is_some(){
                    if ui.button("Next").clicked(){match form::validate_step(&edit.form){Ok(())=>{edit.form.step=edit.form.next_present_step().unwrap_or(edit.form.step);edit.form.error=None;},Err(e)=>edit.form.error=Some(e)}}
                }else if ui.button("Create service").clicked(){save=true;}
            }else {
                let mut button = egui::Button::new(action_label);
                if edit.dirty() {
                    button = button.fill(ui.visuals().selection.bg_fill);
                }
                if ui.add_enabled(edit.dirty()||matches!(edit.intent,Intent::Connection(None)|Intent::ProjectCreate|Intent::ServiceCreate|Intent::Port|Intent::Mount(None)|Intent::Redirect|Intent::Domain(None)),button).clicked(){save=true;}
            }
            if ui.button("Discard").clicked(){if edit.dirty(){edit.discard=true;}else{discard=true;}}
            if edit.form.visible_here().iter().any(|&i|matches!(edit.form.fields[i].kind,FieldKind::Editor))&&ui.add_enabled(!cx.busy("import") && !cx.busy("native-picker") && state.pending_picker.is_none(),egui::Button::new("Import text file…")).clicked(){import=true;}
            if matches!(edit.intent,Intent::Connection(_))&&ui.add_enabled(!cx.busy("connection-test"),egui::Button::new("Test connection")).clicked(){test=true;}
            if edit.dirty() {
                let label = if !cx.target.service.is_empty() {
                    if crate::lifecycle::ops(&cx.target.service_type, "deploy").is_some() {
                        "Unsaved changes · save before deploying"
                    } else {
                        "Unsaved changes · save before restarting"
                    }
                } else {
                    "Unsaved changes"
                };
                ui.label(egui::RichText::new(label).color(ui.visuals().warn_fg_color));
            }
            if let Some(error) = &edit.form.error {
                ui.menu_button(egui::RichText::new("Validation error").color(ui.visuals().error_fg_color), |ui| {
                    ui.set_max_width(400.0);
                    widgets::error(ui, error);
                });
            }
        });
        if edit.discard {
            ui.horizontal(|ui| {
                ui.label("Discard unsaved changes?");
                if ui.button("Discard changes").clicked() {
                    discard = true;
                }
                if ui.button("Keep editing").clicked() {
                    edit.discard = false;
                }
            });
        }
        let height = ui.available_height().max(24.0);
        if matches!(edit.intent, Intent::Environment | Intent::Configuration) {
            if widgets::form_fields_sized(ui, &mut edit.form, height) {
                edit.revision += 1;
            }
        } else {
        egui::ScrollArea::vertical()
            .id_salt("service-editor")
            .max_height(height)
            .min_scrolled_height(24.0)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if widgets::form_fields_sized(ui, &mut edit.form, (height - 64.0).max(24.0)) {
                    edit.revision += 1;
                }
                if matches!(edit.intent, Intent::ServiceCreate) && edit.form.next_present_step().is_none() {
                    ui.label(format!("Create {}/{} ({})", edit.form.by_label("Project"), edit.form.by_label("Name"), edit.form.by_label("Kind")));
                    ui.small("Source and configuration are saved separately. Deployment remains an explicit action.");
                }
            });
        }
    });
    if import && state.pending_picker.is_none() && !cx.busy("native-picker") && !cx.busy("import") {
        if let Some(label) = edit.form.visible_here().into_iter().find_map(|i| {
            matches!(edit.form.fields[i].kind, FieldKind::Editor)
                .then_some(edit.form.fields[i].label)
        }) {
            let before = edit
                .form
                .fields
                .iter()
                .find(|f| f.label == label)
                .map(|f| f.value.clone())
                .unwrap_or_default();
            state.pending_picker = Some(PickerIntent::Import((
                edit.generation,
                label,
                edit.revision,
                before,
            )));
            cx.background("native-picker", "Choose text file to import", move || {
                Ok(Payload::ChosenPath(
                    rfd::FileDialog::new()
                        .set_title("Import text into editor")
                        .pick_file(),
                ))
            });
        }
    }
    if test {
        match connection_draft(edit) {
            Ok(server) => {
                edit.form.error = None;
                let original = match &edit.intent {
                    Intent::Connection(old) => old.as_ref().map(|s| s.name.clone()),
                    _ => None,
                };
                let path = cx.server_config_path().to_owned();
                cx.background(
                    "connection-test",
                    "Test connection and SSH transport",
                    move || {
                        let server = resolve_test_credentials(
                            server,
                            original.as_deref(),
                            &ServerConfig::new(path),
                        )?;
                        Ok(Payload::Text(connections::test_server(&server)?))
                    },
                );
            }
            Err(e) => edit.form.error = Some(e),
        }
    }
    if save && missing {
        edit.form.error = Some("Inventory changed: a selected target is missing. Reopen the resource editor from the current selection; this draft has not been applied.".into());
    } else if save {
        if let Intent::Connection(original) = &edit.intent {
            match connection_draft(edit) {
                Ok(server) => {
                    edit.form.error = None;
                    let original = original.as_ref().map(|s| s.name.clone());
                    let path = cx.server_config_path().to_owned();
                    cx.background("mutation","Save connection",move||{
                        let cfg=ServerConfig::new(path);
                        connections::save_server(&cfg,original.as_deref(),server)?;
                        Ok(Payload::Text("Connection saved. Existing documents retain their original connection.".into()))
                    });
                }
                Err(e) => edit.form.error = Some(e),
            }
        } else {
            match submission(edit, cx) {
                Ok(req) => {
                    if let Intent::Mount(Some(index)) = edit.intent {
                        if let Some(original) = edit.form.original.clone() {
                            guarded_request(cx, req, "mounts", index, original);
                        } else {
                            edit.form.error =
                                Some("Missing original mount; reopen the editor.".into());
                        }
                    } else {
                        cx.request("mutation", req);
                    }
                }
                Err(e) => edit.form.error = Some(e),
            }
        }
        if cx.busy("mutation") {
            state.saving_generation = Some(edit.generation);
        }
    }
    let request_repos = matches!(edit.intent, Intent::Source);
    let repo = edit.form.by_label("Repo");
    if request_repos && state.loaded.insert(format!("repos:{}", edit.generation)) {
        cx.request("repos", Req::Repos);
    }
    if !repo.is_empty() && repo != state.branches_for && !cx.busy("branches") {
        if let Some((owner, name)) = repo.split_once('/') {
            cx.request(
                "branches",
                Req::Branches {
                    owner: owner.into(),
                    repo: name.into(),
                },
            );
            state.branches_for = repo;
        }
    }
    let project = edit.form.by_label("Project");
    if matches!(edit.intent, Intent::Domain(_))
        && !project.is_empty()
        && project != state.domain_project_for
        && !cx.busy("domain-services")
    {
        if !state.domain_project_for.is_empty() {
            if let Some(field) = edit.form.fields.iter_mut().find(|f| f.label == "Service") {
                field.value.clear();
                field.kind = FieldKind::Choice(Vec::new());
            }
        }
        cx.request("domain-services", Req::ServicesFor(project.clone()));
        state.domain_project_for = project;
    }
    if discard {
        state.editor = None;
        state.pending_editor = None;
    }
}
fn setup_request(edit: &Editor) -> Result<Req, String> {
    let Intent::ServiceSetup {
        project,
        service,
        stype,
        phase,
    } = &edit.intent
    else {
        return Err("No pending service setup".into());
    };
    Ok(Req::ServiceSetup {
        project: project.clone(),
        service: service.clone(),
        stype: stype.clone(),
        phase: *phase,
        source: form::source_body(&edit.form)?,
    })
}
fn submission(edit: &Editor, cx: &PageContext<'_>) -> Result<Req, String> {
    let f = &edit.form;
    let project = cx.target.project.clone();
    let service = cx.target.service.clone();
    let stype = cx.target.service_type.clone();
    Ok(match &edit.intent {
        Intent::ProjectCreate => {
            let name = f.by_label("Name");
            if !crate::operations::valid_name(&name) {
                return Err("Project names may only contain a-z, 0-9, - and _".into());
            }
            Req::ProjectCreate(name)
        }
        Intent::ServiceCreate => {
            if f.by_label("Kind") == "app" && !f.by_label("Domain host").is_empty() {
                let port = f.by_label("Domain port");
                if !port.is_empty() {
                    validate_port(&port)?;
                }
            }
            form::service_create_request(f)?
        }
        Intent::ServiceSetup { .. } => setup_request(edit)?,
        Intent::Environment => {
            let env = f.by_label("Environment");
            if service.is_empty() {
                Req::ProjectEnvSave { project, env }
            } else {
                Req::EnvSave {
                    project,
                    service,
                    stype,
                    env,
                }
            }
        }
        Intent::Configuration => Req::ConfigFileSave {
            project,
            service,
            stype,
            config: f.by_label("Config file"),
        },
        Intent::Source => {
            let (op, body, auto_deploy) = form::source_body(f)?;
            Req::ConfigSave {
                project,
                service,
                op,
                body,
                auto_deploy,
            }
        }
        Intent::Build => Req::ConfigSave {
            project,
            service,
            op: "updateBuild",
            body: form::build_body(f)?,
            auto_deploy: None,
        },
        Intent::Resources => Req::ResourceSave {
            project,
            service,
            stype,
            resources: checked_resources(f)?,
        },
        Intent::BulkResources(targets) => {
            if targets.is_empty() {
                return Err("No services selected".into());
            }
            Req::BulkResource {
                targets: targets.clone(),
                resources: checked_resources(f)?,
            }
        }
        Intent::Deploy => {
            let mut deploy = f
                .original
                .clone()
                .filter(Value::is_object)
                .unwrap_or_else(|| json!({}));
            let changes = form::deploy_body(f)?;
            for (key, value) in changes.as_object().ok_or("Invalid deployment settings")? {
                deploy[key] = value.clone();
            }
            Req::DeploySave {
                project,
                service,
                deploy,
            }
        }
        Intent::Port => {
            validate_port(&f.by_label("Published"))?;
            validate_port(&f.by_label("Target"))?;
            Req::PortSave {
                project,
                service,
                values: form::port_body(f)?,
            }
        }
        Intent::Mount(index) => {
            let values = mount_values(f)?;
            match index {
                Some(index) => Req::MountUpdate {
                    project,
                    service,
                    index: *index,
                    values,
                },
                None => Req::MountSave {
                    project,
                    service,
                    values,
                },
            }
        }
        Intent::Redirect => Req::RedirectAdd {
            project,
            service,
            stype,
            redirect: form::redirect_body(f)?,
        },
        Intent::Auth => {
            let first = form::basic_auth_body(f)?;
            let mut auth = f
                .original
                .as_ref()
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if !auth.is_empty() {
                auth.remove(0);
            }
            if let Some(first) = first.as_array().and_then(|a| a.first()) {
                auth.insert(0, first.clone());
            }
            Req::BasicAuthSave {
                project,
                service,
                stype,
                basic_auth: json!(auth),
            }
        }
        Intent::Domain(id) => {
            if f.by_label("Destination") == "service" {
                validate_port(&f.by_label("Port"))?;
            }
            Req::DomainSave {
                id: id.clone(),
                body: form::domain_body(f)?,
            }
        }
        Intent::Connection(_) => return Err("Use the local connection store".into()),
    })
}
fn validate_port(raw: &str) -> Result<u16, String> {
    raw.parse::<u16>()
        .ok()
        .filter(|n| *n > 0)
        .ok_or_else(|| "Port must be a whole number from 1 to 65535".into())
}
fn checked_resources(f: &Form) -> Result<Value, String> {
    for label in [
        "CPU limit (core)",
        "CPU reservation (core)",
        "Memory limit (MB)",
        "Memory reservation (MB)",
    ] {
        let value = f.by_label(label);
        if !value.trim().is_empty()
            && !value
                .trim()
                .parse::<f64>()
                .is_ok_and(|n| n.is_finite() && n >= 0.0)
        {
            return Err(format!("{label} must be a finite, non-negative number"));
        }
    }
    form::resource_body(f)
}
fn mount_values(f: &Form) -> Result<Value, String> {
    let mut values = f
        .original
        .clone()
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    let changes = form::mount_body(f)?;
    if text(&values, "/type") != text(&changes, "/type") {
        values = json!({});
    }
    for (key, value) in changes.as_object().ok_or("Invalid mount settings")? {
        values[key] = value.clone();
    }
    Ok(values)
}
fn confirmation(ui: &mut egui::Ui, state: &mut State, cx: &mut PageContext<'_>) {
    let Some(confirm) = &state.confirmation else {
        return;
    };
    ui.add(
        egui::Label::new(
            egui::RichText::new(&confirm.title)
                .strong()
                .color(ui.visuals().warn_fg_color),
        )
        .truncate(),
    )
    .on_hover_text(&confirm.title);
    let missing = state.confirmation_invalidated
        || match &confirm.action {
            ConfirmAction::Request(Req::Bulk { targets, .. }) => {
                targets.iter().any(|target| state.target_missing(target))
            }
            ConfirmAction::Request(Req::Action {
                project,
                service,
                stype,
                ..
            }) => state.target_missing(&(project.clone(), service.clone(), stype.clone())),
            _ => false,
        };
    if missing {
        ui.colored_label(ui.visuals().warn_fg_color,
            "Inventory changed: a reviewed target is missing. Cancel and review the current selection again.");
    }
    let lifecycle_action = match &confirm.action {
        ConfirmAction::Request(Req::Action { action, .. } | Req::Bulk { action, .. }) => {
            Some(action.as_str())
        }
        _ => None,
    };
    let blocked = lifecycle_action.is_some_and(|action| state.lifecycle_blocked(action));
    if lifecycle_action.is_some_and(|action| matches!(action, "deploy" | "start" | "restart")) {
        ui.label("Only saved configuration is applied. Unsaved edits in this or other documents are not applied.");
    }
    if blocked {
        ui.colored_label(
            ui.visuals().warn_fg_color,
            "Save or discard this editor's changes before continuing. Cancel keeps your draft.",
        );
    }
    if lifecycle_action == Some("destroy") && state.is_dirty() {
        ui.colored_label(
            ui.visuals().warn_fg_color,
            "Your unsaved draft is not saved. Destroying this target may make the draft unusable.",
        );
    }
    let typed = confirm.typed.then_some(
        if matches!(confirm.action, ConfirmAction::Request(Req::Bulk { .. })) {
            "DESTROY"
        } else {
            confirm.target.as_str()
        },
    );
    let mut apply = false;
    let mut cancel = false;
    let indexed = matches!(confirm.action, ConfirmAction::Indexed { .. });
    ui.add_enabled_ui(
        !cx.busy("mutation") && (!indexed || state.editor.is_none()),
        |ui| {
            if let Some(expected) = typed {
                let prompt = format!("Type {expected} to confirm this target:");
                let label = ui
                    .add(egui::Label::new(&prompt).truncate())
                    .on_hover_text(&prompt);
                ui.add(
                    egui::TextEdit::singleline(&mut state.confirmation_entry)
                        .desired_width(f32::INFINITY),
                )
                .labelled_by(label.id);
            }
            ui.horizontal_wrapped(|ui| {
                apply = ui
                    .add_enabled(
                        !blocked
                            && !missing
                            && typed.is_none_or(|expected| state.confirmation_entry == expected),
                        egui::Button::new("Confirm")
                            .fill(ui.visuals().error_fg_color.gamma_multiply(0.20)),
                    )
                    .clicked();
                cancel = ui.button("Cancel").clicked();
            });
        },
    );
    ui.separator();
    egui::ScrollArea::vertical()
        .id_salt("service-review-details")
        .max_height(ui.available_height().max(24.0))
        .min_scrolled_height(24.0)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add(egui::Label::new(&confirm.target).wrap());
        });
    if apply {
        if let Some(confirm) = state.confirmation.take() {
            match confirm.action {
                ConfirmAction::Request(req) => {
                    state.confirmation_invalidated = false;
                    cx.request("mutation", req);
                }
                ConfirmAction::Indexed {
                    req,
                    collection,
                    index,
                    original,
                } => guarded_request(cx, req, collection, index, original),
                ConfirmAction::Export {
                    path,
                    server,
                    project,
                } => write_export(cx, path, server, project),
                ConfirmAction::RemoveConnection(name) => {
                    let path = cx.server_config_path().to_owned();
                    cx.background("mutation", "Remove local connection", move || {
                        connections::remove_server(&ServerConfig::new(path), &name)?;
                        Ok(Payload::Text(format!(
                            "Removed connection {name}. The remote server was not changed."
                        )))
                    });
                }
            }
        }
    }
    if cancel {
        state.confirmation = None;
    }
}
fn export_project(state: &mut State, cx: &mut PageContext<'_>) {
    if cx.busy("export") || cx.busy("native-picker") || state.pending_picker.is_some() {
        return;
    }
    let Some(server) = cx.target.server.clone() else {
        return;
    };
    let project = cx.target.project.clone();
    let filename = format!("{project}.easypanel.json");
    state.pending_picker = Some(PickerIntent::Export { server, project });
    cx.background(
        "native-picker",
        "Choose project export destination",
        move || {
            Ok(Payload::ChosenPath(
                rfd::FileDialog::new()
                    .set_title("Export redacted project configuration")
                    .set_file_name(filename)
                    .add_filter("JSON configuration", &["json"])
                    .save_file(),
            ))
        },
    );
}
fn write_export(
    cx: &mut PageContext<'_>,
    path: std::path::PathBuf,
    server: Server,
    project: String,
) {
    cx.background_transfer("export", "Export redacted project", move || {
        let data = crate::client::EasypanelClient::for_server(&server).call(
            "projects",
            "inspectProject",
            json!({"projectName":project}),
        )?;
        let services = data
            .get("services")
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow::anyhow!("Project response has no services array"))?;
        let mut bytes =
            serde_json::to_vec_pretty(&crate::services::export_project(&project, services))?;
        bytes.push(b'\n');
        atomic_export(&path, &bytes)?;
        Ok(Payload::Text(format!(
            "Exported {} service(s) to {}. Config only; secrets redacted.",
            services.len(),
            path.display()
        )))
    });
}
fn atomic_export(path: &std::path::Path, bytes: &[u8]) -> anyhow::Result<()> {
    use std::io::Write;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."));
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.write_all(bytes)?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

pub(super) fn show_connections(ui: &mut egui::Ui, state: &mut State, cx: &mut PageContext<'_>) {
    pump(state, cx);
    if state.confirmation.is_some() {
        feedback(ui, state, cx);
        confirmation(ui, state, cx);
        return;
    }
    ui.horizontal(|ui| {
        ui.heading("Connections");
        if ui
            .add_enabled(state.editor.is_none(), egui::Button::new("Add connection"))
            .clicked()
        {
            state.edit(connection_form(None), Intent::Connection(None));
        }
    });
    if state.editor.is_none() {
        ui.label("Connections are stored locally. Selecting a server does not change the default or retarget open documents.");
        let rows: Vec<_> = cx
            .servers
            .iter()
            .map(|s| {
                vec![
                    s.name.clone(),
                    s.url.clone(),
                    if s.ssh.is_some() {
                        "SSH tunnel".into()
                    } else {
                        "Direct".into()
                    },
                    if s.default {
                        "Default".into()
                    } else {
                        String::new()
                    },
                ]
            })
            .collect();
        let mut selected = state
            .selected_server
            .as_ref()
            .and_then(|name| cx.servers.iter().position(|s| &s.name == name));
        if selected.is_none() {
            state.selected_server = None;
        }
        if let Some(index) = widgets::table(
            ui,
            "connections",
            &["Name", "Panel URL", "Transport", "Default"],
            &rows,
            &mut selected,
            100.0,
        ) {
            state.selected_server = cx.servers.get(index).map(|s| s.name.clone());
        }
        if let Some(server) = state
            .selected_server
            .as_ref()
            .and_then(|name| cx.servers.iter().find(|s| &s.name == name))
            .cloned()
        {
            ui.horizontal_wrapped(|ui| {
                if ui.button("Open server").clicked() {
                    cx.open(OpenPage {
                        kind: PageKind::Host,
                        target: Target {
                            server: Some(server.clone()),
                            section: "overview".into(),
                            ..Target::default()
                        },
                        title: server.name.clone(),
                    });
                }
                if ui
                    .add_enabled(state.editor.is_none(), egui::Button::new("Edit / rename"))
                    .clicked()
                {
                    state.edit(
                        connection_form(Some(&server)),
                        Intent::Connection(Some(server.clone())),
                    );
                }
                if ui
                    .add_enabled(!cx.busy("connection-test"), egui::Button::new("Test"))
                    .clicked()
                {
                    let server = server.clone();
                    cx.background("connection-test", "Test saved connection", move || {
                        Ok(Payload::Text(connections::test_server(&server)?))
                    });
                }
                if ui
                    .add_enabled(
                        !server.default && !cx.busy("mutation"),
                        egui::Button::new("Make default"),
                    )
                    .clicked()
                {
                    let name = server.name.clone();
                    let path = cx.server_config_path().to_owned();
                    cx.background("mutation", "Set default connection", move || {
                        connections::default_server(&ServerConfig::new(path), &name)?;
                        Ok(Payload::Text(format!(
                            "{name} is now the default connection."
                        )))
                    });
                }
                if ui.button("Remove").clicked() {
                    state.confirm(
                        "Remove local connection",
                        server.name.clone(),
                        true,
                        ConfirmAction::RemoveConnection(server.name.clone()),
                    );
                }
            });
        }
        if cx.servers.is_empty() {
            ui.label(
                "Add your first EasyPanel connection. API tokens and SSH credentials stay masked.",
            );
        }
    }
    feedback(ui, state, cx);
    editor(ui, state, cx);
    if state.confirmation.is_some() {
        ui.ctx().request_repaint();
    }
}
fn connection_form(server: Option<&Server>) -> Form {
    let entry = server.cloned().map(form::ServerEntry::from);
    form::server_form(entry.as_ref()).with_note("Blank token/password on edit keeps the stored credential. SSH URL is the panel address as seen from that host. Test verifies both transport and API access.")
}
fn connection_draft(edit: &Editor) -> Result<Server, String> {
    let Intent::Connection(original) = &edit.intent else {
        return Err("Not a connection editor".into());
    };
    let form::ServerAction::Save {
        name,
        url,
        token,
        ssh,
        ..
    } = form::server_form_action(&edit.form)?
    else {
        return Err("The connection form did not produce a save action".into());
    };
    Ok(Server {
        name,
        url,
        token: token.unwrap_or_default(),
        ssh,
        default: original.as_ref().is_some_and(|s| s.default),
    })
}
fn resolve_test_credentials(
    mut draft: Server,
    original: Option<&str>,
    cfg: &ServerConfig,
) -> anyhow::Result<Server> {
    if let Some(name) = original {
        let old = cfg
            .try_all()?
            .into_iter()
            .find(|s| s.name == name)
            .ok_or_else(|| anyhow::anyhow!("Connection {name} was removed. Reopen the editor."))?;
        if draft.token.is_empty() {
            draft.token = old.token;
        }
        if let (Some(new), Some(old)) = (&mut draft.ssh, old.ssh) {
            if new.auth == old.auth && new.secret.is_none() {
                new.secret = old.secret;
            }
        }
    }
    Ok(draft)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn make_editor(form: Form, intent: Intent) -> Editor {
        let baseline = form.fields.iter().map(|f| f.value.clone()).collect();
        Editor {
            form,
            intent,
            baseline,
            generation: 1,
            revision: 0,
            discard: false,
        }
    }
    fn set(form: &mut Form, label: &str, value: &str) {
        form.fields
            .iter_mut()
            .find(|f| f.label == label)
            .unwrap()
            .value = value.into();
    }
    struct LifecycleHarness {
        ctx: egui::Context,
        backend: super::super::backend::Backend,
        target: Target,
        state: State,
        opened: Vec<OpenPage>,
        reload: bool,
    }
    impl LifecycleHarness {
        fn new(server: &httpmock::MockServer, service: &str) -> Self {
            server.mock(|when, then| {
                when.path("/api/rpc/projects/listProjectsAndServices");
                then.status(200)
                    .json_body(json!({"json":{"projects":[{"name":"project"}],
                        "services":[{"projectName":"project","name":"web","type":"app"}]}}));
            });
            server.mock(|when, then| {
                when.path("/api/rpc/monitorOld/getDockerTaskStats");
                then.status(200).json_body(json!({"json":[]}));
            });
            let ctx = egui::Context::default();
            Self {
                backend: super::super::backend::Backend::new(
                    ctx.clone(),
                    super::super::Runtime::default(),
                ),
                ctx,
                target: Target {
                    server: Some(Server {
                        name: "fixture".into(),
                        url: server.base_url(),
                        token: "fixture-token".into(),
                        default: false,
                        ssh: None,
                    }),
                    project: "project".into(),
                    service: service.into(),
                    service_type: if service.is_empty() { "" } else { "app" }.into(),
                    ..Default::default()
                },
                state: State {
                    initialized: true,
                    last_poll: Some(Instant::now()),
                    detail: Some(json!({"name":"original"})),
                    ..Default::default()
                },
                opened: Vec::new(),
                reload: false,
            }
        }
        fn context(&mut self, f: impl FnOnce(&mut State, &mut PageContext<'_>)) {
            let path = std::path::Path::new("unused-lifecycle-fixture");
            let mut cx = PageContext {
                target: &self.target,
                servers: &[],
                accounts: &[],
                server_config: path,
                cloudflare_config: path,
                watchlist: path,
                doc: 1,
                backend: &mut self.backend,
                opened: &mut self.opened,
                reload: &mut self.reload,
            };
            f(&mut self.state, &mut cx);
        }
        fn frame(&mut self, events: Vec<egui::Event>) -> egui::FullOutput {
            let ctx = self.ctx.clone();
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1200.0, 800.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    egui::CentralPanel::default().show(ui, |ui| {
                        self.context(|state, cx| show(ui, state, cx));
                    });
                },
            );
            output.textures_delta.clear();
            output
        }
        fn click(&mut self, label: &str) {
            let output = self.frame(Vec::new());
            let pos = Self::text(&output, label)
                .expect("action must render")
                .center();
            for pressed in [true, false] {
                self.frame(vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ]);
            }
        }
        fn text(output: &egui::FullOutput, label: &str) -> Option<egui::Rect> {
            output.shapes.iter().find_map(|shape| match &shape.shape {
                egui::epaint::Shape::Text(text) if text.galley.text() == label => {
                    Some(text.galley.rect.translate(text.pos.to_vec2()))
                }
                _ => None,
            })
        }
        fn settle(&mut self, key: &str) {
            let deadline = Instant::now() + Duration::from_secs(5);
            while self.backend.busy(1, key) && Instant::now() < deadline {
                self.backend.pump();
                std::thread::sleep(Duration::from_millis(2));
            }
            assert!(!self.backend.busy(1, key), "{key} did not finish");
            self.context(pump);
        }
        fn destroy(&mut self) {
            self.context(|state, cx| {
                if cx.target.service.is_empty() {
                    state.confirm(
                        "Delete project",
                        "project",
                        true,
                        ConfirmAction::Request(Req::ProjectDestroy("project".into())),
                    );
                    state.confirmation_entry = "project".into();
                } else {
                    quick_action(state, cx, QuickAction::Destroy);
                    state.confirmation_entry = "project/web".into();
                }
            });
            self.click("Confirm");
            self.settle("mutation");
        }
    }
    #[test]
    fn deletion_state_success_stops_inspection_and_actions_for_service_and_project() {
        use httpmock::{Method::POST, MockServer};
        for service in ["web", ""] {
            let server = MockServer::start();
            let delete = server.mock(|when, then| {
                when.method(POST).path(if service.is_empty() {
                    "/api/rpc/projects/destroyProject"
                } else {
                    "/api/rpc/services/app/destroyService"
                });
                then.status(200).json_body(json!({"json":null}));
            });
            let inspect = server.mock(|when, then| {
                when.method(POST).path(if service.is_empty() {
                    "/api/rpc/projects/inspectProject"
                } else {
                    "/api/rpc/services/app/inspectService"
                });
                then.status(404);
            });
            let mut h = LifecycleHarness::new(&server, service);
            h.state.marked.extend([
                ("project".into(), "web".into(), "app".into()),
                ("other".into(), "web".into(), "app".into()),
            ]);
            h.destroy();
            let output = h.frame(Vec::new());
            assert!(
                !h.backend
                    .tasks
                    .iter()
                    .any(|task| task.label.contains("inspectService")
                        || task.label.contains("inspectProject")),
                "successful deletion must not enqueue an inspection of the removed document"
            );
            for label in ["Deploy", "More", "Delete project", "Shared environment"] {
                assert!(
                    LifecycleHarness::text(&output, label).is_none(),
                    "deleted target must not retain {label}"
                );
            }
            assert!(LifecycleHarness::text(&output, "Open inventory").is_some());
            assert!(LifecycleHarness::text(&output, "Refresh").is_some());
            h.context(|state, cx| quick_action(state, cx, QuickAction::Start));
            assert!(
                h.state.confirmation.is_none(),
                "palette must not revive deleted actions"
            );
            assert_eq!(
                h.state.marked,
                BTreeSet::from([("other".into(), "web".into(), "app".into())])
            );
            h.click("Open inventory");
            let opened = h.opened.last().unwrap();
            assert_eq!(opened.kind, PageKind::Project);
            assert!(opened.target.project.is_empty() && opened.target.service.is_empty());
            assert_eq!(
                opened.target.server.as_ref().unwrap().url,
                server.base_url()
            );
            assert_eq!(
                h.target.project, "project",
                "the deleted document must not retarget"
            );
            delete.assert_hits(1);
            inspect.assert_hits(0);
        }
    }
    #[test]
    fn deletion_state_failed_destroy_preserves_document_draft_and_marks() {
        use httpmock::{Method::POST, MockServer};
        let server = MockServer::start();
        let delete = server.mock(|when, then| {
            when.method(POST)
                .path("/api/rpc/services/app/destroyService");
            then.status(403);
        });
        let mut h = LifecycleHarness::new(&server, "web");
        h.state.edit(
            Form::new(
                FormKind::ProjectCreate,
                "Environment",
                vec![Field::editor("Environment", "saved")],
            ),
            Intent::Environment,
        );
        set(
            &mut h.state.editor.as_mut().unwrap().form,
            "Environment",
            "unsaved",
        );
        let mark = ("project".into(), "web".into(), "app".into());
        h.state.marked.insert(mark.clone());
        h.destroy();
        assert!(h.state.error.is_some());
        assert_eq!(h.state.detail, Some(json!({"name":"original"})));
        assert_eq!(
            h.state
                .editor
                .as_ref()
                .unwrap()
                .form
                .by_label("Environment"),
            "unsaved"
        );
        assert!(h.state.marked.contains(&mark));
        assert!(h.opened.is_empty());
        let output = h.frame(Vec::new());
        assert!(LifecycleHarness::text(&output, "More").is_some());
        delete.assert_hits(1);
    }
    #[test]
    fn deletion_state_fresh_inventory_excludes_missing_marks_from_next_bulk_review() {
        use httpmock::{Method::POST, MockServer};
        let server = MockServer::start();
        let valid_start = server.mock(|when, then| {
            when.method(POST)
                .path("/api/rpc/services/app/startService")
                .json_body(json!({"json":{"projectName":"project","serviceName":"web"}}));
            then.status(200).json_body(json!({"json":null}));
        });
        let failed_start = server.mock(|when, then| {
            when.method(POST)
                .path("/api/rpc/services/app/startService")
                .json_body(json!({"json":{"projectName":"other","serviceName":"web"}}));
            then.status(403);
        });
        let missing_start = server.mock(|when, then| {
            when.method(POST)
                .path("/api/rpc/services/app/startService")
                .json_body(json!({"json":{"projectName":"gone","serviceName":"web"}}));
            then.status(200).json_body(json!({"json":null}));
        });
        let mut h = LifecycleHarness::new(&server, "");
        h.target.project.clear();
        let valid = ("project".into(), "web".into(), "app".into());
        let failed = ("other".into(), "web".into(), "app".into());
        let missing = ("gone".into(), "web".into(), "app".into());
        let old_kind = ("project".into(), "web".into(), "mysql".into());
        h.state
            .marked
            .extend([valid.clone(), failed.clone(), missing, old_kind]);
        h.context(|_, cx| {
            cx.background("inventory", "fresh inventory", || {
                Ok(Payload::Worker(Resp::AllServices {
                    projects: vec!["project".into(), "other".into()],
                    services: vec![
                        json!({"projectName":"project","name":"web","type":"app"}),
                        json!({"projectName":"other","name":"web","type":"app"}),
                    ],
                }))
            })
        });
        h.settle("inventory");
        h.click("Selection actions");
        h.click("Start");
        let Some(Confirmation {
            action: ConfirmAction::Request(Req::Bulk { targets, .. }),
            ..
        }) = &h.state.confirmation
        else {
            panic!("bulk review must open")
        };
        assert_eq!(
            targets,
            &vec![failed.clone(), valid.clone()],
            "next bulk request must retain surviving identities, not missing marks or old kinds"
        );
        assert_eq!(h.state.marked.len(), 2);
        h.click("Confirm");
        h.settle("mutation");
        assert!(h.state.marked.contains(&failed));
        assert!(h.state.marked.contains(&valid));
        valid_start.assert_hits(1);
        failed_start.assert_hits(1);
        missing_start.assert_hits(0);
    }
    #[test]
    fn deletion_state_partial_bulk_removes_only_exact_successful_targets() {
        use httpmock::{Method::POST, MockServer};
        let server = MockServer::start();
        let deleted = server.mock(|when, then| {
            when.method(POST)
                .path("/api/rpc/services/app/destroyService")
                .json_body(json!({"json":{"projectName":"project","serviceName":"web"}}));
            then.status(200).json_body(json!({"json":null}));
        });
        let failed = server.mock(|when, then| {
            when.method(POST)
                .path("/api/rpc/services/app/destroyService")
                .json_body(json!({"json":{"projectName":"other","serviceName":"web"}}));
            then.status(403);
        });
        let mut h = LifecycleHarness::new(&server, "web");
        let gone = ("project".into(), "web".into(), "app".into());
        let retained = ("other".into(), "web".into(), "app".into());
        let other_kind = ("project".into(), "web".into(), "mysql".into());
        h.state
            .marked
            .extend([gone.clone(), retained.clone(), other_kind.clone()]);
        h.state.confirm(
            "Destroy 2 services",
            "project/web, other/web",
            true,
            ConfirmAction::Request(Req::Bulk {
                targets: vec![gone.clone(), retained.clone()],
                action: "destroy".into(),
                force: false,
            }),
        );
        h.state.confirmation_entry = "DESTROY".into();
        h.click("Confirm");
        h.settle("mutation");
        assert!(!h.state.marked.contains(&gone));
        assert!(h.state.marked.contains(&retained));
        assert!(h.state.marked.contains(&other_kind));
        assert!(h
            .state
            .outcome
            .iter()
            .any(|line| line.contains("Failed: other/web")));
        let output = h.frame(Vec::new());
        assert!(LifecycleHarness::text(&output, "Deploy").is_none());
        deleted.assert_hits(1);
        failed.assert_hits(1);
    }
    #[test]
    fn deletion_state_refresh_does_not_retarget_an_already_open_bulk_review() {
        use httpmock::MockServer;
        let server = MockServer::start();
        let mut h = LifecycleHarness::new(&server, "");
        h.target.project.clear();
        let captured = vec![
            ("gone".into(), "web".into(), "app".into()),
            ("project".into(), "web".into(), "app".into()),
        ];
        h.state.marked.extend(captured.clone());
        h.state.confirm(
            "Start 2 services",
            "gone/web, project/web",
            false,
            ConfirmAction::Request(Req::Bulk {
                targets: captured.clone(),
                action: "start".into(),
                force: false,
            }),
        );
        h.context(|_, cx| {
            cx.background("inventory", "fresh inventory", || {
                Ok(Payload::Worker(Resp::AllServices {
                    projects: vec!["project".into()],
                    services: vec![json!({"projectName":"project","name":"web","type":"app"})],
                }))
            })
        });
        h.settle("inventory");
        h.click("Confirm");
        let Some(Confirmation {
            action: ConfirmAction::Request(Req::Bulk { targets, .. }),
            ..
        }) = &h.state.confirmation
        else {
            panic!("changed inventory must require a new review")
        };
        assert_eq!(
            targets, &captured,
            "an open review must never silently shrink or retarget"
        );
        assert!(!h.backend.tasks.iter().any(|task| task.label == "mutation"));
        h.state.confirmation = None;
        h.state.edit(
            Form::new(
                FormKind::BulkResourceEdit,
                "Resources",
                form::resource_fields(None),
            ),
            Intent::BulkResources(captured.clone()),
        );
        set(
            &mut h.state.editor.as_mut().unwrap().form,
            "CPU limit (core)",
            "2",
        );
        assert!(
            h.state.is_dirty(),
            "the Save control must be enabled for this draft"
        );
        h.click("Save changes");
        let edit = h.state.editor.as_ref().unwrap();
        assert!(edit.form.error.is_some());
        assert_eq!(edit.form.by_label("CPU limit (core)"), "2");
        assert!(
            edit.dirty(),
            "blocked submission must preserve the unsaved draft"
        );
        assert!(matches!(&edit.intent, Intent::BulkResources(targets) if *targets == captured));
        assert!(!h.backend.tasks.iter().any(|task| task.label == "mutation"));
    }
    #[test]
    fn deletion_state_late_detail_cannot_revive_actions_but_explicit_refresh_can_recover() {
        use httpmock::{Method::POST, MockServer};
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST)
                .path("/api/rpc/services/app/destroyService");
            then.status(200).json_body(json!({"json":null}));
        });
        let inspect = server.mock(|when, then| {
            when.method(POST)
                .path("/api/rpc/services/app/inspectService");
            then.status(200)
                .json_body(json!({"json":{"name":"recreated","type":"app"}}));
        });
        for late_error in [false, true] {
            let mut h = LifecycleHarness::new(&server, "web");
            let (release, receive) = std::sync::mpsc::channel();
            h.context(|_, cx| {
                cx.background("detail", "old detail", move || {
                    receive.recv().unwrap();
                    if late_error {
                        Err(anyhow::anyhow!("old inspection failed"))
                    } else {
                        Ok(Payload::Json(json!({"name":"old"})))
                    }
                })
            });
            h.destroy();
            release.send(()).unwrap();
            h.settle("detail");
            let output = h.frame(Vec::new());
            assert!(LifecycleHarness::text(&output, "Deploy").is_none());
            assert!(
                h.state.error.is_none(),
                "old inspection errors must not replace deletion feedback"
            );
            h.click("Refresh");
            h.settle("detail");
            let output = h.frame(Vec::new());
            assert!(LifecycleHarness::text(&output, "Deploy").is_some());
            assert_eq!(h.state.detail.as_ref().unwrap()["name"], "recreated");
        }
        inspect.assert_hits(2);
    }
    #[test]
    fn deletion_state_explicit_recovery_refreshes_inventory_and_dispatches_reviewed_action() {
        use httpmock::{Method::POST, MockServer};
        let server = MockServer::start();
        let mut missing = server.mock(|when, then| {
            when.method(POST)
                .path("/api/rpc/projects/listProjectsAndServices");
            then.status(200)
                .json_body(json!({"json":{"projects":[{"name":"project"}],"services":[]}}));
        });
        server.mock(|when, then| {
            when.method(POST)
                .path("/api/rpc/services/app/destroyService");
            then.status(200).json_body(json!({"json":null}));
        });
        server.mock(|when, then| {
            when.method(POST)
                .path("/api/rpc/services/app/inspectService");
            then.status(200)
                .json_body(json!({"json":{"name":"web","type":"app"}}));
        });
        let start = server.mock(|when, then| {
            when.method(POST)
                .path("/api/rpc/services/app/startService")
                .json_body(json!({"json":{"projectName":"project","serviceName":"web"}}));
            then.status(200).json_body(json!({"json":null}));
        });
        let mut h = LifecycleHarness::new(&server, "web");
        h.state.inventory_loaded = true;
        h.state.services = vec![json!({"projectName":"project","name":"web","type":"app"})];
        h.destroy();
        h.frame(Vec::new());
        h.settle("inventory");
        assert!(h.state.services.is_empty());
        missing.delete();
        h.click("Refresh");
        h.settle("detail");
        h.settle("inventory");
        h.context(|state, cx| quick_action(state, cx, QuickAction::Start));
        h.click("Confirm");
        h.settle("mutation");
        assert!(
            h.state.confirmation.is_none(),
            "explicitly recovered target must allow a new review"
        );
        start.assert_hits(1);
        use super::super::{Desktop, PageState};
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx);
        let directory = tempfile::tempdir().unwrap();
        let mut app = Desktop::new(&cc, directory.path().join("servers.json"));
        app.backend.deletions = h.backend.deletions.clone();
        let session = || super::super::page(PageKind::Terminal, &h.target, "container", "Terminal");
        app.open(session());
        let old_session = app.utility_doc.unwrap();
        assert!(
            matches!(&app.documents.iter().find(|doc| doc.id == old_session).unwrap().state,
            PageState::Terminal(state) if state.removed())
        );
        app.open(super::super::page(
            PageKind::Service,
            &h.target,
            "overview",
            "Overview",
        ));
        let recovered = app.documents.last_mut().unwrap();
        recovered.state = PageState::Services(Box::new(std::mem::take(&mut h.state)));
        app.open(session());
        let new_session = app.utility_doc.unwrap();
        assert_ne!(
            old_session, new_session,
            "verified recreation opens a new session document, not the old context"
        );
        assert!(
            matches!(&app.documents.iter().find(|doc| doc.id == new_session).unwrap().state,
            PageState::Terminal(state) if !state.removed())
        );
        assert!(
            matches!(&app.documents.iter().find(|doc| doc.id == old_session).unwrap().state,
            PageState::Terminal(state) if state.removed())
        );
    }
    #[test]
    fn deletion_state_cross_document_success_blocks_hidden_drafts_even_after_origin_closes() {
        use super::super::{Desktop, PageState};
        use httpmock::{Method::POST, MockServer};
        for (project_delete, close_origin, status, bulk) in [
            (false, false, 200, false),
            (true, false, 200, false),
            (false, true, 200, false),
            (true, true, 200, false),
            (false, false, 403, false),
            (true, false, 403, false),
            (false, false, 200, true),
            (false, true, 200, true),
        ] {
            let server = MockServer::start();
            let failed_delete = server.mock(|when, then| {
                when.method(POST)
                    .path("/api/rpc/services/app/destroyService")
                    .json_body(json!({"json":{"projectName":"unrelated","serviceName":"web"}}));
                then.status(403);
            });
            let delete = server.mock(|when, then| {
                when.method(POST).path(if project_delete {
                    "/api/rpc/projects/destroyProject"
                } else {
                    "/api/rpc/services/app/destroyService"
                });
                then.status(status).json_body(json!({"json":null}));
            });
            let fixture = LifecycleHarness::new(&server, "web");
            let target = fixture.target.clone();
            let ctx = egui::Context::default();
            let cc = eframe::CreationContext::_new_kittest(ctx.clone());
            let dir = tempfile::tempdir().unwrap();
            let mut app = Desktop::new(&cc, dir.path().join("servers.json"));
            let mut watched = Vec::new();
            for (mut t, section, affected) in [
                (target.clone(), "environment", true),
                (
                    Target {
                        service: "sibling".into(),
                        ..target.clone()
                    },
                    "environment",
                    project_delete,
                ),
                (
                    Target {
                        project: "unrelated".into(),
                        ..target.clone()
                    },
                    "environment",
                    false,
                ),
                (
                    {
                        let mut other = target.clone();
                        other.server.as_mut().unwrap().token = "different-auth-scope".into();
                        other
                    },
                    "environment",
                    false,
                ),
            ] {
                t.section = section.into();
                app.open(super::super::page(
                    PageKind::Service,
                    &t,
                    section,
                    "Environment",
                ));
                let document = app.documents.last_mut().unwrap();
                let PageState::Services(state) = &mut document.state else {
                    unreachable!()
                };
                state.initialized = true;
                state.last_poll = Some(Instant::now());
                state.edit(
                    Form::new(
                        FormKind::ProjectCreate,
                        "Environment",
                        vec![Field::editor("Environment", "saved")],
                    ),
                    Intent::Environment,
                );
                set(
                    &mut state.editor.as_mut().unwrap().form,
                    "Environment",
                    "UNSAVED=exact\n",
                );
                watched.push((document.id, affected));
            }
            let inventory_target = Target {
                server: target.server.clone(),
                ..Default::default()
            };
            app.open(super::super::page(
                PageKind::Project,
                &inventory_target,
                "services",
                "Projects",
            ));
            let inventory = app.documents.last_mut().unwrap();
            let inventory_id = inventory.id;
            let captured = vec![
                ("project".into(), "web".into(), "app".into()),
                ("unrelated".into(), "web".into(), "app".into()),
            ];
            let PageState::Services(state) = &mut inventory.state else {
                unreachable!()
            };
            state.initialized = true;
            state.last_poll = Some(Instant::now());
            state.marked.extend(captured.clone());
            state.confirm(
                "Start selected",
                "project/web, unrelated/web",
                false,
                ConfirmAction::Request(Req::Bulk {
                    targets: captured.clone(),
                    action: "start".into(),
                    force: false,
                }),
            );
            let origin_target = if project_delete {
                Target {
                    service: String::new(),
                    service_type: String::new(),
                    ..target.clone()
                }
            } else {
                target.clone()
            };
            app.open(super::super::page(
                if project_delete {
                    PageKind::Project
                } else {
                    PageKind::Service
                },
                &origin_target,
                "overview",
                "Overview",
            ));
            let origin = app.active.unwrap();
            let mut opened = Vec::new();
            let mut reload = false;
            let path = dir.path();
            let mut cx = PageContext {
                target: &origin_target,
                servers: &[],
                accounts: &[],
                server_config: path,
                cloudflare_config: path,
                watchlist: path,
                doc: origin,
                backend: &mut app.backend,
                opened: &mut opened,
                reload: &mut reload,
            };
            cx.request(
                "mutation",
                if project_delete {
                    Req::ProjectDestroy("project".into())
                } else if bulk {
                    Req::Bulk {
                        targets: captured.clone(),
                        action: "destroy".into(),
                        force: false,
                    }
                } else {
                    Req::Action {
                        project: "project".into(),
                        service: "web".into(),
                        stype: "app".into(),
                        action: "destroy".into(),
                        force: false,
                    }
                },
            );
            if close_origin {
                app.documents.retain(|doc| doc.id != origin);
                app.backend.close(origin);
            }
            let deadline = Instant::now() + Duration::from_secs(5);
            while app.backend.active() != 0 && Instant::now() < deadline {
                app.pump();
                std::thread::sleep(Duration::from_millis(2));
            }
            assert_eq!(app.backend.active(), 0);
            let inventory = app
                .documents
                .iter()
                .find(|doc| doc.id == inventory_id)
                .unwrap();
            let PageState::Services(state) = &inventory.state else {
                unreachable!()
            };
            assert_eq!(state.confirmation_invalidated, status == 200);
            assert!(matches!(&state.confirmation.as_ref().unwrap().action,
                ConfirmAction::Request(Req::Bulk { targets, .. }) if *targets == captured));
            assert!(state.marked.contains(&captured[1]));
            assert_eq!(state.marked.contains(&captured[0]), status != 200);
            for (id, affected) in watched {
                let document = app.documents.iter().find(|doc| doc.id == id).unwrap();
                let PageState::Services(state) = &document.state else {
                    unreachable!()
                };
                assert_eq!(
                    state.deleted,
                    affected && status == 200,
                    "hidden document must receive only matched successful deletion"
                );
                assert_eq!(
                    state.editor.as_ref().unwrap().form.by_label("Environment"),
                    "UNSAVED=exact\n"
                );
                assert!(state.is_dirty());
                app.active = Some(id);
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(1200.0, 800.0),
                        )),
                        ..Default::default()
                    },
                    |ui| {
                        egui::CentralPanel::default()
                            .show(ui, |ui| app.document(ui, &mut Vec::new()));
                    },
                );
                output.textures_delta.clear();
                assert_eq!(
                    LifecycleHarness::text(&output, "Save changes").is_some(),
                    !(affected && status == 200),
                    "deleted drafts must not remain actionable"
                );
            }
            delete.assert_hits(1);
            failed_delete.assert_hits(usize::from(bulk));
        }
    }
    #[test]
    fn environment_editor_preserves_nested_content_and_rejects_malformed_inspection() {
        let server = httpmock::MockServer::start();
        let raw = "\nEXACT= value  \r\n";
        for project in [false, true] {
            for env in [json!(raw), json!({"content":raw}), json!({"content":42})] {
                let mut h = LifecycleHarness::new(&server, if project { "" } else { "web" });
                let malformed = env["content"].is_number();
                let resource = json!({"env":env});
                h.state.detail = Some(if project {
                    json!({"project":resource})
                } else {
                    resource
                });
                h.context(|state, cx| begin_edit(state, cx, Intent::Environment));
                if malformed {
                    assert!(h.state.editor.is_none());
                    assert!(h.state.error.is_some());
                } else {
                    assert_eq!(
                        h.state
                            .editor
                            .as_ref()
                            .unwrap()
                            .form
                            .by_label("Environment")
                            .as_bytes(),
                        raw.as_bytes()
                    );
                    assert!(!h.state.is_dirty());
                }
            }
        }
    }
    #[test]
    fn dirty_service_lifecycle_entry_preserves_draft_and_saved_request_bytes() {
        use super::super::backend::Backend;
        for (intent, field, kind) in [
            (Intent::Environment, "Environment", "app"),
            (Intent::Configuration, "Config file", "mysql"),
        ] {
            let ctx = egui::Context::default();
            let mut backend = Backend::new(ctx.clone(), super::super::Runtime::default());
            let target = Target {
                project: "project".into(),
                service: "service".into(),
                service_type: kind.into(),
                ..Default::default()
            };
            let mut opened = Vec::new();
            let mut reload = false;
            let path = std::path::Path::new("");
            let mut cx = PageContext {
                target: &target,
                servers: &[],
                accounts: &[],
                server_config: path,
                cloudflare_config: path,
                watchlist: path,
                doc: 1,
                backend: &mut backend,
                opened: &mut opened,
                reload: &mut reload,
            };
            for raw in ["", "\n\nEXACT=  value  \r\n"] {
                let mut state = State::default();
                state.edit(
                    Form::new(
                        FormKind::ProjectCreate,
                        "Configuration",
                        vec![Field::editor(field, "original")],
                    ),
                    intent.clone(),
                );
                set(&mut state.editor.as_mut().unwrap().form, field, raw);
                for (label, action, force) in [
                    ("Deploy", "deploy", false),
                    ("Rebuild", "deploy", true),
                    ("Start", "start", false),
                    ("Restart", "restart", false),
                ] {
                    lifecycle(
                        &mut state,
                        ("project".into(), "service".into(), kind.into()),
                        label,
                        action,
                        force,
                    );
                    assert!(
                        state.confirmation.is_none(),
                        "{label} must not review a dirty draft"
                    );
                    assert!(state.error.take().is_some());
                    assert_eq!(state.editor.as_ref().unwrap().form.by_label(field), raw);
                    assert!(state.saving_generation.is_none());
                }
                for action in [
                    QuickAction::Deploy,
                    QuickAction::Rebuild,
                    QuickAction::Start,
                    QuickAction::Restart,
                ] {
                    quick_action(&mut state, &mut cx, action);
                    assert!(state.confirmation.is_none());
                    assert_eq!(state.editor.as_ref().unwrap().form.by_label(field), raw);
                }
                match submission(state.editor.as_ref().unwrap(), &cx).unwrap() {
                    Req::EnvSave { env, .. } => assert_eq!(env.as_bytes(), raw.as_bytes()),
                    Req::ConfigFileSave { config, .. } => {
                        assert_eq!(config.as_bytes(), raw.as_bytes())
                    }
                    _ => panic!("must submit the original configuration operation"),
                }
                quick_action(&mut state, &mut cx, QuickAction::Stop);
                assert!(matches!(state.confirmation.as_ref().map(|c| &c.action),
                    Some(ConfirmAction::Request(Req::Action { action, .. })) if action == "stop"));
                state.confirmation = None;
                quick_action(&mut state, &mut cx, QuickAction::Destroy);
                assert!(state.confirmation.as_ref().unwrap().typed);
                state.confirmation = None;
                let edit = state.editor.as_mut().unwrap();
                edit.baseline = edit.form.fields.iter().map(|f| f.value.clone()).collect();
                assert!(!state.is_dirty());
                let next = if kind == "app" {
                    QuickAction::Deploy
                } else {
                    QuickAction::Restart
                };
                quick_action(&mut state, &mut cx, next);
                assert!(matches!(state.confirmation.as_ref().map(|c| &c.action),
                    Some(ConfirmAction::Request(Req::Action { action, .. })) if action == if kind == "app" { "deploy" } else { "restart" }));
                assert_eq!(state.editor.as_ref().unwrap().form.by_label(field), raw);
            }
        }
    }

    #[test]
    fn palette_lifecycle_routes_to_the_dirty_editor_without_opening_a_clean_bypass() {
        use super::super::{Desktop, PageKind, PageState, PaletteCommand};
        for raw in ["", "\n\nEXACT=  value  \r\n"] {
            let ctx = egui::Context::default();
            let cc = eframe::CreationContext::_new_kittest(ctx);
            let directory = tempfile::tempdir().unwrap();
            let mut app = Desktop::new(&cc, directory.path().join("servers.json"));
            let elsewhere = app.active;
            let target = Target {
                server: Some(Server {
                    name: "host".into(),
                    url: "https://panel.invalid".into(),
                    token: "fixture-token".into(),
                    default: false,
                    ssh: None,
                }),
                project: "project".into(),
                service: "service".into(),
                service_type: "app".into(),
                ..Target::default()
            };
            app.open(super::super::page(
                PageKind::Service,
                &target,
                "overview",
                "Overview",
            ));
            app.open(super::super::page(
                PageKind::Service,
                &target,
                "environment",
                "Environment",
            ));
            let editor_id = app.active.unwrap();
            let document = app.documents.last_mut().unwrap();
            let original = document.page.target.clone();
            let PageState::Services(state) = &mut document.state else {
                unreachable!();
            };
            state.edit(
                Form::new(
                    FormKind::ProjectCreate,
                    "Environment",
                    vec![Field::editor("Environment", "original")],
                ),
                Intent::Environment,
            );
            set(&mut state.editor.as_mut().unwrap().form, "Environment", raw);
            let count = app.documents.len();
            for active in [Some(editor_id), elsewhere] {
                for action in [
                    QuickAction::Deploy,
                    QuickAction::Rebuild,
                    QuickAction::Start,
                    QuickAction::Restart,
                ] {
                    app.active = active;
                    app.execute_palette(PaletteCommand::ServiceAction {
                        target: target.clone(),
                        title: "Fixture lifecycle action".into(),
                        action,
                    });
                    assert_eq!(app.active, Some(editor_id));
                    assert_eq!(app.documents.len(), count);
                    let document = app
                        .documents
                        .iter()
                        .find(|doc| doc.id == editor_id)
                        .unwrap();
                    assert!(document.page.target.same_resource(&original));
                    let PageState::Services(state) = &document.state else {
                        unreachable!();
                    };
                    assert!(state.confirmation.is_none());
                    assert!(state.error.is_some());
                    assert!(state.saving_generation.is_none());
                    assert!(state.is_dirty());
                    assert_eq!(
                        state
                            .editor
                            .as_ref()
                            .unwrap()
                            .form
                            .by_label("Environment")
                            .as_bytes(),
                        raw.as_bytes()
                    );
                    assert!(app
                        .documents
                        .iter()
                        .all(|doc| !app.backend.busy(doc.id, "mutation")));
                }
            }
            app.execute_palette(PaletteCommand::ServiceAction {
                target,
                title: "Destroy service".into(),
                action: QuickAction::Destroy,
            });
            let PageState::Services(state) = &app
                .documents
                .iter()
                .find(|doc| doc.id == editor_id)
                .unwrap()
                .state
            else {
                unreachable!();
            };
            assert!(state.confirmation.as_ref().unwrap().typed);
            assert_eq!(
                state.editor.as_ref().unwrap().form.by_label("Environment"),
                raw
            );
            assert!(app
                .documents
                .iter()
                .all(|doc| !app.backend.busy(doc.id, "mutation")));
        }
    }

    #[test]
    fn palette_lifecycle_reuses_a_clean_active_section_and_keeps_connection_identity() {
        use super::super::{Desktop, PageKind, PageState, PaletteCommand};
        for route in ["active", "overview", "token", "endpoint", "ssh"] {
            let ctx = egui::Context::default();
            let cc = eframe::CreationContext::_new_kittest(ctx);
            let directory = tempfile::tempdir().unwrap();
            let mut app = Desktop::new(&cc, directory.path().join("servers.json"));
            let elsewhere = app.active;
            let target = Target {
                server: Some(Server {
                    name: "host".into(),
                    url: "https://panel.invalid".into(),
                    token: "fixture-token".into(),
                    default: false,
                    ssh: Some(crate::tunnel::SshTunnel {
                        host: "bastion.invalid".into(),
                        secret: Some("fixture-secret".into()),
                        ..Default::default()
                    }),
                }),
                project: "project".into(),
                service: "service".into(),
                service_type: "app".into(),
                ..Target::default()
            };
            app.open(super::super::page(
                PageKind::Service,
                &target,
                "environment",
                "Environment",
            ));
            let editor_id = app.active.unwrap();
            let mut requested = target.clone();
            match route {
                "token" => requested.server.as_mut().unwrap().token = "replacement-token".into(),
                "endpoint" => {
                    requested.server.as_mut().unwrap().url = "https://other.invalid".into()
                }
                "ssh" => {
                    requested
                        .server
                        .as_mut()
                        .unwrap()
                        .ssh
                        .as_mut()
                        .unwrap()
                        .secret = Some("replacement-secret".into());
                }
                "overview" => app.active = elsewhere,
                _ => {}
            }
            let count = app.documents.len();
            app.execute_palette(PaletteCommand::ServiceAction {
                target: requested.clone(),
                title: "Deploy service".into(),
                action: QuickAction::Deploy,
            });
            let document = app
                .documents
                .iter()
                .find(|doc| Some(doc.id) == app.active)
                .unwrap();
            if route == "active" {
                assert_eq!(document.id, editor_id);
                assert_eq!(document.page.target.section, "environment");
                assert_eq!(app.documents.len(), count);
            } else {
                assert_ne!(document.id, editor_id);
                assert_eq!(document.page.target.section, "overview");
                assert_eq!(app.documents.len(), count + 1);
            }
            let expected = super::super::page(
                PageKind::Service,
                &requested,
                &document.page.target.section,
                "Fixture",
            );
            assert!(document.page.target.same_resource(&expected.target));
            let PageState::Services(state) = &document.state else {
                unreachable!();
            };
            assert!(
                matches!(state.confirmation.as_ref().map(|review| &review.action),
                Some(ConfirmAction::Request(Req::Action { action, force: false, .. })) if action == "deploy")
            );
            assert!(app
                .documents
                .iter()
                .all(|doc| !app.backend.busy(doc.id, "mutation")));
        }
    }
    #[test]
    fn inventory_keeps_identity_visible_and_optional_columns_scrollable() {
        use super::super::backend::Backend;
        for size in [egui::vec2(1024.0, 680.0), egui::vec2(1280.0, 800.0)] {
            let ctx = egui::Context::default();
            super::super::theme::apply(&ctx, super::super::theme::Mode::Light);
            let mut backend = Backend::new(ctx.clone(), super::super::Runtime::default());
            let target = Target::default();
            let mut opened = Vec::new();
            let mut reload = false;
            let path = std::path::Path::new("");
            let mut cx = PageContext {
                target: &target,
                servers: &[],
                accounts: &[],
                server_config: path,
                cloudflare_config: path,
                watchlist: path,
                doc: 1,
                backend: &mut backend,
                opened: &mut opened,
                reload: &mut reload,
            };
            let mut state = State {
                services: vec![json!({
                    "projectName": "project", "name": "queue-worker-with-a-long-name", "type": "app",
                    "source": {"type": "image", "image": "registry.example/worker:latest"}
                })],
                ..Default::default()
            };
            state
                .replicas
                .insert("project_queue-worker-with-a-long-name".into(), (0, 1));
            let rows = service_rows(&state);
            let mut measured = None;
            let mut render = |events| {
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                        events,
                        ..Default::default()
                    },
                    |ui| {
                        egui::Panel::left("explorer")
                            .exact_size(298.0)
                            .show(ui, |_| {});
                        egui::CentralPanel::default()
                            .frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(12, 4)))
                            .show(ui, |ui| {
                                let bounds = ui.available_rect_before_wrap();
                                inventory(ui, &mut state, &mut cx, &rows);
                                measured = Some((bounds, ui.min_rect()));
                            });
                    },
                );
                output.textures_delta.clear();
                output
            };
            let output = render(Vec::new());
            let text_rect = |output: &egui::FullOutput, label: &str| {
                output
                    .shapes
                    .iter()
                    .find_map(|clipped| match &clipped.shape {
                        egui::epaint::Shape::Text(text) if text.galley.text() == label => Some((
                            text.galley.rect.translate(text.pos.to_vec2()),
                            clipped.clip_rect,
                        )),
                        _ => None,
                    })
                    .unwrap_or_else(|| panic!("missing {label}"))
            };
            for label in ["Service", "Runtime", "Project", "Type"] {
                let (rect, clip) = text_rect(&output, label);
                assert!(
                    clip.contains_rect(rect),
                    "{label} must be visible at {size:?}"
                );
            }
            let (runtime, _) = text_rect(&output, "Runtime");
            let (project, _) = text_rect(&output, "Project");
            let (kind, _) = text_rect(&output, "Type");
            let (cpu, _) = text_rect(&output, "CPU");
            assert!(
                runtime.max.x < project.min.x
                    && project.max.x < kind.min.x
                    && kind.max.x < cpu.min.x
            );
            if size.x == 1024.0 {
                render(vec![
                    egui::Event::PointerMoved(egui::pos2(800.0, 100.0)),
                    egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        delta: egui::vec2(-2000.0, 0.0),
                        phase: egui::TouchPhase::Move,
                        source: egui::MouseWheelSource::Wheel,
                        modifiers: egui::Modifiers::NONE,
                    },
                ]);
                for _ in 0..30 {
                    render(Vec::new());
                }
                let output = render(Vec::new());
                for label in ["Source", "Auto deploy"] {
                    let (rect, clip) = text_rect(&output, label);
                    assert!(
                        clip.contains_rect(rect),
                        "{label} must be horizontally reachable"
                    );
                }
            }
            let (bounds, content) = measured.unwrap();
            assert!(content.max.x <= bounds.max.x + 0.5);
            assert!(content.max.y <= bounds.max.y + 0.5);
        }
    }
    #[test]
    fn service_review_stays_visible_and_guards_two_hundred_captured_targets() {
        use super::super::backend::Backend;
        fn frame(
            ctx: &egui::Context,
            state: &mut State,
            cx: &mut PageContext<'_>,
            size: egui::Vec2,
            events: Vec<egui::Event>,
        ) -> egui::FullOutput {
            let mut measured = None;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                    events,
                    ..Default::default()
                },
                |ui| {
                    egui::Panel::top("menubar")
                        .exact_size(32.0)
                        .show(ui, |_| {});
                    egui::Panel::bottom("statusbar")
                        .exact_size(24.0)
                        .show(ui, |_| {});
                    egui::Panel::left("activitybar")
                        .exact_size(46.0)
                        .show(ui, |_| {});
                    egui::Panel::left("explorer")
                        .exact_size(252.0)
                        .show(ui, |_| {});
                    egui::Panel::bottom("utility")
                        .exact_size(220.0)
                        .show(ui, |_| {});
                    egui::CentralPanel::default()
                        .frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(12, 4)))
                        .show(ui, |ui| {
                            ui.allocate_space(egui::vec2(ui.available_width(), 32.0));
                            ui.separator();
                            let bounds = ui.available_rect_before_wrap();
                            show(ui, state, cx);
                            measured = Some((bounds, ui.min_rect()));
                        });
                },
            );
            output.textures_delta.clear();
            let (bounds, content) = measured.unwrap();
            assert!(
                content.max.y <= bounds.max.y + 0.5,
                "{content:?} exceeds {bounds:?}"
            );
            assert!(
                content.max.x <= bounds.max.x + 0.5,
                "{content:?} exceeds {bounds:?}"
            );
            output
        }
        fn text_bounds(output: &egui::FullOutput, text: &str) -> (egui::Rect, egui::Rect) {
            output
                .shapes
                .iter()
                .find_map(|clipped| match &clipped.shape {
                    egui::epaint::Shape::Text(shape) if shape.galley.text() == text => Some((
                        shape.galley.rect.translate(shape.pos.to_vec2()),
                        clipped.clip_rect,
                    )),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("missing {text}"))
        }
        fn click(
            ctx: &egui::Context,
            state: &mut State,
            cx: &mut PageContext<'_>,
            size: egui::Vec2,
            pos: egui::Pos2,
        ) {
            for pressed in [true, false] {
                frame(
                    ctx,
                    state,
                    cx,
                    size,
                    vec![
                        egui::Event::PointerMoved(pos),
                        egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ],
                );
            }
        }
        for size in [egui::vec2(1024.0, 680.0), egui::vec2(1280.0, 800.0)] {
            for service in ["", "selected-service"] {
                let ctx = egui::Context::default();
                super::super::theme::apply(&ctx, super::super::theme::Mode::Dark);
                let target = Target {
                    service: service.into(),
                    service_type: "app".into(),
                    ..Default::default()
                };
                let mut backend = Backend::new(ctx.clone(), super::super::Runtime::default());
                let mut opened = Vec::new();
                let mut reload = false;
                let path = std::path::Path::new("");
                let mut cx = PageContext {
                    target: &target,
                    servers: &[],
                    accounts: &[],
                    server_config: path,
                    cloudflare_config: path,
                    watchlist: path,
                    doc: 1,
                    backend: &mut backend,
                    opened: &mut opened,
                    reload: &mut reload,
                };
                let targets: Vec<_> = (0..200)
                    .map(|i| {
                        (
                            "project-with-a-long-identity".to_owned(),
                            format!("service-with-a-long-captured-identity-{i}"),
                            "app".to_owned(),
                        )
                    })
                    .collect();
                let names = targets
                    .iter()
                    .map(|(p, s, _)| format!("{p}/{s}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                let mut state = State {
                    initialized: true,
                    last_poll: Some(Instant::now()),
                    projects: vec!["project-with-a-long-identity".into()],
                    services: targets
                        .iter()
                        .map(|(p, s, k)| {
                            json!({
                                "projectName": p, "name": s, "type": k
                            })
                        })
                        .collect(),
                    ..Default::default()
                };
                for bulk in [false, true] {
                    state.edit(
                        Form::new(
                            FormKind::ProjectCreate,
                            "Environment",
                            vec![Field::editor("Environment", "SAVED=value\n")],
                        ),
                        Intent::Environment,
                    );
                    let req = if bulk {
                        Req::Bulk {
                            targets: vec![("project".into(), "service".into(), "app".into())],
                            action: "deploy".into(),
                            force: true,
                        }
                    } else {
                        Req::Action {
                            project: "project".into(),
                            service: "service".into(),
                            stype: "app".into(),
                            action: "deploy".into(),
                            force: false,
                        }
                    };
                    state.confirm(
                        "Deploy service",
                        "project/service",
                        false,
                        ConfirmAction::Request(req),
                    );
                    let raw = "\n\nSECRET= exact draft  \r\n";
                    set(&mut state.editor.as_mut().unwrap().form, "Environment", raw);
                    let output = frame(&ctx, &mut state, &mut cx, size, Vec::new());
                    click(
                        &ctx,
                        &mut state,
                        &mut cx,
                        size,
                        text_bounds(&output, "Confirm").0.center(),
                    );
                    assert!(
                        state.confirmation.is_some(),
                        "a draft arriving during review must block submission"
                    );
                    assert!(!cx.busy("mutation"));
                    assert_eq!(
                        state.editor.as_ref().unwrap().form.by_label("Environment"),
                        raw
                    );
                    let output = frame(&ctx, &mut state, &mut cx, size, Vec::new());
                    click(
                        &ctx,
                        &mut state,
                        &mut cx,
                        size,
                        text_bounds(&output, "Cancel").0.center(),
                    );
                    assert!(state.confirmation.is_none());
                    assert_eq!(
                        state.editor.as_ref().unwrap().form.by_label("Environment"),
                        raw
                    );
                    state.editor = None;
                }
                state.confirm(
                    "Destroy 200 services",
                    names.clone(),
                    true,
                    ConfirmAction::Request(Req::Bulk {
                        targets: targets.clone(),
                        action: "destroy".into(),
                        force: false,
                    }),
                );
                state.confirmation_entry = "DESTRO".into();
                let output = frame(&ctx, &mut state, &mut cx, size, Vec::new());
                for text in [
                    "Destroy 200 services",
                    "Type DESTROY to confirm this target:",
                    "Confirm",
                    "Cancel",
                ] {
                    let (rect, clip) = text_bounds(&output, text);
                    assert!(clip.contains_rect(rect), "{text} must remain visible");
                }
                assert!(!output.shapes.iter().any(|clipped| matches!(&clipped.shape,
                    egui::epaint::Shape::Text(shape) if ["New project", "Deploy"].contains(&shape.galley.text()))));
                let (details, clip) = text_bounds(&output, &names);
                assert!(
                    details.height() > clip.height(),
                    "target list must scroll inside review"
                );
                click(
                    &ctx,
                    &mut state,
                    &mut cx,
                    size,
                    text_bounds(&output, "Confirm").0.center(),
                );
                let confirmation = state
                    .confirmation
                    .as_ref()
                    .expect("partial guard must not submit");
                let ConfirmAction::Request(Req::Bulk {
                    targets: captured, ..
                }) = &confirmation.action
                else {
                    panic!("captured bulk action must be preserved");
                };
                assert_eq!(captured, &targets);
                click(
                    &ctx,
                    &mut state,
                    &mut cx,
                    size,
                    text_bounds(&output, "Cancel").0.center(),
                );
                assert!(state.confirmation.is_none());
                assert_eq!(state.services.len(), 200);
                state.confirm(
                    "Destroy 200 services",
                    names,
                    true,
                    ConfirmAction::Request(Req::Bulk {
                        targets,
                        action: "destroy".into(),
                        force: false,
                    }),
                );
                state.confirmation_entry = "DESTROY".into();
                let output = frame(&ctx, &mut state, &mut cx, size, Vec::new());
                click(
                    &ctx,
                    &mut state,
                    &mut cx,
                    size,
                    text_bounds(&output, "Confirm").0.center(),
                );
                assert!(
                    state.confirmation.is_none(),
                    "exact guard must submit captured action"
                );
            }
        }
    }
    #[test]
    fn multiline_service_editor_keeps_actions_and_help_above_the_dock() {
        use super::super::backend::Backend;
        for (size, mode) in [
            (egui::vec2(1024.0, 680.0), super::super::theme::Mode::Dark),
            (egui::vec2(1024.0, 680.0), super::super::theme::Mode::Light),
            (egui::vec2(1280.0, 800.0), super::super::theme::Mode::Dark),
            (egui::vec2(1280.0, 800.0), super::super::theme::Mode::Light),
        ] {
            for (raw, must_scroll) in [
                ("".to_owned(), false),
                ("KEY=  preserved \r\nNEXT=value\n".to_owned(), false),
                (
                    "\n\nKEY=  leading blank lines \r\nNEXT=value\n".to_owned(),
                    false,
                ),
                (
                    format!("LONG={}\n{}", "x".repeat(2000), "KEY=value\n".repeat(400)),
                    true,
                ),
            ] {
                let ctx = egui::Context::default();
                super::super::theme::apply(&ctx, mode);
                let mut backend = Backend::new(ctx.clone(), super::super::Runtime::default());
                let target = Target {
                    project: "project".into(),
                    service: "service".into(),
                    service_type: "app".into(),
                    section: "environment".into(),
                    ..Default::default()
                };
                let mut state = State {
                    section: "environment".into(),
                    ..Default::default()
                };
                state.editor = Some(make_editor(Form::new(
                    FormKind::ProjectCreate, "Environment",
                    vec![Field::editor("Environment", "original")],
                ).with_note("Values may contain secrets. Save replaces the complete environment; deploy or restart to apply."), Intent::Environment));
                state.editor.as_mut().unwrap().form.fields[0].value = raw.clone();
                let mut opened = Vec::new();
                let mut reload = false;
                let mut cx = PageContext {
                    target: &target,
                    servers: &[],
                    accounts: &[],
                    server_config: std::path::Path::new(""),
                    cloudflare_config: std::path::Path::new(""),
                    watchlist: std::path::Path::new(""),
                    doc: 1,
                    backend: &mut backend,
                    opened: &mut opened,
                    reload: &mut reload,
                };
                let mut measured = None;
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                        ..Default::default()
                    },
                    |ui| {
                        egui::Panel::top("menubar")
                            .exact_size(32.0)
                            .show(ui, |_| {});
                        egui::Panel::bottom("statusbar")
                            .exact_size(24.0)
                            .show(ui, |_| {});
                        egui::Panel::left("activitybar")
                            .exact_size(46.0)
                            .show(ui, |_| {});
                        egui::Panel::left("explorer")
                            .exact_size(252.0)
                            .show(ui, |_| {});
                        egui::Panel::bottom("utility")
                            .exact_size(220.0)
                            .show(ui, |_| {});
                        egui::CentralPanel::default()
                            .frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(12, 4)))
                            .show(ui, |ui| {
                                ui.allocate_space(egui::vec2(ui.available_width(), 32.0));
                                ui.separator();
                                ui.label(target.label());
                                let bounds = ui.available_rect_before_wrap();
                                service(ui, &mut state, &mut cx);
                                editor(ui, &mut state, &mut cx);
                                measured = Some((bounds, ui.min_rect()));
                            });
                    },
                );
                output.textures_delta.clear();
                let (bounds, content) = measured.expect("service editor must render");
                assert!(content.max.y <= bounds.max.y + 0.5,
                    "editor exceeds body: size={size:?}, raw_bytes={}, bounds={bounds:?}, content={content:?}", raw.len());
                assert!(content.max.x <= bounds.max.x + 0.5,
                    "editor exceeds width: size={size:?}, raw_bytes={}, bounds={bounds:?}, content={content:?}", raw.len());
                for text in ["Save changes", "Discard", "Import text file…",
                    "Tab moves focus. Empty content is saved as empty; Discard leaves the original unchanged."]
                {
                    let shape = output.shapes.iter().find_map(|clipped| match &clipped.shape {
                        egui::epaint::Shape::Text(shape) if shape.galley.text() == text =>
                            Some((clipped.clip_rect, shape)),
                        _ => None,
                    }).unwrap_or_else(|| panic!("missing {text}"));
                    assert!(shape.0.contains_rect(shape.1.galley.rect.translate(shape.1.pos.to_vec2())),
                        "{text} is clipped at {size:?}");
                }
                let action_center = |label: &str| {
                    output
                        .shapes
                        .iter()
                        .find_map(|clipped| match &clipped.shape {
                            egui::epaint::Shape::Text(text) if text.galley.text() == label => {
                                Some(text.galley.rect.translate(text.pos.to_vec2()).center())
                            }
                            _ => None,
                        })
                        .unwrap_or_else(|| panic!("missing {label}"))
                };
                let primary = ctx.global_style().visuals.selection.bg_fill;
                let is_primary = |label: &str| {
                    let center = action_center(label);
                    output.shapes.iter().any(|clipped| matches!(&clipped.shape,
                        egui::epaint::Shape::Rect(rect) if rect.rect.contains(center) && rect.fill == primary))
                };
                assert!(
                    is_primary("Save changes"),
                    "dirty Save must own primary emphasis"
                );
                assert!(
                    !is_primary("Deploy"),
                    "Deploy must not compete with dirty Save"
                );
                assert!(
                    (action_center("Section").y - action_center("Deploy").y).abs() < 5.0,
                    "the measured compact toolbar must keep Section in one row"
                );
                if !raw.is_empty() {
                    let (clip, text) = output
                        .shapes
                        .iter()
                        .find_map(|clipped| match &clipped.shape {
                            egui::epaint::Shape::Text(shape) if shape.galley.text() == raw => {
                                Some((clipped.clip_rect, shape))
                            }
                            _ => None,
                        })
                        .expect("multiline editor must paint its content");
                    assert!(clip.height() >= 124.0,
                        "editor has no useful line box: size={size:?}, raw_bytes={}, clip={clip:?}, text_rect={:?}", raw.len(), text.galley.rect);
                    if must_scroll {
                        assert!(
                            text.galley.rect.height() > clip.height(),
                            "long content must scroll internally"
                        );
                    }
                }
                assert_eq!(state.editor.as_ref().unwrap().form.fields[0].value, raw);
            }
        }
    }
    #[test]
    fn section_caption_tracks_bound_documents_and_retained_editors() {
        let environment = Target {
            section: "environment".into(),
            ..Target::default()
        };
        let mut state = State::default();
        assert_eq!(state.section_label(&environment), "Environment");
        state.initialized = true;
        state.section = "source".into();
        assert_eq!(state.section_label(&environment), "Source");
        state.edit(
            Form::new(
                FormKind::ProjectCreate,
                "Environment",
                vec![Field::editor("Environment", "")],
            ),
            Intent::Environment,
        );
        state.editor.as_mut().unwrap().form.fields[0].value = "KEY=retained".into();
        assert_eq!(state.section_label(&environment), "Environment");
        assert!(state.is_dirty());
        assert_eq!(
            state.editor.as_ref().unwrap().form.fields[0].value,
            "KEY=retained"
        );
    }
    #[test]
    fn nonfinite_resource_limits_cannot_silently_become_json_null() {
        let mut f = Form::new(
            FormKind::BulkResourceEdit,
            "Limits",
            form::resource_fields(None),
        );
        for value in ["NaN", "inf", "-1"] {
            f.fields[0].value = value.into();
            assert!(checked_resources(&f).is_err());
        }
        f.fields[0].value = "0.5".into();
        assert_eq!(
            checked_resources(&f).unwrap()["resources"]["cpuLimit"],
            json!(0.5)
        );
    }
    #[test]
    fn connection_edits_keep_credentials_without_putting_them_in_fields() {
        let server = Server {
            name: "host".into(),
            url: "https://panel.invalid".into(),
            token: "private-token".into(),
            default: true,
            ssh: None,
        };
        let form = connection_form(Some(&server));
        assert!(form.val(2).is_empty());
        let baseline = form.fields.iter().map(|f| f.value.clone()).collect();
        let mut edit = Editor {
            form,
            intent: Intent::Connection(Some(server)),
            baseline,
            generation: 1,
            revision: 0,
            discard: false,
        };
        edit.form.fields[0].value = "renamed".into();
        let draft = connection_draft(&edit).unwrap();
        assert!(draft.token.is_empty());
        assert!(draft.default);
        assert_eq!(draft.name, "renamed");
        assert!(edit.dirty());
    }
    #[test]
    fn connection_editor_clears_obsolete_validation_only_after_valid_test_or_save() {
        use super::super::backend::Backend;
        use httpmock::{Method::POST, MockServer};

        fn frame(
            ctx: &egui::Context,
            state: &mut State,
            cx: &mut PageContext<'_>,
            events: Vec<egui::Event>,
        ) -> egui::FullOutput {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1024.0, 680.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    egui::CentralPanel::default().show(ui, |ui| {
                        feedback(ui, state, cx);
                        editor(ui, state, cx);
                    });
                },
            );
            output.textures_delta.clear();
            output
        }
        fn text_rect(output: &egui::FullOutput, label: &str) -> Option<egui::Rect> {
            output
                .shapes
                .iter()
                .find_map(|clipped| match &clipped.shape {
                    egui::epaint::Shape::Text(text) if text.galley.text() == label => {
                        Some(text.galley.rect.translate(text.pos.to_vec2()))
                    }
                    _ => None,
                })
        }
        fn click(ctx: &egui::Context, state: &mut State, cx: &mut PageContext<'_>, label: &str) {
            let output = frame(ctx, state, cx, Vec::new());
            let pos = text_rect(&output, label)
                .expect("action must render")
                .center();
            for pressed in [true, false] {
                frame(
                    ctx,
                    state,
                    cx,
                    vec![
                        egui::Event::PointerMoved(pos),
                        egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ],
                );
            }
        }

        for (action, status) in [
            ("Test connection", 200),
            ("Test connection", 401),
            ("Add connection", 200),
        ] {
            let server = MockServer::start();
            let api = server.mock(|when, then| {
                when.method(POST).path("/api/rpc/projects/listProjects");
                then.status(status).json_body(json!({"json":[]}));
            });
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("servers.json");
            let ctx = egui::Context::default();
            let mut backend = Backend::new(ctx.clone(), super::super::Runtime::default());
            let target = Target::default();
            let mut opened = Vec::new();
            let mut reload = false;
            let mut cx = PageContext {
                target: &target,
                servers: &[],
                accounts: &[],
                server_config: &path,
                cloudflare_config: &path,
                watchlist: &path,
                doc: 1,
                backend: &mut backend,
                opened: &mut opened,
                reload: &mut reload,
            };
            let mut state = State::default();
            state.edit(connection_form(None), Intent::Connection(None));
            click(&ctx, &mut state, &mut cx, action);
            assert_eq!(
                state.editor.as_ref().unwrap().form.error.as_deref(),
                Some("Name and URL are required")
            );
            let output = frame(&ctx, &mut state, &mut cx, Vec::new());
            assert!(text_rect(&output, "Validation error").is_some());
            assert!(cx.backend.tasks.is_empty());

            let form = &mut state.editor.as_mut().unwrap().form;
            set(form, "Name", "corrected");
            set(form, "URL", &server.base_url());
            click(&ctx, &mut state, &mut cx, action);
            assert_eq!(
                state.editor.as_ref().unwrap().form.error.as_deref(),
                Some("Token is required")
            );
            assert!(
                cx.backend.tasks.is_empty(),
                "invalid retry must not dispatch"
            );

            set(
                &mut state.editor.as_mut().unwrap().form,
                "Token",
                "fixture-token",
            );
            click(&ctx, &mut state, &mut cx, action);
            let key = if action == "Test connection" {
                "connection-test"
            } else {
                "mutation"
            };
            assert!(cx.busy(key), "valid draft must dispatch the real operation");
            assert!(state.editor.as_ref().unwrap().form.error.is_none());
            assert!(state.outcome.is_empty(), "dispatch must not claim success");
            let output = frame(&ctx, &mut state, &mut cx, Vec::new());
            assert!(text_rect(&output, "Validation error").is_none());
            assert!(text_rect(&output, "Operation result").is_none());

            let deadline = Instant::now() + Duration::from_secs(5);
            while cx.busy(key) && Instant::now() < deadline {
                cx.backend.pump();
                std::thread::sleep(Duration::from_millis(2));
            }
            assert!(!cx.busy(key), "connection operation did not finish");
            pump(&mut state, &mut cx);
            if action == "Add connection" {
                let saved = ServerConfig::new(path.clone()).try_all().unwrap();
                assert_eq!(saved.len(), 1);
                assert_eq!(saved[0].name, "corrected");
                assert!(state.editor.is_none());
                assert!(state.outcome[0].starts_with("Connection saved."));
                api.assert_hits(0);
            } else {
                api.assert_hits(1);
                if status == 200 {
                    assert!(state.error.is_none());
                    assert_eq!(
                        state.outcome,
                        vec![format!("API token OK — {} answered", server.base_url())]
                    );
                } else {
                    assert!(state.error.as_deref().unwrap().contains("API check failed"));
                    assert!(state.outcome.is_empty());
                }
                let error = state.error.clone();
                let outcome = state.outcome.clone();
                set(&mut state.editor.as_mut().unwrap().form, "Name", "");
                click(&ctx, &mut state, &mut cx, action);
                assert_eq!(
                    state.editor.as_ref().unwrap().form.error.as_deref(),
                    Some("Name and URL are required")
                );
                assert!(!cx.busy(key));
                assert_eq!(state.error, error);
                assert_eq!(state.outcome, outcome);
                set(
                    &mut state.editor.as_mut().unwrap().form,
                    "Name",
                    "corrected",
                );
                click(&ctx, &mut state, &mut cx, action);
                assert!(cx.busy(key));
                assert!(state.editor.as_ref().unwrap().form.error.is_none());
                assert_eq!(
                    state.error, error,
                    "valid retry must retain API failure evidence"
                );
                assert_eq!(
                    state.outcome, outcome,
                    "valid retry must retain API result evidence"
                );
                let output = frame(&ctx, &mut state, &mut cx, Vec::new());
                assert!(text_rect(&output, "Validation error").is_none());
                let deadline = Instant::now() + Duration::from_secs(5);
                while cx.busy(key) && Instant::now() < deadline {
                    cx.backend.pump();
                    std::thread::sleep(Duration::from_millis(2));
                }
                assert!(!cx.busy(key));
                pump(&mut state, &mut cx);
                api.assert_hits(2);
            }
        }
    }
    #[test]
    fn ports_do_not_open_zero_or_out_of_range_listeners() {
        for raw in ["0", "65536", "-1", "abc"] {
            assert!(validate_port(raw).is_err());
        }
        assert_eq!(validate_port("65535").unwrap(), 65535);
    }
    #[test]
    fn mixed_bulk_outcomes_keep_each_failed_target_visible() {
        let mut state = State::default();
        assert!(result_message(
            &mut state,
            Resp::BulkDone {
                action: "restart".into(),
                ok: vec!["alpha/web".into()],
                failed: vec![("beta/db".into(), "permission denied".into())],
            }
        ));
        assert!(state.outcome.iter().any(|s| s == "Completed: alpha/web"));
        assert!(state
            .outcome
            .iter()
            .any(|s| s == "Failed: beta/db: permission denied"));
    }
    #[test]
    fn editing_file_contents_can_truncate_without_losing_mount_metadata() {
        let original =
            json!({"type":"file","mountPath":"/etc/app.conf","content":"before\n","owner":"app"});
        let mut form = Form::new(
            FormKind::MountCreate {
                project: "p".into(),
                service: "s".into(),
            },
            "Mount",
            form::mount_fields(Some(&original)),
        )
        .with_original(original);
        form.fields
            .iter_mut()
            .find(|f| f.label == "Content")
            .unwrap()
            .value
            .clear();
        let values = mount_values(&form).unwrap();
        assert_eq!(values["content"], "");
        assert_eq!(values["mountPath"], "/etc/app.conf");
        assert_eq!(values["owner"], "app");
        form.original = None;
        assert_eq!(form::mount_body(&form).unwrap()["content"], "");
    }
    #[test]
    fn blank_connection_edits_preserve_latest_rotated_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = ServerConfig::new(dir.path().join("servers.json"));
        let original = Server {
            name: "host".into(),
            url: "http://panel.invalid".into(),
            token: "old-token".into(),
            default: true,
            ssh: Some(crate::tunnel::SshTunnel {
                host: "jump.invalid".into(),
                auth: crate::tunnel::SshAuth::Password,
                secret: Some("old-password".into()),
                ..Default::default()
            }),
        };
        connections::save_server(&cfg, None, original.clone()).unwrap();
        let form = connection_form(Some(&original));
        assert!(form.val(2).is_empty());
        assert!(form.by_label("SSH password").is_empty());
        let mut editor = make_editor(form, Intent::Connection(Some(original.clone())));
        set(&mut editor.form, "Name", "renamed");
        let mut rotated = original;
        rotated.token = "rotated-token".into();
        rotated.ssh.as_mut().unwrap().secret = Some(" rotated password \n".into());
        connections::save_server(&cfg, Some("host"), rotated).unwrap();
        let draft = connection_draft(&editor).unwrap();
        assert!(draft.token.is_empty());
        assert!(draft.ssh.as_ref().unwrap().secret.is_none());
        let tested = resolve_test_credentials(draft.clone(), Some("host"), &cfg).unwrap();
        assert_eq!(tested.token, "rotated-token");
        assert_eq!(
            tested.ssh.unwrap().secret.as_deref(),
            Some(" rotated password \n")
        );
        connections::save_server(&cfg, Some("host"), draft).unwrap();
        let saved = cfg.get("renamed").unwrap();
        assert_eq!(saved.token, "rotated-token");
        assert_eq!(
            saved.ssh.unwrap().secret.as_deref(),
            Some(" rotated password \n")
        );
    }
    #[test]
    fn editor_and_secret_values_preserve_bytes_while_numbers_trim() {
        let raw = "\n  KEY=value  \r\n";
        let form = Form::new(
            FormKind::ProjectCreate,
            "Values",
            vec![
                Field::editor("Environment", raw),
                Field::secret_val("Password", " pass \n"),
                Field::text("Port", " 8080 "),
            ],
        );
        assert_eq!(form.val(0), raw);
        assert_eq!(form.by_label("Environment"), raw);
        assert_eq!(form.val(1), " pass \n");
        assert_eq!(form.by_label("Password"), " pass \n");
        assert_eq!(form.by_label("Port"), "8080");
        let auth = Form::new(
            FormKind::ProjectCreate,
            "Auth",
            vec![
                Field::text("Username", " user "),
                Field::secret_val("Password", " pass \n"),
            ],
        );
        assert_eq!(
            form::basic_auth_body(&auth).unwrap()[0]["password"],
            " pass \n"
        );
    }
    #[test]
    fn cancelled_native_selection_preserves_editor_and_existing_review() {
        let mut state = State::default();
        state.edit(
            Form::new(
                FormKind::ProjectCreate,
                "Env",
                vec![Field::editor("Environment", "draft\n")],
            ),
            Intent::Environment,
        );
        state.confirm(
            "Existing review",
            "original",
            false,
            ConfirmAction::RemoveConnection("host".into()),
        );
        state.confirmation_entry = "keep entry".into();
        state.pending_picker = Some(PickerIntent::Import((
            1,
            "Environment",
            0,
            "draft\n".into(),
        )));
        assert!(state.chosen_path(None).is_none());
        assert!(state.pending_picker.is_none());
        assert!(state.import_generation.is_none());
        assert_eq!(
            state.editor.as_ref().unwrap().form.by_label("Environment"),
            "draft\n"
        );
        assert!(!state.is_dirty());
        assert_eq!(state.confirmation.as_ref().unwrap().target, "original");
        assert_eq!(state.confirmation_entry, "keep entry");
        state.pending_picker = Some(PickerIntent::Export {
            server: Server {
                name: "host".into(),
                url: "https://panel.invalid".into(),
                token: String::new(),
                default: false,
                ssh: None,
            },
            project: "project".into(),
        });
        assert!(state.chosen_path(None).is_none());
        assert!(state.pending_picker.is_none());
        assert_eq!(state.confirmation.as_ref().unwrap().target, "original");
        assert_eq!(state.confirmation_entry, "keep entry");
        assert_eq!(
            state.editor.as_ref().unwrap().form.by_label("Environment"),
            "draft\n"
        );
    }

    #[test]
    fn stale_native_import_selection_never_starts_a_read_for_changed_editor() {
        for change in 0..3 {
            let mut state = State::default();
            state.edit(
                Form::new(
                    FormKind::ProjectCreate,
                    "Env",
                    vec![Field::editor("Environment", "before\n")],
                ),
                Intent::Environment,
            );
            state.pending_picker = Some(PickerIntent::Import((
                1,
                "Environment",
                0,
                "before\n".into(),
            )));
            let edit = state.editor.as_mut().unwrap();
            match change {
                0 => edit.generation += 1,
                1 => edit.revision += 1,
                _ => set(&mut edit.form, "Environment", "new input\n"),
            }
            let before = edit.form.by_label("Environment").to_owned();
            assert!(state.chosen_path(Some("selected.txt".into())).is_none());
            assert!(state.pending_picker.is_none());
            assert!(state.import_generation.is_none());
            assert_eq!(
                state.editor.as_ref().unwrap().form.by_label("Environment"),
                before
            );
            assert!(state.error.is_some());
        }
    }

    #[test]
    fn selected_export_destination_reviews_captured_project_without_writing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chosen.json");
        let mut state = State {
            pending_picker: Some(PickerIntent::Export {
                server: Server {
                    name: "original host".into(),
                    url: "https://original.invalid".into(),
                    token: "private".into(),
                    default: false,
                    ssh: None,
                },
                project: "original project".into(),
            }),
            ..Default::default()
        };
        assert!(state.chosen_path(Some(path.clone())).is_none());
        assert!(state.pending_picker.is_none());
        assert!(!path.exists());
        let review = state.confirmation.take().unwrap();
        assert!(review.target.contains("original host"));
        assert!(review.target.contains("original project"));
        let ConfirmAction::Export {
            path: chosen,
            server,
            project,
        } = review.action
        else {
            panic!("selection must open an export review");
        };
        assert_eq!(chosen, path);
        assert_eq!(server.url, "https://original.invalid");
        assert_eq!(project, "original project");
    }

    #[test]
    fn imports_cannot_replace_newer_input_or_a_different_editor() {
        let form = Form::new(
            FormKind::ProjectCreate,
            "Env",
            vec![Field::editor("Environment", "before\n")],
        );
        let mut editor = make_editor(form, Intent::Environment);
        set(&mut editor.form, "Environment", "new typing\n");
        assert!(!editor.apply_import(1, "Environment", 0, "before\n", "imported\n".into()));
        assert_eq!(editor.form.by_label("Environment"), "new typing\n");
        assert!(!editor.apply_import(2, "Environment", 0, "new typing\n", "other editor\n".into()));
        assert!(editor.apply_import(
            1,
            "Environment",
            0,
            "new typing\n",
            " exact imported \r\n".into()
        ));
        assert_eq!(editor.form.by_label("Environment"), " exact imported \r\n");
    }
    #[test]
    fn partial_setup_retains_source_bytes_and_only_builds_a_retry() {
        let mut form = form::service_create_form(&["original".into()], "original");
        set(&mut form, "Name", "new-service");
        set(&mut form, "Source", "dockerfile");
        let bytes = "\nFROM alpine\nRUN printf ' trailing  '\n\n";
        set(&mut form, "Dockerfile", bytes);
        set(&mut form, "Environment", " x=one \n");
        let mut editor = make_editor(form, Intent::ServiceCreate);
        editor.retain_setup(
            "created-project".into(),
            "created-service".into(),
            "app".into(),
            ServiceSetupPhase::Source,
        );
        assert!(editor.dirty());
        assert_eq!(editor.form.by_label("Dockerfile"), bytes);
        assert_eq!(editor.form.by_label("Environment"), " x=one \n");
        match setup_request(&editor).unwrap() {
            Req::ServiceSetup {
                project,
                service,
                phase,
                source,
                ..
            } => {
                assert_eq!(
                    (project.as_str(), service.as_str()),
                    ("created-project", "created-service")
                );
                assert_eq!(phase, ServiceSetupPhase::Source);
                assert_eq!(source.1["dockerfile"], bytes);
            }
            _ => panic!("must retry setup, never create"),
        }
        set(&mut editor.form, "Source", "github");
        set(&mut editor.form, "Repo", "owner/repo");
        set(&mut editor.form, "Branch", "main");
        editor.retain_setup(
            "created-project".into(),
            "created-service".into(),
            "app".into(),
            ServiceSetupPhase::AutoDeploy,
        );
        assert_eq!(
            editor
                .form
                .visible_here()
                .iter()
                .map(|i| editor.form.fields[*i].label)
                .collect::<Vec<_>>(),
            vec!["Auto deploy"]
        );
        assert!(matches!(
            setup_request(&editor).unwrap(),
            Req::ServiceSetup {
                phase: ServiceSetupPhase::AutoDeploy,
                ..
            }
        ));
    }
    #[test]
    fn whitespace_only_file_contents_are_not_truncated() {
        let mut form = Form::new(
            FormKind::MountCreate {
                project: "p".into(),
                service: "s".into(),
            },
            "File",
            form::mount_fields(None),
        );
        set(&mut form, "Type", "file");
        set(&mut form, "Mount path", "/etc/app.conf");
        set(&mut form, "Content", " \n\t\r\n");
        assert_eq!(mount_values(&form).unwrap()["content"], " \n\t\r\n");
    }
    #[test]
    fn changed_indexed_rows_are_rejected_before_any_mutation() {
        use httpmock::{Method::POST, MockServer};
        for collection in ["mounts", "ports", "redirects"] {
            let server = MockServer::start();
            let path = match collection {
                "mounts" => "/api/rpc/mounts/listMounts",
                "ports" => "/api/rpc/ports/listPorts",
                _ => "/api/rpc/services/app/inspectService",
            };
            let original = json!({"type":"file","mountPath":"/reviewed","content":"before\n"});
            let changed =
                json!({"type":"file","mountPath":"/different","content":"do not touch\n"});
            let list = server.mock(|when, then| {
                when.method(POST).path(path);
                then.status(200).json_body(if collection == "redirects" {
                    json!({"json":{"redirects":[changed]}})
                } else {
                    json!({"json":[changed]})
                });
            });
            let mutation_path = match collection {
                "mounts" => "/api/rpc/mounts/updateMount",
                "ports" => "/api/rpc/ports/deletePort",
                _ => "/api/rpc/services/app/updateRedirects",
            };
            let mutation = server.mock(|when, then| {
                when.method(POST).path(mutation_path);
                then.status(200).json_body(json!({"json":null}));
            });
            let req = match collection {
                "mounts" => Req::MountUpdate {
                    project: "p".into(),
                    service: "s".into(),
                    index: 0,
                    values: json!({"type":"file","mountPath":"/reviewed","content":"new"}),
                },
                "ports" => Req::PortDelete {
                    project: "p".into(),
                    service: "s".into(),
                    index: 0,
                },
                _ => Req::RedirectDelete {
                    project: "p".into(),
                    service: "s".into(),
                    stype: "app".into(),
                    index: 0,
                },
            };
            let emit: crate::worker::Emit = std::sync::Arc::new(|_| {});
            assert!(guarded_operation(
                &crate::client::EasypanelClient::new(&server.base_url(), "fixture"),
                req,
                collection,
                0,
                &original,
                &emit
            )
            .is_err());
            list.assert_hits(1);
            mutation.assert_hits(0);
        }
    }
    #[test]
    fn metrics_filtering_and_project_totals_join_only_actual_service_identities() {
        let mut state = State {
            services: vec![
                json!({"projectName":"p","name":"web","type":"app"}),
                json!({"projectName":"p","name":"db","type":"postgres"}),
            ],
            metrics: vec![
                json!({"projectName":"p","serviceName":"web","cpu":0.5,"memory":1024}),
                json!({"projectName":"p","serviceName":"system-only","cpu":999,"memory":999999}),
            ],
            actions: vec![
                json!({"projectName":"p","serviceName":"web","type":"deployment","status":"pending"}),
            ],
            ..Default::default()
        };
        state.replicas.insert("p_web".into(), (0, 1));
        let rows = service_rows(&state);
        assert!(rows[0].matches(&crate::filter::FilterMatcher::new("deploy.*pending")));
        assert!(rows[1].matches(&crate::filter::FilterMatcher::new("^postgres$")));
        assert!(rows[0].matches(&crate::filter::FilterMatcher::new("^0\\.5%$")));
        let usage = project_usage(&rows);
        assert_eq!(usage["p"].services, 2);
        assert_eq!(usage["p"].sampled, 1);
        assert_eq!(usage["p"].cpu, 0.5);
        assert_eq!(usage["p"].memory, 1024.0);
        state.actions.clear();
        assert!(runtime_label(&state, "p", "web").starts_with("Down"));
    }
    #[test]
    fn source_and_auto_deploy_cells_filter_by_repository_branch_image_and_real_state() {
        let state = State {
            services: vec![
                json!({"projectName":"p","name":"github","type":"app","source":{"type":"github","owner":"owner","repo":"repository","ref":"feature/parity","autoDeploy":true,"password":"never-display"}}),
                json!({"projectName":"p","name":"image","type":"app","source":{"type":"image","image":"registry.example/team/image:v2","username":"private-user","password":"private-password"}}),
                json!({"projectName":"p","name":"git","type":"app","source":{"type":"git","repo":"https://user:secret@git.example/team/repo.git?token=hidden","ref":"release"}}),
                json!({"projectName":"p","name":"off","type":"app","source":{"type":"github","owner":"owner","repo":"off","ref":"main","autoDeploy":false}}),
                json!({"projectName":"p","name":"unknown","type":"app","source":{"type":"github","owner":"owner","repo":"unknown","ref":"main"}}),
            ],
            ..Default::default()
        };
        let rows = service_rows(&state);
        assert_eq!(rows[0].source, "owner/repository#feature/parity");
        assert_eq!(rows[0].auto_deploy, "Enabled");
        assert!(rows[0].matches(&crate::filter::FilterMatcher::new("repository#feature/.*")));
        assert!(rows[0].matches(&crate::filter::FilterMatcher::new("^enabled$")));
        assert_eq!(rows[1].source, "registry.example/team/image:v2");
        assert_eq!(rows[1].auto_deploy, "Not applicable");
        assert!(rows[1].matches(&crate::filter::FilterMatcher::new("image:v2")));
        assert_eq!(rows[2].source, "https://git.example/team/repo.git#release");
        assert_eq!(rows[3].auto_deploy, "Disabled");
        assert_eq!(rows[4].auto_deploy, "Unknown");
        for row in rows {
            assert!(!row.source.contains("private-password"));
            assert!(!row.source.contains("never-display"));
            assert!(!row.source.contains("secret"));
            assert!(!row.source.contains("hidden"));
        }
    }
    #[test]
    fn service_capabilities_do_not_advertise_unsupported_data_operations() {
        assert!(!data_available("database", "redis"));
        assert!(data_available("db", "redis"));
        assert!(!data_available("dumps", "postgres"));
        assert!(!data_available("copy", "mongo"));
        assert!(data_available("restore-from", "postgres"));
        assert!(data_available("copy", "mariadb"));
        let mut form = form::service_create_form(&["p".into()], "p");
        set(&mut form, "Name", "box-service");
        set(&mut form, "Kind", "box");
        assert!(
            matches!(form::service_create_request(&form).unwrap(),Req::ServiceCreate{stype,..} if stype=="box")
        );
    }
    #[test]
    fn bulk_deploy_results_say_dispatched_not_completed() {
        let mut state = State::default();
        result_message(
            &mut state,
            Resp::BulkDone {
                action: "deploy".into(),
                ok: vec!["alpha/web".into()],
                failed: vec![],
            },
        );
        assert!(state.outcome.iter().any(|s| s == "Dispatched: alpha/web"));
        assert!(!state.outcome.iter().any(|s| s.contains("Completed")));
    }
    #[test]
    fn explicitly_confirmed_export_replaces_atomically_and_failure_preserves_existing_items() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("project.json");
        std::fs::write(&path, b"old export").unwrap();
        atomic_export(&path, b"new export\n").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new export\n");
        let directory = dir.path().join("directory");
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join("keep"), b"original").unwrap();
        assert!(atomic_export(&directory, b"replacement").is_err());
        assert_eq!(std::fs::read(directory.join("keep")).unwrap(), b"original");
    }
    #[test]
    fn invalidation_waits_for_old_busy_reads_then_fetches_a_fresh_inventory() {
        use super::super::backend::{Backend, Runtime};
        use httpmock::{Method::POST, MockServer};
        let server = MockServer::start();
        let fresh=server.mock(|when,then|{when.method(POST).path("/api/rpc/projects/listProjectsAndServices");then.status(200).json_body(json!({"json":{"projects":[{"name":"fresh"}],"services":[{"projectName":"fresh","name":"web","type":"app"}]}}));});
        server.mock(|when, then| {
            when.method(POST)
                .path("/api/rpc/monitorOld/getDockerTaskStats");
            then.status(200).json_body(json!({"json":[]}));
        });
        let target = Target {
            server: Some(Server {
                name: "fixture".into(),
                url: server.base_url(),
                token: "fixture-token".into(),
                default: false,
                ssh: None,
            }),
            ..Default::default()
        };
        let mut backend = Backend::new(egui::Context::default(), Runtime::default());
        let mut opened = Vec::new();
        let mut reload = false;
        let path = std::path::Path::new("unused-fixture-path");
        let mut cx = PageContext {
            target: &target,
            servers: &[],
            accounts: &[],
            server_config: path,
            cloudflare_config: path,
            watchlist: path,
            doc: 1,
            backend: &mut backend,
            opened: &mut opened,
            reload: &mut reload,
        };
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        cx.background("inventory", "old inventory", move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            Ok(Payload::Worker(Resp::AllServices {
                projects: vec!["stale".into()],
                services: vec![],
            }))
        });
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let mut state = State::default();
        refresh(&mut state, &mut cx);
        flush_reads(&mut state, &mut cx);
        assert!(cx.busy("inventory"));
        assert!(state.invalidated.contains("inventory"));
        release_tx.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while state.projects != vec!["fresh".to_string()] && Instant::now() < deadline {
            cx.backend.pump();
            pump(&mut state, &mut cx);
            flush_reads(&mut state, &mut cx);
            assert!(!state.projects.iter().any(|p| p == "stale"));
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(state.projects, vec!["fresh"]);
        fresh.assert_hits(1);
    }
}
