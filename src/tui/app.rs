use std::collections::{HashMap, HashSet};
use std::sync::mpsc::Sender;
use std::time::Instant;

use ratatui::layout::Rect;
use ratatui::widgets::{ListState, TableState};
use serde_json::{json, Value};

use crate::commands;
use crate::output::field;

use super::actions::{Menu, Palette};
use super::backup_ui::BackupUi;
use super::form::*;
use super::render::cap;
use super::table::*;
use super::worker::{Refresh, Req, Resp, View};
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
    Viewer,
    /// An embedded container terminal; opened from a service.
    Terminal,
}

/// Viewer is deliberately NOT here: it's the result of opening something on a
/// service, not a destination of its own. As a tab it would just be an empty box
/// until the user arrives from Projects.
pub(super) const TABS: [&str; 7] = [
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
];

/// Tab (by label order) → Screen, the inverse of Screen::index. For clicking a tab.
pub(super) const TAB_SCREENS: [Screen; 7] = [
    Screen::Dashboard,
    Screen::Hosts,
    Screen::Maintenance,
    Screen::Actions,
    Screen::Monitor,
    Screen::Domains,
    Screen::Projects,
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
            // Viewer & Terminal are always opened from Projects, so that tab stays
            // highlighted — neither has its own tab.
            Screen::Viewer | Screen::Terminal => 6,
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
            Screen::Projects => Screen::Dashboard,
            Screen::Viewer | Screen::Terminal => Screen::Dashboard,
        }
    }
    /// The previous tab (for ←). Wraps through TAB_SCREENS; Viewer/Terminal count
    /// as being on the Projects tab (index 6).
    pub(super) fn prev(self) -> Self {
        let i = self.index();
        TAB_SCREENS[(i + TAB_SCREENS.len() - 1) % TAB_SCREENS.len()]
    }
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
pub(super) enum ServerAction {
    Save {
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
    /// The active terminal screen emulator (a vt100 parser fed by WebSocket output).
    pub(super) term_parser: Option<vt100::Parser>,
    /// Send keystrokes/resizes to the WebSocket thread. Dropping it = close the session.
    pub(super) term_input: Option<Sender<super::terminal::TermMsg>>,
    /// The terminal pane title (project/service).
    pub(super) term_title: String,

    pub(super) screen: Screen,
    pub(super) should_quit: bool,
    pub(super) refresh_inflight: bool,
    pub(super) status: String,

    pub(super) stats: Option<Value>,
    pub(super) nodes: Vec<Value>,

    pub(super) actions: Vec<Value>,
    pub(super) actions_state: TableState,
    pub(super) monitor: Vec<Value>,
    pub(super) monitor_state: TableState,
    /// Swarm replicas per service (actual/desired), keyed by "{project}_{service}".
    /// The source of the "down" status in the Services table. Empty = not loaded yet.
    pub(super) task_stats: HashMap<String, (i64, i64)>,
    pub(super) storage: Vec<Value>,
    pub(super) monitor_view: MonitorView,
    pub(super) domains: Vec<Value>,
    pub(super) domains_state: TableState,
    /// The (project, service) origin when entering the Domains tab via `o` from a
    /// service — used to prefill the "New domain" form to that service. None = the
    /// Domains tab was opened normally.
    pub(super) domain_scope: Option<(String, String)>,

    pub(super) projects: Vec<String>,
    /// All services across projects. A flat list replaces the project -> service
    /// hierarchy: drill-down can't be searched and collapses under hundreds of
    /// services.
    pub(super) all_services: Vec<Value>,
    pub(super) services_table: TableState,

    /// The destination screen when Esc'ing from the Viewer — the viewer can be
    /// opened from Services (back to Services) or from Actions (back to Actions).
    pub(super) viewer_from: Screen,
    pub(super) viewer_title: String,
    pub(super) viewer_lines: Vec<String>,
    pub(super) viewer_scroll: u16,
    /// How far right the viewer is scrolled, in columns.
    ///
    /// The viewer neither wraps nor reflows, so a line longer than the pane used
    /// to be simply unreachable — and this is the screen logs open in.
    pub(super) viewer_hscroll: u16,
    pub(super) viewer_ctx: Option<(View, String, String, String)>,
    /// The highlighted row, for the views that ARE rows (ports, mounts,
    /// redirects). Deleting used to be "press the digit printed on the line",
    /// which capped the list at ten and collided with the tab keys.
    pub(super) viewer_row: TableState,
    /// The action whose detail the viewer is showing, if any.
    ///
    /// An action detail has no `viewer_ctx` (it is not a service view), so
    /// `refresh` had nothing to re-send: `r` reported "Refreshing..." and left a
    /// RUNNING deploy's log frozen at the moment it was first fetched — on the
    /// screen you open precisely to watch one.
    pub(super) action_detail: Option<String>,
    /// The newest log timestamp already shown; the resume marker for the tail.
    /// Some = the tail is active (only for View::Logs).
    pub(super) log_cursor: Option<String>,
    /// The viewer sticks to the last line. Logs grow from the bottom, so without
    /// this a new line arrives off-screen and the tail looks dead.
    pub(super) viewer_follow: bool,

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
    /// Comparators to detect a tab/selection change without hooking every handler.
    pub(super) last_screen: Screen,
    pub(super) last_sel: Option<usize>,
    /// Per-tab click hitboxes (start,end column), filled in during render_tabs. Plus its row.
    pub(super) tab_spans: Vec<(u16, u16)>,
    pub(super) tab_row: u16,
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
            restore_from_req: None,
            busy: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            edit_env: None,
            edit_config: None,
            edit_field: None,
            terminal_req: None,
            term_parser: None,
            term_input: None,
            term_title: String::new(),
            screen: Screen::Dashboard,
            should_quit: false,
            refresh_inflight: false,
            status: "Ready".into(),
            stats: None,
            nodes: Vec::new(),
            actions: Vec::new(),
            actions_state: TableState::default(),
            monitor: Vec::new(),
            task_stats: HashMap::new(),
            monitor_state: TableState::default(),
            storage: Vec::new(),
            monitor_view: MonitorView::Services,
            domains: Vec::new(),
            domains_state: TableState::default(),
            domain_scope: None,
            projects: Vec::new(),
            all_services: Vec::new(),
            services_table: TableState::default(),
            viewer_from: Screen::Projects,
            viewer_title: "Viewer".into(),
            viewer_lines: Vec::new(),
            viewer_scroll: 0,
            viewer_row: TableState::default(),
            action_detail: None,
            viewer_hscroll: 0,
            viewer_ctx: None,
            log_cursor: None,
            viewer_follow: false,
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
            last_screen: Screen::Dashboard,
            last_sel: None,
            tab_spans: Vec::new(),
            tab_row: 0,
            table_area: Rect::default(),
            menu: None,
            palette: None,
        }
    }

    /// The number of rows CURRENTLY rendered in the active screen's table (after
    /// filtering). Used by clicks: the clicked index must be within the range
    /// actually on screen.
    pub(super) fn visible_table_len(&self) -> usize {
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
        if matches!(self.screen, Screen::Viewer | Screen::Terminal) {
            self.screen = Screen::Projects;
        }
        self.term_input = None;
        self.term_parser = None;
        self.stats = None;
        self.nodes.clear();
        self.actions.clear();
        self.actions_state = TableState::default();
        self.monitor.clear();
        self.monitor_state = TableState::default();
        self.storage.clear();
        self.domains.clear();
        self.domains_state = TableState::default();
        self.projects.clear();
        self.all_services.clear();
        self.services_table = TableState::default();
        self.viewer_lines.clear();
        self.viewer_ctx = None;
    }

    pub(super) fn handle(&mut self, resp: Resp, req: &Sender<Req>) {
        match resp {
            Resp::Stats(v) => {
                self.refresh_inflight = false;
                self.stats = Some(v);
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
                self.domains = d;
                select_first(&mut self.domains_state, self.domains.len());
            }
            Resp::Projects(p) => self.projects = p,
            Resp::AllServices { projects, services } => {
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
                // The first batch arrives into an empty viewer_lines, so appending
                // = replacing; later rounds append. No need to know which: `since`
                // decides what the server sends.
                if !lines.is_empty() {
                    self.viewer_lines.extend(lines);
                    // An hours-long tail must not pile up without bound.
                    let extra = self.viewer_lines.len().saturating_sub(LOG_BUFFER);
                    self.viewer_lines.drain(..extra);
                }
                if cursor.is_some() {
                    self.log_cursor = cursor;
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
                    lines.push(format!("    {:<21}{:<20}{}", "When", "From", "File"));
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
                self.viewer_from = self.screen;
                self.show_viewer("Done — please read".into(), lines);
                self.viewer_ctx = None;
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
                self.viewer_ctx = None;
                self.status = format!("{} of {} failed", failed.len(), failed.len() + ok.len());
            }
            Resp::Viewer(title, lines) => {
                self.show_viewer(title, lines);
                self.status = "Ready".into();
            }
            Resp::TermOutput(bytes) => {
                if let Some(p) = self.term_parser.as_mut() {
                    p.process(&bytes);
                }
            }
            Resp::TermClosed => {
                // Shell exited / socket closed: back to Services.
                self.term_parser = None;
                self.term_input = None;
                if self.screen == Screen::Terminal {
                    self.screen = Screen::Projects;
                    self.status = format!("Terminal {} closed", self.term_title);
                }
            }
            Resp::Done(msg, what) => {
                self.status = msg;
                self.apply_refresh(what, req);
            }
            Resp::Err(e) => self.status = format!("Error: {e}"),
        }
    }

    /// Put `lines` on screen in the viewer.
    ///
    /// Seven places used to assemble the same five fields by hand, and they had
    /// already drifted: two forgot `viewer_ctx`, one forgot `viewer_from` — so
    /// Esc left it going back to whichever screen the last viewer happened to
    /// record. One definition, and what each caller does DIFFERENTLY is now a
    /// visible line next to it instead of buried in a nine-line block.
    fn show_viewer(&mut self, title: String, lines: Vec<String>) {
        self.viewer_title = title;
        self.viewer_lines = lines;
        self.viewer_scroll = 0;
        self.viewer_hscroll = 0;
        // The SELECTED row resets too. It used to survive, so opening a
        // collection inherited whatever index the last one was left on — a
        // different service, a different resource, a row the user never chose,
        // sitting armed under `x delete`.
        self.viewer_row = TableState::default();
        self.screen = Screen::Viewer;
    }

    /// A viewer whose rows are CHOSEN from: the selection starts on the first
    /// `[n]` row rather than on the heading above it, and Esc returns to the
    /// screen the picker was opened from.
    fn show_picker(&mut self, title: String, lines: Vec<String>) {
        // Captured before `show_viewer` makes the current screen the Viewer.
        self.viewer_from = self.screen;
        self.show_viewer(title, lines);
        let first = self.viewer_lines.iter().position(|l| is_row(l));
        self.viewer_row = TableState::default().with_selected(first);
        self.viewer_ctx = None;
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

    /// The filter shrinks the list, so the selected row can fall out of bounds.
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
        match len {
            0 => state.select(None),
            n => {
                let i = state.selected().unwrap_or(0).min(n - 1);
                state.select(Some(i));
            }
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
        commands::storage_rows(&self.storage)
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
        let all = commands::monitor_rows(&self.monitor);
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
        let Some(p) = self.term_parser.as_mut() else {
            return;
        };
        // vt100 clamps the far end to the history it actually holds, so only the
        // near end needs guarding — holding the key stops at the newest line
        // instead of wrapping past it.
        let at = p.screen().scrollback() as isize;
        p.set_scrollback((at + delta).max(0) as usize);
    }

    pub(super) fn close_terminal(&mut self) {
        self.term_input = None;
        self.term_parser = None;
        self.screen = Screen::Projects;
        self.status = format!("Terminal {} closed", self.term_title);
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

    /// The domains that pass the filter.
    ///
    /// Render AND actions (e/x/P) must both go through here. If render is filtered
    /// while actions use full-list indices, `x` would delete the wrong domain.
    pub(super) fn visible_domains(&self) -> Vec<&Value> {
        self.domains
            .iter()
            .filter(|d| keep(&commands::domain_row(d), &self.filter))
            .collect()
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
                lines.push(format!("Load      {}", commands::load_avg(v)));
            }
        }
        self.viewer_title = format!("Host · {}", h.name);
        self.viewer_lines = lines;
        self.viewer_scroll = 0;
        self.viewer_hscroll = 0;
        self.viewer_from = Screen::Hosts;
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
    /// derived from `viewer_ctx`, so the restore picker — which has no
    /// viewer_ctx — would have rendered as a selectable list in one place and
    /// scrolled like prose in another.
    pub(super) fn viewer_is_collection(&self) -> bool {
        self.backups.restore_into.is_some()
            || self.backups.backup_from.is_some()
            || self
                .viewer_ctx
                .as_ref()
                .is_some_and(|(v, ..)| v.is_collection())
    }

    /// Back the selected database up once, into the panel's storage provider.
    pub(super) fn backup_now(&mut self, req: &Sender<Req>) {
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
        let Some((_, where_to)) = self.backups.provider.clone() else {
            return;
        };
        // Nothing to choose from is an answer, not an empty box: it means the
        // engine could not be asked and the panel recorded no database either.
        if names.is_empty() {
            self.status = format!("No database found in {project}/{service} — nothing to back up");
            return;
        }
        self.backups.names = names;
        self.backups.marked.clear();
        self.backups.header = format!("Back up from {project}/{service} to {where_to}");
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
        self.viewer_row
            .selected()
            .and_then(|r| self.viewer_lines.get(r))
            .and_then(|l| row_index(l))
    }

    pub(super) fn toggle_backup_mark(&mut self) {
        // Row 0 is "All", which is not a database to tick — `toggle` says so.
        if !self.picker_row().is_some_and(|i| self.backups.toggle(i)) {
            self.status = "Move to a database row first".into();
            return;
        }
        // Rebuild in place: the selection must not jump because a tick appeared.
        let keep = self.viewer_row.selected();
        self.viewer_lines = self.backups.picker_lines();
        self.viewer_row.select(keep);
        self.status = self.backups.hint();
    }

    /// Confirm the backup of whatever the picker has selected.
    pub(super) fn ask_backup(&mut self) {
        let Some((project, service)) = self.backups.backup_from.clone() else {
            return;
        };
        let Some((provider, where_to)) = self.backups.provider.clone() else {
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
        self.confirm = Some(Confirm {
            action: "backup".into(),
            project,
            service,
            stype: provider,
            label: format!("Back {what} up to {where_to}?"),
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
                    FormKind::ServerEdit { name } => (
                        name.clone(),
                        form.val(0),
                        match form.val(1) {
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
                self.viewer_lines = vec!["Searching across all services...".into()];
                self.viewer_scroll = 0;
                self.viewer_hscroll = 0;
                self.viewer_follow = false;
                self.log_cursor = None;
                self.viewer_title = format!("Search '{query}'");
                self.viewer_ctx = None;
                self.viewer_from = Screen::Projects;
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
        self.action_detail = None;
        {
            self.viewer_from = Screen::Projects;
            self.viewer_ctx = Some((view, p.clone(), s.clone(), t.clone()));
            self.status = format!("Loading {}...", view.title());
            // A log is a stream, not a document: start empty, stick to the last
            // line, and let the poll lane keep it going. Other views are snapshots
            // and start at the top.
            if view == View::Logs {
                self.viewer_lines.clear();
                self.viewer_scroll = 0;
                self.viewer_hscroll = 0;
                self.log_cursor = None;
                self.viewer_follow = true;
                // Other views switch screens via Resp::Viewer; logs don't go through
                // there, so the switch has to happen here. Without it, Enter would
                // seem to do nothing.
                self.viewer_title = format!("Logs · {p}/{s}");
                self.screen = Screen::Viewer;
                let _ = req.send(Req::LogTail {
                    project: p,
                    service: s,
                    since: None,
                });
                return;
            }
            self.viewer_follow = false;
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
            Screen::Projects => {
                let _ = req.send(Req::AllServices);
                let _ = req.send(Req::MonitorData);
            }
            Screen::Viewer => {
                if let Some((view, p, s, t)) = self.viewer_ctx.clone() {
                    let _ = req.send(Req::Fetch {
                        view,
                        project: p,
                        service: s,
                        stype: t,
                    });
                } else if let Some(id) = self.action_detail.clone() {
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
            Screen::Terminal => {}
            Screen::Maintenance => {
                let _ = req.send(Req::MaintInfo);
            }
            Screen::Dashboard => {}
        }
        self.status = "Refreshing...".into();
    }
}
