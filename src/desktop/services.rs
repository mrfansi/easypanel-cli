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
    import_generation: Option<(u64, &'static str, u64, String)>,
    generation: u64,
    saving_generation: Option<u64>,
    invalidated: BTreeSet<String>,
    actions: Vec<Value>,
    last_poll: Option<Instant>,
    runtime_updated: Option<Instant>,
    pending_mount: Option<(usize, Value)>,
    metrics: Vec<Value>,
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
    Export(std::path::PathBuf),
    RemoveConnection(String),
}
impl State {
    pub(super) fn is_dirty(&self) -> bool {
        self.editor.as_ref().is_some_and(Editor::dirty)
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
        self.confirmation_entry.clear();
        self.confirmation = Some(Confirmation {
            title: title.into(),
            target: target.into(),
            typed,
            action,
        });
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
    if !cx.target.project.is_empty() {
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
    if let Some(r) = cx.take("inventory") {
        if !state.invalidated.contains("inventory") {
            match r {
                Ok(Payload::Worker(Resp::AllServices { projects, services })) => {
                    state.projects = projects;
                    state.services = services;
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
        if !state.invalidated.contains("detail") {
            match r {
                Ok(Payload::Json(v)) => {
                    state.detail = Some(v);
                    if let Some(intent) = state.pending_editor.take() {
                        begin_edit(state, cx, intent);
                    }
                }
                Err(e) => {
                    state.error = Some(e);
                    state.pending_editor = None;
                }
                _ => {}
            }
        }
    }
    for key in ["ports", "mounts"] {
        if let Some(r) = cx.take(key) {
            if !state.invalidated.contains(key) {
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
        if !state.invalidated.contains("domains") {
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
    confirmation(ui, state, cx);
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
                            !targets.is_empty() && !cx.busy("mutation"),
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
        .max_height(height)
        .show(ui, |ui| {
            let widths = [32.0, 156.0, 116.0, 64.0, 90.0, 104.0, 72.0, 160.0, 84.0];
            let width: f32 = widths.iter().sum();
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                for (label, width) in [
                    "Select",
                    "Service",
                    "Runtime",
                    "CPU",
                    "Memory",
                    "Project",
                    "Type",
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
                                    row.cpu_label.as_str(),
                                    row.memory_label.as_str(),
                                    id.0.as_str(),
                                    id.2.as_str(),
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
fn service(ui: &mut egui::Ui, state: &mut State, cx: &mut PageContext<'_>) {
    let kind = cx.target.service_type.clone();
    let id = (
        cx.target.project.clone(),
        cx.target.service.clone(),
        kind.clone(),
    );
    ui.horizontal(|ui| {
        if crate::lifecycle::ops(&kind, "deploy").is_some()
            && ui
                .add_enabled(
                    !cx.busy("mutation"),
                    egui::Button::new("Deploy").fill(ui.visuals().selection.bg_fill),
                )
                .clicked()
        {
            lifecycle(state, id.clone(), "Deploy", "deploy", false);
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
                            .add_enabled(!cx.busy("mutation"), egui::Button::new(label))
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
        ui.add(egui::Label::new(runtime_label(state, &id.0, &id.1)).truncate())
            .on_hover_text(state.runtime_updated.map_or_else(
                || "Runtime status has not been refreshed yet".into(),
                |updated| format!("Runtime updated {} s ago", updated.elapsed().as_secs()),
            ));
    });
    let mut sections = vec![("overview", "Overview"), ("environment", "Environment")];
    if kind == "app" {
        sections.extend([
            ("source", "Source"),
            ("build", "Build"),
            ("deploy", "Deployment"),
        ]);
    }
    if crate::lifecycle::has_resource_limits(&kind) {
        sections.push(("resources", "Resources"));
    }
    if crate::lifecycle::has_config_file(&kind) {
        sections.push(("configuration", "Config file"));
    }
    if crate::lifecycle::has_mounts_and_ports(&kind) {
        sections.extend([("ports", "Ports"), ("mounts", "Mounts")]);
    }
    if crate::lifecycle::is_web(&kind) {
        sections.extend([
            ("domains", "Domains"),
            ("redirects", "Redirects"),
            ("auth", "Basic auth"),
        ]);
    }
    sections.push(("backups", "Backups"));
    ui.horizontal(|ui| {
        ui.label("Section");
        egui::ComboBox::from_id_salt("service-section")
            .selected_text(state.section_label(cx.target))
            .width(180.0)
            .show_ui(ui, |ui| {
                for (key, label) in &sections {
                    if ui.selectable_label(state.section == *key, *label).clicked()
                        && state.section != *key
                    {
                        open(cx, PageKind::Service, key, None);
                        ui.close();
                    }
                }
            });
        if ui
            .add_enabled(
                state.editor.is_none() && !cx.busy("detail"),
                egui::Button::new("Refresh"),
            )
            .clicked()
        {
            refresh(state, cx);
        }
    });
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
                        if section == "environment"
                            && crate::operations::HAS_UPDATE_ENV.contains(&kind.as_str())
                        {
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
    let f=match &intent{
        Intent::Environment=>Form::new(FormKind::ProjectCreate,"Environment",vec![Field::editor("Environment",if s.is_empty(){d.pointer("/project/env").and_then(Value::as_str).unwrap_or("")}else{d.get("env").and_then(Value::as_str).unwrap_or("")})]).with_note("Values may contain secrets. Save replaces the complete environment; deploy or restart to apply."),
        Intent::Configuration=>Form::new(FormKind::ProjectCreate,"Database / advanced configuration",vec![Field::editor("Config file",d.get("configFile").and_then(Value::as_str).unwrap_or(""))]).with_note("Restart after saving to apply. Image, command and environment are preserved."),
        Intent::Source=>Form::new(FormKind::SourceEdit{project:p,service:s},"Source",form::source_fields(d.get("source"),Vec::new())),
        Intent::Build=>Form::new(FormKind::BuildEdit{project:p,service:s},"Build",form::build_fields(d.get("build"))).with_original(d.get("build").cloned().unwrap_or(Value::Null)),
        Intent::Resources=>Form::new(FormKind::ResourceEdit{project:p,service:s,stype:k},"Resources",form::resource_fields(d.get("resources"))).with_note("CPU in cores, memory in MB. 0 = unlimited."),
        Intent::Deploy=>Form::new(FormKind::DeployEdit{project:p,service:s},"Deployment",form::deploy_fields(d.get("deploy"))).with_original(d.get("deploy").cloned().unwrap_or_else(||json!({}))),
        Intent::Port=>Form::new(FormKind::PortCreate{project:p,service:s},"Add port",form::port_fields()),
        Intent::Mount(None)=>Form::new(FormKind::MountCreate{project:p,service:s},"Add mount",form::mount_fields(None)),
        Intent::Redirect=>Form::new(FormKind::RedirectCreate{project:p,service:s,stype:k},"Add redirect",form::redirect_fields()),
        Intent::Auth=>Form::new(FormKind::BasicAuthEdit{project:p,service:s,stype:k},"Basic authentication",form::basic_auth_fields(Some(&d))).with_original(d.get("basicAuth").cloned().unwrap_or_else(||json!([]))).with_note("Clear both fields to remove the first credential. Other credentials are preserved."),
        Intent::Domain(None)=>{let mut f=Form::new(FormKind::DomainCreate,"Add service domain",form::domain_fields(None,&state.projects));for(label,value)in[("Project",p),("Service",s)]{if let Some(field)=f.fields.iter_mut().find(|f|f.label==label){field.value=value;}}f},
        _=>return,
    };
    state.edit(f, intent);
}

fn editor(ui: &mut egui::Ui, state: &mut State, cx: &mut PageContext<'_>) {
    let Some(edit) = state.editor.as_mut() else {
        return;
    };
    ui.horizontal(|ui| {
        let width = (ui.available_width() - if edit.dirty() { 132.0 } else { 0.0 }).max(80.0);
        ui.add_sized(
            [width, 20.0],
            egui::Label::new(egui::RichText::new(&edit.form.title).strong()).truncate(),
        )
        .on_hover_text(&edit.form.title);
        if edit.dirty() {
            ui.label(egui::RichText::new("Unsaved changes").color(ui.visuals().warn_fg_color));
        }
    });
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
            }else if ui.add_enabled(edit.dirty()||matches!(edit.intent,Intent::Connection(None)|Intent::ProjectCreate|Intent::ServiceCreate|Intent::Port|Intent::Mount(None)|Intent::Redirect|Intent::Domain(None)),egui::Button::new(action_label)).clicked(){save=true;}
            if ui.button("Discard").clicked(){if edit.dirty(){edit.discard=true;}else{discard=true;}}
            if edit.form.visible_here().iter().any(|&i|matches!(edit.form.fields[i].kind,FieldKind::Editor))&&ui.add_enabled(!cx.busy("import"),egui::Button::new("Import text file…")).clicked(){import=true;}
            if matches!(edit.intent,Intent::Connection(_))&&ui.add_enabled(!cx.busy("connection-test"),egui::Button::new("Test connection")).clicked(){test=true;}
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
    if import {
        if let Some(label) = edit.form.visible_here().into_iter().find_map(|i| {
            matches!(edit.form.fields[i].kind, FieldKind::Editor)
                .then_some(edit.form.fields[i].label)
        }) {
            if let Some(path) = rfd::FileDialog::new()
                .set_title("Import text into editor")
                .pick_file()
            {
                let before = edit
                    .form
                    .fields
                    .iter()
                    .find(|f| f.label == label)
                    .map(|f| f.value.clone())
                    .unwrap_or_default();
                state.import_generation = Some((edit.generation, label, edit.revision, before));
                cx.background("import", "Read selected text file", move || {
                    Ok(Payload::Text(std::fs::read_to_string(path)?))
                });
            }
        }
    }
    if test {
        match connection_draft(edit) {
            Ok(server) => {
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
    if save {
        if let Intent::Connection(original) = &edit.intent {
            match connection_draft(edit) {
                Ok(server) => {
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
    ui.separator();
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
            apply = widgets::confirm(
                ui,
                &confirm.title,
                &confirm.target,
                typed,
                &mut state.confirmation_entry,
            );
            if ui.button("Cancel").clicked() {
                cancel = true;
            }
        },
    );
    if apply {
        if let Some(confirm) = state.confirmation.take() {
            match confirm.action {
                ConfirmAction::Request(req) => cx.request("mutation", req),
                ConfirmAction::Indexed {
                    req,
                    collection,
                    index,
                    original,
                } => guarded_request(cx, req, collection, index, original),
                ConfirmAction::Export(path) => write_export(cx, path),
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
    if cx.busy("export") {
        return;
    }
    let project = &cx.target.project;
    if let Some(path) = rfd::FileDialog::new()
        .set_title("Export redacted project configuration")
        .set_file_name(format!("{project}.easypanel.json"))
        .add_filter("JSON configuration", &["json"])
        .save_file()
    {
        state.confirm(
            "Export configuration — replaces the destination file if it exists",
            path.display().to_string(),
            false,
            ConfirmAction::Export(path),
        );
    }
}
fn write_export(cx: &mut PageContext<'_>, path: std::path::PathBuf) {
    let Some(server) = cx.target.server.clone() else {
        return;
    };
    let project = cx.target.project.clone();
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
    confirmation(ui, state, cx);
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
    #[test]
    fn multiline_service_editor_keeps_actions_and_help_above_the_dock() {
        use super::super::backend::Backend;
        for size in [egui::vec2(1024.0, 680.0), egui::vec2(1280.0, 800.0)] {
            for raw in ["".to_owned(), "KEY=  preserved \r\nNEXT=value\n".to_owned(),
                format!("LONG={}\n{}", "x".repeat(2000), "KEY=value\n".repeat(100))]
            {
                let ctx = egui::Context::default();
                super::super::theme::apply(&ctx, super::super::theme::Mode::Dark);
                let mut backend = Backend::new(ctx.clone(), super::super::Runtime::default());
                let target = Target {
                    project: "project".into(), service: "service".into(),
                    service_type: "app".into(), section: "environment".into(),
                    ..Default::default()
                };
                let mut state = State { section: "environment".into(), ..Default::default() };
                state.editor = Some(make_editor(Form::new(
                    FormKind::ProjectCreate, "Environment",
                    vec![Field::editor("Environment", "original")],
                ).with_note("Values may contain secrets. Save replaces the complete environment; deploy or restart to apply."), Intent::Environment));
                state.editor.as_mut().unwrap().form.fields[0].value = raw.clone();
                let mut opened = Vec::new();
                let mut reload = false;
                let mut cx = PageContext {
                    target: &target, servers: &[], accounts: &[],
                    server_config: std::path::Path::new(""), cloudflare_config: std::path::Path::new(""),
                    watchlist: std::path::Path::new(""), doc: 1,
                    backend: &mut backend, opened: &mut opened, reload: &mut reload,
                };
                let mut measured = None;
                let mut output = ctx.run_ui(egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                    ..Default::default()
                }, |ui| {
                    egui::Panel::top("menubar").exact_size(32.0).show(ui, |_| {});
                    egui::Panel::bottom("statusbar").exact_size(24.0).show(ui, |_| {});
                    egui::Panel::left("activitybar").exact_size(46.0).show(ui, |_| {});
                    egui::Panel::left("explorer").exact_size(252.0).show(ui, |_| {});
                    egui::Panel::bottom("utility").exact_size(220.0).show(ui, |_| {});
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
                });
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
                if !raw.is_empty() {
                    let (clip, text) = output.shapes.iter().find_map(|clipped| match &clipped.shape {
                        egui::epaint::Shape::Text(shape) if shape.galley.text() == raw =>
                            Some((clipped.clip_rect, shape)),
                        _ => None,
                    }).expect("multiline editor must paint its content");
                    assert!(clip.height() > 80.0,
                        "editor has no useful line box: size={size:?}, raw_bytes={}, clip={clip:?}, text_rect={:?}", raw.len(), text.galley.rect);
                    if raw.lines().count() > 3 {
                        assert!(text.galley.rect.height() > clip.height(), "long content must scroll internally");
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
