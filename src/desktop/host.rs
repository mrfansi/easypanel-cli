use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use eframe::egui;
use serde_json::{json, Value};

use super::model::{OpenPage, PageContext, PageKind, Payload, Target};
use super::widgets;
use crate::client::EasypanelClient;
use crate::form::{self, Field, Form, FormKind};
use crate::output::{field, format_bytes, format_rate};
use crate::worker::{Req, Resp};

#[derive(Default)]
struct Snapshot {
    value: Option<Value>,
    error: Option<String>,
    fetched: Option<Instant>,
    attempted: Option<Instant>,
    requested: bool,
    input: Option<Value>,
}

struct Editor {
    kind: EditKind,
    form: Form,
    initial: Vec<String>,
    discard: bool,
}

enum EditKind {
    Domain(Option<String>),
    Check(String),
    Schedule {
        id: Option<String>,
        project: String,
        service: String,
    },
}

impl Editor {
    fn new(kind: EditKind, form: Form) -> Self {
        let initial = form.fields.iter().map(|f| f.value.clone()).collect();
        Self {
            kind,
            form,
            initial,
            discard: false,
        }
    }

    fn dirty(&self) -> bool {
        self.form
            .fields
            .iter()
            .map(|f| &f.value)
            .ne(self.initial.iter())
    }

    fn cancel(&mut self) -> bool {
        self.discard = self.dirty();
        !self.discard
    }
}

enum Apply {
    Rpc {
        group: &'static str,
        op: &'static str,
        body: Value,
    },
    Worker(Box<Req>),
    KillAction(String),
    BackupMany {
        project: String,
        service: String,
        databases: Vec<String>,
        provider: String,
    },
    Shell,
}

struct Review {
    title: String,
    target: String,
    typed: String,
    entry: String,
    apply: Apply,
}

#[derive(Default)]
pub(super) struct State {
    section: String,
    snapshots: HashMap<String, Snapshot>,
    selected: HashMap<String, Option<usize>>,
    editor: Option<Editor>,
    editor_request: Option<&'static str>,
    review: Option<Review>,
    message: String,
    message_section: &'static str,
    message_partial: bool,
    mutation_section: &'static str,
    mutation_error: String,
    filter: String,
    metrics_filter: String,
    storage_filter: String,
    action_filter: String,
    marked: HashSet<String>,
    bulk_target: String,
    bulk_find: String,
    bulk_replace: String,
    bulk_plan: Vec<crate::domains::Change>,
    bulk_error: String,
    action_project: String,
    action_service: String,
    action_type: String,
    action_status: String,
    action_limit: u32,
    action_detail: Option<String>,
    follow_actions: bool,
    watch: Vec<crate::uptime::Check>,
    probes: Vec<crate::uptime::Probe>,
    watch_loaded: bool,
    watch_requested: bool,
    check_inputs: Vec<crate::uptime::Check>,
    watch_url: String,
    reveal_check_secrets: bool,
    backup_project: String,
    backup_service: String,
    backup_type: String,
    backup_database: String,
    backup_provider: String,
    backup_source: String,
    history_sources: HashMap<String, crate::config::Server>,
    history_pending_sources: HashMap<String, crate::config::Server>,
    database_names: Vec<String>,
    backup_marked: HashSet<String>,
    backup_jobs: Vec<(String, String)>,
    backup_outcomes: Vec<Vec<String>>,
    backup_volume: bool,
    history_mode: bool,
    fleet_started: HashSet<String>,
    fleet_selection: Option<usize>,
}

impl State {
    pub(super) fn is_dirty(&self) -> bool {
        self.editor.as_ref().is_some_and(Editor::dirty)
            || !self.bulk_plan.is_empty()
            || !self.bulk_find.is_empty()
            || !self.bulk_replace.is_empty()
    }
    pub(super) fn section_label(&self, target: &Target) -> &'static str {
        if target.server.is_none() {
            return "Fleet";
        }
        let section = if self.section.is_empty() {
            target.section.as_str()
        } else {
            self.section.as_str()
        };
        SECTIONS
            .iter()
            .find(|(id, _)| *id == section)
            .map_or("System", |(_, label)| *label)
    }

    pub(super) fn breadcrumb(&self, target: &Target) -> String {
        match &target.server {
            Some(server) => format!("{} / {}", server.name, self.section_label(target)),
            None => "Fleet".into(),
        }
    }

    fn clear_success_notice(&mut self) {
        if !self.message_partial {
            self.message.clear();
        }
    }

    fn notice(&mut self, ui: &mut egui::Ui, target: &Target, allow_success: bool) {
        if self.message.is_empty()
            || (!self.message_partial
                && (!allow_success || self.message_section != self.section_label(target)))
        {
            return;
        }
        ui.horizontal_wrapped(|ui| {
            ui.label(format!(
                "{} outcome: {}",
                self.message_section, self.message
            ));
            if ui.small_button("Dismiss notice").clicked() {
                self.message.clear();
                self.message_partial = false;
            }
        });
    }

    fn rpc(
        &mut self,
        cx: &mut PageContext<'_>,
        key: &str,
        group: &str,
        op: &str,
        body: Value,
        refresh: bool,
    ) {
        self.fetch(cx, key, (group, op, body), refresh, false);
    }

    fn poll(&mut self, cx: &mut PageContext<'_>, key: &str, group: &str, op: &str, body: Value) {
        self.fetch(cx, key, (group, op, body), true, true);
    }

    fn fetch(
        &mut self,
        cx: &mut PageContext<'_>,
        key: &str,
        call: (&str, &str, Value),
        refresh: bool,
        polling: bool,
    ) {
        let (group, op, body) = call;
        let slot = self.snapshots.entry(key.into()).or_default();
        let changed = slot.input.as_ref() != Some(&body);
        if !cx.busy(key) && (!slot.requested || changed || refresh) {
            slot.requested = true;
            slot.input = Some(body.clone());
            slot.attempted = Some(Instant::now());
            if changed {
                self.selected.remove(key);
            }
            if polling {
                cx.poll_rpc(key, group, op, body);
            } else {
                cx.rpc(key, group, op, body);
            }
        }
    }

    fn value(&self, key: &str) -> Option<&Value> {
        self.snapshots.get(key).and_then(|s| s.value.as_ref())
    }

    fn rows(&self, key: &str) -> Vec<Value> {
        self.value(key)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }

    fn status(&self, ui: &mut egui::Ui, cx: &PageContext<'_>, key: &str, resource: &str) {
        if cx.busy(key) {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(if self.value(key).is_some() {
                    "Refreshing; showing last successful snapshot"
                } else {
                    "Loading…"
                });
            });
        }
        if let Some(s) = self.snapshots.get(key) {
            if let Some(error) = &s.error {
                widgets::error(ui, error);
                if s.value.is_some() {
                    ui.label("Stale data retained. Refresh before acting.");
                }
            }
            if let Some(at) = s.fetched {
                ui.weak(format!(
                    "{resource}: received {}s ago",
                    at.elapsed().as_secs()
                ));
            }
        }
    }

    fn choose(&mut self, key: &str, len: usize) -> Option<usize> {
        self.selected
            .get(key)
            .copied()
            .flatten()
            .filter(|&i| i < len)
    }

    fn review(&mut self, title: &str, target: String, typed: String, apply: Apply) {
        self.clear_success_notice();
        self.review = Some(Review {
            title: title.into(),
            target,
            typed,
            entry: String::new(),
            apply,
        });
    }

    fn invalidate(&mut self) {
        for slot in self.snapshots.values_mut() {
            slot.requested = false;
        }
    }

    fn apply_probes(&mut self, probes: Vec<crate::uptime::Probe>) {
        self.selected.remove("uptime");
        self.probes = probes
            .into_iter()
            .filter(|probe| {
                self.check_inputs
                    .iter()
                    .any(|before| before.url == probe.url && self.watch.contains(before))
            })
            .collect();
    }

    fn replace_watch(&mut self, watch: Vec<crate::uptime::Check>) {
        self.selected.remove("uptime");
        self.watch = watch;
        self.watch_loaded = true;
        self.probes.clear();
    }

    fn pump(&mut self, cx: &mut PageContext<'_>) {
        let keys: Vec<_> = self.snapshots.keys().cloned().collect();
        for key in keys {
            if let Some(result) = cx.take(&key) {
                let slot = self.snapshots.get_mut(&key).unwrap();
                match result {
                    Ok(Payload::Json(v)) => {
                        if slot.value.as_ref() != Some(&v) {
                            self.selected.remove(&key);
                        }
                        slot.value = Some(v);
                        slot.fetched = Some(Instant::now());
                        slot.error = None;
                        if let Some(source) = self.history_pending_sources.remove(&key) {
                            self.history_sources.insert(key.clone(), source);
                        }
                    }
                    Ok(_) => {
                        slot.error = Some("Unexpected response type; refresh this view".into())
                    }
                    Err(e) => {
                        slot.error = Some(e);
                        self.history_pending_sources.remove(&key);
                    }
                }
            }
        }
        if let Some(result) = cx.take("host-mutation") {
            let saved_editor = self.editor_request == Some("host-mutation");
            if saved_editor {
                self.editor_request = None;
            }
            self.mutation_error.clear();
            self.message.clear();
            self.message_partial = false;
            self.message_section = self.mutation_section;
            match result {
                Err(e) => self.mutation_error = format!("{e}\nA timed-out operation may still be running remotely. Refresh history before retrying; dismissing this review does not cancel it."),
                Ok(Payload::Worker(Resp::Err(e))) => self.mutation_error = format!("{e}\nFor a timeout or gateway error, the remote outcome is unknown. Refresh Actions before retrying. Dismissing this error does not cancel remote work."),
                Ok(Payload::Worker(Resp::DomainsEdited { ok, failed }))
                | Ok(Payload::Worker(Resp::DomainsDeleted { ok, failed })) => {
                    self.message = format!("{ok} domain operations completed");
                    self.message_partial = !failed.is_empty();
                    for (name, error) in failed { self.mutation_error.push_str(&format!("{name}: {error}\n")); }
                    self.bulk_plan.clear(); self.marked.clear(); self.invalidate();
                }
                Ok(Payload::Worker(Resp::Done(message, _))) => { self.message = message; self.invalidate(); if saved_editor { self.editor = None; } }
                Ok(Payload::Worker(Resp::Notes { msg, notes, .. })) => { self.message_partial = !notes.is_empty(); self.message = format!("{msg}\n{}", notes.join("\n")); self.invalidate(); }
                Ok(_) => { self.message = "Request accepted. Refresh history to inspect remote completion.".into(); self.invalidate(); if saved_editor { self.editor = None; } }
            }
        }
        if let Some(result) = cx.take("watch-load") {
            match result {
                Ok(Payload::Json(v)) => match serde_json::from_value(v) {
                    Ok(watch) => self.replace_watch(watch),
                    Err(e) => self.mutation_error = e.to_string(),
                },
                Err(e) => self.mutation_error = e,
                _ => self.mutation_error = "Unexpected watchlist response".into(),
            }
        }
        if let Some(result) = cx.take("watch-save") {
            let saved_editor = self.editor_request == Some("watch-save");
            if saved_editor {
                self.editor_request = None;
            }
            self.clear_success_notice();
            match result {
                Ok(Payload::Json(v)) => match serde_json::from_value(v) {
                    Ok(watch) => {
                        self.replace_watch(watch);
                        if saved_editor {
                            self.editor = None;
                        }
                        if !self.message_partial {
                            self.message_section = "Uptime";
                            self.message = "Watchlist saved; previous answers cleared because the request changed".into();
                        }
                    }
                    Err(e) => self.mutation_error = e.to_string(),
                },
                Err(e) => self.mutation_error = e,
                _ => self.mutation_error = "Unexpected watchlist response".into(),
            }
        }
        if let Some(result) = cx.take("checks") {
            match result {
                Ok(Payload::Worker(Resp::Checked(probes))) => self.apply_probes(probes),
                Ok(Payload::Worker(Resp::Err(e))) | Err(e) => self.mutation_error = e,
                _ => self.mutation_error = "Unexpected check response".into(),
            }
        }
        if let Some(result) = cx.take("database-names") {
            match result {
                Ok(Payload::Worker(Resp::DatabasesIn {
                    project,
                    service,
                    names,
                })) if project == self.backup_project && service == self.backup_service => {
                    self.database_names = names
                }
                Ok(Payload::Worker(Resp::Err(e))) | Err(e) => self.mutation_error = e,
                _ => {}
            }
        }
        let jobs = self.backup_jobs.clone();
        for (key, name) in jobs {
            if let Some(result) = cx.take(&key) {
                let (status, message) = match result {
                    Ok(Payload::Worker(Resp::Done(message, _))) => ("Accepted", message),
                    Ok(Payload::Worker(Resp::Notes { msg, notes, .. })) => ("Accepted with notes", format!("{msg}\n{}", notes.join("\n"))),
                    Ok(Payload::Worker(Resp::Err(e))) => ("Request error; inspect Actions", format!("{e}\nA timeout or gateway failure does not prove the remote backup stopped.")),
                    Err(e) => ("Unknown remote outcome", e),
                    _ => ("Error", "Unexpected backup response; inspect action history before retrying".into()),
                };
                self.backup_outcomes
                    .push(vec![name, status.into(), message]);
                self.backup_jobs.retain(|(pending, _)| pending != &key);
                self.invalidate();
            }
        }
    }
}

pub(super) fn show_fleet(ui: &mut egui::Ui, state: &mut State, cx: &mut PageContext<'_>) {
    state.pump(cx);
    let refresh = ui
        .horizontal(|ui| {
            ui.heading("Fleet");
            let refresh = ui.button("Refresh all hosts").clicked();
            ui.weak(
                "Each host reports independently; an unreachable host cannot block the others.",
            );
            refresh
        })
        .inner;
    for server in cx.servers {
        let key = format!("fleet:{}", server.name);
        if (refresh || state.fleet_started.insert(key.clone())) && !cx.busy(&key) {
            state.snapshots.entry(key.clone()).or_default();
            let server = server.clone();
            cx.background(&key, "Checking host health", move || {
                EasypanelClient::for_server(&server)
                    .call("metrics", "getSystemStats", json!({}))
                    .map(Payload::Json)
                    .map_err(|e| anyhow::anyhow!(redact_host(&e.to_string(), &server)))
            });
        }
    }
    let rows: Vec<_> = cx
        .servers
        .iter()
        .map(|server| {
            let key = format!("fleet:{}", server.name);
            let slot = state.snapshots.get(&key);
            let v = slot.and_then(|s| s.value.as_ref());
            let status = if cx.busy(&key) {
                "Checking"
            } else if slot.is_some_and(|s| s.error.is_some()) {
                "Unreachable / error"
            } else if v.is_some() {
                "Panel reachable"
            } else {
                "Unknown"
            };
            vec![
                server.name.clone(),
                status.into(),
                metric(v, "cpu", true),
                metric(v, "memory", true),
                metric(v, "disk", true),
                slot.and_then(|s| s.fetched)
                    .map(|at| format!("{}s ago", at.elapsed().as_secs()))
                    .unwrap_or_else(|| "Never".into()),
                slot.and_then(|s| s.error.clone()).unwrap_or_default(),
            ]
        })
        .collect();
    widgets::table(
        ui,
        "fleet-hosts",
        &[
            "Host", "Health", "CPU", "Memory", "Disk", "Received", "Reason",
        ],
        &rows,
        &mut state.fleet_selection,
        100.0,
    );
    if let Some(server) = state.fleet_selection.and_then(|i| cx.servers.get(i)) {
        let server = server.clone();
        ui.horizontal(|ui| {
            if ui.button("Open host workspace").clicked() {
                let mut target = cx.target.clone();
                target.server = Some(server.clone());
                target.project.clear();
                target.service.clear();
                target.section = "overview".into();
                cx.open(OpenPage {
                    kind: PageKind::Host,
                    target,
                    title: server.name.clone(),
                });
            }
            if ui.button("Privileged host terminal…").clicked() {
                let mut target = cx.target.clone();
                target.server = Some(server.clone());
                target.project.clear();
                target.service.clear();
                target.section = "host".into();
                cx.open(OpenPage {
                    kind: PageKind::Terminal,
                    target,
                    title: format!("Host shell · {}", server.name),
                });
            }
        });
    }
    if rows.is_empty() {
        ui.label("No configured hosts. Add a connection in Connections to begin.");
    }
    ui.ctx().request_repaint_after(Duration::from_millis(250));
}

const SECTIONS: &[(&str, &str)] = &[
    ("overview", "System"),
    ("metrics", "Services"),
    ("storage", "Storage"),
    ("nodes", "Nodes"),
    ("domains", "Domains"),
    ("certificates", "Certificates"),
    ("notifications", "Notifications"),
    ("actions", "Actions"),
    ("uptime", "Uptime"),
    ("maintenance", "Maintenance"),
    ("backups", "Backups"),
];

pub(super) fn show(ui: &mut egui::Ui, state: &mut State, cx: &mut PageContext<'_>) {
    state.pump(cx);
    if cx.target.server.is_none() {
        ui.label("Choose a host from the fleet or Connections.");
        return;
    }
    if state.section.is_empty() {
        state.section = if SECTIONS.iter().any(|(id, _)| *id == cx.target.section) {
            cx.target.section.clone()
        } else {
            "overview".into()
        };
        state.backup_project = cx.target.project.clone();
        state.backup_service = cx.target.service.clone();
        state.backup_type = cx.target.service_type.clone();
    }
    load_watch(state, cx);
    ui.ctx().request_repaint_after(Duration::from_millis(500));
    // Editors own the document viewport, never the space left below a resource table.
    if state.review.is_some() {
        review(ui, state, cx);
        return;
    }
    if state.editor.is_some() {
        editor(ui, state, cx);
        return;
    }
    ui.horizontal_wrapped(|ui| {
        for &(id, title) in SECTIONS {
            if ui
                .selectable_value(&mut state.section, id.to_string(), title)
                .changed()
            {
                state.clear_success_notice();
            }
        }
    });
    ui.separator();
    state.notice(ui, cx.target, true);
    if !state.mutation_error.is_empty() {
        widgets::error(ui, &state.mutation_error);
        if ui
            .small_button("Dismiss error (does not cancel work)")
            .clicked()
        {
            state.mutation_error.clear();
        }
    }
    if cx.busy("host-mutation") {
        ui.horizontal_wrapped(|ui| {
            ui.spinner();
            ui.label(cx.progress("host-mutation").unwrap_or(
                "Waiting for the panel; remote work can continue if this document is closed.",
            ));
        });
    }
    egui::ScrollArea::vertical()
        .id_salt("host-workspace")
        .show(ui, |ui| match state.section.as_str() {
            "overview" => system(ui, state, cx),
            "metrics" => metrics(ui, state, cx),
            "storage" => storage(ui, state, cx),
            "nodes" => nodes(ui, state, cx),
            "domains" => domains(ui, state, cx),
            "certificates" => certificates(ui, state, cx),
            "notifications" => notifications(ui, state, cx),
            "actions" => actions(ui, state, cx),
            "uptime" => uptime(ui, state, cx),
            "maintenance" => maintenance(ui, state, cx),
            "backups" => backups(ui, state, cx),
            _ => {}
        });
    ui.ctx().request_repaint_after(Duration::from_millis(500));
}

fn refresh_toolbar(ui: &mut egui::Ui, title: &str) -> bool {
    ui.horizontal(|ui| {
        ui.heading(title);
        ui.button("Refresh").clicked()
    })
    .inner
}

fn redact_host(text: &str, server: &crate::config::Server) -> String {
    let mut secrets = vec![server.token.clone()];
    if let Some(secret) = server.ssh.as_ref().and_then(|ssh| ssh.secret.clone()) {
        secrets.push(secret);
    }
    super::backend::redact(text, &secrets)
}

fn metric(value: Option<&Value>, key: &str, percent: bool) -> String {
    let Some(n) = value
        .and_then(|v| v.get(key))
        .and_then(Value::as_array)
        .and_then(|s| s.last())
        .and_then(|p| p.get(1))
        .and_then(number)
    else {
        return "Unknown".into();
    };
    if percent {
        format!("{n:.1}%")
    } else {
        format_rate(n)
    }
}

fn number(v: &Value) -> Option<f64> {
    v.as_f64()
        .or_else(|| v.as_str()?.parse().ok())
        .filter(|n| n.is_finite())
}

fn system(ui: &mut egui::Ui, state: &mut State, cx: &mut PageContext<'_>) {
    let refresh = refresh_toolbar(ui, "System monitoring");
    let due = state
        .snapshots
        .get("system")
        .and_then(|s| s.attempted)
        .is_some_and(|at| at.elapsed() >= Duration::from_secs(10));
    if due && !refresh {
        state.poll(cx, "system", "metrics", "getSystemStats", json!({}));
    } else {
        state.rpc(
            cx,
            "system",
            "metrics",
            "getSystemStats",
            json!({}),
            refresh,
        );
    }
    state.status(ui, cx, "system", "System metrics");
    if let Some(v) = state.value("system") {
        let rows = vec![
            vec![
                "CPU".into(),
                metric(Some(v), "cpu", true),
                field(v, "/cpuCores") + " cores",
            ],
            vec![
                "Memory".into(),
                metric(Some(v), "memory", true),
                bytes_pair(v, "memory"),
            ],
            vec![
                "Disk".into(),
                metric(Some(v), "disk", true),
                bytes_pair(v, "disk"),
            ],
            vec![
                "Network in".into(),
                metric(Some(v), "networkIn", false),
                String::new(),
            ],
            vec![
                "Network out".into(),
                metric(Some(v), "networkOut", false),
                String::new(),
            ],
            vec![
                "Load average".into(),
                crate::monitor::load_avg(v),
                String::new(),
            ],
        ];
        widgets::table(
            ui,
            "system-summary",
            &["Metric", "Latest", "Capacity"],
            &rows,
            &mut None,
            100.0,
        );
        for (key, title, pct) in [
            ("cpu", "CPU history (%)", true),
            ("memory", "Memory history (%)", true),
            ("disk", "Disk history (%)", true),
            ("networkIn", "Network in (bytes/s)", false),
            ("networkOut", "Network out (bytes/s)", false),
        ] {
            chart(ui, v, key, title, pct);
        }
    }
    if ui.button("Open privileged host terminal…").clicked() {
        let host = cx.target.server.as_ref().unwrap().name.clone();
        state.review("Open a privileged root shell", format!("{host}: this grants access to the real host filesystem, all projects, host network and processes. Closing the terminal disconnects its session."), host, Apply::Shell);
    }
}

fn bytes_pair(v: &Value, key: &str) -> String {
    let used = v.get(format!("{key}UsedBytes")).and_then(number);
    let total = v.get(format!("{key}TotalBytes")).and_then(number);
    match (used, total) {
        (Some(a), Some(b)) => format!("{} / {}", format_bytes(a), format_bytes(b)),
        _ => "Capacity unknown".into(),
    }
}

fn chart(ui: &mut egui::Ui, value: &Value, key: &str, title: &str, percent: bool) {
    ui.label(title);
    let points: Vec<_> = value
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|p| Some((p.get(0)?.clone(), number(p.get(1)?)?)))
        .collect();
    if points.is_empty() {
        ui.weak("No history samples received");
        return;
    }
    let ceiling = if percent {
        100.0
    } else {
        points.iter().map(|(_, n)| *n).fold(0.0, f64::max).max(1.0)
    };
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width().max(80.0), 86.0),
        egui::Sense::hover(),
    );
    let painter = ui.painter_at(rect);
    painter.hline(
        rect.x_range(),
        rect.bottom(),
        ui.visuals().widgets.noninteractive.bg_stroke,
    );
    let plot: Vec<_> = points
        .iter()
        .enumerate()
        .map(|(i, (_, n))| {
            egui::pos2(
                rect.left()
                    + rect.width() * i as f32 / points.len().saturating_sub(1).max(1) as f32,
                rect.bottom() - rect.height() * (n / ceiling).clamp(0.0, 1.0) as f32,
            )
        })
        .collect();
    if plot.len() > 1 {
        painter.add(egui::Shape::line(
            plot,
            egui::Stroke::new(1.5, ui.visuals().selection.bg_fill),
        ));
    }
    response.on_hover_text(format!(
        "{} actual samples; vertical scale 0–{}{}",
        points.len(),
        ceiling,
        if percent { "%" } else { " bytes/s" }
    ));
    egui::CollapsingHeader::new(format!("{} samples · inspect values", points.len()))
        .id_salt(key)
        .show(ui, |ui| {
            let rows = points
                .iter()
                .rev()
                .map(|(ts, n)| {
                    vec![
                        if let Some(s) = ts.as_str() {
                            s.to_string()
                        } else {
                            ts.to_string()
                        },
                        if percent {
                            format!("{n:.2}%")
                        } else {
                            format_rate(*n)
                        },
                    ]
                })
                .collect::<Vec<_>>();
            widgets::table(
                ui,
                key,
                &["Timestamp", "Measured value"],
                &rows,
                &mut None,
                100.0,
            );
        });
}

fn filtered_metrics(values: &[Value], pattern: &str) -> (Vec<Vec<String>>, usize) {
    let all = crate::monitor::monitor_rows(values);
    let total = all.len();
    if pattern.is_empty() {
        return (all, total);
    }
    let matcher = crate::filter::FilterMatcher::new(pattern);
    let mut rows = Vec::new();
    let mut remaining = all.into_iter().peekable();
    while let Some(project) = remaining.next() {
        let project_matches = matcher.matches_any(project.iter().map(String::as_str));
        let mut services = Vec::new();
        while remaining
            .peek()
            .is_some_and(|row| row.first().is_some_and(|name| name.starts_with("  ")))
        {
            let service = remaining.next().unwrap();
            if project_matches || matcher.matches_any(service.iter().map(String::as_str)) {
                services.push(service);
            }
        }
        if project_matches || !services.is_empty() {
            rows.push(project);
            rows.append(&mut services);
        }
    }
    (rows, total)
}

fn filtered_storage(values: &[Value], pattern: &str) -> (Vec<Vec<String>>, usize) {
    let all = crate::monitor::storage_rows(values);
    let total = all.len();
    let matcher = crate::filter::FilterMatcher::new(pattern);
    (
        all.into_iter()
            .filter(|row| matcher.matches_any(row.iter().map(String::as_str)))
            .collect(),
        total,
    )
}

fn metrics(ui: &mut egui::Ui, state: &mut State, cx: &mut PageContext<'_>) {
    let refresh = refresh_toolbar(ui, "Service metrics");
    let due = state
        .snapshots
        .get("metrics")
        .and_then(|s| s.attempted)
        .is_some_and(|at| at.elapsed() > Duration::from_secs(10));
    if due && !refresh {
        state.poll(cx, "metrics", "metrics", "getAllServicesStats", json!({}));
    } else {
        state.rpc(
            cx,
            "metrics",
            "metrics",
            "getAllServicesStats",
            json!({}),
            refresh,
        );
    }
    state.status(ui, cx, "metrics", "Service metrics");
    let values = state.rows("metrics");
    ui.horizontal(|ui| {
        ui.label("Filter (text or regex)");
        ui.text_edit_singleline(&mut state.metrics_filter);
    });
    let (rows, total) = filtered_metrics(&values, &state.metrics_filter);
    ui.weak(format!(
        "{} of {total} rows shown; project headers remain attached to matching services",
        rows.len()
    ));
    widgets::table(
        ui,
        "service-metrics",
        &crate::monitor::MONITOR_HEADERS,
        &rows,
        &mut None,
        100.0,
    );
    if state.value("metrics").is_some() && values.is_empty() {
        ui.label("No running service metrics reported.");
    }
}

fn storage(ui: &mut egui::Ui, state: &mut State, cx: &mut PageContext<'_>) {
    let refresh = refresh_toolbar(ui, "Service storage usage");
    state.rpc(
        cx,
        "storage",
        "monitorOld",
        "getStorageStats",
        Value::Null,
        refresh,
    );
    state.status(ui, cx, "storage", "Service storage");
    ui.horizontal(|ui| {
        ui.label("Filter (text or regex)");
        ui.text_edit_singleline(&mut state.storage_filter);
    });
    let (rows, total) = filtered_storage(&state.rows("storage"), &state.storage_filter);
    ui.weak(format!("{} of {total} storage rows shown", rows.len()));
    widgets::table(
        ui,
        "storage-usage",
        &crate::monitor::STORAGE_HEADERS,
        &rows,
        &mut None,
        100.0,
    );
}

fn nodes(ui: &mut egui::Ui, state: &mut State, cx: &mut PageContext<'_>) {
    let refresh = refresh_toolbar(ui, "Cluster nodes");
    state.rpc(cx, "nodes", "cluster", "listNodes", Value::Null, refresh);
    state.status(ui, cx, "nodes", "Cluster nodes");
    let rows = state
        .rows("nodes")
        .iter()
        .map(|v| {
            [
                "/Description/Hostname",
                "/Spec/Role",
                "/Status/State",
                "/Spec/Availability",
                "/Status/Addr",
            ]
            .iter()
            .map(|p| field(v, p))
            .collect()
        })
        .collect::<Vec<_>>();
    widgets::table(
        ui,
        "cluster-nodes",
        &["Hostname", "Role", "State", "Availability", "Address"],
        &rows,
        &mut None,
        100.0,
    );
    if state.value("nodes").is_some() && rows.is_empty() {
        ui.label("No nodes reported; this host may not be a cluster.");
    }
}

fn inventory(state: &mut State, cx: &mut PageContext<'_>, refresh: bool) {
    state.rpc(
        cx,
        "inventory",
        "projects",
        "listProjectsAndServices",
        Value::Null,
        refresh,
    );
}

fn inventory_services(state: &State) -> Vec<Value> {
    state
        .value("inventory")
        .and_then(|v| v.get("services"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn inventory_projects(state: &State) -> Vec<String> {
    state
        .value("inventory")
        .and_then(|v| v.get("projects"))
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|p| p.get("name").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn domain_editor(state: &mut State, existing: Option<Value>) {
    let projects = inventory_projects(state);
    let id = existing
        .as_ref()
        .and_then(|v| v.get("id"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let form = Form::new(
        FormKind::DomainCreate,
        if id.is_some() {
            "Edit domain"
        } else {
            "Create domain"
        },
        form::domain_fields(existing.as_ref(), &projects),
    );
    let mut form = form;
    form.original = existing;
    state.clear_success_notice();
    state.section = "domains".into();
    state.editor = Some(Editor::new(EditKind::Domain(id), form));
}

fn domains(ui: &mut egui::Ui, state: &mut State, cx: &mut PageContext<'_>) {
    let refresh = refresh_toolbar(ui, "Global domains");
    state.rpc(cx, "domains", "domains", "listDomains", json!({}), refresh);
    inventory(state, cx, refresh);
    state.status(ui, cx, "inventory", "Service inventory");
    state.status(ui, cx, "domains", "Domains");
    ui.horizontal(|ui| {
        ui.label("Filter");
        if ui.text_edit_singleline(&mut state.filter).changed() {
            state.selected.remove("domains");
        }
        if ui.button("Create domain").clicked() && state.editor.is_none() {
            domain_editor(state, None);
        }
    });
    let values = state.rows("domains");
    let services = state.value("inventory").map(|_| {
        inventory_services(state)
            .iter()
            .map(|s| (field(s, "/projectName"), field(s, "/name")))
            .collect::<HashSet<_>>()
    });
    let matcher = crate::filter::FilterMatcher::new(&state.filter);
    let visible: Vec<_> = values
        .iter()
        .filter(|v| {
            matcher.matches_any(
                crate::domains::domain_filter_row(v)
                    .iter()
                    .map(String::as_str),
            )
        })
        .collect();
    ui.horizontal_wrapped(|ui| {
        ui.weak(format!(
            "{} of {} domains · {} marked",
            visible.len(),
            values.len(),
            state.marked.len()
        ));
        if ui.button("Mark filtered").clicked() {
            state.marked.extend(visible.iter().map(|v| field(v, "/id")));
        }
        if ui.button("Clear marks").clicked() {
            state.marked.clear();
        }
    });
    let rows = visible
        .iter()
        .map(|v| {
            vec![
                if state.marked.contains(&field(v, "/id")) {
                    "Marked"
                } else {
                    ""
                }
                .into(),
                crate::domains::domain_source(v),
                crate::domains::domain_destination(v),
                if crate::domains::is_orphan(v, services.as_ref()) {
                    "Dead route: service missing"
                } else if services.is_none() {
                    "Destination unknown"
                } else {
                    "Configured"
                }
                .into(),
                field(v, "/id"),
            ]
        })
        .collect::<Vec<_>>();
    let mut selected = state.choose("domains", rows.len());
    widgets::table(
        ui,
        "host-domains",
        &[
            "Selection",
            "Source (wildcard shown)",
            "Destination",
            "Route status",
            "ID",
        ],
        &rows,
        &mut selected,
        100.0,
    );
    state.selected.insert("domains".into(), selected);
    if let Some(v) = selected.and_then(|i| visible.get(i)).copied() {
        let id = field(v, "/id");
        ui.horizontal_wrapped(|ui| {
            if ui.button("Copy source").clicked() {
                ui.ctx().copy_text(crate::domains::domain_source(v));
            }
            if ui.button("Copy destination").clicked() {
                ui.ctx().copy_text(crate::domains::domain_destination(v));
            }
            if ui
                .button(if state.marked.contains(&id) {
                    "Unmark"
                } else {
                    "Mark"
                })
                .clicked()
                && !state.marked.remove(&id)
            {
                state.marked.insert(id.clone());
            }
            if ui.button("Edit route").clicked() && state.editor.is_none() {
                domain_editor(state, Some(v.clone()));
            }
            if ui.button("Set primary").clicked() {
                state.review(
                    "Set primary domain",
                    crate::domains::domain_source(v),
                    String::new(),
                    Apply::Worker(Box::new(Req::DomainSetPrimary(id.clone()))),
                );
            }
            if ui.button("Delete domain…").clicked() {
                state.review(
                    "Delete domain",
                    crate::domains::domain_source(v),
                    field(v, "/host"),
                    Apply::Worker(Box::new(Req::DomainDelete(id.clone()))),
                );
            }
            if ui
                .add_enabled(state.watch_loaded, egui::Button::new("Watch uptime…"))
                .clicked()
                && state.editor.is_none()
            {
                open_check(state, &crate::domains::domain_source(v));
            }
        });
    }
    egui::CollapsingHeader::new("Bulk routing changes").show(ui, |ui| {
        if state.bulk_target.is_empty() { state.bulk_target = crate::domains::TARGETS[0].into(); }
        let changed = ui.horizontal_wrapped(|ui| {
            let before = state.bulk_target.clone();
            egui::ComboBox::from_id_salt("bulk-domain-part").selected_text(&state.bulk_target).show_ui(ui, |ui| { for &part in crate::domains::TARGETS { ui.selectable_value(&mut state.bulk_target, part.into(), part); } });
            ui.label("Find"); let a = ui.text_edit_singleline(&mut state.bulk_find).changed();
            ui.label("Replace"); let b = ui.text_edit_singleline(&mut state.bulk_replace).changed();
            a || b || before != state.bulk_target
        }).inner;
        if changed { state.bulk_plan.clear(); }
        if ui.button("Preview marked domains").clicked() {
            let targets = values.iter().filter(|v| state.marked.contains(&field(v, "/id"))).collect::<Vec<_>>();
            match crate::domains::plan(&targets, &state.bulk_target, &state.bulk_find, &state.bulk_replace) {
                Ok(plan) => { state.bulk_plan = plan; state.bulk_error.clear(); }
                Err(e) => state.bulk_error = e,
            }
        }
        if !state.bulk_error.is_empty() { widgets::error(ui, &state.bulk_error); }
        let rows = state.bulk_plan.iter().map(|p| vec![p.host.clone(), p.before.clone(), p.after.clone()]).collect::<Vec<_>>();
        widgets::table(ui, "domain-rewrite-review", &["Domain", "Before", "After"], &rows, &mut None, 100.0);
        if !state.bulk_plan.is_empty() && ui.button("Review applying displayed changes…").clicked() {
            let changes = state.bulk_plan.iter().map(|c| (c.id.clone(), c.host.clone(), c.body.clone())).collect();
            let preview = state.bulk_plan.iter().map(|c| format!("{}\nBefore: {}\nAfter: {}", c.host, c.before, c.after)).collect::<Vec<_>>().join("\n\n");
            state.review("Apply domain rewrites", format!("{preview}\n\n{} displayed changes on {}. Partial failures are reported per domain; there is no transaction rollback.", state.bulk_plan.len(), cx.target.server.as_ref().unwrap().name), state.bulk_plan.len().to_string(), Apply::Worker(Box::new(Req::DomainBulkEdit { changes })));
        }
        if !state.marked.is_empty() && ui.button("Review deleting marked domains…").clicked() {
            let domains = values.iter().filter(|v| state.marked.contains(&field(v, "/id"))).map(|v| (field(v, "/id"), crate::domains::domain_source(v))).collect::<Vec<_>>();
            let list = domains.iter().map(|(_, host)| host.as_str()).collect::<Vec<_>>().join("\n");
            state.review("Delete marked domains", list, domains.len().to_string(), Apply::Worker(Box::new(Req::DomainBulkDelete { domains })));
        }
        if ui.button("Discard bulk draft").clicked() { state.bulk_plan.clear(); state.bulk_find.clear(); state.bulk_replace.clear(); state.bulk_error.clear(); }
    });
}

fn certificates(ui: &mut egui::Ui, state: &mut State, cx: &mut PageContext<'_>) {
    let refresh = refresh_toolbar(ui, "Certificates");
    state.rpc(
        cx,
        "certificates",
        "certificates",
        "listCertificates",
        Value::Null,
        refresh,
    );
    state.status(ui, cx, "certificates", "Certificates");
    let values = state.rows("certificates");
    let rows = values
        .iter()
        .map(|v| {
            vec![
                field(v, "/domain/main"),
                v.pointer("/domain/sans")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default(),
            ]
        })
        .collect::<Vec<_>>();
    let mut selected = state.choose("certificates", values.len());
    widgets::table(
        ui,
        "host-certificates",
        &["Primary domain", "Subject alternative names"],
        &rows,
        &mut selected,
        100.0,
    );
    state.selected.insert("certificates".into(), selected);
    if let Some(v) = selected.and_then(|i| values.get(i)) {
        if ui.button("Remove certificate…").clicked() {
            let domain = field(v, "/domain/main");
            state.review(
                "Remove certificate",
                format!(
                    "{} on {}. TLS can be interrupted until a certificate is issued again.",
                    domain,
                    cx.target.server.as_ref().unwrap().name
                ),
                domain.clone(),
                Apply::Rpc {
                    group: "certificates",
                    op: "removeCertificate",
                    body: json!({"domain":domain}),
                },
            );
        }
    }
    ui.weak("Certificates are issued through domain TLS/resolver configuration. Private certificate material is never rendered here.");
}

fn notifications(ui: &mut egui::Ui, state: &mut State, cx: &mut PageContext<'_>) {
    let refresh = refresh_toolbar(ui, "Notification channels");
    state.rpc(
        cx,
        "notifications",
        "notifications",
        "listNotificationChannels",
        Value::Null,
        refresh,
    );
    state.status(ui, cx, "notifications", "Notification channels");
    let channels = state
        .value("notifications")
        .and_then(|v| v.get("notificationChannels"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let rows = channels
        .iter()
        .map(|v| vec![field(v, "/name"), field(v, "/type"), field(v, "/id")])
        .collect::<Vec<_>>();
    let mut selected = state.choose("notifications", rows.len());
    widgets::table(
        ui,
        "notification-channels",
        &["Name", "Channel type", "ID"],
        &rows,
        &mut selected,
        100.0,
    );
    state.selected.insert("notifications".into(), selected);
    if let Some(v) = selected.and_then(|i| channels.get(i)) {
        ui.weak("Channel credentials and webhook URLs are hidden.");
        if ui.button("Delete channel…").clicked() {
            let name = field(v, "/name");
            state.review(
                "Delete notification channel",
                format!(
                    "{name} on {}. This stops its notifications.",
                    cx.target.server.as_ref().unwrap().name
                ),
                name,
                Apply::Rpc {
                    group: "notifications",
                    op: "destroyNotificationChannel",
                    body: json!({"id":field(v,"/id")}),
                },
            );
        }
    }
}

fn action_is_running(v: &Value) -> bool {
    v.get("status").and_then(Value::as_str) == Some("running")
}

fn filtered_actions(
    values: &[Value],
    pattern: &str,
    status: &str,
) -> (Vec<usize>, Vec<Vec<String>>) {
    let matcher = crate::filter::FilterMatcher::new(pattern);
    let mut indices = Vec::new();
    let mut rows = Vec::new();
    for (index, value) in values.iter().enumerate() {
        if !status.is_empty()
            && if status == "!done" {
                field(value, "/status") == "done"
            } else {
                field(value, "/status") != status
            }
        {
            continue;
        }
        let row = vec![
            field(value, "/status"),
            format!(
                "{}/{}",
                field(value, "/projectName"),
                field(value, "/serviceName")
            ),
            field(value, "/type"),
            field(value, "/description"),
            field(value, "/createdAt"),
            field(value, "/id"),
        ];
        if matcher.matches_any(row.iter().map(String::as_str)) {
            indices.push(index);
            rows.push(row);
        }
    }
    (indices, rows)
}

fn actions(ui: &mut egui::Ui, state: &mut State, cx: &mut PageContext<'_>) {
    let mut refresh = refresh_toolbar(ui, "Action history");
    if state.action_limit == 0 {
        state.action_limit = 100;
    }
    ui.horizontal_wrapped(|ui| {
        ui.label("Project");
        refresh |= ui
            .text_edit_singleline(&mut state.action_project)
            .lost_focus();
        ui.label("Service");
        refresh |= ui
            .text_edit_singleline(&mut state.action_service)
            .lost_focus();
        ui.label("Type");
        refresh |= ui.text_edit_singleline(&mut state.action_type).lost_focus();
        ui.label("Limit");
        refresh |= ui
            .add(egui::DragValue::new(&mut state.action_limit).range(1..=1000))
            .changed();
        ui.checkbox(&mut state.follow_actions, "Poll every 5s");
        ui.label("Status");
        egui::ComboBox::from_id_salt("action-status-filter")
            .selected_text(if state.action_status.is_empty() {
                "All"
            } else {
                &state.action_status
            })
            .show_ui(ui, |ui| {
                for (value, label) in [
                    ("", "All"),
                    ("running", "Running"),
                    ("!done", "Not completed cleanly"),
                    ("error", "Error"),
                    ("done", "Done"),
                ] {
                    if ui
                        .selectable_value(&mut state.action_status, value.into(), label)
                        .changed()
                    {
                        state.selected.remove("actions");
                    }
                }
            });
    });
    ui.horizontal(|ui| {
        ui.label("Filter results (text or regex)");
        if ui.text_edit_singleline(&mut state.action_filter).changed() {
            state.selected.remove("actions");
        }
    });
    let mut input = json!({"limit":state.action_limit});
    for (key, value) in [
        ("projectName", &state.action_project),
        ("serviceName", &state.action_service),
        ("type", &state.action_type),
    ] {
        if !value.trim().is_empty() {
            input[key] = json!(value.trim());
        }
    }
    let due = state.follow_actions
        && state
            .snapshots
            .get("actions")
            .and_then(|s| s.attempted)
            .is_some_and(|at| at.elapsed() >= Duration::from_secs(5));
    if due && !refresh {
        state.poll(cx, "actions", "actions", "listActions", input);
    } else {
        state.rpc(cx, "actions", "actions", "listActions", input, refresh);
    }
    state.status(ui, cx, "actions", "Action history");
    let values = state.rows("actions");
    let (visible, rows) = filtered_actions(&values, &state.action_filter, &state.action_status);
    ui.weak(format!(
        "{} of {} returned actions shown",
        rows.len(),
        values.len()
    ));
    let mut selected = state.choose("actions", rows.len());
    widgets::table(
        ui,
        "host-actions",
        &["Status", "Target", "Type", "Description", "Created", "ID"],
        &rows,
        &mut selected,
        100.0,
    );
    state.selected.insert("actions".into(), selected);
    if let Some(v) = selected
        .and_then(|i| visible.get(i))
        .and_then(|&i| values.get(i))
    {
        ui.horizontal(|ui| {
            if ui.button("Open details / deployment log").clicked() {
                let id = field(v, "/id");
                state.action_detail = Some(id.clone());
                state.rpc(cx, "action-detail", "actions", "getAction", json!({"id":id}), true);
            }
            if action_is_running(v) && ui.button("Request remote cancellation…").clicked() {
                let id = field(v, "/id");
                state.review("Kill running action", format!("{}: {}. This rechecks its status, then sends actions.killAction only while running. Already-completed writes are not rolled back.", id, field(v, "/description")), id.clone(), Apply::KillAction(id));
            }
        });
    }
    if let Some(id) = state.action_detail.clone() {
        state.rpc(
            cx,
            "action-detail",
            "actions",
            "getAction",
            json!({"id":id}),
            false,
        );
        ui.separator();
        ui.horizontal(|ui| {
            ui.strong(format!("Action {id}"));
            if ui.button("Refresh detail").clicked() {
                state.rpc(
                    cx,
                    "action-detail",
                    "actions",
                    "getAction",
                    json!({"id":id}),
                    true,
                );
            }
            if ui.button("Close detail").clicked() {
                state.action_detail = None;
            }
        });
        state.status(ui, cx, "action-detail", "Selected action");
        if let Some(v) = state
            .value("action-detail")
            .filter(|v| field(v, "/id") == id)
        {
            let rows = [
                "status",
                "type",
                "description",
                "projectName",
                "serviceName",
                "createdAt",
                "updatedAt",
            ]
            .iter()
            .map(|key| vec![(*key).into(), field(v, &format!("/{key}"))])
            .collect::<Vec<_>>();
            widgets::table(
                ui,
                "action-properties",
                &["Property", "Value"],
                &rows,
                &mut None,
                100.0,
            );
            let raw = v
                .get("log")
                .and_then(Value::as_str)
                .unwrap_or("No deployment/action log was recorded.");
            let log = redact_host(raw, cx.target.server.as_ref().unwrap());
            if ui.button("Copy log").clicked() {
                ui.ctx().copy_text(log.clone());
            }
            egui::ScrollArea::both()
                .id_salt("action-log")
                .max_height(360.0)
                .show(ui, |ui| {
                    ui.add(
                        egui::Label::new(egui::RichText::new(&log).monospace()).selectable(true),
                    );
                });
        }
        let running = state.value("action-detail").is_some_and(action_is_running);
        let due = state.follow_actions
            && running
            && state
                .snapshots
                .get("action-detail")
                .and_then(|s| s.attempted)
                .is_some_and(|at| at.elapsed() >= Duration::from_secs(5));
        if due {
            state.poll(
                cx,
                "action-detail",
                "actions",
                "getAction",
                json!({"id":id}),
            );
        }
    }
}

fn open_check(state: &mut State, url: &str) {
    state.clear_success_notice();
    state.section = "uptime".into();
    state.reveal_check_secrets = false;
    let check = state
        .watch
        .iter()
        .find(|c| c.url == url)
        .cloned()
        .unwrap_or_else(|| crate::uptime::Check::get(url));
    state.editor = Some(Editor::new(
        EditKind::Check(url.into()),
        Form::new(
            FormKind::CheckEdit { url: url.into() },
            format!("Request check: {url}"),
            form::check_fields(&check),
        ),
    ));
}

fn load_watch(state: &mut State, cx: &mut PageContext<'_>) {
    if !state.watch_requested {
        state.watch_requested = true;
        let host = cx.target.server.as_ref().unwrap().name.clone();
        let path = cx.watchlist_path().to_owned();
        cx.background("watch-load", "Loading enrolled uptime checks", move || {
            Ok(Payload::Json(serde_json::to_value(
                crate::config::Watchlist::new(path).all(&host),
            )?))
        });
    }
}

fn uptime(ui: &mut egui::Ui, state: &mut State, cx: &mut PageContext<'_>) {
    ui.heading("Uptime watchlist");
    if !state.watch_loaded {
        ui.label("Waiting for the saved watchlist.");
        if ui.button("Retry loading watchlist").clicked() {
            state.watch_requested = false;
        }
    }
    ui.weak("Only enrolled requests are sent, from this computer. Redirects are not followed. A POST, PUT, PATCH or DELETE check can modify its destination.");
    ui.horizontal_wrapped(|ui| {
        if ui.add_enabled(!state.watch.is_empty() && !cx.busy("checks"), egui::Button::new("Run enrolled checks")).clicked() {
            let mutating = state.watch.iter().any(|c| !matches!(c.method.as_str(), "GET" | "HEAD"));
            if mutating { state.review("Send configured HTTP requests", state.watch.iter().map(|c| format!("{} {}", c.method, c.url)).collect::<Vec<_>>().join("\n") + "\nThese methods may change remote data. Only continue for endpoints intended for monitoring.", "SEND".into(), Apply::Worker(Box::new(Req::RunChecks(state.watch.clone())))); }
            else { state.check_inputs = state.watch.clone(); cx.request("checks", Req::RunChecks(state.watch.clone())); }
        }
        if cx.busy("checks") { ui.spinner(); ui.label("Checks running; dismissing this page does not cancel requests already sent."); }
        ui.label("URL"); ui.text_edit_singleline(&mut state.watch_url);
        if ui.add_enabled(state.watch_loaded, egui::Button::new("Configure check…")).clicked() && state.editor.is_none() { let url = state.watch_url.trim().to_string(); open_check(state, &url); }
    });
    let ranked = crate::uptime::ranked(&state.watch, &state.probes);
    let median = crate::uptime::median_head(&state.probes);
    if let Some(median) = median {
        ui.label(format!(
            "Peer median response head: {}",
            crate::uptime::human(median)
        ));
    }
    let rows = ranked
        .iter()
        .map(|(c, p)| {
            let (status, head, total, error) = match p {
                Some(p) => match &p.outcome {
                    crate::uptime::Outcome::Answered {
                        status,
                        head,
                        total,
                    } => (
                        format!("{status} · {:?}", p.verdict(c)),
                        crate::uptime::human(*head),
                        crate::uptime::human(*total),
                        crate::uptime::slowness(p, median)
                            .map(|n| format!("{n:.1}× peer median"))
                            .unwrap_or_default(),
                    ),
                    crate::uptime::Outcome::Failed(error) => (
                        "Unreachable".into(),
                        String::new(),
                        String::new(),
                        error.clone(),
                    ),
                },
                None => (
                    "Not checked".into(),
                    String::new(),
                    String::new(),
                    String::new(),
                ),
            };
            vec![c.method.clone(), c.url.clone(), status, head, total, error]
        })
        .collect::<Vec<_>>();
    let urls = ranked
        .iter()
        .map(|(c, _)| c.url.clone())
        .collect::<Vec<_>>();
    let mut selected = state.choose("uptime", rows.len());
    widgets::table(
        ui,
        "uptime-results",
        &[
            "Method",
            "URL",
            "Result",
            "Response head",
            "Total",
            "Finding",
        ],
        &rows,
        &mut selected,
        100.0,
    );
    state.selected.insert("uptime".into(), selected);
    if let Some(url) = selected.and_then(|i| urls.get(i)) {
        ui.horizontal(|ui| {
            if ui.button("Edit request…").clicked() && state.editor.is_none() {
                open_check(state, url);
            }
            if ui.button("Stop watching").clicked() && !cx.busy("watch-save") {
                let host = cx.target.server.as_ref().unwrap().name.clone();
                let url = url.clone();
                let path = cx.watchlist_path().to_owned();
                cx.background("watch-save", "Removing enrolled check", move || {
                    let store = crate::config::Watchlist::new(path);
                    store.remove(&host, &url)?;
                    Ok(Payload::Json(serde_json::to_value(store.all(&host))?))
                });
            }
        });
    }
    if state.watch.is_empty() {
        ui.label("No requests enrolled. Configure a URL here or choose Watch uptime on a domain.");
    }
}

fn maintenance(ui: &mut egui::Ui, state: &mut State, cx: &mut PageContext<'_>) {
    let refresh = refresh_toolbar(ui, "Docker maintenance");
    for op in [
        "getDockerVersion",
        "getServerIp",
        "checkForUpdates",
        "getDailyDockerCleanup",
    ] {
        state.rpc(cx, op, "settings", op, Value::Null, refresh);
    }
    let rows = [
        ("Docker", "getDockerVersion"),
        ("Server IP", "getServerIp"),
        ("Update available", "checkForUpdates"),
        ("Daily cleanup", "getDailyDockerCleanup"),
    ]
    .iter()
    .map(|(label, key)| {
        vec![
            (*label).into(),
            state
                .value(key)
                .map(|v| field(v, ""))
                .unwrap_or_else(|| "Unknown".into()),
        ]
    })
    .collect::<Vec<_>>();
    widgets::table(
        ui,
        "maintenance-info",
        &["Property", "Value"],
        &rows,
        &mut None,
        100.0,
    );
    for (op, resource) in [
        ("getDockerVersion", "Docker version"),
        ("getServerIp", "Server IP"),
        ("checkForUpdates", "Update availability"),
        ("getDailyDockerCleanup", "Daily cleanup settings"),
    ] {
        state.status(ui, cx, op, resource);
    }
    ui.separator();
    ui.strong("Host-wide cleanup");
    ui.label("These operations permanently remove unused Docker resources across every project. There is no undo.");
    for (label, op, consequence) in [
        (
            "Prune Docker system…",
            "systemPrune",
            "Remove unused containers, networks, images and build cache",
        ),
        (
            "Remove unused images…",
            "cleanupDockerImages",
            "Remove unused Docker images",
        ),
        (
            "Remove build cache…",
            "cleanupDockerBuilder",
            "Remove Docker build cache",
        ),
    ] {
        if ui.button(label).clicked() {
            let host = cx.target.server.as_ref().unwrap().name.clone();
            state.review(consequence, format!("{consequence} on {host}. This affects the entire host, not a selected service."), host, Apply::Worker(Box::new(Req::MaintAction(op))));
        }
    }
}

fn backups(ui: &mut egui::Ui, state: &mut State, cx: &mut PageContext<'_>) {
    ui.heading("Backup schedules and completed history");
    let refresh = ui
        .button("Refresh services and storage providers")
        .clicked();
    inventory(state, cx, refresh);
    state.rpc(
        cx,
        "providers",
        "storageProviders/common",
        "list",
        Value::Null,
        refresh,
    );
    state.status(ui, cx, "inventory", "Service inventory");
    state.status(ui, cx, "providers", "Storage providers");
    let services = inventory_services(state);
    let mut target = format!("{}/{}", state.backup_project, state.backup_service);
    let old = target.clone();
    ui.horizontal_wrapped(|ui| {
        ui.label("Destination service");
        egui::ComboBox::from_id_salt("backup-service")
            .selected_text(&target)
            .width(260.0)
            .show_ui(ui, |ui| {
                for service in &services {
                    let name = format!(
                        "{}/{}",
                        field(service, "/projectName"),
                        field(service, "/name")
                    );
                    ui.selectable_value(
                        &mut target,
                        name.clone(),
                        format!("{} ({})", name, field(service, "/type")),
                    );
                }
            });
        ui.selectable_value(&mut state.history_mode, false, "Schedules");
        ui.selectable_value(&mut state.history_mode, true, "Completed history / restore");
    });
    if target != old {
        state.clear_success_notice();
        if let Some(service) = services
            .iter()
            .find(|s| format!("{}/{}", field(s, "/projectName"), field(s, "/name")) == target)
        {
            state.backup_project = field(service, "/projectName");
            state.backup_service = field(service, "/name");
            state.backup_type = field(service, "/type");
            state.database_names.clear();
            state.backup_database.clear();
            state.backup_marked.clear();
        }
    }
    if state.backup_project.is_empty() || state.backup_service.is_empty() {
        ui.label(
            "Choose a service to manage its schedules or restore a completed database backup.",
        );
        return;
    }
    let body = json!({"projectName":state.backup_project,"serviceName":state.backup_service});
    let supported = crate::backup::can_back_up(&state.backup_type);
    if supported {
        ui.horizontal(|ui| {
            ui.label("Database");
            egui::ComboBox::from_id_salt("backup-database")
                .selected_text(&state.backup_database)
                .show_ui(ui, |ui| {
                    for name in &state.database_names {
                        ui.selectable_value(&mut state.backup_database, name.clone(), name);
                    }
                });
            ui.text_edit_singleline(&mut state.backup_database);
            if ui.button("Discover databases").clicked() && !cx.busy("database-names") {
                cx.request(
                    "database-names",
                    Req::DatabasesIn {
                        project: state.backup_project.clone(),
                        service: state.backup_service.clone(),
                        stype: state.backup_type.clone(),
                    },
                );
            }
        });
        if !state.history_mode {
            egui::CollapsingHeader::new("Select multiple databases for an immediate backup").show(ui, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("Mark all discovered databases").clicked() { state.backup_marked.extend(state.database_names.iter().cloned()); }
                    if ui.button("Clear database marks").clicked() { state.backup_marked.clear(); }
                });
                for name in &state.database_names {
                    let mut marked=state.backup_marked.contains(name);
                    if ui.checkbox(&mut marked,name).changed() { if marked {state.backup_marked.insert(name.clone());} else {state.backup_marked.remove(name);} }
                }
                if !state.backup_marked.is_empty() && ui.button("Review marked database backups…").clicked() {
                    let mut databases=state.backup_marked.iter().cloned().collect::<Vec<_>>();databases.sort();
                    if state.rows("providers").iter().any(|p|field(p,"/id")==state.backup_provider) {
                        state.review("Back up marked databases",format!("{}/{}\nDatabases: {}\nProvider: {}\nEach database gets its own temporary schedule and reports its own outcome.",state.backup_project,state.backup_service,databases.join(", "),state.backup_provider),databases.len().to_string(),Apply::BackupMany{project:state.backup_project.clone(),service:state.backup_service.clone(),databases,provider:state.backup_provider.clone()});
                    } else {state.mutation_error="Choose an available storage provider first.".into();}
                }
            });
        }
    }
    if !state.backup_jobs.is_empty() {
        ui.label(format!("{} database backup requests still pending. Already-sent jobs are not cancelled by leaving this page.",state.backup_jobs.len()));
        for (key, name) in &state.backup_jobs {
            ui.label(format!(
                "{name}: {}",
                cx.progress(key).unwrap_or("Waiting for the panel")
            ));
        }
    }
    if !state.backup_outcomes.is_empty() {
        widgets::table(
            ui,
            "backup-batch-results",
            &["Database", "Request outcome", "Details"],
            &state.backup_outcomes,
            &mut None,
            100.0,
        );
    }
    let providers = state.rows("providers");
    provider_picker(
        ui,
        "backup-provider",
        &providers,
        &mut state.backup_provider,
    );
    if state.history_mode {
        backup_history(ui, state, cx);
        return;
    }
    ui.horizontal(|ui| {
        ui.selectable_value(&mut state.backup_volume, false, "Database schedules");
        ui.selectable_value(&mut state.backup_volume, true, "Volume schedules");
    });
    let group = if state.backup_volume {
        "volumeBackups"
    } else {
        "databaseBackups"
    };
    let op = if state.backup_volume {
        "listVolumeBackups"
    } else {
        "listDatabaseBackups"
    };
    let key = format!(
        "backup-schedules:{group}:{}:{}",
        state.backup_project, state.backup_service
    );
    let refresh = ui.button("Refresh schedules").clicked();
    state.rpc(cx, &key, group, op, body, refresh);
    state.status(ui, cx, &key, "Backup schedules");
    let schedules = state.rows(&key);
    let rows = schedules
        .iter()
        .map(|v| {
            vec![
                field(v, "/id"),
                field(
                    v,
                    if state.backup_volume {
                        "/volumeName"
                    } else {
                        "/databaseName"
                    },
                ),
                field(v, "/schedule"),
                field(v, "/enabled"),
                field(v, "/storageProviderId"),
                field(v, "/storageProviderPath"),
            ]
        })
        .collect::<Vec<_>>();
    let mut selected = state.choose(&key, schedules.len());
    widgets::table(
        ui,
        "backup-schedules",
        &[
            "ID",
            "Database / volume",
            "Cron schedule",
            "Enabled",
            "Provider",
            "Provider path",
        ],
        &rows,
        &mut selected,
        100.0,
    );
    state.selected.insert(key, selected);
    ui.horizontal_wrapped(|ui| {
        if !state.backup_volume && supported && ui.button("Create schedule…").clicked() && state.editor.is_none() { schedule_editor(state, None); }
        if !state.backup_volume && supported && ui.add_enabled(state.backup_jobs.is_empty(),egui::Button::new("Back up database now…")).clicked() {
            if crate::backup::is_named(&state.backup_database) && providers.iter().any(|p|field(p,"/id")==state.backup_provider) {
                state.review("Run database backup now", format!("{}/{} database {} → provider {} / {}. A temporary disabled schedule is created, run, then removed.", state.backup_project,state.backup_service,state.backup_database,state.backup_provider,crate::backup::default_path(&state.backup_project)), state.backup_database.clone(), Apply::Worker(Box::new(Req::BackupNow { project:state.backup_project.clone(),service:state.backup_service.clone(),database:state.backup_database.clone(),provider:state.backup_provider.clone(),path:crate::backup::default_path(&state.backup_project) })));
            } else { state.mutation_error = "Choose a named database and an available storage provider first.".into(); }
        }
        if let Some(v) = selected.and_then(|i| schedules.get(i)) {
            let id = field(v,"/id");
            if !state.backup_volume && supported && ui.button("Edit schedule…").clicked() && state.editor.is_none() { schedule_editor(state, Some(v.clone())); }
            if ui.button("Run schedule now…").clicked() {
                state.review("Run backup schedule", format!("{}/{} schedule {} → {}",state.backup_project,state.backup_service,id,field(v,"/storageProviderId")), id.clone(), Apply::Rpc {group,op:if state.backup_volume {"runVolumeBackup"} else {"runDatabaseBackup"},body:json!({"id":id})});
            }
            if ui.button("Delete schedule…").clicked() { state.review("Delete backup schedule", format!("{}/{}: {}. This removes the schedule, not historical backup files.", state.backup_project,state.backup_service,id), id.clone(), Apply::Rpc {group,op:if state.backup_volume {"destroyVolumeBackup"} else {"deleteDatabaseBackup"},body:json!({"id":id})}); }
        }
    });
    if !state.backup_volume && !supported {
        ui.label("Built-in database backup supports MySQL, MariaDB, PostgreSQL and MongoDB. Redis does not support database backup schedules.");
    }
    egui::CollapsingHeader::new("Registered storage providers").show(ui, |ui| {
        let rows = providers
            .iter()
            .map(|p| {
                vec![
                    field(p, "/name"),
                    field(p, "/type"),
                    field(p, "/path"),
                    field(p, "/id"),
                ]
            })
            .collect::<Vec<_>>();
        widgets::table(
            ui,
            "provider-list",
            &["Name", "Type", "Path", "ID"],
            &rows,
            &mut None,
            100.0,
        );
    });
}

fn provider_picker(ui: &mut egui::Ui, id: &str, providers: &[Value], chosen: &mut String) {
    ui.horizontal(|ui| {
        ui.label("Storage provider");
        let label = providers
            .iter()
            .find(|p| field(p, "/id") == *chosen)
            .map(|p| format!("{} ({})", field(p, "/name"), field(p, "/type")))
            .unwrap_or_else(|| "Choose provider".into());
        egui::ComboBox::from_id_salt(id)
            .selected_text(label)
            .show_ui(ui, |ui| {
                for p in providers {
                    ui.selectable_value(
                        chosen,
                        field(p, "/id"),
                        format!("{} ({})", field(p, "/name"), field(p, "/type")),
                    );
                }
            });
    });
}

fn schedule_editor(state: &mut State, existing: Option<Value>) {
    let get = |key: &str, default: &str| {
        existing
            .as_ref()
            .and_then(|v| v.get(key))
            .and_then(Value::as_str)
            .unwrap_or(default)
            .to_string()
    };
    let id = existing
        .as_ref()
        .and_then(|v| v.get("id"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let fields = vec![
        Field::text("Database", &get("databaseName", &state.backup_database)),
        Field::text("Cron schedule", &get("schedule", "0 2 * * *")),
        Field::boolean(
            "Enabled",
            existing
                .as_ref()
                .and_then(|v| v.get("enabled"))
                .and_then(Value::as_bool)
                .unwrap_or(true),
        ),
        Field::choice_owned(
            "Storage provider",
            state
                .rows("providers")
                .iter()
                .map(|p| field(p, "/id"))
                .collect(),
            &get("storageProviderId", &state.backup_provider),
        ),
        Field::text(
            "Provider path",
            &get(
                "storageProviderPath",
                &crate::backup::default_path(&state.backup_project),
            ),
        ),
    ];
    let mut form = Form::new(FormKind::ProjectCreate, "Database backup schedule", fields);
    form.original = existing;
    state.clear_success_notice();
    state.section = "backups".into();
    state.editor = Some(Editor::new(
        EditKind::Schedule {
            id,
            project: state.backup_project.clone(),
            service: state.backup_service.clone(),
        },
        form,
    ));
}

fn restorable_history(
    actions: &[Value],
    providers: &[Value],
    cross_host: bool,
) -> (Vec<(String, crate::backup::BackupFile)>, usize) {
    let all = crate::backup::history_all(actions);
    let total = all.len();
    let files = all
        .into_iter()
        .filter(|(_, f)| {
            crate::backup::is_named(&f.path)
                && crate::backup::is_named(&f.database)
                && crate::backup::is_named(&f.storage_provider_id)
                && (!cross_host
                    || providers.iter().any(|p| {
                        field(p, "/id") == f.storage_provider_id
                            && p.get("type")
                                .and_then(Value::as_str)
                                .is_some_and(crate::backup::is_remote)
                    }))
        })
        .collect::<Vec<_>>();
    let hidden = total - files.len();
    (files, hidden)
}

fn backup_history(ui: &mut egui::Ui, state: &mut State, cx: &mut PageContext<'_>) {
    let host = cx.target.server.as_ref().unwrap().name.clone();
    if state.backup_source.is_empty() {
        state.backup_source = host.clone();
    }
    ui.horizontal(|ui| {
        ui.label("Backup source host");
        egui::ComboBox::from_id_salt("backup-source-host")
            .selected_text(&state.backup_source)
            .show_ui(ui, |ui| {
                for server in cx.servers {
                    ui.selectable_value(
                        &mut state.backup_source,
                        server.name.clone(),
                        &server.name,
                    );
                }
            });
    });
    let key = format!("backup-history:{}", state.backup_source);
    let refresh = ui.button("Refresh completed history").clicked();
    let slot = state.snapshots.entry(key.clone()).or_default();
    if (!slot.requested || refresh) && !cx.busy(&key) {
        if let Some(server) = cx
            .servers
            .iter()
            .find(|s| s.name == state.backup_source)
            .cloned()
        {
            slot.requested = true;
            state
                .history_pending_sources
                .insert(key.clone(), server.clone());
            cx.background(&key, "Loading completed backup history", move || {
                let client = EasypanelClient::for_server(&server);
                let actions = client
                    .call("actions", "listActions", json!({"limit":200}))
                    .map_err(|e| anyhow::anyhow!(redact_host(&e.to_string(), &server)))?;
                let providers = client
                    .call("storageProviders/common", "list", Value::Null)
                    .map_err(|e| anyhow::anyhow!(redact_host(&e.to_string(), &server)))?;
                Ok(Payload::Json(
                    json!({"actions":actions,"providers":providers}),
                ))
            });
        }
    }
    state.status(ui, cx, &key, "Completed backup history");
    let source = state.value(&key);
    let actions = source
        .and_then(|v| v.get("actions"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let source_providers = source
        .and_then(|v| v.get("providers"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let (files, hidden) =
        restorable_history(&actions, &source_providers, state.backup_source != host);
    ui.weak("History is recovered from the latest 200 successful backup actions, not a complete listing of storage objects. Failed/incomplete runs are never offered for restore.");
    if hidden > 0 {
        ui.label(format!("{hidden} backups excluded: incomplete database/file metadata, or a local/unidentified provider that cannot be read from this host."));
    }
    let rows = files
        .iter()
        .map(|(origin, f)| {
            vec![
                f.when.clone(),
                origin.clone(),
                f.database.clone(),
                f.path.clone(),
                f.storage_provider_id.clone(),
            ]
        })
        .collect::<Vec<_>>();
    let mut selected = state.choose(&key, rows.len());
    widgets::table(
        ui,
        "backup-files",
        &[
            "Completed",
            "Source service",
            "Database",
            "Recorded path",
            "Source provider",
        ],
        &rows,
        &mut selected,
        100.0,
    );
    state.selected.insert(key.clone(), selected);
    if let Some((origin, file)) = selected.and_then(|i| files.get(i)) {
        ui.label(format!(
            "Restore {} / {} into {} / {} / {}",
            state.backup_source, origin, host, state.backup_project, state.backup_service
        ));
        ui.weak("Open this selected backup in the transfer workspace to verify source and destination engines, provider identity and destination database. No restore is executed from this page.");
        if crate::backup::can_back_up(&state.backup_type)
            && ui.button("Open restore preflight…").clicked()
        {
            if let Some(source) = state.history_sources.get(&key) {
                let mut destination = cx.target.clone();
                destination.project = state.backup_project.clone();
                destination.service = state.backup_service.clone();
                destination.service_type = state.backup_type.clone();
                cx.open(super::data::backup_restore_page(
                    source,
                    destination,
                    origin,
                    file,
                    &state.backup_database,
                    &state.backup_provider,
                ));
            } else {
                state.mutation_error="Refresh backup history to capture its source connection before opening a restore.".into();
            }
        }
    }
}

fn editor(ui: &mut egui::Ui, state: &mut State, cx: &mut PageContext<'_>) {
    let Some(mut edit) = state.editor.take() else {
        return;
    };
    ui.horizontal(|ui| {
        let width = (ui.available_width() - if edit.dirty() { 132.0 } else { 0.0 }).max(80.0);
        ui.add_sized(
            [width, 20.0],
            egui::Label::new(egui::RichText::new(&edit.form.title).strong()).truncate(),
        );
        if edit.dirty() {
            ui.label(egui::RichText::new("Unsaved changes").color(ui.visuals().warn_fg_color));
        }
    });
    let host = &cx.target.server.as_ref().unwrap().name;
    let target = match &edit.kind {
        EditKind::Domain(id) => {
            format!("Host: {host} · Domain: {}", id.as_deref().unwrap_or("new"))
        }
        EditKind::Check(url) => format!("Host: {host} · Request: {url}"),
        EditKind::Schedule {
            id,
            project,
            service,
        } => format!(
            "Host: {host} · {project}/{service} · Schedule: {}",
            id.as_deref().unwrap_or("new"),
        ),
    };
    ui.add(egui::Label::new(&target).truncate())
        .on_hover_text(&target);
    if let EditKind::Domain(_) = &edit.kind {
        let projects = inventory_projects(state);
        if let Some(f) = edit.form.fields.iter_mut().find(|f| f.label == "Project") {
            f.set_options(projects);
        }
        let project = edit.form.by_label("Project");
        let choices = inventory_services(state)
            .iter()
            .filter(|v| field(v, "/projectName") == project)
            .map(|v| field(v, "/name"))
            .collect();
        if let Some(f) = edit.form.fields.iter_mut().find(|f| f.label == "Service") {
            f.set_options(choices);
        }
    }
    let busy = cx.busy("host-mutation") || cx.busy("watch-save");
    let save_label = match &edit.kind {
        EditKind::Domain(None) => "Create domain",
        EditKind::Schedule { id: None, .. } => "Create schedule",
        EditKind::Check(url) if !state.watch.iter().any(|check| check.url == *url) => {
            "Create request"
        }
        _ => "Save changes",
    };
    let mut save = false;
    let mut discard = false;
    if !busy
        && !egui::Popup::is_any_open(ui.ctx())
        && ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
    {
        discard = edit.cancel();
    }
    ui.add_enabled_ui(!busy, |ui| {
        ui.horizontal_wrapped(|ui| {
            save = ui.button(save_label).clicked();
            if ui.button("Cancel").clicked() {
                discard = edit.cancel();
            }
            if edit.form.error.is_some() {
                ui.colored_label(ui.visuals().error_fg_color, "Validation error — see form");
            }
            if !state.mutation_error.is_empty() {
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    "Operation needs attention — see details",
                );
            }
        });
        if edit.discard {
            ui.horizontal_wrapped(|ui| {
                ui.label("Discard unsaved changes?");
                if ui.button("Discard changes").clicked() {
                    discard = true;
                }
                if ui.button("Keep editing").clicked() {
                    edit.discard = false;
                }
            });
        }
    });
    if busy {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("Saving; wait before leaving this editor.");
        });
    }
    if save {
        match &edit.kind {
            EditKind::Domain(id) => match form::domain_body(&edit.form) {
                Ok(body) => {
                    state.editor_request = Some("host-mutation");
                    state.mutation_section = state.section_label(cx.target);
                    cx.request(
                        "host-mutation",
                        Req::DomainSave {
                            id: id.clone(),
                            body,
                        },
                    );
                    edit.form.error = None;
                }
                Err(e) => edit.form.error = Some(e),
            },
            EditKind::Check(url) => match form::check_body(url, &edit.form) {
                Ok(check) => {
                    let host = cx.target.server.as_ref().unwrap().name.clone();
                    state.editor_request = Some("watch-save");
                    let path = cx.watchlist_path().to_owned();
                    cx.background("watch-save", "Saving enrolled request", move || {
                        let store = crate::config::Watchlist::new(path);
                        store.put(&host, check)?;
                        Ok(Payload::Json(serde_json::to_value(store.all(&host))?))
                    });
                    edit.form.error = None;
                }
                Err(e) => edit.form.error = Some(e),
            },
            EditKind::Schedule {
                id,
                project,
                service,
            } => {
                let database = edit.form.by_label("Database");
                let schedule = edit.form.by_label("Cron schedule");
                let provider = edit.form.by_label("Storage provider");
                if !crate::backup::is_named(&database)
                    || schedule.is_empty()
                    || !state
                        .rows("providers")
                        .iter()
                        .any(|p| field(p, "/id") == provider)
                {
                    edit.form.error = Some(
                        "A named database, cron schedule and available provider are required."
                            .into(),
                    );
                } else {
                    let mut body = crate::backup::schedule_body(
                        project,
                        service,
                        &database,
                        &schedule,
                        edit.form.is_on_label("Enabled"),
                        &provider,
                        &edit.form.by_label("Provider path"),
                    );
                    if let Some(original) = edit.form.original.as_ref().and_then(Value::as_object) {
                        let mut merged = Value::Object(original.clone());
                        for (k, v) in body.as_object().unwrap() {
                            merged[k] = v.clone();
                        }
                        body = merged;
                    }
                    if let Some(id) = id {
                        body["id"] = json!(id);
                    }
                    state.editor_request = Some("host-mutation");
                    state.mutation_section = state.section_label(cx.target);
                    cx.rpc(
                        "host-mutation",
                        "databaseBackups",
                        if id.is_some() {
                            "updateDatabaseBackup"
                        } else {
                            "createDatabaseBackup"
                        },
                        body,
                    );
                    edit.form.error = None;
                }
            }
        }
    }
    ui.separator();
    let height = ui.available_height().max(24.0);
    egui::ScrollArea::vertical()
        .id_salt(("host-editor", &edit.form.title))
        .max_height(height)
        .min_scrolled_height(24.0)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if matches!(edit.kind, EditKind::Domain(_)) {
                let refresh = ui
                    .add_enabled(
                        !cx.busy("inventory"),
                        egui::Button::new("Refresh inventory").small(),
                    )
                    .clicked();
                inventory(state, cx, refresh);
                state.status(ui, cx, "inventory", "Service inventory");
            }
            state.notice(ui, cx.target, false);
            if !state.mutation_error.is_empty() {
                widgets::error(ui, &state.mutation_error);
            }
            ui.add_enabled_ui(!busy, |ui| {
                if matches!(edit.kind, EditKind::Check(_)) {
                    ui.checkbox(
                        &mut state.reveal_check_secrets,
                        "Reveal request headers and body",
                    );
                    for f in &mut edit.form.fields {
                        if matches!(f.label, "Headers" | "Body") {
                            f.kind = if state.reveal_check_secrets {
                                crate::form::FieldKind::Editor
                            } else {
                                crate::form::FieldKind::Secret
                            };
                        }
                    }
                }
                widgets::form_fields_sized(ui, &mut edit.form, (height - 64.0).max(24.0));
            });
        });
    if !discard {
        state.editor = Some(edit);
    }
}

fn review(ui: &mut egui::Ui, state: &mut State, cx: &mut PageContext<'_>) {
    let Some(mut review) = state.review.take() else {
        return;
    };
    ui.add(egui::Label::new(egui::RichText::new(&review.title).strong()).truncate());
    let host = format!("Host: {}", cx.target.server.as_ref().unwrap().name);
    ui.add(egui::Label::new(&host).truncate())
        .on_hover_text(&host);
    let resource_id = match &review.apply {
        Apply::Rpc { body, .. } => body.get("id").and_then(Value::as_str),
        Apply::KillAction(id) => Some(id.as_str()),
        Apply::Worker(req) => match req.as_ref() {
            Req::DomainDelete(id) | Req::DomainSetPrimary(id) => Some(id.as_str()),
            _ => None,
        },
        _ => None,
    };
    if let Some(id) = resource_id {
        ui.add(egui::Label::new(format!("Target ID: {id}")).truncate())
            .on_hover_text(id);
    }
    let approved = ui
        .add_enabled_ui(!cx.busy("host-mutation") && !cx.busy("checks"), |ui| {
            if !review.typed.is_empty() {
                let prompt = format!("Type {} to confirm this target:", review.typed);
                let label = ui
                    .add(egui::Label::new(&prompt).truncate())
                    .on_hover_text(&prompt);
                ui.add(egui::TextEdit::singleline(&mut review.entry).desired_width(f32::INFINITY))
                    .labelled_by(label.id);
            }
            ui.add_enabled(
                review.typed.is_empty() || review.entry == review.typed,
                egui::Button::new(&review.title)
                    .fill(ui.visuals().error_fg_color.gamma_multiply(0.20)),
            )
            .clicked()
        })
        .inner;
    let dismiss = ui
        .button("Dismiss review (does not cancel remote work)")
        .clicked()
        || ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
    ui.separator();
    egui::ScrollArea::vertical()
        .id_salt("host-review-details")
        .max_height(ui.available_height().max(24.0))
        .min_scrolled_height(24.0)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add(egui::Label::new(&review.target).wrap());
            state.notice(ui, cx.target, false);
            if !state.mutation_error.is_empty() {
                widgets::error(ui, &state.mutation_error);
            }
        });
    if approved && !cx.busy("host-mutation") {
        state.mutation_section = state.section_label(cx.target);
        match review.apply {
            Apply::Rpc { group, op, body } => cx.rpc("host-mutation", group, op, body),
            Apply::KillAction(id) => {
                let server = cx.target.server.as_ref().unwrap().clone();
                cx.background(
                    "host-mutation",
                    "Cancelling running remote action",
                    move || {
                        let client = EasypanelClient::for_server(&server);
                        let current = client.call("actions", "getAction", json!({"id":id}))?;
                        if !action_is_running(&current) {
                            anyhow::bail!("Action is no longer running; no cancellation was sent.");
                        }
                        Ok(Payload::Json(client.call(
                            "actions",
                            "killAction",
                            json!({"id":id}),
                        )?))
                    },
                );
            }
            Apply::BackupMany {
                project,
                service,
                databases,
                provider,
            } => {
                if state.backup_jobs.is_empty() {
                    state.backup_outcomes.clear();
                    for database in databases {
                        let key = format!("backup-now:{project}:{service}:{database}");
                        cx.request(
                            &key,
                            Req::BackupNow {
                                project: project.clone(),
                                service: service.clone(),
                                database: database.clone(),
                                provider: provider.clone(),
                                path: crate::backup::default_path(&project),
                            },
                        );
                        state.backup_jobs.push((key, database));
                    }
                } else {
                    state.mutation_error="Wait for the pending database backup requests before starting another batch.".into();
                }
            }
            Apply::Worker(req) => match *req {
                Req::RunChecks(checks) => {
                    state.check_inputs = checks.clone();
                    cx.request("checks", Req::RunChecks(checks));
                }
                req => cx.request("host-mutation", req),
            },
            Apply::Shell => {
                let mut target = cx.target.clone();
                target.project.clear();
                target.service.clear();
                target.section = "host".into();
                cx.open(OpenPage {
                    kind: PageKind::Terminal,
                    title: format!("Host shell · {}", target.server.as_ref().unwrap().name),
                    target,
                });
            }
        }
    } else if !dismiss {
        state.review = Some(review);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_host_task_context_never_retargets_the_document() {
        let target = Target {
            server: Some(crate::config::Server {
                name: "fixed-host".into(),
                url: "http://127.0.0.1".into(),
                token: "fixture-token".into(),
                default: false,
                ssh: None,
            }),
            project: "seeded-project".into(),
            service: "seeded-database".into(),
            section: "domains".into(),
            ..Default::default()
        };
        let original = target.clone();
        let mut state = State::default();
        for &(id, label) in SECTIONS {
            let initial = Target {
                section: id.into(),
                ..target.clone()
            };
            assert_eq!(state.section_label(&initial), label);
        }
        for &(id, label) in SECTIONS {
            state.section = id.into();
            assert_eq!(state.section_label(&target), label);
            assert_eq!(state.breadcrumb(&target), format!("fixed-host / {label}"));
            assert!(target.same_resource(&original));
        }
        open_check(&mut state, "https://fixture.invalid/health");
        assert_eq!(state.section, "uptime");
        schedule_editor(&mut state, None);
        assert_eq!(state.section, "backups");
        domain_editor(&mut state, None);
        assert_eq!(state.section, "domains");
        assert!(target.same_resource(&original));
        assert_eq!(state.section_label(&Target::default()), "Fleet");
        assert_eq!(state.breadcrumb(&Target::default()), "Fleet");
        let invalid = Target {
            section: "unknown".into(),
            ..target
        };
        assert_eq!(State::default().section_label(&invalid), SECTIONS[0].1);
    }

    #[test]
    fn live_host_caption_growth_reveals_close_once_without_stopping_manual_tab_browsing() {
        use super::super::{theme, Desktop, PageKind, PageState};
        fn frame(
            ctx: &egui::Context,
            app: &mut Desktop,
            width: f32,
            events: Vec<egui::Event>,
        ) -> (egui::FullOutput, egui::Rect) {
            let mut viewport = egui::Rect::NOTHING;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(width, 680.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    egui::Panel::left("test-explorer")
                        .exact_size(298.0)
                        .show(ui, |_| {});
                    egui::CentralPanel::default()
                        .frame(egui::Frame::NONE)
                        .show(ui, |ui| {
                            viewport = ui.available_rect_before_wrap();
                            app.tabs(ui, theme::Palette::for_dark(false));
                        });
                },
            );
            output.textures_delta.clear();
            (output, viewport)
        }
        fn active_tab(output: &egui::FullOutput, caption: &str) -> egui::Rect {
            let nodes = &output
                .platform_output
                .accesskit_update
                .as_ref()
                .unwrap()
                .nodes;
            let title = nodes
                .iter()
                .find(|(_, node)| node.label() == Some(caption))
                .unwrap()
                .1
                .bounds()
                .unwrap();
            let close = nodes
                .iter()
                .filter(|(_, node)| node.label() == Some("Close document"))
                .filter_map(|(_, node)| node.bounds())
                .filter(|rect| rect.x0 >= title.x1)
                .min_by(|left, right| left.x0.total_cmp(&right.x0))
                .unwrap();
            egui::Rect::from_min_max(
                egui::pos2(title.x0 as f32, title.y0 as f32),
                egui::pos2(close.x1 as f32, close.y1 as f32),
            )
        }
        for width in [1024.0, 1280.0] {
            let ctx = egui::Context::default();
            ctx.enable_accesskit();
            let cc = eframe::CreationContext::_new_kittest(ctx.clone());
            let directory = tempfile::tempdir().unwrap();
            let mut app = Desktop::new(&cc, directory.path().join("servers.json"));
            for section in ["overview", "source", "build", "resources", "environment"] {
                app.open(super::super::page(
                    PageKind::Service,
                    &Target {
                        project: "operations".into(),
                        service: "long-service-identity-to-overflow-the-tab-strip".into(),
                        service_type: "app".into(),
                        ..Default::default()
                    },
                    section,
                    section,
                ));
            }
            app.open(super::super::page(
                PageKind::Host,
                &Target {
                    server: Some(crate::config::Server {
                        name: "alpha".into(),
                        url: "https://panel.invalid".into(),
                        token: "fixture-token".into(),
                        default: false,
                        ssh: None,
                    }),
                    ..Default::default()
                },
                "domains",
                "Domains",
            ));
            let id = app.active;
            let index = app.documents.len() - 1;
            let original = app.documents[index].page.target.clone();
            frame(&ctx, &mut app, width, Vec::new());
            let (before, viewport) = frame(&ctx, &mut app, width, Vec::new());
            let old_caption = app.documents[index].caption();
            assert!(viewport.contains_rect(active_tab(&before, &old_caption)));
            let PageState::Host(state) = &mut app.documents[index].state else {
                unreachable!();
            };
            state.section = "notifications".into();
            let caption = app.documents[index].caption();
            frame(&ctx, &mut app, width, Vec::new());
            let (after, viewport) = frame(&ctx, &mut app, width, Vec::new());
            let grown = active_tab(&after, &caption);
            assert!(grown.width() > active_tab(&before, &old_caption).width());
            assert!(viewport.contains_rect(grown), "{width}: {grown:?}");
            frame(
                &ctx,
                &mut app,
                width,
                vec![
                    egui::Event::PointerMoved(egui::pos2(500.0, 16.0)),
                    egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        delta: egui::vec2(2000.0, 0.0),
                        phase: egui::TouchPhase::Move,
                        source: egui::MouseWheelSource::Wheel,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
            for _ in 0..30 {
                frame(&ctx, &mut app, width, Vec::new());
            }
            let (browsed, viewport) = frame(&ctx, &mut app, width, Vec::new());
            assert!(!viewport.contains_rect(active_tab(&browsed, &caption)));
            assert_eq!(app.active, id);
            assert!(app.documents[index].page.target.same_resource(&original));
        }
    }

    #[test]
    fn late_watch_save_and_partial_failures_keep_their_task_context_in_new_forms() {
        use super::super::backend::{Backend, Runtime};

        fn settle(state: &mut State, cx: &mut PageContext<'_>, key: &str) {
            let deadline = Instant::now() + Duration::from_secs(5);
            while cx.busy(key) && Instant::now() < deadline {
                cx.backend.pump();
                std::thread::sleep(Duration::from_millis(2));
            }
            assert!(!cx.busy(key));
            state.pump(cx);
        }

        fn render(
            ctx: &egui::Context,
            state: &mut State,
            cx: &mut PageContext<'_>,
            size: egui::Vec2,
        ) -> Vec<String> {
            let mut bounds = egui::Rect::NOTHING;
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |ui| {
                    show(ui, state, cx);
                    bounds = ui.min_rect();
                },
            );
            output.textures_delta.clear();
            assert!(bounds.right() <= screen.right() + 1.0);
            assert!(bounds.bottom() <= screen.bottom() + 1.0);
            output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::epaint::Shape::Text(text) => Some(text.galley.text().to_owned()),
                    _ => None,
                })
                .collect()
        }

        let target = Target {
            server: Some(crate::config::Server {
                name: "host-with-a-long-immutable-connection-name".into(),
                url: "http://127.0.0.1".into(),
                token: "fixture-token".into(),
                default: false,
                ssh: None,
            }),
            section: "domains".into(),
            ..Default::default()
        };
        let ctx = egui::Context::default();
        let mut backend = Backend::new(ctx.clone(), Runtime::default());
        let mut opened = Vec::new();
        let mut reload = false;
        let path = std::path::Path::new("unused-host-task-fixture");
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
            section: "uptime".into(),
            watch_requested: true,
            backup_project: "shop".into(),
            backup_service: "db".into(),
            backup_database: "orders".into(),
            backup_jobs: vec![("still-running".into(), "orders".into())],
            backup_outcomes: vec![vec!["older-result".into()]],
            message: "earlier-success".into(),
            message_section: "Uptime",
            ..Default::default()
        };
        cx.background("watch-save", "Fixture watch save", || {
            Ok(Payload::Json(json!([])))
        });
        schedule_editor(&mut state, None);
        assert!(state.message.is_empty());
        settle(&mut state, &mut cx, "watch-save");
        let late_notice = state.message.clone();
        assert!(!late_notice.is_empty());
        assert_eq!(state.message_section, "Uptime");
        assert!(state.editor.is_some());
        for size in [egui::vec2(727.0, 480.0), egui::vec2(983.0, 600.0)] {
            let text = render(&ctx, &mut state, &mut cx, size);
            assert!(!text.iter().any(|line| line.contains(late_notice.as_str())));
            assert_eq!(
                state.editor.as_ref().unwrap().form.by_label("Database"),
                "orders"
            );
        }
        cx.background("host-mutation", "Fixture failed request", || {
            anyhow::bail!("uncertain-operation")
        });
        settle(&mut state, &mut cx, "host-mutation");
        assert!(
            state.message.is_empty(),
            "An earlier success cannot describe the failed operation"
        );
        assert!(state.mutation_error.contains("uncertain-operation"));

        state.mutation_section = "Domains";
        cx.background("host-mutation", "Fixture partial request", || {
            Ok(Payload::Worker(Resp::DomainsEdited {
                ok: 1,
                failed: vec![("failed-domain".into(), "unavailable".into())],
            }))
        });
        settle(&mut state, &mut cx, "host-mutation");
        let partial = state.message.clone();
        let failure = state.mutation_error.clone();
        assert!(state.message_partial);
        schedule_editor(&mut state, None);
        let text = render(&ctx, &mut state, &mut cx, egui::vec2(727.0, 480.0));
        assert!(text.iter().any(|line| line.contains(partial.as_str())));
        assert_eq!(state.mutation_error, failure);
        state.review(
            "Review fixture",
            "immutable target".into(),
            "fixed-host".into(),
            Apply::Shell,
        );
        assert_eq!(state.message, partial);
        assert_eq!(state.mutation_error, failure);
        assert_eq!(state.backup_jobs.len(), 1);
        assert_eq!(state.backup_outcomes, vec![vec!["older-result"]]);
    }

    #[test]
    fn fleet_missing_metrics_are_unknown_not_zero() {
        assert_eq!(metric(None, "cpu", true), "Unknown");
        assert_eq!(metric(Some(&json!({"cpu":[]})), "cpu", true), "Unknown");
        assert_eq!(
            metric(Some(&json!({"cpu":[[100,"0"]]})), "cpu", true),
            "0.0%"
        );
        assert_eq!(
            metric(Some(&json!({"cpu":[[100,"NaN"]]})), "cpu", true),
            "Unknown"
        );
    }

    #[test]
    fn remote_restore_history_rejects_failed_local_and_unidentified_storage() {
        let action = |status: &str, provider: &str| json!({"type":"backup","status":status,"projectName":"source","serviceName":"db","meta":{"databaseName":"shop","path":"shop/dump.sql.gz","storageProviderId":provider}});
        let actions = vec![
            action("done", "remote"),
            action("done", "local"),
            action("done", "missing"),
            action("error", "remote"),
        ];
        let providers = vec![
            json!({"id":"remote","type":"s3"}),
            json!({"id":"local","type":"local"}),
        ];
        let (files, hidden) = restorable_history(&actions, &providers, true);
        assert_eq!(files.len(), 1);
        assert_eq!(hidden, 2);
        assert_eq!(files[0].1.storage_provider_id, "remote");
        assert_eq!(restorable_history(&actions, &providers, false).0.len(), 3);
    }

    #[test]
    fn only_running_actions_offer_remote_kill() {
        assert!(action_is_running(&json!({"status":"running"})));
        for status in ["done", "error", "queued", ""] {
            assert!(!action_is_running(&json!({"status":status})));
        }
        assert!(!action_is_running(&json!({})));
    }

    #[test]
    fn editor_dirty_state_tracks_actual_values_and_discard_baseline() {
        let mut editor = Editor::new(
            EditKind::Domain(None),
            Form::new(
                FormKind::DomainCreate,
                "Domain",
                vec![Field::text("Host", "a.test")],
            ),
        );
        assert!(!editor.dirty());
        editor.form.fields[0].value = "b.test".into();
        assert!(editor.dirty());
        editor.form.fields[0].value = "a.test".into();
        assert!(!editor.dirty());
    }

    #[test]
    fn cancelling_each_host_editor_preserves_dirty_values_until_explicit_discard() {
        for kind in [
            EditKind::Domain(None),
            EditKind::Domain(Some("domain-id".into())),
            EditKind::Check("https://example.test".into()),
            EditKind::Schedule {
                id: None,
                project: "shop".into(),
                service: "db".into(),
            },
            EditKind::Schedule {
                id: Some("schedule-id".into()),
                project: "shop".into(),
                service: "db".into(),
            },
        ] {
            let mut edit = Editor::new(
                kind,
                Form::new(
                    FormKind::DomainCreate,
                    "Editor",
                    vec![Field::text("Value", "original")],
                ),
            );
            assert!(edit.cancel());
            assert!(!edit.discard);
            edit.form.fields[0].value = "draft".into();
            assert!(!edit.cancel());
            assert!(edit.discard);
            assert_eq!(edit.form.fields[0].value, "draft");
            assert_eq!(edit.initial, vec!["original"]);
            edit.discard = false; // Keep editing must not replace the saved baseline.
            assert!(edit.dirty());
            edit.form.fields[0].value = "original".into();
            assert!(edit.cancel());
            assert!(!edit.discard);
        }
    }

    #[test]
    fn domain_draft_recovers_inventory_and_escape_closes_dropdown_before_discard() {
        use super::super::backend::{Backend, Runtime};
        use super::super::model::Target;
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
                        egui::vec2(900.0, 900.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| show(ui, state, cx),
            );
            // Headless frames have no renderer to consume texture uploads.
            output.textures_delta.clear();
            output
        }

        fn text_position(output: &egui::FullOutput, text: &str) -> egui::Pos2 {
            output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::epaint::Shape::Text(shape) if shape.galley.text() == text => {
                        Some(shape.pos + shape.galley.rect.center().to_vec2())
                    }
                    _ => None,
                })
                .unwrap_or_else(|| panic!("Missing rendered text: {text}"))
        }

        fn click(
            ctx: &egui::Context,
            state: &mut State,
            cx: &mut PageContext<'_>,
            pos: egui::Pos2,
        ) {
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

        let server = MockServer::start();
        let inventory = server.mock(|when, then| {
            when.method(POST)
                .path("/api/rpc/projects/listProjectsAndServices");
            then.status(200).json_body(json!({"json":{
                "projects":[{"name":"shop"}],
                "services":[{"projectName":"shop","name":"web"}]
            }}));
        });
        let target = Target {
            server: Some(crate::config::Server {
                name: "fixture".into(),
                url: server.base_url(),
                token: "fixture-token".into(),
                default: false,
                ssh: None,
            }),
            ..Default::default()
        };
        let ctx = egui::Context::default();
        let mut backend = Backend::new(ctx.clone(), Runtime::default());
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
        let mut state = State {
            section: "domains".into(),
            watch_requested: true,
            ..Default::default()
        };
        state.snapshots.insert(
            "inventory".into(),
            Snapshot {
                error: Some("Inventory unavailable".into()),
                requested: true,
                input: Some(Value::Null),
                ..Default::default()
            },
        );
        domain_editor(&mut state, None);
        state.editor.as_mut().unwrap().form.fields[0].value = "draft.example.test".into();

        let output = frame(&ctx, &mut state, &mut cx, vec![]);
        text_position(&output, "Inventory unavailable");
        let retry = text_position(&output, "Refresh inventory");
        click(&ctx, &mut state, &mut cx, retry);
        assert!(cx.busy("inventory"));
        let loading = frame(&ctx, &mut state, &mut cx, vec![]);
        text_position(&loading, "Loading…");
        let deadline = Instant::now() + Duration::from_secs(5);
        while cx.busy("inventory") && Instant::now() < deadline {
            cx.backend.pump();
            std::thread::sleep(Duration::from_millis(2));
        }
        let output = frame(&ctx, &mut state, &mut cx, vec![]);
        assert!(state.snapshots["inventory"].error.is_none());
        let edit = state.editor.as_ref().unwrap();
        assert_eq!(edit.form.by_label("Host"), "draft.example.test");
        assert_eq!(edit.form.by_label("Project"), "shop");
        assert_eq!(edit.form.by_label("Service"), "web");
        inventory.assert_hits(1);

        let dropdown = text_position(&output, "shop");
        click(&ctx, &mut state, &mut cx, dropdown);
        assert!(egui::Popup::is_any_open(&ctx));
        let escape = |pressed| {
            vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }]
        };
        frame(&ctx, &mut state, &mut cx, escape(true));
        assert!(!egui::Popup::is_any_open(&ctx));
        assert!(!state.editor.as_ref().unwrap().discard);
        frame(&ctx, &mut state, &mut cx, escape(false));
        frame(&ctx, &mut state, &mut cx, escape(true));
        let edit = state.editor.as_ref().unwrap();
        assert!(edit.discard);
        assert_eq!(edit.form.by_label("Host"), "draft.example.test");
    }
}

#[cfg(test)]
#[test]
fn incomplete_backup_metadata_is_never_a_restore_candidate() {
    let invalid =
        json!({"type":"backup","status":"done","projectName":"p","serviceName":"db","meta":{}});
    assert!(restorable_history(&[invalid], &[], false).0.is_empty());
}

#[cfg(test)]
#[test]
fn edited_requests_discard_late_results_for_the_previous_headers() {
    let original = crate::uptime::Check::get("https://health.invalid/");
    let mut state = State {
        watch: vec![original.clone()],
        check_inputs: vec![original.clone()],
        ..State::default()
    };
    state.watch[0]
        .headers
        .push(("Authorization".into(), "test-only".into()));
    let probe = crate::uptime::Probe {
        url: original.url.clone(),
        outcome: crate::uptime::Outcome::Failed("old request".into()),
    };
    state.apply_probes(vec![probe.clone()]);
    assert!(
        state.probes.is_empty(),
        "An old response must not describe the newly configured request"
    );
    state.watch[0] = original;
    state.apply_probes(vec![probe]);
    assert_eq!(state.probes.len(), 1);
}

#[cfg(test)]
#[test]
fn newly_ranked_probe_results_clear_the_previously_selected_uptime_row() {
    let a = crate::uptime::Check::get("https://a.invalid/");
    let b = crate::uptime::Check::get("https://b.invalid/");
    let mut state = State {
        watch: vec![a.clone(), b.clone()],
        check_inputs: vec![a.clone(), b.clone()],
        ..State::default()
    };
    state.selected.insert("uptime".into(), Some(0));
    assert_eq!(
        crate::uptime::ranked(&state.watch, &state.probes)[0].0.url,
        a.url
    );
    state.apply_probes(vec![crate::uptime::Probe {
        url: b.url.clone(),
        outcome: crate::uptime::Outcome::Failed("unreachable".into()),
    }]);
    assert_eq!(
        crate::uptime::ranked(&state.watch, &state.probes)[0].0.url,
        b.url
    );
    assert_eq!(
        state.choose("uptime", 2),
        None,
        "The prior selection must not retarget Stop watching to a newly ranked URL"
    );
}

#[cfg(test)]
#[test]
fn saving_or_removing_watched_requests_clears_the_previous_row_selection() {
    let a = crate::uptime::Check::get("https://a.invalid/");
    let b = crate::uptime::Check::get("https://b.invalid/");
    let mut state = State {
        watch: vec![a, b.clone()],
        ..State::default()
    };
    state.selected.insert("uptime".into(), Some(0));
    state.replace_watch(vec![b.clone()]);
    assert_eq!(state.watch[0].url, b.url);
    assert_eq!(
        state.choose("uptime", 1),
        None,
        "A successful watch-save/remove cannot leave an index selecting a different URL"
    );
}

#[cfg(test)]
#[test]
fn metric_filter_preserves_parent_headers_and_matching_project_children() {
    let service = |project: &str, name: &str| json!({"projectName":project,"serviceName":name,"memory":1024,"cpu":1,"networkIn":0,"networkOut":0});
    let values = vec![
        service("alpha", "web"),
        service("alpha", "queue"),
        service("beta", "web"),
    ];
    let (rows, total) = filtered_metrics(&values, "^  web$");
    assert_eq!(total, 5);
    assert_eq!(rows.len(), 4);
    assert!(rows[0][0].starts_with("alpha") || rows[0][0].starts_with("beta"));
    assert_eq!(rows[1][0], "  web");
    assert_eq!(rows[3][0], "  web");
    let (project_rows, _) = filtered_metrics(&values, "^alpha ");
    assert_eq!(project_rows.len(), 3);
    assert!(project_rows[0][0].starts_with("alpha"));
}

#[cfg(test)]
#[test]
fn storage_filter_matches_paths_and_returns_shown_total_counts() {
    let values = vec![
        json!({"projectName":"one","serviceName":"db","size":1024,"path":"/volumes/orders"}),
        json!({"projectName":"two","serviceName":"db","size":2048,"path":"/volumes/archive"}),
    ];
    let (rows, total) = filtered_storage(&values, "orders$");
    assert_eq!(total, 2);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0], "one");
    assert_eq!(
        filtered_storage(&values, "[").0.len(),
        0,
        "Incomplete regex falls back to literal matching"
    );
}

#[cfg(test)]
#[test]
fn action_description_regex_preserves_original_action_identity_with_status_filter() {
    let action = |id: &str, status: &str, description: &str| json!({"id":id,"status":status,"projectName":"shop","serviceName":"api","type":"deploy","description":description,"createdAt":"2026-01-01"});
    let values = vec![
        action("first", "done", "deploy checkout"),
        action("second", "error", "build checkout failed"),
        action("third", "running", "deploy catalogue"),
    ];
    let (indices, rows) = filtered_actions(&values, "^build .* failed$", "!done");
    assert_eq!(indices, vec![1]);
    assert_eq!(rows[0][5], "second");
    assert_eq!(filtered_actions(&values, "^shop/api$", "").0, vec![0, 1, 2]);
}
