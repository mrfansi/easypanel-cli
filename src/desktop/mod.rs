mod backend;

use crate::{config::Server, filter::FilterMatcher};
use backend::{ConnectionForm, Inventory, Job, Reply, Service, Worker};
use eframe::egui::{self, Color32, RichText, TextStyle};
use std::{path::PathBuf, sync::mpsc::TryRecvError};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct Scope {
    server: u64,
    service: u64,
}

impl Scope {
    fn accepts(self, request: Self, service: bool) -> bool {
        self.server == request.server && (!service || self.service == request.service)
    }
}

#[derive(Clone, Copy)]
enum Request {
    Load,
    Save,
    Inventory,
    Details,
    Logs,
    Action,
}

struct Pending {
    request: Request,
    scope: Scope,
    label: String,
}

struct Confirmation {
    server: Server,
    service: Service,
    action: &'static str,
    scope: Scope,
}

pub struct Desktop {
    path: PathBuf,
    worker: Worker,
    pending: Option<Pending>,
    servers: Vec<Server>,
    selected_server: Option<String>,
    project: Option<String>,
    selected: Option<Service>,
    scope: Scope,
    inventory: Inventory,
    search: String,
    focus_search: bool,
    needs_inventory: bool,
    needs_details: bool,
    needs_logs: bool,
    enabled: Option<bool>,
    details_loaded: bool,
    log_text: String,
    logs_loaded: bool,
    logs_tab: bool,
    config_loaded: bool,
    config_error: Option<String>,
    inventory_error: Option<String>,
    details_error: Option<String>,
    logs_error: Option<String>,
    editor: Option<ConnectionForm>,
    editor_error: Option<String>,
    editor_focus: bool,
    confirmation: Option<Confirmation>,
    notice: Option<String>,
}

impl Desktop {
    pub fn new(cc: &eframe::CreationContext<'_>, path: PathBuf) -> Self {
        let mut style = (*cc.egui_ctx.global_style()).clone();
        style.visuals = egui::Visuals::light();
        style.visuals.panel_fill = Color32::from_rgb(244, 246, 248);
        style.visuals.override_text_color = Some(Color32::from_rgb(22, 44, 61));
        style.visuals.selection.bg_fill = Color32::from_rgb(220, 231, 255);
        style.visuals.selection.stroke.color = Color32::from_rgb(36, 94, 204);
        style.spacing.item_spacing = egui::vec2(8.0, 8.0);
        style.spacing.button_padding = egui::vec2(12.0, 8.0);
        style.spacing.interact_size.y = 32.0;
        style
            .text_styles
            .insert(TextStyle::Body, egui::FontId::proportional(15.0));
        style
            .text_styles
            .insert(TextStyle::Button, egui::FontId::proportional(15.0));
        style
            .text_styles
            .insert(TextStyle::Heading, egui::FontId::proportional(22.0));
        cc.egui_ctx.set_global_style(style);
        let worker = Worker::new(path.clone(), cc.egui_ctx.clone());
        let mut app = Self {
            path,
            worker,
            pending: None,
            servers: Vec::new(),
            selected_server: None,
            project: None,
            selected: None,
            scope: Scope::default(),
            inventory: Inventory::default(),
            search: String::new(),
            focus_search: false,
            needs_inventory: false,
            needs_details: false,
            needs_logs: false,
            enabled: None,
            details_loaded: false,
            log_text: String::new(),
            logs_loaded: false,
            logs_tab: false,
            config_loaded: false,
            config_error: None,
            inventory_error: None,
            details_error: None,
            logs_error: None,
            editor: None,
            editor_error: None,
            editor_focus: false,
            confirmation: None,
            notice: None,
        };
        app.dispatch(Job::Load, Request::Load, "Loading connections".into());
        app
    }

    fn server(&self) -> Option<&Server> {
        self.servers
            .iter()
            .find(|s| Some(&s.name) == self.selected_server.as_ref())
    }

    fn safe_text(&self, text: &str) -> String {
        let secrets: Vec<&str> = self
            .servers
            .iter()
            .flat_map(|s| {
                std::iter::once(s.token.as_str())
                    .chain(s.ssh.as_ref().and_then(|ssh| ssh.secret.as_deref()))
            })
            .collect();
        backend::redact(text, &secrets)
    }

    fn dispatch(&mut self, job: Job, request: Request, label: String) {
        if self.pending.is_some() {
            return;
        }
        match self.worker.send(job) {
            Ok(()) => {
                self.pending = Some(Pending {
                    request,
                    scope: self.scope,
                    label,
                })
            }
            Err(error) => match request {
                Request::Load => {
                    self.config_loaded = true;
                    self.config_error = Some(error);
                }
                Request::Save => self.editor_error = Some(error),
                Request::Inventory => self.inventory_error = Some(error),
                Request::Details => self.details_error = Some(error),
                Request::Logs => self.logs_error = Some(error),
                Request::Action => self.notice = Some(error),
            },
        }
    }

    fn clear_service(&mut self) {
        self.scope.service = self.scope.service.wrapping_add(1);
        self.selected = None;
        self.enabled = None;
        self.details_loaded = false;
        self.log_text.clear();
        self.logs_loaded = false;
        self.details_error = None;
        self.logs_error = None;
        self.needs_details = false;
        self.needs_logs = false;
        self.confirmation = None;
    }

    fn select_server(&mut self, name: String) {
        self.scope.server = self.scope.server.wrapping_add(1);
        self.selected_server = Some(name);
        self.project = None;
        self.clear_service();
        self.inventory = Inventory::default();
        self.inventory_error = None;
        self.notice = None;
        self.needs_inventory = true;
    }

    fn select_service(&mut self, service: Service) {
        self.clear_service();
        self.selected = Some(service);
        self.needs_details = true;
        self.needs_logs = self.logs_tab;
    }

    fn receive(&mut self) {
        let result = match self.worker.rx.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                if self.pending.take().is_some() {
                    self.config_loaded = true;
                    self.notice =
                        Some("The request worker closed. Reopen EasyPanel to reconnect.".into());
                }
                return;
            }
        };
        let Some(pending) = self.pending.take() else {
            return;
        };
        let service_request = matches!(pending.request, Request::Details | Request::Logs);
        if !matches!(
            pending.request,
            Request::Load | Request::Save | Request::Action
        ) && !self.scope.accepts(pending.scope, service_request)
        {
            return;
        }
        match result {
            Ok(Reply::Connections(servers)) => {
                self.config_loaded = true;
                self.config_error = None;
                let saved = matches!(pending.request, Request::Save);
                let name = if saved {
                    self.editor.as_ref().map(|form| form.name.trim().to_owned())
                } else {
                    self.selected_server
                        .clone()
                        .filter(|name| servers.iter().any(|s| &s.name == name))
                        .or_else(|| {
                            servers
                                .iter()
                                .find(|s| s.default)
                                .or(servers.first())
                                .map(|s| s.name.clone())
                        })
                };
                self.servers = servers;
                if saved {
                    self.editor = None;
                    self.editor_error = None;
                }
                if let Some(name) = name {
                    self.select_server(name);
                }
            }
            Ok(Reply::Inventory(inventory)) => {
                if self
                    .project
                    .as_ref()
                    .is_some_and(|p| !inventory.projects.contains(p))
                {
                    self.project = None;
                    self.clear_service();
                }
                if self
                    .selected
                    .as_ref()
                    .is_some_and(|s| !inventory.services.contains(s))
                {
                    self.clear_service();
                }
                self.inventory = inventory;
                self.inventory_error = None;
            }
            Ok(Reply::Details(enabled)) => {
                self.enabled = enabled;
                self.details_loaded = true;
                self.details_error = None;
            }
            Ok(Reply::Logs(text)) => {
                self.log_text = self.safe_text(&text);
                self.logs_loaded = true;
                self.logs_error = None;
            }
            Ok(Reply::Action) => {
                self.notice = Some(format!(
                    "{} completed. Refreshing runtime status; readiness may take a moment.",
                    pending.label
                ));
                if self.scope.accepts(pending.scope, false) {
                    self.needs_inventory = true;
                    if self.scope.accepts(pending.scope, true) {
                        self.enabled = None;
                        self.details_loaded = false;
                        self.details_error = None;
                        self.needs_details = true;
                    }
                }
            }
            Err(error) => {
                let mut error = self.safe_text(&error);
                if let Some(form) = &self.editor {
                    error = backend::redact(&error, &[form.token.as_str()]);
                }
                match pending.request {
                    Request::Load => {
                        self.config_loaded = true;
                        self.config_error = Some(error);
                    }
                    Request::Save => self.editor_error = Some(error),
                    Request::Inventory => self.inventory_error = Some(error),
                    Request::Details => self.details_error = Some(error),
                    Request::Logs => self.logs_error = Some(error),
                    Request::Action => {
                        self.notice = Some(format!(
                            "{}: {error}. Outcome may be partial; refresh before retrying.",
                            pending.label
                        ));
                        if self.scope.accepts(pending.scope, false) {
                            self.needs_inventory = true;
                            if self.scope.accepts(pending.scope, true) {
                                self.enabled = None;
                                self.details_loaded = false;
                                self.details_error = None;
                                self.needs_details = true;
                            }
                        }
                    }
                }
            }
        }
    }

    fn pump(&mut self) {
        if self.pending.is_some()
            || !(self.needs_inventory || self.needs_details || self.needs_logs)
        {
            return;
        }
        let Some(server) = self.server().cloned() else {
            return;
        };
        if self.needs_inventory {
            self.needs_inventory = false;
            self.dispatch(
                Job::Inventory(server),
                Request::Inventory,
                "Loading projects, services and runtime status".into(),
            );
        } else if let Some(service) = self.selected.clone() {
            if self.needs_details {
                self.needs_details = false;
                self.dispatch(
                    Job::Details(server, service),
                    Request::Details,
                    "Loading service details".into(),
                );
            } else if self.needs_logs {
                self.needs_logs = false;
                self.dispatch(
                    Job::Logs(server, service),
                    Request::Logs,
                    "Loading recent logs".into(),
                );
            }
        }
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.heading("EasyPanel");
        ui.label("Desktop console");
        ui.add_space(16.0);
        ui.strong("Connections");
        let mut selected_server = None;
        for server in &self.servers {
            if ui
                .add(
                    egui::Button::selectable(
                        self.selected_server.as_ref() == Some(&server.name),
                        &server.name,
                    )
                    .truncate(),
                )
                .on_hover_text(&server.name)
                .clicked()
            {
                selected_server = Some(server.name.clone());
            }
        }
        if let Some(name) = selected_server {
            self.select_server(name);
        }
        let can_edit = self.config_loaded && self.config_error.is_none() && self.pending.is_none();
        ui.horizontal(|ui| {
            if ui
                .add_enabled(can_edit, egui::Button::new("Add server..."))
                .clicked()
            {
                self.editor = Some(ConnectionForm::default());
                self.editor_error = None;
                self.editor_focus = true;
            }
            if ui
                .add_enabled(
                    can_edit && self.server().is_some(),
                    egui::Button::new("Edit..."),
                )
                .clicked()
            {
                if let Some(server) = self.server() {
                    self.editor = Some(ConnectionForm {
                        original: Some(server.name.clone()),
                        name: server.name.clone(),
                        url: server.url.clone(),
                        token: server.token.clone(),
                    });
                    self.editor_error = None;
                    self.editor_focus = true;
                }
            }
        });
        ui.separator();
        ui.strong("Projects");
        if ui
            .selectable_label(self.project.is_none(), "All projects")
            .clicked()
        {
            self.project = None;
            self.clear_service();
        }
        let mut selected_project = None;
        for project in &self.inventory.projects {
            if ui
                .add(
                    egui::Button::selectable(self.project.as_ref() == Some(project), project)
                        .truncate(),
                )
                .on_hover_text(project)
                .clicked()
            {
                selected_project = Some(project.clone());
            }
        }
        if let Some(project) = selected_project {
            self.project = Some(project);
            self.clear_service();
        }
        ui.add_space(16.0);
        ui.small("Connections use the CLI's configuration. Selecting a server here does not change the CLI default.");
    }

    fn content(&mut self, ui: &mut egui::Ui) {
        ui.heading(
            self.selected_server
                .as_deref()
                .unwrap_or("Connect to EasyPanel"),
        );
        ui.label(self.project.as_deref().unwrap_or("All projects"));
        ui.add_space(8.0);
        if let Some(error) = &self.config_error {
            error_text(ui, "Could not read connections", error);
            ui.label("The file has not been changed. Repair it, then reload connections.");
            ui.label(self.path.display().to_string());
            if ui
                .add_enabled(
                    self.pending.is_none(),
                    egui::Button::new("Reload connections"),
                )
                .clicked()
            {
                self.dispatch(Job::Load, Request::Load, "Reloading connections".into());
            }
            return;
        }
        if !self.config_loaded {
            ui.label("Loading your saved connections...");
            return;
        }
        if self.servers.is_empty() {
            ui.add_space(32.0);
            ui.heading("Manage your services in one place");
            ui.label(
                "Add an EasyPanel server URL and API token to see its projects, services and logs.",
            );
            if ui.button("Add your first server...").clicked() {
                self.editor = Some(ConnectionForm::default());
                self.editor_error = None;
                self.editor_focus = true;
            }
            return;
        }
        ui.horizontal(|ui| {
            ui.label("Search services");
            let search = ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .hint_text("Name, project or type")
                    .desired_width((ui.available_width() - 110.0).max(140.0)),
            );
            if self.focus_search {
                search.request_focus();
                self.focus_search = false;
            }
            if ui
                .add_enabled(self.pending.is_none(), egui::Button::new("Refresh"))
                .clicked()
            {
                self.needs_inventory = true;
                self.needs_details = self.selected.is_some();
            }
        });
        if let Some(error) = &self.inventory_error {
            error_text(ui, "Could not refresh services", error);
        }
        if let Some(error) = &self.inventory.status_error {
            ui.label(
                RichText::new(
                    "Runtime status unavailable; configuration is not a running-state signal.",
                )
                .color(Color32::from_rgb(135, 81, 0)),
            );
            ui.small(self.safe_text(error));
        }
        ui.add_space(8.0);
        let matcher = FilterMatcher::new(&self.search);
        let visible: Vec<usize> = self
            .inventory
            .services
            .iter()
            .enumerate()
            .filter_map(|(i, s)| {
                (self.project.as_ref().is_none_or(|p| p == &s.project)
                    && matcher.matches_any([s.name.as_str(), s.project.as_str(), s.kind.as_str()]))
                .then_some(i)
            })
            .collect();
        ui.label(format!("{} services", visible.len()));
        let list_height = (ui.available_height() * 0.43).max(110.0);
        let mut chosen = None;
        egui::ScrollArea::both()
            .id_salt("service_list")
            .max_height(list_height)
            .show(ui, |ui| {
                egui::Grid::new("services")
                    .num_columns(4)
                    .spacing([20.0, 8.0])
                    .striped(true)
                    .show(ui, |ui| {
                        ui.strong("Service");
                        ui.strong("Project");
                        ui.strong("Type");
                        ui.strong("Runtime");
                        ui.end_row();
                        for index in visible.iter().copied() {
                            let service = &self.inventory.services[index];
                            if ui
                                .add_sized(
                                    [170.0, 32.0],
                                    egui::Button::selectable(
                                        self.selected.as_ref() == Some(service),
                                        &service.name,
                                    )
                                    .truncate(),
                                )
                                .on_hover_text(&service.name)
                                .clicked()
                            {
                                chosen = Some(service.clone());
                            }
                            ui.label(&service.project);
                            ui.label(&service.kind);
                            ui.label(self.inventory.status(service));
                            ui.end_row();
                        }
                    });
                if visible.is_empty() {
                    ui.add_space(16.0);
                    ui.label(if !self.search.is_empty() {
                        "No services match this search. Clear or change the search."
                    } else if self.needs_inventory
                        || matches!(
                            self.pending.as_ref().map(|p| p.request),
                            Some(Request::Inventory)
                        )
                    {
                        "Loading services..."
                    } else {
                        "No services in this project. Choose another project or refresh."
                    });
                }
            });
        if let Some(service) = chosen {
            self.select_service(service);
        }
        ui.separator();
        egui::ScrollArea::vertical()
            .id_salt("service_inspector")
            .show(ui, |ui| self.inspector(ui));
    }

    fn inspector(&mut self, ui: &mut egui::Ui) {
        let Some(service) = self.selected.clone() else {
            ui.add_space(16.0);
            ui.strong("Select a service");
            ui.label("View its state, manage its lifecycle, or read recent logs.");
            return;
        };
        ui.label(RichText::new(&service.name).size(18.0).strong());
        ui.label(format!(
            "Project: {}    Type: {}",
            service.project, service.kind
        ));
        ui.label(format!("Runtime: {}", self.inventory.status(&service)));
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.logs_tab, false, "Overview");
            if ui
                .selectable_value(&mut self.logs_tab, true, "Logs")
                .clicked()
                && !self.logs_loaded
            {
                self.needs_logs = true;
            }
        });
        if self.logs_tab {
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(self.pending.is_none(), egui::Button::new("Refresh logs"))
                    .clicked()
                {
                    self.needs_logs = true;
                }
                if ui
                    .add_enabled(
                        self.logs_loaded && !self.log_text.is_empty(),
                        egui::Button::new("Copy logs"),
                    )
                    .clicked()
                {
                    ui.ctx().copy_text(self.log_text.clone());
                }
                ui.small("Most recent 200 lines. Refresh to update.");
            });
            if let Some(error) = &self.logs_error {
                error_text(ui, "Could not load logs", error);
            }
            if !self.logs_loaded {
                if self.logs_error.is_none() {
                    ui.label("Waiting for recent logs...");
                }
            } else if self.log_text.is_empty() {
                ui.label("No recent log lines returned by this service.");
            } else {
                // A borrowed string keeps logs selectable and copyable, but read-only.
                let mut text = self.log_text.as_str();
                ui.add(
                    egui::TextEdit::multiline(&mut text)
                        .font(TextStyle::Monospace)
                        .desired_width(f32::INFINITY)
                        .desired_rows(10),
                );
            }
        } else {
            if let Some(error) = &self.details_error {
                error_text(ui, "Could not load details", error);
            }
            if self.details_error.is_none() {
                ui.label(match (self.details_loaded, self.enabled) {
                    (false, _) => "Loading service configuration...",
                    (true, Some(true)) => "Configuration: Enabled (not a runtime health check)",
                    (true, Some(false)) => "Configuration: Disabled",
                    (true, None) => "Configuration: No enabled flag reported",
                });
            }
            ui.horizontal(|ui| {
                for (action, title) in [
                    ("start", "Start"),
                    ("stop", "Stop..."),
                    ("restart", "Restart..."),
                ] {
                    if ui
                        .add_enabled(
                            self.pending.is_none() && !self.needs_inventory,
                            egui::Button::new(title),
                        )
                        .clicked()
                    {
                        if let Some(server) = self.server().cloned() {
                            if action == "start" {
                                let label = format!(
                                    "Start on {}: {}/{}",
                                    server.name, service.project, service.name
                                );
                                self.dispatch(
                                    Job::Action(server, service.clone(), action),
                                    Request::Action,
                                    label,
                                );
                            } else {
                                self.confirmation = Some(Confirmation {
                                    server,
                                    service: service.clone(),
                                    action,
                                    scope: self.scope,
                                });
                            }
                        }
                    }
                }
            });
            if crate::lifecycle::is_database(&service.kind) {
                ui.small("Database restart disables, then enables the service. Availability may be interrupted.");
            }
            ui.small("Actions request a state change. Refresh checks the actual replica state.");
        }
    }

    fn editor(&mut self, ctx: &egui::Context) {
        let Some(mut form) = self.editor.take() else {
            return;
        };
        let mut save = false;
        let mut cancel = false;
        let saving = matches!(
            self.pending.as_ref().map(|p| p.request),
            Some(Request::Save)
        );
        let response = egui::Modal::new(egui::Id::unique("connection_editor")).show(ctx, |ui| {
            let width = 450.0_f32.min(ctx.content_rect().width() - 64.0);
            ui.set_width(width);
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
            ui.heading(if form.original.is_some() { "Edit server connection" } else { "Add server connection" });
            let label = ui.label("Server name");
            let name = ui.add_enabled(!saving, egui::TextEdit::singleline(&mut form.name).desired_width(width)).labelled_by(label.id);
            if self.editor_focus { name.request_focus(); self.editor_focus = false; }
            let label = ui.label("Panel URL");
            ui.add_enabled(!saving, egui::TextEdit::singleline(&mut form.url).hint_text("https://panel.example.com").desired_width(width)).labelled_by(label.id);
            let label = ui.label("API token");
            ui.add_enabled(!saving, egui::TextEdit::singleline(&mut form.token).password(true).desired_width(width)).labelled_by(label.id);
            ui.small("Stored in the same local credentials file as the CLI. Existing SSH settings and CLI defaults are preserved.");
            if let Some(error) = &self.editor_error { error_text(ui, "Connection not saved", error); }
            ui.horizontal(|ui| {
                save = ui.add_enabled(self.pending.is_none(), egui::Button::new("Save connection")).clicked();
                cancel = ui.add_enabled(!saving, egui::Button::new("Cancel")).clicked();
                if saving { ui.spinner(); ui.label("Saving connection..."); }
            });
        });
        if (cancel || response.should_close()) && !saving {
            self.editor_error = None;
            return;
        }
        if save {
            if form.name.trim().is_empty()
                || form.url.trim().is_empty()
                || form.token.trim().is_empty()
            {
                self.editor_error = Some("Enter a name, panel URL and API token.".into());
            } else {
                // Submission is a snapshot; editing and dismissal are disabled until saved.
                form.name = form.name.trim().to_owned();
                self.dispatch(
                    Job::Save(form.clone()),
                    Request::Save,
                    "Saving connection".into(),
                );
            }
        }
        self.editor = Some(form);
    }

    fn confirm(&mut self, ctx: &egui::Context) {
        let Some(confirmation) = self.confirmation.take() else {
            return;
        };
        if !self.scope.accepts(confirmation.scope, true) {
            return;
        }
        let mut confirmed = false;
        let mut cancel = false;
        let title = if confirmation.action == "stop" {
            "Stop service"
        } else {
            "Restart service"
        };
        let response = egui::Modal::new(egui::Id::unique("confirm_action")).show(ctx, |ui| {
            ui.set_width(420.0_f32.min(ctx.content_rect().width() - 64.0));
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
            ui.heading(format!("{title}?"));
            ui.label(format!("Server: {}", confirmation.server.name));
            ui.label(format!("Project: {}", confirmation.service.project));
            ui.label(format!("Service: {}", confirmation.service.name));
            ui.label("This interrupts the service and may disconnect active users.");
            ui.horizontal(|ui| {
                cancel = ui.button("Cancel").clicked();
                confirmed = ui
                    .add_enabled(self.pending.is_none(), egui::Button::new(title))
                    .clicked();
            });
        });
        if confirmed {
            let label = format!(
                "{title} on {}: {}/{}",
                confirmation.server.name, confirmation.service.project, confirmation.service.name
            );
            self.dispatch(
                Job::Action(
                    confirmation.server,
                    confirmation.service,
                    confirmation.action,
                ),
                Request::Action,
                label,
            );
        } else if !cancel && !response.should_close() {
            self.confirmation = Some(confirmation);
        }
    }
}

impl eframe::App for Desktop {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.receive();
        let ctx = ui.ctx().clone();
        if self.editor.is_none()
            && self.confirmation.is_none()
            && ctx.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::F))
        {
            self.focus_search = true;
        }
        egui::Panel::bottom("activity").show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("activity_messages")
                .max_height(96.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    if let Some(pending) = &self.pending {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.add(egui::Label::new(&pending.label).wrap());
                        });
                    } else {
                        ui.small("Ready");
                    }
                    if let Some(notice) = &self.notice {
                        ui.add(
                            egui::Label::new(self.safe_text(notice))
                                .wrap()
                                .selectable(true),
                        );
                    }
                });
        });
        egui::Panel::left("navigation")
            .default_size(230.0)
            .min_size(220.0)
            .max_size(260.0)
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(233, 238, 243))
                    .inner_margin(16.0),
            )
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| self.sidebar(ui));
            });
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(244, 246, 248))
                    .inner_margin(24.0),
            )
            .show(ui, |ui| self.content(ui));
        self.editor(&ctx);
        self.confirm(&ctx);
        self.pump();
        if self.pending.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }
}

fn error_text(ui: &mut egui::Ui, title: &str, error: &str) {
    ui.label(
        RichText::new(title)
            .strong()
            .color(Color32::from_rgb(165, 44, 44)),
    );
    ui.add(egui::Label::new(error).wrap());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_discards_previous_server_and_service_responses() {
        let before = Scope {
            server: 1,
            service: 1,
        };
        let new_service = Scope {
            server: 1,
            service: 2,
        };
        let new_server = Scope {
            server: 2,
            service: 3,
        };
        assert!(!new_service.accepts(before, true));
        assert!(new_service.accepts(before, false));
        assert!(!new_server.accepts(before, false));
        assert!(before.accepts(before, true));
    }

    fn test_app(worker: Worker) -> Desktop {
        Desktop {
            path: PathBuf::from("unused-test-config.json"),
            worker,
            pending: None,
            servers: Vec::new(),
            selected_server: None,
            project: None,
            selected: Some(Service {
                project: "old".into(),
                name: "old".into(),
                kind: "app".into(),
            }),
            scope: Scope::default(),
            inventory: Inventory::default(),
            search: String::new(),
            focus_search: false,
            needs_inventory: false,
            needs_details: true,
            needs_logs: true,
            enabled: Some(true),
            details_loaded: true,
            log_text: "private old logs".into(),
            logs_loaded: true,
            logs_tab: true,
            config_loaded: true,
            config_error: None,
            inventory_error: None,
            details_error: Some("old".into()),
            logs_error: Some("old".into()),
            editor: None,
            editor_error: None,
            editor_focus: false,
            confirmation: None,
            notice: None,
        }
    }

    #[test]
    fn changing_selection_clears_private_previous_service_data() {
        let mut app = test_app(Worker::with_result(Ok(Reply::Action)));
        app.select_service(Service {
            project: "new".into(),
            name: "new".into(),
            kind: "mysql".into(),
        });
        assert!(app.log_text.is_empty());
        assert!(!app.logs_loaded && !app.details_loaded);
        assert!(app.enabled.is_none() && app.details_error.is_none() && app.logs_error.is_none());
        assert!(app.needs_details && app.needs_logs);
    }

    #[test]
    fn partially_failed_action_invalidates_configuration_and_refreshes_same_service() {
        let mut app = test_app(Worker::with_result(Err("enableService failed".into())));
        app.needs_details = false;
        app.pending = Some(Pending {
            request: Request::Action,
            scope: app.scope,
            label: "Restart on prod: shop/db".into(),
        });

        app.receive();

        assert!(app.enabled.is_none());
        assert!(!app.details_loaded);
        assert!(app.details_error.is_none());
        assert!(app.needs_inventory && app.needs_details);
        assert!(app.pending.is_none());
        let notice = app.notice.as_deref().unwrap();
        assert!(notice.contains("prod: shop/db") && notice.contains("enableService failed"));
    }

    #[test]
    fn old_action_error_does_not_invalidate_new_service_configuration() {
        let mut app = test_app(Worker::with_result(Err("enableService failed".into())));
        app.needs_details = false;
        app.pending = Some(Pending {
            request: Request::Action,
            scope: Scope {
                server: 0,
                service: 9,
            },
            label: "Restart on prod: shop/db".into(),
        });

        app.receive();

        assert_eq!(app.enabled, Some(true));
        assert!(app.details_loaded && !app.needs_details);
        assert!(app.needs_inventory);
        assert!(app.notice.as_deref().unwrap().contains("prod: shop/db"));
    }
}
