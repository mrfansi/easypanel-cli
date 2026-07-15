use anyhow::{anyhow, Result};
use dialoguer::{Input, Password};
use serde_json::{json, Value};

use crate::client::EasypanelClient;
use crate::config::ServerConfig;
use crate::output::{field, table};
use crate::{logs, menu};

/// Resolve klien untuk server aktif (dari --server atau default).
pub fn resolve_client(cfg: &ServerConfig, server: &Option<String>) -> Result<EasypanelClient> {
    let s = match server {
        Some(name) => cfg
            .get(name)
            .ok_or_else(|| anyhow!("Server '{}' tidak ditemukan. Lihat: easypanel server list", name))?,
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
    client.call(&format!("services/{stype}"), &format!("{action}Service"), input)?;
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

// ---------- Menu (delegasi) ----------

pub fn run_menu(cfg: &ServerConfig) -> Result<()> {
    menu::run(cfg)
}
