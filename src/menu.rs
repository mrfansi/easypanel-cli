use anyhow::Result;
use dialoguer::{Confirm, Select};
use serde_json::{json, Value};

use crate::client::EasypanelClient;
use crate::commands;
use crate::config::{Server, ServerConfig};

/// Menu interaktif bertingkat: server -> kategori -> project -> service -> aksi.
pub fn run(cfg: &ServerConfig) -> Result<()> {
    if cfg.all().is_empty() {
        println!("Belum ada server. Jalankan: easypanel server add");
        return Ok(());
    }

    loop {
        let Some(server) = pick_server(cfg)? else { break };
        let client = EasypanelClient::new(&server.url, &server.token);
        let multi = cfg.all().len() > 1;
        server_menu(&client, &server.name, multi)?;
        if !multi {
            break;
        }
    }
    Ok(())
}

/// Select dengan item "kembali/keluar" di akhir; None berarti mundur satu level.
fn select(title: &str, items: &[String], back: &str) -> Result<Option<usize>> {
    let mut all = items.to_vec();
    all.push(back.to_string());

    let idx = Select::new()
        .with_prompt(title)
        .items(&all)
        .default(0)
        .interact_opt()?;

    Ok(match idx {
        Some(i) if i < items.len() => Some(i),
        _ => None, // item back, atau Esc
    })
}

fn guard(result: Result<()>) {
    if let Err(e) = result {
        eprintln!("Error: {e}");
    }
}

fn pick_server(cfg: &ServerConfig) -> Result<Option<Server>> {
    let servers = cfg.all();
    if servers.len() == 1 {
        return Ok(servers.into_iter().next().map(Some).unwrap_or(None));
    }

    let labels: Vec<String> = servers
        .iter()
        .map(|s| {
            format!(
                "{}{} — {}",
                s.name,
                if s.default { " (default)" } else { "" },
                s.url
            )
        })
        .collect();

    Ok(select("Pilih server", &labels, "Keluar")?.map(|i| servers[i].clone()))
}

fn server_menu(client: &EasypanelClient, name: &str, multi: bool) -> Result<()> {
    let items = vec![
        "Projects".to_string(),
        "Monitoring (system stats)".to_string(),
        "Node cluster".to_string(),
    ];
    let back = if multi { "Ganti server" } else { "Keluar" };

    loop {
        let Some(choice) = select(&format!("Server: {name}"), &items, back)? else {
            return Ok(());
        };
        guard(match choice {
            0 => projects_menu(client),
            1 => commands::stats(client),
            2 => commands::node_list(client),
            _ => Ok(()),
        });
    }
}

fn projects_menu(client: &EasypanelClient) -> Result<()> {
    loop {
        let projects = client.call("projects", "listProjects", Value::Null)?;
        let arr = projects.as_array().cloned().unwrap_or_default();
        if arr.is_empty() {
            println!("Tidak ada project.");
            return Ok(());
        }

        let names: Vec<String> = arr
            .iter()
            .filter_map(|p| p.get("name").and_then(Value::as_str).map(str::to_string))
            .collect();

        let Some(i) = select("Pilih project", &names, "Kembali")? else {
            return Ok(());
        };
        guard(services_menu(client, &names[i]));
    }
}

fn services_menu(client: &EasypanelClient, project: &str) -> Result<()> {
    loop {
        let data = client.call("projects", "inspectProject", json!({ "projectName": project }))?;
        let services = data
            .get("services")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if services.is_empty() {
            println!("Project tanpa service.");
            return Ok(());
        }

        let pairs: Vec<(String, String)> = services
            .iter()
            .map(|s| {
                (
                    s.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
                    s.get("type").and_then(Value::as_str).unwrap_or("app").to_string(),
                )
            })
            .collect();
        let labels: Vec<String> = pairs.iter().map(|(n, t)| format!("{n} ({t})")).collect();

        let Some(i) = select(&format!("Project: {project}"), &labels, "Kembali")? else {
            return Ok(());
        };
        let (service, stype) = pairs[i].clone();
        guard(action_menu(client, project, &service, &stype));
    }
}

fn action_menu(client: &EasypanelClient, project: &str, service: &str, stype: &str) -> Result<()> {
    let items = vec![
        "Deploy".to_string(),
        "Restart".to_string(),
        "Start".to_string(),
        "Stop".to_string(),
        "Lihat logs (100 baris)".to_string(),
    ];
    let actions = ["deploy", "restart", "start", "stop", "logs"];

    loop {
        let Some(i) = select(&format!("{project} / {service} ({stype})"), &items, "Kembali")? else {
            return Ok(());
        };
        guard(run_action(client, project, service, stype, actions[i]));
    }
}

fn run_action(
    client: &EasypanelClient,
    project: &str,
    service: &str,
    stype: &str,
    action: &str,
) -> Result<()> {
    if action == "logs" {
        return commands::service_logs(client, project, service, 100);
    }

    // Aksi yang memengaruhi service nyata butuh konfirmasi.
    if matches!(action, "deploy" | "restart" | "stop") {
        let confirmed = Confirm::new()
            .with_prompt(format!(
                "{} '{}' pada '{}'? Ini memengaruhi service nyata.",
                commands::ucfirst(action),
                service,
                project
            ))
            .default(action == "deploy")
            .interact()?;
        if !confirmed {
            return Ok(());
        }
    }

    commands::service_action(client, project, service, stype, action, false)
}
