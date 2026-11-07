mod backend;
mod cloudflare;
mod connections;
mod data;
mod host;
mod model;
mod services;
mod terminal;
mod theme;
mod widgets;

use crate::{
    cloudflare::CloudflareAccount,
    config::{CloudflareConfig, Server, ServerConfig},
};
pub(crate) use backend::Runtime;
use backend::{Backend, Lane, TaskStatus, TrustContext};
use eframe::egui::{self, RichText};
use model::{DocId, OpenPage, PageContext, PageKind, Payload, Target};
use serde_json::{json, Value};
use std::{collections::HashMap, path::PathBuf};
use theme::{Icon, Palette};

enum PageState {
    Services(Box<services::State>),
    Host(Box<host::State>),
    Cloudflare(Box<cloudflare::State>),
    Data(Box<data::State>),
    Terminal(Box<terminal::State>),
}
impl PageState {
    fn new(kind: &PageKind) -> Self {
        match kind {
            PageKind::Fleet | PageKind::Host => Self::Host(Box::default()),
            PageKind::Cloudflare => Self::Cloudflare(Box::default()),
            PageKind::Data => Self::Data(Box::default()),
            PageKind::Terminal => Self::Terminal(Box::default()),
            _ => Self::Services(Box::default()),
        }
    }
    fn deleted(&self) -> bool {
        match self {
            Self::Services(state) => state.deleted(),
            Self::Data(state) => state.deleted(),
            Self::Terminal(state) => state.removed(),
            _ => false,
        }
    }
    fn sync_deletions(&mut self, target: &Target, notices: &[Target]) {
        match self {
            Self::Services(state) => state.sync_deletions(target, notices),
            Self::Data(state) => state.sync_deletions(target, notices),
            Self::Terminal(state) => state.sync_deletions(target, notices),
            _ => {}
        }
    }
    fn is_dirty(&self) -> bool {
        match self {
            Self::Services(s) => s.is_dirty(),
            Self::Host(s) => s.is_dirty(),
            Self::Cloudflare(s) => s.is_dirty(),
            Self::Data(s) => s.is_dirty(),
            Self::Terminal(s) => s.is_dirty(),
        }
    }
    fn running_session(&self) -> bool {
        matches!(self, Self::Terminal(s) if s.has_running_session())
    }
}
struct Document {
    id: DocId,
    page: OpenPage,
    state: PageState,
    docked: bool,
}
impl Document {
    fn service_section(&self) -> Option<&'static str> {
        match &self.state {
            PageState::Services(state) if self.page.kind == PageKind::Service => {
                Some(state.section_label(&self.page.target))
            }
            _ => None,
        }
    }
    fn caption(&self) -> String {
        if let PageState::Host(state) = &self.state {
            if self.page.kind == PageKind::Host {
                let section = state.section_label(&self.page.target);
                return self.page.target.server.as_ref().map_or_else(
                    || section.to_owned(),
                    |server| format!("{} / {section}", server.name),
                );
            }
        }
        self.service_section().map_or_else(
            || self.page.title.clone(),
            |section| format!("{} / {section}", self.page.target.service),
        )
    }
    fn breadcrumb(&self) -> String {
        if let PageState::Cloudflare(state) = &self.state {
            return state.breadcrumb(&self.page.target);
        }
        if let PageState::Host(state) = &self.state {
            if self.page.kind == PageKind::Host {
                return state.breadcrumb(&self.page.target);
            }
        }
        let mut label = self.page.target.label();
        if let Some(section) = self.service_section() {
            if self.page.target.section.is_empty() {
                label.push_str(" / ");
            } else {
                label.truncate(label.len() - self.page.target.section.len());
            }
            label.push_str(section);
        }
        label
    }
}
#[derive(Default)]
struct Catalog {
    data: Option<Value>,
    error: Option<String>,
    requested: bool,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Utility {
    Tasks,
    Logs,
    Terminal,
}
#[derive(Clone, Copy)]
enum Close {
    Document(DocId),
    Application,
}

enum PaletteCommand {
    Open(OpenPage),
    ServiceAction {
        target: Target,
        title: String,
        action: services::QuickAction,
    },
}
impl PaletteCommand {
    fn title(&self) -> &str {
        match self {
            Self::Open(page) => &page.title,
            Self::ServiceAction { title, .. } => title,
        }
    }
    fn target(&self) -> &Target {
        match self {
            Self::Open(page) => &page.target,
            Self::ServiceAction { target, .. } => target,
        }
    }
}

pub struct Desktop {
    path: PathBuf,
    cf_path: PathBuf,
    checks_path: PathBuf,
    backend: Backend,
    servers: Vec<Server>,
    accounts: Vec<CloudflareAccount>,
    config_errors: Vec<String>,
    loaded: bool,
    reload: bool,
    generation: u64,
    catalog: HashMap<String, Catalog>,
    documents: Vec<Document>,
    next_doc: DocId,
    active: Option<DocId>,
    last_tab_view: Option<(Option<DocId>, f32, f32)>,
    utility_doc: Option<DocId>,
    last_logs_doc: Option<DocId>,
    last_terminal_doc: Option<DocId>,
    utility: Utility,
    explorer_visible: bool,
    utility_visible: bool,
    last_explorer_doc: Option<DocId>,
    explorer_filter: String,
    palette_open: bool,
    palette_query: String,
    palette_selection: usize,
    palette_focus: bool,
    focus: Option<u8>,
    close: Option<Close>,
    allow_quit: bool,
    mode: theme::Mode,
    applied_theme: Option<bool>,
    shortcuts_open: bool,
}
impl Desktop {
    pub fn new(cc: &eframe::CreationContext<'_>, path: PathBuf) -> Self {
        Self::with_runtime(cc, path, Runtime::default())
    }
    pub(crate) fn with_runtime(
        cc: &eframe::CreationContext<'_>,
        path: PathBuf,
        runtime: Runtime,
    ) -> Self {
        let cf_path = path.with_file_name("cloudflare.json");
        let checks_path = path.with_file_name("checks.json");
        let mut app = Self {
            path,
            cf_path,
            checks_path,
            backend: Backend::new(cc.egui_ctx.clone(), runtime),
            servers: Vec::new(),
            accounts: Vec::new(),
            config_errors: Vec::new(),
            loaded: false,
            reload: true,
            generation: 0,
            catalog: HashMap::new(),
            documents: Vec::new(),
            next_doc: 1,
            active: None,
            last_tab_view: None,
            utility_doc: None,
            last_logs_doc: None,
            last_terminal_doc: None,
            utility: Utility::Tasks,
            explorer_visible: true,
            utility_visible: false,
            last_explorer_doc: None,
            explorer_filter: String::new(),
            palette_open: false,
            palette_query: String::new(),
            palette_selection: 0,
            palette_focus: false,
            focus: None,
            close: None,
            allow_quit: false,
            mode: theme::Mode::System,
            applied_theme: None,
            shortcuts_open: false,
        };
        theme::install_fonts(&cc.egui_ctx);
        theme::apply(&cc.egui_ctx, app.mode);
        cc.egui_ctx
            .send_viewport_cmd(egui::ViewportCommand::MinInnerSize(egui::vec2(
                1024.0, 680.0,
            )));
        cc.egui_ctx
            .send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(1280.0, 800.0)));
        app.open(OpenPage {
            kind: PageKind::Fleet,
            target: Target::default(),
            title: "Fleet health".into(),
        });
        app.load_connections();
        app
    }
    fn load_connections(&mut self) {
        if self.backend.busy(0, "connections") {
            return;
        }
        self.reload = false;
        let path = self.path.clone();
        let cf = self.cf_path.clone();
        self.backend.submit(0, "connections", "Read connection profiles", TrustContext { target: "Local configuration".into(), secrets: Vec::new() }, Lane::Interactive, Box::new(move |_| {
            let servers = ServerConfig::new(path).try_all();
            let accounts = CloudflareConfig::new(cf).try_all();
            let (servers, server_error) = match servers { Ok(v) => (Some(v), None), Err(e) => (None, Some(e.to_string())) };
            let (accounts, account_error) = match accounts { Ok(v) => (Some(v), None), Err(e) => (None, Some(e.to_string())) };
            Ok(Payload::Json(json!({"servers":servers,"accounts":accounts,"server_error":server_error,"account_error":account_error})))
        }));
    }
    fn pump(&mut self) {
        self.backend.pump();
        let mut opened = Vec::new();
        for document in &mut self.documents {
            document
                .state
                .sync_deletions(&document.page.target, &self.backend.deletions);
            if let PageState::Terminal(state) = &mut document.state {
                let mut cx = PageContext {
                    target: &document.page.target,
                    servers: &self.servers,
                    accounts: &self.accounts,
                    server_config: &self.path,
                    cloudflare_config: &self.cf_path,
                    watchlist: &self.checks_path,
                    doc: document.id,
                    backend: &mut self.backend,
                    opened: &mut opened,
                    reload: &mut self.reload,
                };
                state.tick(&mut cx);
            }
        }
        for page in opened {
            self.open(page);
        }
        if let Some(result) = self.backend.take(0, "connections") {
            self.loaded = true;
            match result {
                Ok(Payload::Json(data)) => {
                    self.config_errors.clear();
                    for name in ["server_error", "account_error"] {
                        if let Some(error) = data[name].as_str() {
                            self.config_errors.push(error.to_owned());
                        }
                    }
                    if !data["servers"].is_null() {
                        match serde_json::from_value(data["servers"].clone()) {
                            Ok(servers) => self.servers = servers,
                            Err(e) => self.config_errors.push(e.to_string()),
                        }
                    }
                    if !data["accounts"].is_null() {
                        match serde_json::from_value(data["accounts"].clone()) {
                            Ok(accounts) => self.accounts = accounts,
                            Err(e) => self.config_errors.push(e.to_string()),
                        }
                    }
                    self.generation += 1;
                    self.catalog.clear();
                    self.backend.close(0);
                }
                Err(error) => self.config_errors.push(error),
                _ => self
                    .config_errors
                    .push("Unexpected connection response. Reload profiles.".into()),
            }
        }
        for server in &self.servers {
            let key = format!("catalog:{}:{}", self.generation, server.name);
            if let Some(result) = self.backend.take(0, &key) {
                let catalog = self.catalog.entry(server.name.clone()).or_default();
                match result {
                    Ok(Payload::Json(data))
                        if data.get("projects").is_some_and(Value::is_array)
                            && data.get("services").is_some_and(Value::is_array) =>
                    {
                        catalog.data = Some(data);
                        catalog.error = None;
                    }
                    Ok(_) => {
                        catalog.error = Some(
                            "The panel returned an invalid resource inventory. Refresh to retry."
                                .into(),
                        )
                    }
                    Err(error) => catalog.error = Some(error),
                }
            }
        }
        if self.reload {
            self.load_connections();
        }
    }
    fn open(&mut self, page: OpenPage) {
        for document in &mut self.documents {
            document
                .state
                .sync_deletions(&document.page.target, &self.backend.deletions);
        }
        let verified_terminal = page.kind == PageKind::Terminal
            && self.documents.iter().any(|doc| {
                doc.page.kind == PageKind::Service
                    && doc.page.target.same_server(&page.target)
                    && doc.page.target.project == page.target.project
                    && doc.page.target.service == page.target.service
                    && doc.page.target.service_type == page.target.service_type
                    && matches!(&doc.state, PageState::Services(state) if state.verified_resource())
            });
        if let Some(document) = self.documents.iter().find(|doc| {
            doc.page.kind == page.kind
                && doc.page.target.same_resource(&page.target)
                && !(verified_terminal
                    && matches!(&doc.state, PageState::Terminal(state) if state.removed()))
        }) {
            let id = document.id;
            if document.docked {
                self.reveal_utility(
                    if matches!(page.target.section.as_str(), "logs" | "log-search") {
                        Utility::Logs
                    } else {
                        Utility::Terminal
                    },
                );
                self.utility_doc = Some(id);
            } else {
                self.active = Some(id);
                self.last_tab_view = None;
            }
            return;
        }
        let id = self.next_doc;
        self.next_doc += 1;
        let docked = page.kind == PageKind::Terminal;
        let mut state = if verified_terminal {
            PageState::Terminal(Box::new(terminal::State::verified(
                self.backend.deletions.len(),
            )))
        } else {
            PageState::new(&page.kind)
        };
        state.sync_deletions(&page.target, &self.backend.deletions);
        if docked {
            self.reveal_utility(
                if matches!(page.target.section.as_str(), "logs" | "log-search") {
                    Utility::Logs
                } else {
                    Utility::Terminal
                },
            );
            self.utility_doc = Some(id);
        } else {
            self.active = Some(id);
            self.last_tab_view = None;
        }
        self.documents.push(Document {
            id,
            page,
            state,
            docked,
        });
    }
    fn reveal_utility(&mut self, utility: Utility) {
        match self.utility {
            Utility::Logs => self.last_logs_doc = self.utility_doc,
            Utility::Terminal => self.last_terminal_doc = self.utility_doc,
            Utility::Tasks => {}
        }
        if utility != self.utility {
            self.utility_doc = match utility {
                Utility::Logs => self.last_logs_doc,
                Utility::Terminal => self.last_terminal_doc,
                Utility::Tasks => self.utility_doc,
            };
        }
        self.utility = utility;
        self.utility_visible = true;
    }
    fn request_close(&mut self, id: DocId) {
        if let Some(document) = self.documents.iter().find(|doc| doc.id == id) {
            if document.state.is_dirty() || document.state.running_session() {
                self.close = Some(Close::Document(id));
            } else {
                self.remove_document(id);
            }
        }
    }
    fn remove_document(&mut self, id: DocId) {
        self.documents.retain(|doc| doc.id != id);
        self.backend.close(id);
        if self.last_logs_doc == Some(id) {
            self.last_logs_doc = None;
        }
        if self.last_terminal_doc == Some(id) {
            self.last_terminal_doc = None;
        }
        if self.active == Some(id) {
            self.active = self
                .documents
                .iter()
                .rev()
                .find(|doc| !doc.docked)
                .map(|doc| doc.id);
        }
        if self.utility_doc == Some(id) {
            self.utility_doc = self
                .documents
                .iter()
                .rev()
                .find(|doc| {
                    doc.docked
                        && matches!(doc.page.target.section.as_str(), "logs" | "log-search")
                            == (self.utility == Utility::Logs)
                })
                .map(|doc| doc.id);
        }
    }
    fn open_palette(&mut self) {
        self.palette_open = true;
        self.palette_focus = true;
        self.palette_selection = 0;
        self.palette_query.clear();
    }
    fn current_target(&self) -> Option<Target> {
        self.documents
            .iter()
            .find(|doc| Some(doc.id) == self.active)
            .map(|doc| doc.page.target.clone())
    }
    fn shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.viewport().close_requested())
            && !self.allow_quit
            && (self
                .documents
                .iter()
                .any(|doc| doc.state.is_dirty() || doc.state.running_session())
                || self.backend.active() > 0)
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.close = Some(Close::Application);
        }
        if self.palette_open || self.close.is_some() {
            return;
        }
        let terminal_focus = ctx.data(|data| {
            data.get_temp::<bool>(egui::Id::unique("desktop-terminal-focus"))
                .unwrap_or(false)
        });
        let consume = |key, shift| {
            ctx.input_mut(|i| {
                i.consume_shortcut(&egui::KeyboardShortcut::new(
                    egui::Modifiers {
                        command: true,
                        shift,
                        ..Default::default()
                    },
                    key,
                ))
            })
        };
        if consume(egui::Key::P, true)
            || (!terminal_focus && (consume(egui::Key::P, false) || consume(egui::Key::K, false)))
        {
            self.open_palette();
        }
        if !terminal_focus {
            if consume(egui::Key::W, false) {
                if let Some(id) = self.active {
                    self.request_close(id);
                }
            }
            if consume(egui::Key::B, false) {
                self.explorer_visible = !self.explorer_visible;
            }
            if consume(egui::Key::J, false) {
                self.utility_visible = !self.utility_visible;
            }
        }
        for (key, focus) in [
            (egui::Key::Num1, 1),
            (egui::Key::Num2, 2),
            (egui::Key::Num3, 3),
        ] {
            if consume(key, false) {
                self.focus = Some(focus);
                if focus == 1 {
                    self.explorer_visible = true;
                }
                if focus == 3 {
                    self.utility_visible = true;
                    ctx.data_mut(|data| {
                        data.insert_temp(egui::Id::unique("desktop-terminal-focus-request"), true)
                    });
                }
            }
        }
        if consume(egui::Key::Tab, false) {
            let visible: Vec<_> = self
                .documents
                .iter()
                .filter(|doc| !doc.docked)
                .map(|doc| doc.id)
                .collect();
            if !visible.is_empty() {
                let index = visible
                    .iter()
                    .position(|id| Some(*id) == self.active)
                    .map_or(0, |i| (i + 1) % visible.len());
                self.active = Some(visible[index]);
            }
        }
    }
    fn menu(&mut self, ui: &mut egui::Ui, palette: Palette, opened: &mut Vec<OpenPage>) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.label(RichText::new("EasyPanel").strong());
            ui.menu_button("File", |ui| {
                if ui.button("Manage connections").clicked() {
                    opened.push(connection_page());
                    ui.close();
                }
                if ui.button("Cloudflare accounts").clicked() {
                    opened.push(account_page());
                    ui.close();
                }
                if ui.button("Reload profiles").clicked() {
                    self.reload = true;
                    ui.close();
                }
                ui.separator();
                if ui.button("Close document    Cmd/Ctrl W").clicked() {
                    if let Some(id) = self.active {
                        self.request_close(id);
                    }
                    ui.close();
                }
                if ui.button("Quit").clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                    ui.close();
                }
            });
            ui.menu_button("View", |ui| {
                ui.checkbox(
                    &mut self.explorer_visible,
                    "Resource explorer    Cmd/Ctrl B",
                );
                ui.checkbox(&mut self.utility_visible, "Utility pane    Cmd/Ctrl J");
                for (utility, title) in [
                    (Utility::Tasks, "Tasks"),
                    (Utility::Logs, "Logs"),
                    (Utility::Terminal, "Terminal sessions"),
                ] {
                    if ui.button(title).clicked() {
                        self.reveal_utility(utility);
                        ui.close();
                    }
                }
                if ui.button("Command palette    Cmd/Ctrl K").clicked() {
                    self.open_palette();
                    ui.close();
                }
                ui.separator();
                for (mode, title) in [
                    (theme::Mode::System, "System theme"),
                    (theme::Mode::Light, "Light theme"),
                    (theme::Mode::Dark, "Dark theme"),
                ] {
                    if ui.selectable_value(&mut self.mode, mode, title).changed() {
                        self.applied_theme = None;
                    }
                }
            });
            ui.menu_button("Help", |ui| {
                if ui.button("Keyboard shortcuts").clicked() {
                    self.shortcuts_open = true;
                    ui.close();
                }
                ui.label(format!("EasyPanel {}", env!("CARGO_PKG_VERSION")));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_sized(
                        [ui.available_width().min(340.0), 24.0],
                        egui::Button::new(
                            RichText::new("Search resources and commands    Cmd/Ctrl K")
                                .color(palette.muted),
                        )
                        .fill(palette.explorer),
                    )
                    .clicked()
                {
                    self.open_palette();
                }
            });
        });
    }
    fn explorer(&mut self, ui: &mut egui::Ui, palette: Palette, opened: &mut Vec<OpenPage>) {
        ui.spacing_mut().item_spacing.y = 4.0;
        let reveal_active = self.active != self.last_explorer_doc;
        self.last_explorer_doc = self.active;
        let current = self
            .documents
            .iter()
            .find(|doc| Some(doc.id) == self.active);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Explorer").strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::icon(
                    ui,
                    Icon::Refresh,
                    "Refresh explorer inventory",
                    false,
                    palette,
                )
                .clicked()
                {
                    for c in self.catalog.values_mut() {
                        c.requested = false;
                    }
                }
            });
        });
        let response = ui.add(
            egui::TextEdit::singleline(&mut self.explorer_filter)
                .hint_text("Filter resources")
                .desired_width(f32::INFINITY),
        );
        if self.focus == Some(1) {
            response.request_focus();
            self.focus = None;
        }
        ui.add_space(8.0);
        let needle = self.explorer_filter.to_lowercase();
        let mut load = Vec::new();
        egui::ScrollArea::vertical()
            .id_salt("resource-explorer")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.label(RichText::new("EasyPanel").strong());
                if explorer_link(ui, "Fleet health", false).clicked() {
                    opened.push(OpenPage {
                        kind: PageKind::Fleet,
                        target: Target::default(),
                        title: "Fleet health".into(),
                    });
                }
                if explorer_link(ui, "Connections", false).clicked() {
                    opened.push(connection_page());
                }
                if self.loaded && self.servers.is_empty() {
                    ui.add(
                        egui::Label::new(
                            RichText::new(
                                "No servers. Add a connection to discover projects and services.",
                            )
                            .small()
                            .color(palette.muted),
                        )
                        .wrap(),
                    );
                }
                for server in &self.servers {
                    let catalog = self.catalog.entry(server.name.clone()).or_default();
                    let server_matches = server.name.to_lowercase().contains(&needle);
                    let current_server = current.is_some_and(|doc| {
                        doc.page
                            .target
                            .server
                            .as_ref()
                            .is_some_and(|s| s.name == server.name)
                    });
                    explorer_group(
                        ui,
                        egui::Id::unique(("server", self.generation, &server.name)),
                        &server.name,
                        server.default || current_server,
                        reveal_active && current_server,
                        |ui| {
                            if !catalog.requested {
                                load.push(server.clone());
                                catalog.requested = true;
                            }
                            let target = Target {
                                server: Some(server.clone()),
                                ..Target::default()
                            };
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("Projects").strong());
                                if ui.small_button("Manage").clicked() {
                                    opened.push(page(
                                        PageKind::Project,
                                        &target,
                                        "services",
                                        "Projects",
                                    ));
                                }
                            });
                            if let Some(error) = &catalog.error {
                                widgets::error(ui, error);
                                if ui.button("Retry inventory").clicked() {
                                    catalog.requested = false;
                                }
                            }
                            if let Some(data) = &catalog.data {
                                if let (Some(projects), Some(resources)) =
                                    (data["projects"].as_array(), data["services"].as_array())
                                {
                                    for project in projects {
                                        let Some(name) = project
                                            .get("name")
                                            .and_then(Value::as_str)
                                            .or_else(|| project.as_str())
                                        else {
                                            continue;
                                        };
                                        let project_match =
                                            server_matches || name.to_lowercase().contains(&needle);
                                        let children: Vec<_> = resources
                                            .iter()
                                            .filter(|r| r["projectName"].as_str() == Some(name))
                                            .collect();
                                        if !project_match
                                            && !children.iter().any(|r| {
                                                r["name"].as_str().is_some_and(|n| {
                                                    n.to_lowercase().contains(&needle)
                                                })
                                            })
                                        {
                                            continue;
                                        }
                                        let current_project = current_server
                                            && current
                                                .is_some_and(|doc| doc.page.target.project == name);
                                        explorer_group(
                                            ui,
                                            egui::Id::unique(("project", &server.name, name)),
                                            name,
                                            !needle.is_empty() || current_project,
                                            reveal_active && current_project,
                                            |ui| {
                                                let project_target = Target {
                                                    project: name.into(),
                                                    ..target.clone()
                                                };
                                                if explorer_link(ui, "Project settings", false)
                                                    .clicked()
                                                {
                                                    opened.push(page(
                                                        PageKind::Project,
                                                        &project_target,
                                                        "overview",
                                                        name,
                                                    ));
                                                }
                                                for resource in &children {
                                                    let Some(service) = resource["name"].as_str()
                                                    else {
                                                        continue;
                                                    };
                                                    if !project_match
                                                        && !service.to_lowercase().contains(&needle)
                                                    {
                                                        continue;
                                                    }
                                                    let service_target = Target {
                                                        service: service.into(),
                                                        service_type: resource["type"]
                                                            .as_str()
                                                            .unwrap_or("")
                                                            .into(),
                                                        resource: (*resource).clone(),
                                                        ..project_target.clone()
                                                    };
                                                    let selected =
                                                        self.documents.iter().any(|doc| {
                                                            Some(doc.id) == self.active
                                                                && doc.page.target.same_resource(
                                                                    &Target {
                                                                        section: doc
                                                                            .page
                                                                            .target
                                                                            .section
                                                                            .clone(),
                                                                        ..service_target.clone()
                                                                    },
                                                                )
                                                        });
                                                    let response =
                                                        explorer_link(ui, service, selected);
                                                    if response.clicked() {
                                                        opened.push(page(
                                                            PageKind::Service,
                                                            &service_target,
                                                            "overview",
                                                            service,
                                                        ));
                                                    }
                                                    response.context_menu(|ui| {
                                                        resource_actions(
                                                            ui,
                                                            &service_target,
                                                            opened,
                                                        );
                                                    });
                                                }
                                            },
                                        );
                                    }
                                    if projects.is_empty() {
                                        ui.small("No projects. Use Manage to create one.");
                                    }
                                }
                            } else if catalog.error.is_none() {
                                ui.horizontal(|ui| {
                                    ui.spinner();
                                    ui.small("Loading projects…");
                                });
                            }
                            ui.add_space(4.0);
                            explorer_group(
                                ui,
                                egui::Id::unique(("host-tools", &server.name)),
                                "Host tools",
                                false,
                                false,
                                |ui| {
                                    for (section, title) in HOST_SECTIONS {
                                        if (server_matches
                                            || title.to_lowercase().contains(&needle))
                                            && explorer_link(ui, title, false).clicked()
                                        {
                                            opened.push(page(
                                                PageKind::Host,
                                                &target,
                                                section,
                                                title,
                                            ));
                                        }
                                    }
                                },
                            );
                        },
                    );
                }
                ui.add_space(12.0);
                ui.separator();
                ui.add_space(4.0);
                ui.label(RichText::new("Cloudflare").strong());
                if explorer_link(ui, "Accounts", false).clicked() {
                    opened.push(account_page());
                }
                if self.loaded && self.accounts.is_empty() {
                    ui.add(
                        egui::Label::new(
                            RichText::new(
                                "Add an account to manage DNS, tunnels, storage and Workers.",
                            )
                            .small()
                            .color(palette.muted),
                        )
                        .wrap(),
                    );
                }
                for account in &self.accounts {
                    let target = Target {
                        account: Some(account.clone()),
                        ..Target::default()
                    };
                    explorer_group(
                        ui,
                        egui::Id::unique(("cf-account", &account.name)),
                        &account.name,
                        account.default,
                        false,
                        |ui| {
                            for (section, title) in CF_SECTIONS {
                                if (needle.is_empty()
                                    || account.name.to_lowercase().contains(&needle)
                                    || title.to_lowercase().contains(&needle))
                                    && explorer_link(ui, title, false).clicked()
                                {
                                    opened.push(page(
                                        PageKind::Cloudflare,
                                        &target,
                                        section,
                                        title,
                                    ));
                                }
                            }
                        },
                    );
                }
            });
        for server in load {
            let key = format!("catalog:{}:{}", self.generation, server.name);
            let secrets = vec![
                server.token.clone(),
                server
                    .ssh
                    .as_ref()
                    .and_then(|s| s.secret.clone())
                    .unwrap_or_default(),
            ];
            self.backend.submit(
                0,
                &key,
                "Discover projects and services",
                TrustContext {
                    target: server.name.clone(),
                    secrets,
                },
                Lane::Interactive,
                Box::new(move |_| {
                    Ok(Payload::Json(
                        crate::client::EasypanelClient::for_server(&server).call(
                            "projects",
                            "listProjectsAndServices",
                            Value::Null,
                        )?,
                    ))
                }),
            );
        }
    }
    fn tabs(&mut self, ui: &mut egui::Ui, palette: Palette) {
        let width = ui.available_width();
        let reveal = self
            .last_tab_view
            .is_none_or(|(active, strip_width, _)| active != self.active || strip_width != width);
        let mut active_title_width = 0.0;
        let mut close = None;
        egui::ScrollArea::horizontal()
            .id_salt("documents")
            .animated(false)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    for document in self.documents.iter().filter(|doc| !doc.docked) {
                        let active = self.active == Some(document.id);
                        let mut title = document.caption();
                        if document.state.is_dirty() {
                            title.push_str(" •");
                        }
                        let title_width =
                            (title.chars().count() as f32 * 6.8 + 16.0).clamp(100.0, 260.0);
                        if active {
                            active_title_width = title_width;
                        }
                        let mut activated = false;
                        let tab = egui::Frame::new()
                            .fill(if active {
                                palette.canvas
                            } else {
                                palette.explorer
                            })
                            .inner_margin(egui::Margin::symmetric(8, 0))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    if ui
                                        .add_sized(
                                            [title_width, 32.0],
                                            egui::Button::new(title)
                                                .truncate()
                                                .frame(false)
                                                .selected(active),
                                        )
                                        .on_hover_text(document.breadcrumb())
                                        .clicked()
                                    {
                                        self.active = Some(document.id);
                                        activated = true;
                                    }
                                    if theme::icon(
                                        ui,
                                        Icon::Close,
                                        "Close document",
                                        false,
                                        palette,
                                    )
                                    .clicked()
                                    {
                                        close = Some(document.id);
                                    }
                                });
                            });
                        if activated {
                            active_title_width = title_width;
                        }
                        // Reveal once on activation or layout changes, not during manual browsing.
                        let title_resized = self
                            .last_tab_view
                            .is_none_or(|(_, _, previous)| previous != title_width);
                        if (active && (reveal || title_resized)) || activated {
                            tab.response.scroll_to_me(None);
                        }
                    }
                });
            });
        self.last_tab_view = Some((self.active, width, active_title_width));
        if let Some(id) = close {
            self.request_close(id);
        }
    }
    fn document(&mut self, ui: &mut egui::Ui, opened: &mut Vec<OpenPage>) {
        let Some(document) = self
            .documents
            .iter_mut()
            .find(|doc| Some(doc.id) == self.active && !doc.docked)
        else {
            ui.heading("Workspace");
            ui.label("Open a resource from the explorer or search commands.");
            if ui.button("Manage connections").clicked() {
                opened.push(connection_page());
            }
            return;
        };
        if document.page.kind == PageKind::Service {
            let breadcrumb = document.breadcrumb();
            let response = ui
                .allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), 20.0),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.add(
                            egui::Label::new(
                                RichText::new(&breadcrumb)
                                    .small()
                                    .color(ui.visuals().weak_text_color()),
                            )
                            .truncate()
                            .sense(egui::Sense::click()),
                        )
                    },
                )
                .inner
                .on_hover_text(&breadcrumb);
            if self.focus == Some(2) {
                response.request_focus();
                self.focus = None;
            }
        } else {
            ui.horizontal_wrapped(|ui| {
                let breadcrumb = document.breadcrumb();
                let response = ui
                    .add(
                        egui::Label::new(
                            RichText::new(&breadcrumb).color(ui.visuals().weak_text_color()),
                        )
                        .truncate()
                        .sense(egui::Sense::click()),
                    )
                    .on_hover_text(&breadcrumb);
                if self.focus == Some(2) {
                    response.request_focus();
                    self.focus = None;
                }
                if document.page.target.server.is_some() && !document.state.deleted() {
                    resource_actions(ui, &document.page.target, opened);
                }
                if document.page.kind == PageKind::Terminal && ui.button("Dock session").clicked() {
                    match self.utility {
                        Utility::Logs => self.last_logs_doc = self.utility_doc,
                        Utility::Terminal => self.last_terminal_doc = self.utility_doc,
                        Utility::Tasks => {}
                    }
                    document.docked = true;
                    self.utility_doc = Some(document.id);
                    self.utility =
                        if matches!(document.page.target.section.as_str(), "logs" | "log-search") {
                            Utility::Logs
                        } else {
                            Utility::Terminal
                        };
                    self.utility_visible = true;
                    self.active = None;
                }
            });
            ui.separator();
        }
        if document.page.kind == PageKind::Fleet && self.servers.is_empty() {
            ui.add_space(16.0);
            ui.heading("Connect your infrastructure");
            ui.label("Add an EasyPanel server or a Cloudflare account to begin.");
            ui.add_space(12.0);
            let palette = Palette::for_dark(ui.visuals().dark_mode);
            ui.horizontal(|ui| {
                if theme::primary(ui, "Manage connections", palette).clicked() {
                    opened.push(connection_page());
                }
                if ui.button("Cloudflare accounts").clicked() {
                    opened.push(account_page());
                }
            });
            ui.add_space(24.0);
            ui.label("Documents stay attached to their original target while you browse other resources.");
            ui.label(
                "Open service configuration, database queries, logs and transfers side by side.",
            );
            return;
        }
        let mut cx = PageContext {
            target: &document.page.target,
            servers: &self.servers,
            accounts: &self.accounts,
            server_config: &self.path,
            cloudflare_config: &self.cf_path,
            watchlist: &self.checks_path,
            doc: document.id,
            backend: &mut self.backend,
            opened,
            reload: &mut self.reload,
        };
        ui.push_id(document.id, |ui| match &mut document.state {
            PageState::Services(state) if document.page.kind == PageKind::Connections => {
                services::show_connections(ui, state, &mut cx)
            }
            PageState::Services(state) => services::show(ui, state, &mut cx),
            PageState::Host(state) if document.page.kind == PageKind::Fleet => {
                host::show_fleet(ui, state, &mut cx)
            }
            PageState::Host(state) => host::show(ui, state, &mut cx),
            PageState::Cloudflare(state) => cloudflare::show(ui, state, &mut cx),
            PageState::Data(state) => data::show(ui, state, &mut cx),
            PageState::Terminal(state) => terminal::show(ui, state, &mut cx),
        });
        if self.active.is_none() {
            self.active = self
                .documents
                .iter()
                .rev()
                .find(|doc| !doc.docked)
                .map(|doc| doc.id);
        }
    }
    fn utility(&mut self, ui: &mut egui::Ui, palette: Palette, opened: &mut Vec<OpenPage>) {
        let mut close = None;
        ui.horizontal(|ui| {
            for (tab, title) in [
                (Utility::Tasks, "Tasks"),
                (Utility::Logs, "Logs"),
                (Utility::Terminal, "Terminal"),
            ] {
                let response = ui.selectable_label(self.utility == tab, title);
                if self.focus == Some(3) && self.utility == tab {
                    response.request_focus();
                    self.focus = None;
                }
                if response.clicked() {
                    self.reveal_utility(tab);
                }
            }
            if self.utility != Utility::Tasks {
                let logs = self.utility == Utility::Logs;
                let selected = self
                    .documents
                    .iter()
                    .find(|doc| {
                        doc.docked
                            && Some(doc.id) == self.utility_doc
                            && matches!(doc.page.target.section.as_str(), "logs" | "log-search")
                                == logs
                    })
                    .or_else(|| {
                        self.documents.iter().find(|doc| {
                            doc.docked
                                && matches!(doc.page.target.section.as_str(), "logs" | "log-search")
                                    == logs
                        })
                    });
                if let Some(document) = selected {
                    self.utility_doc = Some(document.id);
                    let target = document.page.target.label();
                    let width = (ui.available_width() - 150.0).clamp(120.0, 360.0);
                    // ComboBox::width is a minimum; truncation needs a bounded child.
                    ui.allocate_ui_with_layout(
                        egui::vec2(width, 32.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            egui::ComboBox::from_id_salt("utility-sessions")
                                .width(width)
                                .selected_text(&target)
                                .truncate()
                                .show_ui(ui, |ui| {
                                    ui.set_max_width(600.0);
                                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
                                    for document in self.documents.iter().filter(|doc| {
                                        doc.docked
                                            && matches!(
                                                doc.page.target.section.as_str(),
                                                "logs" | "log-search"
                                            ) == logs
                                    }) {
                                        ui.selectable_value(
                                            &mut self.utility_doc,
                                            Some(document.id),
                                            document.page.target.label(),
                                        );
                                    }
                                })
                                .response
                                .on_hover_text(target);
                        },
                    );
                    if ui
                        .small_button("Close")
                        .on_hover_text("Close selected session")
                        .clicked()
                    {
                        close = self.utility_doc;
                    }
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::icon(ui, Icon::Close, "Hide utility pane", false, palette).clicked() {
                    self.utility_visible = false;
                }
                if self.utility != Utility::Tasks
                    && theme::icon(
                        ui,
                        Icon::Maximize,
                        "Focus session in document workspace",
                        false,
                        palette,
                    )
                    .clicked()
                {
                    if let Some(document) = self
                        .documents
                        .iter_mut()
                        .find(|doc| Some(doc.id) == self.utility_doc)
                    {
                        document.docked = false;
                        self.active = Some(document.id);
                        self.utility_doc = None;
                    }
                }
            });
        });
        ui.separator();
        if self.utility == Utility::Tasks {
            let mut include_polling = ui.data_mut(|d| {
                d.get_temp::<bool>(egui::Id::unique("show-polling-tasks"))
                    .unwrap_or(false)
            });
            ui.checkbox(&mut include_polling, "Include background refreshes");
            ui.data_mut(|d| d.insert_temp(egui::Id::unique("show-polling-tasks"), include_polling));
            if self
                .backend
                .tasks
                .iter()
                .all(|task| task.polling && !include_polling)
            {
                ui.label(
                    RichText::new(
                        "No background operations. Targets, progress and outcomes appear here.",
                    )
                    .color(palette.muted),
                );
            }
            egui::ScrollArea::vertical()
                .id_salt("task-center")
                .show(ui, |ui| {
                    for task in self
                        .backend
                        .tasks
                        .iter()
                        .filter(|task| include_polling || !task.polling)
                    {
                        ui.push_id(task.id, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                let (status, color) = match task.status {
                                    TaskStatus::Queued => ("Queued", palette.muted),
                                    TaskStatus::Running => ("Running", palette.accent),
                                    TaskStatus::Complete => ("Completed", palette.success),
                                    TaskStatus::Partial => ("Partial", palette.warning),
                                    TaskStatus::Failed => ("Failed", palette.error),
                                };
                                ui.label(RichText::new(status).color(color));
                                if ui.selectable_label(false, &task.label).clicked() {
                                    if let Some(document) =
                                        self.documents.iter().find(|doc| doc.id == task.doc)
                                    {
                                        if document.docked {
                                            self.utility_doc = Some(document.id);
                                            self.utility = if matches!(
                                                document.page.target.section.as_str(),
                                                "logs" | "log-search"
                                            ) {
                                                Utility::Logs
                                            } else {
                                                Utility::Terminal
                                            };
                                        } else {
                                            self.active = Some(document.id);
                                        }
                                    }
                                }
                                ui.label(RichText::new(&task.target).small().color(palette.muted));
                                ui.label(
                                    RichText::new(format!("{}s", task.started.elapsed().as_secs()))
                                        .small()
                                        .color(palette.muted),
                                );
                            });
                            ui.add(egui::Label::new(&task.progress).wrap());
                            ui.separator();
                        });
                    }
                });
            return;
        }
        let logs = self.utility == Utility::Logs;
        if let Some(id) = close {
            self.request_close(id);
        }
        let selected = self
            .documents
            .iter()
            .position(|doc| {
                doc.docked
                    && Some(doc.id) == self.utility_doc
                    && matches!(doc.page.target.section.as_str(), "logs" | "log-search") == logs
            })
            .or_else(|| {
                self.documents.iter().position(|doc| {
                    doc.docked
                        && matches!(doc.page.target.section.as_str(), "logs" | "log-search") == logs
                })
            });
        if let Some(index) = selected {
            let document = &mut self.documents[index];
            self.utility_doc = Some(document.id);
            if let PageState::Terminal(state) = &mut document.state {
                let mut cx = PageContext {
                    target: &document.page.target,
                    servers: &self.servers,
                    accounts: &self.accounts,
                    server_config: &self.path,
                    cloudflare_config: &self.cf_path,
                    watchlist: &self.checks_path,
                    doc: document.id,
                    backend: &mut self.backend,
                    opened,
                    reload: &mut self.reload,
                };
                ui.push_id(document.id, |ui| terminal::show_utility(ui, state, &mut cx));
            }
        } else {
            ui.label(if logs {
                "Open live logs or search logs from a service or host resource."
            } else {
                "Open a container, database or host terminal from its resource."
            });
            if let Some(target) = self.current_target().filter(|t| t.server.is_some()) {
                if ui
                    .button(if logs {
                        "Open target logs"
                    } else {
                        "Open target terminal"
                    })
                    .clicked()
                {
                    let section = if logs {
                        if target.service.is_empty() {
                            "log-search"
                        } else {
                            "logs"
                        }
                    } else if target.service.is_empty() {
                        "host"
                    } else {
                        "container"
                    };
                    opened.push(page(
                        PageKind::Terminal,
                        &target,
                        section,
                        if logs { "Logs" } else { "Terminal" },
                    ));
                }
            }
        }
        self.focus = self.focus.filter(|focus| *focus != 3);
    }
    fn command_palette(&mut self, ctx: &egui::Context) {
        if !self.palette_open {
            return;
        }
        let mut commands: Vec<_> = [
            connection_page(),
            account_page(),
            OpenPage {
                kind: PageKind::Fleet,
                target: Target::default(),
                title: "Fleet health".into(),
            },
        ]
        .into_iter()
        .map(PaletteCommand::Open)
        .collect();
        for server in &self.servers {
            let target = Target {
                server: Some(server.clone()),
                ..Target::default()
            };
            for (section, title) in HOST_SECTIONS {
                commands.push(PaletteCommand::Open(page(
                    PageKind::Host,
                    &target,
                    section,
                    &format!("{} / {title}", server.name),
                )));
            }
            if let Some(data) = self.catalog.get(&server.name).and_then(|c| c.data.as_ref()) {
                if let Some(resources) = data["services"].as_array() {
                    for resource in resources {
                        let (Some(project), Some(service), Some(kind)) = (
                            resource["projectName"].as_str(),
                            resource["name"].as_str(),
                            resource["type"].as_str(),
                        ) else {
                            continue;
                        };
                        let target = Target {
                            project: project.into(),
                            service: service.into(),
                            service_type: kind.into(),
                            resource: resource.clone(),
                            ..target.clone()
                        };
                        commands.push(PaletteCommand::Open(page(
                            PageKind::Service,
                            &target,
                            "overview",
                            &format!("{} / {project} / {service}", server.name),
                        )));
                    }
                }
            }
        }
        for account in &self.accounts {
            let target = Target {
                account: Some(account.clone()),
                ..Target::default()
            };
            for (section, title) in CF_SECTIONS {
                commands.push(PaletteCommand::Open(page(
                    PageKind::Cloudflare,
                    &target,
                    section,
                    &format!("{} / {title}", account.name),
                )));
            }
        }
        if let Some(target) = self.current_target() {
            for (kind, section, title) in context_actions(&target) {
                commands.push(PaletteCommand::Open(page(kind, &target, section, title)));
            }
            if target.server.is_some() && !target.service.is_empty() {
                for (action, verb, title) in [
                    (services::QuickAction::Deploy, "deploy", "Deploy service"),
                    (
                        services::QuickAction::Rebuild,
                        "deploy",
                        "Rebuild service without cache",
                    ),
                    (services::QuickAction::Start, "start", "Start service"),
                    (services::QuickAction::Stop, "stop", "Stop service"),
                    (services::QuickAction::Restart, "restart", "Restart service"),
                    (services::QuickAction::Destroy, "destroy", "Destroy service"),
                ] {
                    if crate::lifecycle::ops(&target.service_type, verb).is_some() {
                        commands.push(PaletteCommand::ServiceAction {
                            target: target.clone(),
                            title: format!("{} / {title}", target.service),
                            action,
                        });
                    }
                }
            }
        }
        let mut selected = None;
        let palette_id = egui::Id::unique("command-palette");
        let response = egui::Modal::new(palette_id)
            .area(
                egui::Modal::default_area(palette_id).anchor(egui::Align2::CENTER_TOP, [0.0, 70.0]),
            )
            .show(ctx, |ui| {
                ui.set_width(600.0_f32.min(ctx.content_rect().width() - 48.0));
                ui.heading("Go to resource or command");
                let search = ui.add(
                    egui::TextEdit::singleline(&mut self.palette_query)
                        .hint_text("Search hosts, services, accounts and contextual workspaces")
                        .desired_width(f32::INFINITY),
                );
                if self.palette_focus {
                    search.request_focus();
                    self.palette_focus = false;
                }
                if search.changed() {
                    self.palette_selection = 0;
                }
                let needle = self.palette_query.to_lowercase();
                let filtered: Vec<_> = commands
                    .iter()
                    .enumerate()
                    .filter(|(_, command)| {
                        needle.split_whitespace().all(|word| {
                            command.title().to_lowercase().contains(word)
                                || command.target().label().to_lowercase().contains(word)
                        })
                    })
                    .collect();
                self.palette_selection =
                    self.palette_selection.min(filtered.len().saturating_sub(1));
                if ui.input(|i| i.key_pressed(egui::Key::ArrowDown)) {
                    self.palette_selection =
                        (self.palette_selection + 1).min(filtered.len().saturating_sub(1));
                }
                if ui.input(|i| i.key_pressed(egui::Key::ArrowUp)) {
                    self.palette_selection = self.palette_selection.saturating_sub(1);
                }
                ui.separator();
                egui::ScrollArea::vertical()
                    .max_height(420.0)
                    .show(ui, |ui| {
                        for (index, (command_index, command)) in filtered.iter().enumerate() {
                            let response = ui
                                .selectable_label(self.palette_selection == index, command.title())
                                .on_hover_text(command.target().label());
                            if response.clicked() {
                                selected = Some(*command_index);
                            }
                            if self.palette_selection == index
                                && ui.input(|i| {
                                    i.key_pressed(egui::Key::ArrowDown)
                                        || i.key_pressed(egui::Key::ArrowUp)
                                })
                            {
                                response.scroll_to_me(Some(egui::Align::Center));
                            }
                        }
                        if filtered.is_empty() {
                            ui.label("No matches. Clear the filter or use the explorer.");
                        }
                    });
                ui.small("Up / Down to choose · Enter to open · Escape to dismiss");
                if ui.input(|i| i.key_pressed(egui::Key::Enter)) && !filtered.is_empty() {
                    selected = Some(filtered[self.palette_selection].0);
                }
            });
        if let Some(index) = selected {
            self.execute_palette(commands.swap_remove(index));
            self.palette_open = false;
        } else {
            self.palette_open = !response.should_close();
        }
    }
    fn execute_palette(&mut self, command: PaletteCommand) {
        let PaletteCommand::ServiceAction { target, action, .. } = command else {
            if let PaletteCommand::Open(page) = command {
                self.open(page);
            }
            return;
        };
        let matches_service = |document: &&Document| {
            document.page.kind == PageKind::Service
                && matches!(document.state, PageState::Services(_))
                && document.page.target.same_resource_across_sections(&target)
        };
        let existing = self
            .documents
            .iter()
            .filter(matches_service)
            .find(|document| Some(document.id) == self.active)
            .or_else(|| {
                self.documents
                    .iter()
                    .filter(matches_service)
                    .find(|document| document.state.is_dirty())
            })
            .map(|document| document.id);
        if let Some(id) = existing {
            self.active = Some(id);
            self.last_tab_view = None;
        } else {
            self.open(page(
                PageKind::Service,
                &target,
                "overview",
                &target.service,
            ));
        }
        let mut opened = Vec::new();
        if let Some(document) = self
            .documents
            .iter_mut()
            .find(|doc| Some(doc.id) == self.active)
        {
            if let PageState::Services(state) = &mut document.state {
                let mut cx = PageContext {
                    target: &document.page.target,
                    servers: &self.servers,
                    accounts: &self.accounts,
                    server_config: &self.path,
                    cloudflare_config: &self.cf_path,
                    watchlist: &self.checks_path,
                    doc: document.id,
                    backend: &mut self.backend,
                    opened: &mut opened,
                    reload: &mut self.reload,
                };
                services::quick_action(state, &mut cx, action);
            }
        }
        for page in opened {
            self.open(page);
        }
    }
    fn close_confirmation(&mut self, ctx: &egui::Context) {
        let Some(close) = self.close else {
            return;
        };
        let mut confirmed = false;
        let mut dismissed = false;
        egui::Modal::new(egui::Id::unique("close-workspace")).show(ctx, |ui| {
            ui.set_width(480.0_f32.min(ctx.content_rect().width() - 48.0));
            ui.heading("Close workspace?");
            match close {
                Close::Document(id) => {
                    if let Some(document) = self.documents.iter().find(|doc| doc.id == id) {
                        ui.label(RichText::new(&document.page.title).strong()); ui.add(egui::Label::new(document.page.target.label()).wrap());
                        if document.state.is_dirty() { ui.label("This document has unsaved changes. Closing discards those edits."); }
                        if document.state.running_session() { ui.label("Closing disconnects this terminal. Remote processes may continue running."); }
                        ui.small("Started background operations continue and remain visible in Tasks.");
                    }
                }
                Close::Application => { ui.label("Unsaved edits will be discarded and terminal sessions disconnected."); if self.backend.active() > 0 { ui.label("Background operations are still running. Quitting stops local work; remote operations may continue with a partial outcome."); } }
            }
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button("Keep open").clicked() {
                    dismissed = true;
                }
                if ui.button(match close { Close::Document(_) => "Discard and close", Close::Application => "Quit anyway" }).clicked() {
                    confirmed = true;
                }
            });
        });
        if confirmed {
            self.close = None;
            match close {
                Close::Document(id) => self.remove_document(id),
                Close::Application => {
                    self.allow_quit = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
        if dismissed || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.close = None;
        }
    }
}
impl eframe::App for Desktop {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.pump();
        let ctx = ui.ctx().clone();
        self.shortcuts(&ctx);
        let dark = match self.mode {
            theme::Mode::System => ctx.theme() == egui::Theme::Dark,
            theme::Mode::Light => false,
            theme::Mode::Dark => true,
        };
        let palette = if self.applied_theme != Some(dark) {
            self.applied_theme = Some(dark);
            theme::apply(&ctx, self.mode)
        } else {
            Palette::for_dark(dark)
        };
        let mut opened = Vec::new();
        ctx.data_mut(|data| data.insert_temp(egui::Id::unique("desktop-terminal-focus"), false));
        egui::Panel::top("menubar")
            .exact_size(32.0)
            .frame(
                egui::Frame::new()
                    .fill(palette.chrome)
                    .inner_margin(egui::Margin::symmetric(10, 2)),
            )
            .show(ui, |ui| self.menu(ui, palette, &mut opened));
        egui::Panel::bottom("statusbar")
            .exact_size(24.0)
            .frame(
                egui::Frame::new()
                    .fill(palette.chrome)
                    .inner_margin(egui::Margin::symmetric(10, 2)),
            )
            .show(ui, |ui| {
                ui.spacing_mut().interact_size.y = 20.0;
                ui.spacing_mut().button_padding.y = 2.0;
                ui.horizontal(|ui| {
                    if self.backend.active() > 0 {
                        ui.spinner();
                    }
                    if ui
                        .small_button(format!(
                            "{} hosts / {} accounts",
                            self.servers.len(),
                            self.accounts.len()
                        ))
                        .clicked()
                    {
                        opened.push(connection_page());
                    }
                    let dirty = self
                        .documents
                        .iter()
                        .filter(|doc| doc.state.is_dirty())
                        .count();
                    if dirty > 0 {
                        ui.label(
                            RichText::new(format!("{dirty} unsaved"))
                                .small()
                                .color(palette.warning),
                        );
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .small_button(format!("Tasks · {} active", self.backend.active()))
                            .clicked()
                        {
                            self.reveal_utility(Utility::Tasks);
                        }
                    });
                });
            });
        egui::Panel::left("activitybar")
            .exact_size(46.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(palette.chrome).inner_margin(7))
            .show(ui, |ui| {
                if theme::icon(
                    ui,
                    Icon::Server,
                    "Resource explorer",
                    self.explorer_visible,
                    palette,
                )
                .clicked()
                {
                    self.explorer_visible = !self.explorer_visible;
                }
                if theme::icon(ui, Icon::Cloud, "Cloudflare accounts", false, palette).clicked() {
                    opened.push(account_page());
                }
                if theme::icon(
                    ui,
                    Icon::Search,
                    "Search resources and commands",
                    self.palette_open,
                    palette,
                )
                .clicked()
                {
                    self.open_palette();
                }
                if theme::icon(
                    ui,
                    Icon::Terminal,
                    "Terminal sessions",
                    self.utility_visible && self.utility == Utility::Terminal,
                    palette,
                )
                .clicked()
                {
                    self.reveal_utility(Utility::Terminal);
                }
                if theme::icon(
                    ui,
                    Icon::Tasks,
                    "Tasks",
                    self.utility_visible && self.utility == Utility::Tasks,
                    palette,
                )
                .clicked()
                {
                    self.reveal_utility(Utility::Tasks);
                }
                ui.separator();
                if theme::icon(ui, Icon::Settings, "Manage connections", false, palette).clicked() {
                    opened.push(connection_page());
                }
            });
        if self.explorer_visible {
            egui::Panel::left("explorer")
                .default_size(252.0)
                .size_range(220.0..=280.0)
                .resizable(true)
                .frame(egui::Frame::new().fill(palette.explorer).inner_margin(10))
                .show(ui, |ui| self.explorer(ui, palette, &mut opened));
        }
        if self.utility_visible {
            let panel = egui::Panel::bottom("utility")
                .default_size(220.0)
                .size_range(130.0..=utility_height_limit(ui.available_height()))
                .resizable(true)
                .frame(
                    egui::Frame::new()
                        .fill(palette.canvas)
                        .inner_margin(egui::Margin::symmetric(12, 5)),
                )
                .show(ui, |ui| {
                    // Child content must not expand the user's chosen splitter height.
                    let rect = ui.available_rect_before_wrap();
                    let mut content = ui.new_child(
                        egui::UiBuilder::new()
                            .id_salt("utility-content")
                            .max_rect(rect),
                    );
                    content.shrink_clip_rect(rect);
                    self.utility(&mut content, palette, &mut opened);
                    ui.advance_cursor_after_rect(rect);
                });
            let edge = panel.response.rect;
            // Paint inside the existing resize hit area without another input target or row.
            ui.painter().hline(
                edge.center().x - 18.0..=edge.center().x + 18.0,
                edge.top() + 2.0,
                egui::Stroke::new(2.0, palette.muted),
            );
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(palette.canvas).inner_margin(0))
            .show(ui, |ui| {
                self.tabs(ui, palette);
                ui.separator();
                egui::Frame::new()
                    .inner_margin(egui::Margin::symmetric(12, 4))
                    .show(ui, |ui| {
                        for error in &self.config_errors {
                            widgets::error(ui, error);
                        }
                        if !self.config_errors.is_empty()
                            && ui.button("Reload connection profiles").clicked()
                        {
                            self.reload = true;
                        }
                        if !self.loaded {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label("Reading connection profiles…");
                            });
                        } else {
                            self.document(ui, &mut opened);
                        }
                    });
            });
        for page in opened {
            self.open(page);
        }
        self.command_palette(&ctx);
        self.close_confirmation(&ctx);
        if self.shortcuts_open {
            egui::Window::new("Keyboard shortcuts")
                .open(&mut self.shortcuts_open)
                .resizable(false)
                .show(&ctx, |ui| {
                    for (keys, action) in [
                        ("Cmd/Ctrl K or P", "Search resources and commands"),
                        ("Cmd/Ctrl Shift P", "Palette, including from terminals"),
                        ("Cmd/Ctrl W", "Close active document"),
                        ("Cmd/Ctrl B", "Show or hide explorer"),
                        ("Cmd/Ctrl J", "Show or hide utilities"),
                        ("Cmd/Ctrl 1 / 2 / 3", "Focus explorer / document / utility"),
                        ("Cmd/Ctrl Tab", "Next document"),
                        ("Escape", "Dismiss palette or confirmation"),
                    ] {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(keys).strong());
                            ui.label(action);
                        });
                    }
                });
        }
    }
}

const HOST_SECTIONS: &[(&str, &str)] = &[
    ("overview", "Host overview"),
    ("metrics", "Monitoring"),
    ("nodes", "Nodes"),
    ("storage", "Storage"),
    ("domains", "Domains"),
    ("uptime", "Uptime checks"),
    ("actions", "Action history"),
    ("certificates", "Certificates"),
    ("notifications", "Notifications"),
    ("backups", "Backups"),
    ("maintenance", "Maintenance"),
];
const CF_SECTIONS: &[(&str, &str)] = &[
    ("analytics", "Analytics"),
    ("zones", "Zones and DNS"),
    ("tunnels", "Tunnels"),
    ("r2", "R2 storage"),
    ("workers", "Workers"),
];
fn connection_page() -> OpenPage {
    OpenPage {
        kind: PageKind::Connections,
        target: Target::default(),
        title: "Connections".into(),
    }
}
fn account_page() -> OpenPage {
    OpenPage {
        kind: PageKind::Cloudflare,
        target: Target {
            section: "accounts".into(),
            ..Target::default()
        },
        title: "Cloudflare accounts".into(),
    }
}
fn page(kind: PageKind, target: &Target, section: &str, title: &str) -> OpenPage {
    let identity = if !target.service.is_empty() {
        Some(target.service.as_str())
    } else if !target.project.is_empty() {
        Some(target.project.as_str())
    } else {
        target
            .account
            .as_ref()
            .map(|a| a.name.as_str())
            .or_else(|| target.server.as_ref().map(|s| s.name.as_str()))
    };
    let title = identity
        .filter(|identity| !title.contains(*identity))
        .map_or_else(
            || title.to_owned(),
            |identity| format!("{identity} / {title}"),
        );
    OpenPage {
        kind,
        target: Target {
            section: section.into(),
            ..target.clone()
        },
        title,
    }
}
fn utility_height_limit(available_height: f32) -> f32 {
    (available_height - 320.0).max(130.0)
}

fn explorer_group(
    ui: &mut egui::Ui,
    id: egui::Id,
    title: &str,
    default_open: bool,
    reveal: bool,
    body: impl FnOnce(&mut egui::Ui),
) {
    let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
        ui.ctx(),
        ui.make_persistent_id(id),
        default_open,
    );
    if reveal {
        state.set_open(true);
    }
    let mut clicked = false;
    let mut header = state.show_header(ui, |ui| {
        clicked = explorer_link(ui, title, false).clicked();
    });
    if clicked {
        header.toggle();
    }
    header.body(body);
}

fn explorer_link(ui: &mut egui::Ui, title: &str, selected: bool) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 27.0), egui::Sense::click());
    if selected || response.hovered() || response.has_focus() {
        ui.painter().rect_filled(
            rect,
            3.0,
            if selected {
                ui.visuals().selection.bg_fill
            } else {
                ui.visuals().widgets.hovered.bg_fill
            },
        );
    }
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect.shrink(1.0),
            3.0,
            ui.visuals().selection.stroke,
            egui::StrokeKind::Inside,
        );
    }
    let galley = egui::WidgetText::from(title).into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        (rect.width() - 14.0).max(0.0),
        egui::TextStyle::Body,
    );
    let text_pos = egui::pos2(rect.left() + 7.0, rect.center().y - galley.size().y * 0.5);
    ui.painter()
        .with_clip_rect(rect)
        .galley(text_pos, galley, ui.visuals().text_color());
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::Button, ui.is_enabled(), selected, title)
    });
    response.on_hover_text(title)
}
fn context_actions(target: &Target) -> Vec<(PageKind, &'static str, &'static str)> {
    if target.server.is_none() {
        return Vec::new();
    }
    let mut actions = vec![(PageKind::Terminal, "log-search", "Search all service logs")];
    if !target.service.is_empty() {
        actions.push((PageKind::Service, "environment", "Edit service environment"));
        if target.service_type == "app" {
            actions.extend([
                (PageKind::Service, "source", "Edit service source"),
                (PageKind::Service, "build", "Edit service build"),
                (PageKind::Service, "deploy", "Deployment settings"),
            ]);
        }
        if crate::lifecycle::has_resource_limits(&target.service_type) {
            actions.push((PageKind::Service, "resources", "Edit service resources"));
        }
        actions.extend([
            (PageKind::Terminal, "logs", "Live logs"),
            (PageKind::Terminal, "container", "Container terminal"),
            (PageKind::Data, "clone", "Clone configuration"),
            (PageKind::Data, "migrate", "Migrate configuration"),
            (PageKind::Data, "diff", "Compare configurations"),
        ]);
        if matches!(
            target.service_type.as_str(),
            "mysql" | "mariadb" | "postgres" | "mongo" | "redis"
        ) {
            actions.extend([
                (PageKind::Data, "database", "Database explorer"),
                (PageKind::Data, "credentials", "Database credentials"),
                (PageKind::Terminal, "db", "Database terminal"),
                (PageKind::Data, "dumps", "Database dumps"),
                (PageKind::Data, "restore", "Restore database"),
                (PageKind::Data, "copy", "Copy databases"),
            ]);
        }
    } else {
        actions.push((PageKind::Terminal, "host", "Privileged host terminal"));
        if !target.project.is_empty() {
            actions.extend([
                (
                    PageKind::Data,
                    "project-diff",
                    "Compare project configuration",
                ),
                (
                    PageKind::Data,
                    "export-project",
                    "Export project configuration",
                ),
            ]);
        }
    }
    actions
}
fn resource_actions(ui: &mut egui::Ui, target: &Target, opened: &mut Vec<OpenPage>) {
    if !target.service.is_empty() {
        if ui.button("Live logs").clicked() {
            opened.push(page(PageKind::Terminal, target, "logs", "Live logs"));
        }
        if ui.button("Terminal").clicked() {
            opened.push(page(
                PageKind::Terminal,
                target,
                "container",
                "Container terminal",
            ));
        }
    }
    ui.menu_button("Open workspace", |ui| {
        for (kind, section, title) in context_actions(target) {
            if ui.button(title).clicked() {
                opened.push(page(kind, target, section, title));
                ui.close();
            }
        }
    });
}

#[cfg(test)]
mod layout_tests {
    use super::*;

    #[test]
    fn utility_resize_keeps_a_minimum_document_viewport() {
        for available in [624.0, 744.0, 1024.0] {
            assert_eq!(available - utility_height_limit(available), 320.0);
            assert!(utility_height_limit(available) >= 220.0);
        }
        assert_eq!(utility_height_limit(300.0), 130.0);
    }

    #[test]
    fn service_caption_and_breadcrumb_follow_section_without_retargeting() {
        let document = Document {
            id: 1,
            page: OpenPage {
                kind: PageKind::Service,
                target: Target {
                    project: "commerce".into(),
                    service: "api-é".into(),
                    service_type: "app".into(),
                    section: "environment".into(),
                    ..Target::default()
                },
                title: "api-é / overview".into(),
            },
            state: PageState::Services(Box::default()),
            docked: false,
        };
        assert_eq!(document.caption(), "api-é / Environment");
        assert_eq!(
            document.breadcrumb(),
            "Workspace / commerce / api-é / Environment"
        );
        assert_eq!(
            document.page.target.label(),
            "Workspace / commerce / api-é / environment"
        );
    }

    #[test]
    fn service_breadcrumb_is_left_aligned_bounded_and_accessible_at_workspace_sizes() {
        for width in [1024.0, 1280.0] {
            for mode in [theme::Mode::Light, theme::Mode::Dark] {
                for service in ["api", &"long-service-identity-".repeat(12)] {
                    let ctx = egui::Context::default();
                    ctx.enable_accesskit();
                    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
                    let directory = tempfile::tempdir().unwrap();
                    let mut app = Desktop::new(&cc, directory.path().join("servers.json"));
                    theme::apply(&ctx, mode);
                    app.open(page(
                        PageKind::Service,
                        &Target {
                            project: "commerce".into(),
                            service: service.into(),
                            service_type: "app".into(),
                            ..Target::default()
                        },
                        "environment",
                        "Environment",
                    ));
                    let breadcrumb = app.documents.last().unwrap().breadcrumb();
                    let mut viewport = egui::Rect::NOTHING;
                    let mut root = egui::Rect::NOTHING;
                    let mut output = ctx.run_ui(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(width, 680.0),
                            )),
                            ..Default::default()
                        },
                        |ui| {
                            egui::Panel::left("test-explorer")
                                .exact_size(298.0)
                                .show(ui, |_| {});
                            egui::CentralPanel::default().show(ui, |ui| {
                                viewport = ui.available_rect_before_wrap();
                                app.document(ui, &mut Vec::new());
                            });
                            root = ui.min_rect();
                        },
                    );
                    output.textures_delta.clear();
                    let text = output
                        .shapes
                        .iter()
                        .find_map(|shape| match &shape.shape {
                            egui::Shape::Text(text) if text.galley.text() == breadcrumb => {
                                Some(text)
                            }
                            _ => None,
                        })
                        .expect("breadcrumb must be painted");
                    assert!((text.pos.x - viewport.left()).abs() < 0.5);
                    assert!(text.galley.size().x <= viewport.width() + 0.5);
                    assert!(root.right() <= width + 0.5);
                    let nodes = &output
                        .platform_output
                        .accesskit_update
                        .as_ref()
                        .unwrap()
                        .nodes;
                    // egui exposes Label text as its semantic value, including elided text.
                    assert!(
                        nodes
                            .iter()
                            .any(|(_, node)| node.value() == Some(breadcrumb.as_str())),
                        "breadcrumb semantic text missing: {nodes:#?}"
                    );
                }
            }
        }
    }

    #[test]
    fn utility_focus_control_moves_the_selected_session_without_recreating_it() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let directory = tempfile::tempdir().unwrap();
        let mut app = Desktop::new(&cc, directory.path().join("servers.json"));
        let palette = theme::apply(&ctx, theme::Mode::Light);
        let target = Target {
            project: "operations-with-a-long-project-name".into(),
            service: "service-with-a-long-identity".into(),
            ..Target::default()
        };
        app.open(page(PageKind::Terminal, &target, "container", "Terminal"));
        let id = app.utility_doc.unwrap();
        let index = app.documents.iter().position(|doc| doc.id == id).unwrap();
        let identity = app.documents[index].page.target.clone();
        let PageState::Terminal(state) = &app.documents[index].state else {
            unreachable!();
        };
        let original_state = std::ptr::from_ref(state.as_ref());
        let mut frame = |events| {
            let mut bounds = egui::Rect::NOTHING;
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
                    egui::Panel::left("test-explorer")
                        .exact_size(298.0)
                        .show(ui, |_| {});
                    egui::Panel::bottom("utility")
                        .exact_size(220.0)
                        .frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(12, 5)))
                        .show(ui, |ui| {
                            bounds = ui.available_rect_before_wrap();
                            app.utility(ui, palette, &mut Vec::new());
                        });
                    egui::CentralPanel::default().show(ui, |_| {});
                },
            );
            output.textures_delta.clear();
            (output, bounds)
        };
        let (output, bounds) = frame(Vec::new());
        let control = output
            .platform_output
            .accesskit_update
            .as_ref()
            .unwrap()
            .nodes
            .iter()
            .find(|(_, node)| node.label() == Some("Focus session in document workspace"))
            .unwrap()
            .1
            .bounds()
            .unwrap();
        let control = egui::Rect::from_min_max(
            egui::pos2(control.x0 as f32, control.y0 as f32),
            egui::pos2(control.x1 as f32, control.y1 as f32),
        );
        assert!(bounds.contains_rect(control));
        let pos = control.center();
        for pressed in [true, false] {
            frame(vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ]);
        }
        assert_eq!(app.active, Some(id));
        assert!(!app.documents[index].docked);
        assert!(app.documents[index].page.target.same_resource(&identity));
        let PageState::Terminal(state) = &app.documents[index].state else {
            unreachable!();
        };
        assert_eq!(original_state, std::ptr::from_ref(state.as_ref()));
    }

    #[test]
    fn active_tab_reveals_title_and_close_on_activation_and_resize_but_allows_browsing() {
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
                            app.tabs(ui, Palette::for_dark(true));
                        });
                },
            );
            output.textures_delta.clear();
            (output, viewport)
        }
        fn controls(output: &egui::FullOutput, label: &str) -> Vec<egui::Rect> {
            output
                .platform_output
                .accesskit_update
                .as_ref()
                .unwrap()
                .nodes
                .iter()
                .filter(|(_, node)| node.label() == Some(label))
                .filter_map(|(_, node)| node.bounds())
                .map(|rect| {
                    egui::Rect::from_min_max(
                        egui::pos2(rect.x0 as f32, rect.y0 as f32),
                        egui::pos2(rect.x1 as f32, rect.y1 as f32),
                    )
                })
                .collect()
        }
        fn tab(output: &egui::FullOutput, title: &str) -> (egui::Rect, egui::Rect) {
            let title = controls(output, title)[0];
            let close = controls(output, "Close document")
                .into_iter()
                .filter(|rect| rect.min.x >= title.max.x)
                .min_by(|a, b| a.min.x.total_cmp(&b.min.x))
                .unwrap();
            (title, close)
        }
        fn click(ctx: &egui::Context, app: &mut Desktop, pos: egui::Pos2) {
            for pressed in [true, false] {
                frame(
                    ctx,
                    app,
                    1024.0,
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
        fn browse_left(ctx: &egui::Context, app: &mut Desktop) -> (egui::FullOutput, egui::Rect) {
            frame(
                ctx,
                app,
                1024.0,
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
                frame(ctx, app, 1024.0, Vec::new());
            }
            frame(ctx, app, 1024.0, Vec::new())
        }

        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let directory = tempfile::tempdir().unwrap();
        let mut app = Desktop::new(&cc, directory.path().join("servers.json"));
        let host = Target {
            server: Some(Server {
                name: "alpha-fixture-production-with-a-long-host-name".into(),
                url: "http://127.0.0.1:1".into(),
                token: "fixture-token".into(),
                default: false,
                ssh: None,
            }),
            ..Target::default()
        };
        app.open(page(PageKind::Host, &host, "overview", "Host overview"));
        let service = Target {
            project: "qa-project".into(),
            service: "qa-web".into(),
            service_type: "app".into(),
            ..host
        };
        for section in ["overview", "resources", "source", "environment"] {
            app.open(page(PageKind::Service, &service, section, section));
        }
        let environment = app.active;
        let document_count = app.documents.len();
        for width in [1280.0, 1024.0] {
            frame(&ctx, &mut app, width, Vec::new());
            let (output, viewport) = frame(&ctx, &mut app, width, Vec::new());
            let (title, close) = tab(&output, "qa-web / Environment");
            assert!(
                viewport.contains_rect(title.union(close)),
                "{width}: {title:?} {close:?}"
            );
            assert_eq!(app.active, environment);
            assert_eq!(app.documents.len(), document_count);
        }

        let (output, viewport) = browse_left(&ctx, &mut app);
        let (title, close) = tab(&output, "qa-web / Environment");
        assert!(
            !viewport.contains_rect(title.union(close)),
            "manual scroll snapped back"
        );
        assert_eq!(app.active, environment);
        let fleet = controls(&output, "Fleet health")[0];
        assert!(viewport.contains_rect(fleet));
        click(&ctx, &mut app, fleet.center());
        assert_eq!(app.active, Some(1));

        for _ in 0..2 {
            app.open(page(
                PageKind::Service,
                &service,
                "environment",
                "Environment",
            ));
            frame(&ctx, &mut app, 1024.0, Vec::new());
            let (output, viewport) = frame(&ctx, &mut app, 1024.0, Vec::new());
            let (title, close) = tab(&output, "qa-web / Environment");
            assert!(viewport.contains_rect(title.union(close)));
            assert_eq!(app.active, environment);
            assert_eq!(app.documents.len(), document_count);
            let (output, viewport) = browse_left(&ctx, &mut app);
            let (title, close) = tab(&output, "qa-web / Environment");
            assert!(!viewport.contains_rect(title.union(close)));
        }
        app.open(page(
            PageKind::Service,
            &service,
            "environment",
            "Environment",
        ));
        frame(&ctx, &mut app, 1024.0, Vec::new());
        let (output, viewport) = frame(&ctx, &mut app, 1024.0, Vec::new());
        let (title, close) = tab(&output, "qa-web / Environment");
        assert!(viewport.contains_rect(title.union(close)));
        click(&ctx, &mut app, close.center());
        assert_eq!(app.documents.len(), document_count - 1);
        assert!(!app.documents.iter().any(|doc| Some(doc.id) == environment));
    }

    #[test]
    fn explorer_reveal_preserves_a_later_manual_collapse() {
        egui::__run_test_ui(|ui| {
            ui.set_width(220.0);
            let id = egui::Id::unique("layout-test-group");
            let state_id = ui.make_persistent_id(id);
            let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
                ui.ctx(),
                state_id,
                false,
            );
            state.set_open(false);
            state.store(ui.ctx());
            explorer_group(
                ui,
                id,
                "An intentionally very long host identity",
                true,
                false,
                |_| {},
            );
            assert!(
                !egui::collapsing_header::CollapsingState::load(ui.ctx(), state_id)
                    .unwrap()
                    .is_open()
            );
            let reveal_id = egui::Id::unique("layout-test-reveal");
            explorer_group(
                ui,
                reveal_id,
                "A second intentionally long host identity",
                false,
                true,
                |_| {},
            );
            assert!(egui::collapsing_header::CollapsingState::load(
                ui.ctx(),
                ui.make_persistent_id(reveal_id)
            )
            .unwrap()
            .is_open());
            assert!(ui.min_rect().width() <= 220.0);
        });
    }
}
