use anyhow::{anyhow, Result};
use dialoguer::{Confirm, Input, Password};
use serde_json::{json, Value};
use std::io::Read;

use crate::client::EasypanelClient;
use crate::config::ServerConfig;
use crate::output::{field, table, yes_no};
use crate::{logs, menu};

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

pub fn stats(client: &EasypanelClient) -> Result<()> {
    let s = client.call("monitorOld", "getSystemStats", Value::Null)?;
    table(
        &["Metrik", "Nilai"],
        vec![
            vec!["CPU cores".into(), field(&s, "/cpuInfo/count")],
            vec!["CPU used %".into(), field(&s, "/cpuInfo/usedPercentage")],
            vec!["Mem used %".into(), field(&s, "/memInfo/usedMemPercentage")],
            vec!["Mem used MB".into(), field(&s, "/memInfo/usedMemMb")],
            vec!["Disk used %".into(), field(&s, "/diskInfo/usedPercentage")],
            vec!["Disk free GB".into(), field(&s, "/diskInfo/freeGb")],
            vec!["Uptime (s)".into(), field(&s, "/uptime")],
        ],
    );
    Ok(())
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

// ---------- Menu (delegasi) ----------

pub fn run_menu(cfg: &ServerConfig) -> Result<()> {
    menu::run(cfg)
}
