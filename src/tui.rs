use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::prelude::*;
use ratatui::widgets::{
    Block, Clear, Gauge, List, ListItem, ListState, Paragraph, Row, Sparkline, Table, TableState,
    Tabs,
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

    let names: Vec<(String, String)> = cfg.all().into_iter().map(|s| (s.name, s.url)).collect();
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
            app.handle(resp, &w.user);
        }

        terminal.draw(|f| ui(f, app))?;

        // Metrik jalan di lajur poll. Guard in-flight menjaga agar ronde tak
        // menumpuk saat server lebih lambat dari interval refresh.
        if last_stats.elapsed() >= REFRESH && !app.refresh_inflight {
            let _ = w.poll.send(Req::Stats);
            // Metrik per service ikut live, tapi hanya di layar yang menampilkannya.
            if matches!(app.screen, Screen::Monitor | Screen::Projects) {
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

        // Layar Hosts: satu thread per server. Fan-out ada di sini karena hanya
        // event_loop yang memegang ServerConfig (url + token tiap host).
        if app.load_hosts {
            app.load_hosts = false;
            app.hosts = cfg
                .all()
                .into_iter()
                .map(|s| HostRow {
                    name: s.name,
                    url: s.url,
                    state: HostState::Loading,
                })
                .collect();
            for s in cfg.all() {
                let tx = w.resp_tx.clone();
                thread::spawn(move || {
                    let client = EasypanelClient::new(&s.url, &s.token);
                    let data = client
                        .call("metrics", "getSystemStats", json!({}))
                        .map_err(|e| e.to_string());
                    let _ = tx.send(Resp::HostStat { name: s.name, data });
                });
            }
        }

        // Perubahan daftar server perlu ServerConfig, yang hanya ada di sini.
        if let Some(action) = app.server_action.take() {
            app.status = match apply_server_action(cfg, action) {
                Ok(msg) => msg,
                Err(e) => format!("Error: {e}"),
            };
            app.all_servers = cfg.all().into_iter().map(|s| (s.name, s.url)).collect();
        }

        // Edit env: lepas terminal, buka $EDITOR, lalu ambil alih lagi.
        if let Some((project, service, stype)) = app.edit_env.take() {
            match edit_env_in_editor(&w.user, &w.resp, terminal, &project, &service, &stype) {
                Ok(Some(env)) => {
                    let _ = w.user.send(Req::EnvSave {
                        project,
                        service,
                        stype,
                        env,
                    });
                    app.status = "Menyimpan env...".into();
                }
                Ok(None) => app.status = "Env tidak berubah".into(),
                Err(e) => app.status = format!("Error: {e}"),
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

fn apply_server_action(cfg: &ServerConfig, action: ServerAction) -> Result<String> {
    match action {
        ServerAction::Save { name, url, token } => {
            // Token tak pernah ditampilkan kembali ke layar; membiarkannya kosong
            // saat edit berarti "pakai yang lama", bukan "kosongkan".
            let token = match token {
                Some(t) => t,
                None => cfg
                    .get(&name)
                    .map(|s| s.token)
                    .ok_or_else(|| anyhow::anyhow!("server '{name}' tak ditemukan"))?,
            };
            cfg.add(&name, &url, &token)?;
            Ok(format!("Server '{name}' disimpan"))
        }
        ServerAction::Remove(name) => {
            cfg.remove(&name)?;
            Ok(format!("Server '{name}' dihapus"))
        }
    }
}

/// Ambil env service, buka di `$EDITOR`, kembalikan isinya bila berubah.
///
/// Memakai editor milik user (pola `kubectl edit`) alih-alih menulis textarea
/// sendiri di ratatui: jauh lebih sedikit kode dan sudah familier. Terminal
/// dilepas selama editor jalan, lalu diambil alih kembali.
fn edit_env_in_editor(
    req: &Sender<Req>,
    resp: &Receiver<Resp>,
    terminal: &mut ratatui::DefaultTerminal,
    project: &str,
    service: &str,
    stype: &str,
) -> Result<Option<String>> {
    // Ambil env terkini lebih dulu (blocking; user memang sedang menunggu).
    req.send(Req::Fetch {
        view: View::Env,
        project: project.to_string(),
        service: service.to_string(),
        stype: stype.to_string(),
    })?;

    let deadline = Instant::now() + Duration::from_secs(30);
    let current = loop {
        match resp.recv_timeout(Duration::from_millis(200)) {
            Ok(Resp::Viewer(_, lines)) => break lines.join("\n"),
            Ok(Resp::Err(e)) => return Err(anyhow::anyhow!(e)),
            Ok(_) => {}
            Err(_) if Instant::now() > deadline => {
                return Err(anyhow::anyhow!("timeout mengambil env"))
            }
            Err(_) => {}
        }
    };

    let path = std::env::temp_dir().join(format!("easypanel-{project}-{service}.env"));
    std::fs::write(&path, &current)?;

    ratatui::restore();
    let opened = open_in_editor(&path);
    *terminal = ratatui::init();
    terminal.clear()?;
    opened?;

    let edited = std::fs::read_to_string(&path)?;
    let _ = std::fs::remove_file(&path);

    Ok((edited.trim_end() != current.trim_end()).then_some(edited))
}

/// Kandidat editor: pilihan user dulu, lalu cadangan yang pasti ada di Unix.
///
/// Tiap entri dipecah jadi program + argumen, supaya `EDITOR="code -w"` bekerja
/// dan tidak dicari sebagai satu biner bernama "code -w".
fn editor_candidates() -> Vec<Vec<String>> {
    let mut out: Vec<Vec<String>> = ["VISUAL", "EDITOR"]
        .iter()
        .filter_map(|k| std::env::var(k).ok())
        .map(|v| v.split_whitespace().map(String::from).collect::<Vec<_>>())
        .filter(|v| !v.is_empty())
        .collect();
    out.push(vec!["vi".into()]);
    out.push(vec!["nano".into()]);
    out
}

/// Buka file di editor pertama yang benar-benar ada.
///
/// $EDITOR yang menunjuk editor tak terpasang (mis. `nvim` yang belum dipasang)
/// dulu gagal dengan "No such file or directory (os error 2)" — pesan yang
/// terbaca seolah file env-nya yang hilang, bukan editornya. Sekarang kandidat
/// yang hilang dilewati, dan kalau semuanya hilang pesannya menyebut nama-namanya.
fn open_in_editor(path: &std::path::Path) -> Result<()> {
    let mut missing = Vec::new();
    for cand in editor_candidates() {
        let (prog, args) = cand.split_first().expect("kandidat tak pernah kosong");
        match std::process::Command::new(prog)
            .args(args)
            .arg(path)
            .status()
        {
            Ok(status) if status.success() => return Ok(()),
            Ok(status) => anyhow::bail!("editor '{prog}' keluar dengan {status}"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => missing.push(prog.clone()),
            Err(e) => return Err(e.into()),
        }
    }
    anyhow::bail!(
        "tak ada editor yang bisa dipakai (dicoba: {}). Set $EDITOR ke editor yang terpasang.",
        missing.join(", ")
    )
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
    Source,
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
            View::Source => "Source & build",
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
    /// Semua service lintas project dalam satu panggilan.
    AllServices,
    /// Muat service sebuah project untuk dropdown di form (bukan panel Projects).
    ServicesFor(String),
    /// Buka form source/build: butuh inspectService (nilai sekarang) dan —
    /// untuk source — daftar repo GitHub buat dropdown-nya.
    ConfigForm {
        project: String,
        service: String,
        build: bool,
    },
    /// Info server untuk tab Maintenance (versi Docker, IP, ketersediaan update).
    MaintInfo,
    /// Pembersihan Docker: systemPrune / cleanupDockerImages / cleanupDockerBuilder.
    MaintAction(&'static str),
    /// Branch sebuah repo untuk dropdown "Branch" (dipicu setelah repo dipilih).
    Branches {
        owner: String,
        repo: String,
    },
    /// `op` menentukan endpoint: updateSourceGithub/Git/Image, atau updateBuild.
    ///
    /// `auto_deploy` menyusul lewat enable/disableGithubDeploy: updateSourceGithub
    /// selalu mereset autoDeploy jadi false (terverifikasi di server), jadi nilainya
    /// harus dipasang ulang setelah update — kalau tidak, mengubah branch akan
    /// mematikan auto-deploy diam-diam.
    ConfigSave {
        project: String,
        service: String,
        op: &'static str,
        body: Value,
        auto_deploy: Option<bool>,
    },
    ProjectCreate(String),
    ProjectDestroy(String),
    ServiceCreate {
        project: String,
        service: String,
        stype: String,
    },
    DomainSave {
        id: Option<String>,
        body: Value,
    },
    DomainDelete(String),
    DomainSetPrimary(String),
    EnvSave {
        project: String,
        service: String,
        stype: String,
        env: String,
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
    /// Semua service lintas project + nama project untuk dropdown form.
    AllServices {
        projects: Vec<String>,
        services: Vec<Value>,
    },
    ServicesFor(String, Vec<String>),
    /// Data untuk membuka form source/build: hasil inspectService + daftar repo.
    ConfigForm {
        project: String,
        service: String,
        build: bool,
        data: Value,
        repos: Vec<String>,
    },
    Branches(Vec<String>),
    MaintInfo(Vec<(String, String)>),
    /// Hasil satu host di layar Hosts; tiap host tiba sendiri-sendiri supaya
    /// host lambat/mati tak menahan yang lain.
    HostStat {
        name: String,
        data: std::result::Result<Value, String>,
    },
    Viewer(String, Vec<String>),
    /// Mutasi berhasil: pesan status + data mana yang perlu dimuat ulang.
    Done(String, Refresh),
    Msg(String),
    Err(String),
}

/// Data yang perlu di-refresh setelah sebuah mutasi.
enum Refresh {
    Projects,
    Domains,
    None,
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
    /// Untuk fan-out layar Hosts: tiap host dapat thread sendiri, jadi hasilnya
    /// tak lewat lajur user/poll yang terikat satu client.
    resp_tx: Sender<Resp>,
}

fn spawn_workers(client: EasypanelClient) -> Workers {
    let (resp_tx, resp) = mpsc::channel::<Resp>();
    let user = spawn_worker(client.clone(), resp_tx.clone());
    let poll = spawn_worker(client, resp_tx.clone());
    Workers {
        user,
        poll,
        resp,
        resp_tx,
    }
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
        Req::AllServices => match client.call("projects", "listProjectsAndServices", Value::Null) {
            Ok(v) => Resp::AllServices {
                projects: v
                    .get("projects")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().map(|p| field(p, "/name")).collect())
                    .unwrap_or_default(),
                services: v
                    .get("services")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default(),
            },
            Err(e) => Resp::Err(e.to_string()),
        },
        Req::ServicesFor(project) => {
            match client.call(
                "projects",
                "inspectProject",
                json!({ "projectName": project }),
            ) {
                Ok(v) => Resp::ServicesFor(
                    project,
                    parse_services(&v).into_iter().map(|(n, _)| n).collect(),
                ),
                Err(e) => Resp::Err(e.to_string()),
            }
        }
        Req::ConfigForm {
            project,
            service,
            build,
        } => {
            let ps = json!({ "projectName": project, "serviceName": service });
            match client.call("services/app", "inspectService", ps) {
                // Repo hanya perlu untuk form source. Kegagalan searchRepos tidak
                // menggagalkan form: field "Repo" jatuh ke input teks biasa.
                Ok(data) => {
                    let repos = if build {
                        Vec::new()
                    } else {
                        github_repos(client)
                    };
                    Resp::ConfigForm {
                        project,
                        service,
                        build,
                        data,
                        repos,
                    }
                }
                Err(e) => Resp::Err(e.to_string()),
            }
        }
        Req::MaintInfo => {
            // Tiap baris berdiri sendiri: satu endpoint gagal tak boleh
            // mengosongkan seluruh tab.
            let one = |op: &str| match client.call("settings", op, Value::Null) {
                Ok(v) => field(&v, ""),
                Err(e) => format!("error: {e}"),
            };
            Resp::MaintInfo(vec![
                ("Docker".into(), one("getDockerVersion")),
                ("IP server".into(), one("getServerIp")),
                ("Update tersedia".into(), one("checkForUpdates")),
                ("Bersih-bersih harian".into(), one("getDailyDockerCleanup")),
            ])
        }
        Req::MaintAction(op) => match client.call("settings", op, Value::Null) {
            Ok(_) => Resp::Done(format!("{op} selesai"), Refresh::None),
            Err(e) => Resp::Err(e.to_string()),
        },
        Req::Branches { owner, repo } => {
            match client.call(
                "github",
                "searchBranches",
                json!({ "owner": owner, "repo": repo, "search": "" }),
            ) {
                // searchBranches mengembalikan array string datar (bukan {items:[...]}).
                Ok(v) => Resp::Branches(
                    v.as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(Value::as_str)
                                .map(String::from)
                                .collect()
                        })
                        .unwrap_or_default(),
                ),
                Err(e) => Resp::Err(e.to_string()),
            }
        }
        Req::ConfigSave {
            project,
            service,
            op,
            body,
            auto_deploy,
        } => {
            let ps = json!({ "projectName": project, "serviceName": service });
            let mut input = body;
            input["projectName"] = json!(project);
            input["serviceName"] = json!(service);
            match client.call("services/app", op, input) {
                Ok(_) => match auto_deploy {
                    Some(on) => {
                        let ep = if on {
                            "enableGithubDeploy"
                        } else {
                            "disableGithubDeploy"
                        };
                        match client.call("services/app", ep, ps) {
                            Ok(_) => Resp::Done("Tersimpan".into(), Refresh::None),
                            Err(e) => {
                                Resp::Err(format!("source tersimpan, auto deploy gagal: {e}"))
                            }
                        }
                    }
                    None => Resp::Done("Tersimpan".into(), Refresh::None),
                },
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
        Req::ProjectCreate(name) => {
            match client.call("projects", "createProject", json!({ "name": name })) {
                Ok(_) => Resp::Done(format!("Project '{name}' dibuat"), Refresh::Projects),
                Err(e) => Resp::Err(e.to_string()),
            }
        }
        Req::ProjectDestroy(name) => {
            match client.call("projects", "destroyProject", json!({ "name": name })) {
                Ok(_) => Resp::Done(format!("Project '{name}' dihapus"), Refresh::Projects),
                Err(e) => Resp::Err(e.to_string()),
            }
        }
        Req::ServiceCreate {
            project,
            service,
            stype,
        } => match client.call(
            &format!("services/{stype}"),
            "createService",
            json!({ "projectName": project, "serviceName": service }),
        ) {
            Ok(_) => Resp::Done(
                format!("Service '{service}' ({stype}) dibuat"),
                Refresh::Projects,
            ),
            Err(e) => Resp::Err(e.to_string()),
        },
        Req::DomainSave { id, body } => {
            // createDomain mewajibkan `id` tapi server mengabaikannya dan membuat
            // cuid sendiri, jadi placeholder cukup untuk domain baru.
            let op = if id.is_some() {
                "updateDomain"
            } else {
                "createDomain"
            };
            let mut input = body;
            input["id"] = json!(id.clone().unwrap_or_else(|| "new".to_string()));
            match client.call("domains", op, input) {
                Ok(_) => Resp::Done(
                    if id.is_some() {
                        "Domain diperbarui".into()
                    } else {
                        "Domain dibuat".into()
                    },
                    Refresh::Domains,
                ),
                Err(e) => Resp::Err(e.to_string()),
            }
        }
        Req::DomainDelete(id) => {
            match client.call("domains", "deleteDomain", json!({ "id": id })) {
                Ok(_) => Resp::Done("Domain dihapus".into(), Refresh::Domains),
                Err(e) => Resp::Err(e.to_string()),
            }
        }
        Req::DomainSetPrimary(id) => {
            match client.call("domains", "setPrimaryDomain", json!({ "id": id })) {
                Ok(_) => Resp::Done("Domain jadi primary".into(), Refresh::Domains),
                Err(e) => Resp::Err(e.to_string()),
            }
        }
        Req::EnvSave {
            project,
            service,
            stype,
            env,
        } => match client.call(
            &format!("services/{stype}"),
            "updateEnv",
            json!({ "projectName": project, "serviceName": service, "env": env }),
        ) {
            Ok(_) => Resp::Done(format!("Env {project}/{service} disimpan"), Refresh::None),
            Err(e) => Resp::Err(e.to_string()),
        },
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
        View::Source => {
            let v = client.call(&format!("services/{stype}"), "inspectService", ps)?;
            let mut out = Vec::new();
            // Sengaja tidak menampilkan `token` (deploy token) dan `env`:
            // keduanya kredensial, dan env sudah punya view sendiri.
            for (title, key) in [
                ("Source", "source"),
                ("Build", "build"),
                ("Deploy", "deploy"),
                ("Resources", "resources"),
            ] {
                out.push(format!("── {title}"));
                match v.get(key) {
                    // pointer "" = akar nilai itu sendiri, jadi string tampil tanpa kutip.
                    Some(Value::Object(o)) if !o.is_empty() => out.extend(
                        o.iter()
                            .map(|(k, val)| format!("  {k}: {}", field(val, ""))),
                    ),
                    _ => out.push("  (belum diatur)".into()),
                }
                out.push(String::new());
            }
            out
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

/// Daftar repo GitHub sebagai "owner/repo" untuk dropdown.
///
/// GitHub belum tentu tersambung di sebuah host, dan itu bukan alasan untuk
/// menggagalkan form: daftar kosong membuat "Repo" jadi input teks biasa.
fn github_repos(client: &EasypanelClient) -> Vec<String> {
    let Ok(v) = client.call("github", "searchRepos", Value::Null) else {
        return Vec::new();
    };
    v.get("items")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|r| {
                    Some(format!(
                        "{}/{}",
                        r.get("owner")?.as_str()?,
                        r.get("repo")?.as_str()?
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

const SERVICE_TYPES: &[&str] = &[
    "app",
    "mysql",
    "mariadb",
    "postgres",
    "mongo",
    "redis",
    "wordpress",
    "compose",
];
const DEST_KINDS: &[&str] = &["service", "custom"];
const PROTOCOLS: &[&str] = &["http", "https"];
const SOURCE_TYPES: &[&str] = &["github", "git", "image"];
const BUILD_TYPES: &[&str] = &[
    "nixpacks",
    "railpack",
    "dockerfile",
    "buildpacks",
    "heroku-buildpacks",
    "paketo-buildpacks",
];

/// Field form source; `source` adalah objek `source` dari inspectService.
///
/// `repos` kosong (GitHub tak tersambung / gagal) membuat "Repo" jadi input teks.
fn source_fields(source: Option<&Value>, repos: Vec<String>) -> Vec<Field> {
    let get = |ptr: &str, default: &str| match source.map(|s| field(s, ptr)) {
        Some(v) if v != "-" => v,
        _ => default.to_string(),
    };
    let stype = get("/type", "github");
    let (owner, repo) = (get("/owner", ""), get("/repo", ""));
    let current = if owner.is_empty() {
        String::new()
    } else {
        format!("{owner}/{repo}")
    };
    let branch = get("/ref", "");

    let mut repos = repos;
    if current.is_empty() {
        // Service baru belum punya source. Tanpa pilihan kosong, choice_owned
        // memilih repo pertama daftar — Enter tanpa sadar akan menunjuk source
        // ke repo acak, bukan gagal dengan jelas.
        repos.insert(0, String::new());
    } else if !repos.contains(&current) {
        // Repo yang sedang dipakai wajib ada di daftar. Kalau tidak, choice_owned
        // akan diam-diam memilih repo pertama — mengganti source service saat user
        // cuma bermaksud mengubah branch.
        repos.insert(0, current.clone());
    }
    let repo_field = if repos.is_empty() {
        Field::text("Repo", &current)
    } else {
        Field::choice_owned("Repo", repos, &current)
    };

    let auto_deploy = source
        .and_then(|s| s.get("autoDeploy"))
        .and_then(Value::as_bool)
        .unwrap_or(false);

    vec![
        Field::choice("Tipe", SOURCE_TYPES, &stype),
        repo_field.when("github"),
        // Diisi setelah branch repo tsb dimuat; nilai lama dipertahankan supaya
        // mode edit tak kehilangan pilihannya sebelum data tiba.
        Field::choice_owned("Branch", vec![branch.clone()], &branch).when("github"),
        Field::boolean("Auto deploy", auto_deploy).when("github"),
        Field::text("Git URL", if stype == "git" { &repo } else { "" }).when("git"),
        Field::text("Ref", &branch).when("git"),
        Field::text("Path", &get("/path", "/")).when("github,git"),
        Field::text("Image", &get("/image", "")).when("image"),
        Field::text("Username", &get("/username", "")).when("image"),
        Field::secret_val("Password", &get("/password", "")).when("image"),
    ]
}

/// Field form build; `build` adalah objek `build` dari inspectService.
///
/// nixpacks dan railpack berbagi label perintah yang sama — aman karena hanya
/// satu tipe yang tampil sekaligus, dan `by_label` membaca field yang tampil itu.
fn build_fields(build: Option<&Value>) -> Vec<Field> {
    let get = |ptr: &str, default: &str| match build.map(|b| field(b, ptr)) {
        Some(v) if v != "-" => v,
        _ => default.to_string(),
    };
    vec![
        Field::choice("Tipe", BUILD_TYPES, &get("/type", "nixpacks")),
        Field::text("Install command", &get("/installCommand", "")).when("nixpacks,railpack"),
        Field::text("Build command", &get("/buildCommand", "")).when("nixpacks,railpack"),
        Field::text("Start command", &get("/startCommand", "")).when("nixpacks,railpack"),
        Field::text("Nix packages", &get("/nixPackages", "")).when("nixpacks"),
        Field::text("Apt packages", &get("/aptPackages", "")).when("nixpacks"),
        Field::text("Mise packages", &get("/misePackages", "")).when("railpack"),
        Field::text("Dockerfile", &get("/file", "Dockerfile")).when("dockerfile"),
        Field::text("Builder", &get("/buildpacksBuilder", "heroku/builder:24")).when("buildpacks"),
    ]
}

/// Field form domain; `existing` mengisi nilai awal saat mode edit.
///
/// Field service dan custom ditampilkan sekaligus; yang dipakai ditentukan
/// "Tujuan". Ini mengikuti dialog panel, yang juga punya Protocol dan destination
/// custom (URL + weight) — keduanya tak boleh hilang saat mengedit.
fn domain_fields(existing: Option<&Value>, projects: &[String]) -> Vec<Field> {
    let get = |ptr: &str, default: &str| match existing.map(|d| field(d, ptr)) {
        Some(v) if v != "-" => v,
        _ => default.to_string(),
    };
    let https = existing
        .and_then(|d| d.get("https"))
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let server = existing.and_then(|d| d.pointer("/customDestination/servers/0"));
    let service = get("/serviceDestination/serviceName", "");

    let wildcard = existing
        .and_then(|d| d.get("wildcard"))
        .and_then(Value::as_bool)
        .unwrap_or(false);

    vec![
        Field::text("Host", &get("/host", "")),
        Field::text("Path", &get("/path", "/")),
        Field::boolean("HTTPS", https),
        // Nama resolver Traefik ditentukan konfigurasi server (mis. "letsencrypt",
        // "google"); tak ada endpoint untuk mendaftarnya, jadi teks bebas —
        // menebak-nebak isi dropdown justru menyesatkan.
        Field::text("SSL resolver", &get("/certificateResolver", "")),
        Field::boolean("Wildcard", wildcard),
        Field::choice("Tujuan", DEST_KINDS, &get("/destinationType", "service")),
        Field::choice_owned(
            "Project",
            projects.to_vec(),
            &get("/serviceDestination/projectName", ""),
        )
        .when("service"),
        // Diisi setelah service project tsb dimuat; nilai lama dipertahankan
        // supaya mode edit tidak kehilangan pilihannya sebelum data tiba.
        Field::choice_owned("Service", vec![service.clone()], &service).when("service"),
        Field::choice(
            "Protocol",
            PROTOCOLS,
            &get("/serviceDestination/protocol", "http"),
        )
        .when("service"),
        Field::text("Port", &get("/serviceDestination/port", "80")).when("service"),
        Field::text("Path tujuan", &get("/serviceDestination/path", "/")).when("service"),
        Field::text(
            "Server URL",
            &server.map(|s| field(s, "/url")).unwrap_or_default(),
        )
        .when("custom"),
        Field::text(
            "Weight",
            &server.map(|s| field(s, "/weight")).unwrap_or("1".into()),
        )
        .when("custom"),
    ]
}

/// Endpoint + body updateSource* dari form.
///
/// Tiap tipe source punya endpoint sendiri dengan field yang persis ditentukan
/// skema, jadi body dibangun dari nol — tak ada field tak termodel yang perlu
/// dilestarikan seperti pada domain.
/// `auto_deploy` hanya relevan untuk source github (endpoint lain tak punya konsep ini).
fn source_body(form: &Form) -> std::result::Result<(&'static str, Value, Option<bool>), String> {
    let path = match form.by_label("Path").as_str() {
        "" => "/".to_string(),
        p => p.to_string(),
    };
    if !path.starts_with('/') {
        return Err("Path harus diawali /".into());
    }

    match form.by_label("Tipe").as_str() {
        "github" => {
            let full = form.by_label("Repo");
            if full.is_empty() {
                return Err("Repo wajib dipilih".into());
            }
            let (owner, repo) = full
                .split_once('/')
                .ok_or("Repo harus berbentuk owner/repo")?;
            let branch = form.by_label("Branch");
            if owner.is_empty() || repo.is_empty() || branch.is_empty() {
                return Err("Repo dan Branch wajib diisi".into());
            }
            Ok((
                "updateSourceGithub",
                json!({ "owner": owner, "repo": repo, "ref": branch, "path": path }),
                Some(form.is_on_label("Auto deploy")),
            ))
        }
        "git" => {
            let (repo, git_ref) = (form.by_label("Git URL"), form.by_label("Ref"));
            if repo.is_empty() || git_ref.is_empty() {
                return Err("Git URL dan Ref wajib diisi".into());
            }
            Ok((
                "updateSourceGit",
                json!({ "repo": repo, "ref": git_ref, "path": path }),
                None,
            ))
        }
        _ => {
            let image = form.by_label("Image");
            if image.is_empty() {
                return Err("Image wajib diisi".into());
            }
            let mut body = json!({ "image": image });
            // username/password opsional: kosong = tak dikirim, bukan dikirim "".
            for (label, key) in [("Username", "username"), ("Password", "password")] {
                let v = form.by_label(label);
                if !v.is_empty() {
                    body[key] = json!(v);
                }
            }
            Ok(("updateSourceImage", body, None))
        }
    }
}

/// Body updateBuild dari form.
///
/// Berangkat dari build asli hanya bila tipenya tak berubah, supaya field yang
/// tak ada di form (nixpacksVersion, railpackVersion) tetap utuh. Saat tipe
/// diganti, field tipe lama justru tak boleh ikut terbawa.
fn build_body(form: &Form) -> std::result::Result<Value, String> {
    let t = form.by_label("Tipe");
    let same_type =
        form.original.as_ref().map(|o| field(o, "/type")).as_deref() == Some(t.as_str());

    let mut build = match form.original.clone() {
        Some(o) if same_type && o.is_object() => o,
        _ => json!({}),
    };
    build["type"] = json!(t);

    let keys: &[(&str, &str)] = match t.as_str() {
        "nixpacks" => &[
            ("Install command", "installCommand"),
            ("Build command", "buildCommand"),
            ("Start command", "startCommand"),
            ("Nix packages", "nixPackages"),
            ("Apt packages", "aptPackages"),
        ],
        "railpack" => &[
            ("Install command", "installCommand"),
            ("Build command", "buildCommand"),
            ("Start command", "startCommand"),
            ("Mise packages", "misePackages"),
        ],
        "dockerfile" => &[("Dockerfile", "file")],
        "buildpacks" => &[("Builder", "buildpacksBuilder")],
        // heroku-buildpacks / paketo-buildpacks cuma butuh `type`.
        _ => &[],
    };

    let obj = build.as_object_mut().ok_or("bentuk build tak dikenal")?;
    for (label, key) in keys {
        match form.by_label(label) {
            v if v.is_empty() => obj.remove(*key),
            v => obj.insert((*key).to_string(), json!(v)),
        };
    }
    Ok(json!({ "build": build }))
}

/// Body createDomain/updateDomain dari form.
///
/// Saat edit, berangkat dari JSON domain aslinya sehingga field yang tak ada
/// di form (middlewares) tetap utuh — bukan ditimpa nilai default.
fn domain_body(form: &Form) -> std::result::Result<Value, String> {
    let host = form.by_label("Host");
    if host.is_empty() {
        return Err("Host wajib diisi".into());
    }

    let mut body = form.original.clone().unwrap_or_else(
        || json!({ "wildcard": false, "certificateResolver": "", "middlewares": [] }),
    );
    body["host"] = json!(host);
    body["path"] = json!(match form.by_label("Path").as_str() {
        "" => "/".to_string(),
        p => p.to_string(),
    });
    body["https"] = json!(form.is_on_label("HTTPS"));
    body["certificateResolver"] = json!(form.by_label("SSL resolver"));
    body["wildcard"] = json!(form.is_on_label("Wildcard"));

    let obj = body.as_object_mut().ok_or("bentuk domain tak dikenal")?;
    if form.by_label("Tujuan") == "custom" {
        let url = form.by_label("Server URL");
        if url.is_empty() {
            return Err("Server URL wajib diisi untuk tujuan custom".into());
        }
        let weight: u32 = form
            .by_label("Weight")
            .parse()
            .map_err(|_| "Weight harus angka")?;

        // Form hanya memodelkan server pertama. Server lain (kalau ada) harus
        // ikut utuh — memangkasnya diam-diam sama saja merusak konfigurasi.
        let mut servers = form
            .original
            .as_ref()
            .and_then(|o| o.pointer("/customDestination/servers"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let first = json!({ "url": url, "weight": weight });
        if servers.is_empty() {
            servers.push(first);
        } else {
            servers[0] = first;
        }

        obj.remove("serviceDestination");
        obj.insert("destinationType".into(), json!("custom"));
        obj.insert("customDestination".into(), json!({ "servers": servers }));
    } else {
        let (project, service) = (form.by_label("Project"), form.by_label("Service"));
        if project.is_empty() || service.is_empty() {
            return Err("Project dan service wajib diisi".into());
        }
        let port: u32 = form
            .by_label("Port")
            .parse()
            .map_err(|_| "Port harus angka")?;
        obj.remove("customDestination");
        obj.insert("destinationType".into(), json!("service"));
        obj.insert(
            "serviceDestination".into(),
            json!({
                "projectName": project,
                "serviceName": service,
                "port": port,
                "protocol": form.by_label("Protocol"),
                "path": match form.by_label("Path tujuan").as_str() {
                    "" => "/".to_string(),
                    p => p.to_string(),
                }
            }),
        );
    }
    Ok(body)
}

/// Nama+tipe service dari inspectProject, untuk dropdown Service di form domain.
fn parse_services(v: &Value) -> Vec<(String, String)> {
    v.get("services")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|s| {
                    (
                        field(s, "/name"),
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

const SERVICE_HEADERS: [&str; 9] = [
    "Project", "Service", "Tipe", "Status", "Source", "CPU %", "Memory", "Net In", "Net Out",
];

/// Satu baris tabel service datar.
///
/// `source` diringkas dari inspectService-nya listProjectsAndServices, jadi repo
/// dan branch terlihat tanpa membuka apa pun.
fn service_row(s: &Value) -> Vec<String> {
    let source = match field(s, "/source/type").as_str() {
        "github" => format!(
            "{}/{}#{}",
            field(s, "/source/owner"),
            field(s, "/source/repo"),
            field(s, "/source/ref")
        ),
        "git" => format!("{}#{}", field(s, "/source/repo"), field(s, "/source/ref")),
        "image" => field(s, "/source/image"),
        _ => "-".to_string(),
    };
    let enabled = s.get("enabled").and_then(Value::as_bool).unwrap_or(true);
    vec![
        field(s, "/projectName"),
        field(s, "/name"),
        field(s, "/type"),
        if enabled { "aktif" } else { "mati" }.to_string(),
        source,
    ]
}

/// Kolom metrik untuk sebuah service; "-" bila metriknya belum/tak ada.
///
/// Dipisah dari service_row() supaya filter hanya mencocokkan identitas
/// (project/service/tipe/source) — mencari "1" tak seharusnya cocok ke setiap
/// baris hanya karena angka CPU-nya.
fn metric_cols(m: Option<&Value>) -> Vec<String> {
    let Some(m) = m else {
        return vec!["-".into(), "-".into(), "-".into(), "-".into()];
    };
    vec![
        format!("{:.1} %", num(m, "/cpu")),
        format_bytes(num(m, "/memory")),
        format_rate(num(m, "/networkIn")),
        format_rate(num(m, "/networkOut")),
    ]
}

/// Apakah sebuah baris lolos filter.
///
/// Dicocokkan ke teks yang DITAMPILKAN, bukan ke JSON mentahnya: yang dicari user
/// adalah yang terlihat di layar.
fn keep(row: &[String], filter: &str) -> bool {
    if filter.is_empty() {
        return true;
    }
    let f = filter.to_lowercase();
    row.iter().any(|c| c.to_lowercase().contains(&f))
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
    /// Semua host sekaligus — satu-satunya layar yang tak bisa digantikan panel web.
    Hosts,
    /// Info & pembersihan Docker pada server aktif.
    Maintenance,
    Actions,
    Monitor,
    Domains,
    Projects,
    Viewer,
}

/// Viewer sengaja TIDAK ada di sini: ia hasil dari membuka sesuatu pada sebuah
/// service, bukan tujuan tersendiri. Sebagai tab ia hanya kotak kosong sampai
/// user datang dari Projects.
const TABS: [&str; 7] = [
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

impl Screen {
    fn index(self) -> usize {
        match self {
            Screen::Dashboard => 0,
            Screen::Hosts => 1,
            Screen::Maintenance => 2,
            Screen::Actions => 3,
            Screen::Monitor => 4,
            Screen::Domains => 5,
            Screen::Projects => 6,
            // Viewer selalu dibuka dari Projects, jadi tab itu yang tetap
            // tersorot — Viewer sendiri tak punya tab.
            Screen::Viewer => 6,
        }
    }
    fn next(self) -> Self {
        match self {
            Screen::Dashboard => Screen::Hosts,
            Screen::Hosts => Screen::Maintenance,
            Screen::Maintenance => Screen::Actions,
            Screen::Actions => Screen::Monitor,
            Screen::Monitor => Screen::Domains,
            Screen::Domains => Screen::Projects,
            Screen::Projects => Screen::Dashboard,
            Screen::Viewer => Screen::Dashboard,
        }
    }
}

/// Satu baris di layar Hosts. Host yang mati harus tampil sebagai baris error,
/// bukan menggagalkan seluruh tabel.
struct HostRow {
    name: String,
    url: String,
    state: HostState,
}

enum HostState {
    Loading,
    Ok(Box<Value>),
    Err(String),
}

/// Sub-tab pada layar Monitor (mengikuti panel).
#[derive(PartialEq, Clone, Copy)]
enum MonitorView {
    Services,
    Storage,
}

struct Confirm {
    action: String,
    project: String,
    service: String,
    stype: String,
    label: String,
}

// ---------- Form (ratatui tak punya widget input, jadi dibuat sendiri) ----------

#[derive(PartialEq, Clone)]
enum FieldKind {
    Text,
    Secret,
    Bool,
    /// Pilihan dari data nyata (project/service/protocol), digilir dgn spasi/←/→.
    /// Dinamis supaya isinya bisa datang dari API, bukan diketik manual.
    Choice(Vec<String>),
}

impl FieldKind {
    fn is_typed(&self) -> bool {
        matches!(self, FieldKind::Text | FieldKind::Secret)
    }
}

struct Field {
    label: &'static str,
    value: String,
    kind: FieldKind,
    /// Bila diisi, field hanya tampil saat field switch form bernilai salah satu
    /// dari daftar ini (dipisah koma, mis. "github,git").
    /// Panel juga begini: memilih Service/Custom mengganti field di bawahnya,
    /// bukan menampilkan keduanya sekaligus.
    only_for: Option<&'static str>,
}

impl Field {
    fn text(label: &'static str, value: &str) -> Self {
        Self {
            label,
            value: value.into(),
            kind: FieldKind::Text,
            only_for: None,
        }
    }
    /// Tampilkan field ini hanya saat switch form bernilai `dest`
    /// (boleh beberapa nilai dipisah koma, mis. "github,git").
    fn when(mut self, dest: &'static str) -> Self {
        self.only_for = Some(dest);
        self
    }
    fn secret(label: &'static str) -> Self {
        Self::secret_val(label, "")
    }
    fn secret_val(label: &'static str, value: &str) -> Self {
        Self {
            label,
            value: value.into(),
            kind: FieldKind::Secret,
            only_for: None,
        }
    }
    fn boolean(label: &'static str, on: bool) -> Self {
        Self {
            label,
            value: if on { "ya".into() } else { "tidak".into() },
            kind: FieldKind::Bool,
            only_for: None,
        }
    }
    fn choice(label: &'static str, options: &[&str], value: &str) -> Self {
        Self::choice_owned(
            label,
            options.iter().map(|o| o.to_string()).collect(),
            value,
        )
    }
    fn choice_owned(label: &'static str, options: Vec<String>, value: &str) -> Self {
        let value = if options.iter().any(|o| o == value) {
            value.to_string()
        } else {
            options.first().cloned().unwrap_or_default()
        };
        Self {
            label,
            value,
            kind: FieldKind::Choice(options),
            only_for: None,
        }
    }
    /// Ganti daftar pilihan (mis. service terisi setelah project dipilih).
    ///
    /// Nilai yang sedang dipakai selalu dipertahankan, meski tak ada di daftar
    /// baru: melompat diam-diam ke pilihan pertama akan mengubah konfigurasi yang
    /// tak diminta user — mis. `ref` yang berupa tag akan berganti jadi branch
    /// pertama sesuai abjad, lalu ikut ter-deploy.
    fn set_options(&mut self, mut options: Vec<String>) {
        if !self.value.is_empty() && !options.contains(&self.value) {
            options.insert(0, self.value.clone());
        }
        if !options.contains(&self.value) {
            self.value = options.first().cloned().unwrap_or_default();
        }
        self.kind = FieldKind::Choice(options);
    }
    /// Gilir ke pilihan berikutnya (Bool diperlakukan sebagai ya/tidak).
    fn cycle(&mut self) {
        match self.kind {
            FieldKind::Bool => {
                self.value = if self.is_on() {
                    "tidak".into()
                } else {
                    "ya".into()
                }
            }
            FieldKind::Choice(ref opts) => {
                if opts.is_empty() {
                    return;
                }
                let i = opts.iter().position(|o| *o == self.value).unwrap_or(0);
                self.value = opts[(i + 1) % opts.len()].clone();
            }
            _ => {}
        }
    }
    fn is_on(&self) -> bool {
        self.value == "ya"
    }
    fn shown(&self) -> String {
        match self.kind {
            FieldKind::Secret => "•".repeat(self.value.chars().count()),
            _ => self.value.clone(),
        }
    }
}

/// Apa yang dilakukan form saat disubmit.
enum FormKind {
    ServerAdd,
    ServerEdit {
        name: String,
    },
    ProjectCreate,
    /// Project ikut jadi field form: daftar datar tak punya "project yang
    /// sedang dibuka" untuk diwarisi.
    ServiceCreate,
    DomainCreate,
    DomainEdit {
        id: String,
    },
    SourceEdit {
        project: String,
        service: String,
    },
    BuildEdit {
        project: String,
        service: String,
    },
}

struct Form {
    kind: FormKind,
    title: String,
    fields: Vec<Field>,
    focus: usize,
    /// Label field yang menentukan field lain mana yang tampil ("Tujuan" di form
    /// domain, "Tipe" di form source/build).
    switch: &'static str,
    /// JSON asli saat mode edit. Submit berangkat dari sini supaya field yang
    /// tak ada di form (middlewares pada domain, nixpacksVersion pada build)
    /// ikut utuh.
    original: Option<Value>,
}

impl Form {
    fn new(kind: FormKind, title: impl Into<String>, fields: Vec<Field>) -> Self {
        Self {
            kind,
            title: title.into(),
            fields,
            focus: 0,
            switch: "Tujuan",
            original: None,
        }
    }
    /// Ganti field penentu visibilitas (default "Tujuan").
    fn switch(mut self, label: &'static str) -> Self {
        self.switch = label;
        self
    }
    fn with_original(mut self, original: Value) -> Self {
        self.original = Some(original);
        self
    }
    /// Indeks field yang tampil untuk nilai switch yang sedang dipilih.
    fn visible(&self) -> Vec<usize> {
        let cur = self.by_label(self.switch);
        self.fields
            .iter()
            .enumerate()
            .filter(|(_, f)| match f.only_for {
                None => true,
                Some(d) => d.split(',').any(|t| t == cur),
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// Pindah fokus `delta` langkah di antara field yang tampil saja.
    fn move_focus(&mut self, delta: isize) {
        let vis = self.visible();
        if vis.is_empty() {
            return;
        }
        let at = vis.iter().position(|i| *i == self.focus).unwrap_or(0) as isize;
        let next = (at + delta).rem_euclid(vis.len() as isize) as usize;
        self.focus = vis[next];
    }

    /// Setelah Tujuan berganti, fokus bisa tertinggal di field yang kini tersembunyi.
    fn clamp_focus(&mut self) {
        let vis = self.visible();
        if !vis.contains(&self.focus) {
            self.focus = vis.first().copied().unwrap_or(0);
        }
    }

    fn val(&self, i: usize) -> String {
        self.fields[i].value.trim().to_string()
    }
    fn by_label(&self, label: &str) -> String {
        self.fields
            .iter()
            .find(|f| f.label == label)
            .map(|f| f.value.trim().to_string())
            .unwrap_or_default()
    }
    fn is_on_label(&self, label: &str) -> bool {
        self.fields
            .iter()
            .find(|f| f.label == label)
            .map(Field::is_on)
            .unwrap_or(false)
    }
}

/// Dropdown untuk sebuah field Choice: daftar pilihan + filter ketik.
///
/// Menggilir pilihan dengan spasi tidak terpakai untuk daftar panjang (11 service),
/// jadi field Choice membuka daftar sungguhan yang bisa dicari.
struct Chooser {
    field: usize,
    label: &'static str,
    options: Vec<String>,
    filter: String,
    state: ListState,
}

impl Chooser {
    fn new(field: usize, label: &'static str, options: Vec<String>, current: &str) -> Self {
        let mut state = ListState::default();
        state.select(Some(options.iter().position(|o| o == current).unwrap_or(0)));
        Self {
            field,
            label,
            options,
            filter: String::new(),
            state,
        }
    }

    /// Pilihan yang lolos filter (case-insensitive, substring).
    fn matches(&self) -> Vec<String> {
        let f = self.filter.to_lowercase();
        self.options
            .iter()
            .filter(|o| f.is_empty() || o.to_lowercase().contains(&f))
            .cloned()
            .collect()
    }

    fn selected(&self) -> Option<String> {
        let m = self.matches();
        self.state.selected().and_then(|i| m.get(i).cloned())
    }

    /// Jaga agar indeks terpilih tetap valid setelah filter berubah.
    fn clamp(&mut self) {
        let len = self.matches().len();
        let i = self.state.selected().unwrap_or(0);
        self.state
            .select(if len == 0 { None } else { Some(i.min(len - 1)) });
    }
}

/// Perubahan daftar server: dieksekusi di event_loop yang memegang ServerConfig.
enum ServerAction {
    Save {
        name: String,
        url: String,
        /// None = pertahankan token yang tersimpan (form edit yang dibiarkan kosong).
        token: Option<String>,
    },
    Remove(String),
}

struct App {
    server_name: String,
    /// (nama, url) tiap server. URL ikut disimpan supaya form edit bisa
    /// terisi nilai sekarang, bukan kosong seperti form tambah.
    all_servers: Vec<(String, String)>,
    switch_to: Option<String>,
    picker: Option<ListState>,
    form: Option<Form>,
    chooser: Option<Chooser>,
    server_action: Option<ServerAction>,
    edit_env: Option<(String, String, String)>,

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
    /// Semua service lintas project. Daftar datar menggantikan hirarki
    /// project -> service: drill-down tak bisa dicari dan runtuh di ratusan service.
    all_services: Vec<Value>,
    services_table: TableState,

    viewer_title: String,
    viewer_lines: Vec<String>,
    viewer_scroll: u16,
    viewer_ctx: Option<(View, String, String, String)>,

    /// Teks filter untuk tabel layar aktif ("" = tanpa filter).
    filter: String,
    /// Sedang mengetik filter (tombol masuk ke filter, bukan ke layar).
    filter_input: bool,
    /// Overlay bantuan sedang terbuka.
    help: bool,
    /// Baris info tab Maintenance: (label, nilai).
    maint: Vec<(String, String)>,
    hosts: Vec<HostRow>,
    hosts_state: TableState,
    /// Diset saat layar Hosts perlu data; fan-out-nya dijalankan event_loop.
    load_hosts: bool,

    confirm: Option<Confirm>,
}

impl App {
    fn new(server_name: String, all_servers: Vec<(String, String)>) -> Self {
        Self {
            server_name,
            all_servers,
            switch_to: None,
            picker: None,
            form: None,
            chooser: None,
            server_action: None,
            edit_env: None,
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
            all_services: Vec::new(),
            services_table: TableState::default(),
            viewer_title: "Viewer".into(),
            viewer_lines: Vec::new(),
            viewer_scroll: 0,
            viewer_ctx: None,
            filter: String::new(),
            filter_input: false,
            help: false,
            maint: Vec::new(),
            hosts: Vec::new(),
            hosts_state: TableState::default(),
            load_hosts: false,
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
        self.all_services.clear();
        self.services_table = TableState::default();
        self.viewer_lines.clear();
        self.viewer_ctx = None;
    }

    fn handle(&mut self, resp: Resp, req: &Sender<Req>) {
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
            Resp::Projects(p) => self.projects = p,
            Resp::AllServices { projects, services } => {
                self.projects = projects;
                self.all_services = services;
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
                self.form = Some(form.switch("Tipe"));
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
            Resp::Branches(names) => {
                if let Some(form) = self.form.as_mut() {
                    if let Some(f) = form.fields.iter_mut().find(|f| f.label == "Branch") {
                        f.set_options(names);
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
            Resp::Msg(m) => self.status = m,
            Resp::Err(e) => self.status = format!("Error: {e}"),
        }
    }

    fn on_key(&mut self, code: KeyCode, req: &Sender<Req>) {
        // Bantuan menutup dengan tombol apa pun: user membukanya untuk membaca,
        // bukan untuk menghafal cara keluar.
        if self.help {
            self.help = false;
            return;
        }
        if self.filter_input {
            self.filter_key(code);
            return;
        }
        if self.chooser.is_some() {
            self.chooser_key(code, req);
            return;
        }
        if self.form.is_some() {
            self.form_key(code, req);
            return;
        }
        if self.confirm.is_some() {
            self.confirm_key(code, req);
            return;
        }
        if self.picker.is_some() {
            self.picker_key(code, req);
            return;
        }

        match code {
            KeyCode::Esc if !self.filter.is_empty() => self.clear_filter(),
            // Esc TIDAK menutup aplikasi. Esc berarti "batal": ia menutup form,
            // dropdown, konfirmasi, atau filter — dan bila tak ada yang perlu
            // dibatalkan, ia tak melakukan apa-apa. Menutup TUI karena satu
            // ketukan Esc refleks adalah kehilangan konteks tanpa peringatan.
            // Keluar: 'q' atau Ctrl-C.
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('1') => self.screen = Screen::Dashboard,
            KeyCode::Char('2') => self.goto(Screen::Hosts, req),
            KeyCode::Char('3') => self.goto(Screen::Maintenance, req),
            KeyCode::Char('4') => self.goto(Screen::Actions, req),
            KeyCode::Char('5') => self.goto(Screen::Monitor, req),
            KeyCode::Char('6') => self.goto(Screen::Domains, req),
            KeyCode::Char('7') => self.goto(Screen::Projects, req),
            KeyCode::Tab => self.goto(self.screen.next(), req),
            KeyCode::Char('?') => self.help = true,
            KeyCode::Char('s') => self.open_picker(),
            KeyCode::Char('r') => self.refresh(req),
            KeyCode::Char('/') if self.filterable() => {
                self.filter_input = true;
                self.filter.clear();
            }
            _ => match self.screen {
                Screen::Projects => self.services_key(code, req),
                Screen::Viewer => self.viewer_key(code),
                Screen::Actions => move_table(&mut self.actions_state, code, self.actions.len()),
                Screen::Domains => self.domains_key(code, req),
                Screen::Monitor => self.monitor_key(code, req),
                Screen::Hosts => move_table(&mut self.hosts_state, code, self.hosts.len()),
                Screen::Maintenance => self.maint_key(code),
                Screen::Dashboard => {}
            },
        }
    }

    fn filterable(&self) -> bool {
        matches!(
            self.screen,
            Screen::Domains | Screen::Actions | Screen::Monitor | Screen::Projects
        )
    }

    fn clear_filter(&mut self) {
        self.filter.clear();
        self.filter_input = false;
        self.clamp_filtered();
    }

    /// Filter mengecilkan daftar, jadi baris terpilih bisa jatuh di luar batas.
    fn clamp_filtered(&mut self) {
        let len = match self.screen {
            Screen::Domains => self.visible_domains().len(),
            Screen::Actions => self.visible_actions().len(),
            Screen::Monitor => self.visible_monitor_rows().len(),
            Screen::Projects => self.visible_services().len(),
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

    fn filter_key(&mut self, code: KeyCode) {
        match code {
            // Esc membatalkan filter sepenuhnya; Enter menyimpannya dan kembali
            // ke navigasi biasa.
            KeyCode::Esc => self.clear_filter(),
            KeyCode::Enter => self.filter_input = false,
            KeyCode::Backspace => {
                self.filter.pop();
                self.clamp_filtered();
            }
            KeyCode::Char(c) => {
                self.filter.push(c);
                self.clamp_filtered();
            }
            _ => {}
        }
    }

    fn visible_actions(&self) -> Vec<&Value> {
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
    fn visible_monitor_rows(&self) -> Vec<Vec<String>> {
        commands::monitor_rows(self.monitor.clone())
            .into_iter()
            .filter(|r| keep(r, &self.filter))
            .collect()
    }

    /// Pindah layar dan muat datanya bila belum ada.
    fn goto(&mut self, screen: Screen, req: &Sender<Req>) {
        // Filter milik layar tempat ia diketik. Membawanya ke layar lain berarti
        // menyembunyikan baris tanpa sebab yang terlihat.
        self.filter.clear();
        self.filter_input = false;
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

    /// Pembersihan Docker itu destruktif dan tak bisa dibatalkan, jadi tiap aksi
    /// lewat konfirmasi — sama seperti deploy/destroy.
    fn maint_key(&mut self, code: KeyCode) {
        let (op, label) = match code {
            KeyCode::Char('p') => (
                "systemPrune",
                "Prune sistem Docker? Container, network, image, dan build cache yang tak terpakai akan dihapus.",
            ),
            KeyCode::Char('i') => (
                "cleanupDockerImages",
                "Hapus image Docker yang tak terpakai?",
            ),
            KeyCode::Char('c') => (
                "cleanupDockerBuilder",
                "Hapus build cache Docker?",
            ),
            _ => return,
        };
        self.confirm = Some(Confirm {
            action: format!("maint:{op}"),
            project: String::new(),
            service: String::new(),
            stype: String::new(),
            label: label.into(),
        });
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

    /// (nama, url) server yang sedang disorot di picker.
    fn picker_selected(&self) -> Option<(String, String)> {
        self.picker
            .as_ref()
            .and_then(|s| s.selected())
            .and_then(|i| self.all_servers.get(i).cloned())
    }

    fn start_env_edit(&mut self) {
        if let Some((p, s, t)) = self.selected_row() {
            self.edit_env = Some((p, s, t));
        }
    }

    /// Service yang lolos filter.
    ///
    /// Render DAN aksi wajib lewat sini: kalau render difilter sementara aksi
    /// memakai indeks daftar penuh, `x` akan menghapus service yang salah.
    fn visible_services(&self) -> Vec<&Value> {
        self.all_services
            .iter()
            .filter(|s| keep(&service_row(s), &self.filter))
            .collect()
    }

    /// Metrik untuk sebuah service, dijoin lewat (projectName, serviceName).
    ///
    /// getAllServicesStats memuat lebih banyak entri daripada daftar service
    /// (service sistem, sub-service compose), jadi yang tak cocok diabaikan.
    fn metric_for(&self, project: &str, service: &str) -> Option<&Value> {
        self.monitor.iter().find(|m| {
            m.get("projectName").and_then(Value::as_str) == Some(project)
                && m.get("serviceName").and_then(Value::as_str) == Some(service)
        })
    }

    /// Baris tabel lengkap: identitas + metrik.
    fn service_row_full(&self, s: &Value) -> Vec<String> {
        let mut row = service_row(s);
        let m = self.metric_for(&row[0], &row[1]);
        row.extend(metric_cols(m));
        row
    }

    /// (project, service, tipe) dari baris yang disorot di daftar datar.
    fn selected_row(&self) -> Option<(String, String, String)> {
        let vis = self.visible_services();
        let s = self.services_table.selected().and_then(|i| vis.get(i))?;
        Some((
            field(s, "/projectName"),
            field(s, "/name"),
            field(s, "/type"),
        ))
    }

    /// Domain yang lolos filter.
    ///
    /// Render DAN aksi (e/x/P) wajib lewat sini. Kalau render difilter sementara
    /// aksi memakai indeks daftar penuh, `x` akan menghapus domain yang salah.
    fn visible_domains(&self) -> Vec<&Value> {
        self.domains
            .iter()
            .filter(|d| keep(&commands::domain_row(d), &self.filter))
            .collect()
    }

    fn domains_key(&mut self, code: KeyCode, req: &Sender<Req>) {
        let selected = self
            .domains_state
            .selected()
            .and_then(|i| self.visible_domains().get(i).map(|d| (*d).clone()));

        match code {
            KeyCode::Char('n') => {
                let fields = domain_fields(None, &self.projects);
                self.form = Some(Form::new(FormKind::DomainCreate, " Domain baru ", fields));
                self.load_form_services(req);
            }
            KeyCode::Char('e') => {
                if let Some(d) = selected {
                    self.form = Some(
                        Form::new(
                            FormKind::DomainEdit {
                                id: field(&d, "/id"),
                            },
                            format!(" Edit domain: {} ", field(&d, "/host")),
                            domain_fields(Some(&d), &self.projects),
                        )
                        .with_original(d),
                    );
                    self.load_form_services(req);
                }
            }
            KeyCode::Char('x') => {
                if let Some(d) = selected {
                    self.confirm = Some(Confirm {
                        action: "domain-delete".into(),
                        project: field(&d, "/id"),
                        service: String::new(),
                        stype: String::new(),
                        label: format!("Hapus domain '{}'?", field(&d, "/host")),
                    });
                }
            }
            KeyCode::Char('P') => {
                if let Some(d) = selected {
                    let _ = req.send(Req::DomainSetPrimary(field(&d, "/id")));
                }
            }
            _ => {
                let n = self.visible_domains().len();
                move_table(&mut self.domains_state, code, n)
            }
        }
    }

    /// Muat daftar service untuk project yang sedang dipilih di form, supaya
    /// field Service jadi pilihan nyata dan bukan ketikan bebas.
    fn load_form_services(&mut self, req: &Sender<Req>) {
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
    fn open_config_form(&mut self, build: bool, req: &Sender<Req>) {
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

    /// Muat branch repo yang sedang dipilih ke dropdown "Branch".
    fn load_form_branches(&mut self, req: &Sender<Req>) {
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
    fn open_chooser(&mut self) {
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

    fn chooser_key(&mut self, code: KeyCode, req: &Sender<Req>) {
        let Some(ch) = self.chooser.as_mut() else {
            return;
        };
        match code {
            KeyCode::Esc => self.chooser = None,
            KeyCode::Down => {
                let len = ch.matches().len();
                let i = ch.state.selected().unwrap_or(0);
                if len > 0 {
                    ch.state.select(Some((i + 1).min(len - 1)));
                }
            }
            KeyCode::Up => {
                let i = ch.state.selected().unwrap_or(0);
                ch.state.select(Some(i.saturating_sub(1)));
            }
            KeyCode::Backspace => {
                ch.filter.pop();
                ch.clamp();
            }
            KeyCode::Char(c) => {
                ch.filter.push(c);
                ch.clamp();
            }
            KeyCode::Enter => {
                let picked = ch.selected();
                let (idx, label) = (ch.field, ch.label);
                self.chooser = None;
                if let (Some(value), Some(form)) = (picked, self.form.as_mut()) {
                    form.fields[idx].value = value;
                    // Ganti Tujuan/Tipe -> set field yang tampil ikut berubah.
                    form.clamp_focus();
                    // Ganti project/repo -> daftar turunannya ikut dimuat ulang.
                    match label {
                        "Project" => self.load_form_services(req),
                        // Branch lama milik repo lain: kosongkan supaya
                        // set_options tidak mempertahankannya.
                        "Repo" => {
                            if let Some(f) = form.fields.iter_mut().find(|f| f.label == "Branch") {
                                f.value.clear();
                            }
                            self.load_form_branches(req);
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    fn form_key(&mut self, code: KeyCode, req: &Sender<Req>) {
        let Some(form) = self.form.as_mut() else {
            return;
        };
        let typed = form.fields[form.focus].kind.is_typed();

        match code {
            KeyCode::Esc => {
                self.form = None;
                self.status = "Dibatalkan".into();
            }
            KeyCode::Tab | KeyCode::Down => form.move_focus(1),
            KeyCode::BackTab | KeyCode::Up => form.move_focus(-1),
            // Bool cukup di-toggle; Choice membuka dropdown yang bisa dicari.
            KeyCode::Char(' ') | KeyCode::Enter | KeyCode::Left | KeyCode::Right if !typed => {
                if form.fields[form.focus].kind == FieldKind::Bool {
                    form.fields[form.focus].cycle();
                    form.clamp_focus();
                } else {
                    self.open_chooser();
                }
            }
            KeyCode::Backspace if typed => {
                form.fields[form.focus].value.pop();
            }
            KeyCode::Char(c) if typed => form.fields[form.focus].value.push(c),
            KeyCode::Enter => self.submit_form(req),
            _ => {}
        }
    }

    fn submit_form(&mut self, req: &Sender<Req>) {
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
                    self.status = "Nama service hanya boleh a-z, 0-9, -, _".into();
                    return;
                }
                let _ = req.send(Req::ServiceCreate {
                    project,
                    service,
                    stype,
                });
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

    fn confirm_key(&mut self, code: KeyCode, req: &Sender<Req>) {
        let c = self.confirm.take().unwrap();
        if !matches!(code, KeyCode::Char('y') | KeyCode::Char('Y')) {
            self.status = "Dibatalkan".into();
            return;
        }

        // Hapus project/domain punya endpoint sendiri; sisanya aksi service biasa
        // (deploy/restart/stop/start/destroy -> services/{type}/{action}Service).
        let _ = match c.action.as_str() {
            "destroy-project" => req.send(Req::ProjectDestroy(c.project.clone())),
            "domain-delete" => req.send(Req::DomainDelete(c.project.clone())),
            // Hapus server: perubahan config, bukan panggilan API.
            "server-remove" => {
                self.server_action = Some(ServerAction::Remove(c.project));
                self.status = "Menghapus server...".into();
                return;
            }
            "maint:systemPrune" => req.send(Req::MaintAction("systemPrune")),
            "maint:cleanupDockerImages" => req.send(Req::MaintAction("cleanupDockerImages")),
            "maint:cleanupDockerBuilder" => req.send(Req::MaintAction("cleanupDockerBuilder")),
            action => req.send(Req::Action {
                project: c.project,
                service: c.service,
                stype: c.stype,
                action: action.to_string(),
            }),
        };
        self.status = "Mengirim...".into();
    }

    /// Buka daftar server (pilih / tambah / edit / hapus).
    ///
    /// Tidak boleh menolak saat cuma ada satu server: picker ini satu-satunya
    /// jalan menambah server dari TUI, jadi menolaknya membuat server kedua
    /// mustahil dibuat tanpa keluar ke CLI.
    fn open_picker(&mut self) {
        let cur = self
            .all_servers
            .iter()
            .position(|(n, _)| n == &self.server_name)
            .unwrap_or(0);
        let mut st = ListState::default();
        st.select(Some(cur));
        self.picker = Some(st);
    }

    fn picker_key(&mut self, code: KeyCode, _req: &Sender<Req>) {
        let Some(state) = self.picker.as_mut() else {
            return;
        };
        match code {
            KeyCode::Esc | KeyCode::Char('s') => self.picker = None,
            KeyCode::Char('n') => {
                self.picker = None;
                self.form = Some(Form::new(
                    FormKind::ServerAdd,
                    " Tambah server ",
                    vec![
                        Field::text("Nama", ""),
                        Field::text("URL", "https://"),
                        Field::secret("Token"),
                    ],
                ));
            }
            KeyCode::Char('e') => {
                if let Some((name, url)) = self.picker_selected() {
                    self.picker = None;
                    self.form = Some(Form::new(
                        FormKind::ServerEdit { name: name.clone() },
                        format!(" Edit server: {name} "),
                        vec![
                            Field::text("URL", &url),
                            // Token sengaja tak diisi ulang: menampilkannya kembali ke
                            // layar tak perlu. Kosong = pakai token yang tersimpan.
                            Field::secret("Token (kosong = tak diubah)"),
                        ],
                    ));
                }
            }
            KeyCode::Char('x') => {
                // Menghapus server ikut membuang tokennya, dan token tak bisa
                // dibaca balik dari mana pun — sekali salah tekan, kredensialnya
                // hilang. Setiap aksi destruktif lain di sini minta konfirmasi;
                // yang ini dulu tidak.
                if let Some((name, url)) = self.picker_selected() {
                    self.picker = None;
                    self.confirm = Some(Confirm {
                        action: "server-remove".into(),
                        project: name.clone(),
                        service: String::new(),
                        stype: String::new(),
                        label: format!(
                            "Hapus server '{name}' ({url})? Tokennya ikut hilang dan tak bisa dikembalikan."
                        ),
                    });
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let i = (state.selected().unwrap_or(0) + 1).min(self.all_servers.len() - 1);
                state.select(Some(i));
            }
            KeyCode::Up | KeyCode::Char('k') => {
                let i = state.selected().unwrap_or(0).saturating_sub(1);
                state.select(Some(i));
            }
            KeyCode::Enter => {
                if let Some((name, _)) = state
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

    fn services_key(&mut self, code: KeyCode, req: &Sender<Req>) {
        match code {
            KeyCode::Enter => self.open_view(View::Logs, req),
            KeyCode::Char('e') => self.open_view(View::Env, req),
            KeyCode::Char('p') => self.open_view(View::Ports, req),
            KeyCode::Char('m') => self.open_view(View::Mounts, req),
            KeyCode::Char('o') => self.open_view(View::Domains, req),
            KeyCode::Char('b') => self.open_view(View::Backups, req),
            KeyCode::Char('u') => self.open_view(View::Source, req),
            KeyCode::Char('U') => self.open_config_form(false, req),
            KeyCode::Char('B') => self.open_config_form(true, req),
            KeyCode::Char('E') => self.start_env_edit(),
            KeyCode::Char('n') => self.new_service_form(),
            KeyCode::Char('x') => self.ask_action("destroy"),
            // Panel Projects sudah tak ada, tapi project tetap harus bisa
            // dibuat/dihapus dari TUI.
            KeyCode::Char('N') => {
                self.form = Some(Form::new(
                    FormKind::ProjectCreate,
                    " Project baru ",
                    vec![Field::text("Nama", "")],
                ));
            }
            KeyCode::Char('X') => {
                if let Some((p, _, _)) = self.selected_row() {
                    self.confirm = Some(Confirm {
                        action: "destroy-project".into(),
                        project: p.clone(),
                        service: String::new(),
                        stype: String::new(),
                        label: format!("Hapus project '{p}' BESERTA SEMUA service di dalamnya?"),
                    });
                }
            }
            KeyCode::Char('d') => self.ask_action("deploy"),
            KeyCode::Char('R') => self.ask_action("restart"),
            KeyCode::Char('S') => self.ask_action("stop"),
            KeyCode::Char('T') => self.ask_action("start"),
            _ => {
                let n = self.visible_services().len();
                move_table(&mut self.services_table, code, n)
            }
        }
    }

    /// Form service baru. Project dipilih dari dropdown: daftar datar tak punya
    /// "project yang sedang dibuka", jadi ia harus disebut eksplisit.
    fn new_service_form(&mut self) {
        if self.projects.is_empty() {
            self.status = "Daftar project belum termuat".into();
            return;
        }
        let project = self
            .selected_row()
            .map(|(p, _, _)| p)
            .unwrap_or_else(|| self.projects[0].clone());
        self.form = Some(Form::new(
            FormKind::ServiceCreate,
            " Service baru ",
            vec![
                Field::choice_owned("Project", self.projects.clone(), &project),
                Field::text("Nama", ""),
                Field::choice("Tipe", SERVICE_TYPES, "app"),
            ],
        ));
    }

    fn viewer_key(&mut self, code: KeyCode) {
        match code {
            // Viewer dimasuki dari sebuah service, jadi Esc mengembalikan ke sana.
            KeyCode::Esc => self.screen = Screen::Projects,
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

    fn open_view(&mut self, view: View, req: &Sender<Req>) {
        if let Some((p, s, t)) = self.selected_row() {
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

    fn refresh(&mut self, req: &Sender<Req>) {
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
            Screen::Maintenance => {
                let _ = req.send(Req::MaintInfo);
            }
            Screen::Dashboard => {}
        }
        self.status = "Refresh...".into();
    }
}

// ---------- Keybinding (satu sumber untuk baris status dan overlay bantuan) ----------

/// Satu keybinding: tombol + artinya.
struct Key(&'static str, &'static str);

/// Tombol yang berlaku di layar mana pun.
const GLOBAL_KEYS: &[Key] = &[
    Key("1-7 / Tab", "pindah tab"),
    Key("?", "bantuan ini"),
    Key("s", "daftar server (pilih/tambah/edit/hapus)"),
    Key("r", "refresh"),
    Key("Esc", "batal: tutup form/dropdown/konfirmasi/filter"),
    Key("q / Ctrl-C", "keluar"),
];

/// Tombol khusus sebuah layar.
///
/// Baris status memakai beberapa entri PERTAMA dari daftar yang sama, jadi ia
/// tak bisa menyimpang dari bantuan: dua daftar terpisah pasti akan berbeda
/// seiring waktu, dan bantuan yang berbohong lebih buruk daripada tak ada.
fn screen_keys(screen: Screen) -> &'static [Key] {
    match screen {
        Screen::Dashboard => &[],
        Screen::Hosts => &[Key("↑↓", "pilih host")],
        Screen::Maintenance => &[
            Key("p", "prune sistem Docker"),
            Key("i", "hapus image tak terpakai"),
            Key("c", "hapus build cache"),
        ],
        Screen::Actions => &[
            Key("/", "cari"),
            Key("↑↓", "pilih"),
            Key("PgUp/PgDn", "lompat"),
        ],
        Screen::Monitor => &[
            Key("/", "cari"),
            Key("v", "ganti Services / Storage"),
            Key("↑↓", "pilih"),
        ],
        Screen::Domains => &[
            Key("/", "cari"),
            Key("n", "domain baru"),
            Key("e", "edit domain"),
            Key("x", "hapus domain"),
            Key("P", "jadikan primary"),
            Key("↑↓", "pilih"),
        ],
        Screen::Projects => &[
            Key("/", "cari service"),
            Key("Enter", "logs"),
            Key("n", "service baru"),
            Key("x", "hapus service"),
            Key("d", "deploy"),
            Key("R", "restart"),
            Key("S", "stop"),
            Key("T", "start"),
            Key("e", "lihat env"),
            Key("p", "lihat ports"),
            Key("m", "lihat mounts"),
            Key("o", "lihat domains"),
            Key("b", "lihat backups"),
            Key("u", "lihat source & build"),
            Key("E", "edit env di $EDITOR"),
            Key("U", "atur source (service app)"),
            Key("B", "atur build (service app)"),
            Key("N", "project baru"),
            Key("X", "hapus project"),
        ],
        Screen::Viewer => &[
            Key("↑↓", "scroll"),
            Key("PgUp/PgDn", "lompat"),
            Key("Esc", "kembali ke Services"),
        ],
    }
}

/// Tombol di dalam overlay; berlaku di form dan dropdown mana pun.
const OVERLAY_KEYS: &[Key] = &[
    Key("Tab / ↑↓", "pindah field"),
    Key("Enter", "simpan, atau buka dropdown pada field pilihan"),
    Key("Spasi", "toggle field ya/tidak"),
    Key("ketik", "saring isi dropdown"),
    Key("Esc", "batal"),
];

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
        Screen::Hosts => render_hosts(f, chunks[1], app),
        Screen::Maintenance => render_maintenance(f, chunks[1], app),
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
    if let Some(form) = &app.form {
        render_form(f, form);
    }
    if let Some(ch) = app.chooser.as_mut() {
        render_chooser(f, ch);
    }
    if app.help {
        render_help(f, app);
    }
}

/// Overlay bantuan: tombol global, tombol layar aktif, dan tombol di dalam form.
fn render_help(f: &mut Frame, app: &App) {
    let rows = screen_keys(app.screen);
    let area = centered(66, 92, f.area());
    f.render_widget(Clear, area);

    let head = |t: &str| {
        Line::from(Span::styled(
            format!(" {t}"),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ))
    };
    let row = |Key(k, d): &Key| {
        Line::from(vec![
            Span::styled(
                format!("   {k:<12}"),
                Style::default().fg(Color::Indexed(252)),
            ),
            Span::styled((*d).to_string(), Style::default().fg(Color::Gray)),
        ])
    };

    let mut lines = vec![head(&format!("{} — layar ini", TABS[app.screen.index()]))];
    if rows.is_empty() {
        lines.push(Line::from("   (tak ada tombol khusus)"));
    }
    lines.extend(rows.iter().map(row));
    lines.push(Line::from(""));
    lines.push(head("Di mana saja"));
    lines.extend(GLOBAL_KEYS.iter().map(row));
    lines.push(Line::from(""));
    lines.push(head("Di dalam form & dropdown"));
    lines.extend(OVERLAY_KEYS.iter().map(row));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "   tekan tombol apa saja untuk menutup",
        Style::default().fg(Color::DarkGray),
    )));

    f.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .title(" Bantuan ")
                .border_style(Style::default().fg(Color::Yellow)),
        ),
        area,
    );
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
    let rows: Vec<Vec<String>> = app
        .visible_services()
        .iter()
        .map(|s| app.service_row_full(s))
        .collect();
    let title = count_title("Services", rows.len(), app.all_services.len(), app);
    render_table(
        f,
        area,
        title,
        &SERVICE_HEADERS,
        &[
            Constraint::Length(18),
            Constraint::Length(22),
            Constraint::Length(8),
            Constraint::Length(7),
            Constraint::Min(16),
            Constraint::Length(8),
            Constraint::Length(10),
            Constraint::Length(11),
            Constraint::Length(11),
        ],
        rows,
        &mut app.services_table,
    );
}

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

/// Info server + pembersihan Docker. Aksinya destruktif dan tak bisa dibatalkan,
/// jadi tombolnya ditulis apa adanya beserta akibatnya, bukan disamarkan.
fn render_maintenance(f: &mut Frame, area: Rect, app: &App) {
    let mut lines: Vec<Line> = vec![
        Line::from(Span::styled(
            format!(" Server aktif: {}", app.server_name),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];
    if app.maint.is_empty() {
        lines.push(Line::from("  memuat…"));
    }
    for (k, v) in &app.maint {
        lines.push(Line::from(vec![
            Span::styled(format!("  {k:<24}"), Style::default().fg(Color::DarkGray)),
            Span::raw(v.clone()),
        ]));
    }
    lines.extend([
        Line::from(""),
        Line::from(Span::styled(
            "  Pembersihan (tak bisa dibatalkan, minta konfirmasi dulu)",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from("    [p] prune sistem — container, network, image, build cache tak terpakai"),
        Line::from("    [i] hapus image Docker tak terpakai"),
        Line::from("    [c] hapus build cache Docker"),
    ]);
    f.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(" Maintenance ")),
        area,
    );
}

/// Judul tabel: sebutkan filter yang sedang aktif beserta berapa yang tersaring.
/// Filter yang tak terlihat lebih buruk daripada tak ada filter — user akan
/// mengira baris yang hilang itu memang tak ada.
fn count_title(name: &str, shown: usize, total: usize, app: &App) -> String {
    if app.filter.is_empty() && !app.filter_input {
        return format!(" {name} ({total}) ");
    }
    let cursor = if app.filter_input { "▏" } else { "" };
    format!(" {name} ({shown}/{total})  /{}{cursor} ", app.filter)
}

/// Semua host sekaligus. Baris diwarnai per status karena inti layar ini adalah
/// menemukan host bermasalah sekilas — error yang tampil sewarna teks biasa
/// justru terlewat.
fn render_hosts(f: &mut Frame, area: Rect, app: &mut App) {
    let rows: Vec<Row> = app
        .hosts
        .iter()
        .map(|h| {
            let (cells, style) = match &h.state {
                HostState::Loading => (
                    vec![
                        h.name.clone(),
                        "memuat…".into(),
                        String::new(),
                        String::new(),
                        String::new(),
                        String::new(),
                        h.url.clone(),
                    ],
                    Style::default().fg(Color::DarkGray),
                ),
                HostState::Err(e) => (
                    vec![
                        h.name.clone(),
                        format!("MATI — {}", crate::output::first_line(e, 40)),
                        "-".into(),
                        "-".into(),
                        "-".into(),
                        "-".into(),
                        h.url.clone(),
                    ],
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                ),
                HostState::Ok(v) => {
                    let pair = |used: &str, total: &str| {
                        format!(
                            "{} / {}",
                            format_bytes(num(v, used)),
                            format_bytes(num(v, total))
                        )
                    };
                    let cpu = series_last(v, "cpu");
                    (
                        vec![
                            h.name.clone(),
                            "ok".into(),
                            format!("{cpu:.1}%"),
                            pair("/memoryUsedBytes", "/memoryTotalBytes"),
                            pair("/diskUsedBytes", "/diskTotalBytes"),
                            // loadAvg bukan deret berstempel-waktu seperti cpu/memory:
                            // isinya tiga string rata-rata 1/5/15 menit. series_last()
                            // mencari p[1] di tiap titik, tak menemukannya, lalu
                            // mengembalikan 0.00 — angka salah yang tampak meyakinkan.
                            commands::load_avg(v),
                            h.url.clone(),
                        ],
                        // Host sehat tak perlu menarik perhatian.
                        Style::default(),
                    )
                }
            };
            Row::new(cells).style(style)
        })
        .collect();

    let header = Row::new(vec![
        "Server", "Status", "CPU", "Memory", "Disk", "Load", "URL",
    ])
    .style(
        Style::default()
            .fg(Color::Black)
            .bg(Color::Green)
            .add_modifier(Modifier::BOLD),
    );
    let table = Table::new(
        rows,
        vec![
            Constraint::Length(14),
            Constraint::Min(16),
            Constraint::Length(7),
            Constraint::Length(19),
            Constraint::Length(19),
            Constraint::Length(18),
            Constraint::Length(30),
        ],
    )
    .header(header)
    .block(Block::bordered().title(format!(" Hosts ({}) ", app.hosts.len())))
    .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED))
    .highlight_symbol("› ");
    f.render_stateful_widget(table, area, &mut app.hosts_state);
}

fn render_actions(f: &mut Frame, area: Rect, app: &mut App) {
    let rows: Vec<Vec<String>> = app
        .visible_actions()
        .iter()
        .map(|a| commands::action_row(a, commands::ACTION_DESC_TUI))
        .collect();
    let title = count_title("Actions", rows.len(), app.actions.len(), app);
    render_table(
        f,
        area,
        title,
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
    let rows: Vec<Vec<String>> = app
        .visible_domains()
        .iter()
        .map(|d| commands::domain_row(d))
        .collect();
    let title = count_title("Domains", rows.len(), app.domains.len(), app);
    render_table(
        f,
        area,
        title,
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
            let data = app.visible_monitor_rows();
            let total = commands::monitor_rows(app.monitor.clone()).len();
            let title = format!(
                "{}· [v] Storage ",
                count_title("Services", data.len(), total, app)
            );
            render_table(
                f,
                rows[1],
                title,
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
    if app.filter_input {
        // Saat mengetik filter, tombol layar tak berlaku — jangan tampilkan yang
        // tidak akan bekerja.
        let bar = Style::default().bg(Color::Indexed(238)).fg(Color::White);
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(" filter: ", bar.fg(Color::Indexed(252))),
                Span::styled(format!("{}▏", app.filter), bar.add_modifier(Modifier::BOLD)),
                Span::styled("  Enter pakai · Esc batal", bar.fg(Color::Indexed(244))),
            ]))
            .style(bar),
            area,
        );
        return;
    }
    // Baris status = beberapa tombol pertama layar ini + "? bantuan" untuk
    // selebihnya. Diambil dari tabel yang sama dengan overlay bantuan.
    let mut parts: Vec<String> = screen_keys(app.screen)
        .iter()
        .take(6)
        .map(|Key(k, d)| format!("{k} {d}"))
        .collect();
    parts.push("? bantuan".into());
    parts.push("q keluar".into());
    let keys = parts.join(" · ");
    let keys = keys.as_str();

    // Warna bernama (Color::Blue) ditafsirkan tema terminal dan bisa jadi biru
    // terang, sehingga teks putih di atasnya nyaris tak terbaca. Indeks palet
    // memberi abu-abu gelap yang pasti, dengan status di-bold agar menonjol.
    let bar = Style::default().bg(Color::Indexed(238)).fg(Color::White);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(format!(" {keys} "), bar.fg(Color::Indexed(252))),
            Span::styled("│ ", bar.fg(Color::Indexed(244))),
            Span::styled(app.status.clone(), bar.add_modifier(Modifier::BOLD)),
        ]))
        .style(bar),
        area,
    );
}

fn render_form(f: &mut Frame, form: &Form) {
    let visible = form.visible();
    let height = (visible.len() as u16 + 5).min(f.area().height);
    let area = centered_abs(64, height, f.area());
    f.render_widget(Clear, area);
    f.render_widget(
        Block::bordered()
            .title(form.title.clone())
            .border_style(Style::default().fg(Color::Cyan)),
        area,
    );

    let inner = area.inner(Margin::new(2, 1));
    let mut rows = vec![Constraint::Length(1); visible.len()];
    rows.push(Constraint::Min(1));
    let slots = Layout::vertical(rows).split(inner);

    for (slot, &idx) in visible.iter().enumerate() {
        let field = &form.fields[idx];
        let focused = idx == form.focus;
        let hint = if focused && !field.kind.is_typed() {
            "  ⌄ Enter untuk pilih"
        } else {
            ""
        };
        let line = Line::from(vec![
            Span::styled(
                format!("{:<14}", field.label),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(
                format!("{}{}", field.shown(), if focused { "▏" } else { "" }),
                if focused {
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                },
            ),
            Span::styled(hint, Style::default().fg(Color::DarkGray)),
        ]);
        f.render_widget(Paragraph::new(line), slots[slot]);
    }

    f.render_widget(
        Paragraph::new("[Enter] pilih/simpan   [Tab] pindah field   [Esc] batal")
            .style(Style::default().fg(Color::DarkGray)),
        slots[visible.len()],
    );
}

fn render_chooser(f: &mut Frame, ch: &mut Chooser) {
    let items = ch.matches();
    let height = (items.len() as u16 + 4).clamp(5, 16);
    let area = centered_abs(48, height, f.area());
    f.render_widget(Clear, area);

    let title = if ch.filter.is_empty() {
        format!(" {} — ketik untuk mencari ", ch.label)
    } else {
        format!(" {} — cari: {} ", ch.label, ch.filter)
    };
    let list = List::new(items.into_iter().map(ListItem::new).collect::<Vec<_>>())
        .block(
            Block::bordered()
                .title(title)
                .border_style(Style::default().fg(Color::Cyan)),
        )
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("› ");
    f.render_stateful_widget(list, area, &mut ch.state);
}

fn render_confirm(f: &mut Frame, c: &Confirm) {
    let area = centered(52, 22, f.area());
    f.render_widget(Clear, area);
    // Sebutkan target sebenarnya. Kalimat "Memengaruhi service nyata" dulu
    // dipasang untuk semua konfirmasi — keliru untuk aksi maintenance, yang
    // justru mengenai seluruh host, bukan satu service.
    let target = match (c.project.as_str(), c.service.as_str()) {
        ("", _) => "Memengaruhi SELURUH host.".to_string(),
        (p, "") => format!("Target: {p}"),
        (p, s) => format!("Target: {p}/{s}"),
    };
    f.render_widget(
        Paragraph::new(format!(
            "\n{}\n\n{target}\n\n[y] Ya      [n] Batal",
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
        .map(|(n, url)| {
            let mark = if n == &app.server_name {
                " (aktif)"
            } else {
                ""
            };
            // URL ikut ditampilkan: nama saja tak cukup untuk memastikan host mana
            // yang akan diedit atau dihapus.
            ListItem::new(Line::from(vec![
                Span::raw(format!("{n}{mark}  ")),
                Span::styled(url.clone(), Style::default().fg(Color::DarkGray)),
            ]))
        })
        .collect();
    let list = List::new(items)
        .block(
            Block::bordered()
                .title(" Server: Enter pilih · n baru · e edit · x hapus ")
                .border_style(Style::default().fg(Color::Yellow)),
        )
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("› ");
    let state = app.picker.as_mut().unwrap();
    f.render_stateful_widget(list, area, state);
}

// ---------- Helpers ----------

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

/// Overlay dengan lebar persen dan tinggi baris tetap.
fn centered_abs(pct_x: u16, height: u16, r: Rect) -> Rect {
    let pad = r.height.saturating_sub(height) / 2;
    let v = Layout::vertical([
        Constraint::Length(pad),
        Constraint::Length(height),
        Constraint::Min(0),
    ])
    .split(r);
    Layout::horizontal([
        Constraint::Percentage((100 - pct_x) / 2),
        Constraint::Percentage(pct_x),
        Constraint::Percentage((100 - pct_x) / 2),
    ])
    .split(v[1])[1]
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

#[cfg(test)]
mod tests {
    use super::*;

    fn form(fields: Vec<Field>) -> Form {
        Form::new(FormKind::ProjectCreate, "t", fields).switch("Tipe")
    }

    fn f_val(f: &Form, label: &str) -> String {
        f.by_label(label)
    }

    #[test]
    fn source_without_config_does_not_default_to_first_repo() {
        // Service baru: Enter tanpa sadar tak boleh menunjuk source ke repo acak.
        // inspectService mengembalikan `source: null`, bukan field yang absen.
        let f = form(source_fields(
            Some(&Value::Null),
            vec!["caesario/Kuze".into(), "acme/web".into()],
        ));
        assert_eq!(f_val(&f, "Repo"), "");
        assert_eq!(source_body(&f).unwrap_err(), "Repo wajib dipilih");
    }

    #[test]
    fn domain_edit_keeps_middlewares_and_extra_servers() {
        // Middleware belum bisa diedit dari TUI, jadi HARUS ikut utuh. Begitu juga
        // server custom kedua dst., yang tak dimodelkan form.
        let original = json!({
            "id": "d1", "host": "a.test", "path": "/", "https": true,
            "wildcard": false, "certificateResolver": "google",
            "middlewares": ["mw1", "mw2"],
            "destinationType": "custom",
            "customDestination": { "servers": [
                { "url": "http://a:1", "weight": 1 },
                { "url": "http://b:2", "weight": 5 }
            ]}
        });
        let mut f = Form::new(
            FormKind::DomainEdit { id: "d1".into() },
            "t",
            domain_fields(Some(&original), &[]),
        );
        f.original = Some(original);
        let body = domain_body(&f).unwrap();
        assert_eq!(body["middlewares"], json!(["mw1", "mw2"]));
        assert_eq!(body["certificateResolver"], json!("google"));
        // Server kedua tak boleh terpangkas diam-diam.
        assert_eq!(
            body["customDestination"]["servers"][1],
            json!({ "url": "http://b:2", "weight": 5 })
        );
    }

    #[test]
    fn domain_ssl_resolver_and_wildcard_are_editable() {
        let original = json!({
            "id": "d1", "host": "a.test", "path": "/", "https": true,
            "wildcard": false, "certificateResolver": "", "middlewares": [],
            "destinationType": "service",
            "serviceDestination": { "projectName": "p", "serviceName": "s",
                                    "port": 80, "protocol": "http", "path": "/" }
        });
        let mut f = Form::new(
            FormKind::DomainEdit { id: "d1".into() },
            "t",
            domain_fields(Some(&original), &["p".into()]),
        );
        f.original = Some(original);
        f.fields
            .iter_mut()
            .find(|x| x.label == "SSL resolver")
            .unwrap()
            .value = "letsencrypt".into();
        f.fields
            .iter_mut()
            .find(|x| x.label == "Wildcard")
            .unwrap()
            .value = "ya".into();
        let body = domain_body(&f).unwrap();
        assert_eq!(body["certificateResolver"], json!("letsencrypt"));
        assert_eq!(body["wildcard"], json!(true));
    }

    fn svc(project: &str, name: &str, t: &str) -> Value {
        json!({ "projectName": project, "name": name, "type": t, "enabled": true })
    }

    #[test]
    fn service_row_summarises_source_without_opening_anything() {
        let github = json!({
            "projectName": "p", "name": "api", "type": "app", "enabled": true,
            "source": { "type": "github", "owner": "acme", "repo": "web", "ref": "dev" }
        });
        assert_eq!(service_row(&github)[4], "acme/web#dev");

        let image = json!({
            "projectName": "p", "name": "cache", "type": "redis", "enabled": false,
            "source": { "type": "image", "image": "redis:7" }
        });
        let row = service_row(&image);
        assert_eq!(row[4], "redis:7");
        assert_eq!(row[3], "mati");

        // Service tanpa source (baru dibuat) tak boleh bikin panik.
        assert_eq!(service_row(&svc("p", "kosong", "app"))[4], "-");
    }

    #[test]
    fn metric_cols_render_bytes_and_rates() {
        let m = json!({ "cpu": 0.257, "memory": 573857792.0,
                        "networkIn": 12540.9, "networkOut": 32653.2 });
        assert_eq!(
            metric_cols(Some(&m)),
            vec!["0.3 %", "547.3 MB", "12.2 KB/s", "31.9 KB/s"]
        );
        // Service tanpa metrik tak boleh bikin panik atau menampilkan 0 palsu.
        assert_eq!(metric_cols(None), vec!["-", "-", "-", "-"]);
    }

    #[test]
    fn metrics_join_by_project_and_service() {
        // getAllServicesStats memuat lebih banyak entri daripada daftar service
        // (service sistem, sub-service compose) — dan nama service yang sama bisa
        // ada di project berbeda, jadi kuncinya harus pasangan, bukan nama saja.
        let mut app = App::new("s".into(), vec![]);
        app.all_services = vec![svc("proj-a", "mysql", "mysql")];
        app.monitor = vec![
            json!({ "projectName": "proj-b", "serviceName": "mysql",
                    "cpu": 9.0, "memory": 1.0, "networkIn": 0.0, "networkOut": 0.0 }),
            json!({ "projectName": "proj-a", "serviceName": "mysql",
                    "cpu": 1.0, "memory": 2048.0, "networkIn": 0.0, "networkOut": 0.0 }),
        ];
        let row = app.service_row_full(&app.all_services[0]);
        // Harus mengambil proj-a, bukan proj-b yang namanya sama.
        assert_eq!(row[5], "1.0 %");
        assert_eq!(row[6], "2.0 KB");

        // Service yang tak punya metrik tetap tampil, kolomnya "-".
        app.all_services.push(svc("proj-c", "hantu", "app"));
        let row = app.service_row_full(&app.all_services[1]);
        assert_eq!(row[5], "-");
    }

    #[test]
    fn flat_list_filters_across_projects() {
        // Inti daftar datar: cari "mysql" menemukannya di project mana pun,
        // tanpa perlu tahu ia ada di project yang mana.
        let mut app = App::new("s".into(), vec![]);
        app.all_services = vec![
            svc("harisenin-net", "api", "app"),
            svc("harisenin-net-db", "mysql", "mysql"),
            svc("edukasistudio-db", "mysql-r1", "mysql"),
            svc("edukasistudio", "web", "app"),
        ];
        assert_eq!(app.visible_services().len(), 4);

        app.filter = "mysql".into();
        let vis = app.visible_services();
        assert_eq!(vis.len(), 2);
        assert_eq!(field(vis[0], "/projectName"), "harisenin-net-db");
        assert_eq!(field(vis[1], "/projectName"), "edukasistudio-db");

        // Nama project juga ikut dicocokkan, bukan cuma nama service.
        app.filter = "edukasistudio".into();
        assert_eq!(app.visible_services().len(), 2);
    }

    #[test]
    fn selected_row_follows_the_filtered_list() {
        // Kalau aksi memakai indeks daftar penuh, 'x' akan menghapus service lain.
        let mut app = App::new("s".into(), vec![]);
        app.all_services = vec![
            svc("p1", "satu", "app"),
            svc("p2", "dua", "app"),
            svc("p3", "tiga", "app"),
        ];
        app.filter = "tiga".into();
        app.services_table.select(Some(0));
        assert_eq!(
            app.selected_row(),
            Some(("p3".into(), "tiga".into(), "app".into()))
        );
    }

    /// Tiap layar yang punya tombol harus mendaftarkannya; layar yang lupa
    /// mendaftar akan tampil kosong di bantuan tanpa ada yang menyadarinya.
    #[test]
    fn every_interactive_screen_documents_its_keys() {
        for sc in [
            Screen::Hosts,
            Screen::Maintenance,
            Screen::Actions,
            Screen::Monitor,
            Screen::Domains,
            Screen::Projects,
            Screen::Viewer,
        ] {
            assert!(
                !screen_keys(sc).is_empty(),
                "{:?} tak punya keybinding terdaftar",
                TABS[sc.index()]
            );
        }
    }

    #[test]
    fn help_lists_the_destructive_keys_that_exist() {
        // Tombol destruktif paling perlu ditemukan sebelum ditekan, bukan sesudah.
        let projects: Vec<&str> = screen_keys(Screen::Projects).iter().map(|k| k.0).collect();
        for k in ["x", "X", "d", "R", "S", "T"] {
            assert!(
                projects.contains(&k),
                "'{k}' tak terdokumentasi di Services"
            );
        }
        let maint: Vec<&str> = screen_keys(Screen::Maintenance)
            .iter()
            .map(|k| k.0)
            .collect();
        for k in ["p", "i", "c"] {
            assert!(
                maint.contains(&k),
                "'{k}' tak terdokumentasi di Maintenance"
            );
        }
    }

    #[test]
    fn help_key_and_quit_key_are_documented_globally() {
        let g: Vec<&str> = GLOBAL_KEYS.iter().map(|k| k.0).collect();
        assert!(g.contains(&"?"));
        assert!(g.contains(&"q / Ctrl-C"));
        // Esc membatalkan, dan itu harus tertulis: sebelumnya Esc menutup TUI.
        assert!(g.contains(&"Esc"));
    }

    #[test]
    fn keep_matches_any_column_case_insensitively() {
        let row = vec![
            "https://Rezabelle.com/".to_string(),
            "http://proxy:80/".into(),
        ];
        assert!(keep(&row, ""));
        assert!(keep(&row, "rezabelle"));
        assert!(keep(&row, "PROXY"));
        assert!(!keep(&row, "tidakada"));
    }

    #[test]
    fn filter_narrows_domains_and_actions_use_the_same_list() {
        // Kalau render difilter tapi aksi memakai indeks daftar penuh, `x` akan
        // menghapus domain yang salah. Keduanya wajib lewat visible_domains().
        let mut app = App::new("s".into(), vec![]);
        app.domains = vec![
            json!({ "id": "a", "host": "satu.com", "https": true, "path": "/",
                    "destinationType": "service",
                    "serviceDestination": { "projectName": "p", "serviceName": "x",
                                            "port": 80, "protocol": "http", "path": "/" } }),
            json!({ "id": "b", "host": "dua.com", "https": true, "path": "/",
                    "destinationType": "service",
                    "serviceDestination": { "projectName": "p", "serviceName": "y",
                                            "port": 80, "protocol": "http", "path": "/" } }),
        ];
        assert_eq!(app.visible_domains().len(), 2);

        app.filter = "dua".into();
        let vis = app.visible_domains();
        assert_eq!(vis.len(), 1);
        // Indeks 0 dari daftar terfilter harus "dua.com" — bukan "satu.com".
        assert_eq!(vis[0]["id"], json!("b"));
    }

    #[test]
    fn clamp_keeps_selection_inside_filtered_list() {
        let mut app = App::new("s".into(), vec![]);
        app.screen = Screen::Domains;
        app.domains = vec![
            json!({ "id": "a", "host": "satu.com", "https": true, "path": "/" }),
            json!({ "id": "b", "host": "dua.com", "https": true, "path": "/" }),
        ];
        app.domains_state.select(Some(1));
        app.filter = "satu".into();
        app.clamp_filtered();
        // Hanya 1 baris tersisa; baris ke-1 sudah tak ada.
        assert_eq!(app.domains_state.selected(), Some(0));

        app.filter = "tidakadayangcocok".into();
        app.clamp_filtered();
        assert_eq!(app.domains_state.selected(), None);
    }

    #[test]
    fn source_github_sends_owner_and_repo_split() {
        let f = form(source_fields(
            Some(&json!({
                "type": "github", "owner": "acme", "repo": "web", "ref": "dev", "path": "/",
                "autoDeploy": true
            })),
            vec!["acme/web".into()],
        ));
        let (op, body, auto) = source_body(&f).unwrap();
        assert_eq!(op, "updateSourceGithub");
        assert_eq!(
            body,
            json!({ "owner": "acme", "repo": "web", "ref": "dev", "path": "/" })
        );
        // updateSourceGithub mereset autoDeploy jadi false di server; nilainya
        // harus ikut supaya bisa dipasang ulang setelahnya.
        assert_eq!(auto, Some(true));
    }

    #[test]
    fn source_git_and_image_have_no_auto_deploy() {
        // Hanya source github yang punya konsep auto deploy.
        let f = form(source_fields(
            Some(&json!({ "type": "image", "image": "nginx" })),
            vec![],
        ));
        assert_eq!(source_body(&f).unwrap().2, None);
    }

    #[test]
    fn source_rejects_path_without_leading_slash() {
        let mut f = form(source_fields(Some(&json!({ "type": "github" })), vec![]));
        f.fields
            .iter_mut()
            .find(|x| x.label == "Path")
            .unwrap()
            .value = "sub".into();
        assert!(source_body(&f).is_err());
    }

    #[test]
    fn source_image_omits_empty_credentials() {
        let f = form(source_fields(
            Some(&json!({ "type": "image", "image": "nginx:latest" })),
            vec![],
        ));
        let (op, body, _) = source_body(&f).unwrap();
        assert_eq!(op, "updateSourceImage");
        // Kirim "" akan menimpa kredensial registry jadi kosong.
        assert_eq!(body, json!({ "image": "nginx:latest" }));
    }

    #[test]
    fn build_keeps_unmodelled_version_on_same_type() {
        let original = json!({
            "type": "nixpacks", "installCommand": "npm ci", "nixpacksVersion": "1.41.0"
        });
        let mut f = form(build_fields(Some(&original)));
        f.original = Some(original);
        let body = build_body(&f).unwrap();
        // nixpacksVersion tak ada di form; hilang = build berubah diam-diam.
        assert_eq!(body["build"]["nixpacksVersion"], json!("1.41.0"));
        assert_eq!(body["build"]["installCommand"], json!("npm ci"));
    }

    #[test]
    fn build_drops_old_fields_when_type_changes() {
        let original = json!({
            "type": "nixpacks", "installCommand": "npm ci", "nixpacksVersion": "1.41.0"
        });
        let mut f = form(build_fields(Some(&original)));
        f.original = Some(original);
        f.fields
            .iter_mut()
            .find(|x| x.label == "Tipe")
            .unwrap()
            .value = "dockerfile".into();
        let body = build_body(&f).unwrap();
        assert_eq!(body["build"]["type"], json!("dockerfile"));
        assert_eq!(body["build"]["file"], json!("Dockerfile"));
        assert!(body["build"].get("nixpacksVersion").is_none());
        assert!(body["build"].get("installCommand").is_none());
    }

    #[test]
    fn build_removes_field_emptied_by_user() {
        let original = json!({ "type": "nixpacks", "installCommand": "npm ci" });
        let mut f = form(build_fields(Some(&original)));
        f.original = Some(original);
        f.fields
            .iter_mut()
            .find(|x| x.label == "Install command")
            .unwrap()
            .value
            .clear();
        let body = build_body(&f).unwrap();
        assert!(body["build"].get("installCommand").is_none());
    }

    #[test]
    fn set_options_keeps_current_value_missing_from_list() {
        // `ref` bisa berupa tag; searchBranches tak memuatnya. Melompat ke branch
        // pertama akan mengganti apa yang ter-deploy.
        let mut f = Field::choice_owned("Branch", vec!["v1.2.0".into()], "v1.2.0");
        f.set_options(vec!["main".into(), "dev".into()]);
        assert_eq!(f.value, "v1.2.0");
        match &f.kind {
            FieldKind::Choice(o) => assert_eq!(o[0], "v1.2.0"),
            _ => panic!("harus tetap Choice"),
        }
    }

    #[test]
    fn source_fields_keep_repo_absent_from_list() {
        // Repo yang dipakai tak ada di searchRepos (mis. hilang akses) -> jangan
        // diam-diam pindah ke repo pertama.
        let f = source_fields(
            Some(&json!({ "type": "github", "owner": "acme", "repo": "old", "ref": "dev" })),
            vec!["other/new".into()],
        );
        assert_eq!(
            f.iter().find(|x| x.label == "Repo").unwrap().value,
            "acme/old"
        );
    }

    #[test]
    fn visible_follows_switch_and_multi_tag() {
        let f = form(source_fields(Some(&json!({ "type": "github" })), vec![]));
        let shown =
            |f: &Form| -> Vec<&str> { f.visible().iter().map(|i| f.fields[*i].label).collect() };
        assert!(shown(&f).contains(&"Branch"));
        assert!(shown(&f).contains(&"Path")); // when("github,git")
        assert!(!shown(&f).contains(&"Image"));

        let mut f = f;
        f.fields
            .iter_mut()
            .find(|x| x.label == "Tipe")
            .unwrap()
            .value = "image".into();
        assert!(shown(&f).contains(&"Image"));
        assert!(!shown(&f).contains(&"Path"));
        assert!(!shown(&f).contains(&"Branch"));
    }
}
