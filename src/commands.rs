use anyhow::{anyhow, Result};
use dialoguer::{Confirm, Input, Password};
use serde_json::{json, Value};
use std::io::Read;

use crate::client::EasypanelClient;
use crate::config::ServerConfig;
use crate::logs;
use crate::output::{
    age_of, duration_between, field, first_line, format_bytes, format_rate, num, series_last,
    table, yes_no,
};

/// Resolve klien untuk server aktif (dari --server atau default).
pub fn resolve_client(cfg: &ServerConfig, server: &Option<String>) -> Result<EasypanelClient> {
    let s = match server {
        Some(name) => cfg.get(name).ok_or_else(|| {
            anyhow!(
                "Server '{}' tidak ditemukan. Lihat: easypanel server list",
                name
            )
        })?,
        None => cfg
            .default()
            .ok_or_else(|| anyhow!("Belum ada server default. Jalankan: easypanel server add"))?,
    };
    Ok(EasypanelClient::new(&s.url, &s.token))
}

pub fn valid_name(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

pub fn ucfirst(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

// ---------- Server ----------

pub fn server_add(
    cfg: &ServerConfig,
    name: Option<String>,
    url: Option<String>,
    token: Option<String>,
) -> Result<()> {
    let name = match name {
        Some(n) => n,
        None => Input::new().with_prompt("Nama server").interact_text()?,
    };
    if !valid_name(&name) {
        return Err(anyhow!("Nama server hanya boleh a-z, 0-9, -, _"));
    }
    let url = match url {
        Some(u) => u,
        None => Input::new()
            .with_prompt("URL host (mis. https://panel.example.com)")
            .interact_text()?,
    };
    let token = match token {
        Some(t) => t,
        None => Password::new().with_prompt("API token").interact()?,
    };

    let url = url.trim_end_matches('/').to_string();
    cfg.add(&name, &url, &token)?;

    let is_default = cfg.default().map(|s| s.name == name).unwrap_or(false);
    println!(
        "Server '{}' ditambahkan.{}",
        name,
        if is_default { " (default)" } else { "" }
    );
    Ok(())
}

pub fn server_list(cfg: &ServerConfig) -> Result<()> {
    let servers = cfg.all();
    if servers.is_empty() {
        println!("Belum ada server. Jalankan: easypanel server add");
        return Ok(());
    }

    let rows = servers
        .iter()
        .map(|s| {
            vec![
                if s.default { "*".into() } else { String::new() },
                s.name.clone(),
                s.url.clone(),
                mask_token(&s.token),
            ]
        })
        .collect();
    table(&["Default", "Nama", "URL", "Token"], rows);
    Ok(())
}

pub fn server_use(cfg: &ServerConfig, name: &str) -> Result<()> {
    if cfg.get(name).is_none() {
        return Err(anyhow!("Server '{}' tidak ditemukan.", name));
    }
    cfg.set_default(name)?;
    println!("Server default sekarang: {name}");
    Ok(())
}

pub fn server_remove(cfg: &ServerConfig, name: &str) -> Result<()> {
    if cfg.get(name).is_none() {
        return Err(anyhow!("Server '{}' tidak ditemukan.", name));
    }
    cfg.remove(name)?;
    println!("Server '{name}' dihapus.");
    Ok(())
}

fn mask_token(token: &str) -> String {
    if token.len() <= 10 {
        "***".to_string()
    } else {
        format!("{}…{}", &token[..6], &token[token.len() - 4..])
    }
}

// ---------- Projects ----------

pub fn project_list(client: &EasypanelClient) -> Result<()> {
    let projects = client.call("projects", "listProjects", Value::Null)?;
    let arr = projects.as_array().cloned().unwrap_or_default();
    if arr.is_empty() {
        println!("Tidak ada project.");
        return Ok(());
    }

    let rows = arr
        .iter()
        .map(|p| {
            vec![
                field(p, "/name"),
                field(p, "/createdAt"),
                p.get("members")
                    .and_then(Value::as_array)
                    .map(|m| m.len())
                    .unwrap_or(0)
                    .to_string(),
            ]
        })
        .collect();
    table(&["Nama", "Dibuat", "Members"], rows);
    Ok(())
}

pub fn project_create(client: &EasypanelClient, name: &str) -> Result<()> {
    if !valid_name(name) {
        return Err(anyhow!("Nama project hanya boleh a-z, 0-9, -, _"));
    }
    client.call("projects", "createProject", json!({ "name": name }))?;
    println!("Project '{name}' dibuat.");
    Ok(())
}

pub fn project_inspect(client: &EasypanelClient, name: &str) -> Result<()> {
    let data = client.call("projects", "inspectProject", json!({ "projectName": name }))?;
    let services = data
        .get("services")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    if services.is_empty() {
        println!("Project tanpa service.");
        return Ok(());
    }

    let rows = services
        .iter()
        .map(|s| {
            vec![
                field(s, "/name"),
                field(s, "/type"),
                if s.get("enabled").and_then(Value::as_bool).unwrap_or(false) {
                    "ya".into()
                } else {
                    "tidak".into()
                },
            ]
        })
        .collect();
    table(&["Service", "Tipe", "Aktif"], rows);
    Ok(())
}

// ---------- Services ----------

pub fn service_action(
    client: &EasypanelClient,
    project: &str,
    service: &str,
    stype: &str,
    action: &str,
    force: bool,
) -> Result<()> {
    let mut input = json!({ "projectName": project, "serviceName": service });
    if action == "deploy" {
        input["forceRebuild"] = json!(force);
    }
    client.call(
        &format!("services/{stype}"),
        &format!("{action}Service"),
        input,
    )?;
    println!("{} dipicu untuk {}/{}.", ucfirst(action), project, service);
    Ok(())
}

pub fn service_logs(
    client: &EasypanelClient,
    project: &str,
    service: &str,
    limit: u32,
) -> Result<()> {
    let result = client.call(
        "logs",
        "queryServiceLogs",
        json!({ "projectName": project, "serviceName": service, "limit": limit }),
    )?;

    let lines = logs::format(&result);
    if lines.is_empty() {
        println!("Tidak ada log.");
        return Ok(());
    }
    for line in lines {
        println!("{line}");
    }
    Ok(())
}

// ---------- Monitoring & Cluster ----------

/// Rangkuman metrik host dari grup `metrics` (Prometheus): ~0,3 detik dan sudah
/// berisi laju network + total/used byte, tak seperti `monitorOld` (~2,3 detik).
pub fn stats(client: &EasypanelClient) -> Result<()> {
    let s = client.call("metrics", "getSystemStats", json!({}))?;
    table(&["Metrik", "Nilai"], stats_rows(&s));
    Ok(())
}

pub fn load_avg(s: &Value) -> String {
    s.get("loadAvg")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|v| v.as_str().unwrap_or("-").to_string())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_else(|| "-".to_string())
}

pub fn stats_rows(s: &Value) -> Vec<Vec<String>> {
    let pair = |pct: f64, used: &str, total: &str| {
        format!(
            "{pct:.1} % ({} / {})",
            format_bytes(num(s, used)),
            format_bytes(num(s, total))
        )
    };
    vec![
        vec!["CPU".into(), format!("{:.1} %", series_last(s, "cpu"))],
        vec!["Cores".into(), field(s, "/cpuCores")],
        vec!["Load avg".into(), load_avg(s)],
        vec![
            "Memory".into(),
            pair(
                series_last(s, "memory"),
                "/memoryUsedBytes",
                "/memoryTotalBytes",
            ),
        ],
        vec![
            "Disk".into(),
            pair(series_last(s, "disk"), "/diskUsedBytes", "/diskTotalBytes"),
        ],
        vec![
            "Network In".into(),
            format_rate(series_last(s, "networkIn")),
        ],
        vec![
            "Network Out".into(),
            format_rate(series_last(s, "networkOut")),
        ],
    ]
}

pub fn node_list(client: &EasypanelClient) -> Result<()> {
    let nodes = client.call("cluster", "listNodes", Value::Null)?;
    let arr = nodes.as_array().cloned().unwrap_or_default();
    if arr.is_empty() {
        println!("Tidak ada node (atau host bukan cluster).");
        return Ok(());
    }

    let rows = arr
        .iter()
        .map(|n| {
            vec![
                field(n, "/Description/Hostname"),
                field(n, "/Spec/Role"),
                field(n, "/Status/State"),
                field(n, "/Spec/Availability"),
                field(n, "/Status/Addr"),
            ]
        })
        .collect();
    table(&["Hostname", "Role", "State", "Availability", "Addr"], rows);
    Ok(())
}

// ---------- Env ----------

pub fn service_env(
    client: &EasypanelClient,
    project: &str,
    service: &str,
    stype: &str,
) -> Result<()> {
    let svc = client.call(
        &format!("services/{stype}"),
        "inspectService",
        json!({ "projectName": project, "serviceName": service }),
    )?;
    let env = svc.get("env").and_then(Value::as_str).unwrap_or("");
    print!("{env}");
    if !env.ends_with('\n') {
        println!();
    }
    Ok(())
}

pub fn service_set_env(
    client: &EasypanelClient,
    project: &str,
    service: &str,
    stype: &str,
    file: Option<String>,
) -> Result<()> {
    let env = match file {
        Some(path) => std::fs::read_to_string(path)?,
        None => {
            let mut buf = String::new();
            std::io::stdin().read_to_string(&mut buf)?;
            buf
        }
    };
    client.call(
        &format!("services/{stype}"),
        "updateEnv",
        json!({ "projectName": project, "serviceName": service, "env": env }),
    )?;
    println!("Env untuk {project}/{service} diperbarui.");
    Ok(())
}

// ---------- Ports (grup "ports") ----------

pub fn ports_list(client: &EasypanelClient, project: &str, service: &str) -> Result<()> {
    let ports = client.call(
        "ports",
        "listPorts",
        json!({ "projectName": project, "serviceName": service }),
    )?;
    let arr = ports.as_array().cloned().unwrap_or_default();
    if arr.is_empty() {
        println!("Tidak ada port ter-expose.");
        return Ok(());
    }
    let rows = arr
        .iter()
        .enumerate()
        .map(|(i, p)| {
            vec![
                i.to_string(),
                field(p, "/protocol"),
                field(p, "/published"),
                field(p, "/target"),
            ]
        })
        .collect();
    table(&["Index", "Protokol", "Published", "Target"], rows);
    Ok(())
}

pub fn port_add(
    client: &EasypanelClient,
    project: &str,
    service: &str,
    published: u32,
    target: u32,
    protocol: &str,
) -> Result<()> {
    client.call(
        "ports",
        "createPort",
        json!({
            "projectName": project,
            "serviceName": service,
            "values": { "published": published, "target": target, "protocol": protocol }
        }),
    )?;
    println!("Port {published}->{target}/{protocol} ditambahkan ke {project}/{service}.");
    Ok(())
}

pub fn port_remove(
    client: &EasypanelClient,
    project: &str,
    service: &str,
    index: u32,
) -> Result<()> {
    client.call(
        "ports",
        "deletePort",
        json!({ "projectName": project, "serviceName": service, "index": index }),
    )?;
    println!("Port index {index} dihapus dari {project}/{service}.");
    Ok(())
}

// ---------- Mounts (grup "mounts") ----------

pub fn mounts_list(client: &EasypanelClient, project: &str, service: &str) -> Result<()> {
    let mounts = client.call(
        "mounts",
        "listMounts",
        json!({ "projectName": project, "serviceName": service }),
    )?;
    let arr = mounts.as_array().cloned().unwrap_or_default();
    if arr.is_empty() {
        println!("Tidak ada mount.");
        return Ok(());
    }
    let rows = arr
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let detail = match field(m, "/type").as_str() {
                "bind" => format!("{} -> {}", field(m, "/hostPath"), field(m, "/mountPath")),
                "volume" => format!("{} -> {}", field(m, "/name"), field(m, "/mountPath")),
                _ => field(m, "/mountPath"),
            };
            vec![i.to_string(), field(m, "/type"), detail]
        })
        .collect();
    table(&["Index", "Tipe", "Detail"], rows);
    Ok(())
}

pub fn mount_add(
    client: &EasypanelClient,
    project: &str,
    service: &str,
    kind: &str,
    mount_path: &str,
    name: Option<String>,
    host_path: Option<String>,
) -> Result<()> {
    let values = match kind {
        "volume" => json!({
            "type": "volume",
            "name": name.ok_or_else(|| anyhow!("--name wajib untuk mount tipe volume"))?,
            "mountPath": mount_path
        }),
        "bind" => json!({
            "type": "bind",
            "hostPath": host_path.ok_or_else(|| anyhow!("--host-path wajib untuk mount tipe bind"))?,
            "mountPath": mount_path
        }),
        other => {
            return Err(anyhow!(
                "Tipe mount tidak didukung: {other} (pakai volume|bind)"
            ))
        }
    };
    client.call(
        "mounts",
        "createMount",
        json!({ "projectName": project, "serviceName": service, "values": values }),
    )?;
    println!("Mount {kind} ditambahkan ke {project}/{service}.");
    Ok(())
}

pub fn mount_remove(
    client: &EasypanelClient,
    project: &str,
    service: &str,
    index: u32,
) -> Result<()> {
    client.call(
        "mounts",
        "deleteMount",
        json!({ "projectName": project, "serviceName": service, "index": index }),
    )?;
    println!("Mount index {index} dihapus dari {project}/{service}.");
    Ok(())
}

// ---------- Domains (grup "domains") ----------

pub fn domains_list(client: &EasypanelClient, project: &str, service: &str) -> Result<()> {
    let domains = client.call(
        "domains",
        "listDomains",
        json!({ "projectName": project, "serviceName": service }),
    )?;
    let arr = domains.as_array().cloned().unwrap_or_default();
    if arr.is_empty() {
        println!("Tidak ada domain.");
        return Ok(());
    }
    let rows = arr
        .iter()
        .map(|dm| {
            vec![
                field(dm, "/id"),
                field(dm, "/host"),
                if dm.get("https").and_then(Value::as_bool).unwrap_or(false) {
                    "ya".into()
                } else {
                    "tidak".into()
                },
                field(dm, "/path"),
                field(dm, "/serviceDestination/port"),
            ]
        })
        .collect();
    table(&["ID", "Host", "HTTPS", "Path", "Port"], rows);
    Ok(())
}

pub fn domain_delete(client: &EasypanelClient, id: &str) -> Result<()> {
    client.call("domains", "deleteDomain", json!({ "id": id }))?;
    println!("Domain {id} dihapus.");
    Ok(())
}

pub fn domain_set_primary(client: &EasypanelClient, id: &str) -> Result<()> {
    client.call("domains", "setPrimaryDomain", json!({ "id": id }))?;
    println!("Domain {id} dijadikan primary.");
    Ok(())
}

// ---------- Lifecycle ----------

pub fn service_create(
    client: &EasypanelClient,
    project: &str,
    service: &str,
    stype: &str,
) -> Result<()> {
    if !valid_name(service) {
        return Err(anyhow!("Nama service hanya boleh a-z, 0-9, -, _"));
    }
    client.call(
        &format!("services/{stype}"),
        "createService",
        json!({ "projectName": project, "serviceName": service }),
    )?;
    println!("Service {service} ({stype}) dibuat di {project}.");
    Ok(())
}

pub fn service_destroy(
    client: &EasypanelClient,
    project: &str,
    service: &str,
    stype: &str,
    yes: bool,
) -> Result<()> {
    if !confirm(
        &format!("Hapus service '{service}' pada '{project}'? Tidak bisa dibatalkan."),
        yes,
    )? {
        return Ok(());
    }
    client.call(
        &format!("services/{stype}"),
        "destroyService",
        json!({ "projectName": project, "serviceName": service }),
    )?;
    println!("Service {project}/{service} dihapus.");
    Ok(())
}

pub fn project_destroy(client: &EasypanelClient, name: &str, yes: bool) -> Result<()> {
    if !confirm(
        &format!("Hapus project '{name}' beserta semua service-nya? Tidak bisa dibatalkan."),
        yes,
    )? {
        return Ok(());
    }
    client.call("projects", "destroyProject", json!({ "name": name }))?;
    println!("Project {name} dihapus.");
    Ok(())
}

fn confirm(prompt: &str, yes: bool) -> Result<bool> {
    if yes {
        return Ok(true);
    }
    Ok(Confirm::new()
        .with_prompt(prompt)
        .default(false)
        .interact()?)
}

// ---------- Certificates ----------

pub fn certificate_list(client: &EasypanelClient) -> Result<()> {
    let certs = client.call("certificates", "listCertificates", Value::Null)?;
    let arr = certs.as_array().cloned().unwrap_or_default();
    if arr.is_empty() {
        println!("Tidak ada certificate.");
        return Ok(());
    }
    let rows = arr.iter().map(|c| vec![field(c, "/domain/main")]).collect();
    table(&["Domain"], rows);
    Ok(())
}

pub fn certificate_remove(client: &EasypanelClient, domain: &str) -> Result<()> {
    client.call(
        "certificates",
        "removeCertificate",
        json!({ "domain": domain }),
    )?;
    println!("Certificate untuk {domain} dihapus.");
    Ok(())
}

// ---------- Notifications ----------

pub fn notification_list(client: &EasypanelClient) -> Result<()> {
    let res = client.call("notifications", "listNotificationChannels", Value::Null)?;
    let arr = res
        .get("notificationChannels")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if arr.is_empty() {
        println!("Tidak ada notification channel.");
        return Ok(());
    }
    let rows = arr
        .iter()
        .map(|c| vec![field(c, "/id"), field(c, "/name")])
        .collect();
    table(&["ID", "Nama"], rows);
    Ok(())
}

pub fn notification_delete(client: &EasypanelClient, id: &str) -> Result<()> {
    client.call(
        "notifications",
        "destroyNotificationChannel",
        json!({ "id": id }),
    )?;
    println!("Notification channel {id} dihapus.");
    Ok(())
}

// ---------- Databases & Backups ----------

pub fn service_databases(client: &EasypanelClient, project: &str, service: &str) -> Result<()> {
    let dbs = client.call(
        "databaseBackups",
        "getServiceDatabases",
        json!({ "projectName": project, "serviceName": service }),
    )?;
    let arr = dbs.as_array().cloned().unwrap_or_default();
    if arr.is_empty() {
        println!("Tidak ada database.");
        return Ok(());
    }
    for db in arr {
        if let Some(name) = db.as_str() {
            println!("{name}");
        }
    }
    Ok(())
}

pub fn db_backup_list(client: &EasypanelClient, project: &str, service: &str) -> Result<()> {
    let res = client.call(
        "databaseBackups",
        "listDatabaseBackups",
        json!({ "projectName": project, "serviceName": service }),
    )?;
    let arr = res.as_array().cloned().unwrap_or_default();
    if arr.is_empty() {
        println!("Tidak ada database backup.");
        return Ok(());
    }
    let rows = arr
        .iter()
        .map(|b| {
            vec![
                field(b, "/id"),
                field(b, "/databaseName"),
                field(b, "/schedule"),
                yes_no(b, "/enabled"),
            ]
        })
        .collect();
    table(&["ID", "Database", "Schedule", "Aktif"], rows);
    Ok(())
}

pub fn db_backup_run(client: &EasypanelClient, id: &str) -> Result<()> {
    client.call("databaseBackups", "runDatabaseBackup", json!({ "id": id }))?;
    println!("Backup database {id} dijalankan.");
    Ok(())
}

pub fn db_backup_delete(client: &EasypanelClient, id: &str) -> Result<()> {
    client.call(
        "databaseBackups",
        "deleteDatabaseBackup",
        json!({ "id": id }),
    )?;
    println!("Backup database {id} dihapus.");
    Ok(())
}

pub fn volume_backup_list(client: &EasypanelClient, project: &str, service: &str) -> Result<()> {
    let res = client.call(
        "volumeBackups",
        "listVolumeBackups",
        json!({ "projectName": project, "serviceName": service }),
    )?;
    let arr = res.as_array().cloned().unwrap_or_default();
    if arr.is_empty() {
        println!("Tidak ada volume backup.");
        return Ok(());
    }
    let rows = arr
        .iter()
        .map(|b| {
            vec![
                field(b, "/id"),
                field(b, "/volumeName"),
                field(b, "/schedule"),
                yes_no(b, "/enabled"),
            ]
        })
        .collect();
    table(&["ID", "Volume", "Schedule", "Aktif"], rows);
    Ok(())
}

pub fn volume_backup_run(client: &EasypanelClient, id: &str) -> Result<()> {
    client.call("volumeBackups", "runVolumeBackup", json!({ "id": id }))?;
    println!("Volume backup {id} dijalankan.");
    Ok(())
}

pub fn volume_backup_delete(client: &EasypanelClient, id: &str) -> Result<()> {
    client.call("volumeBackups", "destroyVolumeBackup", json!({ "id": id }))?;
    println!("Volume backup {id} dihapus.");
    Ok(())
}

// ---------- Actions ----------

/// Bangun input listActions dari filter opsional.
pub fn actions_input(
    limit: u32,
    project: &Option<String>,
    service: &Option<String>,
    atype: &Option<String>,
) -> Value {
    let mut input = json!({ "limit": limit });
    if let Some(p) = project {
        input["projectName"] = json!(p);
    }
    if let Some(s) = service {
        input["serviceName"] = json!(s);
    }
    if let Some(t) = atype {
        input["type"] = json!(t);
    }
    input
}

/// Batas deskripsi untuk tabel CLI: comfy-table melebarkan kolom sepanjang
/// isinya, jadi baris panjang harus dipotong di sini.
pub const ACTION_DESC_CLI: usize = 60;
/// TUI memakai batas longgar karena widget tabelnya meng-clip sendiri sesuai
/// lebar kolom — memotong lebih awal justru menyisakan ruang kosong.
pub const ACTION_DESC_TUI: usize = 200;

/// Baris tabel untuk satu action; deskripsi dipotong pada `desc_max`.
pub fn action_row(a: &Value, desc_max: usize) -> Vec<String> {
    let target = match (
        field(a, "/projectName").as_str(),
        field(a, "/serviceName").as_str(),
    ) {
        ("-", _) => "-".to_string(),
        (p, "-") => p.to_string(),
        (p, s) => format!("{p}/{s}"),
    };
    vec![
        field(a, "/status"),
        target,
        first_line(&field(a, "/description"), desc_max),
        duration_between(&field(a, "/createdAt"), &field(a, "/updatedAt")),
        age_of(&field(a, "/createdAt")),
    ]
}

pub const ACTION_HEADERS: [&str; 5] = ["Status", "Target", "Deskripsi", "Durasi", "Umur"];

pub fn action_list(
    client: &EasypanelClient,
    limit: u32,
    project: Option<String>,
    service: Option<String>,
    atype: Option<String>,
) -> Result<()> {
    let input = actions_input(limit, &project, &service, &atype);
    let actions = client.call("actions", "listActions", input)?;
    let arr = actions.as_array().cloned().unwrap_or_default();
    if arr.is_empty() {
        println!("Tidak ada action.");
        return Ok(());
    }
    table(
        &ACTION_HEADERS,
        arr.iter().map(|a| action_row(a, ACTION_DESC_CLI)).collect(),
    );
    Ok(())
}

pub fn action_kill(client: &EasypanelClient, id: &str) -> Result<()> {
    client.call("actions", "killAction", json!({ "id": id }))?;
    println!("Action {id} dihentikan.");
    Ok(())
}

// ---------- Monitor ----------

/// Nama service dari containerName ("proj_svc.1.hash" -> "svc").
///
/// Field `serviceName` dari API keliru untuk sub-service compose: container
/// `proj_mysql_phpmyadmin.1.x` dilaporkan sebagai `mysql`. Panel menurunkannya
/// dari containerName, jadi kita ikut supaya namanya cocok.
/// Baris tabel monitor per project (header project + service-nya), urut memori terbesar.
///
/// Sumber: `metrics/getAllServicesStats` — `networkIn`/`networkOut` sudah berupa
/// laju byte/detik, dan `serviceName` benar untuk sub-service compose.
pub fn monitor_rows(services: Vec<Value>) -> Vec<Vec<String>> {
    let mem = |c: &Value| num(c, "/memory");
    let mut groups: std::collections::HashMap<String, Vec<Value>> =
        std::collections::HashMap::new();
    for c in services {
        groups.entry(field(&c, "/projectName")).or_default().push(c);
    }
    let mut groups: Vec<(String, Vec<Value>)> = groups.into_iter().collect();
    let total = |v: &[Value]| -> f64 { v.iter().map(mem).sum() };
    groups.sort_by(|a, b| total(&b.1).total_cmp(&total(&a.1)));

    let mut rows = Vec::new();
    for (project, mut svcs) in groups {
        svcs.sort_by(|a, b| mem(b).total_cmp(&mem(a)));
        let sum = |ptr: &str| -> f64 { svcs.iter().map(|c| num(c, ptr)).sum() };
        rows.push(vec![
            format!("{project} ({})", svcs.len()),
            format!("{:.1} %", sum("/cpu")),
            format_bytes(sum("/memory")),
            format_rate(sum("/networkIn")),
            format_rate(sum("/networkOut")),
        ]);
        for c in svcs {
            rows.push(vec![
                format!("  {}", field(&c, "/serviceName")),
                format!("{:.1} %", num(&c, "/cpu")),
                format_bytes(num(&c, "/memory")),
                format_rate(num(&c, "/networkIn")),
                format_rate(num(&c, "/networkOut")),
            ]);
        }
    }
    rows
}

pub const MONITOR_HEADERS: [&str; 5] =
    ["Project / Service", "CPU %", "Memory", "Net In", "Net Out"];

pub fn monitor_services(client: &EasypanelClient) -> Result<()> {
    let data = client.call("metrics", "getAllServicesStats", json!({}))?;
    let arr = data.as_array().cloned().unwrap_or_default();
    if arr.is_empty() {
        println!("Tidak ada service berjalan.");
        return Ok(());
    }
    table(&MONITOR_HEADERS, monitor_rows(arr));
    Ok(())
}

/// Baris tabel storage, urut terbesar.
pub fn storage_rows(mut arr: Vec<Value>) -> Vec<Vec<String>> {
    arr.sort_by(|a, b| num(b, "/size").total_cmp(&num(a, "/size")));
    arr.iter()
        .map(|s| {
            vec![
                field(s, "/projectName"),
                field(s, "/serviceName"),
                format_bytes(num(s, "/size")),
                field(s, "/path"),
            ]
        })
        .collect()
}

pub const STORAGE_HEADERS: [&str; 4] = ["Project", "Service", "Ukuran", "Path"];

pub fn monitor_storage(client: &EasypanelClient) -> Result<()> {
    let data = client.call("monitorOld", "getStorageStats", Value::Null)?;
    let arr = data.as_array().cloned().unwrap_or_default();
    if arr.is_empty() {
        println!("Tidak ada data storage.");
        return Ok(());
    }
    table(&STORAGE_HEADERS, storage_rows(arr));
    Ok(())
}

// ---------- Domains (host-wide) ----------

/// Sumber domain: "https://host/path".
pub fn domain_source(d: &Value) -> String {
    let scheme = if d.get("https").and_then(Value::as_bool).unwrap_or(false) {
        "https"
    } else {
        "http"
    };
    format!("{scheme}://{}{}", field(d, "/host"), field(d, "/path"))
}

/// Tujuan domain: service internal, atau daftar server custom dengan bobotnya.
pub fn domain_destination(d: &Value) -> String {
    match field(d, "/destinationType").as_str() {
        "service" => format!(
            "{}://{}_{}:{}{}",
            field(d, "/serviceDestination/protocol"),
            field(d, "/serviceDestination/projectName"),
            field(d, "/serviceDestination/serviceName"),
            field(d, "/serviceDestination/port"),
            field(d, "/serviceDestination/path"),
        ),
        "custom" => d
            .pointer("/customDestination/servers")
            .and_then(Value::as_array)
            .map(|servers| {
                servers
                    .iter()
                    .map(|s| format!("{} ({})", field(s, "/url"), field(s, "/weight")))
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_else(|| "-".to_string()),
        _ => "-".to_string(),
    }
}

pub const DOMAIN_HEADERS: [&str; 3] = ["Source", "Destination", "ID"];

pub fn domain_row(d: &Value) -> Vec<String> {
    vec![domain_source(d), domain_destination(d), field(d, "/id")]
}

pub fn domain_list_all(client: &EasypanelClient) -> Result<()> {
    let domains = client.call("domains", "listDomains", json!({}))?;
    let arr = domains.as_array().cloned().unwrap_or_default();
    if arr.is_empty() {
        println!("Tidak ada domain.");
        return Ok(());
    }
    table(&DOMAIN_HEADERS, arr.iter().map(domain_row).collect());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn svc(project: &str, name: &str, mem: f64, cpu: f64) -> Value {
        json!({
            "projectName": project, "serviceName": name,
            "cpu": cpu, "memory": mem, "networkIn": 1024.0, "networkOut": 2048.0
        })
    }

    #[test]
    fn monitor_groups_by_project_and_sorts_by_memory() {
        let rows = monitor_rows(vec![
            svc("small", "a", 10.0, 0.1),
            svc("big", "kecil", 1.0, 0.2),
            svc("big", "besar", 1_073_741_824.0, 0.5),
        ]);

        // Project dengan memori terbesar lebih dulu, lalu service-nya urut memori.
        let names: Vec<&str> = rows.iter().map(|r| r[0].as_str()).collect();
        assert_eq!(
            names,
            vec!["big (2)", "  besar", "  kecil", "small (1)", "  a"]
        );
        // Baris project = jumlah service-nya.
        assert_eq!(rows[0][1], "0.7 %");
        assert_eq!(rows[0][4], "4.0 KB/s"); // 2048*2
    }

    #[test]
    fn monitor_formats_memory_and_rates() {
        let rows = monitor_rows(vec![svc("p", "s", 1_073_741_824.0, 12.34)]);
        assert_eq!(rows[1][0], "  s");
        assert_eq!(rows[1][1], "12.3 %");
        assert_eq!(rows[1][2], "1.0 GB");
        assert_eq!(rows[1][3], "1.0 KB/s");
    }

    #[test]
    fn domain_destination_handles_service_and_custom() {
        let service = json!({
            "destinationType": "service",
            "serviceDestination": {
                "protocol": "http", "projectName": "proj", "serviceName": "api",
                "port": 8000, "path": "/"
            }
        });
        assert_eq!(domain_destination(&service), "http://proj_api:8000/");

        let custom = json!({
            "destinationType": "custom",
            "customDestination": { "servers": [
                { "url": "https://a.test", "weight": 1 },
                { "url": "https://b.test", "weight": 2 }
            ]}
        });
        assert_eq!(
            domain_destination(&custom),
            "https://a.test (1), https://b.test (2)"
        );

        assert_eq!(
            domain_destination(&json!({ "destinationType": "aneh" })),
            "-"
        );
    }

    #[test]
    fn domain_source_uses_scheme_from_https_flag() {
        assert_eq!(
            domain_source(&json!({ "https": true, "host": "a.test", "path": "/x" })),
            "https://a.test/x"
        );
        assert_eq!(
            domain_source(&json!({ "https": false, "host": "a.test", "path": "/" })),
            "http://a.test/"
        );
    }

    #[test]
    fn action_row_shows_target_duration_and_trims_description() {
        let a = json!({
            "projectName": "proj", "serviceName": "api", "status": "done",
            "description": "Deploy service: baris pertama\nbaris kedua diabaikan",
            "createdAt": "2026-07-16 05:55:15", "updatedAt": "2026-07-16 06:03:14"
        });
        let row = action_row(&a, ACTION_DESC_CLI);
        assert_eq!(row[0], "done");
        assert_eq!(row[1], "proj/api");
        assert_eq!(row[2], "Deploy service: baris pertama");
        assert_eq!(row[3], "7 menit"); // 05:55:15 -> 06:03:14
    }

    #[test]
    fn action_row_target_falls_back_when_not_service_scoped() {
        let login = json!({
            "status": "done", "description": "User masuk",
            "createdAt": "2026-07-16 05:55:15", "updatedAt": "2026-07-16 05:55:15"
        });
        assert_eq!(action_row(&login, ACTION_DESC_CLI)[1], "-");
        assert_eq!(action_row(&login, ACTION_DESC_CLI)[3], "0 detik");
    }

    #[test]
    fn actions_input_only_includes_given_filters() {
        let bare = actions_input(10, &None, &None, &None);
        assert_eq!(bare, json!({ "limit": 10 }));

        let filtered = actions_input(
            5,
            &Some("p".into()),
            &Some("s".into()),
            &Some("deployment".into()),
        );
        assert_eq!(
            filtered,
            json!({ "limit": 5, "projectName": "p", "serviceName": "s", "type": "deployment" })
        );
    }

    #[test]
    fn stats_rows_read_metrics_series_and_byte_totals() {
        let s = json!({
            "cpu": [[1, "1.0"], [2, "5.5"]],
            "cpuCores": "16",
            "loadAvg": ["0.10", "0.20", "0.30"],
            "memory": [[1, "25.0"]],
            "memoryUsedBytes": "1073741824",
            "memoryTotalBytes": "2147483648",
            "disk": [[1, "16.2"]],
            "diskUsedBytes": "1073741824",
            "diskTotalBytes": "10737418240",
            "networkIn": [[1, "1024"]],
            "networkOut": [[1, "2048"]]
        });
        let rows = stats_rows(&s);
        assert_eq!(rows[0], vec!["CPU", "5.5 %"]); // titik terakhir
        assert_eq!(rows[1], vec!["Cores", "16"]);
        assert_eq!(rows[2], vec!["Load avg", "0.10, 0.20, 0.30"]);
        assert_eq!(rows[3], vec!["Memory", "25.0 % (1.0 GB / 2.0 GB)"]);
        assert_eq!(rows[4], vec!["Disk", "16.2 % (1.0 GB / 10.0 GB)"]);
        assert_eq!(rows[5], vec!["Network In", "1.0 KB/s"]);
    }

    #[test]
    fn storage_rows_sorted_by_size_desc() {
        let rows = storage_rows(vec![
            json!({ "projectName": "p", "serviceName": "kecil", "size": 1024, "path": "/a" }),
            json!({ "projectName": "p", "serviceName": "besar", "size": 1048576, "path": "/b" }),
        ]);
        assert_eq!(rows[0][1], "besar");
        assert_eq!(rows[0][2], "1.0 MB");
        assert_eq!(rows[1][1], "kecil");
    }
}
