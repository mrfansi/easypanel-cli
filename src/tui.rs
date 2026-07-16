use std::collections::VecDeque;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::prelude::*;
use ratatui::widgets::{
    Block, Clear, Gauge, List, ListItem, ListState, Paragraph, Row, Sparkline, Table, Tabs, Wrap,
};
use serde_json::{json, Value};

use crate::client::EasypanelClient;
use crate::config::ServerConfig;
use crate::output::field;

const CPU_HISTORY: usize = 120;
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
    let (mut req_tx, mut resp_rx) = spawn_worker(client);
    send_initial(&req_tx);
    let mut last_stats = Instant::now();

    loop {
        while let Ok(resp) = resp_rx.try_recv() {
            app.handle(resp);
        }

        terminal.draw(|f| ui(f, app))?;

        if last_stats.elapsed() >= REFRESH {
            let _ = req_tx.send(Req::Stats);
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
                    app.on_key(key.code, &req_tx);
                }
            }
        }

        // Ganti server: bangun worker baru (worker lama berhenti saat req_tx lama di-drop).
        if let Some(name) = app.switch_to.take() {
            if let Some(server) = cfg.get(&name) {
                let (tx, rx) = spawn_worker(EasypanelClient::new(&server.url, &server.token));
                req_tx = tx;
                resp_rx = rx;
                app.reset_for_server(name);
                send_initial(&req_tx);
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
    Services(String, Vec<(String, String)>),
    Viewer(String, Vec<String>),
    Msg(String),
    Err(String),
}

fn spawn_worker(client: EasypanelClient) -> (Sender<Req>, Receiver<Resp>) {
    let (req_tx, req_rx) = mpsc::channel::<Req>();
    let (resp_tx, resp_rx) = mpsc::channel::<Resp>();

    thread::spawn(move || {
        while let Ok(req) = req_rx.recv() {
            let resp = handle_req(&client, req);
            if resp_tx.send(resp).is_err() {
                break;
            }
        }
    });

    (req_tx, resp_rx)
}

fn handle_req(client: &EasypanelClient, req: Req) -> Resp {
    match req {
        Req::Stats => match client.call("monitorOld", "getSystemStats", Value::Null) {
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
    Projects,
    Viewer,
}

impl Screen {
    fn index(self) -> usize {
        match self {
            Screen::Dashboard => 0,
            Screen::Projects => 1,
            Screen::Viewer => 2,
        }
    }
    fn next(self) -> Self {
        match self {
            Screen::Dashboard => Screen::Projects,
            Screen::Projects => Screen::Viewer,
            Screen::Viewer => Screen::Dashboard,
        }
    }
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
    status: String,

    stats: Option<Value>,
    cpu_history: VecDeque<u64>,
    nodes: Vec<Value>,

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
            status: "Siap".into(),
            stats: None,
            cpu_history: VecDeque::with_capacity(CPU_HISTORY),
            nodes: Vec::new(),
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
        self.cpu_history.clear();
        self.nodes.clear();
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
                let cpu = num(&v, "/cpuInfo/usedPercentage").round() as u64;
                self.cpu_history.push_back(cpu.min(100));
                while self.cpu_history.len() > CPU_HISTORY {
                    self.cpu_history.pop_front();
                }
                self.stats = Some(v);
            }
            Resp::Nodes(n) => self.nodes = n,
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
            KeyCode::Char('2') => self.goto_projects(req),
            KeyCode::Char('3') => self.screen = Screen::Viewer,
            KeyCode::Tab => {
                self.screen = self.screen.next();
                if self.screen == Screen::Projects {
                    self.goto_projects(req);
                }
            }
            KeyCode::Char('s') => self.open_picker(),
            KeyCode::Char('r') => self.refresh(req),
            _ => match self.screen {
                Screen::Projects => self.projects_key(code, req),
                Screen::Viewer => self.viewer_key(code),
                Screen::Dashboard => {}
            },
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

    fn goto_projects(&mut self, req: &Sender<Req>) {
        self.screen = Screen::Projects;
        if self.projects.is_empty() {
            let _ = req.send(Req::Projects);
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
    let tabs = Tabs::new(vec!["Dashboard", "Projects", "Viewer"])
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
    let cpu = num(&stats, "/cpuInfo/usedPercentage");
    let mem = num(&stats, "/memInfo/usedMemPercentage");
    let disk = num(&stats, "/diskInfo/usedPercentage");
    let uptime = num(&stats, "/uptime");

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
    render_gauge(f, left[0], "CPU", cpu);
    render_gauge(f, left[1], "Memory", mem);
    render_gauge(f, left[2], "Disk", disk);
    f.render_widget(
        Paragraph::new(format!(
            " Uptime: {}    Cores: {}",
            fmt_uptime(uptime),
            field(&stats, "/cpuInfo/count")
        )),
        left[3],
    );

    let data: Vec<u64> = app.cpu_history.iter().copied().collect();
    let spark = Sparkline::default()
        .block(Block::bordered().title(" CPU History (%) "))
        .data(data)
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
        Screen::Dashboard => "1/2/3 tab · s server · r refresh · q keluar",
        Screen::Projects => {
            "↑↓ pilih · ←→ panel · Enter logs · e/p/m/o/b view · d/R/S/T aksi · s server · q keluar"
        }
        Screen::Viewer => "↑↓ scroll · PgUp/PgDn · r refresh · 1/2/3 tab · q keluar",
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

fn num(v: &Value, ptr: &str) -> f64 {
    match v.pointer(ptr) {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0),
        Some(Value::String(s)) => s.parse().unwrap_or(0.0),
        _ => 0.0,
    }
}

fn fmt_uptime(secs: f64) -> String {
    let s = secs as u64;
    format!(
        "{}d {}h {}m",
        s / 86400,
        (s % 86400) / 3600,
        (s % 3600) / 60
    )
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
