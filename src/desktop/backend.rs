use crate::{
    client::EasypanelClient,
    config::{Server, ServerConfig},
    lifecycle, logs,
};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::mpsc::{self, Receiver, SyncSender},
};

impl ServerConfig {
    /// Save a desktop connection without changing the CLI default or SSH settings.
    /// Read and replace in one write so a rename cannot lose a credential halfway.
    pub fn save_connection(
        &self,
        original: Option<&str>,
        name: &str,
        url: &str,
        token: &str,
    ) -> Result<()> {
        let mut servers = self.try_all()?;
        if name.trim().is_empty() || token.trim().is_empty() {
            anyhow::bail!("Enter a server name and API token.");
        }
        let parsed = reqwest::Url::parse(url)?;
        if !matches!(parsed.scheme(), "http" | "https")
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
        {
            anyhow::bail!("Use an HTTP(S) panel URL without credentials, query, or fragment.");
        }
        if servers
            .iter()
            .any(|s| s.name == name && Some(s.name.as_str()) != original)
        {
            anyhow::bail!("A server called '{name}' already exists.");
        }
        if let Some(original) = original {
            let server = servers
                .iter_mut()
                .find(|s| s.name == original)
                .ok_or_else(|| {
                    anyhow::anyhow!("The connection changed. Reload connections and try again.")
                })?;
            if let Some(ssh) = &server.ssh {
                crate::tunnel::validate(ssh, url)?;
            }
            server.name = name.to_owned();
            server.url = url.trim_end_matches('/').to_owned();
            server.token = token.to_owned();
        } else {
            servers.push(Server {
                name: name.to_owned(),
                url: url.trim_end_matches('/').to_owned(),
                token: token.to_owned(),
                default: false,
                ssh: None,
            });
        }
        self.save(&servers)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Service {
    pub project: String,
    pub name: String,
    pub kind: String,
}

impl Service {
    fn input(&self) -> Value {
        json!({"projectName": self.project, "serviceName": self.name})
    }
}

#[derive(Default)]
pub struct Inventory {
    pub projects: Vec<String>,
    pub services: Vec<Service>,
    pub replicas: BTreeMap<String, (i64, i64)>,
    pub status_error: Option<String>,
}

impl Inventory {
    pub fn status(&self, service: &Service) -> String {
        match self
            .replicas
            .get(&format!("{}_{}", service.project, service.name))
        {
            Some((actual, desired)) if *desired == 0 && *actual == 0 => {
                "Stopped (0 replicas)".into()
            }
            Some((actual, desired)) if actual == desired => format!("Running ({actual}/{desired})"),
            Some((actual, desired)) => format!("Not ready ({actual}/{desired} replicas)"),
            None => "Unknown".into(),
        }
    }
}

#[derive(Clone, Default)]
pub struct ConnectionForm {
    pub original: Option<String>,
    pub name: String,
    pub url: String,
    pub token: String,
}

pub enum Job {
    Load,
    Save(ConnectionForm),
    Inventory(Server),
    Details(Server, Service),
    Logs(Server, Service),
    Action(Server, Service, &'static str),
}

pub enum Reply {
    Connections(Vec<Server>),
    Inventory(Inventory),
    Details(Option<bool>),
    Logs(String),
    Action,
}

pub struct Worker {
    tx: SyncSender<Job>,
    pub rx: Receiver<std::result::Result<Reply, String>>,
}

impl Worker {
    pub fn new(path: PathBuf, ctx: eframe::egui::Context) -> Self {
        let (tx, jobs) = mpsc::sync_channel(1);
        let (results, rx) = mpsc::sync_channel(1);
        // No join on exit: an in-flight HTTP/SSH request has its own timeout and
        // must not hold the window open. Disconnected channels end the worker.
        let spawned = std::thread::Builder::new()
            .name("desktop-api".into())
            .spawn(move || {
                while let Ok(job) = jobs.recv() {
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        execute(&path, job)
                    }))
                    .unwrap_or_else(|_| {
                        Err(anyhow::anyhow!(
                            "The request worker stopped unexpectedly. Refresh to retry."
                        ))
                    })
                    .map_err(|e| e.to_string());
                    if results.send(result).is_err() {
                        break;
                    }
                    ctx.request_repaint();
                }
            });
        // A failed spawn drops both worker channel ends; the UI reports the
        // disconnected worker on its first request rather than panicking.
        drop(spawned);
        Self { tx, rx }
    }
    #[cfg(test)]
    pub(super) fn with_result(result: std::result::Result<Reply, String>) -> Self {
        let (tx, _jobs) = mpsc::sync_channel(1);
        let (results, rx) = mpsc::sync_channel(1);
        assert!(results.send(result).is_ok());
        Self { tx, rx }
    }

    pub fn send(&self, job: Job) -> std::result::Result<(), String> {
        self.tx
            .try_send(job)
            .map_err(|_| "The request worker is unavailable. Close and reopen EasyPanel.".into())
    }
}

fn execute(path: &std::path::Path, job: Job) -> Result<Reply> {
    let config = ServerConfig::new(path.to_owned());
    match job {
        Job::Load => Ok(Reply::Connections(config.try_all()?)),
        Job::Save(form) => {
            config.save_connection(
                form.original.as_deref(),
                form.name.trim(),
                form.url.trim(),
                form.token.trim(),
            )?;
            Ok(Reply::Connections(config.try_all()?))
        }
        Job::Inventory(server) => with_client(&server, |client| {
            let data = client.call("projects", "listProjectsAndServices", Value::Null)?;
            let mut inventory = parse_inventory(&data)?;
            match client.call("monitorOld", "getDockerTaskStats", Value::Null) {
                Ok(stats) => inventory.replicas = parse_replicas(&stats),
                Err(error) => inventory.status_error = Some(error.to_string()),
            }
            Ok(Reply::Inventory(inventory))
        }),
        Job::Details(server, service) => with_client(&server, |client| {
            let data = client.call(
                &format!("services/{}", service.kind),
                "inspectService",
                service.input(),
            )?;
            if !data.is_object() {
                bail!("The panel returned invalid service details. Refresh to retry.");
            }
            Ok(Reply::Details(data.get("enabled").and_then(Value::as_bool)))
        }),
        Job::Logs(server, service) => with_client(&server, |client| {
            let data = client.call(
                "logs",
                "queryServiceLogs",
                json!({
                    "projectName": service.project, "serviceName": service.name, "limit": 200
                }),
            )?;
            let text = logs::format(&data).join("\n");
            let mut bounded: String = text.chars().take(262_144).collect();
            if text.chars().count() > 262_144 {
                bounded.push_str("\n[Display limited to 256 Ki characters]");
            }
            Ok(Reply::Logs(redact(&bounded, &[server.token.as_str()])))
        }),
        Job::Action(server, service, action) => with_client(&server, |client| {
            run_action(client, &service, action)?;
            Ok(Reply::Action)
        }),
    }
}

fn with_client(
    server: &Server,
    run: impl FnOnce(&EasypanelClient) -> Result<Reply>,
) -> Result<Reply> {
    let url =
        reqwest::Url::parse(&server.url).context("Invalid panel URL. Edit this connection.")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        bail!("Invalid panel URL. Use HTTP(S) without embedded credentials, query, or fragment.");
    }
    let secrets = [
        server.token.as_str(),
        server
            .ssh
            .as_ref()
            .and_then(|ssh| ssh.secret.as_deref())
            .unwrap_or(""),
    ];
    run(&EasypanelClient::for_server(server))
        .map_err(|e| anyhow::anyhow!(redact(&e.to_string(), &secrets)))
}

pub fn redact(text: &str, secrets: &[&str]) -> String {
    let mut text = text.to_owned();
    for secret in secrets.iter().filter(|s| !s.is_empty()) {
        text = text.replace(secret, "[redacted]");
    }
    text
}

fn parse_inventory(data: &Value) -> Result<Inventory> {
    let projects = data
        .get("projects")
        .and_then(Value::as_array)
        .context("The panel returned no project list. Refresh to retry.")?;
    let services = data
        .get("services")
        .and_then(Value::as_array)
        .context("The panel returned no service list. Refresh to retry.")?;
    let mut inventory = Inventory::default();
    for project in projects {
        inventory.projects.push(
            project
                .get("name")
                .and_then(Value::as_str)
                .context("Invalid project name in panel response.")?
                .to_owned(),
        );
    }
    for service in services {
        let get = |field| {
            service
                .get(field)
                .and_then(Value::as_str)
                .map(str::to_owned)
                .context("Invalid service in panel response.")
        };
        inventory.services.push(Service {
            project: get("projectName")?,
            name: get("name")?,
            kind: get("type")?,
        });
    }
    inventory.projects.sort();
    inventory.projects.dedup();
    inventory
        .services
        .sort_by(|a, b| (&a.project, &a.name).cmp(&(&b.project, &b.name)));
    Ok(inventory)
}

fn parse_replicas(data: &Value) -> BTreeMap<String, (i64, i64)> {
    data.as_object()
        .into_iter()
        .flatten()
        .filter_map(|(name, entry)| {
            let actual = entry.get("actual")?.as_i64()?;
            let desired = entry.get("desired")?.as_i64()?;
            (actual >= 0 && desired >= 0).then(|| (name.clone(), (actual, desired)))
        })
        .collect()
}

fn run_action(client: &EasypanelClient, service: &Service, action: &str) -> Result<()> {
    if !matches!(action, "start" | "stop" | "restart") {
        bail!("Unsupported desktop action.");
    }
    let operations = lifecycle::ops(&service.kind, action)
        .context("This service does not support that action.")?;
    for operation in operations {
        client.call(&format!("services/{}", service.kind), operation, service.input())
            .map_err(|error| anyhow::anyhow!("{action} did not complete at {operation}: {error}; refresh status before retrying"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;
    use std::fs;

    fn temp_config() -> (tempfile::TempDir, ServerConfig) {
        let dir = tempfile::tempdir().unwrap();
        let path = path_of(&dir);
        (dir, ServerConfig::new(path))
    }

    fn path_of(dir: &tempfile::TempDir) -> PathBuf {
        dir.path().join("servers.json")
    }
    #[test]
    fn desktop_edit_preserves_ssh_and_defaults_without_duplicate_overwrite() {
        let (dir, cfg) = temp_config();
        let hop = crate::tunnel::SshTunnel {
            host: "ssh.example.com".into(),
            ..Default::default()
        };
        cfg.add(
            "prod",
            "http://localhost:3000",
            "old-token",
            Some(hop.clone()),
        )
        .unwrap();
        cfg.save_connection(
            Some("prod"),
            "renamed",
            "http://localhost:4000/",
            "new-token",
        )
        .unwrap();
        let server = cfg.get("renamed").unwrap();
        assert_eq!(server.ssh, Some(hop));
        assert!(server.default);
        assert_eq!(server.url, "http://localhost:4000");
        let before = fs::read(path_of(&dir)).unwrap();
        assert!(cfg
            .save_connection(None, "renamed", "https://other.test", "other-token")
            .is_err());
        assert_eq!(fs::read(path_of(&dir)).unwrap(), before);
        assert!(cfg
            .save_connection(Some("renamed"), "renamed", "https://remote.test", "token")
            .is_err());
        assert_eq!(fs::read(path_of(&dir)).unwrap(), before);
        cfg.save_connection(None, "extra", "http://localhost:3000", "extra-token")
            .unwrap();
        assert_eq!(cfg.default().unwrap().name, "renamed");
    }

    #[test]
    fn desktop_never_overwrites_corrupt_config_or_chooses_a_cli_default() {
        let (dir, cfg) = temp_config();
        cfg.save_connection(None, "first", "https://panel.test", "token")
            .unwrap();
        assert!(cfg.default().is_none());
        fs::write(path_of(&dir), "not json").unwrap();
        assert!(cfg
            .save_connection(None, "second", "https://panel.test", "token")
            .is_err());
        assert_eq!(fs::read_to_string(path_of(&dir)).unwrap(), "not json");
    }

    #[test]
    fn desktop_rejects_unsafe_urls_before_writing_credentials() {
        let (dir, cfg) = temp_config();
        for url in [
            "file:///tmp/panel",
            "https://user:password@panel.test",
            "https://panel.test/?token=secret",
            "https://panel.test/#secret",
        ] {
            assert!(cfg.save_connection(None, "unsafe", url, "token").is_err());
        }
        assert!(!path_of(&dir).exists());
    }

    #[test]
    fn runtime_status_never_treats_enabled_configuration_as_running() {
        let mut inventory = parse_inventory(&json!({"projects":[{"name":"p"}],"services":[{"projectName":"p","name":"db","type":"mysql","enabled":true}]})).unwrap();
        assert_eq!(inventory.status(&inventory.services[0]), "Unknown");
        inventory.replicas = parse_replicas(&json!({"p_db":{"actual":0,"desired":1}}));
        assert_eq!(
            inventory.status(&inventory.services[0]),
            "Not ready (0/1 replicas)"
        );
        assert!(parse_replicas(&json!({"p_db":{"actual":-1,"desired":1}})).is_empty());
    }

    #[test]
    fn database_restart_stops_at_first_failure() {
        let server = MockServer::start();
        let disable = server.mock(|when, then| {
            when.method(POST)
                .path("/api/rpc/services/mysql/disableService");
            then.status(400)
                .json_body(json!({"json":{"message":"denied"}}));
        });
        let enable = server.mock(|when, then| {
            when.method(POST)
                .path("/api/rpc/services/mysql/enableService");
            then.status(200).json_body(json!({"json":null}));
        });
        let client = EasypanelClient::new(&server.base_url(), "token");
        let service = Service {
            project: "p".into(),
            name: "db".into(),
            kind: "mysql".into(),
        };
        assert!(run_action(&client, &service, "restart").is_err());
        disable.assert_hits(1);
        enable.assert_hits(0);
    }
    #[test]
    fn database_restart_reports_failed_enable_after_successful_disable() {
        let server = MockServer::start();
        let disable = server.mock(|when, then| {
            when.method(POST)
                .path("/api/rpc/services/mysql/disableService");
            then.status(200).json_body(json!({"json":null}));
        });
        let enable = server.mock(|when, then| {
            when.method(POST)
                .path("/api/rpc/services/mysql/enableService");
            then.status(400)
                .json_body(json!({"json":{"message":"cannot enable"}}));
        });
        let client = EasypanelClient::new(&server.base_url(), "token");
        let service = Service {
            project: "p".into(),
            name: "db".into(),
            kind: "mysql".into(),
        };
        let error = run_action(&client, &service, "restart")
            .unwrap_err()
            .to_string();
        assert!(error.contains("enableService") && error.contains("cannot enable"));
        disable.assert_hits(1);
        enable.assert_hits(1);
    }

    #[test]
    fn credentials_are_removed_from_displayed_text() {
        assert_eq!(
            redact("failure secret123", &["", "secret123"]),
            "failure [redacted]"
        );
    }

    #[test]
    fn malformed_inventory_is_an_error_not_an_empty_success() {
        assert!(parse_inventory(&json!({"unexpected":[]})).is_err());
        assert!(parse_inventory(&json!({"projects":[],"services":[{}]})).is_err());
        assert!(parse_inventory(&json!({"projects":[],"services":[]}))
            .unwrap()
            .services
            .is_empty());
    }
}
