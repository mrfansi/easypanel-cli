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
#[derive(Clone, Copy, PartialEq)]
enum Family {
    Database,
    Backups,
    Configuration,
}
impl Family {
    fn title(self) -> &'static str {
        match self {
            Self::Database => "Database",
            Self::Backups => "Backups & data transfer",
            Self::Configuration => "Configuration",
        }
    }
    fn tasks(self) -> &'static [Mode] {
        match self {
            Self::Database => &[Mode::Database, Mode::Credentials],
            Self::Backups => &[Mode::Dumps, Mode::Restore, Mode::Copy, Mode::BackupRestore],
            Self::Configuration => &[
                Mode::Clone,
                Mode::Migrate,
                Mode::Diff,
                Mode::ProjectDiff,
                Mode::Export,
            ],
        }
    }
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
    fn family(self) -> Family {
        match self {
            Self::Database | Self::Credentials => Family::Database,
            Self::Dumps | Self::Restore | Self::Copy | Self::BackupRestore => Family::Backups,
            Self::Clone | Self::Migrate | Self::Diff | Self::ProjectDiff | Self::Export => {
                Family::Configuration
            }
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
    deleted: bool,
    project_deleted: bool,
    inventory_invalidated: bool,
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
    fn same_server(&self, target: &Target) -> bool {
        self.server.as_ref().is_some_and(|s| {
            target.server.as_ref().is_some_and(|d| {
                (&s.name, &s.url, &s.token, &s.ssh) == (&d.name, &d.url, &d.token, &d.ssh)
            })
        })
    }
    fn deleted_by(&self, deleted: &Target) -> bool {
        self.same_server(deleted)
            && self.project == deleted.project
            && (deleted.service.is_empty()
                || (self.service == deleted.service && self.kind == deleted.service_type))
    }
    fn project_deleted_by(&self, deleted: &Target) -> bool {
        deleted.service.is_empty() && self.same_server(deleted) && self.project == deleted.project
    }
    fn require_project(&self) -> Result<()> {
        if self.project_deleted {
            bail!("This project was deleted. Explicitly refresh its inventory before continuing.");
        }
        if self.project.is_empty() {
            bail!("Choose an existing project");
        }
        Ok(())
    }
    fn require_identity(&self) -> Result<()> {
        if self.project.is_empty() || self.service.is_empty() {
            bail!("Choose a project and service");
        }
        Ok(())
    }
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
        self.require_project()?;
        if self.deleted {
            bail!("This endpoint was deleted. Explicitly refresh its inventory before preparing another operation.");
        }
        if self.kind.is_empty() {
            bail!("Choose an existing service type");
        }
        self.require_identity()
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
    query_focused: bool,
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

enum PendingPicker {
    Query(String),
    Export {
        text: String,
        source: String,
    },
    Dump {
        dump: SavedDump,
        source: Box<Endpoint>,
    },
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
impl Review {
    fn affected_by(&self, deleted: &Target) -> bool {
        match &self.job {
            Job::Dump { .. } => self.source.deleted_by(deleted),
            Job::Restore { .. } | Job::Backup { .. } => self.destination.deleted_by(deleted),
            Job::Copy { .. } => {
                self.source.deleted_by(deleted) || self.destination.deleted_by(deleted)
            }
            Job::Clone => {
                self.source.deleted_by(deleted) || self.destination.project_deleted_by(deleted)
            }
            Job::Migrate { services } => {
                self.destination.project_deleted_by(deleted)
                    || (self.source.same_server(deleted)
                        && services.iter().any(|service| {
                            service.project == deleted.project
                                && (deleted.service.is_empty()
                                    || (service.name == deleted.service
                                        && service.kind == deleted.service_type))
                        }))
            }
        }
    }
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
    pending_picker: Option<PendingPicker>,
    draft: bool,
    seen_deletions: usize,
    deleted: bool,
    recovering: bool,
    review_invalidated: bool,
}
impl State {
    pub(super) fn deleted(&self) -> bool {
        self.deleted && matches!(self.mode, Mode::Database | Mode::Credentials)
    }
    pub(super) fn sync_deletions(&mut self, target: &Target, notices: &[Target]) {
        for deleted in &notices[self.seen_deletions..] {
            if target.deleted_by(deleted) {
                self.deleted = true;
                self.recovering = false;
            }
            for review in self.review.iter().chain(self.pending_review.iter()) {
                self.review_invalidated |= review.affected_by(deleted);
            }
            for endpoint in [&mut self.source, &mut self.destination] {
                if !endpoint.same_server(deleted) {
                    continue;
                }
                endpoint.deleted |= endpoint.deleted_by(deleted);
                endpoint.project_deleted |= endpoint.project_deleted_by(deleted);
                endpoint.inventory_invalidated = true;
                endpoint.loaded = true;
                endpoint.services.retain(|service| {
                    service.project != deleted.project
                        || (!deleted.service.is_empty()
                            && (service.name != deleted.service
                                || service.kind != deleted.service_type))
                });
                if deleted.service.is_empty() {
                    endpoint.projects.retain(|p| *p != deleted.project);
                }
            }
        }
        self.seen_deletions = notices.len();
    }
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
    state.sync_deletions(cx.target, &cx.backend.deletions);
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
    if let Some(result) = cx.take("deleted-recovery") {
        if state.recovering {
            state.recovering = false;
            match result {
                Ok(Payload::Json(_)) => {
                    state.deleted = false;
                    state.error = None;
                    for endpoint in [&mut state.source, &mut state.destination] {
                        if endpoint.same_server(cx.target) && endpoint.project == cx.target.project
                        {
                            endpoint.project_deleted = false;
                            if endpoint.service == cx.target.service
                                && endpoint.kind == cx.target.service_type
                            {
                                endpoint.deleted = false;
                            }
                        }
                    }
                }
                Err(error) => state.error = Some(error),
                _ => {}
            }
        }
    }
    if state.deleted() {
        ui.heading("Resource deleted");
        ui.add(egui::Label::new(cx.target.label()).wrap());
        if let Some(server) = &cx.target.server {
            ui.add(egui::Label::new(&server.url).wrap());
        }
        ui.label("The confirmed deletion succeeded. Drafts and prior results are retained, but this resource is no longer actionable.");
        if let Some(error) = &state.error {
            widgets::error(ui, error);
        }
        ui.horizontal(|ui| {
            if ui.button("Open inventory").clicked() {
                let target = Target {
                    server: cx.target.server.clone(),
                    section: "services".into(),
                    ..Default::default()
                };
                cx.open(OpenPage {
                    kind: PageKind::Project,
                    target,
                    title: "Projects".into(),
                });
            }
            if ui
                .add_enabled(
                    !state.recovering && !cx.busy("deleted-recovery"),
                    egui::Button::new("Refresh"),
                )
                .clicked()
            {
                state.recovering = true;
                if cx.target.service.is_empty() {
                    cx.rpc(
                        "deleted-recovery",
                        "projects",
                        "inspectProject",
                        json!({"projectName":cx.target.project}),
                    );
                } else {
                    cx.rpc(
                        "deleted-recovery",
                        &format!("services/{}", cx.target.service_type),
                        "inspectService",
                        json!({"projectName":cx.target.project,"serviceName":cx.target.service}),
                    );
                }
            }
        });
        ui.label("Refresh explicitly checks the original target for recreation. Transfer reviews must be prepared again.");
        return;
    }
    hydrate_seed(state, cx);
    navigation(ui, state, cx);
    ui.separator();
    if state.review.is_some() {
        review_ui(ui, state, cx);
        return;
    }
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
fn navigation(ui: &mut egui::Ui, state: &mut State, cx: &mut PageContext<'_>) {
    let enabled = !state.seeded_endpoints && !task_busy(cx) && state.review.is_none();
    ui.horizontal_wrapped(|ui| {
        for family in [Family::Database, Family::Backups, Family::Configuration] {
            if ui
                .add_enabled(
                    enabled,
                    egui::Button::new(family.title()).selected(state.mode.family() == family),
                )
                .clicked()
                && state.mode.family() != family
            {
                state.mode = family.tasks()[0];
            }
        }
    });
    ui.horizontal(|ui| {
        let label = ui.label("Task");
        let width = 240.0_f32.min((ui.available_width() - 120.0).max(100.0));
        ui.allocate_ui_with_layout(
            egui::vec2(width, 0.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.add_enabled_ui(enabled, |ui| {
                    egui::ComboBox::from_id_salt("data-task")
                        .width(width)
                        .truncate()
                        .selected_text(state.mode.title())
                        .show_ui(ui, |ui| {
                            for &mode in state.mode.family().tasks() {
                                ui.selectable_value(&mut state.mode, mode, mode.title());
                            }
                        })
                        .response
                        .labelled_by(label.id);
                });
            },
        );
        if ui
            .add_enabled(
                matches!(
                    cx.target.service_type.as_str(),
                    "mysql" | "mariadb" | "postgres" | "mongo" | "redis"
                ),
                egui::Button::new("DB terminal"),
            )
            .on_hover_text(format!(
                "Open the database terminal for {}/{}",
                cx.target.project, cx.target.service
            ))
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
}
fn task_busy(cx: &PageContext<'_>) -> bool {
    [
        "native-picker",
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
        ui.label(&cx.target.service_type);
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
        let label = ui.label("Query database");
        ui.add_enabled(
            !cx.busy("browser"),
            egui::TextEdit::singleline(&mut state.browser.query_database)
                .desired_width(f32::INFINITY),
        )
        .labelled_by(label.id);
    });
    ui.small("SQL / mongosh runs exactly as entered with the database account's privileges. Statements can change or delete data; no LIMIT is added.");
    let query_label = ui.horizontal_wrapped(|ui| {
        let label = ui.label(RichText::new("Query").strong());
        if ui
            .add_enabled(
                !cx.busy("browser") && !state.browser.query.trim().is_empty(),
                egui::Button::new("Run statement"),
            )
            .clicked()
        {
            state.browser.query_focused = false;
            let database = state.browser.query_database.clone();
            let query = state.browser.query.clone();
            db_request(state, cx, Level::Query, database, String::new(), query);
        }
        if ui.button("Discard query edits").clicked() {
            state.browser.query.clone_from(&state.browser.saved_query);
        }
        if ui
            .add_enabled(
                !cx.busy("query-save")
                    && !cx.busy("native-picker")
                    && state.pending_picker.is_none(),
                egui::Button::new("Save query…"),
            )
            .clicked()
        {
            choose_query(state, cx);
        }
        if ui.selectable_label(state.browser.query_focused, if state.browser.query_focused { "Show results" } else { "Focus query" })
            .on_hover_text("Expand the query editor; Run statement returns to query and results. Drag the divider in split view to resize.").clicked() {
            state.browser.query_focused = !state.browser.query_focused;
        }
        label.id
    }).inner;
    if state.browser.query_focused {
        query_editor(ui, &mut state.browser.query, query_label);
        return;
    }
    let available = ui.available_height();
    let max_editor = (available - 200.0).max(64.0);
    let pane = egui::Panel::top("data-query-pane")
        .frame(egui::Frame::NONE)
        .resizable(true)
        .default_size((available * 0.4).clamp(64.0, max_editor))
        .size_range(64.0..=max_editor)
        .show(ui, |ui| {
            query_editor(ui, &mut state.browser.query, query_label);
        });
    let grip = pane.response.rect.center_bottom();
    ui.painter().line_segment(
        [grip - egui::vec2(18.0, 1.0), grip + egui::vec2(18.0, -1.0)],
        ui.visuals().widgets.noninteractive.bg_stroke,
    );
    let height = ui.available_height().max(28.0);
    egui::ScrollArea::vertical()
        .id_salt("data-result-pane")
        .max_height(height)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_max_height(height);
            query_results(ui, state, cx);
        });
}

fn query_editor(
    ui: &mut egui::Ui,
    query: &mut String,
    label: egui::Id,
) -> egui::containers::scroll_area::ScrollAreaOutput<egui::text_edit::TextEditOutput> {
    let height = ui.available_height().max(24.0);
    egui::ScrollArea::vertical()
        .id_salt("query-text-scroll")
        .max_height(height)
        .min_scrolled_height(24.0)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let font = egui::FontSelection::from(egui::TextStyle::Monospace).resolve(ui.style());
            let line_height = ui.fonts_mut(|fonts| fonts.row_height(&font))
                + ui.spacing().extra_text_line_spacing;
            let rows = ((height - 2.0 * ui.spacing().button_padding.y) / line_height)
                .floor()
                .max(1.0) as usize;
            let mut output = egui::TextEdit::multiline(query)
                .code_editor()
                .lock_focus(false)
                .desired_rows(rows)
                .desired_width(f32::INFINITY)
                .min_size(egui::vec2(0.0, height))
                .show(ui);
            output.response.response = output.response.response.labelled_by(label);
            output
        })
}

fn query_results(ui: &mut egui::Ui, state: &mut State, cx: &mut PageContext<'_>) {
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
        0.0,
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
            .min_col_width(120.0)
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
    if cx.busy(key) {
        return;
    }
    endpoint.inventory_invalidated = false;
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
                .truncate()
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
                            endpoint.deleted = false;
                            endpoint.project_deleted = false;
                            changed = true;
                        }
                    }
                });
            ui.horizontal(|ui| {
                let label = ui.label("Project");
                let width = ui.available_width();
                ui.allocate_ui_with_layout(
                    egui::vec2(width, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        egui::ComboBox::from_id_salt("project")
                            .width(width)
                            .truncate()
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
                                        endpoint.deleted = false;
                                        endpoint.project_deleted = false;
                                        changed = true;
                                    }
                                }
                            })
                            .response
                            .labelled_by(label.id)
                            .on_hover_text(&endpoint.project);
                    },
                );
            });
            if service {
                ui.add(
                    egui::TextEdit::singleline(&mut endpoint.filter).hint_text("Filter services"),
                );
                egui::ComboBox::from_id_salt("service")
                    .width(ui.available_width())
                    .truncate()
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
                                endpoint.deleted = false;
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
        ui.add(egui::Label::new(RichText::new(endpoint.label()).small()).wrap());
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
                    if ui.add_enabled(state.pending_picker.is_none() && !cx.busy("native-picker") && !cx.busy("export-save"), egui::Button::new("Save redacted project…")).clicked() {
                        state.pending_picker = Some(PendingPicker::Export { text: text.clone(), source: state.source.label() });
                        let name = format!("{}.easypanel.json", state.source.project);
                        cx.background("native-picker", "Choose redacted project export file", move || {
                            Ok(Payload::ChosenPath(rfd::FileDialog::new().set_title("Export redacted project configuration").set_file_name(name).save_file()))
                        });
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
                100.0,
            );
        }
    }
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
                source.require_identity()?;
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
                100.0,
            );
        });
    if rows.is_empty() {
        ui.small("List dumps to choose an actual stored object; a failed listing keeps the last successful results.");
    }
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                state.selected_dump.is_some()
                    && !cx.busy("download")
                    && !cx.busy("native-picker")
                    && state.pending_picker.is_none(),
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
        100.0,
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
    state.sync_deletions(cx.target, &cx.backend.deletions);
    let missing = match state.mode {
        Mode::Dumps => state.source.deleted,
        Mode::Restore | Mode::BackupRestore => state.destination.deleted,
        Mode::Copy | Mode::Diff => state.source.deleted || state.destination.deleted,
        Mode::Clone => {
            state.source.deleted
                || (state.destination.project == state.new_project
                    && state.destination.project_deleted)
        }
        Mode::Migrate => {
            if state.whole_project {
                state.source.project_deleted
            } else {
                state.source.deleted
            }
        }
        Mode::Export => state.source.project_deleted,
        Mode::ProjectDiff => state.source.project_deleted || state.destination.project_deleted,
        Mode::Database | Mode::Credentials => state.deleted,
    };
    if missing {
        state.error = Some("A selected endpoint was deleted. Explicitly refresh its inventory before preparing another operation.".into());
        return;
    }
    state.review_invalidated = false;
    state.error = None;
    state.confirmation.clear();
    let source = state.source.clone();
    let mut destination = state.destination.clone();
    if matches!(state.mode, Mode::Clone | Mode::Migrate) {
        if destination.project != state.new_project {
            destination.project_deleted = false;
        }
        destination.project.clone_from(&state.new_project);
    }
    if state.mode == Mode::Clone {
        destination.service.clone_from(&state.new_service);
        destination.kind.clone_from(&source.kind);
        destination.deleted = false;
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
            review.destination.require_project()?;
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
            review.source.require_project()?;
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
    let destructive = matches!(
        review.job,
        Job::Copy { .. } | Job::Restore { .. } | Job::Backup { .. }
    );
    let target = if matches!(review.job, Job::Dump { .. }) {
        review.source.label()
    } else {
        review.destination.label()
    };
    let (apply, cancel) = egui::Panel::bottom("data-review-controls")
        .frame(egui::Frame::NONE)
        .show(ui, |ui| {
            if destructive {
                ui.add(egui::Label::new(RichText::new("Destination data can be overwritten or dropped. There is no rollback; errors can leave partial writes.").color(ui.visuals().warn_fg_color)).wrap());
            }
            let mut apply = false;
            ui.add_enabled_ui(!state.review_invalidated, |ui| {
                apply = widgets::confirm(
                    ui,
                    if destructive { "Confirm destructive destination write" } else { "Confirm transfer" },
                    &target,
                    Some(&target),
                    &mut state.confirmation,
                );
            });
            if state.review_invalidated {
                ui.label("A reviewed endpoint was deleted. Cancel this review and prepare it again from current inventory.");
            }
            (apply, ui.button("Cancel review — nothing has run").clicked())
        }).inner;
    egui::ScrollArea::vertical()
        .id_salt("resolved-transfer-details")
        .max_height(ui.available_height().max(24.0))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.label(RichText::new("Resolved transfer review").strong());
            ui.add(egui::Label::new(format!("Source: {}", review.source.label())).wrap());
            ui.add(egui::Label::new(format!("Destination: {}", review.destination.label())).wrap());
            match &review.job {
                Job::Restore { dump } => {
                    ui.add(egui::Label::new(format!("Source provider: {}", dump.provider)).wrap());
                    ui.add(
                        egui::Label::new(format!("Destination provider: {}", review.provider))
                            .wrap(),
                    );
                }
                Job::Backup { file, .. } => {
                    ui.add(
                        egui::Label::new(format!("Source provider: {}", file.provider.name)).wrap(),
                    );
                }
                _ => {}
            }
            egui::Grid::new("transfer-facts")
                .num_columns(2)
                .min_col_width(160.0)
                .spacing([16.0, 8.0])
                .show(ui, |ui| {
                    for (label, value) in &review.facts {
                        ui.add(egui::Label::new(RichText::new(label).strong()).wrap());
                        ui.add(egui::Label::new(value).wrap());
                        ui.end_row();
                    }
                });
        });
    if cancel {
        state.review = None;
        state.confirmation.clear();
        return;
    }
    if apply && !state.review_invalidated && !task_busy(cx) {
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

fn choose_query(state: &mut State, cx: &mut PageContext<'_>) {
    if state.pending_picker.is_some() || cx.busy("native-picker") || cx.busy("query-save") {
        return;
    }
    state.pending_picker = Some(PendingPicker::Query(state.browser.query.clone()));
    cx.background("native-picker", "Choose query file", || {
        Ok(Payload::ChosenPath(
            rfd::FileDialog::new()
                .set_title("Save database query")
                .set_file_name("query.sql")
                .save_file(),
        ))
    });
}

fn download(state: &mut State, cx: &mut PageContext<'_>) {
    if state.pending_picker.is_some() || cx.busy("native-picker") || cx.busy("download") {
        return;
    }
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
        .unwrap_or("dump.sql.gz")
        .to_owned();
    state.pending_picker = Some(PendingPicker::Dump {
        dump,
        source: Box::new(state.source.clone()),
    });
    cx.background(
        "native-picker",
        "Choose database dump destination",
        move || {
            Ok(Payload::ChosenPath(
                rfd::FileDialog::new()
                    .set_title("Download database dump — choose a new file")
                    .set_file_name(name)
                    .save_file(),
            ))
        },
    );
}

fn receive_picker(
    state: &mut State,
    cx: &mut PageContext<'_>,
    result: std::result::Result<Payload, String>,
) {
    let Some(intent) = state.pending_picker.take() else {
        return;
    };
    let path = match result {
        Ok(Payload::ChosenPath(Some(path))) => path,
        Ok(Payload::ChosenPath(None)) => {
            state.outcome = Some("Selection cancelled".into());
            return;
        }
        Err(error) => {
            state.error = Some(error);
            return;
        }
        _ => {
            state.error = Some("Unexpected file selection result".into());
            return;
        }
    };
    match intent {
        PendingPicker::Query(text) => {
            state.browser.saving_query = Some(text.clone());
            cx.background("query-save", "Save query", move || {
                write_new_file(&path, text.as_bytes())?;
                Ok(Payload::Text("Query saved".into()))
            });
        }
        PendingPicker::Export { text, source } => {
            cx.background("export-save", "Save redacted project", move || {
                write_new_file(&path, text.as_bytes())?;
                Ok(Payload::Text(format!(
                    "Redacted configuration from {source} saved to {}",
                    path.display()
                )))
            });
        }
        PendingPicker::Dump { dump, source } => {
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
    }
}
fn compare(state: &mut State, cx: &mut PageContext<'_>) {
    state.sync_deletions(cx.target, &cx.backend.deletions);
    let project = state.mode == Mode::ProjectDiff;
    let missing = if project {
        state.source.project_deleted || state.destination.project_deleted
    } else {
        state.source.deleted || state.destination.deleted
    };
    if missing {
        state.error = Some(
            "A comparison resource was deleted. Refresh its inventory before comparing.".into(),
        );
        return;
    }
    let source = state.source.clone();
    let target = state.destination.clone();
    let label = format!("{} compared with {}", source.label(), target.label());
    cx.background("comparison", "Compare configuration", move || {
        let a = source.client()?;
        let b = target.client()?;
        let mut rows: Vec<Vec<String>> = Vec::new();
        if project {
            source.require_project()?;
            target.require_project()?;
        }
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
    state.sync_deletions(cx.target, &cx.backend.deletions);
    if let Err(error) = state.source.require_project() {
        state.error = Some(error.to_string());
        return;
    }
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
    if let Some(result) = cx.take("native-picker") {
        receive_picker(state, cx, result);
    }
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
            if endpoint.inventory_invalidated {
                continue;
            }
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
                    endpoint.deleted &= !endpoint.services.iter().any(|s| {
                        s.project == endpoint.project
                            && s.name == endpoint.service
                            && s.kind == endpoint.kind
                    });
                    endpoint.project_deleted &= !endpoint.projects.contains(&endpoint.project);
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
                    if key == "query-save" {
                        state.browser.saving_query = None;
                    }
                    if key == "execute" {
                        state.draft = true;
                    }
                    state.error = Some(error);
                }
                _ => {
                    if key == "query-save" {
                        state.browser.saving_query = None;
                    }
                    state.error = Some("Unexpected operation outcome".into());
                }
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
    fn with_context(run: impl FnOnce(&mut PageContext<'_>)) {
        with_target_context(&Target::default(), run);
    }
    fn with_target_context(target: &Target, run: impl FnOnce(&mut PageContext<'_>)) {
        let mut backend = super::super::backend::Backend::new(
            egui::Context::default(),
            super::super::backend::Runtime::default(),
        );
        let mut opened = Vec::new();
        let mut reload = false;
        let path = std::path::Path::new("unused-test-config");
        run(&mut PageContext {
            target,
            servers: &[],
            accounts: &[],
            server_config: path,
            cloudflare_config: path,
            watchlist: path,
            doc: 1,
            backend: &mut backend,
            opened: &mut opened,
            reload: &mut reload,
        });
    }

    fn finish_operation(state: &mut State, cx: &mut PageContext<'_>, key: &str) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while cx.busy(key) {
            assert!(std::time::Instant::now() < deadline, "{key} did not finish");
            cx.backend.pump();
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        receive(state, cx);
    }

    fn data_frame(
        ctx: &egui::Context,
        state: &mut State,
        cx: &mut PageContext<'_>,
        size: egui::Vec2,
        events: Vec<egui::Event>,
        navigation_only: bool,
    ) -> (egui::FullOutput, egui::Rect) {
        let (output, bounds, _) =
            data_geometry_frame(ctx, state, cx, size, events, navigation_only);
        (output, bounds)
    }

    #[derive(Default)]
    struct QueryGeometry {
        pane: Option<egui::Rect>,
        caret: Option<egui::Rect>,
        ime_rect: Option<egui::Rect>,
        focused: Option<egui::Id>,
        cursor: Option<egui::text::CCursor>,
    }

    fn data_geometry_frame(
        ctx: &egui::Context,
        state: &mut State,
        cx: &mut PageContext<'_>,
        size: egui::Vec2,
        events: Vec<egui::Event>,
        navigation_only: bool,
    ) -> (egui::FullOutput, egui::Rect, QueryGeometry) {
        let mut bounds = egui::Rect::NOTHING;
        let mut content = egui::Rect::NOTHING;
        let mut geometry = QueryGeometry::default();
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                events,
                ..Default::default()
            },
            |ui| {
                egui::Panel::top("test-menu")
                    .exact_size(32.0)
                    .show(ui, |_| {});
                egui::Panel::bottom("test-status")
                    .exact_size(24.0)
                    .show(ui, |_| {});
                egui::Panel::left("test-explorer")
                    .exact_size(298.0)
                    .show(ui, |_| {});
                egui::CentralPanel::default()
                    .frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(12, 4)))
                    .show(ui, |ui| {
                        ui.allocate_space(egui::vec2(ui.available_width(), 72.0));
                        bounds = ui.available_rect_before_wrap();
                        if navigation_only {
                            navigation(ui, state, cx);
                        } else {
                            show(ui, state, cx);
                        }
                        content = ui.min_rect();
                        geometry.pane = egui::containers::panel::PanelState::load(
                            ui.ctx(),
                            ui.scope_id().with("data-query-pane"),
                        )
                        .map(|pane| pane.outer_rect);
                        let ime = ui.ctx().output(|output| output.ime);
                        geometry.caret = ime.map(|ime| ime.cursor_rect);
                        geometry.ime_rect = ime.map(|ime| ime.rect);
                        let focused = ui.memory(|memory| memory.focused());
                        geometry.focused = focused;
                        geometry.cursor = focused
                            .and_then(|id| egui::TextEdit::load_state(ui.ctx(), id))
                            .and_then(|state| state.cursor.char_range())
                            .map(|range| range.primary);
                    });
            },
        );
        output.textures_delta.clear();
        assert!(
            content.max.x <= bounds.max.x + 0.5,
            "{content:?} exceeds {bounds:?}"
        );
        assert!(
            content.max.y <= bounds.max.y + 0.5,
            "{content:?} exceeds {bounds:?}"
        );
        (output, bounds, geometry)
    }

    fn painted_text(output: &egui::FullOutput, text: &str) -> (egui::Rect, egui::Rect) {
        output
            .shapes
            .iter()
            .rev()
            .find_map(|clipped| match &clipped.shape {
                egui::epaint::Shape::Text(shape) if shape.galley.text() == text => Some((
                    shape.galley.rect.translate(shape.pos.to_vec2()),
                    clipped.clip_rect,
                )),
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing control or content: {text}"))
    }

    fn semantic_control(
        output: &egui::FullOutput,
        role: egui::Role,
        label: Option<&str>,
    ) -> egui::Rect {
        let rect = output
            .platform_output
            .accesskit_update
            .as_ref()
            .expect("accessibility frame")
            .nodes
            .iter()
            .map(|(_, node)| node)
            .find(|node| {
                node.role() == role && label.is_none_or(|label| node.label() == Some(label))
            })
            .and_then(|node| node.bounds())
            .expect("semantic control bounds");
        egui::Rect::from_min_max(
            egui::pos2(rect.x0 as f32, rect.y0 as f32),
            egui::pos2(rect.x1 as f32, rect.y1 as f32),
        )
    }
    fn click_data(
        ctx: &egui::Context,
        state: &mut State,
        cx: &mut PageContext<'_>,
        size: egui::Vec2,
        position: egui::Pos2,
        navigation_only: bool,
    ) {
        for pressed in [true, false] {
            data_frame(
                ctx,
                state,
                cx,
                size,
                vec![
                    egui::Event::PointerMoved(position),
                    egui::Event::PointerButton {
                        pos: position,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
                navigation_only,
            );
        }
    }
    #[test]
    fn deletion_state_dump_ignores_unused_destination_but_blocks_its_actual_source() {
        use httpmock::{Method::POST, MockServer};
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/api/rpc/storageProviders/common/list");
            then.status(200).json_body(json!({"json":[{
                "id":"store-id","name":"store","type":"s3","endpoint":"https://objects.invalid","bucket":"backups"
            }]}));
        });
        let project = server.mock(|when, then| {
            when.method(POST)
                .path("/api/rpc/projects/inspectProject")
                .json_body(json!({"json":{"projectName":"source"}}));
            then.status(200)
                .json_body(json!({"json":{"services":[{"name":"db","type":"mysql"}]}}));
        });
        let inspect = server.mock(|when, then| {
            when.method(POST)
                .path("/api/rpc/services/mysql/inspectService")
                .json_body(json!({"json":{"projectName":"source","serviceName":"db"}}));
            then.status(200)
                .json_body(json!({"json":{"image":"mysql:8"}}));
        });
        let mut source = endpoint(&server.base_url(), "source", "db");
        source.kind = "mysql".into();
        let mut destination = endpoint(&server.base_url(), "unused", "db");
        destination.kind = "mysql".into();
        let target = Target {
            server: source.server.clone(),
            project: source.project.clone(),
            service: source.service.clone(),
            service_type: source.kind.clone(),
            section: "dumps".into(),
            ..Default::default()
        };
        let unused = Target {
            project: destination.project.clone(),
            ..target.clone()
        };
        with_target_context(&target, |cx| {
            let mut state = State {
                initialized: true,
                mode: Mode::Dumps,
                source,
                destination,
                provider: "store".into(),
                chosen_databases: BTreeSet::from(["database".into()]),
                ..Default::default()
            };
            cx.backend.deletions.push(unused.clone());
            prepare(&mut state, cx);
            assert!(
                cx.busy("prepare"),
                "unused deleted destination must not block source-only dump preflight"
            );
            finish_operation(&mut state, cx, "prepare");
            let review = state
                .review
                .as_ref()
                .expect("real dump preflight must reach review");
            assert!(matches!(&review.job, Job::Dump { databases } if databases == &["database"]));
            assert!(state.destination.deleted);
            assert!(!state.review_invalidated);
            // A repeated notice for the unused endpoint must not invalidate an already prepared dump.
            cx.backend.deletions.push(unused);
            state.sync_deletions(&target, &cx.backend.deletions);
            assert!(!state.review_invalidated);
            cx.backend.deletions.push(target.clone());
            let ctx = egui::Context::default();
            let size = egui::vec2(1280.0, 900.0);
            let (output, _) = data_frame(&ctx, &mut state, cx, size, Vec::new(), false);
            click_data(
                &ctx,
                &mut state,
                cx,
                size,
                painted_text(&output, "Confirm").0.center(),
                false,
            );
            assert!(state.review_invalidated);
            assert!(state.review.is_some());
            assert!(!cx.busy("execute"));
            state.review = None;
            let before = cx.backend.tasks.len();
            prepare(&mut state, cx);
            assert_eq!(
                cx.backend.tasks.len(),
                before,
                "known-missing live source must not dispatch preflight"
            );
            assert!(state.error.is_some());
        });
        project.assert_hits(1);
        inspect.assert_hits(1);
    }
    #[test]
    fn deletion_state_stored_restores_allow_missing_origin_but_require_live_destination() {
        use httpmock::{Method::POST, MockServer};
        for mode in [Mode::Restore, Mode::BackupRestore] {
            let server = MockServer::start();
            server.mock(|when, then| {
                when.method(POST).path("/api/rpc/storageProviders/common/list");
                then.status(200).json_body(json!({"json":[{
                    "id":"store-id","name":"store","type":"s3","endpoint":"https://objects.invalid","bucket":"backups"
                }]}));
            });
            let project = server.mock(|when, then| {
                when.method(POST)
                    .path("/api/rpc/projects/inspectProject")
                    .json_body(json!({"json":{"projectName":"destination"}}));
                then.status(200)
                    .json_body(json!({"json":{"services":[{"name":"db","type":"mysql"}]}}));
            });
            server.mock(|when, then| {
                when.method(POST)
                    .path("/api/rpc/services/mysql/inspectService")
                    .json_body(json!({"json":{"projectName":"destination","serviceName":"db"}}));
                then.status(200)
                    .json_body(json!({"json":{"image":"mysql:8"}}));
            });
            server.mock(|when, then| {
                when.method(POST)
                    .path("/api/rpc/databaseBackups/getServiceDatabases");
                then.status(200).json_body(json!({"json":["database"]}));
            });
            let mut source = endpoint(&server.base_url(), "source", "db");
            source.kind = "mysql".into();
            let mut destination = endpoint(&server.base_url(), "destination", "db");
            destination.kind = "mysql".into();
            let target = Target {
                server: source.server.clone(),
                project: "source".into(),
                service: "db".into(),
                service_type: "mysql".into(),
                section: "restore".into(),
                ..Default::default()
            };
            with_target_context(&target, |cx| {
                let mut state = State {
                    initialized: true,
                    mode,
                    source,
                    destination,
                    destination_provider: "store".into(),
                    destination_database: "database".into(),
                    backup_engine: "mysql".into(),
                    backup_engine_confirmed: true,
                    selected_backup: Some(0),
                    selected_dump: Some(0),
                    dumps: vec![SavedDump {
                        key: "saved.sql.zst".into(),
                        provider: "store".into(),
                        origin: "source/db".into(),
                    }],
                    backups: vec![SavedBackup {
                        database: "database".into(),
                        provider: Provider {
                            id: "store-id".into(),
                            name: "store".into(),
                            kind: "s3".into(),
                            endpoint: "https://objects.invalid".into(),
                            bucket: "backups".into(),
                        },
                        path: "saved-backup.sql".into(),
                        origin: "source/db".into(),
                        when: String::new(),
                        kind: "mysql".into(),
                    }],
                    ..Default::default()
                };
                cx.backend.deletions.push(target.clone());
                prepare(&mut state, cx);
                assert!(
                    cx.busy("prepare"),
                    "stored artifacts do not require their original live service"
                );
                finish_operation(&mut state, cx, "prepare");
                assert!(
                    state.review.is_some(),
                    "stored restore preflight must reach a real review"
                );
                assert!(!state.deleted());
                assert!(!state.review_invalidated);
                assert!(state.source.deleted);
                cx.backend.deletions.push(target.clone());
                state.sync_deletions(&target, &cx.backend.deletions);
                assert!(
                    !state.review_invalidated,
                    "origin removal must not invalidate stored-restore review"
                );
                cx.backend.deletions.push(Target {
                    project: "destination".into(),
                    ..target.clone()
                });
                state.sync_deletions(&target, &cx.backend.deletions);
                assert!(state.review_invalidated);
                let before = cx.backend.tasks.len();
                prepare(&mut state, cx);
                assert_eq!(cx.backend.tasks.len(), before);
                assert!(state.error.is_some());
            });
            project.assert_hits(1);
        }
    }
    #[test]
    fn deletion_state_collection_reads_distinguish_project_from_service_removal() {
        use httpmock::{Method::POST, MockServer};
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/api/rpc/projects/inspectProject");
            then.status(200).json_body(json!({"json":{"services":[]}}));
        });
        server.mock(|when, then| {
            when.method(POST)
                .path("/api/rpc/services/mysql/inspectService");
            then.status(200)
                .json_body(json!({"json":{"name":"db","type":"mysql","env":""}}));
        });
        for mode in [Mode::Export, Mode::ProjectDiff, Mode::Diff] {
            for (project, service) in [
                ("source", "db"),
                ("destination", "db"),
                ("source", ""),
                ("destination", ""),
                ("source", "unselected"),
            ] {
                let mut source = endpoint(&server.base_url(), "source", "db");
                source.kind = "mysql".into();
                let mut destination = endpoint(&server.base_url(), "destination", "db");
                destination.kind = "mysql".into();
                let target = Target {
                    server: source.server.clone(),
                    project: "source".into(),
                    service: "db".into(),
                    service_type: "mysql".into(),
                    ..Default::default()
                };
                let allowed = match mode {
                    Mode::Export => !(project == "source" && service.is_empty()),
                    Mode::ProjectDiff => !service.is_empty(),
                    Mode::Diff => service == "unselected",
                    _ => unreachable!(),
                };
                with_target_context(&target, |cx| {
                    let mut state = State {
                        initialized: true,
                        mode,
                        source,
                        destination,
                        ..Default::default()
                    };
                    cx.backend.deletions.push(Target {
                        project: project.into(),
                        service: service.into(),
                        service_type: if service.is_empty() { "" } else { "mysql" }.into(),
                        ..target.clone()
                    });
                    if mode == Mode::Export {
                        export(&mut state, cx);
                    } else {
                        compare(&mut state, cx);
                    }
                    if allowed {
                        finish_operation(
                            &mut state,
                            cx,
                            if mode == Mode::Export {
                                "export"
                            } else {
                                "comparison"
                            },
                        );
                        assert!(state.error.is_none());
                        assert!(if mode == Mode::Export {
                            state.export.is_some()
                        } else {
                            state.compared
                        });
                    } else {
                        assert!(
                            cx.backend.tasks.is_empty(),
                            "known-missing resources used by this operation must not dispatch"
                        );
                        assert!(state.error.is_some());
                    }
                });
            }
        }
    }
    #[test]
    fn deletion_state_configuration_transfers_use_requested_collection_and_captured_services() {
        use httpmock::{Method::POST, MockServer};
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST)
                .path("/api/rpc/projects/inspectProject")
                .json_body(json!({"json":{"projectName":"new-project"}}));
            then.status(200).json_body(json!({"json":{"services":[]}}));
        });
        server.mock(|when, then| {
            when.method(POST)
                .path("/api/rpc/projects/listProjectsAndServices");
            then.status(200)
                .json_body(json!({"json":{"projects":[],"services":[]}}));
        });
        for (mode, whole_project) in [
            (Mode::Clone, false),
            (Mode::Migrate, true),
            (Mode::Migrate, false),
        ] {
            let mut source = endpoint(&server.base_url(), "source", "old");
            source.kind = "mysql".into();
            source.services = vec![
                Service {
                    project: "source".into(),
                    name: "old".into(),
                    kind: "mysql".into(),
                },
                Service {
                    project: "source".into(),
                    name: "live".into(),
                    kind: "mysql".into(),
                },
            ];
            let mut destination = endpoint(
                &server.base_url(),
                "unused-old-project",
                "unused-old-service",
            );
            destination.kind = "mysql".into();
            let target = Target {
                server: source.server.clone(),
                project: "source".into(),
                service: "old".into(),
                service_type: "mysql".into(),
                ..Default::default()
            };
            with_target_context(&target, |cx| {
                let mut state = State {
                    initialized: true,
                    mode,
                    source,
                    destination,
                    whole_project,
                    new_project: "new-project".into(),
                    new_service: "new-service".into(),
                    ..Default::default()
                };
                cx.backend.deletions.push(Target {
                    project: "unused-old-project".into(),
                    service: String::new(),
                    service_type: String::new(),
                    ..target.clone()
                });
                if mode == Mode::Migrate {
                    cx.backend.deletions.push(target.clone());
                }
                prepare(&mut state, cx);
                if mode == Mode::Migrate && !whole_project {
                    assert!(
                        cx.backend.tasks.is_empty(),
                        "single-service migration must reject its missing live source"
                    );
                    assert!(state.error.is_some());
                    return;
                }
                finish_operation(&mut state, cx, "prepare");
                let review = state
                    .review
                    .as_ref()
                    .expect("actual requested targets must reach preflight review");
                assert_eq!(review.destination.project, "new-project");
                assert!(!review.destination.project_deleted);
                if mode == Mode::Migrate {
                    assert!(matches!(&review.job, Job::Migrate { services }
                        if services.len() == 1 && services[0].name == "live"));
                    cx.backend.deletions.push(target.clone());
                    state.sync_deletions(&target, &cx.backend.deletions);
                    assert!(
                        !state.review_invalidated,
                        "removed service is not among the captured migration inputs"
                    );
                    cx.backend.deletions.push(Target {
                        service: "live".into(),
                        ..target.clone()
                    });
                } else {
                    // The destination service is newly created; only its existing project collection is live.
                    cx.backend.deletions.push(Target {
                        project: "new-project".into(),
                        service: "irrelevant".into(),
                        ..target.clone()
                    });
                    state.sync_deletions(&target, &cx.backend.deletions);
                    assert!(!state.review_invalidated);
                    cx.backend.deletions.push(Target {
                        project: "new-project".into(),
                        service: String::new(),
                        service_type: String::new(),
                        ..target.clone()
                    });
                }
                state.sync_deletions(&target, &cx.backend.deletions);
                assert!(
                    state.review_invalidated,
                    "removal of an actual captured resource must invalidate review"
                );
            });
        }
    }
    #[test]
    fn deletion_state_captured_transfer_destination_blocks_confirm_without_retargeting() {
        let server = httpmock::MockServer::start();
        let mut source = endpoint(&server.base_url(), "source", "db");
        source.kind = "mysql".into();
        let mut destination = endpoint(&server.base_url(), "destination", "db");
        destination.kind = "mysql".into();
        let target = Target {
            server: source.server.clone(),
            project: source.project.clone(),
            service: source.service.clone(),
            service_type: source.kind.clone(),
            section: "copy".into(),
            ..Default::default()
        };
        let deleted = Target {
            server: destination.server.clone(),
            project: destination.project.clone(),
            service: destination.service.clone(),
            service_type: destination.kind.clone(),
            ..Default::default()
        };
        with_target_context(&target, |cx| {
            let mut state = State {
                initialized: true,
                mode: Mode::Copy,
                source: source.clone(),
                destination: destination.clone(),
                draft: true,
                confirmation: destination.label(),
                review: Some(Review {
                    source,
                    destination,
                    provider: "storage".into(),
                    facts: Vec::new(),
                    copy_plan: None,
                    job: Job::Copy {
                        databases: vec!["database".into()],
                    },
                }),
                ..Default::default()
            };
            cx.backend.deletions.push(deleted);
            let ctx = egui::Context::default();
            let size = egui::vec2(1280.0, 900.0);
            let (output, _) = data_frame(&ctx, &mut state, cx, size, Vec::new(), false);
            let pos = painted_text(&output, "Confirm").0.center();
            click_data(&ctx, &mut state, cx, size, pos, false);
            assert!(state.review_invalidated);
            let review = state.review.as_ref().expect("blocked review is retained");
            assert_eq!(review.source.project, "source");
            assert_eq!(review.destination.project, "destination");
            assert!(state.draft);
            assert!(
                !state.deleted,
                "a secondary endpoint deletion must not retarget the source document"
            );
            assert!(state.destination.deleted);
            assert!(!cx.backend.tasks.iter().any(|task| task.label.contains('→')));
            assert!(!cx.busy("execute"));
        });
    }
    #[test]
    fn deletion_state_late_data_recovery_cannot_revive_a_newly_deleted_document() {
        let source = endpoint("https://fixture.invalid", "project", "db");
        let target = Target {
            server: source.server.clone(),
            project: "project".into(),
            service: "db".into(),
            service_type: "mysql".into(),
            section: "database".into(),
            ..Default::default()
        };
        with_target_context(&target, |cx| {
            let mut state = State {
                initialized: true,
                deleted: true,
                recovering: true,
                ..Default::default()
            };
            state.browser.query = "SELECT exact_unsaved_draft".into();
            cx.background("deleted-recovery", "old recovery", || {
                Ok(Payload::Json(json!({"name":"db"})))
            });
            finish_operation(&mut state, cx, "deleted-recovery");
            cx.backend.deletions.push(target.clone());
            let ctx = egui::Context::default();
            data_frame(
                &ctx,
                &mut state,
                cx,
                egui::vec2(1280.0, 900.0),
                Vec::new(),
                false,
            );
            assert!(
                state.deleted,
                "a recovery predating the latest deletion cannot release the document"
            );
            assert!(!state.recovering);
            assert_eq!(state.browser.query, "SELECT exact_unsaved_draft");
        });
    }

    #[test]
    fn family_navigation_reaches_every_task_and_preserves_work_and_frozen_routes() {
        with_context(|cx| {
            let ctx = egui::Context::default();
            ctx.enable_accesskit();
            super::super::theme::apply(&ctx, super::super::theme::Mode::Dark);
            let size = egui::vec2(1024.0, 680.0);
            let mut state = State {
                initialized: true,
                draft: true,
                new_project: "unsaved-destination".into(),
                chosen_databases: BTreeSet::from(["chosen_schema".into()]),
                browser: Browser {
                    query: "\n\nselect unsaved;\r\n".into(),
                    ..Browser::default()
                },
                ..State::default()
            };
            let mut visited = Vec::new();
            for family in [Family::Database, Family::Backups, Family::Configuration] {
                let (output, _) = data_frame(&ctx, &mut state, cx, size, vec![], true);
                let family_button =
                    semantic_control(&output, egui::Role::Button, Some(family.title()));
                click_data(&ctx, &mut state, cx, size, family_button.center(), true);
                assert!(state.mode.family() == family);
                for &mode in family.tasks() {
                    let (output, _) = data_frame(&ctx, &mut state, cx, size, vec![], true);
                    let selected = semantic_control(&output, egui::Role::ComboBox, None).center();
                    click_data(&ctx, &mut state, cx, size, selected, true);
                    let (popup, _) = data_frame(&ctx, &mut state, cx, size, vec![], true);
                    click_data(
                        &ctx,
                        &mut state,
                        cx,
                        size,
                        painted_text(&popup, mode.title()).0.center(),
                        true,
                    );
                    assert!(state.mode == mode);
                    assert!(!visited.contains(&mode));
                    visited.push(mode);
                }
            }
            assert_eq!(visited.len(), 11);
            assert_eq!(state.browser.query, "\n\nselect unsaved;\r\n");
            assert_eq!(state.new_project, "unsaved-destination");
            assert!(state.chosen_databases.contains("chosen_schema"));
            assert!(state.is_dirty());
            for seeded in [true, false] {
                state.seeded_endpoints = seeded;
                state.review = (!seeded).then(|| Review {
                    source: Endpoint::default(),
                    destination: Endpoint::default(),
                    provider: String::new(),
                    facts: vec![],
                    copy_plan: None,
                    job: Job::Clone,
                });
                let original = state.mode;
                let (output, _) = data_frame(&ctx, &mut state, cx, size, vec![], true);
                click_data(
                    &ctx,
                    &mut state,
                    cx,
                    size,
                    painted_text(&output, Family::Database.title()).0.center(),
                    true,
                );
                assert!(state.mode == original);
            }
            assert!(cx.backend.tasks.is_empty());
        });
    }

    #[test]
    fn long_endpoint_choices_fit_both_columns_without_growing_the_document() {
        with_context(|cx| {
            let ctx = egui::Context::default();
            super::super::theme::apply(&ctx, super::super::theme::Mode::Light);
            let mut source = endpoint(
                "https://source.invalid",
                &"long-project-".repeat(12),
                &"long-service-".repeat(12),
            );
            source.server.as_mut().unwrap().name = "long-host-".repeat(12);
            source.kind = "mysql".into();
            source.loaded = true;
            let mut destination = source.clone();
            for width in [1024.0, 1280.0] {
                let mut measured = Vec::new();
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(width, 800.0),
                        )),
                        ..Default::default()
                    },
                    |ui| {
                        egui::Panel::left("test-explorer")
                            .exact_size(298.0)
                            .show(ui, |_| {});
                        egui::CentralPanel::default().show(ui, |ui| {
                            ui.columns(2, |columns| {
                                for (index, endpoint) in
                                    [&mut source, &mut destination].into_iter().enumerate()
                                {
                                    let bounds = columns[index].available_rect_before_wrap();
                                    endpoint_ui(
                                        &mut columns[index],
                                        endpoint,
                                        cx,
                                        if index == 0 {
                                            "source-inventory"
                                        } else {
                                            "target-inventory"
                                        },
                                        if index == 0 {
                                            "Source"
                                        } else {
                                            "Destination / comparison target"
                                        },
                                        true,
                                        true,
                                    );
                                    measured.push((bounds, columns[index].min_rect()));
                                }
                            });
                        });
                    },
                );
                output.textures_delta.clear();
                for (bounds, content) in measured {
                    assert!(
                        content.max.x <= bounds.max.x + 0.5,
                        "{content:?} exceeds {bounds:?}"
                    );
                }
                assert!(cx.backend.tasks.is_empty());
            }
        });
    }

    #[test]
    fn query_caret_viewport_scales_with_the_real_editor_height_without_changing_bytes() {
        let ctx = egui::Context::default();
        super::super::theme::apply(&ctx, super::super::theme::Mode::Dark);
        let original = format!("\n\n{}", "select exact_bytes;\r\n".repeat(100));
        let mut query = original.clone();
        let mut measurements = Vec::new();
        for height in [120.0, 320.0] {
            let mut viewport = egui::Rect::NOTHING;
            let mut allocation = egui::Rect::NOTHING;
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(680.0, height),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        allocation = ui.available_rect_before_wrap();
                        let label = ui.scope_id().with("query-label");
                        viewport = query_editor(ui, &mut query, label).inner_rect;
                    },
                );
            });
            output.textures_delta.clear();
            assert!(
                allocation.contains_rect(viewport),
                "{viewport:?} exceeds actual allocation {allocation:?}"
            );
            assert_eq!(
                viewport.height(),
                allocation.height(),
                "the real scroll viewport must fill its allocation"
            );
            measurements.push((allocation.height(), viewport.height()));
        }
        let allocated_growth = measurements[1].0 - measurements[0].0;
        let viewport_growth = measurements[1].1 - measurements[0].1;
        assert!(allocated_growth > 0.0);
        assert_eq!(viewport_growth, allocated_growth);
        assert_eq!(query, original);
    }

    #[test]
    fn split_query_actions_and_retained_results_stay_reachable_and_focus_preserves_identity() {
        let target = Target {
            project: "project-with-a-long-identity".into(),
            service: "database-with-a-long-identity".into(),
            service_type: "mysql".into(),
            ..Target::default()
        };
        for size in [egui::vec2(1024.0, 680.0), egui::vec2(1280.0, 800.0)] {
            for theme in [
                super::super::theme::Mode::Light,
                super::super::theme::Mode::Dark,
            ] {
                with_target_context(&target, |cx| {
                    let ctx = egui::Context::default();
                    super::super::theme::apply(&ctx, theme);
                    let mut state = State {
                        initialized: true,
                        browser: Browser {
                            loaded: true,
                            level: Some(Level::Query),
                            query: "\n\nselect draft;\r\n".into(),
                            saved_query: "select saved;".into(),
                            database: "original_result_schema".into(),
                            query_database: "different_query_schema".into(),
                            result_query: "select original;".into(),
                            result_label: "original result identity".into(),
                            columns: vec!["retained-column".into()],
                            rows: vec![
                                vec!["retained-visible-cell".into()],
                                vec!["NULL".into()],
                                vec![String::new()],
                            ],
                            ..Browser::default()
                        },
                        ..State::default()
                    };
                    let (output, bounds) = data_frame(&ctx, &mut state, cx, size, vec![], false);
                    for label in ["Run statement", "Discard query edits", "Save query…"] {
                        let (rect, clip) = painted_text(&output, label);
                        assert!(
                            bounds.contains_rect(rect) && clip.contains_rect(rect),
                            "{label} must stay visible"
                        );
                    }
                    let (cell, clip) = painted_text(&output, "retained-visible-cell");
                    assert!(clip.intersects(cell) && bounds.intersects(cell));
                    click_data(
                        &ctx,
                        &mut state,
                        cx,
                        size,
                        painted_text(&output, "Focus query").0.center(),
                        false,
                    );
                    assert!(state.browser.query_focused);
                    let (focused, _) = data_frame(&ctx, &mut state, cx, size, vec![], false);
                    click_data(
                        &ctx,
                        &mut state,
                        cx,
                        size,
                        painted_text(&focused, "Show results").0.center(),
                        false,
                    );
                    assert!(!state.browser.query_focused);
                    state.browser.error = Some("engine refused the draft".into());
                    data_frame(&ctx, &mut state, cx, size, vec![], false);
                    assert_eq!(state.browser.refresh_request().1, "original_result_schema");
                    assert_eq!(state.browser.refresh_request().3, "select original;");
                    assert_eq!(state.browser.rows[0][0], "retained-visible-cell");
                    assert_eq!(state.browser.rows[1][0], "NULL");
                    assert_eq!(state.browser.rows[2][0], "");
                    assert_eq!(state.browser.query, "\n\nselect draft;\r\n");
                    assert!(cx.backend.tasks.is_empty());
                });
            }
        }
    }

    #[test]
    fn minimum_database_long_line_caret_and_pointer_splitter_preserve_work_and_guards() {
        let target = Target {
            service_type: "mysql".into(),
            ..Target::default()
        };
        with_target_context(&target, |cx| {
            let ctx = egui::Context::default();
            super::super::theme::apply(&ctx, super::super::theme::Mode::Dark);
            let mut style = (*ctx.global_style()).clone();
            style.visuals.text_cursor.blink = false;
            ctx.set_global_style(style);
            let size = egui::vec2(1024.0, 680.0);
            let original = format!("select {}final_value;", "some_column || ".repeat(512));
            let mut state = State {
                initialized: true,
                browser: Browser {
                    loaded: true,
                    level: Some(Level::Query),
                    query: original.clone(),
                    saved_query: "saved query baseline".into(),
                    database: "previous_result_schema".into(),
                    query_database: "draft_query_schema".into(),
                    result_query: "select previous_result;".into(),
                    result_label: "previous result identity".into(),
                    columns: vec!["Value".into()],
                    rows: vec![
                        vec!["retained".into()],
                        vec!["NULL".into()],
                        vec![String::new()],
                    ],
                    capped: true,
                    truncated: true,
                    ..Browser::default()
                },
                ..State::default()
            };
            data_geometry_frame(&ctx, &mut state, cx, size, vec![], false);
            let (_, bounds, initial) =
                data_geometry_frame(&ctx, &mut state, cx, size, vec![], false);
            let original_pane = initial.pane.expect("actual database query pane");
            assert!(bounds.contains_rect(original_pane));
            click_data(&ctx, &mut state, cx, size, original_pane.center(), false);
            let key = |key, modifiers| egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            };
            let suffix = " -- typed at document end";
            let (mut caret_frame, _, mut typed) = data_geometry_frame(
                &ctx,
                &mut state,
                cx,
                size,
                vec![
                    key(egui::Key::End, egui::Modifiers::NONE),
                    key(
                        egui::Key::ArrowDown,
                        egui::Modifiers {
                            command: true,
                            mac_cmd: true,
                            ..egui::Modifiers::NONE
                        },
                    ),
                    egui::Event::Text(suffix.into()),
                ],
                false,
            );
            let expected = format!("{original}{suffix}");
            assert_eq!(state.browser.query, expected);
            let rendered_caret = |frame: &egui::FullOutput, geometry: &QueryGeometry| {
                let cursor = geometry.cursor?;
                frame.shapes.iter().find_map(|shape| match &shape.shape {
                    egui::epaint::Shape::Text(text) if text.galley.text() == expected => Some((
                        text.galley
                            .pos_from_cursor(cursor)
                            .translate(text.pos.to_vec2()),
                        shape.clip_rect,
                    )),
                    _ => None,
                })
            };
            let caret_visible = |frame: &egui::FullOutput, geometry: &QueryGeometry| {
                rendered_caret(frame, geometry).is_some_and(|(row, clip)| {
                    clip.contains_rect(row)
                        && geometry.pane.is_some_and(|pane| pane.contains_rect(row))
                })
            };
            // ScrollArea animates its real scroll-to-caret request over subsequent frames.
            for _ in 0..60 {
                if caret_visible(&caret_frame, &typed) {
                    break;
                }
                (caret_frame, _, typed) =
                    data_geometry_frame(&ctx, &mut state, cx, size, vec![], false);
            }
            let (caret_row, actual_paint_clip) =
                rendered_caret(&caret_frame, &typed).expect("focused rendered SQL caret");
            let raw_ime_caret = typed.caret.expect("focused SQL IME caret");
            // IME includes stock caret bleed; visibility belongs to the rendered glyph row and paint clips.
            let actual_caret_clip = caret_frame
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::epaint::Shape::LineSegment { points, .. }
                        if *points
                            == [raw_ime_caret.center_top(), raw_ime_caret.center_bottom()] =>
                    {
                        Some(shape.clip_rect)
                    }
                    _ => None,
                })
                .expect("actual focused caret stroke");
            assert!(actual_caret_clip.contains_rect(caret_row), "glyph-row caret {caret_row:?} exceeds actual caret clip {actual_caret_clip:?}; raw IME={raw_ime_caret:?}, pane={:?}, focus={:?}", typed.pane, typed.focused);
            assert_eq!(
                typed.cursor.map(|cursor| cursor.index),
                Some(egui::epaint::text::CharIndex(expected.chars().count()))
            );
            let (wrapped, _, before_drag) =
                data_geometry_frame(&ctx, &mut state, cx, size, vec![], false);
            assert!(wrapped.shapes.iter().any(|shape| matches!(
                &shape.shape, egui::epaint::Shape::Text(text) if text.galley.text() == expected && text.galley.rows.len() > 1
            )), "the actual long logical line must wrap within the database editor");

            let before = before_drag.pane.unwrap();
            let start = before.center_bottom();
            let end = start - egui::vec2(0.0, 40.0);
            data_geometry_frame(
                &ctx,
                &mut state,
                cx,
                size,
                vec![egui::Event::PointerMoved(start)],
                false,
            );
            data_geometry_frame(
                &ctx,
                &mut state,
                cx,
                size,
                vec![
                    egui::Event::PointerMoved(start),
                    egui::Event::PointerButton {
                        pos: start,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
                false,
            );
            data_geometry_frame(
                &ctx,
                &mut state,
                cx,
                size,
                vec![egui::Event::PointerMoved(end)],
                false,
            );
            let (after_output, after_bounds, after_drag) = data_geometry_frame(
                &ctx,
                &mut state,
                cx,
                size,
                vec![
                    egui::Event::PointerMoved(end),
                    egui::Event::PointerButton {
                        pos: end,
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
                false,
            );
            let after = after_drag.pane.unwrap();
            assert!(
                after.height() <= before.height() - 20.0,
                "pointer drag must resize {before:?} to {after:?}"
            );
            assert!(after_bounds.contains_rect(after));
            let (retained, retained_clip) = painted_text(&after_output, "retained");
            assert!(retained_clip.intersects(retained) && after_bounds.intersects(retained));
            for label in ["Run statement", "Discard query edits", "Save query…"] {
                let (control, clip) = painted_text(&after_output, label);
                assert!(after_bounds.contains_rect(control) && clip.contains_rect(control));
            }
            assert_eq!(
                state.browser.query, expected,
                "divider drag must not edit SQL bytes"
            );
            assert_eq!(state.browser.saved_query, "saved query baseline");
            assert_eq!(state.browser.refresh_request().1, "previous_result_schema");
            assert_eq!(state.browser.refresh_request().3, "select previous_result;");
            assert_eq!(
                state.browser.rows,
                vec![vec!["retained"], vec!["NULL"], vec![""]]
            );
            assert!(state.browser.capped && state.browser.truncated);
            assert!(state.is_dirty());
            assert!(state.pending_picker.is_none() && state.browser.saving_query.is_none());
            assert!(state.review.is_none() && state.pending_review.is_none());
            assert!(
                cx.backend.tasks.is_empty(),
                "caret input and divider drag must not submit, save or transfer"
            );
            assert!(
                caret_visible(&caret_frame, &typed),
                "rendered caret row {caret_row:?} must be inside actual paint clip {actual_paint_clip:?}; raw IME caret={raw_ime_caret:?}, IME inner={:?}, pane={:?}, focus={:?}",
                typed.ime_rect,
                typed.pane,
                typed.focused,
            );
        });
    }

    #[test]
    fn resolved_review_omits_obsolete_setup_and_keeps_guard_and_cancel_outside_long_details() {
        for size in [egui::vec2(1024.0, 680.0), egui::vec2(1280.0, 800.0)] {
            with_context(|cx| {
                let ctx = egui::Context::default();
                super::super::theme::apply(&ctx, super::super::theme::Mode::Dark);
                let destination = endpoint(
                    "https://resolved.invalid",
                    "resolved-long-destination-project",
                    "resolved-database",
                );
                let mut state = State {
                    initialized: true,
                    mode: Mode::Clone,
                    draft: true,
                    destination: endpoint(
                        "https://obsolete.invalid",
                        "obsolete-project",
                        "obsolete-database",
                    ),
                    new_project: "retained-transfer-draft".into(),
                    confirmation: "wrong target".into(),
                    review: Some(Review {
                        source: endpoint(
                            "https://source.invalid",
                            "source-project",
                            "source-database",
                        ),
                        destination: destination.clone(),
                        provider: String::new(),
                        copy_plan: None,
                        job: Job::Clone,
                        facts: (0..40)
                            .map(|i| {
                                (
                                    format!("Resolved fact {i}"),
                                    "long immutable review detail ".repeat(8),
                                )
                            })
                            .collect(),
                    }),
                    ..State::default()
                };
                let (output, bounds) = data_frame(&ctx, &mut state, cx, size, vec![], false);
                for label in ["Confirm", "Cancel review — nothing has run"] {
                    let (rect, clip) = painted_text(&output, label);
                    assert!(
                        bounds.contains_rect(rect) && clip.contains_rect(rect),
                        "{label} must stay fixed"
                    );
                }
                let (resolved, clip) =
                    painted_text(&output, &format!("Destination: {}", destination.label()));
                assert!(bounds.intersects(resolved) && clip.intersects(resolved));
                click_data(
                    &ctx,
                    &mut state,
                    cx,
                    size,
                    painted_text(&output, "Confirm").0.center(),
                    false,
                );
                assert!(
                    state.review.is_some(),
                    "wrong typed destination must never apply"
                );
                assert!(
                    cx.backend.tasks.is_empty(),
                    "resolved review must not load obsolete endpoint inventory"
                );
                let (output, _) = data_frame(&ctx, &mut state, cx, size, vec![], false);
                click_data(
                    &ctx,
                    &mut state,
                    cx,
                    size,
                    painted_text(&output, "Cancel review — nothing has run")
                        .0
                        .center(),
                    false,
                );
                assert!(state.review.is_none());
                assert!(state.confirmation.is_empty());
                assert_eq!(state.new_project, "retained-transfer-draft");
                assert_eq!(state.destination.project, "obsolete-project");
                assert!(cx.backend.tasks.is_empty());
            });
        }
    }

    #[test]
    fn cancelling_each_native_picker_does_not_write_download_or_advance_query_baseline() {
        with_context(|cx| {
            let mut state = State::default();
            state.browser.query = "select changed".into();
            state.browser.saved_query = "select saved".into();
            for intent in [
                PendingPicker::Query(state.browser.query.clone()),
                PendingPicker::Export {
                    text: "redacted configuration".into(),
                    source: "original project".into(),
                },
                PendingPicker::Dump {
                    dump: SavedDump {
                        key: "original.sql.gz".into(),
                        provider: "original-provider".into(),
                        origin: "original/database".into(),
                    },
                    source: Box::new(endpoint(
                        "https://must-not-be-requested.invalid",
                        "original",
                        "db",
                    )),
                },
            ] {
                state.pending_picker = Some(intent);
                receive_picker(&mut state, cx, Ok(Payload::ChosenPath(None)));
                assert!(state.pending_picker.is_none());
                assert!(cx.backend.tasks.is_empty(), "cancel must not queue I/O");
                assert!(state.browser.saving_query.is_none());
                assert_eq!(state.browser.saved_query, "select saved");
                assert_eq!(state.outcome.as_deref(), Some("Selection cancelled"));
                assert!(state.is_dirty());
            }
        });
    }

    #[test]
    fn query_picker_saves_requested_bytes_and_baseline_not_later_editor_changes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("query.sql");
        with_context(|cx| {
            let mut state = State::default();
            let requested = "-- raw UTF-8 SQL: café\r\nselect 1;\n";
            state.browser.query = requested.into();
            state.pending_picker = Some(PendingPicker::Query(state.browser.query.clone()));
            state.browser.query = "select edited_while_choosing".into();
            receive_picker(&mut state, cx, Ok(Payload::ChosenPath(Some(path.clone()))));
            state.browser.query = "select edited_while_writing".into();
            finish_operation(&mut state, cx, "query-save");
            assert_eq!(std::fs::read(&path).unwrap(), requested.as_bytes());
            assert_eq!(state.browser.saved_query, requested);
            assert_eq!(state.browser.query, "select edited_while_writing");
            assert!(state.browser.saving_query.is_none());
            assert!(state.is_dirty());
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(
                    std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                    0o600
                );
            }
        });
    }

    #[test]
    fn refused_query_save_keeps_file_baseline_and_previous_grid() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("existing.sql");
        std::fs::write(&path, b"do not replace").unwrap();
        with_context(|cx| {
            let mut state = State::default();
            state.browser.saved_query = "saved baseline".into();
            state.browser.query = "new draft".into();
            state.browser.rows = vec![vec!["old grid".into()]];
            state.browser.error = Some("old query error".into());
            state.pending_picker = Some(PendingPicker::Query(state.browser.query.clone()));
            receive_picker(&mut state, cx, Ok(Payload::ChosenPath(Some(path.clone()))));
            finish_operation(&mut state, cx, "query-save");
            assert_eq!(std::fs::read(&path).unwrap(), b"do not replace");
            assert_eq!(state.browser.saved_query, "saved baseline");
            assert_eq!(state.browser.rows, vec![vec!["old grid"]]);
            assert_eq!(state.browser.error.as_deref(), Some("old query error"));
            assert!(state
                .error
                .as_deref()
                .unwrap()
                .contains("Existing files are preserved"));
            assert!(state.browser.saving_query.is_none());
            assert!(state.is_dirty());
        });
    }

    #[test]
    fn export_picker_saves_frozen_redacted_text_and_original_source_identity() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("original.easypanel.json");
        with_context(|cx| {
            let mut state = State::default();
            state.source = endpoint("https://original.invalid", "original", "");
            state.export = Some("{\"environment\":\"<redacted>\"}\n".into());
            state.pending_picker = Some(PendingPicker::Export {
                text: state.export.clone().unwrap(),
                source: state.source.label(),
            });
            state.source = endpoint("https://new.invalid", "new-project", "");
            state.export = Some("different preview".into());
            receive_picker(&mut state, cx, Ok(Payload::ChosenPath(Some(path.clone()))));
            finish_operation(&mut state, cx, "export-save");
            assert_eq!(
                std::fs::read_to_string(&path).unwrap(),
                "{\"environment\":\"<redacted>\"}\n"
            );
            let outcome = state.outcome.as_deref().unwrap();
            assert!(outcome.contains("fixture / original / "));
            assert!(!outcome.contains("new-project"));
        });
    }

    #[test]
    fn duplicate_picker_request_preserves_intent_and_queue_failure_releases_it() {
        with_context(|cx| {
            let mut state = State {
                pending_picker: Some(PendingPicker::Query("first snapshot".into())),
                browser: Browser {
                    query: "newer draft".into(),
                    ..Browser::default()
                },
                ..State::default()
            };
            choose_query(&mut state, cx);
            download(&mut state, cx);
            assert!(
                matches!(&state.pending_picker, Some(PendingPicker::Query(text)) if text == "first snapshot")
            );
            assert!(cx.backend.tasks.is_empty());
            receive_picker(&mut state, cx, Err("The operation queue is full".into()));
            assert!(state.pending_picker.is_none());
            assert!(state.browser.saving_query.is_none());
            assert_eq!(state.browser.query, "newer draft");
            assert!(state.browser.saved_query.is_empty());
            assert_eq!(state.error.as_deref(), Some("The operation queue is full"));
        });
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
