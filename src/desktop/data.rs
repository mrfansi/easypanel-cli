//! Target-bound database browsing, transfers and configuration comparisons.
use std::collections::BTreeSet;
use std::path::PathBuf;

use anyhow::{anyhow, bail, Context, Result};
use eframe::egui::{self, RichText};
use serde_json::{json, Value};

use super::model::{OpenPage, PageContext, PageKind, Payload, Target};
use super::widgets;
use crate::client::EasypanelClient;
use crate::config::Server;
use crate::dbms::Level;
use crate::output::field;
use crate::worker::{Req, Resp};

/// Carry recorded backup provenance between documents without serializing credentials.
pub(super) fn backup_restore_page(
    source: &Server,
    mut destination: Target,
    origin: &str,
    file: &crate::backup::BackupFile,
    destination_database: &str,
    destination_provider_id: &str,
) -> OpenPage {
    destination.section = "restore-from".into();
    destination.resource = json!({
        "kind": "recorded-backup-restore",
        "source": connection_identity(source),
        "origin": origin,
        "database": file.database,
        "provider_id": file.storage_provider_id,
        "path": file.path,
        "when": file.when,
        "destination_database": destination_database,
        "destination_provider_id": destination_provider_id,
    });
    OpenPage {
        kind: PageKind::Data,
        title: format!(
            "Restore backup · {}/{}",
            destination.project, destination.service
        ),
        target: destination,
    }
}
/// Compare marked rows using frozen identities, not the explorer's later selection.
pub(super) fn compare_services_page(
    server: &Server,
    a: (String, String, String),
    b: (String, String, String),
) -> OpenPage {
    let title = format!("Compare {}/{} with {}/{}", a.0, a.1, b.0, b.1);
    let target = Target {
        server: Some(server.clone()),
        project: a.0,
        service: a.1,
        service_type: a.2,
        section: "diff".into(),
        resource: json!({"kind":"marked-service-comparison","comparison_target":{"project":b.0,"service":b.1,"service_type":b.2}}),
        ..Target::default()
    };
    OpenPage {
        kind: PageKind::Data,
        target,
        title,
    }
}
fn connection_identity(server: &Server) -> Value {
    json!({
        "name":server.name,"url":server.url,
        "ssh":server.ssh.as_ref().map(|ssh|json!({"host":ssh.host,"port":ssh.port,"user":ssh.user})),
    })
}
#[derive(Clone)]
struct BackupSeed {
    origin: String,
    database: String,
    provider_id: String,
    path: String,
    when: String,
    destination_database: String,
    destination_provider_id: String,
}

#[derive(Clone, Copy, Default, PartialEq)]
enum Mode {
    #[default]
    Database,
    Credentials,
    Dumps,
    Restore,
    Copy,
    Clone,
    Migrate,
    Diff,
    ProjectDiff,
    Export,
    BackupRestore,
}
impl Mode {
    fn from_section(section: &str) -> Self {
        match section {
            "credentials" => Self::Credentials,
            "dumps" => Self::Dumps,
            "restore" => Self::Restore,
            "copy" => Self::Copy,
            "clone" => Self::Clone,
            "migrate" => Self::Migrate,
            "diff" => Self::Diff,
            "project-diff" => Self::ProjectDiff,
            "export-project" => Self::Export,
            "restore-from" => Self::BackupRestore,
            _ => Self::Database,
        }
    }
    fn title(self) -> &'static str {
        match self {
            Self::Database => "Database",
            Self::Credentials => "Credentials",
            Self::Dumps => "Dumps",
            Self::Restore => "Restore dump",
            Self::Copy => "Copy databases",
            Self::Clone => "Clone configuration",
            Self::Migrate => "Migrate configuration",
            Self::Diff => "Compare services",
            Self::ProjectDiff => "Compare projects",
            Self::Export => "Export project",
            Self::BackupRestore => "Restore backup",
        }
    }
}

#[derive(Clone, Default)]
struct Endpoint {
    server: Option<Server>,
    project: String,
    service: String,
    kind: String,
    projects: Vec<String>,
    services: Vec<Service>,
    providers: Vec<Provider>,
    loaded: bool,
    filter: String,
    error: Option<String>,
}
#[derive(Clone)]
struct Service {
    project: String,
    name: String,
    kind: String,
}
#[derive(Clone)]
struct Provider {
    id: String,
    name: String,
    kind: String,
    endpoint: String,
    bucket: String,
}
impl Endpoint {
    fn label(&self) -> String {
        format!(
            "{} / {} / {}",
            self.server.as_ref().map_or("No host", |s| s.name.as_str()),
            self.project,
            self.service
        )
    }
    fn client(&self) -> Result<EasypanelClient> {
        Ok(EasypanelClient::for_server(
            self.server
                .as_ref()
                .ok_or_else(|| anyhow!("Choose a host"))?,
        ))
    }
    fn require_service(&self) -> Result<()> {
        if self.project.is_empty() || self.service.is_empty() || self.kind.is_empty() {
            bail!("Choose an existing project and service");
        }
        Ok(())
    }
    fn provider(&self, name: &str) -> Result<&Provider> {
        self.providers
            .iter()
            .find(|p| {
                p.name == name
                    && crate::backup::is_named(p.id.trim())
                    && crate::backup::is_named(p.name.trim())
            })
            .ok_or_else(|| anyhow!("Choose a storage provider available on {}", self.label()))
    }
}

#[derive(Default)]
struct Browser {
    loaded: bool,
    database: String,
    table: String,
    level: Option<Level>,
    columns: Vec<String>,
    rows: Vec<Vec<String>>,
    selected: Option<usize>,
    query: String,
    query_database: String,
    result_query: String,
    pending_query: Option<String>,
    saved_query: String,
    saving_query: Option<String>,
    result_label: String,
    capped: bool,
    truncated: bool,
    error: Option<String>,
    credentials: Option<Vec<crate::credentials::Cred>>,
    credentials_loaded: bool,
    revealed: BTreeSet<usize>,
}
impl Browser {
    fn refresh_request(&self) -> (Level, String, String, String) {
        (
            self.level.unwrap_or(Level::Databases),
            self.database.clone(),
            self.table.clone(),
            self.result_query.clone(),
        )
    }
    fn open_request(&self, index: usize) -> Option<(Level, String, String)> {
        let name = self.rows.get(index)?.first()?.clone();
        match self.level? {
            Level::Databases => Some((Level::Tables, name, String::new())),
            Level::Tables => Some((Level::Rows, self.database.clone(), name)),
            _ => None,
        }
    }
    fn begin_request(&mut self, level: Level, query: &str) {
        self.pending_query = (level == Level::Query).then(|| query.to_owned());
        self.error = None;
    }
    fn file_saved(&mut self) {
        if let Some(saved) = self.saving_query.take() {
            self.saved_query = saved;
        }
    }
}
#[derive(Clone)]
struct SavedDump {
    key: String,
    provider: String,
    origin: String,
}
#[derive(Clone)]
struct SavedBackup {
    database: String,
    provider: Provider,
    path: String,
    origin: String,
    when: String,
    kind: String,
}

#[derive(Clone)]
enum Job {
    Dump {
        databases: Vec<String>,
    },
    Copy {
        databases: Vec<String>,
    },
    Restore {
        dump: SavedDump,
    },
    Backup {
        file: Box<SavedBackup>,
        database: String,
        provider: String,
        original_engine: String,
    },
    Clone,
    Migrate {
        services: Vec<Service>,
    },
}
#[derive(Clone)]
struct Review {
    source: Endpoint,
    destination: Endpoint,
    provider: String,
    facts: Vec<(String, String)>,
    copy_plan: Option<crate::operations::CopyPlan>,
    job: Job,
}

#[derive(Default)]
pub(super) struct State {
    initialized: bool,
    mode: Mode,
    source: Endpoint,
    destination: Endpoint,
    seeded_endpoints: bool,
    seed: Option<BackupSeed>,
    browser: Browser,
    databases: Vec<String>,
    chosen_databases: BTreeSet<String>,
    all_databases: bool,
    provider: String,
    destination_provider: String,
    all_services: bool,
    manual_dump: String,
    manual_backup_path: String,
    manual_backup_provider: String,
    dumps: Vec<SavedDump>,
    backups: Vec<SavedBackup>,
    selected_dump: Option<usize>,
    selected_backup: Option<usize>,
    destination_database: String,
    backup_engine: String,
    backup_engine_confirmed: bool,
    new_project: String,
    new_service: String,
    whole_project: bool,
    review: Option<Review>,
    pending_review: Option<Review>,
    confirmation: String,
    comparison: Vec<Vec<String>>,
    comparison_label: String,
    compared: bool,
    export: Option<String>,
    outcome: Option<String>,
    error: Option<String>,
    draft: bool,
}
impl State {
    pub(super) fn is_dirty(&self) -> bool {
        self.browser.query != self.browser.saved_query || self.draft || self.review.is_some()
    }
}
fn initialize_seed(state: &mut State, target: &Target, servers: &[Server]) -> Result<()> {
    let resource = &target.resource;
    if resource.get("kind").and_then(Value::as_str) == Some("marked-service-comparison") {
        state.seeded_endpoints = true;
        state.source = Endpoint {
            server: target.server.clone(),
            project: target.project.clone(),
            service: target.service.clone(),
            kind: target.service_type.clone(),
            loaded: true,
            ..Endpoint::default()
        };
        for key in [
            "/comparison_target/project",
            "/comparison_target/service",
            "/comparison_target/service_type",
        ] {
            if !named_json(resource, key) {
                bail!("Marked comparison is missing {key}");
            }
        }
        state.destination = Endpoint {
            server: target.server.clone(),
            project: field(resource, "/comparison_target/project"),
            service: field(resource, "/comparison_target/service"),
            kind: field(resource, "/comparison_target/service_type"),
            loaded: true,
            ..Endpoint::default()
        };
        state.source.require_service()?;
        state.destination.require_service()?;
        return Ok(());
    }
    if resource.get("kind").and_then(Value::as_str) != Some("recorded-backup-restore") {
        return Ok(());
    }
    state.seeded_endpoints = true;
    let identity = resource
        .get("source")
        .ok_or_else(|| anyhow!("Recorded backup has no source connection identity"))?;
    let server=servers.iter().find(|server|connection_identity(server)==*identity)
        .ok_or_else(||anyhow!("The backup source connection was removed or retargeted. Reopen the selected backup from its source history"))?;
    let origin = field(resource, "/origin");
    let (project, service) = origin
        .split_once('/')
        .filter(|(p, s)| !p.is_empty() && !s.is_empty())
        .ok_or_else(|| anyhow!("Recorded backup has no valid source project/service"))?;
    for key in [
        "database",
        "provider_id",
        "path",
        "destination_database",
        "destination_provider_id",
    ] {
        if !named_json(resource, &format!("/{key}")) {
            bail!("Recorded restore is missing {key}");
        }
    }
    state.source = Endpoint {
        server: Some(server.clone()),
        project: project.into(),
        service: service.into(),
        ..Endpoint::default()
    };
    state.destination_database = field(resource, "/destination_database");
    state.seed = Some(BackupSeed {
        origin,
        database: field(resource, "/database"),
        provider_id: field(resource, "/provider_id"),
        path: field(resource, "/path"),
        when: field(resource, "/when"),
        destination_database: field(resource, "/destination_database"),
        destination_provider_id: field(resource, "/destination_provider_id"),
    });
    Ok(())
}
fn hydrate_seed(state: &mut State, cx: &mut PageContext<'_>) {
    if !state.source.loaded
        || !state.destination.loaded
        || cx.busy("source-inventory")
        || cx.busy("target-inventory")
    {
        return;
    }
    let Some(seed) = state.seed.as_ref() else {
        return;
    };
    let Some(provider) = state
        .source
        .providers
        .iter()
        .find(|p| p.id == seed.provider_id)
        .cloned()
    else {
        state.error=Some("The recorded source provider is missing or could not be loaded. Refresh source inventory/providers before restoring.".into());
        return;
    };
    let Some(destination_provider) = state
        .destination
        .providers
        .iter()
        .find(|p| p.id == seed.destination_provider_id)
    else {
        state.error=Some("The selected destination provider is missing or could not be loaded. Reopen restore from the host backup view after choosing an existing provider.".into());
        return;
    };
    let kind = state
        .source
        .services
        .iter()
        .find(|s| format!("{}/{}", s.project, s.name) == seed.origin)
        .map(|s| s.kind.clone())
        .unwrap_or_default();
    state
        .destination_provider
        .clone_from(&destination_provider.name);
    state
        .destination_database
        .clone_from(&seed.destination_database);
    state.backup_engine.clear();
    state.backup_engine_confirmed = false;
    state.backups = vec![SavedBackup {
        database: seed.database.clone(),
        provider,
        path: seed.path.clone(),
        origin: seed.origin.clone(),
        when: seed.when.clone(),
        kind: kind.clone(),
    }];
    state.selected_backup = Some(0);
    state.seed = None;
    state.error = None;
    state.draft = true;
}

pub(super) fn show(ui: &mut egui::Ui, state: &mut State, cx: &mut PageContext<'_>) {
    if !state.initialized {
        state.initialized = true;
        state.mode = Mode::from_section(&cx.target.section);
        state.source.server = cx.target.server.clone();
        state.source.project.clone_from(&cx.target.project);
        state.source.service.clone_from(&cx.target.service);
        state.source.kind.clone_from(&cx.target.service_type);
        state.destination = state.source.clone();
        state.new_project.clone_from(&cx.target.project);
        if let Err(error) = initialize_seed(state, cx.target, cx.servers) {
            state.error = Some(error.to_string());
        }
    }
    receive(state, cx);
    hydrate_seed(state, cx);
    ui.horizontal_wrapped(|ui| {
        ui.heading(state.mode.title());
        if ui
            .add_enabled(
                matches!(
                    cx.target.service_type.as_str(),
                    "mysql" | "mariadb" | "postgres" | "mongo" | "redis"
                ),
                egui::Button::new(format!(
                    "DB terminal: {}/{}",
                    cx.target.project, cx.target.service
                )),
            )
            .clicked()
        {
            let mut target = cx.target.clone();
            target.section = "db".into();
            cx.open(OpenPage {
                kind: PageKind::Terminal,
                title: format!("DB terminal · {}", target.service),
                target,
            });
        }
    });
    if !state.seeded_endpoints {
        ui.horizontal_wrapped(|ui| {
            for mode in [
                Mode::Database,
                Mode::Credentials,
                Mode::Dumps,
                Mode::Restore,
                Mode::Copy,
                Mode::BackupRestore,
                Mode::Clone,
                Mode::Migrate,
                Mode::Diff,
                Mode::ProjectDiff,
                Mode::Export,
            ] {
                if ui
                    .add_enabled(
                        !task_busy(cx) && state.review.is_none(),
                        egui::Button::new(mode.title()).selected(state.mode == mode),
                    )
                    .clicked()
                {
                    state.mode = mode;
                }
            }
        });
    }
    ui.separator();
    if let Some(error) = &state.error {
        egui::ScrollArea::vertical()
            .id_salt("data-error")
            .max_height(120.0)
            .show(ui, |ui| {
                widgets::error(ui, error);
            });
    }
    if let Some(outcome) = &state.outcome {
        egui::CollapsingHeader::new("Last operation outcome")
            .default_open(true)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("data-outcome")
                    .max_height(160.0)
                    .show(ui, |ui| {
                        ui.add(egui::Label::new(outcome).wrap());
                    });
            });
    }
    if task_busy(cx) {
        ui.horizontal_wrapped(|ui| {
            ui.spinner();
            ui.label(
                cx.progress("execute")
                    .or_else(|| cx.progress("download"))
                    .or_else(|| cx.progress("prepare"))
                    .unwrap_or("Working with the displayed endpoints…"),
            );
        });
        ui.small("Closing this view or leaving the workspace does not cancel remote work. A timed-out restore may still be writing; inspect the target before retrying.");
    }
    match state.mode {
        Mode::Database => database(ui, state, cx),
        Mode::Credentials => credentials(ui, state, cx),
        _ => {
            egui::ScrollArea::vertical()
                .id_salt("data-workspace")
                .show(ui, |ui| {
                    transfer(ui, state, cx);
                });
        }
    }
}
fn task_busy(cx: &PageContext<'_>) -> bool {
    [
        "prepare",
        "execute",
        "download",
        "export-save",
        "source-inventory",
        "target-inventory",
        "database-list",
        "dump-list",
        "backup-list",
        "comparison",
        "export",
    ]
    .iter()
    .any(|key| cx.busy(key))
}

fn db_request(
    state: &mut State,
    cx: &mut PageContext<'_>,
    level: Level,
    database: String,
    table: String,
    query: String,
) {
    state.browser.begin_request(level, &query);
    cx.request(
        "browser",
        Req::Dbms {
            project: cx.target.project.clone(),
            service: cx.target.service.clone(),
            stype: cx.target.service_type.clone(),
            level,
            database,
            table,
            query,
        },
    );
}
fn open_browser_row(state: &mut State, cx: &mut PageContext<'_>, index: usize) {
    if let Some((level, database, table)) = state.browser.open_request(index) {
        db_request(state, cx, level, database, table, String::new());
    }
}
fn database(ui: &mut egui::Ui, state: &mut State, cx: &mut PageContext<'_>) {
    if let Some(reason) = crate::dbms::unsupported_reason(&cx.target.service_type) {
        ui.add(egui::Label::new(reason).wrap());
        return;
    }
    if !state.browser.loaded {
        state.browser.loaded = true;
        db_request(
            state,
            cx,
            Level::Databases,
            String::new(),
            String::new(),
            String::new(),
        );
    }
    ui.horizontal_wrapped(|ui| {
        ui.label(format!(
            "{} · {}",
            cx.target.service_type, cx.target.service
        ));
        if ui
            .add_enabled(!cx.busy("browser"), egui::Button::new("Databases"))
            .clicked()
        {
            db_request(
                state,
                cx,
                Level::Databases,
                String::new(),
                String::new(),
                String::new(),
            );
        }
        if !state.browser.database.is_empty()
            && ui
                .add_enabled(
                    !cx.busy("browser"),
                    egui::Button::new(format!(
                        "{} in {}",
                        if cx.target.service_type == "mongo" {
                            "Collections"
                        } else {
                            "Tables"
                        },
                        state.browser.database
                    )),
                )
                .clicked()
        {
            let database = state.browser.database.clone();
            db_request(
                state,
                cx,
                Level::Tables,
                database,
                String::new(),
                String::new(),
            );
        }
        if ui
            .add_enabled(!cx.busy("browser"), egui::Button::new("Refresh"))
            .clicked()
        {
            let (level, database, table, query) = state.browser.refresh_request();
            db_request(state, cx, level, database, table, query);
        }
        if cx.busy("browser") {
            ui.spinner();
            ui.label("Running engine client…");
        }
    });
    ui.horizontal(|ui| {
        ui.label("Query database");
        ui.add_enabled(
            !cx.busy("browser"),
            egui::TextEdit::singleline(&mut state.browser.query_database),
        );
    });
    ui.small("SQL / mongosh runs exactly as entered with the database account's privileges. Statements can change or delete data; no LIMIT is added.");
    ui.add(
        egui::TextEdit::multiline(&mut state.browser.query)
            .code_editor()
            .lock_focus(false)
            .desired_rows(5)
            .desired_width(f32::INFINITY),
    );
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                !cx.busy("browser") && !state.browser.query.trim().is_empty(),
                egui::Button::new("Run statement"),
            )
            .clicked()
        {
            let database = state.browser.query_database.clone();
            let query = state.browser.query.clone();
            db_request(state, cx, Level::Query, database, String::new(), query);
        }
        if ui.button("Discard query edits").clicked() {
            state.browser.query.clone_from(&state.browser.saved_query);
        }
        if ui
            .add_enabled(!cx.busy("query-save"), egui::Button::new("Save query…"))
            .clicked()
        {
            if let Some(path) = rfd::FileDialog::new()
                .set_title("Save database query")
                .set_file_name("query.sql")
                .save_file()
            {
                let text = state.browser.query.clone();
                state.browser.saving_query = Some(text.clone());
                cx.background("query-save", "Save query", move || {
                    write_new_file(&path, text.as_bytes())?;
                    Ok(Payload::Text("Query saved".into()))
                });
            }
        }
    });
    if let Some(error) = &state.browser.error {
        egui::ScrollArea::vertical()
            .id_salt("query-error")
            .max_height(100.0)
            .show(ui, |ui| {
                widgets::error(ui, error);
            });
        ui.small("Previous successful result retained below.");
    }
    ui.separator();
    ui.label(&state.browser.result_label);
    if state.browser.capped {
        ui.colored_label(
            ui.visuals().warn_fg_color,
            format!(
                "Preview reached the {}-row cap; more rows may exist.",
                crate::dbms::PREVIEW_LIMIT
            ),
        );
    }
    if state.browser.truncated {
        ui.colored_label(
            ui.visuals().warn_fg_color,
            "Output exceeded the 256 KiB capture budget and is incomplete.",
        );
    }
    let can_open = matches!(state.browser.level, Some(Level::Databases | Level::Tables));
    if can_open {
        ui.small("Select a database, table or collection, then Open. Arrow keys select; Enter activates selection.");
        if ui
            .add_enabled(
                !cx.busy("browser") && state.browser.selected.is_some(),
                egui::Button::new("Open selected"),
            )
            .clicked()
        {
            if let Some(index) = state.browser.selected {
                open_browser_row(state, cx, index);
            }
        }
    }
    if !can_open {
        if let Some(row) = state
            .browser
            .selected
            .and_then(|i| state.browser.rows.get(i))
        {
            egui::CollapsingHeader::new("Selected row values").show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("selected-row-values")
                    .max_height(150.0)
                    .show(ui, |ui| {
                        for (index, value) in row.iter().enumerate() {
                            ui.label(
                                RichText::new(
                                    state
                                        .browser
                                        .columns
                                        .get(index)
                                        .map_or("Value", String::as_str),
                                )
                                .strong(),
                            );
                            ui.add(egui::Label::new(value).wrap());
                            if ui
                                .small_button(format!("Copy value {}", index + 1))
                                .clicked()
                            {
                                ui.ctx().copy_text(value.clone());
                            }
                        }
                    });
            });
        }
    }
    let headers: Vec<&str> = state.browser.columns.iter().map(String::as_str).collect();
    widgets::table(
        ui,
        "query-results",
        &headers,
        &state.browser.rows,
        &mut state.browser.selected,
    );
    if !cx.busy("browser") {
        if let Some(index) = widgets::table_activation(ui, "query-results") {
            open_browser_row(state, cx, index);
        }
    }
}
fn credentials(ui: &mut egui::Ui, state: &mut State, cx: &mut PageContext<'_>) {
    if !matches!(
        cx.target.service_type.as_str(),
        "mysql" | "mariadb" | "postgres" | "mongo" | "redis"
    ) {
        ui.label("Select a database service to view its connection credentials.");
        return;
    }
    if !state.browser.credentials_loaded && !cx.busy("credentials") {
        state.browser.credentials_loaded = true;
        cx.rpc(
            "credentials",
            &format!("services/{}", cx.target.service_type),
            "inspectService",
            json!({"projectName":cx.target.project,"serviceName":cx.target.service}),
        );
    }
    if ui
        .add_enabled(
            !cx.busy("credentials"),
            egui::Button::new("Refresh credentials"),
        )
        .clicked()
    {
        state.browser.revealed.clear();
        cx.rpc(
            "credentials",
            &format!("services/{}", cx.target.service_type),
            "inspectService",
            json!({"projectName":cx.target.project,"serviceName":cx.target.service}),
        );
    }
    ui.small("Internal addresses resolve inside the EasyPanel network. Copy deliberately places the real value on the system clipboard.");
    if let Some(creds) = &state.browser.credentials {
        egui::Grid::new("database-credentials")
            .num_columns(4)
            .spacing([16.0, 12.0])
            .show(ui, |ui| {
                for (index, cred) in creds.iter().enumerate() {
                    ui.label(&cred.label);
                    let reveal = state.browser.revealed.contains(&index);
                    ui.add(
                        egui::Label::new(if cred.secret && !reveal {
                            "••••••••"
                        } else {
                            &cred.value
                        })
                        .wrap(),
                    );
                    if cred.secret {
                        if ui.button(if reveal { "Hide" } else { "Reveal" }).clicked() {
                            if reveal {
                                state.browser.revealed.remove(&index);
                            } else {
                                state.browser.revealed.insert(index);
                            }
                        }
                    } else {
                        ui.label("");
                    }
                    if ui.button("Copy").clicked() {
                        ui.ctx().copy_text(cred.value.clone());
                    }
                    ui.end_row();
                }
            });
    } else if cx.busy("credentials") {
        ui.spinner();
    }
}

fn load_endpoint(endpoint: &mut Endpoint, cx: &mut PageContext<'_>, key: &'static str) {
    let Some(server) = endpoint.server.clone() else {
        return;
    };
    endpoint.loaded = true;
    endpoint.error = None;
    cx.background(key, "Load endpoint inventory", move || {
        let client = EasypanelClient::for_server(&server);
        let inventory = client.call("projects", "listProjectsAndServices", Value::Null)?;
        // Never retain provider credentials or secret-bearing inventory fields in picker data.
        let services: Vec<Value> = inventory.get("services").and_then(Value::as_array).into_iter().flatten().map(|s| json!({"project":field(s,"/projectName"),"name":field(s,"/name"),"kind":field(s,"/type")})).collect();
        let projects: Vec<String> = inventory.get("projects").and_then(Value::as_array).into_iter().flatten().filter_map(|p| p.as_str().map(str::to_owned).or_else(|| p.get("name").and_then(Value::as_str).map(str::to_owned))).collect();
        let providers = match client.call("storageProviders/common", "list", Value::Null) {
            Ok(v) => json!({"rows":v.as_array().into_iter().flatten().map(|p| json!({"id":field(p,"/id"),"name":field(p,"/name"),"type":field(p,"/type"),"endpoint":field(p,"/endpoint"),"bucket":field(p,"/bucket")})).collect::<Vec<_>>()}),
            Err(e) => json!({"rows":[],"error":e.to_string()}),
        };
        Ok(Payload::Json(json!({"projects":projects,"services":services,"providers":providers})))
    });
}
fn endpoint_ui(
    ui: &mut egui::Ui,
    endpoint: &mut Endpoint,
    cx: &mut PageContext<'_>,
    key: &'static str,
    title: &str,
    service: bool,
    enabled: bool,
) -> bool {
    let mut changed = false;
    ui.push_id(key, |ui| {
        ui.label(RichText::new(title).strong());
        ui.add_enabled_ui(enabled && !cx.busy(key), |ui| {
            egui::ComboBox::from_id_salt("host")
                .width(ui.available_width())
                .selected_text(
                    endpoint
                        .server
                        .as_ref()
                        .map_or("Choose host", |s| s.name.as_str()),
                )
                .show_ui(ui, |ui| {
                    for server in cx.servers {
                        if ui
                            .selectable_label(
                                endpoint
                                    .server
                                    .as_ref()
                                    .is_some_and(|s| s.name == server.name && s.url == server.url),
                                &server.name,
                            )
                            .clicked()
                        {
                            endpoint.server = Some(server.clone());
                            endpoint.loaded = false;
                            endpoint.projects.clear();
                            endpoint.services.clear();
                            endpoint.providers.clear();
                            endpoint.project.clear();
                            endpoint.service.clear();
                            endpoint.kind.clear();
                            changed = true;
                        }
                    }
                });
            ui.horizontal(|ui| {
                ui.label("Project");
                egui::ComboBox::from_id_salt("project")
                    .selected_text(if endpoint.project.is_empty() {
                        "Choose project"
                    } else {
                        &endpoint.project
                    })
                    .show_ui(ui, |ui| {
                        for project in &endpoint.projects {
                            if ui
                                .selectable_label(endpoint.project == *project, project)
                                .clicked()
                            {
                                endpoint.project.clone_from(project);
                                endpoint.service.clear();
                                endpoint.kind.clear();
                                changed = true;
                            }
                        }
                    });
            });
            if service {
                ui.add(
                    egui::TextEdit::singleline(&mut endpoint.filter).hint_text("Filter services"),
                );
                egui::ComboBox::from_id_salt("service")
                    .width(ui.available_width())
                    .selected_text(if endpoint.service.is_empty() {
                        "Choose service"
                    } else {
                        &endpoint.service
                    })
                    .show_ui(ui, |ui| {
                        for item in endpoint.services.iter().filter(|s| {
                            s.project == endpoint.project
                                && s.name
                                    .to_lowercase()
                                    .contains(&endpoint.filter.to_lowercase())
                        }) {
                            if ui
                                .selectable_label(
                                    endpoint.service == item.name,
                                    format!("{} ({})", item.name, item.kind),
                                )
                                .clicked()
                            {
                                endpoint.service.clone_from(&item.name);
                                endpoint.kind.clone_from(&item.kind);
                                changed = true;
                            }
                        }
                    });
            }
            if ui
                .add_enabled(
                    !cx.busy(key),
                    egui::Button::new("Refresh inventory / providers"),
                )
                .clicked()
            {
                load_endpoint(endpoint, cx, key);
            }
        });
        if !endpoint.loaded && !cx.busy(key) {
            load_endpoint(endpoint, cx, key);
        }
        if cx.busy(key) {
            ui.spinner();
        }
        if let Some(error) = &endpoint.error {
            widgets::error(ui, error);
        }
        ui.small(endpoint.label());
    });
    changed
}
fn transfer(ui: &mut egui::Ui, state: &mut State, cx: &mut PageContext<'_>) {
    let locked = task_busy(cx) || state.review.is_some();
    if state.seeded_endpoints {
        ui.small(if state.mode==Mode::Diff {
            "Comparison endpoints are frozen from the marked service selection. Open another comparison to choose different services."
        } else {"Source, destination database and providers are frozen from the selected completed backup. Reopen from history to choose a different destination."});
        if state.seed.is_some()
            && !locked
            && ui.button("Retry loading recorded endpoint facts").clicked()
        {
            load_endpoint(&mut state.source, cx, "source-inventory");
            load_endpoint(&mut state.destination, cx, "target-inventory");
        }
    }
    let dual = matches!(
        state.mode,
        Mode::Copy
            | Mode::Restore
            | Mode::BackupRestore
            | Mode::Migrate
            | Mode::Clone
            | Mode::Diff
            | Mode::ProjectDiff
    );
    let project_only = matches!(state.mode, Mode::ProjectDiff | Mode::Export)
        || state.mode == Mode::Migrate && state.whole_project;
    let mut changed = false;
    ui.columns(if dual { 2 } else { 1 }, |columns| {
        changed |= endpoint_ui(
            &mut columns[0],
            &mut state.source,
            cx,
            "source-inventory",
            "Source",
            !project_only,
            !locked && !state.seeded_endpoints,
        );
        if dual {
            changed |= endpoint_ui(
                &mut columns[1],
                &mut state.destination,
                cx,
                "target-inventory",
                "Destination / comparison target",
                !matches!(state.mode, Mode::Clone | Mode::Migrate | Mode::ProjectDiff),
                !locked && !state.seeded_endpoints,
            );
        }
    });
    if changed {
        state.draft = true;
        state.review = None;
        state.export = None;
        state.compared = false;
        state.dumps.clear();
        state.backups.clear();
        state.selected_dump = None;
        state.selected_backup = None;
        state.databases.clear();
        state.chosen_databases.clear();
        state.confirmation.clear();
    }
    ui.separator();
    ui.add_enabled_ui(!locked, |ui| {
        match state.mode {
            Mode::Dumps | Mode::Copy => {
                ui.small("Non-locking MySQL / MariaDB snapshots use --single-transaction and do not lock tables. Other engines use the panel backup workflow.");
                state.draft |= ui.checkbox(&mut state.all_databases, "All non-system databases (resolved during preflight)").changed();
                if !state.all_databases {
                    if ui.add_enabled(!cx.busy("database-list"), egui::Button::new("Load database choices")).clicked() { load_databases(state, cx); }
                    for name in &state.databases {
                        let mut chosen = state.chosen_databases.contains(name);
                        if ui.checkbox(&mut chosen, name).changed() { if chosen { state.chosen_databases.insert(name.clone()); } else { state.chosen_databases.remove(name); } state.draft = true; }
                    }
                }
                provider_ui(ui, state, true);
                if ui.button(if state.mode == Mode::Copy { "Preflight database copy" } else { "Review new dump" }).clicked() {
                    prepare(state, cx);
                }
                if state.mode == Mode::Dumps { dump_browser(ui, state, cx, false); }
            }
            Mode::Restore => {
                provider_ui(ui, state, true);
                destination_provider_ui(ui, state, true);
                dump_browser(ui, state, cx, true);
            }
            Mode::BackupRestore => {
                destination_provider_ui(ui, state, false);
                backup_browser(ui, state, cx);
            }
            Mode::Clone | Mode::Migrate => {
                ui.label(RichText::new("Configuration only — no database rows, volume contents or uploaded files are copied.").strong());
                if state.mode == Mode::Migrate { state.draft |= ui.checkbox(&mut state.whole_project, "Migrate every service in the source project").changed(); }
                ui.label("Destination project"); state.draft |= ui.text_edit_singleline(&mut state.new_project).changed();
                if state.mode == Mode::Clone { ui.label("New service name"); state.draft |= ui.text_edit_singleline(&mut state.new_service).changed(); }
                ui.small(if state.mode == Mode::Clone { "Clone uses an existing destination project on the same host and does not copy domains." } else { "Migration creates the destination project when missing, copies domains and reports each service's partial outcomes. DNS and data stay at the source." });
                if ui.button("Review configuration transfer").clicked() { prepare(state, cx); }
            }
            Mode::Diff | Mode::ProjectDiff => {
                ui.small("Environment values stay hidden. Keys report set, changed or missing; collections use the existing count comparison policy.");
                if ui.button("Compare configurations").clicked() { compare(state, cx); }
            }
            Mode::Export => {
                ui.small("Configuration snapshot only; environment values and named credential fields are redacted. Review opaque scripts and inline commands for embedded secrets before sharing.");
                if ui.button("Load redacted export").clicked() { export(state, cx); }
                if let Some(text) = &state.export {
                    if ui.button("Save redacted project…").clicked() {
                        if let Some(path) = rfd::FileDialog::new().set_title("Export redacted project configuration").set_file_name(format!("{}.easypanel.json", state.source.project)).save_file() {
                            let text = text.clone();
                            cx.background("export-save", "Save redacted project", move || { write_new_file(&path, text.as_bytes())?; Ok(Payload::Text(format!("Redacted configuration saved to {}", path.display()))) });
                        }
                    }
                    ui.label(format!("{} bytes ready to save. Existing files are never overwritten; choose a new name.", text.len()));
                }
            }
            _ => {}
        }
    });
    if state.compared {
        ui.separator();
        ui.label(RichText::new(&state.comparison_label).strong());
        if state.comparison.is_empty() {
            ui.label("Identical in all compared configuration fields. Runtime data and hosts are not compared.");
        } else {
            widgets::table(
                ui,
                "configuration-comparison",
                &["Service / field", "Source", "Target"],
                &state.comparison,
                &mut None,
            );
        }
    }
    review_ui(ui, state, cx);
    if state.draft
        && !state.seeded_endpoints
        && !locked
        && ui.button("Discard transfer choices").clicked()
    {
        state.provider.clear();
        state.chosen_databases.clear();
        state.all_databases = false;
        state.new_project.clone_from(&state.source.project);
        state.new_service.clear();
        state.destination_database.clear();
        state.review = None;
        state.confirmation.clear();
        state.draft = false;
    }
}
fn provider_ui(ui: &mut egui::Ui, state: &mut State, remote: bool) {
    ui.label("Storage provider");
    egui::ComboBox::from_id_salt("transfer-provider")
        .selected_text(if state.provider.is_empty() {
            if state.mode == Mode::Copy {
                "Automatic · each host's sole remote provider"
            } else {
                "Choose provider"
            }
        } else {
            &state.provider
        })
        .show_ui(ui, |ui| {
            if state.mode == Mode::Copy {
                state.draft |= ui
                    .selectable_value(
                        &mut state.provider,
                        String::new(),
                        "Automatic · resolve each host's sole remote provider",
                    )
                    .changed();
            }
            for provider in state
                .source
                .providers
                .iter()
                .filter(|p| !remote || crate::backup::is_remote(&p.kind))
            {
                if ui
                    .selectable_value(
                        &mut state.provider,
                        provider.name.clone(),
                        format!(
                            "{} · {} · {}",
                            provider.name, provider.kind, provider.bucket
                        ),
                    )
                    .changed()
                {
                    state.draft = true;
                    state.dumps.clear();
                    state.selected_dump = None;
                }
            }
        });
}
fn optional_provider(provider: &str) -> Option<&str> {
    (!provider.trim().is_empty()).then_some(provider)
}
fn destination_provider_ui(ui: &mut egui::Ui, state: &mut State, remote: bool) {
    ui.label("Destination storage provider");
    if state.seeded_endpoints {
        ui.label(if state.destination_provider.is_empty() {
            "Loading recorded destination provider…"
        } else {
            &state.destination_provider
        });
        return;
    }
    egui::ComboBox::from_id_salt("destination-provider")
        .selected_text(if state.destination_provider.is_empty() {
            "Choose target provider"
        } else {
            &state.destination_provider
        })
        .show_ui(ui, |ui| {
            for provider in state
                .destination
                .providers
                .iter()
                .filter(|p| !remote || crate::backup::is_remote(&p.kind))
            {
                state.draft |= ui
                    .selectable_value(
                        &mut state.destination_provider,
                        provider.name.clone(),
                        format!(
                            "{} · {} · {}",
                            provider.name, provider.kind, provider.bucket
                        ),
                    )
                    .changed();
            }
        });
}
fn load_databases(state: &mut State, cx: &mut PageContext<'_>) {
    let source = state.source.clone();
    cx.background("database-list", "Read database names", move || {
        source.require_service()?;
        let client = source.client()?;
        let emit: crate::worker::Emit = std::sync::Arc::new(|_| {});
        let response = crate::worker::handle_req(
            &client,
            Req::DatabasesIn {
                project: source.project,
                service: source.service,
                stype: source.kind,
            },
            &emit,
        );
        Ok(Payload::Worker(response))
    });
}
fn dump_browser(ui: &mut egui::Ui, state: &mut State, cx: &mut PageContext<'_>, restore: bool) {
    state.draft |= ui
        .checkbox(
            &mut state.all_services,
            "Include dumps from every service on the source host",
        )
        .changed();
    if ui
        .add_enabled(
            !cx.busy("dump-list"),
            egui::Button::new("List stored dumps"),
        )
        .clicked()
    {
        let source = state.source.clone();
        let provider = state.provider.clone();
        let all = state.all_services;
        cx.background("dump-list", "List stored dumps", move || {
            let client = source.client()?;
            source.provider(&provider)?;
            let keys: Vec<String> = if all {
                crate::operations::list_all_r2_dumps(&client, Some(&provider))?
                    .into_iter()
                    .map(|d| d.key)
                    .collect()
            } else {
                source.require_service()?;
                crate::operations::list_r2_dumps(
                    &client,
                    &source.project,
                    &source.service,
                    Some(&provider),
                )?
            };
            Ok(Payload::Json(json!({"provider":provider,"keys":keys})))
        });
    }
    ui.horizontal(|ui| {
        ui.label("Exact object key");
        state.draft |= ui.text_edit_singleline(&mut state.manual_dump).changed();
        if ui
            .add_enabled(
                !state.manual_dump.trim().is_empty() && !state.provider.is_empty(),
                egui::Button::new("Use this key"),
            )
            .clicked()
        {
            let key = state.manual_dump.trim().to_owned();
            let origin = crate::dump::parse_dump_key(&key)
                .map_or_else(|| "User-specified SQL dump".into(), |d| d.origin());
            state.dumps.push(SavedDump {
                key,
                origin,
                provider: state.provider.clone(),
            });
            state.selected_dump = Some(state.dumps.len() - 1);
        }
    });
    let rows: Vec<Vec<String>> = state
        .dumps
        .iter()
        .map(|d| vec![d.origin.clone(), d.provider.clone(), d.key.clone()])
        .collect();
    egui::ScrollArea::vertical()
        .id_salt("stored-dumps")
        .max_height(220.0)
        .show(ui, |ui| {
            widgets::table(
                ui,
                "dump-files",
                &["Origin", "Provider", "Object key"],
                &rows,
                &mut state.selected_dump,
            );
        });
    if rows.is_empty() {
        ui.small("List dumps to choose an actual stored object; a failed listing keeps the last successful results.");
    }
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                state.selected_dump.is_some() && !cx.busy("download"),
                egui::Button::new("Download selected…"),
            )
            .clicked()
        {
            download(state, cx);
        }
        if restore
            && ui
                .add_enabled(
                    state.selected_dump.is_some(),
                    egui::Button::new("Preflight selected restore"),
                )
                .clicked()
        {
            prepare(state, cx);
        }
    });
}
fn backup_browser(ui: &mut egui::Ui, state: &mut State, cx: &mut PageContext<'_>) {
    ui.small("Choose a successful history entry, or supply a known panel-backup path. These are panel backups, not the separate non-locking SQL-dump format.");
    if ui
        .add_enabled(
            !state.seeded_endpoints && !cx.busy("backup-list"),
            egui::Button::new("Load eligible backup files"),
        )
        .clicked()
    {
        let source = state.source.clone();
        let destination = state.destination.clone();
        cx.background("backup-list", "Read successful backup history", move || {
            let actions=source.client()?.call("actions","listActions",json!({"limit":200}))?;
            let (files,excluded)=eligible_backups(actions.as_array().ok_or_else(||anyhow!("Unexpected action-history response"))?,&source.providers,!same_host(&source,&destination));
            let rows:Vec<Value>=files.into_iter().map(|(origin,f)|{
                let provider=source.providers.iter().find(|p|p.id==f.storage_provider_id).expect("eligibility retained the provider");
                let service=source.services.iter().find(|s|format!("{}/{}",s.project,s.name)==origin);
                json!({"origin":origin,"database":f.database,"provider":provider.name,"path":f.path,"when":f.when,"kind":service.map(|s|s.kind.as_str()).unwrap_or("")})
            }).collect();
            Ok(Payload::Json(json!({"rows":rows,"excluded":excluded})))
        });
    }
    if !state.seeded_endpoints {
        egui::CollapsingHeader::new("Restore an older or known panel-backup path").show(ui,|ui|{
            ui.small("Use the exact file path from its storage provider. Its original engine and creation time cannot be established from a path.");
            ui.label("Source backup provider");
            egui::ComboBox::from_id_salt("known-backup-provider").selected_text(state.source.providers.iter().find(|p|p.id==state.manual_backup_provider).map_or("Choose source provider",|p|p.name.as_str())).show_ui(ui,|ui|{
                for provider in &state.source.providers {
                    state.draft|=ui.selectable_value(&mut state.manual_backup_provider,provider.id.clone(),format!("{} ({})",provider.name,provider.kind)).changed();
                }
            });
            ui.label("Exact panel-backup path");
            state.draft|=ui.text_edit_singleline(&mut state.manual_backup_path).changed();
            if ui.add_enabled(crate::backup::is_named(state.manual_backup_path.trim())&&!state.manual_backup_provider.is_empty(),egui::Button::new("Use known panel-backup path")).clicked() {
                if let Some(provider)=state.source.providers.iter().find(|p|p.id==state.manual_backup_provider).cloned() {
                    state.backups.push(SavedBackup{database:String::new(),provider,path:state.manual_backup_path.clone(),
                        origin:format!("Known path supplied on {}",state.source.server.as_ref().map_or("source host",|s|s.name.as_str())),
                        when:String::new(),kind:state.source.kind.clone()});
                    let index=state.backups.len()-1;
                    select_backup(state,index);
                }
            }
        });
    }
    let rows: Vec<Vec<String>> = state
        .backups
        .iter()
        .map(|f| {
            vec![
                if f.when.is_empty() {
                    "Not recorded".into()
                } else {
                    f.when.clone()
                },
                f.origin.clone(),
                if f.database.is_empty() {
                    "Not recorded".into()
                } else {
                    f.database.clone()
                },
                f.provider.name.clone(),
                f.path.clone(),
            ]
        })
        .collect();
    let previous = state.selected_backup;
    widgets::table(
        ui,
        "successful-backups",
        &["Completed", "Origin", "Source database", "Provider", "File"],
        &rows,
        &mut state.selected_backup,
    );
    if state.selected_backup != previous {
        if let Some(index) = state.selected_backup {
            select_backup(state, index);
        }
    }
    ui.label("Destination database (must already exist for panel backup restore)");
    state.draft |= ui
        .add_enabled(
            !state.seeded_endpoints,
            egui::TextEdit::singleline(&mut state.destination_database),
        )
        .changed();
    if let Some(file) = state.selected_backup.and_then(|i| state.backups.get(i)) {
        ui.small(format!("Origin service's current type: {}. This is an inference from current configuration, NOT the engine that created this file.",
            if file.kind.is_empty(){"not known"}else{&file.kind}));
        ui.label("Verified original backup engine");
        egui::ComboBox::from_id_salt("original-backup-engine")
            .selected_text(if state.backup_engine.is_empty() {
                "Choose verified original engine"
            } else {
                &state.backup_engine
            })
            .show_ui(ui, |ui| {
                for kind in ["mysql", "mariadb", "postgres", "mongo"] {
                    if ui
                        .selectable_value(&mut state.backup_engine, kind.into(), kind)
                        .changed()
                    {
                        state.backup_engine_confirmed = false;
                        state.draft = true;
                    }
                }
            });
        state.draft |= ui
            .checkbox(
                &mut state.backup_engine_confirmed,
                "I verified this file's original engine and its compatibility with the destination",
            )
            .changed();
    }
    if ui
        .add_enabled(
            state.selected_backup.is_some() && state.backup_engine_confirmed,
            egui::Button::new("Preflight backup restore"),
        )
        .clicked()
    {
        prepare(state, cx);
    }
}
fn select_backup(state: &mut State, index: usize) {
    let Some(file) = state.backups.get(index) else {
        return;
    };
    if !state.seeded_endpoints && !file.database.is_empty() {
        state.destination_database.clone_from(&file.database);
    }
    state.selected_backup = Some(index);
    state.backup_engine.clear();
    state.backup_engine_confirmed = false;
    state.draft = true;
}
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct BackupExclusions {
    invalid_metadata: usize,
    missing_provider: usize,
    local_only: usize,
}
fn named_json(value: &Value, pointer: &str) -> bool {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .is_some_and(|s| crate::backup::is_named(s.trim()))
}
fn eligible_backups(
    actions: &[Value],
    providers: &[Provider],
    across: bool,
) -> (Vec<(String, crate::backup::BackupFile)>, BackupExclusions) {
    let mut files = Vec::new();
    let mut excluded = BackupExclusions::default();
    for action in actions.iter().filter(|a| {
        a.get("type").and_then(Value::as_str) == Some("backup")
            && a.get("status").and_then(Value::as_str) == Some("done")
    }) {
        if ![
            "/projectName",
            "/serviceName",
            "/meta/path",
            "/meta/databaseName",
            "/meta/storageProviderId",
        ]
        .iter()
        .all(|pointer| named_json(action, pointer))
        {
            excluded.invalid_metadata += 1;
            continue;
        }
        let Some(file) = crate::backup::history_all(std::slice::from_ref(action)).pop() else {
            excluded.invalid_metadata += 1;
            continue;
        };
        let Some(provider) = providers
            .iter()
            .find(|p| p.id == file.1.storage_provider_id && crate::backup::is_named(p.kind.trim()))
        else {
            excluded.missing_provider += 1;
            continue;
        };
        if across && !crate::backup::is_remote(&provider.kind) {
            excluded.local_only += 1;
            continue;
        }
        files.push(file);
    }
    (files, excluded)
}
fn backup_job(state: &State) -> Result<Job> {
    let file = state
        .selected_backup
        .and_then(|index| state.backups.get(index))
        .cloned()
        .ok_or_else(|| {
            anyhow!("Choose a completed backup or supply its known panel-backup path")
        })?;
    if !state.backup_engine_confirmed || !crate::backup::can_back_up(&state.backup_engine) {
        bail!("Verify the original backup engine and confirm its compatibility before review");
    }
    validate_backup_selection(&file, &state.destination_database)?;
    if !crate::backup::is_named(state.destination_provider.trim()) {
        bail!("Choose a destination provider");
    }
    Ok(Job::Backup {
        file: Box::new(file),
        database: state.destination_database.clone(),
        provider: state.destination_provider.clone(),
        original_engine: state.backup_engine.clone(),
    })
}
fn validate_backup_selection(file: &SavedBackup, database: &str) -> Result<()> {
    if !crate::backup::is_named(database.trim()) {
        bail!("Choose a real destination database");
    }
    if !crate::backup::is_named(file.path.trim())
        || !crate::backup::is_named(file.provider.id.trim())
    {
        bail!("A real backup path and original source provider are required");
    }
    Ok(())
}

fn prepare(state: &mut State, cx: &mut PageContext<'_>) {
    state.error = None;
    state.confirmation.clear();
    let source = state.source.clone();
    let mut destination = state.destination.clone();
    if matches!(state.mode, Mode::Clone | Mode::Migrate) {
        destination.project.clone_from(&state.new_project);
    }
    if state.mode == Mode::Clone {
        destination.service.clone_from(&state.new_service);
        destination.kind.clone_from(&source.kind);
    }
    let job = match state.mode {
        Mode::Dumps => Job::Dump {
            databases: state.chosen_databases.iter().cloned().collect(),
        },
        Mode::Copy => Job::Copy {
            databases: state.chosen_databases.iter().cloned().collect(),
        },
        Mode::Restore => match state
            .selected_dump
            .and_then(|i| state.dumps.get(i))
            .cloned()
        {
            Some(dump) => Job::Restore { dump },
            None => {
                state.error = Some("Choose a stored dump".into());
                return;
            }
        },
        Mode::BackupRestore => match backup_job(state) {
            Ok(job) => job,
            Err(error) => {
                state.error = Some(error.to_string());
                return;
            }
        },
        Mode::Clone => Job::Clone,
        Mode::Migrate => {
            let services = if state.whole_project {
                source
                    .services
                    .iter()
                    .filter(|s| s.project == source.project)
                    .cloned()
                    .collect()
            } else {
                vec![Service {
                    project: source.project.clone(),
                    name: source.service.clone(),
                    kind: source.kind.clone(),
                }]
            };
            Job::Migrate { services }
        }
        _ => return,
    };
    let provider = if state.mode == Mode::Restore {
        state.destination_provider.clone()
    } else {
        state.provider.clone()
    };
    let review = Review {
        source,
        destination,
        provider,
        facts: Vec::new(),
        copy_plan: None,
        job,
    };
    state.pending_review = Some(review.clone());
    let all = state.all_databases;
    cx.background("prepare", "Resolve and validate transfer", move || {
        let mut review = review;
        preflight(&mut review, all)?;
        Ok(Payload::Json(review_facts(&review)))
    });
}
fn same_host(a: &Endpoint, b: &Endpoint) -> bool {
    a.client()
        .ok()
        .zip(b.client().ok())
        .is_some_and(|(a, b)| a.same_configured_target(&b))
}
fn facts_from_plan(plan: &crate::operations::CopyPlan) -> Vec<(String, String)> {
    vec![
        ("Engine".into(), plan.stype.clone()),
        ("Resolved databases".into(), plan.databases.join(", ")),
        ("Source image".into(), plan.src_image.clone()),
        ("Target image".into(), plan.dst_image.clone()),
        ("Shared endpoint".into(), plan.endpoint.clone()),
        ("Bucket".into(), plan.bucket.clone()),
        ("Source provider".into(), plan.src_provider.clone()),
        ("Target provider".into(), plan.dst_provider.clone()),
    ]
}
fn preflight(review: &mut Review, all: bool) -> Result<()> {
    let src = review.source.client()?;
    if matches!(
        review.job,
        Job::Dump { .. } | Job::Restore { .. } | Job::Backup { .. }
    ) {
        review.source.providers = read_providers(&src)?;
    }
    if matches!(review.job, Job::Restore { .. } | Job::Backup { .. }) {
        review.destination.providers = read_providers(&review.destination.client()?)?;
    }
    match &mut review.job {
        Job::Copy { databases } => {
            review.source.require_service()?;
            review.destination.require_service()?;
            if same_host(&review.source, &review.destination)
                && review.source.project == review.destination.project
                && review.source.service == review.destination.service
            {
                bail!("Refusing to copy a database service onto itself");
            }
            let dst = review.destination.client()?;
            let source = crate::operations::CopyTarget {
                client: &src,
                project: &review.source.project,
                service: &review.source.service,
            };
            let target = crate::operations::CopyTarget {
                client: &dst,
                project: &review.destination.project,
                service: &review.destination.service,
            };
            let plan = crate::operations::plan_copy(
                &source,
                &target,
                databases,
                all,
                optional_provider(&review.provider),
            )?;
            review.facts = facts_from_plan(&plan);
            review.copy_plan = Some(plan);
        }
        Job::Dump { databases } => {
            review.source.require_service()?;
            let kind = crate::operations::service_stype(
                &src,
                &review.source.project,
                &review.source.service,
            )?;
            if !crate::dump::can_dump(&kind) {
                bail!("Non-locking dumps require MySQL or MariaDB; selected engine is {kind}");
            }
            if all {
                let names=src.call("databaseBackups","getServiceDatabases",json!({"projectName":review.source.project,"serviceName":review.source.service}))?;
                *databases = names
                    .as_array()
                    .ok_or_else(|| anyhow!("Unexpected database-list response"))?
                    .iter()
                    .filter_map(Value::as_str)
                    .filter(|n| !crate::dump::is_system_db(n))
                    .map(str::to_owned)
                    .collect();
            }
            validate_databases(databases)?;
            let provider = review.source.provider(&review.provider)?;
            if !crate::backup::is_remote(&provider.kind) {
                bail!("Non-locking dumps require a remote provider");
            }
            let inspect = src.call(
                &format!("services/{kind}"),
                "inspectService",
                json!({"projectName":review.source.project,"serviceName":review.source.service}),
            )?;
            review.facts = vec![
                ("Engine".into(), kind),
                ("Source image".into(), field(&inspect, "/image")),
                ("Resolved databases".into(), databases.join(", ")),
                ("Provider".into(), provider.name.clone()),
                (
                    "Endpoint / bucket".into(),
                    format!("{} / {}", provider.endpoint, provider.bucket),
                ),
            ];
        }
        Job::Restore { dump } => {
            review.destination.require_service()?;
            let dst = review.destination.client()?;
            let kind = crate::operations::service_stype(
                &dst,
                &review.destination.project,
                &review.destination.service,
            )?;
            if !crate::dump::can_dump(&kind) {
                bail!("Dump restore requires MySQL or MariaDB; destination is {kind}");
            }
            let source_provider = review.source.provider(&dump.provider)?;
            let target_provider = review.destination.provider(&review.provider)?;
            if let Some(reason) = crate::dump::store_refusal(
                &source_provider.endpoint,
                &source_provider.bucket,
                &target_provider.endpoint,
                &target_provider.bucket,
            ) {
                bail!(reason);
            }
            if !crate::backup::is_remote(&target_provider.kind) {
                bail!("Dump restore requires a remote provider");
            }
            let inspect=dst.call(&format!("services/{kind}"),"inspectService",json!({"projectName":review.destination.project,"serviceName":review.destination.service}))?;
            review.facts=vec![("Engine".into(),kind),("Source image".into(),"Not recorded in stored dump metadata".into()),("Target image".into(),field(&inspect,"/image")),("Dump origin".into(),dump.origin.clone()),("Object key".into(),dump.key.clone()),("Endpoint / bucket".into(),format!("{} / {}",target_provider.endpoint,target_provider.bucket)),("Schemas".into(),"Schemas embedded in this self-contained SQL dump; each named schema is dropped and recreated".into())];
        }
        Job::Backup {
            file,
            database,
            provider,
            original_engine,
        } => {
            review.destination.require_service()?;
            validate_backup_selection(file, database)?;
            if !crate::backup::can_back_up(original_engine) {
                bail!("Choose and verify the original backup engine; action history does not record it");
            }
            let dst = review.destination.client()?;
            let kind = crate::operations::service_stype(
                &dst,
                &review.destination.project,
                &review.destination.service,
            )?;
            let inspect=dst.call(&format!("services/{kind}"),"inspectService",json!({"projectName":review.destination.project,"serviceName":review.destination.service}))?;
            file.provider = review
                .source
                .providers
                .iter()
                .find(|p| p.id == file.provider.id)
                .cloned()
                .ok_or_else(|| anyhow!("The backup's original storage provider disappeared"))?;
            if kind != *original_engine {
                bail!("Operator-confirmed original engine {original_engine} does not match destination {kind}");
            }
            let target = review.destination.provider(provider)?;
            let remote = crate::backup::is_remote(&file.provider.kind)
                && crate::backup::is_remote(&target.kind);
            if remote {
                if let Some(reason) = crate::dump::store_refusal(
                    &file.provider.endpoint,
                    &file.provider.bucket,
                    &target.endpoint,
                    &target.bucket,
                ) {
                    bail!(reason);
                }
            } else if !same_host(&review.source, &review.destination)
                || file.provider.id != target.id
            {
                bail!("Local-disk backups require the same host and the original storage provider");
            }
            check_backup_database(&dst, &review.destination, database)?;
            review.facts = vec![
                (
                    "Operator-confirmed original engine".into(),
                    original_engine.clone(),
                ),
                ("Destination engine".into(), kind),
                (
                    "Current origin service type (inference only)".into(),
                    if file.kind.is_empty() {
                        "Not known".into()
                    } else {
                        file.kind.clone()
                    },
                ),
                ("Target image".into(), field(&inspect, "/image")),
                ("Backup origin".into(), file.origin.clone()),
                (
                    "Source database".into(),
                    if file.database.is_empty() {
                        "Not recorded for known path".into()
                    } else {
                        file.database.clone()
                    },
                ),
                ("Destination database".into(), database.clone()),
                ("Backup file path".into(), file.path.clone()),
                ("Target provider".into(), target.name.clone()),
                (
                    "Target storage".into(),
                    format!("{} / {}", target.endpoint, target.bucket),
                ),
            ];
        }
        Job::Clone => {
            review.source.require_service()?;
            if !same_host(&review.source, &review.destination) {
                bail!("Clone is same-host; use Migrate configuration for another host");
            }
            validate_new_target(&review.destination)?;
            check_absent(
                &src,
                &review.destination.project,
                &review.destination.service,
            )?;
            review.facts = vec![
                ("Source type".into(), review.source.kind.clone()),
                ("Data copied".into(), "None".into()),
                ("Domains copied".into(), "No".into()),
            ];
        }
        Job::Migrate { services } => {
            if !crate::operations::valid_name(&review.destination.project) {
                bail!("Choose a valid destination project name (a-z, 0-9, _ or -)");
            }
            if services.is_empty() {
                bail!("No services selected for migration");
            }
            let dst = review.destination.client()?;
            let inventory = dst.call("projects", "listProjectsAndServices", Value::Null)?;
            for service in services.iter() {
                if !crate::operations::valid_name(&service.name) || service.kind.is_empty() {
                    bail!("Choose an existing source service");
                }
                if inventory
                    .get("services")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .any(|s| {
                        field(s, "/projectName") == review.destination.project
                            && field(s, "/name") == service.name
                    })
                {
                    bail!("Destination {}/{} already exists; configuration migration never replaces it",review.destination.project,service.name);
                }
            }
            review.facts = vec![
                (
                    "Services".into(),
                    services
                        .iter()
                        .map(|s| format!("{}/{} ({})", s.project, s.name, s.kind))
                        .collect::<Vec<_>>()
                        .join(", "),
                ),
                ("Data copied".into(), "None".into()),
                ("Domains".into(), "Copied; DNS is not moved".into()),
            ];
        }
    }
    Ok(())
}
fn read_providers(client: &EasypanelClient) -> Result<Vec<Provider>> {
    let data = client.call("storageProviders/common", "list", Value::Null)?;
    Ok(data
        .as_array()
        .ok_or_else(|| anyhow!("Unexpected storage-provider response"))?
        .iter()
        .map(|p| Provider {
            id: field(p, "/id"),
            name: field(p, "/name"),
            kind: field(p, "/type"),
            endpoint: field(p, "/endpoint"),
            bucket: field(p, "/bucket"),
        })
        .collect())
}
fn validate_databases(databases: &[String]) -> Result<()> {
    if databases.is_empty() {
        bail!("Choose one or more databases, or choose all non-system databases");
    }
    for name in databases {
        if !crate::dump::valid_db_name(name) || crate::dump::is_system_db(name) {
            bail!("Refusing unsafe or system database {name:?}");
        }
    }
    Ok(())
}
fn validate_new_target(target: &Endpoint) -> Result<()> {
    if !crate::operations::valid_name(&target.project)
        || !crate::operations::valid_name(&target.service)
    {
        bail!(
            "Destination project and service require lowercase names containing a-z, 0-9, _ or -"
        );
    }
    Ok(())
}
fn check_absent(client: &EasypanelClient, project: &str, name: &str) -> Result<()> {
    let data = client.call("projects", "inspectProject", json!({"projectName":project}))?;
    if data
        .get("services")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .any(|s| field(s, "/name") == name)
    {
        bail!("Destination {project}/{name} already exists");
    }
    Ok(())
}
fn check_backup_database(
    client: &EasypanelClient,
    target: &Endpoint,
    database: &str,
) -> Result<()> {
    // Preserve the canonical distinction: unknown enumeration is not proof of absence.
    if let Ok(names) = client.call(
        "databaseBackups",
        "getServiceDatabases",
        json!({"projectName":target.project,"serviceName":target.service}),
    ) {
        if crate::backup::service_lists_database(&names, database) == Some(false) {
            bail!(crate::backup::missing_database_message(
                &target.service,
                database
            ));
        }
    }
    Ok(())
}
fn review_facts(review: &Review) -> Value {
    let databases = match &review.job {
        Job::Dump { databases } => databases.as_slice(),
        _ => &[],
    };
    json!({"facts":review.facts,"databases":databases,"copy_plan":review.copy_plan})
}
fn receive_review(mut review: Review, data: &Value) -> Result<Review> {
    if matches!(review.job, Job::Copy { .. }) {
        let plan: crate::operations::CopyPlan = serde_json::from_value(
            data.get("copy_plan")
                .cloned()
                .ok_or_else(|| anyhow!("Copy preflight did not return its canonical plan"))?,
        )
        .context("Invalid canonical copy preflight plan")?;
        review.facts = facts_from_plan(&plan);
        review.copy_plan = Some(plan);
    } else {
        review.facts = serde_json::from_value(
            data.get("facts")
                .cloned()
                .ok_or_else(|| anyhow!("Preflight facts missing"))?,
        )
        .context("Invalid preflight facts")?;
        if let Job::Dump { databases } = &mut review.job {
            *databases = serde_json::from_value(
                data.get("databases")
                    .cloned()
                    .ok_or_else(|| anyhow!("Resolved databases missing"))?,
            )
            .context("Invalid resolved database list")?;
        }
    }
    Ok(review)
}
fn review_ui(ui: &mut egui::Ui, state: &mut State, cx: &mut PageContext<'_>) {
    let Some(review) = state.review.as_ref() else {
        return;
    };
    ui.separator();
    ui.label(RichText::new("Resolved transfer review").strong());
    ui.label(format!("Source: {}", review.source.label()));
    ui.label(format!("Destination: {}", review.destination.label()));
    egui::Grid::new("transfer-facts")
        .num_columns(2)
        .spacing([16.0, 8.0])
        .show(ui, |ui| {
            for (label, value) in &review.facts {
                ui.label(RichText::new(label).strong());
                ui.add(egui::Label::new(value).wrap());
                ui.end_row();
            }
        });
    let destructive = matches!(
        review.job,
        Job::Copy { .. } | Job::Restore { .. } | Job::Backup { .. }
    );
    if destructive {
        ui.colored_label(ui.visuals().warn_fg_color,"Destination data can be overwritten or dropped. There is no rollback; errors can leave partial writes.");
    }
    let target = if matches!(review.job, Job::Dump { .. }) {
        review.source.label()
    } else {
        review.destination.label()
    };
    let apply = widgets::confirm(
        ui,
        if destructive {
            "Confirm destructive destination write"
        } else {
            "Confirm transfer"
        },
        &target,
        Some(&target),
        &mut state.confirmation,
    );
    if ui.button("Cancel review — nothing has run").clicked() {
        state.review = None;
        state.confirmation.clear();
        return;
    }
    if apply && !task_busy(cx) {
        let review = state.review.take().unwrap();
        state.draft = false;
        state.error = None;
        state.outcome = None;
        let label = format!("{} → {}", review.source.label(), review.destination.label());
        cx.background_progress("execute", &label, move |progress| {
            let result = execute(review, |message| progress(message.to_owned()))?;
            Ok(Payload::Text(result))
        });
    }
}
fn execute(mut review: Review, mut progress: impl FnMut(&str)) -> Result<String> {
    if !matches!(review.job, Job::Copy { .. }) {
        let old_facts = review.facts.clone();
        preflight(&mut review, false)?;
        if review.facts != old_facts {
            bail!("Preflight facts changed since confirmation. Nothing was written; review the new source/target configuration before retrying.");
        }
    }
    let src = review.source.client()?;
    match review.job {
        Job::Dump { databases } => {
            let dump = crate::operations::dump_to_r2(
                &src,
                &review.source.project,
                &review.source.service,
                &databases,
                false,
                Some(&review.provider),
                &mut progress,
            )?;
            Ok(format!(
                "Non-locking dump completed for {}: {}. Kept at {}/{} (provider {}).",
                review.source.label(),
                dump.databases.join(", "),
                dump.bucket,
                dump.key,
                dump.provider
            ))
        }
        Job::Copy { .. } => {
            let confirmed = review.copy_plan.as_ref().ok_or_else(|| {
                anyhow!("The original reviewed copy plan is missing; nothing was written")
            })?;
            let dst = review.destination.client()?;
            let source = crate::operations::CopyTarget {
                client: &src,
                project: &review.source.project,
                service: &review.source.service,
            };
            let target = crate::operations::CopyTarget {
                client: &dst,
                project: &review.destination.project,
                service: &review.destination.service,
            };
            let dump = crate::operations::copy_database(
                &source,
                &target,
                confirmed,
                optional_provider(&review.provider),
                &mut progress,
            )?;
            Ok(format!(
                "Copied {} into {}. Recovery dump retained at {}/{} via {}.",
                dump.databases.join(", "),
                review.destination.label(),
                dump.bucket,
                dump.key,
                dump.provider
            ))
        }
        Job::Restore { dump } => {
            let dst = review.destination.client()?;
            crate::operations::restore_from_r2(
                &dst,
                &review.destination.project,
                &review.destination.service,
                &dump.key,
                Some(&review.provider),
                &mut progress,
            )?;
            Ok(format!(
                "Restored {} into {}",
                dump.key,
                review.destination.label()
            ))
        }
        Job::Backup {
            file,
            database,
            provider,
            ..
        } => {
            let dst = review.destination.client()?;
            let providers = dst.call("storageProviders/common", "list", Value::Null)?;
            let p = providers
                .as_array()
                .into_iter()
                .flatten()
                .find(|p| field(p, "/name") == provider)
                .ok_or_else(|| anyhow!("Destination storage provider disappeared"))?;
            let body = crate::backup::restore_body(
                &review.destination.project,
                &review.destination.service,
                &database,
                &field(p, "/id"),
                &file.path,
            )
            .map_err(anyhow::Error::msg)?;
            progress("Restoring recorded backup; the database container may restart…");
            dst.call("databaseBackups","restoreDatabaseBackup",body).context("Backup restore failed or stopped reporting; destination may be partially written. Check it before retrying")?;
            Ok(format!(
                "Restored {} from {} into {} / {}",
                file.path,
                file.origin,
                review.destination.label(),
                database
            ))
        }
        Job::Clone => {
            let dst = crate::migrate::Target {
                client: &src,
                project: &review.destination.project,
                service: &review.destination.service,
            };
            let notes = crate::migrate::migrate_service(
                &src,
                &review.source.project,
                &review.source.service,
                &review.source.kind,
                &dst,
                false,
            )?;
            Ok(format!(
                "Cloned configuration to {}. No data copied; deploy separately.\n{}",
                review.destination.label(),
                notes.join("\n")
            ))
        }
        Job::Migrate { services } => {
            let dst = review.destination.client()?;
            let created = crate::migrate::ensure_project(&dst, &review.destination.project)?;
            let mut outcomes = vec![format!(
                "Destination project {}. Configuration only; no data copied.",
                if created { "created" } else { "already exists" }
            )];
            let mut failed = false;
            for service in services {
                progress(&format!(
                    "Migrating configuration: {}/{}",
                    service.project, service.name
                ));
                let target = crate::migrate::Target {
                    client: &dst,
                    project: &review.destination.project,
                    service: &service.name,
                };
                match crate::migrate::migrate_service(
                    &src,
                    &service.project,
                    &service.name,
                    &service.kind,
                    &target,
                    true,
                ) {
                    Ok(notes) => {
                        outcomes.push(format!("{}: created\n{}", service.name, notes.join("\n")))
                    }
                    Err(e) => {
                        failed = true;
                        outcomes.push(format!(
                            "{}: FAILED — {e:#}. Service may be partially created.",
                            service.name
                        ));
                    }
                }
            }
            if failed {
                bail!(
                    "Configuration migration partially completed:\n{}",
                    outcomes.join("\n")
                );
            }
            Ok(outcomes.join("\n"))
        }
    }
}

fn download(state: &mut State, cx: &mut PageContext<'_>) {
    let Some(dump) = state
        .selected_dump
        .and_then(|i| state.dumps.get(i))
        .cloned()
    else {
        return;
    };
    let name = dump
        .key
        .rsplit('/')
        .next()
        .filter(|n| !n.is_empty())
        .unwrap_or("dump.sql.gz");
    let Some(path) = rfd::FileDialog::new()
        .set_title("Download database dump — choose a new file")
        .set_file_name(name)
        .save_file()
    else {
        return;
    };
    let source = state.source.clone();
    cx.background_progress("download", "Download database dump", move |progress| {
        let client = source.client()?;
        let output = path
            .to_str()
            .ok_or_else(|| anyhow!("Download path must be valid UTF-8"))?;
        let (_, saved, bytes) = crate::operations::download_r2_dump(
            &client,
            &source.project,
            &source.service,
            Some(&dump.key),
            Some(output),
            Some(&dump.provider),
            |n, total| progress(crate::operations::download_progress(n, total)),
        )?;
        Ok(Payload::Text(format!(
            "Downloaded {} to {} ({})",
            dump.key,
            saved.display(),
            crate::output::format_bytes(bytes as f64)
        )))
    });
}
fn compare(state: &mut State, cx: &mut PageContext<'_>) {
    let source = state.source.clone();
    let target = state.destination.clone();
    let project = state.mode == Mode::ProjectDiff;
    let label = format!("{} compared with {}", source.label(), target.label());
    cx.background("comparison", "Compare configuration", move || {
        let a = source.client()?;
        let b = target.client()?;
        let mut rows: Vec<Vec<String>> = Vec::new();
        if project {
            let left = a.call(
                "projects",
                "inspectProject",
                json!({"projectName":source.project}),
            )?;
            let right = b.call(
                "projects",
                "inspectProject",
                json!({"projectName":target.project}),
            )?;
            let left = left
                .get("services")
                .and_then(Value::as_array)
                .ok_or_else(|| anyhow!("Source project has no service collection"))?;
            let right = right
                .get("services")
                .and_then(Value::as_array)
                .ok_or_else(|| anyhow!("Target project has no service collection"))?;
            let diff = crate::services::project_diff(left, right);
            for name in diff.only_left {
                rows.push(vec![name, "Present".into(), "Missing".into()]);
            }
            for name in diff.only_right {
                rows.push(vec![name, "Missing".into(), "Present".into()]);
            }
            for (name, _) in diff.differing {
                let l = left.iter().find(|s| field(s, "/name") == name).unwrap();
                let r = right.iter().find(|s| field(s, "/name") == name).unwrap();
                append_differences(&mut rows, &name, crate::services::diff(l, r));
            }
            for name in diff.identical {
                rows.push(vec![name, "Identical".into(), "Identical".into()]);
            }
        } else {
            source.require_service()?;
            target.require_service()?;
            let left = a.call(
                &format!("services/{}", source.kind),
                "inspectService",
                json!({"projectName":source.project,"serviceName":source.service}),
            )?;
            let right = b.call(
                &format!("services/{}", target.kind),
                "inspectService",
                json!({"projectName":target.project,"serviceName":target.service}),
            )?;
            append_differences(&mut rows, "", crate::services::diff(&left, &right));
        }
        Ok(Payload::Json(json!({"label":label,"rows":rows})))
    });
}
fn append_differences(
    rows: &mut Vec<Vec<String>>,
    service: &str,
    diffs: Vec<crate::services::Difference>,
) {
    rows.extend(diffs.into_iter().map(|d| {
        vec![
            if service.is_empty() {
                d.what
            } else {
                format!("{service} / {}", d.what)
            },
            d.left.unwrap_or_else(|| "Missing".into()),
            d.right.unwrap_or_else(|| "Missing".into()),
        ]
    }));
}
fn export(state: &mut State, cx: &mut PageContext<'_>) {
    let source = state.source.clone();
    cx.background("export", "Build redacted configuration export", move || {
        let data = source.client()?.call(
            "projects",
            "inspectProject",
            json!({"projectName":source.project}),
        )?;
        let services = data
            .get("services")
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow!("Project response has no service collection"))?;
        Ok(Payload::Text(
            serde_json::to_string_pretty(&crate::services::export_project(
                &source.project,
                services,
            ))? + "\n",
        ))
    });
}
fn write_new_file(path: &PathBuf, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).with_context(|| {
        format!(
            "Cannot create {}. Existing files are preserved; choose a new name",
            path.display()
        )
    })?;
    if let Err(error) = file.write_all(bytes).and_then(|()| file.sync_all()) {
        drop(file);
        let _ = std::fs::remove_file(path);
        return Err(error.into());
    }
    Ok(())
}

fn receive(state: &mut State, cx: &mut PageContext<'_>) {
    if let Some(result) = cx.take("browser") {
        match result {
            Ok(Payload::Worker(Resp::Dbms {
                level,
                database,
                table,
                columns,
                rows,
                capped,
                truncated,
                ..
            })) => {
                if level == Level::Query {
                    state.browser.result_query =
                        state.browser.pending_query.take().unwrap_or_default();
                } else {
                    state.browser.query_database.clone_from(&database);
                    state.browser.result_query.clear();
                }
                state.browser.result_label = format!(
                    "{} · {} / {} · {} rows",
                    cx.target.service,
                    database,
                    table,
                    rows.len()
                );
                state.browser.level = Some(level);
                state.browser.database = database;
                state.browser.table = table;
                state.browser.columns = columns;
                state.browser.rows = rows;
                state.browser.capped = capped;
                state.browser.truncated = truncated;
                state.browser.selected = None;
                state.browser.error = None;
            }
            Ok(Payload::Worker(Resp::DbmsFailed { message } | Resp::Err(message)))
            | Err(message) => state.browser.error = Some(message),
            _ => {
                state.browser.error =
                    Some("Unexpected database response; previous result retained".into())
            }
        }
    }
    if let Some(result) = cx.take("credentials") {
        match result {
            Ok(Payload::Json(data)) => {
                state.browser.credentials =
                    crate::credentials::credentials(&cx.target.service_type, &data);
                state.browser.revealed.clear();
                state.error = None;
            }
            Err(error) => state.error = Some(error),
            _ => state.error = Some("Unexpected credential response".into()),
        }
    }
    for key in ["source-inventory", "target-inventory"] {
        if let Some(result) = cx.take(key) {
            let endpoint = if key == "source-inventory" {
                &mut state.source
            } else {
                &mut state.destination
            };
            match result {
                Ok(Payload::Json(data)) => {
                    endpoint.projects = data
                        .get("projects")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect();
                    endpoint.services = data
                        .get("services")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .map(|s| Service {
                            project: field(s, "/project"),
                            name: field(s, "/name"),
                            kind: field(s, "/kind"),
                        })
                        .collect();
                    endpoint.providers = data
                        .pointer("/providers/rows")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .map(|p| Provider {
                            id: field(p, "/id"),
                            name: field(p, "/name"),
                            kind: field(p, "/type"),
                            endpoint: field(p, "/endpoint"),
                            bucket: field(p, "/bucket"),
                        })
                        .collect();
                    endpoint.error = data
                        .pointer("/providers/error")
                        .and_then(Value::as_str)
                        .map(|e| format!("Inventory loaded; storage providers failed: {e}"));
                }
                Err(error) => endpoint.error = Some(error),
                _ => endpoint.error = Some("Unexpected inventory response".into()),
            }
        }
    }
    if let Some(result) = cx.take("database-list") {
        match result {
            Ok(Payload::Worker(Resp::DatabasesIn { names, .. })) => {
                state.databases = names;
                state.error = None;
            }
            Ok(Payload::Worker(Resp::Err(error))) | Err(error) => state.error = Some(error),
            _ => state.error = Some("Unexpected database-list response".into()),
        }
    }
    if let Some(result) = cx.take("dump-list") {
        match result {
            Ok(Payload::Json(data)) => {
                let provider = field(&data, "/provider");
                state.dumps = data
                    .get("keys")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(|key| SavedDump {
                        key: key.into(),
                        provider: provider.clone(),
                        origin: crate::dump::parse_dump_key(key)
                            .map_or_else(|| "Stored SQL dump".into(), |d| d.origin()),
                    })
                    .collect();
                state.selected_dump = None;
                state.error = None;
            }
            Err(error) => state.error = Some(error),
            _ => state.error = Some("Unexpected stored-dump listing".into()),
        }
    }
    if let Some(result) = cx.take("backup-list") {
        match result {
            Ok(Payload::Json(data)) => {
                state.backups = data
                    .get("rows")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|f| {
                        let provider = state
                            .source
                            .providers
                            .iter()
                            .find(|p| p.name == field(f, "/provider"))?
                            .clone();
                        Some(SavedBackup {
                            database: field(f, "/database"),
                            provider,
                            path: field(f, "/path"),
                            origin: field(f, "/origin"),
                            when: field(f, "/when"),
                            kind: field(f, "/kind"),
                        })
                    })
                    .collect();
                let excluded: BackupExclusions =
                    serde_json::from_value(data.get("excluded").cloned().unwrap_or(Value::Null))
                        .unwrap_or_default();
                state.selected_backup = None;
                state.backup_engine.clear();
                state.backup_engine_confirmed = false;
                state.error = None;
                state.outcome=Some(format!("{} eligible successful backup files. Excluded: {} invalid metadata, {} missing/unidentified providers, {} source-local files for cross-host restore. History covers the latest 200 actions; older known paths can be entered directly.",state.backups.len(),excluded.invalid_metadata,excluded.missing_provider,excluded.local_only));
            }
            Err(error) => state.error = Some(error),
            _ => state.error = Some("Unexpected backup-history response".into()),
        }
    }
    if let Some(result) = cx.take("prepare") {
        match result {
            Ok(Payload::Json(data)) => {
                if let Some(review) = state.pending_review.take() {
                    match receive_review(review, &data) {
                        Ok(review) => {
                            state.review = Some(review);
                            state.error = None;
                        }
                        Err(error) => state.error = Some(error.to_string()),
                    }
                }
            }
            Err(error) => {
                state.pending_review = None;
                state.error = Some(error);
            }
            _ => {
                state.pending_review = None;
                state.error = Some("Unexpected preflight response".into());
            }
        }
    }
    if let Some(result) = cx.take("comparison") {
        match result {
            Ok(Payload::Json(data)) => {
                state.comparison =
                    serde_json::from_value(data.get("rows").cloned().unwrap_or(Value::Null))
                        .unwrap_or_default();
                state.comparison_label = field(&data, "/label");
                state.compared = true;
                state.error = None;
                state.draft = false;
            }
            Err(error) => state.error = Some(error),
            _ => state.error = Some("Unexpected comparison result".into()),
        }
    }
    if let Some(result) = cx.take("export") {
        match result {
            Ok(Payload::Text(text)) => {
                state.export = Some(text);
                state.error = None;
            }
            Err(error) => state.error = Some(error),
            _ => state.error = Some("Unexpected export result".into()),
        }
    }
    for key in ["execute", "download", "export-save", "query-save"] {
        if let Some(result) = cx.take(key) {
            match result {
                Ok(Payload::Text(message)) => {
                    state.outcome = Some(message);
                    state.error = None;
                    if key == "query-save" {
                        state.browser.file_saved();
                    }
                }
                Err(error) => {
                    if key == "execute" {
                        state.draft = true;
                    }
                    state.error = Some(error);
                }
                _ => state.error = Some("Unexpected operation outcome".into()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn endpoint(url: &str, project: &str, service: &str) -> Endpoint {
        Endpoint {
            server: Some(Server {
                name: "fixture".into(),
                url: url.into(),
                token: "test".into(),
                default: false,
                ssh: None,
            }),
            project: project.into(),
            service: service.into(),
            ..Endpoint::default()
        }
    }
    #[test]
    fn self_copy_identity_uses_host_not_connection_alias() {
        assert!(same_host(
            &endpoint("https://panel.invalid/", "shop", "db"),
            &endpoint("https://panel.invalid", "shop", "db")
        ));
        assert!(!same_host(
            &endpoint("https://a.invalid", "shop", "db"),
            &endpoint("https://b.invalid", "shop", "db")
        ));
    }
    #[test]
    fn comparison_preserves_secret_safe_env_policy() {
        let mut rows = Vec::new();
        append_differences(
            &mut rows,
            "db",
            crate::services::diff(
                &json!({"env":"PASSWORD=first-secret"}),
                &json!({"env":"PASSWORD=second-secret"}),
            ),
        );
        assert_eq!(
            rows,
            vec![vec!["db / env PASSWORD", "set", "set (differs)"]]
        );
        assert!(!format!("{rows:?}").contains("secret"));
    }
    #[test]
    fn destructive_database_selection_rejects_empty_system_and_shell_names() {
        assert!(validate_databases(&[]).is_err());
        assert!(validate_databases(&["mysql".into()]).is_err());
        assert!(validate_databases(&["shop;drop".into()]).is_err());
        assert!(validate_databases(&["shop_staging".into()]).is_ok());
    }
    #[test]
    fn dirty_state_tracks_query_and_review_choices() {
        let mut state = State::default();
        assert!(!state.is_dirty());
        state.browser.query = "select 1".into();
        assert!(state.is_dirty());
        state.browser.saved_query.clone_from(&state.browser.query);
        assert!(!state.is_dirty());
        state.draft = true;
        assert!(state.is_dirty());
    }
    #[test]
    fn query_export_never_overwrites_an_existing_file() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "easypanel-data-export-{}-{nonce}",
            std::process::id()
        ));
        write_new_file(&path, b"original").unwrap();
        let result = write_new_file(&path, b"replacement");
        let bytes = std::fs::read(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        assert!(result.is_err());
        assert_eq!(bytes, b"original");
    }
    #[test]
    fn recorded_backup_handoff_keeps_origin_and_target_without_serializing_credentials() {
        let mut source = endpoint("https://source.invalid", "original", "database")
            .server
            .unwrap();
        source.token = "source-panel-secret".into();
        source.ssh = Some(crate::tunnel::SshTunnel {
            host: "source-hop".into(),
            secret: Some("ssh-secret".into()),
            ..Default::default()
        });
        let destination = Target {
            server: endpoint("https://destination.invalid", "staging", "restore-db").server,
            project: "staging".into(),
            service: "restore-db".into(),
            service_type: "mysql".into(),
            ..Target::default()
        };
        let file = crate::backup::BackupFile {
            when: "completed".into(),
            database: "source_schema".into(),
            path: "recorded/file.sql.gz".into(),
            storage_provider_id: "source-provider".into(),
        };
        let page = backup_restore_page(
            &source,
            destination,
            "original/database",
            &file,
            "chosen_target_schema",
            "target-provider",
        );
        let serialized = page.target.resource.to_string();
        assert!(!serialized.contains("source-panel-secret"));
        assert!(!serialized.contains("ssh-secret"));
        let mut state = State {
            destination: Endpoint {
                server: page.target.server.clone(),
                project: page.target.project.clone(),
                service: page.target.service.clone(),
                kind: page.target.service_type.clone(),
                ..Endpoint::default()
            },
            ..State::default()
        };
        initialize_seed(&mut state, &page.target, &[source.clone()]).unwrap();
        assert!(state.seeded_endpoints);
        assert_eq!(state.source.project, "original");
        assert_eq!(state.destination.service, "restore-db");
        let seed = state.seed.as_ref().unwrap();
        assert_eq!(seed.origin, "original/database");
        assert_eq!(seed.path, file.path);
        assert_eq!(seed.destination_database, "chosen_target_schema");
        assert_eq!(seed.destination_provider_id, "target-provider");
        source.url = "https://retargeted.invalid".into();
        assert!(initialize_seed(&mut State::default(), &page.target, &[source]).is_err());
    }
    #[test]
    fn running_or_failing_a_query_does_not_mark_its_text_saved() {
        let mut state = State::default();
        state.browser.query = "select secret from accounts".into();
        let submitted = state.browser.query.clone();
        state.browser.begin_request(Level::Query, &submitted);
        assert!(state.is_dirty());
        assert!(state.browser.saved_query.is_empty());
        state.browser.error = Some("engine refused statement".into());
        assert!(state.is_dirty());
        state.browser.saving_query = Some(submitted.clone());
        state.browser.query.push_str(" where id = 1");
        state.browser.file_saved();
        assert_eq!(state.browser.saved_query, submitted);
        assert!(
            state.is_dirty(),
            "a successful save must not discard edits made during the save"
        );
    }
    #[test]
    fn table_open_and_refresh_use_the_displayed_result_context_not_the_query_editor() {
        let mut browser = Browser {
            database: "database_a".into(),
            query_database: "database_b".into(),
            level: Some(Level::Tables),
            rows: vec![vec!["accounts".into()]],
            ..Browser::default()
        };
        assert_eq!(
            browser.open_request(0),
            Some((Level::Rows, "database_a".into(), "accounts".into()))
        );
        assert_eq!(browser.refresh_request().1, "database_a");
        browser.level = Some(Level::Query);
        browser.result_query = "select 1".into();
        browser.query = "delete from accounts".into();
        assert_eq!(
            browser.refresh_request(),
            (
                Level::Query,
                "database_a".into(),
                String::new(),
                "select 1".into()
            )
        );
    }
    #[test]
    fn history_explains_invalid_missing_provider_and_cross_host_local_exclusions() {
        let provider = |id: &str, kind: &str| Provider {
            id: id.into(),
            name: id.into(),
            kind: kind.into(),
            endpoint: "https://storage.invalid".into(),
            bucket: "backups".into(),
        };
        let action = |path: &str, id: &str| json!({"type":"backup","status":"done","projectName":"p","serviceName":"db","meta":{"path":path,"databaseName":"shop","storageProviderId":id}});
        let actions = vec![
            action("real.sql.gz", "remote"),
            action("-", "remote"),
            action("   ", "remote"),
            action("missing.sql.gz", "absent"),
            action("local.sql.gz", "local"),
        ];
        let (files, excluded) = eligible_backups(
            &actions,
            &[provider("remote", "s3"), provider("local", "local")],
            true,
        );
        assert_eq!(files.len(), 1);
        assert_eq!(excluded.invalid_metadata, 2);
        assert_eq!(excluded.missing_provider, 1);
        assert_eq!(excluded.local_only, 1);
    }
    #[test]
    fn known_panel_backup_path_requires_explicit_original_engine_without_relying_on_history() {
        let provider = Provider {
            id: "source-provider".into(),
            name: "source".into(),
            kind: "s3".into(),
            endpoint: "https://storage.invalid".into(),
            bucket: "backups".into(),
        };
        let mut state = State {
            destination_database: "chosen_target".into(),
            destination_provider: "target".into(),
            backups: vec![SavedBackup {
                database: String::new(),
                provider,
                path: "older/arbitrary-panel-backup.archive".into(),
                origin: "Known path".into(),
                when: String::new(),
                kind: "postgres".into(),
            }],
            ..State::default()
        };
        select_backup(&mut state, 0);
        assert_eq!(state.destination_database, "chosen_target");
        assert!(backup_job(&state).is_err());
        state.backup_engine = "mysql".into();
        state.backup_engine_confirmed = true;
        let Job::Backup {
            file,
            database,
            original_engine,
            ..
        } = backup_job(&state).unwrap()
        else {
            panic!("expected backup restore");
        };
        assert_eq!(file.path, "older/arbitrary-panel-backup.archive");
        assert_eq!(file.kind,"postgres","current configuration must remain a labelled inference, not historical engine metadata");
        assert_eq!(original_engine, "mysql");
        assert_eq!(database, "chosen_target");
        state.backups[0].path = "-".into();
        assert!(backup_job(&state).is_err());
        assert_eq!(file.path, "older/arbitrary-panel-backup.archive");
        assert_eq!(optional_provider(""), None);
        assert_eq!(optional_provider("named"), Some("named"));
    }
    #[test]
    fn marked_service_comparison_freezes_both_rows_without_resource_credentials() {
        let mut server = endpoint("https://panel.invalid", "one", "api")
            .server
            .unwrap();
        server.token = "do-not-serialize-this-token".into();
        let page = compare_services_page(
            &server,
            ("one".into(), "api".into(), "app".into()),
            ("two".into(), "database".into(), "mysql".into()),
        );
        assert!(!page.target.resource.to_string().contains(&server.token));
        let mut state = State::default();
        initialize_seed(&mut state, &page.target, &[]).unwrap();
        assert!(state.seeded_endpoints);
        assert_eq!(
            (
                state.source.project.as_str(),
                state.source.service.as_str(),
                state.source.kind.as_str()
            ),
            ("one", "api", "app")
        );
        assert_eq!(
            (
                state.destination.project.as_str(),
                state.destination.service.as_str(),
                state.destination.kind.as_str()
            ),
            ("two", "database", "mysql")
        );
        assert_eq!(
            state.source.server.as_ref().unwrap().url,
            state.destination.server.as_ref().unwrap().url
        );
    }
    #[test]
    fn copy_review_round_trip_retains_the_original_typed_plan_and_displays_its_facts() {
        let original = crate::operations::CopyPlan {
            stype: "mysql".into(),
            databases: vec!["confirmed_schema".into()],
            bucket: "confirmed-bucket".into(),
            endpoint: "https://confirmed.invalid".into(),
            src_provider: "source-only-name".into(),
            dst_provider: "different-target-name".into(),
            src_image: "mysql:8.0".into(),
            dst_image: "mysql:8.4".into(),
        };
        let pending = Review {
            source: endpoint("https://source.invalid", "a", "db"),
            destination: endpoint("https://target.invalid", "b", "db"),
            provider: String::new(),
            facts: Vec::new(),
            copy_plan: None,
            job: Job::Copy {
                databases: Vec::new(),
            },
        };
        let mut prepared = pending.clone();
        prepared.facts = facts_from_plan(&original);
        prepared.copy_plan = Some(original.clone());
        let mut payload = review_facts(&prepared);
        payload["facts"] = json!([["Bucket", "an-unrelated-later-bucket"]]);
        let restored = receive_review(pending.clone(), &payload).unwrap();
        assert!(restored.copy_plan.as_ref() == Some(&original));
        assert!(
            restored.facts == facts_from_plan(&original),
            "displayed review must come from the exact retained canonical plan"
        );
        assert_eq!(optional_provider(&restored.provider), None);
        payload["copy_plan"] = Value::Null;
        assert!(
            receive_review(pending, &payload).is_err(),
            "missing plan must fail closed, not synthesize a new baseline"
        );
    }
}
