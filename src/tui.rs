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
            app.handle(resp, &w.user);
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

        // Perubahan daftar server perlu ServerConfig, yang hanya ada di sini.
        if let Some(action) = app.server_action.take() {
            app.status = match apply_server_action(cfg, action) {
                Ok(msg) => msg,
                Err(e) => format!("Error: {e}"),
            };
            app.all_servers = cfg.all().into_iter().map(|s| s.name).collect();
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
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
    let status = std::process::Command::new(&editor).arg(&path).status();
    *terminal = ratatui::init();
    terminal.clear()?;
    status?;

    let edited = std::fs::read_to_string(&path)?;
    let _ = std::fs::remove_file(&path);

    Ok((edited.trim_end() != current.trim_end()).then_some(edited))
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
    /// Muat service sebuah project untuk dropdown di form (bukan panel Projects).
    ServicesFor(String),
    /// Buka form source/build: butuh inspectService (nilai sekarang) dan —
    /// untuk source — daftar repo GitHub buat dropdown-nya.
    ConfigForm {
        project: String,
        service: String,
        build: bool,
    },
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
    Services(String, Vec<(String, String)>),
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
    Viewer(String, Vec<String>),
    /// Mutasi berhasil: pesan status + data mana yang perlu dimuat ulang.
    Done(String, Refresh),
    Msg(String),
    Err(String),
}

/// Data yang perlu di-refresh setelah sebuah mutasi.
enum Refresh {
    Projects,
    Services(String),
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
                Refresh::Services(project),
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

    vec![
        Field::text("Host", &get("/host", "")),
        Field::text("Path", &get("/path", "/")),
        Field::boolean("HTTPS", https),
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
    ServerEdit { name: String },
    ProjectCreate,
    ServiceCreate { project: String },
    DomainCreate,
    DomainEdit { id: String },
    SourceEdit { project: String, service: String },
    BuildEdit { project: String, service: String },
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
    /// tak ada di form (middlewares, certificateResolver, wildcard) ikut utuh.
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
        token: String,
    },
    Remove(String),
}

struct App {
    server_name: String,
    all_servers: Vec<String>,
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
                        self.projects.clear();
                        let _ = req.send(Req::Projects);
                    }
                    Refresh::Services(p) => {
                        if self.current_project.as_deref() == Some(p.as_str()) {
                            let _ = req.send(Req::Services(p));
                        }
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
                Screen::Domains => self.domains_key(code, req),
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

    fn picker_selected(&self) -> Option<String> {
        self.picker
            .as_ref()
            .and_then(|s| s.selected())
            .and_then(|i| self.all_servers.get(i).cloned())
    }

    /// 'n' di layar Projects: buat project (fokus kiri) atau service (fokus kanan).
    fn new_from_projects(&mut self) {
        self.form = Some(match self.focus {
            Focus::Projects => Form::new(
                FormKind::ProjectCreate,
                " Project baru ",
                vec![Field::text("Nama", "")],
            ),
            Focus::Services => {
                let Some(project) = self.current_project.clone() else {
                    self.status = "Pilih project dulu".into();
                    return;
                };
                Form::new(
                    FormKind::ServiceCreate {
                        project: project.clone(),
                    },
                    format!(" Service baru di {project} "),
                    vec![
                        Field::text("Nama", ""),
                        Field::choice("Tipe", SERVICE_TYPES, "app"),
                    ],
                )
            }
        });
    }

    /// 'x' di layar Projects: hapus project atau service yang sedang dipilih.
    fn destroy_from_projects(&mut self, _req: &Sender<Req>) {
        match self.focus {
            Focus::Projects => {
                if let Some(p) = self
                    .projects_state
                    .selected()
                    .and_then(|i| self.projects.get(i).cloned())
                {
                    self.confirm = Some(Confirm {
                        action: "destroy-project".into(),
                        project: p.clone(),
                        service: String::new(),
                        stype: String::new(),
                        label: format!("Hapus project '{p}' beserta semua service-nya?"),
                    });
                }
            }
            Focus::Services => self.ask_action("destroy"),
        }
    }

    fn start_env_edit(&mut self) {
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
            self.edit_env = Some((p, s, t));
        }
    }

    fn domains_key(&mut self, code: KeyCode, req: &Sender<Req>) {
        let selected = self
            .domains_state
            .selected()
            .and_then(|i| self.domains.get(i).cloned());

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
            _ => move_table(&mut self.domains_state, code, self.domains.len()),
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
        if self.focus != Focus::Services {
            self.status = "Fokus panel Services dulu (→)".into();
            return;
        }
        let (Some(project), Some((service, stype))) = (
            self.current_project.clone(),
            self.selected_service().cloned(),
        ) else {
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

    /// Body createDomain/updateDomain dari form.
    ///
    /// Saat edit, berangkat dari JSON domain aslinya sehingga field yang tak ada
    /// di form (middlewares, certificateResolver, wildcard) tetap utuh — bukan
    /// ditimpa nilai default.
    fn domain_body(&self, form: &Form) -> std::result::Result<Value, String> {
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

    fn submit_form(&mut self, req: &Sender<Req>) {
        let Some(form) = self.form.as_ref() else {
            return;
        };

        // Validasi minimal di sini; sisanya biar server yang menolak.
        match &form.kind {
            FormKind::ServerAdd | FormKind::ServerEdit { .. } => {
                let (name, url, token) = match &form.kind {
                    FormKind::ServerAdd => (form.val(0), form.val(1), form.val(2)),
                    FormKind::ServerEdit { name } => (name.clone(), form.val(0), form.val(1)),
                    _ => unreachable!(),
                };
                if name.is_empty() || url.is_empty() || token.is_empty() {
                    self.status = "Nama, URL, dan token wajib diisi".into();
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
            FormKind::ServiceCreate { project } => {
                let (service, stype) = (form.val(0), form.val(1));
                if !commands::valid_name(&service) || stype.is_empty() {
                    self.status = "Nama service hanya boleh a-z, 0-9, -, _".into();
                    return;
                }
                let _ = req.send(Req::ServiceCreate {
                    project: project.clone(),
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
            FormKind::DomainCreate | FormKind::DomainEdit { .. } => match self.domain_body(form) {
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
            action => req.send(Req::Action {
                project: c.project,
                service: c.service,
                stype: c.stype,
                action: action.to_string(),
            }),
        };
        self.status = "Mengirim...".into();
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
                if let Some(name) = self.picker_selected() {
                    self.picker = None;
                    self.form = Some(Form::new(
                        FormKind::ServerEdit { name: name.clone() },
                        format!(" Edit server: {name} "),
                        vec![Field::text("URL", ""), Field::secret("Token")],
                    ));
                }
            }
            KeyCode::Char('x') => {
                if let Some(name) = self.picker_selected() {
                    self.picker = None;
                    self.server_action = Some(ServerAction::Remove(name));
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
            KeyCode::Char('u') => self.open_view(View::Source, req),
            KeyCode::Char('U') => self.open_config_form(false, req),
            KeyCode::Char('B') => self.open_config_form(true, req),
            KeyCode::Char('n') => self.new_from_projects(),
            KeyCode::Char('x') => self.destroy_from_projects(req),
            KeyCode::Char('E') => self.start_env_edit(),
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
    if let Some(form) = &app.form {
        render_form(f, form);
    }
    if let Some(ch) = app.chooser.as_mut() {
        render_chooser(f, ch);
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
        Screen::Domains => "n baru · e edit · x hapus · P primary · ↑↓ pilih · r refresh · q keluar",
        Screen::Projects => {
            "n baru · x hapus · E env · U source · B build · e/p/m/o/b/u view · d/R/S/T aksi · q keluar"
        }
        Screen::Viewer => "↑↓ scroll · PgUp/PgDn · r refresh · 1-6 tab · q keluar",
    };
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
                .title(" Server: Enter pilih · n baru · e edit · x hapus ")
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
