use anyhow::{anyhow, Result};
use dialoguer::{Confirm, Input, Password};
use serde_json::{json, Value};
use std::io::Read;

use crate::client::EasypanelClient;
use crate::config::ServerConfig;
use crate::logs;
use crate::output::{
    self, age_of, duration_between, field, first_line, format_bytes, format_rate, num, series_last,
    table, yes_no,
};

/// Resolve the client for the active server (from --server or the default).
pub fn resolve_client(cfg: &ServerConfig, server: &Option<String>) -> Result<EasypanelClient> {
    let s = match server {
        Some(name) => cfg
            .get(name)
            .ok_or_else(|| anyhow!("Server '{}' not found. See: easypanel server list", name))?,
        None => cfg
            .default()
            .ok_or_else(|| anyhow!("No default server. Run: easypanel server add"))?,
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
        None => Input::new().with_prompt("Server name").interact_text()?,
    };
    if !valid_name(&name) {
        return Err(anyhow!("Server names may only contain a-z, 0-9, - and _"));
    }
    let url = match url {
        Some(u) => u,
        None => Input::new()
            .with_prompt("URL host (e.g. https://panel.example.com)")
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
        "Server '{}' added.{}",
        name,
        if is_default { " (default)" } else { "" }
    );
    Ok(())
}

pub fn server_list(cfg: &ServerConfig) -> Result<()> {
    let servers = cfg.all();
    if servers.is_empty() {
        println!("No servers configured yet. Run: easypanel server add");
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
    table(&["Default", "Name", "URL", "Token"], rows);
    Ok(())
}

pub fn server_use(cfg: &ServerConfig, name: &str) -> Result<()> {
    if cfg.get(name).is_none() {
        return Err(anyhow!("Server '{}' not found.", name));
    }
    cfg.set_default(name)?;
    println!("Default server is now: {name}");
    Ok(())
}

pub fn server_remove(cfg: &ServerConfig, name: &str) -> Result<()> {
    if cfg.get(name).is_none() {
        return Err(anyhow!("Server '{}' not found.", name));
    }
    cfg.remove(name)?;
    println!("Server '{name}' removed.");
    Ok(())
}

fn mask_token(token: &str) -> String {
    // Per CHARACTER, not byte: the token comes from a config file that can be
    // hand-edited. `&token[..6]` slices at a byte index, and a token with a
    // multibyte character at that boundary would make `server list` panic —
    // len() counts bytes, so the <= 10 guard alone wouldn't protect against it.
    let chars: Vec<char> = token.chars().collect();
    if chars.len() <= 10 {
        "***".to_string()
    } else {
        let head: String = chars[..6].iter().collect();
        let tail: String = chars[chars.len() - 4..].iter().collect();
        format!("{head}…{tail}")
    }
}

// ---------- Projects ----------

pub fn project_list(client: &EasypanelClient) -> Result<()> {
    let projects = client.call("projects", "listProjects", Value::Null)?;
    if output::json_output() {
        output::print_json(&projects);
        return Ok(());
    }
    let arr = projects.as_array().cloned().unwrap_or_default();
    if arr.is_empty() {
        println!("No projects.");
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
    table(&["Name", "Created", "Members"], rows);
    Ok(())
}

pub fn project_create(client: &EasypanelClient, name: &str) -> Result<()> {
    if !valid_name(name) {
        return Err(anyhow!("Project names may only contain a-z, 0-9, - and _"));
    }
    client.call("projects", "createProject", json!({ "name": name }))?;
    println!("Project '{name}' created.");
    Ok(())
}

pub fn project_inspect(client: &EasypanelClient, name: &str) -> Result<()> {
    let data = client.call("projects", "inspectProject", json!({ "projectName": name }))?;
    if output::json_output() {
        output::print_json(&data);
        return Ok(());
    }
    let services = data
        .get("services")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    if services.is_empty() {
        println!("Project has no services.");
        return Ok(());
    }

    let rows = services
        .iter()
        .map(|s| {
            vec![
                field(s, "/name"),
                field(s, "/type"),
                if s.get("enabled").and_then(Value::as_bool).unwrap_or(false) {
                    "yes".into()
                } else {
                    "no".into()
                },
            ]
        })
        .collect();
    table(&["Service", "Type", "Enabled"], rows);
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
    println!("{} triggered for {}/{}.", ucfirst(action), project, service);
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
        println!("No logs.");
        return Ok(());
    }
    for line in lines {
        println!("{line}");
    }
    Ok(())
}

// ---------- Monitoring & Cluster ----------

/// Host metrics summary from the `metrics` group (Prometheus): ~0.3s and already
/// includes network rate + total/used bytes, unlike `monitorOld` (~2.3s).
pub fn stats(client: &EasypanelClient) -> Result<()> {
    let s = client.call("metrics", "getSystemStats", json!({}))?;
    if output::json_output() {
        output::print_json(&s);
        return Ok(());
    }
    table(&["Metric", "Value"], stats_rows(&s));
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
    if output::json_output() {
        output::print_json(&nodes);
        return Ok(());
    }
    let arr = nodes.as_array().cloned().unwrap_or_default();
    if arr.is_empty() {
        println!("No nodes (or this host is not a cluster).");
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
    save_env(client, project, service, stype, &env)?;
    println!("Env for {project}/{service} updated.");
    Ok(())
}

/// Write a service's env, whichever endpoint its type uses. app-ish types have
/// `updateEnv`; databases keep env inside the Advanced block (`updateAdvanced`).
/// Both replace the whole block they own, so inspect first and keep the fields we
/// aren't editing (`dotEnvPath` / image, command, configFile) instead of wiping them.
pub fn save_env(
    client: &EasypanelClient,
    project: &str,
    service: &str,
    stype: &str,
    env: &str,
) -> Result<()> {
    let grp = format!("services/{stype}");
    let ps = json!({ "projectName": project, "serviceName": service });
    let cur = client.call(&grp, "inspectService", ps)?;
    let (op, body) = if HAS_UPDATE_ENV.contains(&stype) {
        let mut b = json!({ "projectName": project, "serviceName": service, "env": env });
        // The server rejects a null/empty dotEnvPath, so "no file" = omit the field.
        if let Some(dot) = cur.get("dotEnvPath").and_then(Value::as_str) {
            b["dotEnvPath"] = json!(dot);
        }
        ("updateEnv", b)
    } else {
        let mut b = json!({
            "projectName": project,
            "serviceName": service,
            // image & command MUST be strings — null/omitted is rejected.
            "image": cur.get("image").and_then(Value::as_str).unwrap_or(""),
            "command": cur.get("command").and_then(Value::as_str).unwrap_or(""),
            "env": env,
        });
        if let Some(cfg) = cur.get("configFile").and_then(Value::as_str) {
            b["configFile"] = json!(cfg);
        }
        ("updateAdvanced", b)
    };
    client.call(&grp, op, body)?;
    Ok(())
}

/// Service types with an `updateEnv` endpoint. The rest (mysql, postgres, redis, …)
/// do have env, but as part of the Advanced block → `updateAdvanced`.
pub const HAS_UPDATE_ENV: &[&str] = &["app", "box", "compose", "wordpress"];

// ---------- Ports (group "ports") ----------

pub fn ports_list(client: &EasypanelClient, project: &str, service: &str) -> Result<()> {
    let ports = client.call(
        "ports",
        "listPorts",
        json!({ "projectName": project, "serviceName": service }),
    )?;
    if output::json_output() {
        output::print_json(&ports);
        return Ok(());
    }
    let arr = ports.as_array().cloned().unwrap_or_default();
    if arr.is_empty() {
        println!("No exposed ports.");
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
    table(&["Index", "Protocol", "Published", "Target"], rows);
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
    println!("Port {published}->{target}/{protocol} added to {project}/{service}.");
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
    println!("Port {index} removed from {project}/{service}.");
    Ok(())
}

// ---------- Mounts (group "mounts") ----------

pub fn mounts_list(client: &EasypanelClient, project: &str, service: &str) -> Result<()> {
    let mounts = client.call(
        "mounts",
        "listMounts",
        json!({ "projectName": project, "serviceName": service }),
    )?;
    if output::json_output() {
        output::print_json(&mounts);
        return Ok(());
    }
    let arr = mounts.as_array().cloned().unwrap_or_default();
    if arr.is_empty() {
        println!("No mounts.");
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
    table(&["Index", "Type", "Detail"], rows);
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
            "name": name.ok_or_else(|| anyhow!("--name is required for a volume mount"))?,
            "mountPath": mount_path
        }),
        "bind" => json!({
            "type": "bind",
            "hostPath": host_path.ok_or_else(|| anyhow!("--host-path is required for a bind mount"))?,
            "mountPath": mount_path
        }),
        other => return Err(anyhow!("Unsupported mount type: {other} (use volume|bind)")),
    };
    client.call(
        "mounts",
        "createMount",
        json!({ "projectName": project, "serviceName": service, "values": values }),
    )?;
    println!("Mount {kind} added to {project}/{service}.");
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
    println!("Mount {index} removed from {project}/{service}.");
    Ok(())
}

// ---------- Domains (group "domains") ----------

pub fn domains_list(client: &EasypanelClient, project: &str, service: &str) -> Result<()> {
    let domains = client.call(
        "domains",
        "listDomains",
        json!({ "projectName": project, "serviceName": service }),
    )?;
    if output::json_output() {
        output::print_json(&domains);
        return Ok(());
    }
    let arr = domains.as_array().cloned().unwrap_or_default();
    if arr.is_empty() {
        println!("No domains.");
        return Ok(());
    }
    let rows = arr
        .iter()
        .map(|dm| {
            vec![
                field(dm, "/id"),
                field(dm, "/host"),
                if dm.get("https").and_then(Value::as_bool).unwrap_or(false) {
                    "yes".into()
                } else {
                    "no".into()
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
    println!("Domain {id} deleted.");
    Ok(())
}

pub fn domain_set_primary(client: &EasypanelClient, id: &str) -> Result<()> {
    client.call("domains", "setPrimaryDomain", json!({ "id": id }))?;
    println!("Domain {id} is now primary.");
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
        return Err(anyhow!("Service names may only contain a-z, 0-9, - and _"));
    }
    client.call(
        &format!("services/{stype}"),
        "createService",
        json!({ "projectName": project, "serviceName": service }),
    )?;
    println!("Service {service} ({stype}) created in {project}.");
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
        &format!("Destroy service '{service}' in '{project}'? This cannot be undone."),
        yes,
    )? {
        return Ok(());
    }
    client.call(
        &format!("services/{stype}"),
        "destroyService",
        json!({ "projectName": project, "serviceName": service }),
    )?;
    println!("Service {project}/{service} destroyed.");
    Ok(())
}

pub fn project_destroy(client: &EasypanelClient, name: &str, yes: bool) -> Result<()> {
    if !confirm(
        &format!("Destroy project '{name}' and every service in it? This cannot be undone."),
        yes,
    )? {
        return Ok(());
    }
    client.call("projects", "destroyProject", json!({ "name": name }))?;
    println!("Project {name} destroyed.");
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
    if output::json_output() {
        output::print_json(&certs);
        return Ok(());
    }
    let arr = certs.as_array().cloned().unwrap_or_default();
    if arr.is_empty() {
        println!("No certificates.");
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
    println!("Certificate for {domain} removed.");
    Ok(())
}

// ---------- Notifications ----------

pub fn notification_list(client: &EasypanelClient) -> Result<()> {
    let res = client.call("notifications", "listNotificationChannels", Value::Null)?;
    if output::json_output() {
        output::print_json(&res);
        return Ok(());
    }
    let arr = res
        .get("notificationChannels")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if arr.is_empty() {
        println!("No notification channels.");
        return Ok(());
    }
    let rows = arr
        .iter()
        .map(|c| vec![field(c, "/id"), field(c, "/name")])
        .collect();
    table(&["ID", "Name"], rows);
    Ok(())
}

pub fn notification_delete(client: &EasypanelClient, id: &str) -> Result<()> {
    client.call(
        "notifications",
        "destroyNotificationChannel",
        json!({ "id": id }),
    )?;
    println!("Notification channel {id} deleted.");
    Ok(())
}

// ---------- Databases & Backups ----------

pub fn service_databases(client: &EasypanelClient, project: &str, service: &str) -> Result<()> {
    let dbs = client.call(
        "databaseBackups",
        "getServiceDatabases",
        json!({ "projectName": project, "serviceName": service }),
    )?;
    if output::json_output() {
        output::print_json(&dbs);
        return Ok(());
    }
    let arr = dbs.as_array().cloned().unwrap_or_default();
    if arr.is_empty() {
        println!("No databases.");
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
    if output::json_output() {
        output::print_json(&res);
        return Ok(());
    }
    let arr = res.as_array().cloned().unwrap_or_default();
    if arr.is_empty() {
        println!("No database backups.");
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
    table(&["ID", "Database", "Schedule", "Enabled"], rows);
    Ok(())
}

pub fn db_backup_run(client: &EasypanelClient, id: &str) -> Result<()> {
    client.call("databaseBackups", "runDatabaseBackup", json!({ "id": id }))?;
    println!("Database backup {id} started.");
    Ok(())
}

pub fn db_backup_delete(client: &EasypanelClient, id: &str) -> Result<()> {
    client.call(
        "databaseBackups",
        "deleteDatabaseBackup",
        json!({ "id": id }),
    )?;
    println!("Database backup {id} deleted.");
    Ok(())
}

pub fn volume_backup_list(client: &EasypanelClient, project: &str, service: &str) -> Result<()> {
    let res = client.call(
        "volumeBackups",
        "listVolumeBackups",
        json!({ "projectName": project, "serviceName": service }),
    )?;
    if output::json_output() {
        output::print_json(&res);
        return Ok(());
    }
    let arr = res.as_array().cloned().unwrap_or_default();
    if arr.is_empty() {
        println!("No volume backups.");
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
    table(&["ID", "Volume", "Schedule", "Enabled"], rows);
    Ok(())
}

pub fn volume_backup_run(client: &EasypanelClient, id: &str) -> Result<()> {
    client.call("volumeBackups", "runVolumeBackup", json!({ "id": id }))?;
    println!("Volume backup {id} started.");
    Ok(())
}

pub fn volume_backup_delete(client: &EasypanelClient, id: &str) -> Result<()> {
    client.call("volumeBackups", "destroyVolumeBackup", json!({ "id": id }))?;
    println!("Volume backup {id} deleted.");
    Ok(())
}

// ---------- Actions ----------

/// Build listActions input from optional filters.
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

/// Description limit for the CLI table: comfy-table widens columns to fit
/// their content, so long lines need to be truncated here.
pub const ACTION_DESC_CLI: usize = 60;
/// The TUI uses a looser limit because its table widget clips its own content
/// to the column width — truncating earlier would just leave empty space.
pub const ACTION_DESC_TUI: usize = 200;

/// Table row for a single action; description truncated at `desc_max`.
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

pub const ACTION_HEADERS: [&str; 5] = ["Status", "Target", "Description", "Duration", "Age"];

pub fn action_list(
    client: &EasypanelClient,
    limit: u32,
    project: Option<String>,
    service: Option<String>,
    atype: Option<String>,
) -> Result<()> {
    let input = actions_input(limit, &project, &service, &atype);
    let actions = client.call("actions", "listActions", input)?;
    if output::json_output() {
        output::print_json(&actions);
        return Ok(());
    }
    let arr = actions.as_array().cloned().unwrap_or_default();
    if arr.is_empty() {
        println!("No actions.");
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
    println!("Action {id} killed.");
    Ok(())
}

// ---------- Monitor ----------

/// Service name from containerName ("proj_svc.1.hash" -> "svc").
///
/// The API's `serviceName` field is wrong for compose sub-services: the
/// container `proj_mysql_phpmyadmin.1.x` is reported as `mysql`. The panel
/// derives it from containerName, so we follow suit so the name matches.
/// Monitor table rows per project (project header + its services), sorted by memory descending.
///
/// Source: `metrics/getAllServicesStats` — `networkIn`/`networkOut` are already
/// byte/sec rates, and `serviceName` is correct for compose sub-services.
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
    if output::json_output() {
        output::print_json(&data);
        return Ok(());
    }
    let arr = data.as_array().cloned().unwrap_or_default();
    if arr.is_empty() {
        println!("No running services.");
        return Ok(());
    }
    table(&MONITOR_HEADERS, monitor_rows(arr));
    Ok(())
}

/// Storage table rows, sorted largest first.
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

pub const STORAGE_HEADERS: [&str; 4] = ["Project", "Service", "Size", "Path"];

pub fn monitor_storage(client: &EasypanelClient) -> Result<()> {
    let data = client.call("monitorOld", "getStorageStats", Value::Null)?;
    if output::json_output() {
        output::print_json(&data);
        return Ok(());
    }
    let arr = data.as_array().cloned().unwrap_or_default();
    if arr.is_empty() {
        println!("No storage data.");
        return Ok(());
    }
    table(&STORAGE_HEADERS, storage_rows(arr));
    Ok(())
}

// ---------- Domains (host-wide) ----------

/// Domain source: "https://host/path".
pub fn domain_source(d: &Value) -> String {
    let scheme = if d.get("https").and_then(Value::as_bool).unwrap_or(false) {
        "https"
    } else {
        "http"
    };
    format!("{scheme}://{}{}", field(d, "/host"), field(d, "/path"))
}

/// Domain destination: an internal service, or a list of custom servers with their weights.
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
    if output::json_output() {
        output::print_json(&domains);
        return Ok(());
    }
    let arr = domains.as_array().cloned().unwrap_or_default();
    if arr.is_empty() {
        println!("No domains.");
        return Ok(());
    }
    table(&DOMAIN_HEADERS, arr.iter().map(domain_row).collect());
    Ok(())
}

/// Server info for `maintenance info`.
pub fn maintenance_info(client: &EasypanelClient) -> Result<()> {
    let one = |op: &str| match client.call("settings", op, Value::Null) {
        Ok(v) => field(&v, ""),
        Err(e) => format!("error: {e}"),
    };
    table(
        &["Item", "Value"],
        vec![
            vec!["Docker".into(), one("getDockerVersion")],
            vec!["Server IP".into(), one("getServerIp")],
            vec!["Update available".into(), one("checkForUpdates")],
            vec!["Daily cleanup".into(), one("getDailyDockerCleanup")],
        ],
    );
    Ok(())
}

/// Docker cleanup; `op` is already constrained by the CLI enum.
pub fn maintenance_clean(client: &EasypanelClient, op: &str, label: &str, yes: bool) -> Result<()> {
    if !confirm(
        &format!("{label} on the whole host? This cannot be undone."),
        yes,
    )? {
        return Ok(());
    }
    client.call("settings", op, Value::Null)?;
    println!("{label}: done.");
    Ok(())
}

/// Registered storage providers (their id is needed for restore).
pub fn storage_providers(client: &EasypanelClient) -> Result<()> {
    let v = client.call("storageProviders/common", "list", Value::Null)?;
    let rows = v
        .as_array()
        .map(|a| {
            a.iter()
                .map(|p| {
                    vec![
                        field(p, "/id"),
                        field(p, "/name"),
                        field(p, "/type"),
                        field(p, "/path"),
                    ]
                })
                .collect()
        })
        .unwrap_or_default();
    table(&["ID", "Name", "Type", "Path"], rows);
    Ok(())
}

/// Restore a database from a backup file.
///
/// `path` has to be known ahead of time: the EasyPanel API has no endpoint to
/// list existing backup files (check `easypanel-api.json` — only schedules can
/// be listed, not their contents). That's why the path is required explicitly
/// rather than guessed.
#[allow(clippy::too_many_arguments)]
pub fn backup_db_restore(
    client: &EasypanelClient,
    project: &str,
    service: &str,
    database: &str,
    path: &str,
    provider: Option<&str>,
    yes: bool,
) -> Result<()> {
    // The provider may be omitted only when there's exactly one — guessing
    // among several providers isn't the CLI's job.
    let provider_id = match provider {
        Some(p) => p.to_string(),
        None => {
            let v = client.call("storageProviders/common", "list", Value::Null)?;
            let all = v.as_array().cloned().unwrap_or_default();
            match all.len() {
                1 => field(&all[0], "/id"),
                0 => anyhow::bail!("No storage provider is configured."),
                n => anyhow::bail!(
                    "There are {n} storage providers; pick one with --provider \
                     (see: easypanel backup providers)."
                ),
            }
        }
    };

    if !confirm(
        &format!(
            "Restore '{database}' on {project}/{service} from '{path}'? \
             The current database contents will be OVERWRITTEN and cannot be recovered."
        ),
        yes,
    )? {
        return Ok(());
    }

    client.call(
        "databaseBackups",
        "restoreDatabaseBackup",
        json!({
            "projectName": project,
            "serviceName": service,
            "databaseName": database,
            "path": path,
            "storageProviderId": provider_id,
        }),
    )?;
    println!("Restore of '{database}' started.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;

    /// A mysql service has no updateEnv endpoint — its env lives in the Advanced
    /// block. Sending updateEnv there returned 404; save_env must route by type and
    /// keep image/command/configFile intact.
    #[test]
    fn env_of_a_database_goes_through_update_advanced() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.path("/api/rpc/services/mysql/inspectService");
            then.status(200).json_body(json!({ "json": {
                "image": "mysql:8.0", "command": "", "configFile": "[mysqld]"
            }, "meta": [] }));
        });
        let save = server.mock(|when, then| {
            when.path("/api/rpc/services/mysql/updateAdvanced")
                .json_body(json!({ "json": {
                    "projectName": "p", "serviceName": "db", "image": "mysql:8.0",
                    "command": "", "configFile": "[mysqld]", "env": "TZ=Asia/Jakarta"
                }}));
            then.status(200)
                .json_body(json!({ "json": null, "meta": [] }));
        });

        let client = EasypanelClient::new(&server.base_url(), "t");
        save_env(&client, "p", "db", "mysql", "TZ=Asia/Jakarta").unwrap();
        save.assert();
    }

    /// An app service keeps updateEnv — and its dotEnvPath must survive the save,
    /// otherwise editing env silently turns the .env file off.
    #[test]
    fn env_of_an_app_keeps_its_dot_env_path() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.path("/api/rpc/services/app/inspectService");
            then.status(200)
                .json_body(json!({ "json": { "dotEnvPath": ".env" }, "meta": [] }));
        });
        let save = server.mock(|when, then| {
            when.path("/api/rpc/services/app/updateEnv")
                .json_body(json!({ "json": {
                    "projectName": "p", "serviceName": "web", "env": "A=1", "dotEnvPath": ".env"
                }}));
            then.status(200)
                .json_body(json!({ "json": null, "meta": [] }));
        });

        let client = EasypanelClient::new(&server.base_url(), "t");
        save_env(&client, "p", "web", "app", "A=1").unwrap();
        save.assert();
    }

    #[test]
    fn mask_token_never_panics_on_a_hand_edited_token() {
        // The token comes from a config file that can be hand-edited. The old
        // version sliced per byte: a 13-byte token with '€' (3 bytes) sitting at
        // the index-6 boundary made `server list` panic. Counting per character
        // fixes it.
        assert_eq!(mask_token("aaaaa€aaaaa"), "aaaaa€…aaaa");
        assert_eq!(mask_token("short"), "***");
        assert_eq!(
            mask_token("你好世界一二三四五六七"),
            "你好世界一二…四五六七"
        );
        // Plain ASCII behaves the same as before.
        assert_eq!(mask_token("abcdefghijklmnop"), "abcdef…mnop");
    }

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
            svc("big", "tiny", 1.0, 0.2),
            svc("big", "huge", 1_073_741_824.0, 0.5),
        ]);

        // The project with the largest total memory comes first, then its
        // services sorted by memory.
        let names: Vec<&str> = rows.iter().map(|r| r[0].as_str()).collect();
        assert_eq!(
            names,
            vec!["big (2)", "  huge", "  tiny", "small (1)", "  a"]
        );
        // Project row = the total across its services.
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
            domain_destination(&json!({ "destinationType": "unknown" })),
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
            "description": "Deploy service: first line\nsecond line ignored",
            "createdAt": "2026-07-16 05:55:15", "updatedAt": "2026-07-16 06:03:14"
        });
        let row = action_row(&a, ACTION_DESC_CLI);
        assert_eq!(row[0], "done");
        assert_eq!(row[1], "proj/api");
        assert_eq!(row[2], "Deploy service: first line");
        assert_eq!(row[3], "7 minutes"); // 05:55:15 -> 06:03:14
    }

    #[test]
    fn action_row_target_falls_back_when_not_service_scoped() {
        let login = json!({
            "status": "done", "description": "User logged in",
            "createdAt": "2026-07-16 05:55:15", "updatedAt": "2026-07-16 05:55:15"
        });
        assert_eq!(action_row(&login, ACTION_DESC_CLI)[1], "-");
        assert_eq!(action_row(&login, ACTION_DESC_CLI)[3], "0 seconds");
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
        assert_eq!(rows[0], vec!["CPU", "5.5 %"]); // last point
        assert_eq!(rows[1], vec!["Cores", "16"]);
        assert_eq!(rows[2], vec!["Load avg", "0.10, 0.20, 0.30"]);
        assert_eq!(rows[3], vec!["Memory", "25.0 % (1.0 GB / 2.0 GB)"]);
        assert_eq!(rows[4], vec!["Disk", "16.2 % (1.0 GB / 10.0 GB)"]);
        assert_eq!(rows[5], vec!["Network In", "1.0 KB/s"]);
    }

    #[test]
    fn storage_rows_sorted_by_size_desc() {
        let rows = storage_rows(vec![
            json!({ "projectName": "p", "serviceName": "tiny", "size": 1024, "path": "/a" }),
            json!({ "projectName": "p", "serviceName": "huge", "size": 1048576, "path": "/b" }),
        ]);
        assert_eq!(rows[0][1], "huge");
        assert_eq!(rows[0][2], "1.0 MB");
        assert_eq!(rows[1][1], "tiny");
    }
}
