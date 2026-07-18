use std::collections::HashMap;
use std::sync::mpsc::Sender;
use std::time::Instant;

use ratatui::crossterm::event::KeyCode;
use ratatui::layout::Rect;
use ratatui::widgets::{ListState, TableState};
use serde_json::{json, Value};

use crate::commands;
use crate::output::field;

use super::form::*;
use super::render::cap;
use super::table::*;
use super::worker::{Refresh, Req, Resp, View};
use super::LOG_BUFFER;

// ---------- State ----------

#[derive(PartialEq, Clone, Copy)]
pub(super) enum Screen {
    Dashboard,
    /// Semua host sekaligus — satu-satunya layar yang tak bisa digantikan panel web.
    Hosts,
    /// Info & pembersihan Docker pada server aktif.
    Maintenance,
    Actions,
    Monitor,
    Domains,
    Projects,
    Viewer,
    /// Terminal container tertanam; dibuka dari sebuah service.
    Terminal,
}

/// Viewer sengaja TIDAK ada di sini: ia hasil dari membuka sesuatu pada sebuah
/// service, bukan tujuan tersendiri. Sebagai tab ia hanya kotak kosong sampai
/// user datang dari Projects.
pub(super) const TABS: [&str; 7] = [
    "Dashboard",
    "Hosts",
    "Maintenance",
    "Actions",
    "Monitor",
    "Domains",
    // Layar ini mendaftar SERVICE lintas project, bukan project. Namanya masih
    // Screen::Projects di kode (sisa panel lama), tapi labelnya harus jujur.
    "Services",
];

/// Tab (urut label) → Screen, kebalikan dari Screen::index. Untuk klik tab.
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
            // Viewer & Terminal selalu dibuka dari Projects, jadi tab itu yang
            // tetap tersorot — keduanya tak punya tab sendiri.
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
}

/// Satu baris di layar Hosts. Host yang mati harus tampil sebagai baris error,
/// bukan menggagalkan seluruh tabel.
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

/// Sub-tab pada layar Monitor (mengikuti panel).
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

/// Perubahan daftar server: dieksekusi di event_loop yang memegang ServerConfig.
pub(super) enum ServerAction {
    Save {
        name: String,
        url: String,
        /// None = pertahankan token yang tersimpan (form edit yang dibiarkan kosong).
        token: Option<String>,
    },
    Remove(String),
}

pub(super) struct App {
    pub(super) server_name: String,
    /// (nama, url) tiap server. URL ikut disimpan supaya form edit bisa
    /// terisi nilai sekarang, bukan kosong seperti form tambah.
    pub(super) all_servers: Vec<(String, String)>,
    pub(super) switch_to: Option<String>,
    pub(super) picker: Option<ListState>,
    pub(super) form: Option<Form>,
    pub(super) chooser: Option<Chooser>,
    pub(super) server_action: Option<ServerAction>,
    /// (project, service, stype, replace) — menunggu suntingan env di $EDITOR.
    /// `replace` = true membuka editor KOSONG (ganti-cepat: tempel env baru tanpa
    /// menunggu fetch atau menghapus yang lama); false memuat env sekarang.
    pub(super) edit_env: Option<(String, String, String, bool)>,
    /// Indeks field form yang menunggu dibuka di $EDITOR; event_loop yang
    /// mengerjakannya — hanya ia yang memegang terminal.
    pub(super) edit_field: Option<usize>,
    /// (project, service) yang menunggu dibukakan terminal container; event_loop
    /// yang menyambungkannya (ia yang memegang ServerConfig).
    /// (project, service, db) — permintaan membuka terminal container. `db` =
    /// Some(stype) untuk shell database (mysql/mariadb, login root otomatis),
    /// None untuk shell biasa (sh). event_loop yang menyambungkannya.
    pub(super) terminal_req: Option<(String, String, Option<String>)>,
    /// Emulator layar terminal aktif (parser vt100 diisi output WebSocket).
    pub(super) term_parser: Option<vt100::Parser>,
    /// Kirim keystroke/resize ke thread WebSocket. Drop = tutup sesi.
    pub(super) term_input: Option<Sender<super::terminal::TermMsg>>,
    /// Judul pane terminal (project/service).
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
    /// Replika swarm per service (actual/desired), dikunci "{project}_{service}".
    /// Sumber status "turun" di tabel Services. Kosong = belum dimuat.
    pub(super) task_stats: HashMap<String, (i64, i64)>,
    pub(super) storage: Vec<Value>,
    pub(super) monitor_view: MonitorView,
    pub(super) domains: Vec<Value>,
    pub(super) domains_state: TableState,
    /// (project, service) asal saat masuk tab Domains lewat `o` dari sebuah
    /// service — dipakai memprefill form "Domain baru" ke service itu. None = tab
    /// Domains dibuka biasa.
    pub(super) domain_scope: Option<(String, String)>,

    pub(super) projects: Vec<String>,
    /// Semua service lintas project. Daftar datar menggantikan hirarki
    /// project -> service: drill-down tak bisa dicari dan runtuh di ratusan service.
    pub(super) all_services: Vec<Value>,
    pub(super) services_table: TableState,

    /// Layar tujuan saat Esc dari Viewer — viewer bisa dibuka dari Services
    /// (kembali ke Services) atau dari Actions (kembali ke Actions).
    pub(super) viewer_from: Screen,
    pub(super) viewer_title: String,
    pub(super) viewer_lines: Vec<String>,
    pub(super) viewer_scroll: u16,
    pub(super) viewer_ctx: Option<(View, String, String, String)>,
    /// Timestamp log terbaru yang sudah tampil; penanda lanjut buat tail.
    /// Some = tail aktif (hanya untuk View::Logs).
    pub(super) log_cursor: Option<String>,
    /// Viewer menempel di baris terakhir. Log tumbuh dari bawah, jadi tanpa ini
    /// baris baru datang di luar layar dan tail-nya tampak mati.
    pub(super) viewer_follow: bool,

    /// Teks filter untuk tabel layar aktif ("" = tanpa filter).
    pub(super) filter: String,
    /// Sedang mengetik filter (tombol masuk ke filter, bukan ke layar).
    pub(super) filter_input: bool,
    /// Overlay bantuan sedang terbuka.
    pub(super) help: bool,
    /// Baris info tab Maintenance: (label, nilai).
    pub(super) maint: Vec<(String, String)>,
    pub(super) hosts: Vec<HostRow>,
    pub(super) hosts_state: TableState,
    /// Diset saat layar Hosts perlu data; fan-out-nya dijalankan event_loop.
    pub(super) load_hosts: bool,

    pub(super) confirm: Option<Confirm>,

    // ---- Animasi & mouse ----
    /// Jam animasi global; fase spinner/denyut dihitung dari elapsed-nya.
    pub(super) anim: Instant,
    /// Kapan seleksi tabel Services terakhir berpindah (kilat sorot).
    pub(super) nav_at: Instant,
    /// Kapan tab terakhir berganti (kilat tab).
    pub(super) tab_at: Instant,
    /// Pembanding untuk mendeteksi perubahan tab/seleksi tanpa mengait tiap handler.
    pub(super) last_screen: Screen,
    pub(super) last_sel: Option<usize>,
    /// Hitbox klik per tab (start,end kolom), diisi saat render_tabs. Baris tab-nya.
    pub(super) tab_spans: Vec<(u16, u16)>,
    pub(super) tab_row: u16,
    /// Area tabel layar aktif, diisi saat render — memetakan klik ke baris. Hanya
    /// satu layar dirender per frame, jadi satu field cukup untuk semua tabel.
    pub(super) table_area: Rect,
    /// Menu konteks (klik kanan). Item = (label, tombol yang disimulasikan).
    pub(super) menu: Option<Menu>,
}

/// Menu konteks klik-kanan: tiap item mengeksekusi aksi yang SAMA seperti sebuah
/// tombol, jadi tak ada jalur aksi kedua yang bisa menyimpang dari keyboard.
pub(super) struct Menu {
    pub(super) items: Vec<(String, KeyCode)>,
    pub(super) state: ListState,
    /// Posisi kursor saat menu dibuka (sudut kiri-atas menu, sebelum dijepit layar).
    pub(super) col: u16,
    pub(super) row: u16,
    /// Kotak menu yang benar-benar digambar (setelah dijepit ke layar), diisi saat
    /// render — dipakai memetakan klik/hover item.
    pub(super) rect: Rect,
}

impl Menu {
    /// Indeks item di bawah (col,row), atau None bila di luar area item. Item i
    /// digambar di baris `rect.y + 1 + i` (baris pertama & terakhir = border).
    pub(super) fn item_at(&self, col: u16, row: u16) -> Option<usize> {
        let r = self.rect;
        let inside = col >= r.x
            && col < r.x.saturating_add(r.width)
            && row > r.y
            && row < r.y.saturating_add(r.height).saturating_sub(1);
        if !inside {
            return None;
        }
        let i = (row - r.y - 1) as usize;
        (i < self.items.len()).then_some(i)
    }
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
            edit_env: None,
            edit_field: None,
            terminal_req: None,
            term_parser: None,
            term_input: None,
            term_title: String::new(),
            screen: Screen::Dashboard,
            should_quit: false,
            refresh_inflight: false,
            status: "Siap".into(),
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
            viewer_ctx: None,
            log_cursor: None,
            viewer_follow: false,
            filter: String::new(),
            filter_input: false,
            help: false,
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
        }
    }

    /// Jumlah baris yang SEDANG dirender di tabel layar aktif (setelah filter).
    /// Dipakai klik: indeks yang diklik harus dalam rentang yang benar-benar tampil.
    pub(super) fn visible_table_len(&self) -> usize {
        match self.screen {
            Screen::Projects => self.visible_rows().len(),
            Screen::Actions => self.visible_actions().len(),
            Screen::Domains => self.visible_domains().len(),
            Screen::Hosts => self.hosts.len(),
            Screen::Monitor => match self.monitor_view {
                MonitorView::Services => self.visible_monitor_rows().len(),
                MonitorView::Storage => self.storage.len(),
            },
            _ => 0,
        }
    }

    /// TableState tabel layar aktif (untuk memilih baris dari klik). None = layar
    /// tanpa tabel yang bisa dipilih.
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

    /// Item menu konteks untuk baris yang disorot di layar aktif. Kosong = tak ada
    /// menu (mis. tak ada baris terpilih, atau layar tanpa aksi baris).
    pub(super) fn context_items(&self) -> Vec<(String, KeyCode)> {
        match self.screen {
            Screen::Projects if self.selected_row().is_some() => {
                let mut items = vec![
                    ("Logs".into(), KeyCode::Enter),
                    ("Terminal".into(), KeyCode::Char('t')),
                ];
                // Shell DB (login otomatis) hanya relevan untuk service database.
                if matches!(
                    self.selected_row().map(|(_, _, t)| t).as_deref(),
                    Some("mysql" | "mariadb" | "postgres" | "mongo" | "redis")
                ) {
                    items.push(("DB shell (login otomatis)".into(), KeyCode::Char('y')));
                }
                items.extend([
                    ("Deploy".into(), KeyCode::Char('d')),
                    ("Restart".into(), KeyCode::Char('R')),
                    ("Stop".into(), KeyCode::Char('S')),
                    ("Start".into(), KeyCode::Char('T')),
                    ("Lihat env".into(), KeyCode::Char('e')),
                    ("Edit env (sebagian)".into(), KeyCode::Char('E')),
                    ("Ganti seluruh env".into(), KeyCode::Char('w')),
                    ("Toggle file .env".into(), KeyCode::Char('.')),
                    ("Domain".into(), KeyCode::Char('o')),
                    ("Mount baru".into(), KeyCode::Char('M')),
                    ("Redirect baru".into(), KeyCode::Char('F')),
                    ("Basic auth".into(), KeyCode::Char('H')),
                    ("Resource".into(), KeyCode::Char('L')),
                    ("Clone".into(), KeyCode::Char('c')),
                    ("Hapus".into(), KeyCode::Char('x')),
                ]);
                items
            }
            Screen::Domains if self.domains_state.selected().is_some() => vec![
                ("Edit".into(), KeyCode::Char('e')),
                ("Jadikan primary".into(), KeyCode::Char('P')),
                ("Hapus".into(), KeyCode::Char('x')),
            ],
            Screen::Actions if self.selected_action_id().is_some() => {
                vec![("View detail".into(), KeyCode::Enter)]
            }
            _ => vec![],
        }
    }

    /// Id action yang disorot (dari daftar yang tampil, hormati filter). None =
    /// tak ada yang dipilih.
    pub(super) fn selected_action_id(&self) -> Option<String> {
        self.actions_state
            .selected()
            .and_then(|i| self.visible_actions().get(i).map(|a| field(a, "/id")))
    }

    /// Deteksi ganti tab/seleksi (dipanggil tiap frame sebelum draw) untuk memicu
    /// kilat transisi — supaya tak perlu menyisipkan timestamp di tiap handler nav.
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

    /// Frame spinner saat ada operasi berjalan (status diakhiri "..."), else None.
    pub(super) fn spinner(&self) -> Option<char> {
        const F: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
        let loading = self.status.ends_with("...") || self.status.ends_with('…');
        loading.then(|| F[((self.anim.elapsed().as_millis() / 90) % 10) as usize])
    }

    /// Ada animasi aktif? Dipakai event loop untuk merapatkan redraw (lebih mulus)
    /// hanya saat perlu, biar idle tetap murah.
    pub(super) fn animating(&self) -> bool {
        self.spinner().is_some()
            || self.down_count() > 0
            || self.nav_at.elapsed().as_millis() < 260
            || self.tab_at.elapsed().as_millis() < 320
    }

    pub(super) fn reset_for_server(&mut self, name: String) {
        self.server_name = name;
        self.status = "Ganti server".into();
        // Pertahankan layar aktif — ganti server tak boleh melempar user ke
        // Dashboard. Layar turunan (Viewer/Terminal) isinya milik server lama,
        // jadi jatuh ke Services.
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
                let n = self.visible_services().len();
                select_first(&mut self.services_table, n);
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
                self.form = Some(Form::new(
                    FormKind::ResourceEdit {
                        project,
                        service,
                        stype,
                    },
                    title,
                    resource_fields(data.get("resources")),
                ));
                self.status = "Enter simpan · Esc batal · 0 = tak dibatasi".into();
            }
            Resp::BasicAuthForm {
                project,
                service,
                stype,
                data,
            } => {
                let title = format!("Basic auth · {project}/{service}");
                self.form = Some(Form::new(
                    FormKind::BasicAuthEdit {
                        project,
                        service,
                        stype,
                    },
                    title,
                    basic_auth_fields(Some(&data)),
                ));
                self.status = "Enter simpan · Esc batal · kosongkan keduanya = matikan".into();
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
                self.status = "Enter simpan · Esc batal".into();
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
                // Batch pertama tiba ke viewer_lines yang kosong, jadi menambah
                // = mengganti; ronde berikutnya menyambung. Tak perlu tahu yang
                // mana: `since` yang menentukan apa yang dikirim server.
                if !lines.is_empty() {
                    self.viewer_lines.extend(lines);
                    // Tail berjam-jam tak boleh menumpuk tanpa batas.
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
                    // Pilihan kosong wajib ada saat belum ada yang dipilih:
                    // set_options() melompat ke opsi pertama kalau nilai sekarang
                    // tak ada di daftar, jadi tanpa ini form baru diam-diam
                    // menunjuk source ke repo acak.
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
                    // Tanpa daftar branch, dropdown hanya berisi nilai sekarang —
                    // user terkunci di branch itu dan tak bisa menggantinya sama
                    // sekali. Jatuh ke input teks: server tetap menolak branch yang
                    // tak ada ("Branch not found"), jadi tak ada yang hilang selain
                    // kenyamanan memilih.
                    Err(e) => {
                        f.kind = FieldKind::Text;
                        self.status = format!(
                            "Daftar branch tak bisa dimuat ({}) — ketik nama branch manual. \
                             Perbaiki token GitHub di EasyPanel > Settings.",
                            short_reason(&e)
                        );
                    }
                }
            }
            Resp::Viewer(title, lines) => {
                self.viewer_title = title;
                self.viewer_lines = lines;
                self.viewer_scroll = 0;
                self.screen = Screen::Viewer;
                self.status = "Siap".into();
            }
            Resp::TermOutput(bytes) => {
                if let Some(p) = self.term_parser.as_mut() {
                    p.process(&bytes);
                }
            }
            Resp::TermClosed => {
                // Shell keluar / socket tutup: kembali ke Services.
                self.term_parser = None;
                self.term_input = None;
                if self.screen == Screen::Terminal {
                    self.screen = Screen::Projects;
                    self.status = format!("Terminal {} ditutup", self.term_title);
                }
            }
            Resp::Done(msg, what) => {
                self.status = msg;
                match what {
                    Refresh::Projects => {
                        let _ = req.send(Req::AllServices);
                    }
                    Refresh::Domains => {
                        let _ = req.send(Req::Domains);
                    }
                    Refresh::None => {}
                }
            }
            Resp::Err(e) => self.status = format!("Error: {e}"),
        }
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

    /// Filter mengecilkan daftar, jadi baris terpilih bisa jatuh di luar batas.
    pub(super) fn clamp_filtered(&mut self) {
        let len = match self.screen {
            Screen::Domains => self.visible_domains().len(),
            Screen::Actions => self.visible_actions().len(),
            Screen::Monitor => self.visible_monitor_rows().len(),
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

    /// monitor_rows() mengelompokkan seluruh daftar sekaligus, jadi filternya
    /// diterapkan ke baris hasil, bukan ke item mentah.
    pub(super) fn visible_monitor_rows(&self) -> Vec<Vec<String>> {
        commands::monitor_rows(self.monitor.clone())
            .into_iter()
            .filter(|r| keep(r, &self.filter))
            .collect()
    }

    /// Pindah layar dan muat datanya bila belum ada.
    pub(super) fn goto(&mut self, screen: Screen, req: &Sender<Req>) {
        // Filter milik layar tempat ia diketik. Membawanya ke layar lain berarti
        // menyembunyikan baris tanpa sebab yang terlihat.
        self.filter.clear();
        self.filter_input = false;
        // Scope domain hanya berlaku untuk kunjungan `o` dari sebuah service;
        // navigasi biasa mengosongkannya (open_service_domains menyetel ulang
        // sesudah goto).
        self.domain_scope = None;
        self.screen = screen;
        match screen {
            Screen::Projects => {
                if self.all_services.is_empty() {
                    let _ = req.send(Req::AllServices);
                }
                // Metrik per service dijoin ke tabel; tanpa ini kolomnya "-".
                if self.monitor.is_empty() {
                    let _ = req.send(Req::MonitorData);
                }
                // Replika swarm → kolom Status ("turun" untuk yang crash/down).
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

    /// (nama, url) server yang sedang disorot di picker.
    pub(super) fn picker_selected(&self) -> Option<(String, String)> {
        self.picker
            .as_ref()
            .and_then(|s| s.selected())
            .and_then(|i| self.all_servers.get(i).cloned())
    }

    pub(super) fn start_env_edit(&mut self) {
        if let Some((p, s, t)) = self.selected_row() {
            self.edit_env = Some((p, s, t, false));
        }
    }

    /// Ganti-cepat env: buka $EDITOR KOSONG untuk menempel env baru wholesale.
    pub(super) fn start_env_replace(&mut self) {
        if let Some((p, s, t)) = self.selected_row() {
            self.edit_env = Some((p, s, t, true));
        }
    }

    /// Baris yang tampil: header project diikuti service-nya, terfilter.
    ///
    /// Render DAN aksi wajib lewat sini. Kalau render difilter sementara aksi
    /// memakai indeks daftar penuh, `x` akan menghapus service yang salah.
    pub(super) fn visible_rows(&self) -> Vec<Line2<'_>> {
        let f = self.filter.to_lowercase();
        let mut names: Vec<&String> = self.projects.iter().collect();
        names.sort();

        let mut out = Vec::new();
        for p in names {
            // Nama project yang cocok menahan seluruh isinya: mencari
            // "harisenin-net" harus memperlihatkan service-nya, bukan header kosong.
            let project_matches = f.is_empty() || p.to_lowercase().contains(&f);
            let mut kept: Vec<&Value> = self
                .all_services
                .iter()
                .filter(|s| s.get("projectName").and_then(Value::as_str) == Some(p.as_str()))
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

    /// Service yang lolos filter.
    ///
    /// Render DAN aksi wajib lewat sini: kalau render difilter sementara aksi
    /// memakai indeks daftar penuh, `x` akan menghapus service yang salah.
    pub(super) fn visible_services(&self) -> Vec<&Value> {
        self.all_services
            .iter()
            .filter(|s| keep(&service_row(s, None, None), &self.filter))
            .collect()
    }

    /// Metrik untuk sebuah service, dijoin lewat (projectName, serviceName).
    ///
    /// getAllServicesStats memuat lebih banyak entri daripada daftar service
    /// (service sistem, sub-service compose), jadi yang tak cocok diabaikan.
    /// (actual, desired) replika swarm sebuah service, dari getDockerTaskStats.
    /// None = belum dimuat atau service tak punya swarm task.
    pub(super) fn replicas(&self, project: &str, service: &str) -> Option<(i64, i64)> {
        self.task_stats
            .get(&format!("{project}_{service}"))
            .copied()
    }

    /// Jumlah service yang sedang turun (desired>0 tapi actual<desired).
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

    pub(super) fn metric_for(&self, project: &str, service: &str) -> Option<&Value> {
        self.monitor.iter().find(|m| {
            m.get("projectName").and_then(Value::as_str) == Some(project)
                && m.get("serviceName").and_then(Value::as_str) == Some(service)
        })
    }

    /// (project, service, tipe) — hanya bila baris yang disorot adalah SERVICE.
    /// Header project mengembalikan None, jadi aksi service (logs/deploy/hapus)
    /// tak pernah dijalankan pada service yang tak ada.
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

    /// Service terpilih, utuh — selected_row() hanya memberi identitasnya, dan
    /// beberapa aksi perlu isinya (mis. autoDeploy sekarang).
    pub(super) fn selected_service(&self) -> Option<&Value> {
        match self.visible_rows().get(self.services_table.selected()?)? {
            Line2::Service(s) => Some(*s),
            Line2::Project { .. } => None,
        }
    }

    /// Balik auto deploy service terpilih.
    /// Tutup sesi terminal (Ctrl-Q). Drop channel input → thread WS menutup
    /// socket; kembali ke Services segera.
    pub(super) fn close_terminal(&mut self) {
        self.term_input = None;
        self.term_parser = None;
        self.screen = Screen::Projects;
        self.status = format!("Terminal {} ditutup", self.term_title);
    }

    pub(super) fn toggle_auto_deploy(&mut self, req: &Sender<Req>) {
        let picked = self.selected_service().map(|s| {
            (
                field(s, "/projectName"),
                field(s, "/name"),
                // None = tak punya auto deploy sama sekali (database, source
                // image), bukan "mati". Menawarkan toggle di sana cuma akan
                // memancing error dari server.
                match field(s, "/source/type").as_str() {
                    "github" => s.pointer("/source/autoDeploy").and_then(Value::as_bool),
                    _ => None,
                },
            )
        });
        match picked {
            None => self.status = "Pilih sebuah service dulu".into(),
            Some((_, _, None)) => {
                self.status = "Auto deploy hanya ada pada service dengan source GitHub".into()
            }
            Some((project, service, Some(on))) => {
                self.status = format!(
                    "{} auto deploy untuk {service}...",
                    if on { "Mematikan" } else { "Menyalakan" }
                );
                let _ = req.send(Req::AutoDeploy {
                    project,
                    service,
                    on: !on,
                });
            }
        }
    }

    /// Nama project dari baris yang disorot, header maupun service. Dipakai aksi
    /// yang bekerja pada PROJECT: buat service, hapus project.
    pub(super) fn selected_project(&self) -> Option<String> {
        match self.visible_rows().get(self.services_table.selected()?)? {
            Line2::Project { name, .. } => Some((*name).to_string()),
            Line2::Service(s) => Some(field(s, "/projectName")),
        }
    }

    /// Domain yang lolos filter.
    ///
    /// Render DAN aksi (e/x/P) wajib lewat sini. Kalau render difilter sementara
    /// aksi memakai indeks daftar penuh, `x` akan menghapus domain yang salah.
    pub(super) fn visible_domains(&self) -> Vec<&Value> {
        self.domains
            .iter()
            .filter(|d| keep(&commands::domain_row(d), &self.filter))
            .collect()
    }

    /// Muat daftar service untuk project yang sedang dipilih di form, supaya
    /// field Service jadi pilihan nyata dan bukan ketikan bebas.
    pub(super) fn load_form_services(&mut self, req: &Sender<Req>) {
        if let Some(form) = self.form.as_ref() {
            let project = form.by_label("Project");
            if !project.is_empty() {
                let _ = req.send(Req::ServicesFor(project));
            }
        }
    }

    /// Minta data untuk form source/build service yang sedang dipilih.
    ///
    /// Formnya baru terbuka setelah inspectService tiba (lihat Resp::ConfigForm),
    /// karena nilai sekarang harus jadi isi awalnya.
    pub(super) fn open_config_form(&mut self, build: bool, req: &Sender<Req>) {
        let Some((project, service, stype)) = self.selected_row() else {
            return;
        };
        // Source/build hanya ada di service tipe app; tipe lain tak punya konsep ini.
        if stype != "app" {
            self.status = format!("Source & build hanya untuk service app (ini {stype})");
            return;
        }
        let _ = req.send(Req::ConfigForm {
            project,
            service,
            build,
        });
        self.status = "Memuat...".into();
    }

    /// Buka form tambah mount untuk service yang disorot.
    pub(super) fn open_mount_form(&mut self) {
        let Some((project, service, _)) = self.selected_row() else {
            self.status = "Pilih sebuah service dulu".into();
            return;
        };
        self.form = Some(Form::new(
            FormKind::MountCreate { project, service },
            " Mount baru ",
            mount_fields(),
        ));
        self.status = "Enter tambah · Esc batal · hapus mount: 'm' lalu angka".into();
    }

    /// Kelola domain sebuah service: buka tab Domains ter-filter ke service itu.
    /// Reuse CRUD domain penuh (n baru · e edit · x hapus · P primary) alih-alih
    /// viewer read-only. Filter cocok ke destination "protocol://{project}_{service}:…".
    pub(super) fn open_service_domains(&mut self, req: &Sender<Req>) {
        let Some((project, service, _)) = self.selected_row() else {
            self.status = "Pilih sebuah service dulu".into();
            return;
        };
        // goto mengosongkan filter & scope lebih dulu, jadi set keduanya SESUDAHnya.
        self.goto(Screen::Domains, req);
        self.filter = format!("{project}_{service}");
        self.domain_scope = Some((project.clone(), service.clone()));
        self.status = format!("Domain {project}/{service} · n baru · e edit · x hapus · P primary");
    }

    /// Buka form clone untuk service yang disorot. Nama baru diusulkan "{svc}-copy".
    pub(super) fn open_clone_form(&mut self) {
        let Some((project, service, stype)) = self.selected_row() else {
            self.status = "Pilih sebuah service dulu".into();
            return;
        };
        let suggested = format!("{service}-copy");
        // Project tujuan: dropdown project yang SUDAH ada (default: project sumber).
        // Hanya yang sudah ada — project baru network-nya belum siap saat createService.
        let mut projects = self.projects.clone();
        projects.sort();
        let fields = vec![
            Field::choice_owned("Project", projects, &project),
            Field::text("Nama baru", &suggested),
        ];
        self.form = Some(Form::new(
            FormKind::CloneService {
                project,
                service,
                stype,
            },
            " Clone service ",
            fields,
        ));
        self.status = "Config disalin (bukan data) · Enter clone · Esc batal".into();
    }

    /// Buka form tambah redirect untuk service web yang disorot.
    pub(super) fn open_redirect_form(&mut self) {
        let Some((project, service, stype)) = self.selected_row() else {
            self.status = "Pilih sebuah service dulu".into();
            return;
        };
        if !matches!(stype.as_str(), "app" | "box" | "compose" | "wordpress") {
            self.status = format!("Redirect hanya untuk service web (ini {stype})");
            return;
        }
        self.form = Some(Form::new(
            FormKind::RedirectCreate {
                project,
                service,
                stype,
            },
            " Redirect baru ",
            redirect_fields(),
        ));
        self.status = "Enter tambah · Esc batal · hapus: 'f' lalu angka".into();
    }

    /// Buka form basic auth untuk service yang disorot. Hanya service web
    /// (app/box/compose/wordpress) yang punya endpoint ini; DB tak relevan.
    pub(super) fn open_basic_auth_form(&mut self, req: &Sender<Req>) {
        let Some((project, service, stype)) = self.selected_row() else {
            self.status = "Pilih sebuah service dulu".into();
            return;
        };
        if !matches!(stype.as_str(), "app" | "box" | "compose" | "wordpress") {
            self.status = format!("Basic auth hanya untuk service web (ini {stype})");
            return;
        }
        let _ = req.send(Req::BasicAuthForm {
            project,
            service,
            stype,
        });
        self.status = "Memuat...".into();
    }

    /// Buka form limit resource untuk service yang disorot (semua tipe punya).
    pub(super) fn open_resource_form(&mut self, req: &Sender<Req>) {
        let Some((project, service, stype)) = self.selected_row() else {
            self.status = "Pilih sebuah service dulu".into();
            return;
        };
        let _ = req.send(Req::ResourceForm {
            project,
            service,
            stype,
        });
        self.status = "Memuat...".into();
    }

    /// Muat branch repo yang sedang dipilih ke dropdown "Branch".
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

    /// Buka dropdown untuk field Choice yang sedang difokus.
    pub(super) fn open_chooser(&mut self) {
        let Some(form) = self.form.as_ref() else {
            return;
        };
        let f = &form.fields[form.focus];
        if let FieldKind::Choice(opts) = &f.kind {
            if opts.is_empty() {
                self.status = format!("{} belum ada pilihannya", f.label);
                return;
            }
            self.chooser = Some(Chooser::new(form.focus, f.label, opts.clone(), &f.value));
        }
    }

    pub(super) fn submit_form(&mut self, req: &Sender<Req>) {
        let Some(form) = self.form.as_ref() else {
            return;
        };

        // Validasi minimal di sini; sisanya biar server yang menolak.
        match &form.kind {
            FormKind::ServerAdd | FormKind::ServerEdit { .. } => {
                // Tambah: token wajib. Edit: token kosong = pertahankan yang lama,
                // supaya mengganti URL saja tak memaksa mengetik ulang token.
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
                    self.status = "Nama dan URL wajib diisi".into();
                    return;
                }
                if token.as_deref() == Some("") {
                    self.status = "Token wajib diisi".into();
                    return;
                }
                if !commands::valid_name(&name) {
                    self.status = "Nama server hanya boleh a-z, 0-9, -, _".into();
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
                    self.status = "Nama project hanya boleh a-z, 0-9, -, _".into();
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
                // Source diterapkan terpisah (lihat create_source): inline-nya
                // memicu deploy. build/env/domains aman inline — cepat, tak deploy.
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
                    // "Buat file .env" -> tulis env sebagai file di path ini.
                    if form.is_on_label("Buat file .env") {
                        let path = form.by_label("Path file .env");
                        extra["dotEnvPath"] =
                            json!(if path.is_empty() { ".env".into() } else { path });
                    }
                }
                if let Some(domains) = create_domains(form) {
                    extra["domains"] = domains;
                }
                self.status = format!("Membuat '{service}'...");
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
                let new_name = form.by_label("Nama baru");
                let new_name = new_name.trim();
                let target = form.by_label("Project");
                let target = if target.is_empty() {
                    project.clone()
                } else {
                    target
                };
                if new_name.is_empty() {
                    self.status = "Isi nama service baru dulu".into();
                    return;
                }
                // Nama boleh sama asal beda project; sama-persis (project+nama) = tabrakan.
                if target == *project && new_name == service {
                    self.status = "Beda project, atau beda nama — tak boleh sama persis".into();
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
            FormKind::LogSearch => {
                let query = form.by_label("Kata kunci");
                if query.is_empty() {
                    self.status = "Isi kata kunci dulu".into();
                    return;
                }
                // Buka Viewer kosong; hasil menyusul saat fan-out selesai.
                self.viewer_lines = vec!["Mencari di semua service...".into()];
                self.viewer_scroll = 0;
                self.viewer_follow = false;
                self.log_cursor = None;
                self.viewer_title = format!("Cari '{query}'");
                self.viewer_ctx = None;
                self.viewer_from = Screen::Projects;
                self.screen = Screen::Viewer;
                self.status = format!("Mencari '{query}' di semua service...");
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
        self.status = "Mengirim...".into();
    }

    /// Buka daftar server (pilih / tambah / edit / hapus).
    ///
    /// Tidak boleh menolak saat cuma ada satu server: picker ini satu-satunya
    /// jalan menambah server dari TUI, jadi menolaknya membuat server kedua
    /// mustahil dibuat tanpa keluar ke CLI.
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

    /// Form service baru. Project dipilih dari dropdown: daftar datar tak punya
    /// "project yang sedang dibuka", jadi ia harus disebut eksplisit.
    ///
    /// Source ikut di sini, tidak menyusul lewat form edit: createService
    /// menerima `source` inline dan hanya mewajibkan projectName + serviceName,
    /// jadi create-lalu-edit selama ini batasan form ini — bukan batasan API.
    pub(super) fn new_service_form(&mut self, req: &Sender<Req>) {
        if self.projects.is_empty() {
            self.status = "Daftar project belum termuat".into();
            return;
        }
        let project = self
            .selected_project()
            .unwrap_or_else(|| self.projects[0].clone());
        // Field database mengikuti Tipe, seperti dialog panel. Semuanya opsional:
        // kosong berarti server yang membuatkan (password acak, nama database =
        // nama project, image resmi terbaru) — sama persis dengan panel.
        let mut fields = vec![
            Field::choice_owned("Project", self.projects.clone(), &project),
            Field::text("Nama", ""),
            Field::choice("Tipe", SERVICE_TYPES, "app"),
            Field::text("Database", "").when("Tipe", "mysql,mariadb,postgres"),
            Field::text("User", "").when("Tipe", "mysql,mariadb,postgres,mongo"),
            Field::secret("Password").when("Tipe", "mysql,mariadb,postgres,mongo,redis"),
            Field::secret("Root password").when("Tipe", "mysql,mariadb"),
            Field::text("Image", "").when("Tipe", "mysql,mariadb,postgres,mongo,redis"),
        ];
        // Field source membawa syaratnya sendiri (Source=github/git/image);
        // .when() menambah syarat, tidak menimpanya, jadi keduanya berlaku:
        // tampil hanya bila tipe service = app DAN tipe source cocok.
        //
        // Daftar repo menyusul lewat Resp::Repos: menunggunya di sini akan
        // membekukan TUI sampai searchRepos selesai.
        // Wizard mengikuti alur dashboard EasyPanel: Dasar → Source → Build.
        // Field source & build hanya untuk app (`.when("Tipe","app")`), jadi
        // service database tetap satu langkah. `.step()` menaruhnya di halaman
        // masing-masing; nilai submit tetap dibaca lintas-langkah.
        fields.extend(
            source_fields(None, Vec::new())
                .into_iter()
                .map(|f| f.when("Tipe", "app").step(1)),
        );
        fields.extend(
            build_fields(None)
                .into_iter()
                .map(|f| f.when("Tipe", "app").step(2)),
        );
        // Lanjutan alur dashboard: Environment lalu Domains. Keduanya diterima
        // createService inline (`env` string, `domains` array; hanya `host`
        // wajib). Label domain diberi awalan "Domain " supaya "Path" tak
        // bertabrakan dengan "Path" milik source — by_label() memakai find().
        fields.push(Field::editor("Environment", "").when("Tipe", "app").step(3));
        // "Create env file" di dashboard: menulis env sebagai file .env di path
        // tsb (API: dotEnvPath). Path hanya tampil saat toggle-nya nyala.
        fields.push(
            Field::boolean("Buat file .env", false)
                .when("Tipe", "app")
                .step(3),
        );
        fields.push(
            Field::text("Path file .env", ".env")
                .when("Tipe", "app")
                .when("Buat file .env", "ya")
                .step(3),
        );
        fields.extend(
            [
                Field::text("Domain host", ""),
                Field::text("Domain port", "3000"),
                Field::boolean("Domain HTTPS", true),
                Field::text("Domain path", "/"),
            ]
            .map(|f| f.when("Tipe", "app").step(4)),
        );
        self.form = Some(Form::new(FormKind::ServiceCreate, " Service baru ", fields));
        let _ = req.send(Req::Repos);
    }

    pub(super) fn open_view(&mut self, view: View, req: &Sender<Req>) {
        if let Some((p, s, t)) = self.selected_row() {
            self.viewer_from = Screen::Projects;
            self.viewer_ctx = Some((view, p.clone(), s.clone(), t.clone()));
            self.status = format!("Memuat {}...", view.title());
            // Log itu aliran, bukan dokumen: mulai dari kosong, tempel ke baris
            // terakhir, dan biarkan lajur poll menyambungnya. Tampilan lain
            // adalah snapshot dan tetap mulai dari atas.
            if view == View::Logs {
                self.viewer_lines.clear();
                self.viewer_scroll = 0;
                self.log_cursor = None;
                self.viewer_follow = true;
                // Tampilan lain berpindah layar lewat Resp::Viewer; log tak lewat
                // sana, jadi perpindahannya harus di sini. Tanpa ini Enter tampak
                // tak melakukan apa pun.
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
            self.confirm = Some(Confirm {
                action: action.to_string(),
                project: p,
                service: s.clone(),
                stype: t,
                label: format!("{} service '{}'?", cap(action), s),
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
        self.status = "Refresh...".into();
    }
}
