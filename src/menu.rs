use anyhow::Result;
use dialoguer::{Confirm, Input, Select};
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
        let Some(server) = pick_server(cfg)? else {
            break;
        };
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

        let mut labels = names.clone();
        labels.push("＋ Buat project baru".to_string());

        let Some(i) = select("Pilih project", &labels, "Kembali")? else {
            return Ok(());
        };
        if i == names.len() {
            let name: String = Input::new()
                .with_prompt("Nama project baru")
                .interact_text()?;
            guard(commands::project_create(client, &name));
            continue;
        }
        guard(services_menu(client, &names[i]));
    }
}

fn services_menu(client: &EasypanelClient, project: &str) -> Result<()> {
    loop {
        let data = client.call(
            "projects",
            "inspectProject",
            json!({ "projectName": project }),
        )?;
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
            .collect();
        let mut labels: Vec<String> = pairs.iter().map(|(n, t)| format!("{n} ({t})")).collect();
        labels.push("＋ Buat service baru (app)".to_string());

        let Some(i) = select(&format!("Project: {project}"), &labels, "Kembali")? else {
            return Ok(());
        };
        if i == pairs.len() {
            let name: String = Input::new()
                .with_prompt("Nama service baru")
                .interact_text()?;
            guard(commands::service_create(client, project, &name, "app"));
            continue;
        }
        let (service, stype) = pairs[i].clone();
        guard(action_menu(client, project, &service, &stype));
    }
}

fn action_menu(client: &EasypanelClient, project: &str, service: &str, stype: &str) -> Result<()> {
    let items: Vec<String> = [
        "Deploy",
        "Restart",
        "Start",
        "Stop",
        "Lihat logs (100 baris)",
        "Lihat env",
        "Ports",
        "Mounts",
        "Domains",
        "Database backups",
        "Hapus service",
    ]
    .into_iter()
    .map(String::from)
    .collect();

    loop {
        let Some(i) = select(
            &format!("{project} / {service} ({stype})"),
            &items,
            "Kembali",
        )?
        else {
            return Ok(());
        };
        match i {
            0 => guard(run_action(client, project, service, stype, "deploy")),
            1 => guard(run_action(client, project, service, stype, "restart")),
            2 => guard(run_action(client, project, service, stype, "start")),
            3 => guard(run_action(client, project, service, stype, "stop")),
            4 => guard(commands::service_logs(client, project, service, 100)),
            5 => guard(commands::service_env(client, project, service, stype)),
            6 => guard(commands::ports_list(client, project, service)),
            7 => guard(commands::mounts_list(client, project, service)),
            8 => guard(commands::domains_list(client, project, service)),
            9 => guard(commands::db_backup_list(client, project, service)),
            10 => {
                guard(commands::service_destroy(
                    client, project, service, stype, false,
                ));
                // Service kemungkinan sudah terhapus; kembali ke daftar service.
                return Ok(());
            }
            _ => {}
        }
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
