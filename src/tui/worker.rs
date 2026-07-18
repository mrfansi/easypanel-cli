use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use anyhow::Result;
use serde_json::{json, Value};

use crate::client::EasypanelClient;
use crate::output::field;

// ---------- Worker (network di thread terpisah agar UI tak nge-freeze) ----------

#[derive(Clone, Copy, PartialEq)]
pub(super) enum View {
    Logs,
    Env,
    Ports,
    Mounts,
    Domains,
    Backups,
    Source,
}

impl View {
    pub(super) fn title(self) -> &'static str {
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

pub(super) enum Req {
    Stats,
    Nodes,
    Projects,
    Actions,
    MonitorData,
    /// Replika swarm per service (actual/desired) — ground truth "berjalan sesuai
    /// target?". Satu panggilan menutupi semua service; `actual < desired` = turun.
    TaskStats,
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
    /// Buka form limit resource: inspectService (grup services/{stype}) untuk
    /// nilai `resources` sekarang.
    ResourceForm {
        project: String,
        service: String,
        stype: String,
    },
    /// Simpan limit resource (updateResources). Grup ikut `stype` sebab resource
    /// ada di semua tipe service, bukan cuma app.
    ResourceSave {
        project: String,
        service: String,
        stype: String,
        resources: Value,
    },
    /// Info server untuk tab Maintenance (versi Docker, IP, ketersediaan update).
    MaintInfo,
    /// Pembersihan Docker: systemPrune / cleanupDockerImages / cleanupDockerBuilder.
    MaintAction(&'static str),
    /// Cari `query` di log SEMUA service sekaligus (fitur killer). Fan-out
    /// paralel; hasil digabung per service.
    LogSearch {
        query: String,
    },
    /// Ronde tail log berikutnya. `since` = timestamp terbaru yang sudah
    /// terlihat; None = batch pertama.
    LogTail {
        project: String,
        service: String,
        since: Option<String>,
    },
    /// Daftar repo GitHub untuk dropdown "Repo" di form "Service baru".
    ///
    /// Form source memakai ConfigForm, tapi itu butuh service yang SUDAH ada —
    /// form create belum punya satu pun.
    Repos,
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
    /// Nyalakan/matikan auto deploy tanpa menyentuh source.
    ///
    /// Terpisah dari ConfigSave: lewat sana berarti mengirim ulang
    /// updateSourceGithub, yang mereset autoDeploy jadi false lalu memasangnya
    /// kembali — dua panggilan dan satu jendela di mana nilainya salah, hanya
    /// untuk membalik sebuah bool.
    AutoDeploy {
        project: String,
        service: String,
        on: bool,
    },
    ProjectCreate(String),
    ProjectDestroy(String),
    ServiceCreate {
        project: String,
        service: String,
        stype: String,
        /// Field yang aman inline di createService: db (databaseName, user, …),
        /// build, env, dotEnvPath, domains. Semua ini cepat dan TAK memicu deploy.
        /// Hanya field yang diisi user yang ikut: kosong = server yang membuatkan.
        extra: Value,
        /// Source diterapkan TERPISAH setelah createService (updateSource*), sebab
        /// inline-nya memicu deploy 100 detik. (op, body, auto_deploy).
        source: Option<super::form::SourceCall>,
    },
    DomainSave {
        id: Option<String>,
        body: Value,
    },
    /// Tambah port (createPort) ke sebuah service.
    PortSave {
        project: String,
        service: String,
        values: Value,
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

pub(super) enum Resp {
    Stats(Value),
    Nodes(Vec<Value>),
    Projects(Vec<String>),
    Actions(Vec<Value>),
    MonitorData(Vec<Value>),
    /// (actual, desired) replika swarm, dikunci nama "{project}_{service}".
    TaskStats(HashMap<String, (i64, i64)>),
    Storage(Vec<Value>),
    Domains(Vec<Value>),
    /// Semua service lintas project + nama project untuk dropdown form.
    AllServices {
        projects: Vec<String>,
        services: Vec<Value>,
    },
    ServicesFor(String, Vec<String>),
    /// Data untuk membuka form limit resource: hasil inspectService.
    ResourceForm {
        project: String,
        service: String,
        stype: String,
        data: Value,
    },
    /// Data untuk membuka form source/build: hasil inspectService + daftar repo.
    ConfigForm {
        project: String,
        service: String,
        build: bool,
        data: Value,
        repos: Vec<String>,
    },
    /// Baris log yang lebih baru dari `since`, plus kursor untuk ronde berikutnya.
    LogTail {
        lines: Vec<String>,
        cursor: Option<String>,
    },
    /// Daftar kosong = GitHub tak tersambung; "Repo" tetap jadi input teks.
    Repos(Vec<String>),
    /// Err = daftar branch tak bisa dimuat (mis. token GitHub di EasyPanel mati).
    Branches(std::result::Result<Vec<String>, String>),
    MaintInfo(Vec<(String, String)>),
    /// Hasil satu host di layar Hosts; tiap host tiba sendiri-sendiri supaya
    /// host lambat/mati tak menahan yang lain.
    HostStat {
        name: String,
        data: std::result::Result<Value, String>,
    },
    Viewer(String, Vec<String>),
    /// Byte output dari sesi terminal container (diumpankan ke parser vt100).
    TermOutput(Vec<u8>),
    /// Sesi terminal berakhir (shell keluar / socket tutup).
    TermClosed,
    /// Mutasi berhasil: pesan status + data mana yang perlu dimuat ulang.
    Done(String, Refresh),
    Err(String),
}

/// Data yang perlu di-refresh setelah sebuah mutasi.
pub(super) enum Refresh {
    Projects,
    Domains,
    None,
}

/// Satu lajur worker: memproses request berurutan dan mengirim hasilnya ke `resp_tx`.
pub(super) fn spawn_worker(client: EasypanelClient, resp_tx: Sender<Resp>) -> Sender<Req> {
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
pub(super) struct Workers {
    pub(super) user: Sender<Req>,
    pub(super) poll: Sender<Req>,
    pub(super) resp: Receiver<Resp>,
    /// Untuk fan-out layar Hosts: tiap host dapat thread sendiri, jadi hasilnya
    /// tak lewat lajur user/poll yang terikat satu client.
    pub(super) resp_tx: Sender<Resp>,
}

pub(super) fn spawn_workers(client: EasypanelClient) -> Workers {
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

pub(super) fn handle_req(client: &EasypanelClient, req: Req) -> Resp {
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
        Req::TaskStats => match client.call("monitorOld", "getDockerTaskStats", Value::Null) {
            Ok(v) => Resp::TaskStats(parse_task_stats(&v)),
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
        Req::LogTail {
            project,
            service,
            since,
        } => {
            let mut input = json!({
                "projectName": project, "serviceName": service, "limit": 200
            });
            if let Some(ts) = &since {
                input["start"] = json!(crate::logs::after(ts));
            }
            match client.call("logs", "queryServiceLogs", input) {
                Ok(v) => Resp::LogTail {
                    lines: crate::logs::format(&v),
                    cursor: crate::logs::newest_ts(&v),
                },
                Err(e) => Resp::Err(e.to_string()),
            }
        }
        Req::LogSearch { query } => log_search(client, &query),
        Req::Repos => Resp::Repos(github_repos(client)),
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
        Req::ResourceForm {
            project,
            service,
            stype,
        } => {
            let ps = json!({ "projectName": project, "serviceName": service });
            match client.call(&format!("services/{stype}"), "inspectService", ps) {
                Ok(data) => Resp::ResourceForm {
                    project,
                    service,
                    stype,
                    data,
                },
                Err(e) => Resp::Err(e.to_string()),
            }
        }
        Req::ResourceSave {
            project,
            service,
            stype,
            resources,
        } => {
            let mut input = resources;
            input["projectName"] = json!(project);
            input["serviceName"] = json!(service);
            match client.call(&format!("services/{stype}"), "updateResources", input) {
                // Refresh::None: limit tak tampil di tabel; simpan konfigurasi saja,
                // deploy yang menerapkannya (sama seperti port).
                Ok(_) => Resp::Done(
                    format!("Resource {project}/{service} tersimpan — deploy (d) untuk menerapkan"),
                    Refresh::None,
                ),
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
                Ok(v) => Resp::Branches(Ok(v
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .map(String::from)
                            .collect()
                    })
                    .unwrap_or_default())),
                Err(e) => Resp::Branches(Err(e.to_string())),
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
                            // Refresh::Projects, bukan None: tanpa ini kolom Source
                            // di tabel tetap menampilkan branch/source lama sampai
                            // user menekan `r`. Persis kelas bug yang sama dengan
                            // service terhapus yang tak hilang dari tabel.
                            Ok(_) => Resp::Done("Tersimpan".into(), Refresh::Projects),
                            Err(e) => {
                                Resp::Err(format!("source tersimpan, auto deploy gagal: {e}"))
                            }
                        }
                    }
                    None => Resp::Done("Tersimpan".into(), Refresh::Projects),
                },
                Err(e) => Resp::Err(e.to_string()),
            }
        }
        Req::AutoDeploy {
            project,
            service,
            on,
        } => {
            let ep = if on {
                "enableGithubDeploy"
            } else {
                "disableGithubDeploy"
            };
            let ps = json!({ "projectName": project, "serviceName": service });
            match client.call("services/app", ep, ps) {
                Ok(_) => Resp::Done(
                    format!(
                        "Auto deploy {} untuk {service}",
                        if on { "aktif" } else { "mati" }
                    ),
                    Refresh::Projects,
                ),
                Err(e) => Resp::Err(auto_deploy_error(&service, &e.to_string())),
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
            extra,
            source,
        } => {
            let grp = format!("services/{stype}");
            let ps = json!({ "projectName": project, "serviceName": service });
            let mut input = extra;
            input["projectName"] = json!(project);
            input["serviceName"] = json!(service);
            // 1) Buat service. Tanpa source inline ini cepat (~0,2 detik) dan tak
            //    memicu deploy, jadi service langsung muncul di tabel.
            match client.call(&grp, "createService", input) {
                Ok(_) => {
                    // 2) Terapkan source terpisah (updateSource* + autoDeploy).
                    //    Menyimpan konfigurasi saja, tanpa men-deploy.
                    if let Some((op, mut body, auto)) = source {
                        body["projectName"] = json!(project);
                        body["serviceName"] = json!(service);
                        if let Err(e) = client.call(&grp, op, body) {
                            return Resp::Err(format!(
                                "Service '{service}' dibuat, tapi source gagal: {e}"
                            ));
                        }
                        if let Some(on) = auto {
                            let ep = if on {
                                "enableGithubDeploy"
                            } else {
                                "disableGithubDeploy"
                            };
                            let _ = client.call(&grp, ep, ps.clone());
                        }
                    }
                    // Sengaja TIDAK deploy: biar muncul dulu di tabel, lalu user
                    // menekan `d`. Deploy saat create-lah yang dulu bikin error.
                    Resp::Done(
                        format!("Service '{service}' dibuat — tekan d untuk deploy"),
                        Refresh::Projects,
                    )
                }
                Err(e) => Resp::Err(e.to_string()),
            }
        }
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
        Req::PortSave {
            project,
            service,
            values,
        } => {
            // Port tak tampil di tabel Services, jadi tak perlu refresh; user
            // membukanya lagi dengan `p` untuk memeriksa.
            match client.call(
                "ports",
                "createPort",
                json!({ "projectName": project, "serviceName": service, "values": values }),
            ) {
                Ok(_) => Resp::Done(
                    format!("Port ditambahkan ke {project}/{service}"),
                    Refresh::None,
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
            // Deploy DI-DISPATCH, tidak ditunggu. Build lamanya tak tentu — bisa
            // menit, tergantung repo — dan melebihi batas proxy mana pun (diukur:
            // 125 detik lalu 524 dari Cloudflare). Menunggunya = "error sending
            // request" padahal deploy jalan terus. Jadi picu di thread terpisah
            // dan langsung lapor dimulai; server menyelesaikan build sendiri
            // (drop koneksi tak membatalkannya — terbukti di createService).
            if action == "deploy" {
                let c = client.clone();
                let (grp, input) = (
                    format!("services/{stype}"),
                    json!({ "projectName": project, "serviceName": service, "forceRebuild": false }),
                );
                std::thread::spawn(move || {
                    let _ = c.call(&grp, "deployService", input);
                });
                return Resp::Done(
                    format!("Deploy {project}/{service} dimulai — pantau di Logs (Enter)"),
                    Refresh::None,
                );
            }
            let input = json!({ "projectName": project, "serviceName": service });
            match client.call(
                &format!("services/{stype}"),
                &format!("{action}Service"),
                input,
            ) {
                // Refresh, bukan sekadar Msg: destroy/start/stop sudah selesai di
                // server saat panggilan ini kembali (destroyService diukur 0,2-5
                // detik), tapi tabel tak pernah dimuat ulang — service yang sudah
                // dihapus tetap terpampang sampai user menekan `r`. Persis kelas
                // bug "service baru tak langsung muncul" yang dulu diperbaiki
                // untuk create dan terlewat untuk yang ini.
                Ok(_) => Resp::Done(
                    format!("{action} dipicu untuk {project}/{service}"),
                    Refresh::Projects,
                ),
                Err(e) => Resp::Err(e.to_string()),
            }
        }
    }
}

/// Cari `query` di log semua service sekaligus — fitur killer.
///
/// EasyPanel tak punya endpoint "cari lintas service"; kita fan-out
/// `queryServiceLogs` (yang menerima `search`, terverifikasi di server) ke tiap
/// service secara PARALEL. Satu thread per service dengan klien kloningan; log
/// didukung Loki, jadi pencarian dilakukan server-side, cepat. Hasil digabung,
/// dikelompokkan per service, hanya yang punya match.
fn log_search(client: &EasypanelClient, query: &str) -> Resp {
    if query.trim().is_empty() {
        return Resp::Err("Kata kunci pencarian kosong".into());
    }
    let all = match client.call("projects", "listProjectsAndServices", Value::Null) {
        Ok(v) => v,
        Err(e) => return Resp::Err(e.to_string()),
    };
    let services: Vec<(String, String)> = all
        .get("services")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|s| (field(s, "/projectName"), field(s, "/name")))
                .collect()
        })
        .unwrap_or_default();

    // Fan-out paralel: satu thread per service. reqwest blocking berbagi pool,
    // tapi Loki menjawab cepat, jadi puluhan service selesai dalam ~1-2 detik.
    let handles: Vec<_> = services
        .into_iter()
        .map(|(project, service)| {
            let c = client.clone();
            let q = query.to_string();
            thread::spawn(move || {
                let v = c
                    .call(
                        "logs",
                        "queryServiceLogs",
                        json!({
                            "projectName": project, "serviceName": service,
                            "search": q, "limit": 40
                        }),
                    )
                    .ok()?;
                let lines = crate::logs::format(&v);
                if lines.is_empty() {
                    None
                } else {
                    Some((project, service, lines))
                }
            })
        })
        .collect();

    let mut hits: Vec<(String, String, Vec<String>)> =
        handles.into_iter().filter_map(|h| h.join().ok()?).collect();
    hits.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));

    let total: usize = hits.iter().map(|(_, _, l)| l.len()).sum();
    let mut out = Vec::new();
    for (project, service, lines) in &hits {
        out.push(format!("── {project}/{service} ({}) ──", lines.len()));
        out.extend(lines.iter().cloned());
        out.push(String::new());
    }
    if out.is_empty() {
        out.push(format!(
            "Tak ada match untuk '{query}' di service mana pun."
        ));
    }
    Resp::Viewer(
        format!("Cari '{query}' — {total} baris di {} service", hits.len()),
        out,
    )
}

pub(super) fn fetch_view(
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
pub(super) fn github_repos(client: &EasypanelClient) -> Vec<String> {
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

/// Peta `{ "{project}_{service}": {actual, desired} }` dari getDockerTaskStats
/// jadi `swarm_name -> (actual, desired)`. Entri tanpa kedua angka diabaikan.
pub(super) fn parse_task_stats(v: &Value) -> HashMap<String, (i64, i64)> {
    v.as_object()
        .map(|o| {
            o.iter()
                .filter_map(|(k, t)| {
                    Some((
                        k.clone(),
                        (
                            t.get("actual").and_then(Value::as_i64)?,
                            t.get("desired").and_then(Value::as_i64)?,
                        ),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Nama+tipe service dari inspectProject, untuk dropdown Service di form domain.
pub(super) fn parse_services(v: &Value) -> Vec<(String, String)> {
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

pub(super) fn list_or_empty(
    v: &Value,
    empty: &str,
    f: impl Fn(usize, &Value) -> String,
) -> Vec<String> {
    let arr = v.as_array().cloned().unwrap_or_default();
    if arr.is_empty() {
        return vec![empty.to_string()];
    }
    arr.iter().enumerate().map(|(i, x)| f(i, x)).collect()
}

/// Ringkas error API jadi sebab yang bisa ditindaklanjuti.
///
/// EasyPanel membungkus error upstream, jadi token GitHub mati muncul sebagai
/// "[400] Request failed with status code 403 Forbidden" — dua kode status dan
/// nol petunjuk. Yang perlu user tahu adalah kredensialnya ditolak.
/// Pesan gagal auto deploy yang menyebut sebabnya, bukan tumpukan kode status.
///
/// enable/disableGithubDeploy membuat webhook GitHub, jadi ia gagal untuk repo
/// yang tak kita kuasai. EasyPanel meneruskannya sebagai 400 yang di dalamnya
/// ada 404 dari `GET /repos/{owner}/{repo}/hooks` — diamati langsung di server
/// pada sebuah service yang sumbernya repo pihak ketiga.
///
/// Yang tak dikenali dikembalikan apa adanya: pesan server yang panjang tetap
/// lebih berguna daripada "gagal", dan membuangnya adalah bug yang pernah
/// terjadi di proyek ini.
pub(super) fn auto_deploy_error(service: &str, raw: &str) -> String {
    if raw.contains("404") && raw.contains("/hooks") {
        format!("Auto deploy {service}: tak ada akses webhook ke repo itu — biasanya karena repo pihak ketiga")
    } else {
        format!("Auto deploy {service} gagal: {raw}")
    }
}
