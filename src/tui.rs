use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::prelude::*;
use ratatui::widgets::{
    Block, Clear, Gauge, List, ListItem, ListState, Paragraph, Row, Sparkline, Table, TableState,
    Tabs, Wrap,
};
use serde_json::{json, Value};

use crate::client::EasypanelClient;
use crate::commands;
use crate::config::ServerConfig;
use crate::output::{field, format_bytes, format_rate, num, series_last, series_spark};

const REFRESH: Duration = Duration::from_secs(2);

/// Buka TUI untuk server default (atau --server yang sudah di-resolve).
pub fn run(cfg: &ServerConfig, client: EasypanelClient, server_name: String) -> Result<()> {
    if cfg.all().is_empty() {
        println!("Belum ada server. Jalankan: easypanel server add");
        return Ok(());
    }

    let names: Vec<String> = cfg.all().into_iter().map(|s| s.name).collect();
    let mut app = App::new(server_name, names);

    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, &mut app, cfg, client);
    ratatui::restore();
    result
}

fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    cfg: &ServerConfig,
    client: EasypanelClient,
) -> Result<()> {
    let mut w = spawn_workers(client);
    send_initial(&w.user);
    let mut last_stats = Instant::now();

    loop {
        while let Ok(resp) = w.resp.try_recv() {
            app.handle(resp);
        }

        terminal.draw(|f| ui(f, app))?;

        // Metrik jalan di lajur poll. Guard in-flight menjaga agar ronde tak
        // menumpuk saat server lebih lambat dari interval refresh.
        if last_stats.elapsed() >= REFRESH && !app.refresh_inflight {
            let _ = w.poll.send(Req::Stats);
            // Tabel monitor ikut live, tapi hanya saat layarnya dibuka.
            if app.screen == Screen::Monitor {
                let _ = w.poll.send(Req::MonitorData);
            }
            app.refresh_inflight = true;
            last_stats = Instant::now();
        }

        if event::poll(Duration::from_millis(120))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    if key.code == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                    {
                        break;
                    }
                    app.on_key(key.code, &w.user);
                }
            }
        }

        // Ganti server: bangun worker baru (yang lama berhenti saat sender-nya di-drop).
        if let Some(name) = app.switch_to.take() {
            if let Some(server) = cfg.get(&name) {
                w = spawn_workers(EasypanelClient::new(&server.url, &server.token));
                app.reset_for_server(name);
                send_initial(&w.user);
                last_stats = Instant::now();
            }
        }

        if app.should_quit {
            break;
        }
    }
    Ok(())
}

fn send_initial(req_tx: &Sender<Req>) {
    let _ = req_tx.send(Req::Stats);
    let _ = req_tx.send(Req::Nodes);
    let _ = req_tx.send(Req::Projects);
}

// ---------- Worker (network di thread terpisah agar UI tak nge-freeze) ----------

#[derive(Clone, Copy)]
enum View {
    Logs,
    Env,
    Ports,
    Mounts,
    Domains,
    Backups,
}

impl View {
    fn title(self) -> &'static str {
        match self {
            View::Logs => "Logs",
            View::Env => "Env",
            View::Ports => "Ports",
            View::Mounts => "Mounts",
            View::Domains => "Domains",
            View::Backups => "Database backups",
        }
    }
}

enum Req {
    Stats,
    Nodes,
    Projects,
    Actions,
    MonitorData,
    Storage,
    Domains,
    Services(String),
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
    },
}

enum Resp {
    Stats(Value),
    Nodes(Vec<Value>),
    Projects(Vec<String>),
    Actions(Vec<Value>),
    MonitorData(Vec<Value>),
    Storage(Vec<Value>),
    Domains(Vec<Value>),
    Services(String, Vec<(String, String)>),
    Viewer(String, Vec<String>),
    Msg(String),
    Err(String),
}

/// Satu lajur worker: memproses request berurutan dan mengirim hasilnya ke `resp_tx`.
fn spawn_worker(client: EasypanelClient, resp_tx: Sender<Resp>) -> Sender<Req> {
    let (req_tx, req_rx) = mpsc::channel::<Req>();

    thread::spawn(move || {
        while let Ok(req) = req_rx.recv() {
            let resp = handle_req(&client, req);
            if resp_tx.send(resp).is_err() {
                break;
            }
        }
    });

    req_tx
}

/// Dua lajur: `user` untuk aksi user, `poll` untuk metrik periodik.
///
/// getSystemStats/getMonitorTableData bisa makan ~2,5 detik masing-masing. Dengan
/// satu lajur, polling metrik akan menahan aksi user (mis. membuka tab) selama itu.
struct Workers {
    user: Sender<Req>,
    poll: Sender<Req>,
    resp: Receiver<Resp>,
}

fn spawn_workers(client: EasypanelClient) -> Workers {
    let (resp_tx, resp) = mpsc::channel::<Resp>();
    let user = spawn_worker(client.clone(), resp_tx.clone());
    let poll = spawn_worker(client, resp_tx);
    Workers { user, poll, resp }
}

fn handle_req(client: &EasypanelClient, req: Req) -> Resp {
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
        Req::MonitorData => match client.call("metrics", "getAllServicesStats", json!({})) {
            Ok(v) => Resp::MonitorData(v.as_array().cloned().unwrap_or_default()),
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
        Req::Services(project) => {
            match client.call(
                "projects",
                "inspectProject",
                json!({ "projectName": project }),
            ) {
                Ok(v) => Resp::Services(project, parse_services(&v)),
                Err(e) => Resp::Err(e.to_string()),
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
        Req::Action {
            project,
            service,
            stype,
            action,
        } => {
            let mut input = json!({ "projectName": project, "serviceName": service });
            if action == "deploy" {
                input["forceRebuild"] = json!(false);
            }
            match client.call(
                &format!("services/{stype}"),
                &format!("{action}Service"),
                input,
            ) {
                Ok(_) => Resp::Msg(format!("{action} dipicu untuk {project}/{service}")),
                Err(e) => Resp::Err(e.to_string()),
            }
        }
    }
}

fn parse_services(v: &Value) -> Vec<(String, String)> {
    v.get("services")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|s| {
                    (
                        s.get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
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

fn fetch_view(
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
            list_or_empty(&v, "Tidak ada port", |i, p| {
                format!(
                    "[{i}] {} {}->{}",
                    field(p, "/protocol"),
                    field(p, "/published"),
                    field(p, "/target")
                )
            })
        }
        View::Mounts => {
            let v = client.call("mounts", "listMounts", ps)?;
            list_or_empty(&v, "Tidak ada mount", |i, m| {
                let detail = match field(m, "/type").as_str() {
                    "bind" => format!("{} -> {}", field(m, "/hostPath"), field(m, "/mountPath")),
                    "volume" => format!("{} -> {}", field(m, "/name"), field(m, "/mountPath")),
                    _ => field(m, "/mountPath"),
                };
                format!("[{i}] {}  {detail}", field(m, "/type"))
            })
        }
        View::Domains => {
            let v = client.call("domains", "listDomains", ps)?;
            list_or_empty(&v, "Tidak ada domain", |_, d| {
                let scheme = if d.get("https").and_then(Value::as_bool).unwrap_or(false) {
                    "https"
                } else {
                    "http"
                };
                format!(
                    "{} ({scheme})  port {}  [{}]",
                    field(d, "/host"),
                    field(d, "/serviceDestination/port"),
                    field(d, "/id")
                )
            })
        }
        View::Backups => {
            let v = client.call("databaseBackups", "listDatabaseBackups", ps)?;
            list_or_empty(&v, "Tidak ada database backup", |_, b| {
                let state = if b.get("enabled").and_then(Value::as_bool).unwrap_or(false) {
                    "aktif"
                } else {
                    "nonaktif"
                };
                format!(
                    "{}  {}  {}  {state}",
                    field(b, "/id"),
                    field(b, "/databaseName"),
                    field(b, "/schedule")
                )
            })
        }
    };
    Ok(if lines.is_empty() {
        vec!["(kosong)".to_string()]
    } else {
        lines
    })
}

/// Pilih baris pertama bila daftar terisi dan belum ada yang dipilih.
fn select_first(state: &mut TableState, len: usize) {
    if len == 0 {
        state.select(None);
    } else if state.selected().is_none() {
        state.select(Some(0));
    }
}

/// Navigasi tabel: panah/jk, PgUp/PgDn, Home/End.
fn move_table(state: &mut TableState, code: KeyCode, len: usize) {
    if len == 0 {
        return;
    }
    let delta: isize = match code {
        KeyCode::Down | KeyCode::Char('j') => 1,
        KeyCode::Up | KeyCode::Char('k') => -1,
        KeyCode::PageDown => 10,
        KeyCode::PageUp => -10,
        KeyCode::Home => -(len as isize),
        KeyCode::End => len as isize,
        _ => return,
    };
    let cur = state.selected().unwrap_or(0) as isize;
    state.select(Some(
        cur.saturating_add(delta).clamp(0, len as isize - 1) as usize
    ));
}

fn list_or_empty(v: &Value, empty: &str, f: impl Fn(usize, &Value) -> String) -> Vec<String> {
    let arr = v.as_array().cloned().unwrap_or_default();
    if arr.is_empty() {
        return vec![empty.to_string()];
    }
    arr.iter().enumerate().map(|(i, x)| f(i, x)).collect()
}

// ---------- State ----------

#[derive(PartialEq, Clone, Copy)]
enum Screen {
    Dashboard,
    Actions,
    Monitor,
    Domains,
    Projects,
    Viewer,
}

const TABS: [&str; 6] = [
    "Dashboard",
    "Actions",
    "Monitor",
    "Domains",
    "Projects",
    "Viewer",
];

impl Screen {
    fn index(self) -> usize {
        match self {
            Screen::Dashboard => 0,
            Screen::Actions => 1,
            Screen::Monitor => 2,
            Screen::Domains => 3,
            Screen::Projects => 4,
            Screen::Viewer => 5,
        }
    }
    fn next(self) -> Self {
        match self {
            Screen::Dashboard => Screen::Actions,
            Screen::Actions => Screen::Monitor,
            Screen::Monitor => Screen::Domains,
            Screen::Domains => Screen::Projects,
            Screen::Projects => Screen::Viewer,
            Screen::Viewer => Screen::Dashboard,
        }
    }
}

/// Sub-tab pada layar Monitor (mengikuti panel).
#[derive(PartialEq, Clone, Copy)]
enum MonitorView {
    Services,
    Storage,
}

#[derive(PartialEq, Clone, Copy)]
enum Focus {
    Projects,
    Services,
}

struct Confirm {
    action: String,
    project: String,
    service: String,
    stype: String,
    label: String,
}

struct App {
    server_name: String,
    all_servers: Vec<String>,
    switch_to: Option<String>,
    picker: Option<ListState>,

    screen: Screen,
    should_quit: bool,
    refresh_inflight: bool,
    status: String,

    stats: Option<Value>,
    nodes: Vec<Value>,

    actions: Vec<Value>,
    actions_state: TableState,
    monitor: Vec<Value>,
    monitor_state: TableState,
    storage: Vec<Value>,
    monitor_view: MonitorView,
    domains: Vec<Value>,
    domains_state: TableState,

    projects: Vec<String>,
    projects_state: ListState,
    services: Vec<(String, String)>,
    services_state: ListState,
    current_project: Option<String>,
    focus: Focus,

    viewer_title: String,
    viewer_lines: Vec<String>,
    viewer_scroll: u16,
    viewer_ctx: Option<(View, String, String, String)>,

    confirm: Option<Confirm>,
}

impl App {
    fn new(server_name: String, all_servers: Vec<String>) -> Self {
        Self {
            server_name,
            all_servers,
            switch_to: None,
            picker: None,
            screen: Screen::Dashboard,
            should_quit: false,
            refresh_inflight: false,
            status: "Siap".into(),
            stats: None,
            nodes: Vec::new(),
            actions: Vec::new(),
            actions_state: TableState::default(),
            monitor: Vec::new(),
            monitor_state: TableState::default(),
            storage: Vec::new(),
            monitor_view: MonitorView::Services,
            domains: Vec::new(),
            domains_state: TableState::default(),
            projects: Vec::new(),
            projects_state: ListState::default(),
            services: Vec::new(),
            services_state: ListState::default(),
            current_project: None,
            focus: Focus::Projects,
            viewer_title: "Viewer".into(),
            viewer_lines: Vec::new(),
            viewer_scroll: 0,
            viewer_ctx: None,
            confirm: None,
        }
    }

    fn reset_for_server(&mut self, name: String) {
        self.server_name = name;
        self.screen = Screen::Dashboard;
        self.status = "Ganti server".into();
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
        self.projects_state = ListState::default();
        self.services.clear();
        self.services_state = ListState::default();
        self.current_project = None;
        self.focus = Focus::Projects;
        self.viewer_lines.clear();
        self.viewer_ctx = None;
    }

    fn handle(&mut self, resp: Resp) {
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
            Resp::Storage(s) => self.storage = s,
            Resp::Domains(d) => {
                self.domains = d;
                select_first(&mut self.domains_state, self.domains.len());
            }
            Resp::Projects(p) => {
                self.projects = p;
                if self.projects_state.selected().is_none() && !self.projects.is_empty() {
                    self.projects_state.select(Some(0));
                }
            }
            Resp::Services(project, s) => {
                if self.current_project.as_deref() == Some(project.as_str()) {
                    self.services = s;
                    self.services_state.select(if self.services.is_empty() {
                        None
                    } else {
                        Some(0)
                    });
                }
            }
            Resp::Viewer(title, lines) => {
                self.viewer_title = title;
                self.viewer_lines = lines;
                self.viewer_scroll = 0;
                self.screen = Screen::Viewer;
                self.status = "Siap".into();
            }
            Resp::Msg(m) => self.status = m,
            Resp::Err(e) => self.status = format!("Error: {e}"),
        }
    }

    fn on_key(&mut self, code: KeyCode, req: &Sender<Req>) {
        if self.confirm.is_some() {
            self.confirm_key(code, req);
            return;
        }
        if self.picker.is_some() {
            self.picker_key(code);
            return;
        }

        match code {
            KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
            KeyCode::Char('1') => self.screen = Screen::Dashboard,
            KeyCode::Char('2') => self.goto(Screen::Actions, req),
            KeyCode::Char('3') => self.goto(Screen::Monitor, req),
            KeyCode::Char('4') => self.goto(Screen::Domains, req),
            KeyCode::Char('5') => self.goto(Screen::Projects, req),
            KeyCode::Char('6') => self.screen = Screen::Viewer,
            KeyCode::Tab => self.goto(self.screen.next(), req),
            KeyCode::Char('s') => self.open_picker(),
            KeyCode::Char('r') => self.refresh(req),
            _ => match self.screen {
                Screen::Projects => self.projects_key(code, req),
                Screen::Viewer => self.viewer_key(code),
                Screen::Actions => move_table(&mut self.actions_state, code, self.actions.len()),
                Screen::Domains => move_table(&mut self.domains_state, code, self.domains.len()),
                Screen::Monitor => self.monitor_key(code, req),
                Screen::Dashboard => {}
            },
        }
    }

    /// Pindah layar dan muat datanya bila belum ada.
    fn goto(&mut self, screen: Screen, req: &Sender<Req>) {
        self.screen = screen;
        match screen {
            Screen::Projects => {
                if self.projects.is_empty() {
                    let _ = req.send(Req::Projects);
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
            _ => {}
        }
    }

    fn monitor_key(&mut self, code: KeyCode, req: &Sender<Req>) {
        match code {
            KeyCode::Char('v') => {
                self.monitor_view = match self.monitor_view {
                    MonitorView::Services => MonitorView::Storage,
                    MonitorView::Storage => MonitorView::Services,
                };
                self.monitor_state.select(Some(0));
                if self.monitor_view == MonitorView::Storage && self.storage.is_empty() {
                    let _ = req.send(Req::Storage);
                }
            }
            _ => {
                let len = match self.monitor_view {
                    MonitorView::Services => self.monitor.len(),
                    MonitorView::Storage => self.storage.len(),
                };
                move_table(&mut self.monitor_state, code, len);
            }
        }
    }

    fn confirm_key(&mut self, code: KeyCode, req: &Sender<Req>) {
        let c = self.confirm.take().unwrap();
        if matches!(code, KeyCode::Char('y') | KeyCode::Char('Y')) {
            let _ = req.send(Req::Action {
                project: c.project,
                service: c.service,
                stype: c.stype,
                action: c.action.clone(),
            });
            self.status = format!("Mengirim {}...", c.action);
        } else {
            self.status = "Dibatalkan".into();
        }
    }

    fn open_picker(&mut self) {
        if self.all_servers.len() < 2 {
            self.status = "Hanya satu server terkonfigurasi".into();
            return;
        }
        let cur = self
            .all_servers
            .iter()
            .position(|n| n == &self.server_name)
            .unwrap_or(0);
        let mut st = ListState::default();
        st.select(Some(cur));
        self.picker = Some(st);
    }

    fn picker_key(&mut self, code: KeyCode) {
        let Some(state) = self.picker.as_mut() else {
            return;
        };
        match code {
            KeyCode::Esc | KeyCode::Char('s') => self.picker = None,
            KeyCode::Down | KeyCode::Char('j') => {
                let i = (state.selected().unwrap_or(0) + 1).min(self.all_servers.len() - 1);
                state.select(Some(i));
            }
            KeyCode::Up | KeyCode::Char('k') => {
                let i = state.selected().unwrap_or(0).saturating_sub(1);
                state.select(Some(i));
            }
            KeyCode::Enter => {
                if let Some(name) = state
                    .selected()
                    .and_then(|i| self.all_servers.get(i))
                    .cloned()
                {
                    if name != self.server_name {
                        self.switch_to = Some(name);
                    }
                }
                self.picker = None;
            }
            _ => {}
        }
    }

    fn projects_key(&mut self, code: KeyCode, req: &Sender<Req>) {
        match code {
            KeyCode::Left | KeyCode::Char('h') => self.focus = Focus::Projects,
            KeyCode::Right | KeyCode::Char('l') => {
                if self.current_project.is_some() {
                    self.focus = Focus::Services;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
            KeyCode::Enter => self.on_enter(req),
            KeyCode::Char('e') => self.open_view(View::Env, req),
            KeyCode::Char('p') => self.open_view(View::Ports, req),
            KeyCode::Char('m') => self.open_view(View::Mounts, req),
            KeyCode::Char('o') => self.open_view(View::Domains, req),
            KeyCode::Char('b') => self.open_view(View::Backups, req),
            KeyCode::Char('d') => self.ask_action("deploy"),
            KeyCode::Char('R') => self.ask_action("restart"),
            KeyCode::Char('S') => self.ask_action("stop"),
            KeyCode::Char('T') => self.ask_action("start"),
            _ => {}
        }
    }

    fn viewer_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Down | KeyCode::Char('j') => {
                self.viewer_scroll = self.viewer_scroll.saturating_add(1)
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.viewer_scroll = self.viewer_scroll.saturating_sub(1)
            }
            KeyCode::PageDown => self.viewer_scroll = self.viewer_scroll.saturating_add(10),
            KeyCode::PageUp => self.viewer_scroll = self.viewer_scroll.saturating_sub(10),
            _ => {}
        }
    }

    fn move_selection(&mut self, delta: isize) {
        let (state, len) = match self.focus {
            Focus::Projects => (&mut self.projects_state, self.projects.len()),
            Focus::Services => (&mut self.services_state, self.services.len()),
        };
        if len == 0 {
            return;
        }
        let cur = state.selected().unwrap_or(0) as isize;
        let next = (cur + delta).clamp(0, len as isize - 1) as usize;
        state.select(Some(next));
    }

    fn on_enter(&mut self, req: &Sender<Req>) {
        match self.focus {
            Focus::Projects => {
                if let Some(p) = self
                    .projects_state
                    .selected()
                    .and_then(|i| self.projects.get(i).cloned())
                {
                    self.current_project = Some(p.clone());
                    self.services.clear();
                    self.services_state.select(None);
                    self.focus = Focus::Services;
                    let _ = req.send(Req::Services(p));
                }
            }
            Focus::Services => self.open_view(View::Logs, req),
        }
    }

    fn open_view(&mut self, view: View, req: &Sender<Req>) {
        if self.focus != Focus::Services {
            self.status = "Fokus panel Services dulu (→)".into();
            return;
        }
        if let (Some(p), Some((s, t))) = (
            self.current_project.clone(),
            self.services_state
                .selected()
                .and_then(|i| self.services.get(i).cloned()),
        ) {
            self.viewer_ctx = Some((view, p.clone(), s.clone(), t.clone()));
            self.status = format!("Memuat {}...", view.title());
            let _ = req.send(Req::Fetch {
                view,
                project: p,
                service: s,
                stype: t,
            });
        }
    }

    fn ask_action(&mut self, action: &str) {
        if self.focus != Focus::Services {
            self.status = "Fokus panel Services dulu (→) untuk aksi".into();
            return;
        }
        if let (Some(p), Some((s, t))) = (
            self.current_project.clone(),
            self.services_state
                .selected()
                .and_then(|i| self.services.get(i).cloned()),
        ) {
            self.confirm = Some(Confirm {
                action: action.to_string(),
                project: p,
                service: s.clone(),
                stype: t,
                label: format!("{} service '{}'?", cap(action), s),
            });
        }
    }

    fn refresh(&mut self, req: &Sender<Req>) {
        let _ = req.send(Req::Stats);
        let _ = req.send(Req::Nodes);
        match self.screen {
            Screen::Projects => {
                let _ = req.send(Req::Projects);
                if let Some(p) = self.current_project.clone() {
                    let _ = req.send(Req::Services(p));
                }
            }
            Screen::Viewer => {
                if let Some((view, p, s, t)) = self.viewer_ctx.clone() {
                    let _ = req.send(Req::Fetch {
                        view,
                        project: p,
                        service: s,
                        stype: t,
                    });
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
            Screen::Dashboard => {}
        }
        self.status = "Refresh...".into();
    }

    fn selected_service(&self) -> Option<&(String, String)> {
        self.services_state
            .selected()
            .and_then(|i| self.services.get(i))
    }
}

// ---------- Render ----------

fn ui(f: &mut Frame, app: &mut App) {
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .split(f.area());

    render_tabs(f, chunks[0], app);
    match app.screen {
        Screen::Dashboard => render_dashboard(f, chunks[1], app),
        Screen::Actions => render_actions(f, chunks[1], app),
        Screen::Monitor => render_monitor(f, chunks[1], app),
        Screen::Domains => render_domains(f, chunks[1], app),
        Screen::Projects => render_projects(f, chunks[1], app),
        Screen::Viewer => render_viewer(f, chunks[1], app),
    }
    render_status(f, chunks[2], app);

    if let Some(c) = &app.confirm {
        render_confirm(f, c);
    }
    if app.picker.is_some() {
        render_picker(f, app);
    }
}

fn render_tabs(f: &mut Frame, area: Rect, app: &App) {
    let tabs = Tabs::new(TABS.to_vec())
        .select(app.screen.index())
        .block(Block::bordered().title(format!(" EasyPanel — {} ", app.server_name)))
        .highlight_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        );
    f.render_widget(tabs, area);
}

fn render_dashboard(f: &mut Frame, area: Rect, app: &App) {
    let stats = app.stats.clone().unwrap_or(Value::Null);

    let rows = Layout::vertical([Constraint::Length(11), Constraint::Min(0)]).split(area);
    let top =
        Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)]).split(rows[0]);

    let left = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Length(2),
    ])
    .split(top[0]);
    render_gauge(f, left[0], "CPU", series_last(&stats, "cpu"));
    render_gauge(f, left[1], "Memory", series_last(&stats, "memory"));
    render_gauge(f, left[2], "Disk", series_last(&stats, "disk"));
    f.render_widget(
        Paragraph::new(format!(
            " {} cores — load {}",
            field(&stats, "/cpuCores"),
            commands::load_avg(&stats)
        )),
        left[3],
    );

    let spark = Sparkline::default()
        .block(Block::bordered().title(" CPU History (%) "))
        .data(series_spark(&stats, "cpu", 120))
        .max(100)
        .style(Style::default().fg(Color::Cyan));
    f.render_widget(spark, top[1]);

    render_nodes(f, rows[1], app);
}

fn render_gauge(f: &mut Frame, area: Rect, label: &str, pct: f64) {
    let g = Gauge::default()
        .block(Block::bordered().title(format!(" {label} ")))
        .gauge_style(Style::default().fg(gauge_color(pct)))
        .ratio((pct / 100.0).clamp(0.0, 1.0))
        .label(format!("{pct:.1}%"));
    f.render_widget(g, area);
}

fn render_nodes(f: &mut Frame, area: Rect, app: &App) {
    let header = Row::new(["Hostname", "Role", "State", "Availability", "Addr"])
        .style(Style::default().add_modifier(Modifier::BOLD));
    let rows = app.nodes.iter().map(|n| {
        Row::new([
            field(n, "/Description/Hostname"),
            field(n, "/Spec/Role"),
            field(n, "/Status/State"),
            field(n, "/Spec/Availability"),
            field(n, "/Status/Addr"),
        ])
    });
    let table = Table::new(
        rows,
        [
            Constraint::Percentage(30),
            Constraint::Percentage(15),
            Constraint::Percentage(15),
            Constraint::Percentage(20),
            Constraint::Percentage(20),
        ],
    )
    .header(header)
    .block(Block::bordered().title(" Nodes "));
    f.render_widget(table, area);
}

fn render_projects(f: &mut Frame, area: Rect, app: &mut App) {
    let cols = Layout::horizontal([
        Constraint::Percentage(28),
        Constraint::Percentage(34),
        Constraint::Percentage(38),
    ])
    .split(area);

    let p_items: Vec<ListItem> = app
        .projects
        .iter()
        .map(|p| ListItem::new(p.clone()))
        .collect();
    let p_list = List::new(p_items)
        .block(focus_block(" Projects ", app.focus == Focus::Projects))
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("› ");
    f.render_stateful_widget(p_list, cols[0], &mut app.projects_state);

    let title = match &app.current_project {
        Some(p) => format!(" Services · {p} "),
        None => " Services ".to_string(),
    };
    let s_items: Vec<ListItem> = app
        .services
        .iter()
        .map(|(n, t)| ListItem::new(format!("{n}  ({t})")))
        .collect();
    let s_list = List::new(s_items)
        .block(focus_block(&title, app.focus == Focus::Services))
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("› ");
    f.render_stateful_widget(s_list, cols[1], &mut app.services_state);

    let detail = match app.selected_service() {
        Some((s, t)) => format!(
            "Service : {s}\nTipe    : {t}\n\nView (fokus Services):\n  [Enter] Logs   [e] Env\n  [p] Ports      [m] Mounts\n  [o] Domains    [b] Backups\n\nAksi:\n  [d] Deploy  [R] Restart\n  [S] Stop    [T] Start",
        ),
        None => "Enter pada project untuk memuat service.\n→ untuk fokus panel Services.".to_string(),
    };
    f.render_widget(
        Paragraph::new(detail)
            .block(Block::bordered().title(" Detail "))
            .wrap(Wrap { trim: false }),
        cols[2],
    );
}

/// Tabel dengan state + highlight baris terpilih.
fn render_table(
    f: &mut Frame,
    area: Rect,
    title: String,
    headers: &[&str],
    widths: &[Constraint],
    rows: Vec<Vec<String>>,
    state: &mut TableState,
) {
    let header = Row::new(headers.to_vec()).style(
        Style::default()
            .fg(Color::Black)
            .bg(Color::Green)
            .add_modifier(Modifier::BOLD),
    );
    let table = Table::new(rows.into_iter().map(Row::new), widths.to_vec())
        .header(header)
        .block(Block::bordered().title(title))
        .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("› ");
    f.render_stateful_widget(table, area, state);
}

fn render_actions(f: &mut Frame, area: Rect, app: &mut App) {
    let rows: Vec<Vec<String>> = app
        .actions
        .iter()
        .map(|a| commands::action_row(a, commands::ACTION_DESC_TUI))
        .collect();
    render_table(
        f,
        area,
        format!(" Actions ({}) ", app.actions.len()),
        &commands::ACTION_HEADERS,
        &[
            Constraint::Length(8),
            Constraint::Length(28),
            Constraint::Min(20),
            Constraint::Length(10),
            Constraint::Length(14),
        ],
        rows,
        &mut app.actions_state,
    );
}

fn render_domains(f: &mut Frame, area: Rect, app: &mut App) {
    let rows: Vec<Vec<String>> = app.domains.iter().map(commands::domain_row).collect();
    render_table(
        f,
        area,
        format!(" Domains ({}) ", app.domains.len()),
        &commands::DOMAIN_HEADERS,
        &[
            Constraint::Percentage(45),
            Constraint::Percentage(37),
            Constraint::Percentage(18),
        ],
        rows,
        &mut app.domains_state,
    );
}

fn render_monitor(f: &mut Frame, area: Rect, app: &mut App) {
    let rows = Layout::vertical([Constraint::Length(8), Constraint::Min(0)]).split(area);
    render_tiles(f, rows[0], app);

    match app.monitor_view {
        MonitorView::Services => {
            let data = commands::monitor_rows(app.monitor.clone());
            render_table(
                f,
                rows[1],
                format!(" Services ({}) · [v] Storage ", app.monitor.len()),
                &commands::MONITOR_HEADERS,
                &[
                    Constraint::Min(20),
                    Constraint::Length(9),
                    Constraint::Length(11),
                    Constraint::Length(11),
                    Constraint::Length(11),
                ],
                data,
                &mut app.monitor_state,
            );
        }
        MonitorView::Storage => {
            let data = commands::storage_rows(app.storage.clone());
            render_table(
                f,
                rows[1],
                format!(" Storage ({}) · [v] Services ", app.storage.len()),
                &commands::STORAGE_HEADERS,
                &[
                    Constraint::Length(20),
                    Constraint::Length(18),
                    Constraint::Length(11),
                    Constraint::Min(20),
                ],
                data,
                &mut app.monitor_state,
            );
        }
    }
}

/// Lima tile metrik dengan histori (CPU, Memory, Disk, Net In, Net Out).
fn render_tiles(f: &mut Frame, area: Rect, app: &App) {
    let s = app.stats.clone().unwrap_or(Value::Null);
    let pair = |used: &str, total: &str| {
        format!(
            "{} / {}",
            format_bytes(num(&s, used)),
            format_bytes(num(&s, total))
        )
    };

    let tiles = [
        (
            "CPU",
            format!("{:.1}%", series_last(&s, "cpu")),
            format!(
                "{} cores — load {}",
                field(&s, "/cpuCores"),
                commands::load_avg(&s)
            ),
            series_spark(&s, "cpu", 60),
            Color::Yellow,
        ),
        (
            "Memory",
            format!("{:.1}%", series_last(&s, "memory")),
            pair("/memoryUsedBytes", "/memoryTotalBytes"),
            series_spark(&s, "memory", 60),
            Color::Blue,
        ),
        (
            "Disk",
            format!("{:.1}%", series_last(&s, "disk")),
            pair("/diskUsedBytes", "/diskTotalBytes"),
            series_spark(&s, "disk", 60),
            Color::Green,
        ),
        (
            "Network In",
            format_rate(series_last(&s, "networkIn")),
            String::new(),
            series_spark(&s, "networkIn", 60),
            Color::Cyan,
        ),
        (
            "Network Out",
            format_rate(series_last(&s, "networkOut")),
            String::new(),
            series_spark(&s, "networkOut", 60),
            Color::Magenta,
        ),
    ];

    let cols = Layout::horizontal([Constraint::Ratio(1, 5); 5]).split(area);
    for (i, (label, value, sub, data, color)) in tiles.into_iter().enumerate() {
        let inner = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(cols[i].inner(Margin::new(1, 1)));
        f.render_widget(Block::bordered().title(format!(" {label} ")), cols[i]);
        f.render_widget(
            Paragraph::new(value).style(Style::default().add_modifier(Modifier::BOLD)),
            inner[0],
        );
        f.render_widget(
            Paragraph::new(sub).style(Style::default().fg(Color::DarkGray)),
            inner[1],
        );
        f.render_widget(
            Sparkline::default()
                .data(data)
                .max(100)
                .style(Style::default().fg(color)),
            inner[2],
        );
    }
}

fn render_viewer(f: &mut Frame, area: Rect, app: &App) {
    f.render_widget(
        Paragraph::new(app.viewer_lines.join("\n"))
            .block(Block::bordered().title(format!(" {} ", app.viewer_title)))
            .scroll((app.viewer_scroll, 0)),
        area,
    );
}

fn render_status(f: &mut Frame, area: Rect, app: &App) {
    let keys = match app.screen {
        Screen::Dashboard => "1-6/Tab tab · s server · r refresh · q keluar",
        Screen::Actions => "↑↓ pilih · PgUp/PgDn · r refresh · 1-6 tab · q keluar",
        Screen::Monitor => "v Services/Storage · ↑↓ pilih · r refresh · 1-6 tab · q keluar",
        Screen::Domains => "↑↓ pilih · PgUp/PgDn · r refresh · 1-6 tab · q keluar",
        Screen::Projects => {
            "↑↓ pilih · ←→ panel · Enter logs · e/p/m/o/b view · d/R/S/T aksi · s server · q keluar"
        }
        Screen::Viewer => "↑↓ scroll · PgUp/PgDn · r refresh · 1-6 tab · q keluar",
    };
    f.render_widget(
        Paragraph::new(format!(" {keys}   |   {}", app.status))
            .style(Style::default().bg(Color::Blue).fg(Color::White)),
        area,
    );
}

fn render_confirm(f: &mut Frame, c: &Confirm) {
    let area = centered(52, 22, f.area());
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(format!(
            "\n{}\n\nMemengaruhi service nyata.\n\n[y] Ya      [n] Batal",
            c.label
        ))
        .alignment(Alignment::Center)
        .block(
            Block::bordered()
                .title(" Konfirmasi ")
                .border_style(Style::default().fg(Color::Yellow)),
        ),
        area,
    );
}

fn render_picker(f: &mut Frame, app: &mut App) {
    let area = centered(46, 50, f.area());
    f.render_widget(Clear, area);
    let items: Vec<ListItem> = app
        .all_servers
        .iter()
        .map(|n| {
            let mark = if n == &app.server_name {
                " (aktif)"
            } else {
                ""
            };
            ListItem::new(format!("{n}{mark}"))
        })
        .collect();
    let list = List::new(items)
        .block(
            Block::bordered()
                .title(" Pilih server (Enter) ")
                .border_style(Style::default().fg(Color::Yellow)),
        )
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("› ");
    let state = app.picker.as_mut().unwrap();
    f.render_stateful_widget(list, area, state);
}

// ---------- Helpers ----------

fn focus_block(title: &str, focused: bool) -> Block<'_> {
    let block = Block::bordered().title(title.to_string());
    if focused {
        block.border_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        block
    }
}

fn gauge_color(pct: f64) -> Color {
    if pct < 70.0 {
        Color::Green
    } else if pct < 90.0 {
        Color::Yellow
    } else {
        Color::Red
    }
}

fn cap(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(first) => first.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

fn centered(pct_x: u16, pct_y: u16, r: Rect) -> Rect {
    let v = Layout::vertical([
        Constraint::Percentage((100 - pct_y) / 2),
        Constraint::Percentage(pct_y),
        Constraint::Percentage((100 - pct_y) / 2),
    ])
    .split(r);
    Layout::horizontal([
        Constraint::Percentage((100 - pct_x) / 2),
        Constraint::Percentage(pct_x),
        Constraint::Percentage((100 - pct_x) / 2),
    ])
    .split(v[1])[1]
}
