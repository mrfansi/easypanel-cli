//! Cloudflare — a bounded context OUTSIDE EasyPanel: manage one or more Cloudflare
//! accounts' zones and DNS records. Nothing here touches the EasyPanel domain; the two
//! share only the TUI event loop and the config directory (separate files).

use std::collections::HashMap;

use anyhow::{anyhow, Result};
use chrono::{Duration, Utc};
use serde::de::{DeserializeOwned, Deserializer};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// A stored Cloudflare account: a user-labelled scoped API token, kept in cloudflare.json
/// independent of any EasyPanel server (an operator may hold several CF accounts).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudflareAccount {
    pub name: String,
    pub api_token: String,
    /// Needed only to CREATE a zone; not needed to list zones or manage records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    #[serde(default)]
    pub default: bool,
}

// ---------- Domain types (Cloudflare API shapes) ----------

#[derive(Debug, Clone, Deserialize)]
pub struct Zone {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub status: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Record {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub name: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub ttl: u32,
    #[serde(default)]
    pub proxied: bool,
    #[serde(default)]
    pub priority: Option<u16>,
}

/// An R2 bucket (Cloudflare's REST view, not the S3 API). Object browsing is a
/// separate future slice — this slice manages buckets only.
#[derive(Debug, Clone, Deserialize)]
pub struct R2Bucket {
    pub name: String,
    #[serde(default)]
    pub creation_date: String,
    /// Optional — the list endpoint may omit it.
    #[serde(default)]
    pub location: Option<String>,
    #[serde(default)]
    pub storage_class: String,
    #[serde(default)]
    pub jurisdiction: String,
}

/// One Cloudflare Worker script at account scope. The list/content/delete/upload APIs
/// live under `accounts/{account_id}/workers/scripts`, so Workers sits beside R2 as an
/// account product, not under DNS zones.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct WorkerScript {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub created_on: String,
    #[serde(default)]
    pub modified_on: String,
    #[serde(default)]
    pub etag: String,
    #[serde(default)]
    pub handlers: Vec<String>,
    #[serde(default)]
    pub usage_model: String,
}

/// One version entry inside a Worker deployment. Cloudflare lets a deployment point
/// to one version at 100% traffic, or split traffic across two versions.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct WorkerDeploymentVersion {
    #[serde(default)]
    pub percentage: f64,
    #[serde(default)]
    pub version_id: String,
}

/// Free-form metadata Cloudflare attaches to Worker deployments. The dashboard uses
/// these annotations for commit/PR messages and trigger source labels.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct WorkerDeploymentAnnotations {
    #[serde(default, rename = "workers/message")]
    pub message: String,
    #[serde(default, rename = "workers/triggered_by")]
    pub triggered_by: String,
}

/// A Workers deployment/version-history row. The latest deployment returned by
/// Cloudflare is the active deployment.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct WorkerDeployment {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub created_on: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub strategy: String,
    #[serde(default)]
    pub versions: Vec<WorkerDeploymentVersion>,
    #[serde(default)]
    pub annotations: Option<WorkerDeploymentAnnotations>,
    #[serde(default)]
    pub author_email: String,
}

impl WorkerDeployment {
    pub fn short_id(&self) -> String {
        self.id.chars().take(8).collect()
    }

    pub fn message(&self) -> &str {
        self.annotations
            .as_ref()
            .map(|a| a.message.as_str())
            .unwrap_or("")
    }

    pub fn triggered_by(&self) -> &str {
        self.annotations
            .as_ref()
            .map(|a| a.triggered_by.as_str())
            .unwrap_or("")
    }

    pub fn versions_label(&self) -> String {
        if self.versions.is_empty() {
            return "-".into();
        }
        self.versions
            .iter()
            .map(|v| {
                let short: String = v.version_id.chars().take(8).collect();
                format!("{short} {}%", trim_percent(v.percentage))
            })
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// One binding from a Worker's version settings. Cloudflare has many binding
/// variants; keeping the common identifiers plus `extra` lets new binding types
/// render without needing a code release.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WorkerSettingBinding {
    #[serde(default)]
    pub name: String,
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub json: Option<Value>,
    #[serde(default)]
    pub service: String,
    #[serde(default)]
    pub environment: String,
    #[serde(default)]
    pub namespace: String,
    #[serde(default)]
    pub queue_name: String,
    #[serde(default)]
    pub bucket_name: String,
    #[serde(default)]
    pub database_id: String,
    #[serde(default)]
    pub id: String,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

impl WorkerSettingBinding {
    pub fn display_type(&self) -> String {
        self.kind.replace('_', " ")
    }

    pub fn display_value(&self) -> String {
        match self.kind.as_str() {
            "plain_text" => empty_or_dash(&self.text),
            "secret_text" => "secret".into(),
            "json" => self
                .json
                .as_ref()
                .map(short_json)
                .unwrap_or_else(|| "-".into()),
            "service" => {
                if self.environment.is_empty() {
                    empty_or_dash(&self.service)
                } else {
                    format!("{} / {}", empty_or_dash(&self.service), self.environment)
                }
            }
            "assets" | "images" => format!("{} binding", self.kind.replace('_', " ")),
            _ => [
                self.service.as_str(),
                self.namespace.as_str(),
                self.queue_name.as_str(),
                self.bucket_name.as_str(),
                self.database_id.as_str(),
                self.id.as_str(),
            ]
            .into_iter()
            .find(|v| !v.trim().is_empty())
            .map(str::to_string)
            .or_else(|| self.extra.values().next().map(short_json))
            .unwrap_or_else(|| "-".into()),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WorkerSecretBinding {
    #[serde(default)]
    pub name: String,
    #[serde(rename = "type", default)]
    pub kind: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WorkerSchedule {
    #[serde(default)]
    pub cron: String,
    #[serde(default)]
    pub created_on: String,
    #[serde(default)]
    pub modified_on: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WorkerTailConsumer {
    #[serde(default)]
    pub service: String,
    #[serde(default)]
    pub environment: String,
    #[serde(default)]
    pub namespace: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WorkerSamplingSettings {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub invocation_logs: Option<bool>,
    #[serde(default, deserialize_with = "vec_or_null")]
    pub destinations: Vec<String>,
    #[serde(default)]
    pub head_sampling_rate: Option<f64>,
    #[serde(default)]
    pub persist: Option<bool>,
    #[serde(default)]
    pub propagation_policy: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WorkerObservability {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub head_sampling_rate: Option<f64>,
    #[serde(default)]
    pub logs: Option<WorkerSamplingSettings>,
    #[serde(default)]
    pub traces: Option<WorkerSamplingSettings>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WorkerScriptSettings {
    #[serde(default)]
    pub logpush: Option<bool>,
    #[serde(default)]
    pub observability: Option<WorkerObservability>,
    #[serde(default, deserialize_with = "vec_or_null")]
    pub tags: Vec<String>,
    #[serde(default, deserialize_with = "vec_or_null")]
    pub tail_consumers: Vec<WorkerTailConsumer>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WorkerVersionSettings {
    #[serde(default, deserialize_with = "vec_or_null")]
    pub bindings: Vec<WorkerSettingBinding>,
    #[serde(default)]
    pub compatibility_date: String,
    #[serde(default, deserialize_with = "vec_or_null")]
    pub compatibility_flags: Vec<String>,
    #[serde(default)]
    pub usage_model: String,
    #[serde(default)]
    pub placement: Option<Value>,
    #[serde(default)]
    pub cache_options: Option<Value>,
    #[serde(default)]
    pub limits: Option<Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct WorkerSettingsBundle {
    pub version: WorkerVersionSettings,
    pub script: WorkerScriptSettings,
    pub secrets: Vec<WorkerSecretBinding>,
    pub schedules: Vec<WorkerSchedule>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct WorkerSettingsRow {
    pub section: String,
    pub name: String,
    pub value: String,
}

impl WorkerSettingsBundle {
    pub fn rows(&self, worker: &WorkerScript) -> Vec<WorkerSettingsRow> {
        let mut rows = Vec::new();
        push_row(&mut rows, "General", "Name", &worker.id);
        push_row(
            &mut rows,
            "General",
            "Usage model",
            &first_non_empty(&[
                self.version.usage_model.as_str(),
                worker.usage_model.as_str(),
            ]),
        );
        push_row(
            &mut rows,
            "General",
            "Modified",
            &short_cf_date(&worker.modified_on),
        );

        if self.version.bindings.is_empty() && self.secrets.is_empty() {
            push_row(&mut rows, "Variables and secrets", "Bindings", "-");
        } else {
            for binding in &self.version.bindings {
                push_row(
                    &mut rows,
                    "Variables and secrets",
                    &format!(
                        "{} ({})",
                        empty_or_dash(&binding.name),
                        binding.display_type()
                    ),
                    &binding.display_value(),
                );
            }
            for secret in &self.secrets {
                push_row(
                    &mut rows,
                    "Variables and secrets",
                    &format!(
                        "{} ({})",
                        empty_or_dash(&secret.name),
                        empty_or_dash(&secret.kind)
                    ),
                    "secret",
                );
            }
        }

        if worker.handlers.iter().any(|h| h == "fetch") {
            push_row(&mut rows, "Trigger events", "Service fetch()", &worker.id);
        }
        if self.schedules.is_empty() {
            push_row(&mut rows, "Trigger events", "Cron scheduled()", "-");
        } else {
            for schedule in &self.schedules {
                push_row(
                    &mut rows,
                    "Trigger events",
                    "Cron scheduled()",
                    &schedule.cron,
                );
            }
        }

        if let Some(obs) = &self.script.observability {
            push_row(
                &mut rows,
                "Observability",
                "Observability",
                &bool_label(obs.enabled),
            );
            if let Some(logs) = &obs.logs {
                push_row(&mut rows, "Observability", "Logs", &sampling_label(logs));
            }
            if let Some(traces) = &obs.traces {
                push_row(
                    &mut rows,
                    "Observability",
                    "Traces",
                    &sampling_label(traces),
                );
            }
            push_row(
                &mut rows,
                "Observability",
                "Sampling",
                &obs.head_sampling_rate
                    .map(percent_label)
                    .unwrap_or_else(|| "-".into()),
            );
        } else {
            push_row(&mut rows, "Observability", "Observability", "-");
        }
        push_row(
            &mut rows,
            "Observability",
            "Logpush",
            &bool_label(self.script.logpush),
        );
        if self.script.tail_consumers.is_empty() {
            push_row(&mut rows, "Observability", "Tail Worker", "-");
        } else {
            for tail in &self.script.tail_consumers {
                let value = first_non_empty(&[
                    tail.service.as_str(),
                    tail.namespace.as_str(),
                    tail.environment.as_str(),
                ]);
                push_row(&mut rows, "Observability", "Tail Worker", &value);
            }
        }

        push_row(
            &mut rows,
            "Runtime",
            "Compatibility date",
            &empty_or_dash(&self.version.compatibility_date),
        );
        push_row(
            &mut rows,
            "Runtime",
            "Compatibility flags",
            &join_or_dash(&self.version.compatibility_flags),
        );
        push_row(
            &mut rows,
            "Runtime",
            "Placement",
            &self
                .version
                .placement
                .as_ref()
                .map(short_json)
                .unwrap_or_else(|| "-".into()),
        );
        push_row(
            &mut rows,
            "Runtime",
            "Cache",
            &self
                .version
                .cache_options
                .as_ref()
                .map(short_json)
                .unwrap_or_else(|| "-".into()),
        );
        push_row(
            &mut rows,
            "Runtime",
            "Limits",
            &self
                .version
                .limits
                .as_ref()
                .map(short_json)
                .unwrap_or_else(|| "-".into()),
        );

        push_row(&mut rows, "Build", "Repository/build config", "-");
        if !self.script.tags.is_empty() {
            push_row(
                &mut rows,
                "General",
                "Tags",
                &join_or_dash(&self.script.tags),
            );
        }
        rows
    }
}

#[derive(Debug, Deserialize)]
struct WorkerDeploymentsResult {
    #[serde(default)]
    deployments: Vec<WorkerDeployment>,
}

#[derive(Debug, Deserialize)]
struct WorkerSchedulesResult {
    #[serde(default, deserialize_with = "vec_or_null")]
    schedules: Vec<WorkerSchedule>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerUploadMode {
    Module,
    ServiceWorker,
}

fn trim_percent(value: f64) -> String {
    let mut s = format!("{value:.2}");
    while s.contains('.') && s.ends_with('0') {
        s.pop();
    }
    if s.ends_with('.') {
        s.pop();
    }
    s
}

impl WorkerUploadMode {
    pub fn metadata_key(self) -> &'static str {
        match self {
            Self::Module => "main_module",
            Self::ServiceWorker => "body_part",
        }
    }
}

/// The list-buckets `result` is NOT a bare array: it is an object wrapping a
/// `buckets` array. Deserialize `result` into this, not `Vec<R2Bucket>`.
#[derive(Debug, Deserialize)]
struct BucketsResult {
    #[serde(default)]
    buckets: Vec<R2Bucket>,
}

/// One object inside an R2 bucket, from the Cloudflare REST API (`GET
/// /accounts/{account_id}/r2/buckets/{bucket}/objects`) — the SAME Bearer token as
/// buckets/DNS, no S3 credentials. `result` is a bare array of these. Browsing only for
/// now; upload/download/delete are a later slice. serde ignores the other keys the API
/// sends (etag/custom_metadata/http_metadata/ssec).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct R2Object {
    pub key: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub last_modified: String,
    #[serde(default)]
    pub storage_class: String,
}

/// Cloudflare's pagination block. `total_pages` drives the DNS page loop; `cursor` +
/// `is_truncated` drive R2's cursor pagination (`is_truncated` is the authoritative
/// "more pages" flag). serde ignores the other keys (page/per_page/count/total_count)
/// the API also sends.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ResultInfo {
    #[serde(default)]
    pub total_pages: u32,
    /// R2's next-page cursor: pass it back as the `cursor` query param.
    #[serde(default)]
    pub cursor: Option<String>,
    /// R2 objects: true while more pages remain. Absent (buckets) defaults to false.
    #[serde(default)]
    pub is_truncated: bool,
    /// R2 delimiter-mode common prefixes: the subfolders at this level. Present only
    /// when the request carries `delimiter=/`; each entry is a FULL key prefix ending
    /// in `/` (e.g. `assets/css/`). Absent otherwise → empty.
    #[serde(default)]
    pub delimited: Vec<String>,
}

/// One level of an R2 bucket browsed as a folder tree: the subfolders at this level
/// (`folders`, full key prefixes ending in `/`) and the files directly here (`files`).
/// `truncated` is set when a single level held more than one page. Built by
/// [`CloudflareClient::list_r2_level`] with `delimiter=/`, so `/`-delimited object keys
/// browse as nested folders instead of one flat 1000-row dump.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct R2Level {
    pub folders: Vec<String>,
    pub files: Vec<R2Object>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CloudflareTunnel {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub deleted_at: Option<String>,
    #[serde(default)]
    pub tun_type: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub config_src: String,
    #[serde(default)]
    pub conns_active_at: Option<String>,
    #[serde(default)]
    pub conns_inactive_at: Option<String>,
    #[serde(default, deserialize_with = "vec_or_null")]
    pub connections: Vec<TunnelConnection>,
}

impl CloudflareTunnel {
    pub fn is_active(&self) -> bool {
        self.deleted_at.is_none()
            && (self.conns_active_at.is_some()
                || self.status.eq_ignore_ascii_case("healthy")
                || self.status.eq_ignore_ascii_case("active"))
    }

    pub fn status_label(&self) -> String {
        if self.deleted_at.is_some() {
            "deleted".into()
        } else if !self.status.trim().is_empty() {
            self.status.clone()
        } else if self.is_active() {
            "active".into()
        } else {
            "inactive".into()
        }
    }

    pub fn target(&self) -> String {
        format!("{}.cfargotunnel.com", self.id)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TunnelConnection {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub client_id: String,
    #[serde(default)]
    pub client_version: String,
    #[serde(default)]
    pub colo_name: String,
    #[serde(default)]
    pub origin_ip: String,
    #[serde(default)]
    pub opened_at: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TunnelConfiguration {
    #[serde(default)]
    pub account_id: String,
    #[serde(default)]
    pub tunnel_id: String,
    #[serde(default)]
    pub version: Option<u64>,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub config: TunnelConfig,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TunnelConfig {
    #[serde(default, deserialize_with = "vec_or_null")]
    pub ingress: Vec<TunnelIngressRule>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "originRequest"
    )]
    pub origin_request: Option<Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TunnelIngressRule {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub hostname: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub path: String,
    #[serde(default)]
    pub service: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "originRequest"
    )]
    pub origin_request: Option<Value>,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

impl TunnelIngressRule {
    pub fn route(hostname: &str, service: &str, path: &str, origin_request: Option<Value>) -> Self {
        Self {
            hostname: hostname.trim().to_string(),
            path: path.trim().to_string(),
            service: service.trim().to_string(),
            origin_request,
            extra: HashMap::new(),
        }
    }

    pub fn catch_all() -> Self {
        Self {
            hostname: String::new(),
            path: String::new(),
            service: "http_status:404".into(),
            origin_request: None,
            extra: HashMap::new(),
        }
    }

    pub fn is_catch_all(&self) -> bool {
        self.hostname.trim().is_empty()
    }

    pub fn hostname_label(&self) -> String {
        if self.hostname.trim().is_empty() {
            "catch-all".into()
        } else if self.path.trim().is_empty() {
            self.hostname.clone()
        } else {
            format!("{}{}", self.hostname, self.path)
        }
    }

    pub fn origin_label(&self) -> String {
        self.origin_request
            .as_ref()
            .map(short_json)
            .unwrap_or_else(|| "-".into())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct TunnelConfigRow {
    pub hostname: String,
    pub service: String,
    pub origin: String,
}

impl TunnelConfiguration {
    pub fn rows(&self) -> Vec<TunnelConfigRow> {
        self.config
            .ingress
            .iter()
            .map(|rule| TunnelConfigRow {
                hostname: rule.hostname_label(),
                service: empty_or_dash(&rule.service),
                origin: rule.origin_label(),
            })
            .collect()
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct TunnelRouteChange {
    pub hostname: String,
    pub service: Option<String>,
    pub path: Option<String>,
    pub origin_request: Option<Option<Value>>,
}

const TUNNEL_SERVICE_PREFIXES: &[&str] = &[
    "http://",
    "https://",
    "unix:",
    "unix://",
    "tcp://",
    "ssh://",
    "rdp://",
    "unix+tls:",
    "unix+tls://",
    "smb://",
    "http_status:",
    "bastion",
    "hello_world",
];

pub fn parse_tunnel_origin_request(input: &str) -> Result<Option<Value>> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let value: Value = serde_json::from_str(trimmed)
        .map_err(|e| anyhow!("origin request JSON is invalid: {e}"))?;
    if !value.is_object() {
        return Err(anyhow!("origin request JSON must be an object"));
    }
    Ok(Some(value))
}

pub fn validate_tunnel_route(hostname: &str, service: &str) -> Result<()> {
    if hostname.trim().is_empty() {
        return Err(anyhow!("Hostname is required"));
    }
    if service.trim().is_empty() {
        return Err(anyhow!("Service is required"));
    }
    if !TUNNEL_SERVICE_PREFIXES
        .iter()
        .any(|prefix| service.starts_with(prefix))
    {
        return Err(anyhow!(
            "Service must start with one of: {}",
            TUNNEL_SERVICE_PREFIXES.join(", ")
        ));
    }
    Ok(())
}

pub fn add_tunnel_route(config: &mut TunnelConfig, rule: TunnelIngressRule) -> Result<()> {
    validate_tunnel_route(&rule.hostname, &rule.service)?;
    if config.ingress.iter().any(|r| {
        !r.is_catch_all() && r.hostname.eq_ignore_ascii_case(&rule.hostname) && r.path == rule.path
    }) {
        return Err(anyhow!(
            "Route '{}' already exists",
            route_key(&rule.hostname, &rule.path)
        ));
    }
    let insert_at = config
        .ingress
        .iter()
        .position(TunnelIngressRule::is_catch_all)
        .unwrap_or(config.ingress.len());
    config.ingress.insert(insert_at, rule);
    normalize_tunnel_ingress(config);
    Ok(())
}

pub fn edit_tunnel_route(config: &mut TunnelConfig, change: TunnelRouteChange) -> Result<()> {
    let index = find_tunnel_route_index(config, &change.hostname, change.path.as_deref())?;
    if let Some(service) = &change.service {
        validate_tunnel_route(&change.hostname, service)?;
        config.ingress[index].service = service.trim().to_string();
    }
    if let Some(path) = &change.path {
        config.ingress[index].path = path.trim().to_string();
    }
    if let Some(origin_request) = change.origin_request {
        config.ingress[index].origin_request = origin_request;
    }
    normalize_tunnel_ingress(config);
    Ok(())
}

pub fn delete_tunnel_route(
    config: &mut TunnelConfig,
    hostname: &str,
    path: Option<&str>,
) -> Result<()> {
    let index = find_tunnel_route_index(config, hostname, path)?;
    config.ingress.remove(index);
    normalize_tunnel_ingress(config);
    Ok(())
}

fn find_tunnel_route_index(
    config: &TunnelConfig,
    hostname: &str,
    path: Option<&str>,
) -> Result<usize> {
    let matches = config
        .ingress
        .iter()
        .enumerate()
        .filter(|(_, rule)| {
            !rule.is_catch_all()
                && rule.hostname.eq_ignore_ascii_case(hostname.trim())
                && path.map(|p| rule.path == p.trim()).unwrap_or(true)
        })
        .map(|(i, _)| i)
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [index] => Ok(*index),
        [] => Err(anyhow!("No route found for '{}'", hostname.trim())),
        _ => Err(anyhow!(
            "Multiple routes use '{}'; pass --path to choose one",
            hostname.trim()
        )),
    }
}

fn normalize_tunnel_ingress(config: &mut TunnelConfig) {
    let catch_all = config
        .ingress
        .iter()
        .rev()
        .find(|rule| rule.is_catch_all())
        .cloned()
        .unwrap_or_else(TunnelIngressRule::catch_all);
    config.ingress.retain(|rule| !rule.is_catch_all());
    config.ingress.push(catch_all);
}

fn route_key(hostname: &str, path: &str) -> String {
    if path.trim().is_empty() {
        hostname.trim().to_string()
    } else {
        format!("{}{}", hostname.trim(), path.trim())
    }
}

/// Sort a level for display: folders alphabetically (ascending), files newest-first
/// (by `last_modified`, ISO-8601 so a string compare is chronological). Pure, so the
/// ordering is unit-tested from a fixture without a live call.
pub fn sort_r2_level(level: &mut R2Level) {
    level.folders.sort();
    level
        .files
        .sort_by(|a, b| b.last_modified.cmp(&a.last_modified));
}

/// The status-bar feedback for marked rows — EasyPanel's exact wording
/// (`report_marks`), with the noun swapped per screen ("record" on DNS records,
/// "file" on R2 objects). Pure, so the wording parity is unit-tested.
pub fn marks_status(noun: &str, n: usize) -> String {
    format!("{n} {noun}(s) marked — [Space] to act on them, [Esc] to clear")
}

/// The Cloudflare REST object endpoints (`PUT`/`GET`/`DELETE …/objects/{key}`) cap a
/// single request at 300 MB. Larger objects need the S3 API (which this tool uses only
/// for DB dumps), so an oversized upload is rejected up front rather than attempted.
pub const MAX_REST_OBJECT_BYTES: u64 = 300 * 1024 * 1024;

/// Percent-encode an object key for the REST path. Slashes stay LITERAL so the key still
/// browses as a folder tree (`dir/a.gz`, not `dir%2Fa.gz`); every byte outside the
/// unreserved set `A-Za-z0-9-._~` becomes `%XX` (uppercase hex). Pure.
pub fn encode_object_key(key: &str) -> String {
    let mut out = String::with_capacity(key.len());
    for &b in key.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// The last path segment of an object key (the whole key when it has no `/`) — used as the
/// default local filename on download. Pure.
pub fn object_basename(key: &str) -> &str {
    key.rsplit('/').next().unwrap_or(key)
}

/// The destination object key for an upload: `prefix` (already `""` or ending in `/`) plus
/// the OS basename of `local_path` (any directory stripped). Pure. Used by the TUI (Phase
/// B) to upload into the currently-browsed folder; the CLI `put` takes an explicit key.
pub fn upload_key(prefix: &str, local_path: &str) -> String {
    let base = std::path::Path::new(local_path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| local_path.to_string());
    format!("{prefix}{base}")
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct AnalyticsMetric {
    pub label: String,
    pub value: u64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct CountryTraffic {
    pub country: String,
    pub requests: u64,
    pub bandwidth: u64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct AnalyticsSummary {
    pub days: u16,
    pub requests: u64,
    pub bandwidth: u64,
    pub visits: u64,
    pub countries: Vec<CountryTraffic>,
    pub ssl: Vec<AnalyticsMetric>,
    pub cache: Vec<AnalyticsMetric>,
    pub status: Vec<AnalyticsMetric>,
    pub protocols: Vec<AnalyticsMetric>,
    pub content_types: Vec<AnalyticsMetric>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct WebAnalyticsSite {
    pub site_tag: String,
    pub site_token: String,
    pub host: String,
    pub created: String,
    pub auto_install: bool,
    pub enabled: bool,
    pub zone_name: String,
    pub zone_tag: String,
    pub page_views_24h: Option<u64>,
    pub visits_24h: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct WebAnalyticsRule {
    #[serde(default)]
    host: String,
    #[serde(default)]
    is_paused: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct WebAnalyticsRuleset {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    zone_name: String,
    #[serde(default)]
    zone_tag: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct WebAnalyticsSiteRaw {
    #[serde(default)]
    auto_install: bool,
    #[serde(default)]
    created: String,
    #[serde(default)]
    rules: Vec<WebAnalyticsRule>,
    #[serde(default)]
    ruleset: Option<WebAnalyticsRuleset>,
    #[serde(default)]
    site_tag: String,
    #[serde(default)]
    site_token: String,
    #[serde(default)]
    page_views_24h: Option<u64>,
    #[serde(default)]
    visits_24h: Option<u64>,
}

impl From<WebAnalyticsSiteRaw> for WebAnalyticsSite {
    fn from(raw: WebAnalyticsSiteRaw) -> Self {
        let ruleset = raw.ruleset.unwrap_or_default();
        let first_rule = raw.rules.iter().find(|r| !r.host.is_empty());
        let host = if !ruleset.zone_name.is_empty() {
            ruleset.zone_name.clone()
        } else {
            first_rule.map(|r| r.host.clone()).unwrap_or_default()
        };
        let enabled = ruleset.enabled || raw.rules.iter().any(|r| !r.is_paused);
        Self {
            site_tag: raw.site_tag,
            site_token: raw.site_token,
            host,
            created: raw.created,
            auto_install: raw.auto_install,
            enabled,
            zone_name: ruleset.zone_name,
            zone_tag: ruleset.zone_tag,
            page_views_24h: raw.page_views_24h,
            visits_24h: raw.visits_24h,
        }
    }
}

fn graphql_error(body: &str) -> Option<String> {
    let v: Value = serde_json::from_str(body).ok()?;
    v.get("errors")
        .and_then(Value::as_array)
        .and_then(|errs| errs.first())
        .and_then(|e| e.get("message"))
        .and_then(Value::as_str)
        .map(|s| format!("Cloudflare GraphQL: {s}"))
}

fn group_count(g: &Value) -> u64 {
    g.get("count").and_then(Value::as_u64).unwrap_or(0)
}

fn group_sum(g: &Value, key: &str) -> u64 {
    g.get("sum")
        .and_then(|s| s.get(key))
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

fn group_dimension(g: &Value, key: &str) -> String {
    let Some(value) = g.get("dimensions").and_then(|d| d.get(key)) else {
        return "-".into();
    };
    match value {
        Value::String(s) if !s.is_empty() => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        _ => "-".into(),
    }
}

fn country_label(raw: &str) -> String {
    match raw {
        "AD" => "Andorra",
        "AE" => "United Arab Emirates",
        "AF" => "Afghanistan",
        "AG" => "Antigua and Barbuda",
        "AI" => "Anguilla",
        "AL" => "Albania",
        "AM" => "Armenia",
        "AO" => "Angola",
        "AR" => "Argentina",
        "AT" => "Austria",
        "AU" => "Australia",
        "AZ" => "Azerbaijan",
        "BA" => "Bosnia and Herzegovina",
        "BB" => "Barbados",
        "BD" => "Bangladesh",
        "BE" => "Belgium",
        "BF" => "Burkina Faso",
        "BG" => "Bulgaria",
        "BH" => "Bahrain",
        "BI" => "Burundi",
        "BJ" => "Benin",
        "BN" => "Brunei",
        "BO" => "Bolivia",
        "BR" => "Brazil",
        "BS" => "Bahamas",
        "BT" => "Bhutan",
        "BW" => "Botswana",
        "BY" => "Belarus",
        "BZ" => "Belize",
        "CA" => "Canada",
        "CD" => "Congo, Democratic Republic",
        "CG" => "Congo",
        "CH" => "Switzerland",
        "CI" => "Cote d'Ivoire",
        "CL" => "Chile",
        "CM" => "Cameroon",
        "CN" => "China",
        "CO" => "Colombia",
        "CR" => "Costa Rica",
        "CU" => "Cuba",
        "CV" => "Cabo Verde",
        "CY" => "Cyprus",
        "CZ" => "Czechia",
        "DE" => "Germany",
        "DK" => "Denmark",
        "DO" => "Dominican Republic",
        "DZ" => "Algeria",
        "EC" => "Ecuador",
        "EE" => "Estonia",
        "EG" => "Egypt",
        "ES" => "Spain",
        "ET" => "Ethiopia",
        "FI" => "Finland",
        "FR" => "France",
        "GB" => "United Kingdom",
        "GE" => "Georgia",
        "GH" => "Ghana",
        "GR" => "Greece",
        "GT" => "Guatemala",
        "HK" => "Hong Kong",
        "HN" => "Honduras",
        "HR" => "Croatia",
        "HU" => "Hungary",
        "ID" => "Indonesia",
        "IE" => "Ireland",
        "IL" => "Israel",
        "IN" => "India",
        "IQ" => "Iraq",
        "IR" => "Iran",
        "IS" => "Iceland",
        "IT" => "Italy",
        "JM" => "Jamaica",
        "JO" => "Jordan",
        "JP" => "Japan",
        "KE" => "Kenya",
        "KH" => "Cambodia",
        "KR" => "South Korea",
        "KW" => "Kuwait",
        "KZ" => "Kazakhstan",
        "LA" => "Laos",
        "LB" => "Lebanon",
        "LK" => "Sri Lanka",
        "LT" => "Lithuania",
        "LU" => "Luxembourg",
        "LV" => "Latvia",
        "MA" => "Morocco",
        "MD" => "Moldova",
        "MG" => "Madagascar",
        "MM" => "Myanmar",
        "MN" => "Mongolia",
        "MO" => "Macao",
        "MT" => "Malta",
        "MU" => "Mauritius",
        "MX" => "Mexico",
        "MY" => "Malaysia",
        "NG" => "Nigeria",
        "NL" => "Netherlands",
        "NO" => "Norway",
        "NP" => "Nepal",
        "NZ" => "New Zealand",
        "OM" => "Oman",
        "PA" => "Panama",
        "PE" => "Peru",
        "PH" => "Philippines",
        "PK" => "Pakistan",
        "PL" => "Poland",
        "PR" => "Puerto Rico",
        "PT" => "Portugal",
        "QA" => "Qatar",
        "RO" => "Romania",
        "RS" => "Serbia",
        "RU" => "Russia",
        "SA" => "Saudi Arabia",
        "SE" => "Sweden",
        "SG" => "Singapore",
        "SI" => "Slovenia",
        "SK" => "Slovakia",
        "TH" => "Thailand",
        "TN" => "Tunisia",
        "TR" => "Turkey",
        "TW" => "Taiwan",
        "UA" => "Ukraine",
        "US" => "United States",
        "UY" => "Uruguay",
        "UZ" => "Uzbekistan",
        "VE" => "Venezuela",
        "VN" => "Vietnam",
        "ZA" => "South Africa",
        _ => raw,
    }
    .to_string()
}

fn metric_groups(account: &Value, key: &str, dimension: &str) -> Vec<AnalyticsMetric> {
    account
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|g| AnalyticsMetric {
            label: group_dimension(g, dimension),
            value: group_count(g),
        })
        .collect()
}

pub fn parse_account_analytics(body: &str, days: u16) -> Result<AnalyticsSummary> {
    if let Some(e) = graphql_error(body) {
        anyhow::bail!("{e}");
    }
    let v: Value = serde_json::from_str(body)
        .map_err(|e| anyhow::anyhow!("unexpected Cloudflare GraphQL response: {e}"))?;
    let account = v
        .pointer("/data/viewer/accounts")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .ok_or_else(|| anyhow::anyhow!("Cloudflare GraphQL returned no account analytics"))?;
    let total = account
        .get("totals")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .cloned()
        .unwrap_or(Value::Null);
    let countries = account
        .get("countries")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|g| CountryTraffic {
            country: country_label(&group_dimension(g, "clientCountryName")),
            requests: group_count(g),
            bandwidth: group_sum(g, "edgeResponseBytes"),
        })
        .collect();
    Ok(AnalyticsSummary {
        days,
        requests: group_count(&total),
        bandwidth: group_sum(&total, "edgeResponseBytes"),
        visits: group_sum(&total, "visits"),
        countries,
        ssl: metric_groups(account, "ssl", "clientSSLProtocol"),
        cache: metric_groups(account, "cache", "cacheStatus"),
        status: metric_groups(account, "status", "edgeResponseStatus"),
        protocols: metric_groups(account, "protocols", "clientRequestHTTPProtocol"),
        content_types: metric_groups(account, "contentTypes", "edgeResponseContentTypeName"),
    })
}

// ---------- Envelope parsing ----------

#[derive(Debug, Deserialize)]
struct CfError {
    #[allow(dead_code)]
    code: i64,
    message: String,
}

#[derive(Debug, Deserialize)]
struct Envelope<T> {
    success: bool,
    #[serde(default, deserialize_with = "vec_or_null")]
    errors: Vec<CfError>,
    result: Option<T>,
    #[serde(default)]
    result_info: Option<ResultInfo>,
}

fn vec_or_null<'de, D, T>(deserializer: D) -> std::result::Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Option::<Vec<T>>::deserialize(deserializer)?.unwrap_or_default())
}

fn envelope_error(errors: &[CfError]) -> String {
    errors
        .first()
        .map(|e| e.message.clone())
        .unwrap_or_else(|| "Cloudflare rejected the request".into())
}

/// R2 buckets are account-scoped: list needs the "Workers R2 Storage" Read
/// permission, create/delete its Edit. A token scoped only to Zone:DNS gets
/// Cloudflare's generic "Authentication error" here, which never says why — so
/// append the specific permission that is missing. Mirrors how DNS surfaces
/// Cloudflare's own message, but R2-specific.
fn r2_hint(e: anyhow::Error) -> anyhow::Error {
    let msg = e.to_string();
    if msg.to_ascii_lowercase().contains("authentication error") {
        anyhow::anyhow!(
            "{msg} — the token may lack the Workers R2 Storage permission (add it at Account scope)"
        )
    } else {
        e
    }
}

fn workers_hint(e: anyhow::Error) -> anyhow::Error {
    let msg = e.to_string();
    if msg.to_ascii_lowercase().contains("authentication error") {
        anyhow::anyhow!(
            "{msg} — the token may lack the Workers Scripts permission (Read for list/get, Write for deploy/delete)"
        )
    } else {
        e
    }
}

fn tunnels_hint(e: anyhow::Error) -> anyhow::Error {
    let msg = e.to_string();
    if msg.to_ascii_lowercase().contains("authentication error") {
        anyhow::anyhow!(
            "{msg} — the token may lack Cloudflare Tunnel Read or Cloudflare One Connectors Read"
        )
    } else {
        e
    }
}

/// Unwrap a Cloudflare v4 envelope, turning `success:false` into an error carrying the
/// first `errors[].message` (Cloudflare's messages are human-readable), not the status.
pub fn parse_envelope<T: DeserializeOwned>(body: &str) -> Result<T> {
    let env: Envelope<T> = serde_json::from_str(body)
        .map_err(|e| anyhow::anyhow!("unexpected Cloudflare response: {e}"))?;
    if !env.success {
        anyhow::bail!("Cloudflare: {}", envelope_error(&env.errors));
    }
    env.result
        .ok_or_else(|| anyhow::anyhow!("Cloudflare returned no result"))
}

/// Envelope + its pagination info, for the client's page loop.
pub fn parse_envelope_paged<T: DeserializeOwned>(body: &str) -> Result<(T, ResultInfo)> {
    let env: Envelope<T> = serde_json::from_str(body)
        .map_err(|e| anyhow::anyhow!("unexpected Cloudflare response: {e}"))?;
    if !env.success {
        anyhow::bail!("Cloudflare: {}", envelope_error(&env.errors));
    }
    let info = env.result_info.unwrap_or_default();
    env.result
        .map(|r| (r, info))
        .ok_or_else(|| anyhow::anyhow!("Cloudflare returned no result"))
}

// ---------- Record bodies + type guards ----------

const V1_TYPES: &[&str] = &["A", "AAAA", "CNAME", "TXT", "NS", "MX"];

/// v1 supports the flat-`content` types + MX. The `data`-object types (SRV, CAA, LOC, …)
/// need a structured body and are deferred; reject them clearly rather than send garbage.
pub fn valid_record_type(kind: &str) -> bool {
    V1_TYPES.contains(&kind.to_ascii_uppercase().as_str())
}

/// Only A/AAAA/CNAME can ride the orange-cloud proxy.
pub fn proxyable(kind: &str) -> bool {
    matches!(kind.to_ascii_uppercase().as_str(), "A" | "AAAA" | "CNAME")
}

/// A zone's health category, for the at-a-glance status colour in the Zones list — the
/// CF analogue of EasyPanel's coloured Status column. Classifying the raw status string
/// is a domain decision, so it lives here (pure, testable); the renderer only maps a
/// category to a colour.
///
/// `active` serves through Cloudflare; `pending`/`initializing` are not live yet (the
/// nameservers have not been moved to Cloudflare — the operator must act);
/// `moved`/`deactivated`/`deleted` no longer serve. Anything unrecognised stays neutral.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoneHealth {
    Active,
    Pending,
    Inactive,
    Unknown,
}

pub fn zone_health(status: &str) -> ZoneHealth {
    match status {
        "active" => ZoneHealth::Active,
        "pending" | "initializing" => ZoneHealth::Pending,
        "moved" | "deactivated" | "deleted" => ZoneHealth::Inactive,
        _ => ZoneHealth::Unknown,
    }
}

/// The CREATE body. `ttl = 1` means "automatic". `proxied` only rides A/AAAA/CNAME;
/// `priority` only MX (and SRV later). Callers pass values already validated.
pub fn record_body(
    kind: &str,
    name: &str,
    content: &str,
    ttl: u32,
    proxied: bool,
    priority: Option<u16>,
) -> Value {
    let mut b = json!({ "type": kind, "name": name, "content": content, "ttl": ttl });
    if proxyable(kind) {
        b["proxied"] = json!(proxied);
    }
    if let Some(p) = priority {
        b["priority"] = json!(p);
    }
    b
}

/// A partial field-change for a record.
#[derive(Debug, Clone, Default)]
pub struct RecordPatch {
    pub content: Option<String>,
    pub proxied: Option<bool>,
    pub ttl: Option<u32>,
    pub priority: Option<u16>,
}

impl RecordPatch {
    pub fn is_empty(&self) -> bool {
        self.content.is_none()
            && self.proxied.is_none()
            && self.ttl.is_none()
            && self.priority.is_none()
    }
}

/// The PATCH body — only the fields the user set. Sent to `PATCH …/dns_records/{id}`,
/// which preserves every field NOT present (unlike PUT, which overwrites).
pub fn apply_patch(patch: &RecordPatch) -> Value {
    let mut b = json!({});
    if let Some(c) = &patch.content {
        b["content"] = json!(c);
    }
    if let Some(p) = patch.proxied {
        b["proxied"] = json!(p);
    }
    if let Some(t) = patch.ttl {
        b["ttl"] = json!(t);
    }
    if let Some(p) = patch.priority {
        b["priority"] = json!(p);
    }
    b
}

// ---------- Zone resolution, record selection, filter query ----------

#[derive(Debug, Clone, Default)]
pub struct RecordFilter {
    pub kind: Option<String>,
    pub name: Option<String>,
    pub content: Option<String>,
}

/// Cloudflare's dns_records filters are operator-keyed: `name.contains`, `content.contains`,
/// flat `type`. Add `match=all` (AND) when more than one is present.
pub fn filter_query(f: &RecordFilter) -> Vec<(String, String)> {
    let mut q = Vec::new();
    if let Some(t) = &f.kind {
        q.push(("type".into(), t.clone()));
    }
    if let Some(n) = &f.name {
        q.push(("name.contains".into(), n.clone()));
    }
    if let Some(c) = &f.content {
        q.push(("content.contains".into(), c.clone()));
    }
    if q.len() > 1 {
        q.push(("match".into(), "all".into()));
    }
    q
}

/// Resolve a zone by NAME (preferred) or id.
pub fn resolve_zone<'a>(zones: &'a [Zone], needle: &str) -> Option<&'a Zone> {
    zones
        .iter()
        .find(|z| z.name == needle)
        .or_else(|| zones.iter().find(|z| z.id == needle))
}

// ---------- Client-side list filters (narrow an already-loaded list) ----------
//
// The CF-local `/` filter narrows the list already in hand rather than refetching
// (a zone can hold thousands of records). Which fields a needle matches is a domain
// rule, so it lives here; the TUI's `cf_*_shown` selectors are thin callers.

/// The zones whose name/status/id contains `needle` (case-insensitive). An empty
/// needle keeps everything.
pub fn filter_zones<'a>(zones: &'a [Zone], needle: &str) -> Vec<&'a Zone> {
    let n = needle.to_ascii_lowercase();
    zones
        .iter()
        .filter(|z| {
            n.is_empty()
                || z.name.to_ascii_lowercase().contains(&n)
                || z.status.to_ascii_lowercase().contains(&n)
                || z.id.to_ascii_lowercase().contains(&n)
        })
        .collect()
}

/// The records whose type/name/content contains `needle` (case-insensitive). An
/// empty needle keeps everything.
pub fn filter_records<'a>(records: &'a [Record], needle: &str) -> Vec<&'a Record> {
    let n = needle.to_ascii_lowercase();
    records
        .iter()
        .filter(|r| {
            n.is_empty()
                || r.kind.to_ascii_lowercase().contains(&n)
                || r.name.to_ascii_lowercase().contains(&n)
                || r.content.to_ascii_lowercase().contains(&n)
        })
        .collect()
}

/// The R2 buckets whose name/class/location contains `needle` (case-insensitive).
/// An empty needle keeps everything.
pub fn filter_buckets<'a>(buckets: &'a [R2Bucket], needle: &str) -> Vec<&'a R2Bucket> {
    let n = needle.to_ascii_lowercase();
    buckets
        .iter()
        .filter(|b| {
            n.is_empty()
                || b.name.to_ascii_lowercase().contains(&n)
                || b.storage_class.to_ascii_lowercase().contains(&n)
                || b.location
                    .as_deref()
                    .unwrap_or("")
                    .to_ascii_lowercase()
                    .contains(&n)
        })
        .collect()
}

pub fn filter_tunnels<'a>(
    tunnels: &'a [CloudflareTunnel],
    needle: &str,
) -> Vec<&'a CloudflareTunnel> {
    let n = needle.to_ascii_lowercase();
    tunnels
        .iter()
        .filter(|t| {
            n.is_empty()
                || t.name.to_ascii_lowercase().contains(&n)
                || t.id.to_ascii_lowercase().contains(&n)
                || t.status_label().to_ascii_lowercase().contains(&n)
                || t.config_src.to_ascii_lowercase().contains(&n)
                || t.tun_type.to_ascii_lowercase().contains(&n)
                || t.target().to_ascii_lowercase().contains(&n)
        })
        .collect()
}

pub fn filter_tunnel_config_rows(rows: &[TunnelConfigRow], needle: &str) -> Vec<TunnelConfigRow> {
    let n = needle.to_ascii_lowercase();
    rows.iter()
        .filter(|r| {
            n.is_empty()
                || r.hostname.to_ascii_lowercase().contains(&n)
                || r.service.to_ascii_lowercase().contains(&n)
                || r.origin.to_ascii_lowercase().contains(&n)
        })
        .cloned()
        .collect()
}

/// The Worker scripts whose name/handler/usage/etag contains `needle`
/// (case-insensitive). An empty needle keeps everything.
pub fn filter_workers<'a>(workers: &'a [WorkerScript], needle: &str) -> Vec<&'a WorkerScript> {
    let n = needle.to_ascii_lowercase();
    workers
        .iter()
        .filter(|w| {
            n.is_empty()
                || w.id.to_ascii_lowercase().contains(&n)
                || w.usage_model.to_ascii_lowercase().contains(&n)
                || w.etag.to_ascii_lowercase().contains(&n)
                || w.handlers
                    .iter()
                    .any(|h| h.to_ascii_lowercase().contains(&n))
        })
        .collect()
}

/// The Worker deployments whose id/version/source/strategy/annotation/author contains
/// `needle` (case-insensitive). Empty keeps everything.
pub fn filter_worker_deployments<'a>(
    deployments: &'a [WorkerDeployment],
    needle: &str,
) -> Vec<&'a WorkerDeployment> {
    let n = needle.to_ascii_lowercase();
    deployments
        .iter()
        .filter(|d| {
            n.is_empty()
                || d.id.to_ascii_lowercase().contains(&n)
                || d.source.to_ascii_lowercase().contains(&n)
                || d.strategy.to_ascii_lowercase().contains(&n)
                || d.author_email.to_ascii_lowercase().contains(&n)
                || d.message().to_ascii_lowercase().contains(&n)
                || d.triggered_by().to_ascii_lowercase().contains(&n)
                || d.versions.iter().any(|v| {
                    v.version_id.to_ascii_lowercase().contains(&n)
                        || trim_percent(v.percentage).contains(&n)
                })
        })
        .collect()
}

pub fn filter_worker_settings_rows(
    rows: &[WorkerSettingsRow],
    needle: &str,
) -> Vec<WorkerSettingsRow> {
    let n = needle.to_ascii_lowercase();
    rows.iter()
        .filter(|r| {
            n.is_empty()
                || r.section.to_ascii_lowercase().contains(&n)
                || r.name.to_ascii_lowercase().contains(&n)
                || r.value.to_ascii_lowercase().contains(&n)
        })
        .cloned()
        .collect()
}

fn push_row(rows: &mut Vec<WorkerSettingsRow>, section: &str, name: &str, value: &str) {
    rows.push(WorkerSettingsRow {
        section: section.into(),
        name: name.into(),
        value: empty_or_dash(value),
    });
}

fn empty_or_dash(value: &str) -> String {
    if value.trim().is_empty() {
        "-".into()
    } else {
        value.to_string()
    }
}

fn join_or_dash(values: &[String]) -> String {
    if values.is_empty() {
        "-".into()
    } else {
        values.join(", ")
    }
}

fn first_non_empty(values: &[&str]) -> String {
    values
        .iter()
        .find(|v| !v.trim().is_empty())
        .copied()
        .map(str::to_string)
        .unwrap_or_else(|| "-".into())
}

fn bool_label(value: Option<bool>) -> String {
    match value {
        Some(true) => "Enabled".into(),
        Some(false) => "Disabled".into(),
        None => "-".into(),
    }
}

fn percent_label(value: f64) -> String {
    let percentage = if value <= 1.0 { value * 100.0 } else { value };
    format!("{}%", trim_percent(percentage))
}

fn sampling_label(settings: &WorkerSamplingSettings) -> String {
    let mut parts = vec![bool_label(settings.enabled)];
    if let Some(rate) = settings.head_sampling_rate {
        parts.push(percent_label(rate));
    }
    if settings.persist == Some(true) {
        parts.push("persist".into());
    }
    if !settings.destinations.is_empty() {
        parts.push(settings.destinations.join(","));
    }
    parts.retain(|p| p != "-");
    if parts.is_empty() {
        "-".into()
    } else {
        parts.join(" · ")
    }
}

fn short_json(value: &Value) -> String {
    match value {
        Value::Null => "-".into(),
        Value::Bool(v) => bool_label(Some(*v)),
        Value::Number(v) => v.to_string(),
        Value::String(v) => empty_or_dash(v),
        Value::Array(values) => {
            if values.is_empty() {
                "-".into()
            } else {
                values.iter().map(short_json).collect::<Vec<_>>().join(", ")
            }
        }
        Value::Object(map) => {
            let pairs = map
                .iter()
                .take(4)
                .map(|(k, v)| format!("{k}: {}", short_json(v)))
                .collect::<Vec<_>>();
            if pairs.is_empty() {
                "-".into()
            } else {
                pairs.join(", ")
            }
        }
    }
}

fn short_cf_date(value: &str) -> String {
    value.split('T').next().unwrap_or(value).to_string()
}

/// A bulk selection over records: explicit ids and/or where-clauses.
#[derive(Debug, Clone, Default)]
pub struct Selector {
    pub ids: Vec<String>,
    pub where_content: Option<String>,
    pub where_type: Option<String>,
    pub where_name: Option<String>,
}

impl Selector {
    fn is_empty(&self) -> bool {
        self.ids.is_empty()
            && self.where_content.is_none()
            && self.where_type.is_none()
            && self.where_name.is_none()
    }
}

/// The records a selector matches. An EMPTY selector matches NOTHING — a bulk op must
/// never silently fan out over every record in the zone.
pub fn select_records<'a>(records: &'a [Record], sel: &Selector) -> Vec<&'a Record> {
    if sel.is_empty() {
        return Vec::new();
    }
    records
        .iter()
        .filter(|r| {
            if !sel.ids.is_empty() && !sel.ids.iter().any(|id| id == &r.id) {
                return false;
            }
            if let Some(c) = &sel.where_content {
                if &r.content != c {
                    return false;
                }
            }
            if let Some(t) = &sel.where_type {
                if !r.kind.eq_ignore_ascii_case(t) {
                    return false;
                }
            }
            if let Some(n) = &sel.where_name {
                if !r.name.contains(n.as_str()) {
                    return false;
                }
            }
            true
        })
        .collect()
}

// ---------- HTTP client ----------

const BASE: &str = "https://api.cloudflare.com/client/v4";

/// A Cloudflare API client bound to one account's scoped token. Separate from
/// EasypanelClient — different API, auth, and base URL.
pub struct CloudflareClient {
    http: reqwest::blocking::Client,
    token: String,
}

impl CloudflareClient {
    pub fn new(token: &str) -> Self {
        Self {
            http: reqwest::blocking::Client::builder()
                .user_agent("easypanel-cli")
                .build()
                .expect("reqwest client"),
            token: token.to_string(),
        }
    }

    fn get(&self, path: &str, query: &[(String, String)]) -> Result<String> {
        Ok(self
            .http
            .get(format!("{BASE}{path}"))
            .bearer_auth(&self.token)
            .query(query)
            .send()?
            .text()?)
    }

    /// List every zone the token can see. `account_id` narrows to one account when set.
    pub fn list_zones(&self, account_id: Option<&str>) -> Result<Vec<Zone>> {
        let mut all = Vec::new();
        let mut page = 1u32;
        loop {
            let mut q: Vec<(String, String)> = vec![
                ("page".into(), page.to_string()),
                ("per_page".into(), "50".into()),
            ];
            if let Some(acc) = account_id {
                q.push(("account.id".into(), acc.to_string()));
            }
            let body = self.get("/zones", &q)?;
            let (mut zones, info): (Vec<Zone>, ResultInfo) = parse_envelope_paged(&body)?;
            all.append(&mut zones);
            if info.total_pages <= page || info.total_pages == 0 {
                break;
            }
            page += 1;
        }
        Ok(all)
    }

    /// List a zone's DNS records, following pagination, with an optional server-side filter.
    pub fn list_records(&self, zone_id: &str, filter: &RecordFilter) -> Result<Vec<Record>> {
        let mut all = Vec::new();
        let mut page = 1u32;
        loop {
            let mut q = filter_query(filter);
            q.push(("page".into(), page.to_string()));
            q.push(("per_page".into(), "100".into()));
            let body = self.get(&format!("/zones/{zone_id}/dns_records"), &q)?;
            let (mut recs, info): (Vec<Record>, ResultInfo) = parse_envelope_paged(&body)?;
            all.append(&mut recs);
            if info.total_pages <= page || info.total_pages == 0 {
                break;
            }
            page += 1;
        }
        Ok(all)
    }

    /// List all account-level Web Analytics sites. Cloudflare exposes this under the
    /// RUM Site Info REST API; the endpoint returns site metadata, not RUM traffic
    /// totals, so page-view/visit fields stay optional.
    pub fn list_web_analytics_sites(&self, account_id: &str) -> Result<Vec<WebAnalyticsSite>> {
        let mut all = Vec::new();
        let mut page = 1u32;
        loop {
            let q: Vec<(String, String)> = vec![
                ("page".into(), page.to_string()),
                ("per_page".into(), "50".into()),
                ("order_by".into(), "host".into()),
            ];
            let body = self.get(&format!("/accounts/{account_id}/rum/site_info/list"), &q)?;
            let (mut sites, info): (Vec<WebAnalyticsSiteRaw>, ResultInfo) =
                parse_envelope_paged(&body)?;
            all.extend(sites.drain(..).map(WebAnalyticsSite::from));
            if info.total_pages <= page || info.total_pages == 0 {
                break;
            }
            page += 1;
        }
        Ok(all)
    }

    fn send(&self, method: reqwest::Method, path: &str, body: Option<&Value>) -> Result<String> {
        let mut req = self
            .http
            .request(method, format!("{BASE}{path}"))
            .bearer_auth(&self.token);
        if let Some(b) = body {
            req = req.json(b);
        }
        Ok(req.send()?.text()?)
    }

    fn graphql(&self, body: &Value) -> Result<String> {
        Ok(self
            .http
            .post(format!("{BASE}/graphql"))
            .bearer_auth(&self.token)
            .json(body)
            .send()?
            .text()?)
    }

    pub fn account_analytics(&self, account_id: &str, days: u16) -> Result<AnalyticsSummary> {
        let end = Utc::now();
        let start = end - Duration::days(i64::from(days.max(1)));
        let query = r#"
query AccountAnalytics($accountTag: string, $filter: filter) {
  viewer {
    accounts(filter: { accountTag: $accountTag }) {
      totals: httpRequestsAdaptiveGroups(limit: 1, filter: $filter) {
        count
        sum { edgeResponseBytes visits }
      }
      countries: httpRequestsAdaptiveGroups(limit: 10, orderBy: [count_DESC], filter: $filter) {
        count
        sum { edgeResponseBytes }
        dimensions { clientCountryName }
      }
      ssl: httpRequestsAdaptiveGroups(limit: 6, orderBy: [count_DESC], filter: $filter) {
        count
        dimensions { clientSSLProtocol }
      }
      cache: httpRequestsAdaptiveGroups(limit: 8, orderBy: [count_DESC], filter: $filter) {
        count
        dimensions { cacheStatus }
      }
      status: httpRequestsAdaptiveGroups(limit: 8, orderBy: [count_DESC], filter: $filter) {
        count
        dimensions { edgeResponseStatus }
      }
      protocols: httpRequestsAdaptiveGroups(limit: 6, orderBy: [count_DESC], filter: $filter) {
        count
        dimensions { clientRequestHTTPProtocol }
      }
    }
  }
}
"#;
        let body = json!({
            "query": query,
            "variables": {
                "accountTag": account_id,
                "filter": {
                    "datetime_geq": start.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                    "datetime_lt": end.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                    "requestSource": "eyeball"
                }
            }
        });
        let resp = self.graphql(&body)?;
        parse_account_analytics(&resp, days)
    }

    /// Create a zone under `account_id` (required in practice for a token).
    pub fn create_zone(&self, name: &str, account_id: &str) -> Result<Zone> {
        let body = json!({ "name": name, "account": { "id": account_id } });
        parse_envelope(&self.send(reqwest::Method::POST, "/zones", Some(&body))?)
    }

    pub fn delete_zone(&self, zone_id: &str) -> Result<()> {
        let _: Value = parse_envelope(&self.send(
            reqwest::Method::DELETE,
            &format!("/zones/{zone_id}"),
            None,
        )?)?;
        Ok(())
    }

    pub fn create_record(&self, zone_id: &str, body: &Value) -> Result<Record> {
        parse_envelope(&self.send(
            reqwest::Method::POST,
            &format!("/zones/{zone_id}/dns_records"),
            Some(body),
        )?)
    }

    /// PATCH — a partial update; only the fields in `patch` change, the rest are preserved.
    pub fn patch_record(&self, zone_id: &str, record_id: &str, patch: &Value) -> Result<Record> {
        parse_envelope(&self.send(
            reqwest::Method::PATCH,
            &format!("/zones/{zone_id}/dns_records/{record_id}"),
            Some(patch),
        )?)
    }

    pub fn delete_record(&self, zone_id: &str, record_id: &str) -> Result<()> {
        let _: Value = parse_envelope(&self.send(
            reqwest::Method::DELETE,
            &format!("/zones/{zone_id}/dns_records/{record_id}"),
            None,
        )?)?;
        Ok(())
    }

    // ---------- R2 buckets (account-scoped) ----------

    /// List every R2 bucket under `account_id`, following R2's CURSOR pagination.
    /// The list `result` wraps its array in a `buckets` object; the next cursor is
    /// in `result_info.cursor` and loops back as the `cursor` query param until it
    /// is absent/empty.
    pub fn list_r2_buckets(&self, account_id: &str) -> Result<Vec<R2Bucket>> {
        let mut all = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let mut q: Vec<(String, String)> = vec![("per_page".into(), "100".into())];
            if let Some(c) = &cursor {
                q.push(("cursor".into(), c.clone()));
            }
            let body = self
                .get(&format!("/accounts/{account_id}/r2/buckets"), &q)
                .map_err(r2_hint)?;
            let (res, info): (BucketsResult, ResultInfo) =
                parse_envelope_paged(&body).map_err(r2_hint)?;
            all.extend(res.buckets);
            match info.cursor.filter(|c| !c.is_empty()) {
                Some(c) => cursor = Some(c),
                None => break,
            }
        }
        Ok(all)
    }

    // ---------- Cloudflare Tunnel (account-scoped) ----------

    pub fn list_tunnels(&self, account_id: &str) -> Result<Vec<CloudflareTunnel>> {
        let mut all = Vec::new();
        let mut page = 1u32;
        loop {
            let q: Vec<(String, String)> = vec![
                ("page".into(), page.to_string()),
                ("per_page".into(), "100".into()),
                ("is_deleted".into(), "false".into()),
            ];
            let body = self
                .get(&format!("/accounts/{account_id}/cfd_tunnel"), &q)
                .map_err(tunnels_hint)?;
            let (mut tunnels, info): (Vec<CloudflareTunnel>, ResultInfo) =
                parse_envelope_paged(&body).map_err(tunnels_hint)?;
            all.append(&mut tunnels);
            if info.total_pages <= page || info.total_pages == 0 {
                break;
            }
            page += 1;
        }
        Ok(all)
    }

    pub fn create_tunnel(&self, account_id: &str, name: &str) -> Result<CloudflareTunnel> {
        let body = json!({
            "name": name.trim(),
            "config_src": "cloudflare",
        });
        parse_envelope(
            &self
                .send(
                    reqwest::Method::POST,
                    &format!("/accounts/{account_id}/cfd_tunnel"),
                    Some(&body),
                )
                .map_err(tunnels_hint)?,
        )
        .map_err(tunnels_hint)
    }

    pub fn delete_tunnel(&self, account_id: &str, tunnel_id: &str) -> Result<()> {
        let _: Value = parse_envelope(
            &self
                .send(
                    reqwest::Method::DELETE,
                    &format!("/accounts/{account_id}/cfd_tunnel/{tunnel_id}"),
                    None,
                )
                .map_err(tunnels_hint)?,
        )
        .map_err(tunnels_hint)?;
        Ok(())
    }

    pub fn get_tunnel_token(&self, account_id: &str, tunnel_id: &str) -> Result<String> {
        parse_envelope(
            &self
                .get(
                    &format!("/accounts/{account_id}/cfd_tunnel/{tunnel_id}/token"),
                    &[],
                )
                .map_err(tunnels_hint)?,
        )
        .map_err(tunnels_hint)
    }

    pub fn get_tunnel_config(
        &self,
        account_id: &str,
        tunnel_id: &str,
    ) -> Result<TunnelConfiguration> {
        let body = self
            .get(
                &format!("/accounts/{account_id}/cfd_tunnel/{tunnel_id}/configurations"),
                &[],
            )
            .map_err(tunnels_hint)?;
        parse_envelope(&body).map_err(tunnels_hint)
    }

    pub fn put_tunnel_config(
        &self,
        account_id: &str,
        tunnel_id: &str,
        config: &TunnelConfig,
    ) -> Result<TunnelConfiguration> {
        let body = json!({ "config": config });
        parse_envelope(
            &self
                .send(
                    reqwest::Method::PUT,
                    &format!("/accounts/{account_id}/cfd_tunnel/{tunnel_id}/configurations"),
                    Some(&body),
                )
                .map_err(tunnels_hint)?,
        )
        .map_err(tunnels_hint)
    }

    pub fn add_tunnel_route(
        &self,
        account_id: &str,
        tunnel_id: &str,
        rule: TunnelIngressRule,
    ) -> Result<TunnelConfiguration> {
        let mut current = self.get_tunnel_config(account_id, tunnel_id)?;
        crate::cloudflare::add_tunnel_route(&mut current.config, rule)?;
        self.put_tunnel_config(account_id, tunnel_id, &current.config)
    }

    pub fn edit_tunnel_route(
        &self,
        account_id: &str,
        tunnel_id: &str,
        change: TunnelRouteChange,
    ) -> Result<TunnelConfiguration> {
        let mut current = self.get_tunnel_config(account_id, tunnel_id)?;
        crate::cloudflare::edit_tunnel_route(&mut current.config, change)?;
        self.put_tunnel_config(account_id, tunnel_id, &current.config)
    }

    pub fn delete_tunnel_route(
        &self,
        account_id: &str,
        tunnel_id: &str,
        hostname: &str,
        path: Option<&str>,
    ) -> Result<TunnelConfiguration> {
        let mut current = self.get_tunnel_config(account_id, tunnel_id)?;
        crate::cloudflare::delete_tunnel_route(&mut current.config, hostname, path)?;
        self.put_tunnel_config(account_id, tunnel_id, &current.config)
    }

    // ---------- Workers scripts (account-scoped) ----------

    pub fn list_worker_scripts(&self, account_id: &str) -> Result<Vec<WorkerScript>> {
        let body = self
            .get(&format!("/accounts/{account_id}/workers/scripts"), &[])
            .map_err(workers_hint)?;
        parse_envelope(&body).map_err(workers_hint)
    }

    pub fn list_worker_deployments(
        &self,
        account_id: &str,
        name: &str,
    ) -> Result<Vec<WorkerDeployment>> {
        let body = self
            .get(
                &format!("/accounts/{account_id}/workers/scripts/{name}/deployments"),
                &[],
            )
            .map_err(workers_hint)?;
        let result: WorkerDeploymentsResult = parse_envelope(&body).map_err(workers_hint)?;
        Ok(result.deployments)
    }

    pub fn get_worker_version_settings(
        &self,
        account_id: &str,
        name: &str,
    ) -> Result<WorkerVersionSettings> {
        let body = self
            .get(
                &format!("/accounts/{account_id}/workers/scripts/{name}/settings"),
                &[],
            )
            .map_err(workers_hint)?;
        parse_envelope(&body).map_err(workers_hint)
    }

    pub fn get_worker_script_settings(
        &self,
        account_id: &str,
        name: &str,
    ) -> Result<WorkerScriptSettings> {
        let body = self
            .get(
                &format!("/accounts/{account_id}/workers/scripts/{name}/script-settings"),
                &[],
            )
            .map_err(workers_hint)?;
        parse_envelope(&body).map_err(workers_hint)
    }

    pub fn list_worker_secrets(
        &self,
        account_id: &str,
        name: &str,
    ) -> Result<Vec<WorkerSecretBinding>> {
        let body = self
            .get(
                &format!("/accounts/{account_id}/workers/scripts/{name}/secrets"),
                &[],
            )
            .map_err(workers_hint)?;
        let secrets: Option<Vec<WorkerSecretBinding>> =
            parse_envelope(&body).map_err(workers_hint)?;
        Ok(secrets.unwrap_or_default())
    }

    pub fn list_worker_schedules(
        &self,
        account_id: &str,
        name: &str,
    ) -> Result<Vec<WorkerSchedule>> {
        let body = self
            .get(
                &format!("/accounts/{account_id}/workers/scripts/{name}/schedules"),
                &[],
            )
            .map_err(workers_hint)?;
        let result: WorkerSchedulesResult = parse_envelope(&body).map_err(workers_hint)?;
        Ok(result.schedules)
    }

    pub fn get_worker_settings_bundle(
        &self,
        account_id: &str,
        name: &str,
    ) -> Result<WorkerSettingsBundle> {
        Ok(WorkerSettingsBundle {
            version: self.get_worker_version_settings(account_id, name)?,
            script: self.get_worker_script_settings(account_id, name)?,
            secrets: self.list_worker_secrets(account_id, name)?,
            schedules: self.list_worker_schedules(account_id, name)?,
        })
    }

    /// Download a Worker's script content. On success Cloudflare returns raw script bytes
    /// (not a JSON envelope); on error it returns the standard envelope.
    pub fn get_worker_script_content(
        &self,
        account_id: &str,
        name: &str,
        out: &mut dyn std::io::Write,
    ) -> Result<u64> {
        let url = format!("{BASE}/accounts/{account_id}/workers/scripts/{name}/content/v2");
        let mut resp = self
            .http
            .get(url)
            .bearer_auth(&self.token)
            .send()
            .map_err(|e| workers_hint(e.into()))?;
        if resp.status().is_success() {
            Ok(resp.copy_to(out)?)
        } else {
            let body = resp.text().map_err(|e| workers_hint(e.into()))?;
            let _: Value = parse_envelope(&body).map_err(workers_hint)?;
            anyhow::bail!("Cloudflare returned an error status but no error message")
        }
    }

    /// Upload one local file as a Worker script. Cloudflare's content endpoint is
    /// multipart: `metadata` names either `main_module` (module syntax) or `body_part`
    /// (service-worker syntax), and the file part uses the same filename.
    pub fn put_worker_script_content(
        &self,
        account_id: &str,
        name: &str,
        filename: &str,
        bytes: Vec<u8>,
        mode: WorkerUploadMode,
    ) -> Result<WorkerScript> {
        let metadata = json!({ mode.metadata_key(): filename }).to_string();
        let content_type = match mode {
            WorkerUploadMode::Module => "application/javascript+module",
            WorkerUploadMode::ServiceWorker => "application/javascript",
        };
        let file_part = reqwest::blocking::multipart::Part::bytes(bytes)
            .file_name(filename.to_string())
            .mime_str(content_type)?;
        let form = reqwest::blocking::multipart::Form::new()
            .text("metadata", metadata)
            .part(filename.to_string(), file_part);
        let body = self
            .http
            .put(format!(
                "{BASE}/accounts/{account_id}/workers/scripts/{name}/content"
            ))
            .bearer_auth(&self.token)
            .multipart(form)
            .send()
            .and_then(|r| r.text())
            .map_err(|e| workers_hint(e.into()))?;
        parse_envelope(&body).map_err(workers_hint)
    }

    pub fn delete_worker_script(&self, account_id: &str, name: &str, force: bool) -> Result<()> {
        let body = self
            .http
            .delete(format!(
                "{BASE}/accounts/{account_id}/workers/scripts/{name}"
            ))
            .bearer_auth(&self.token)
            .query(&[("force", force)])
            .send()
            .and_then(|r| r.text())
            .map_err(|e| workers_hint(e.into()))?;
        let _: Value = parse_envelope(&body).map_err(workers_hint)?;
        Ok(())
    }

    /// Create an R2 bucket by name under `account_id`.
    pub fn create_r2_bucket(&self, account_id: &str, name: &str) -> Result<R2Bucket> {
        let body = json!({ "name": name });
        parse_envelope(
            &self
                .send(
                    reqwest::Method::POST,
                    &format!("/accounts/{account_id}/r2/buckets"),
                    Some(&body),
                )
                .map_err(r2_hint)?,
        )
        .map_err(r2_hint)
    }

    /// Delete an R2 bucket. Cloudflare requires the bucket be EMPTY — deleting a
    /// non-empty one errors, and that message is surfaced as-is.
    pub fn delete_r2_bucket(&self, account_id: &str, name: &str) -> Result<()> {
        let _: Value = parse_envelope(
            &self
                .send(
                    reqwest::Method::DELETE,
                    &format!("/accounts/{account_id}/r2/buckets/{name}"),
                    None,
                )
                .map_err(r2_hint)?,
        )
        .map_err(r2_hint)?;
        Ok(())
    }

    /// List ONE level of a bucket browsed as a folder tree. Sends `delimiter=/` so the
    /// `/`-delimited keys group into subfolders instead of a flat 1000-row dump: `result`
    /// is the files directly at `prefix` (no further `/`), and `result_info.delimited`
    /// (VERIFIED against the R2 list-objects API docs — an array of full key prefixes
    /// ending in `/`) is the subfolders. `prefix` is "" at the bucket root, or e.g.
    /// `assets/css/` deeper. Same Bearer token as buckets — no S3 credentials.
    ///
    /// A bucket can hold millions of objects, so one level is fetched (up to 1000 rows)
    /// and `truncated` reported rather than walking every cursor page. The result is
    /// sorted for display (folders A→Z, files newest-first) inside the domain.
    pub fn list_r2_level(&self, account_id: &str, bucket: &str, prefix: &str) -> Result<R2Level> {
        let path = format!("/accounts/{account_id}/r2/buckets/{bucket}/objects");
        let mut q: Vec<(String, String)> = vec![
            ("per_page".into(), "1000".into()),
            ("delimiter".into(), "/".into()),
        ];
        if !prefix.is_empty() {
            q.push(("prefix".into(), prefix.to_string()));
        }
        let body = self.get(&path, &q).map_err(r2_hint)?;
        parse_r2_level(&body).map_err(r2_hint)
    }

    /// Upload raw bytes to an object key. `PUT …/objects/{key}` with the raw body and a
    /// `Content-Type` (default `application/octet-stream`); success is the standard
    /// envelope. The caller must enforce [`MAX_REST_OBJECT_BYTES`] before calling.
    pub fn put_object(
        &self,
        account_id: &str,
        bucket: &str,
        key: &str,
        bytes: Vec<u8>,
        content_type: Option<&str>,
    ) -> Result<()> {
        let url = format!(
            "{BASE}/accounts/{account_id}/r2/buckets/{bucket}/objects/{}",
            encode_object_key(key)
        );
        let body = self
            .http
            .put(url)
            .bearer_auth(&self.token)
            .header(
                reqwest::header::CONTENT_TYPE,
                content_type.unwrap_or("application/octet-stream"),
            )
            .body(bytes)
            .send()
            .and_then(|r| r.text())
            .map_err(|e| r2_hint(e.into()))?;
        let _: Value = parse_envelope(&body).map_err(r2_hint)?;
        Ok(())
    }

    /// Download an object, streaming its body into `out` (never buffering the whole file in
    /// memory). On success the body IS the raw object bytes; on error it is a JSON error
    /// envelope, which is parsed so "object not found" / auth errors surface properly.
    /// Returns the byte count written.
    pub fn download_object(
        &self,
        account_id: &str,
        bucket: &str,
        key: &str,
        out: &mut dyn std::io::Write,
    ) -> Result<u64> {
        let url = format!(
            "{BASE}/accounts/{account_id}/r2/buckets/{bucket}/objects/{}",
            encode_object_key(key)
        );
        let mut resp = self
            .http
            .get(url)
            .bearer_auth(&self.token)
            .send()
            .map_err(|e| r2_hint(e.into()))?;
        if resp.status().is_success() {
            Ok(resp.copy_to(out)?)
        } else {
            let body = resp.text().map_err(|e| r2_hint(e.into()))?;
            // A non-2xx body is the JSON error envelope; parse_envelope turns it into the
            // Cloudflare message. It never returns Ok on an error body.
            let _: Value = parse_envelope(&body).map_err(r2_hint)?;
            anyhow::bail!("Cloudflare returned an error status but no error message")
        }
    }

    /// Delete one object key. `DELETE …/objects/{key}`; success is the standard envelope.
    pub fn delete_object(&self, account_id: &str, bucket: &str, key: &str) -> Result<()> {
        let url = format!(
            "{BASE}/accounts/{account_id}/r2/buckets/{bucket}/objects/{}",
            encode_object_key(key)
        );
        let body = self
            .http
            .delete(url)
            .bearer_auth(&self.token)
            .send()
            .and_then(|r| r.text())
            .map_err(|e| r2_hint(e.into()))?;
        let _: Value = parse_envelope(&body).map_err(r2_hint)?;
        Ok(())
    }
}

/// Parse a delimiter-mode objects response into one browse level: the files at this
/// level (`result`) plus the subfolder prefixes (`result_info.delimited`), sorted for
/// display. Pure, so the folder/file split + sort is unit-tested from a fixture with no
/// live call.
fn parse_r2_level(body: &str) -> Result<R2Level> {
    let (files, info): (Vec<R2Object>, ResultInfo) = parse_envelope_paged(body)?;
    let mut level = R2Level {
        folders: info.delimited,
        files,
        truncated: info.is_truncated,
    };
    sort_r2_level(&mut level);
    Ok(level)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zone_health_classifies_the_status_string() {
        assert_eq!(zone_health("active"), ZoneHealth::Active);
        assert_eq!(zone_health("pending"), ZoneHealth::Pending);
        assert_eq!(zone_health("initializing"), ZoneHealth::Pending);
        assert_eq!(zone_health("moved"), ZoneHealth::Inactive);
        assert_eq!(zone_health("deactivated"), ZoneHealth::Inactive);
        // An unrecognised status stays neutral rather than being mislabelled healthy.
        assert_eq!(zone_health("read only"), ZoneHealth::Unknown);
        assert_eq!(zone_health(""), ZoneHealth::Unknown);
    }

    #[test]
    fn envelope_success_unwraps_result() {
        let body = r#"{"success":true,"errors":[],"messages":[],
            "result":[{"id":"z1","name":"example.com","status":"active"}]}"#;
        let zones: Vec<Zone> = parse_envelope(body).unwrap();
        assert_eq!(zones[0].name, "example.com");
    }

    #[test]
    fn envelope_accepts_null_error_lists_from_cloudflare() {
        let body = r#"{"success":true,"errors":null,"messages":null,
            "result":[{"id":"z1","name":"example.com","status":"active"}],
            "result_info":null}"#;
        let (zones, info): (Vec<Zone>, ResultInfo) = parse_envelope_paged(body).unwrap();
        assert_eq!(zones[0].name, "example.com");
        assert_eq!(info.total_pages, 0);
    }

    #[test]
    fn workers_scripts_parse_from_the_standard_cloudflare_envelope() {
        let body = r#"{"success":true,"errors":[],"messages":[],
            "result":[{"id":"api-worker","created_on":"2026-01-01T00:00:00Z",
                       "modified_on":"2026-02-03T04:05:06Z","etag":"abc123",
                       "handlers":["fetch"],"usage_model":"standard"}]}"#;
        let scripts: Vec<WorkerScript> = parse_envelope(body).unwrap();
        assert_eq!(scripts.len(), 1);
        assert_eq!(scripts[0].id, "api-worker");
        assert_eq!(scripts[0].handlers, vec!["fetch"]);
        assert_eq!(scripts[0].usage_model, "standard");
        assert_eq!(scripts[0].modified_on, "2026-02-03T04:05:06Z");
    }

    #[test]
    fn filter_workers_matches_name_handlers_usage_and_etag() {
        let scripts = vec![
            WorkerScript {
                id: "frontend".into(),
                handlers: vec!["fetch".into()],
                usage_model: "standard".into(),
                etag: "aaa".into(),
                ..Default::default()
            },
            WorkerScript {
                id: "cron-job".into(),
                handlers: vec!["scheduled".into()],
                usage_model: "unbound".into(),
                etag: "bbb".into(),
                ..Default::default()
            },
        ];
        assert_eq!(filter_workers(&scripts, "front")[0].id, "frontend");
        assert_eq!(filter_workers(&scripts, "sched")[0].id, "cron-job");
        assert_eq!(filter_workers(&scripts, "unbound")[0].id, "cron-job");
        assert_eq!(filter_workers(&scripts, "bbb")[0].id, "cron-job");
        assert_eq!(filter_workers(&scripts, "").len(), 2);
    }

    #[test]
    fn worker_deployments_parse_from_nested_result_object() {
        let body = r#"{"success":true,"errors":[],"messages":[],
            "result":{"deployments":[{
                "id":"4e907926deployment","created_on":"2026-07-23T04:05:06Z",
                "source":"api","strategy":"percentage",
                "versions":[{"version_id":"abc123456789","percentage":100}],
                "annotations":{
                    "workers/message":"Merge pull request #16",
                    "workers/triggered_by":"github"
                },
                "author_email":"operator@example.com"
            }]}}"#;
        let result: WorkerDeploymentsResult = parse_envelope(body).unwrap();
        let deployments = result.deployments;
        assert_eq!(deployments.len(), 1);
        assert_eq!(deployments[0].short_id(), "4e907926");
        assert_eq!(deployments[0].versions_label(), "abc12345 100%");
        assert_eq!(deployments[0].message(), "Merge pull request #16");
        assert_eq!(deployments[0].triggered_by(), "github");
        assert_eq!(deployments[0].author_email, "operator@example.com");
    }

    #[test]
    fn worker_settings_parse_null_arrays_and_render_rows() {
        let body = r#"{"success":true,"errors":[],"messages":[],
            "result":{
                "bindings":null,
                "compatibility_date":"2025-12-01",
                "compatibility_flags":null,
                "usage_model":"standard"
            }}"#;
        let version: WorkerVersionSettings = parse_envelope(body).unwrap();
        let bundle = WorkerSettingsBundle {
            version,
            script: WorkerScriptSettings {
                observability: Some(WorkerObservability {
                    enabled: Some(true),
                    logs: Some(WorkerSamplingSettings {
                        enabled: Some(true),
                        head_sampling_rate: Some(1.0),
                        persist: Some(true),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            },
            schedules: vec![WorkerSchedule {
                cron: "0 9 * * *".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let rows = bundle.rows(&WorkerScript {
            id: "siakad".into(),
            handlers: vec!["fetch".into()],
            ..Default::default()
        });
        assert!(rows
            .iter()
            .any(|r| r.name == "Cron scheduled()" && r.value == "0 9 * * *"));
        assert!(rows
            .iter()
            .any(|r| r.name == "Compatibility flags" && r.value == "-"));
        assert!(filter_worker_settings_rows(&rows, "observability").len() >= 2);
    }

    #[test]
    fn filter_worker_deployments_matches_versions_annotations_and_author() {
        let deployments = vec![
            WorkerDeployment {
                id: "deploy-one".into(),
                source: "api".into(),
                strategy: "percentage".into(),
                versions: vec![WorkerDeploymentVersion {
                    version_id: "version-alpha".into(),
                    percentage: 100.0,
                }],
                annotations: Some(WorkerDeploymentAnnotations {
                    message: "release checkout".into(),
                    triggered_by: "github".into(),
                }),
                author_email: "ops@example.com".into(),
                ..Default::default()
            },
            WorkerDeployment {
                id: "deploy-two".into(),
                source: "dash".into(),
                strategy: "percentage".into(),
                versions: vec![WorkerDeploymentVersion {
                    version_id: "version-beta".into(),
                    percentage: 20.0,
                }],
                ..Default::default()
            },
        ];
        assert_eq!(
            filter_worker_deployments(&deployments, "checkout")[0].id,
            "deploy-one"
        );
        assert_eq!(
            filter_worker_deployments(&deployments, "version-beta")[0].id,
            "deploy-two"
        );
        assert_eq!(
            filter_worker_deployments(&deployments, "ops@example")[0].id,
            "deploy-one"
        );
        assert_eq!(filter_worker_deployments(&deployments, "").len(), 2);
    }

    #[test]
    fn envelope_failure_surfaces_the_first_error_message() {
        let body = r#"{"success":false,
            "errors":[{"code":81057,"message":"Record already exists."}],
            "messages":[],"result":null}"#;
        let err = parse_envelope::<Vec<Record>>(body).unwrap_err().to_string();
        assert!(err.contains("Record already exists."), "got: {err}");
    }

    #[test]
    fn envelope_failure_with_no_error_array_still_fails_cleanly() {
        let body = r#"{"success":false,"errors":[],"messages":[],"result":null}"#;
        assert!(parse_envelope::<Vec<Record>>(body).is_err());
    }

    // The list-objects REST envelope as Cloudflare documents it: `result` is a BARE
    // array of objects (NOT wrapped like buckets), and pagination lives in
    // `result_info` as `cursor` + `is_truncated`. A fixture pins the verified shape.
    #[test]
    fn r2_objects_result_is_a_bare_array_with_cursor_pagination() {
        let body = r#"{"success":true,"errors":[],"messages":[],
            "result":[
                {"key":"img/logo.png","size":2048,"last_modified":"2026-01-02T03:04:05.000Z",
                 "storage_class":"Standard","etag":"e1"},
                {"key":"img/hero.jpg","size":10485760,"last_modified":"2026-01-03T00:00:00.000Z",
                 "storage_class":"Standard"}
            ],
            "result_info":{"cursor":"next-page-cursor","is_truncated":true,"per_page":1000}}"#;
        let (objects, info): (Vec<R2Object>, ResultInfo) = parse_envelope_paged(body).unwrap();
        assert_eq!(objects.len(), 2);
        assert_eq!(objects[0].key, "img/logo.png");
        assert_eq!(objects[0].size, 2048);
        assert_eq!(objects[0].last_modified, "2026-01-02T03:04:05.000Z");
        assert_eq!(objects[0].storage_class, "Standard");
        assert_eq!(objects[1].size, 10_485_760);
        // Truncated → the caller loops with this cursor.
        assert!(info.is_truncated);
        assert_eq!(info.cursor.as_deref(), Some("next-page-cursor"));
    }

    #[test]
    fn r2_objects_last_page_is_not_truncated_and_empty_is_empty() {
        let last = r#"{"success":true,"errors":[],"messages":[],
            "result":[{"key":"only.txt","size":5,"last_modified":"2026-01-01T00:00:00.000Z",
                       "storage_class":"Standard"}],
            "result_info":{"is_truncated":false,"per_page":1000}}"#;
        let (objects, info): (Vec<R2Object>, ResultInfo) = parse_envelope_paged(last).unwrap();
        assert_eq!(objects.len(), 1);
        assert!(!info.is_truncated, "a non-truncated page never loops");

        let empty = r#"{"success":true,"errors":[],"messages":[],
            "result":[],"result_info":{"is_truncated":false}}"#;
        let (objects, _): (Vec<R2Object>, ResultInfo) = parse_envelope_paged(empty).unwrap();
        assert!(objects.is_empty());
    }

    // The delimiter-mode (`delimiter=/`) envelope: `result` is the FILES at this level and
    // `result_info.delimited` is the SUBFOLDERS (full key prefixes ending in `/`) — the
    // verified field for common prefixes. A fixture pins the shape; the parse splits and
    // sorts it into one browse level.
    #[test]
    fn r2_level_splits_files_from_folders_and_sorts_them() {
        let body = r#"{"success":true,"errors":[],"messages":[],
            "result":[
                {"key":"assets/admin-front-end/app.js","size":10,
                 "last_modified":"2026-01-02T00:00:00.000Z","storage_class":"Standard"},
                {"key":"assets/admin-front-end/index.html","size":20,
                 "last_modified":"2026-03-09T00:00:00.000Z","storage_class":"Standard"}
            ],
            "result_info":{"is_truncated":true,"per_page":1000,
                "delimited":["assets/admin-front-end/js/","assets/admin-front-end/css/"]}}"#;
        let level = parse_r2_level(body).unwrap();
        // Folders come from `delimited`, sorted A→Z.
        assert_eq!(
            level.folders,
            vec![
                "assets/admin-front-end/css/".to_string(),
                "assets/admin-front-end/js/".to_string()
            ]
        );
        // Files come from `result`, newest-first (index.html is the newer one).
        assert_eq!(level.files.len(), 2);
        assert_eq!(level.files[0].key, "assets/admin-front-end/index.html");
        assert_eq!(level.files[1].key, "assets/admin-front-end/app.js");
        // A level with more than one page reports truncated for the "narrow with /" note.
        assert!(level.truncated);
    }

    #[test]
    fn r2_level_root_with_no_subfolders_parses_empty_folders() {
        let body = r#"{"success":true,"errors":[],"messages":[],
            "result":[{"key":"readme.txt","size":3,
                       "last_modified":"2026-01-01T00:00:00.000Z","storage_class":"Standard"}],
            "result_info":{"is_truncated":false,"per_page":1000}}"#;
        let level = parse_r2_level(body).unwrap();
        assert!(level.folders.is_empty(), "no delimited → no subfolders");
        assert_eq!(level.files.len(), 1);
        assert!(!level.truncated);
    }

    #[test]
    fn sort_r2_level_folders_ascending_files_newest_first() {
        let mk = |key: &str, ts: &str| R2Object {
            key: key.into(),
            size: 1,
            last_modified: ts.into(),
            storage_class: "Standard".into(),
        };
        let mut level = R2Level {
            folders: vec!["z/".into(), "a/".into(), "m/".into()],
            files: vec![
                mk("old.txt", "2026-01-01T00:00:00.000Z"),
                mk("new.txt", "2026-06-01T00:00:00.000Z"),
                mk("mid.txt", "2026-03-01T00:00:00.000Z"),
            ],
            truncated: false,
        };
        sort_r2_level(&mut level);
        assert_eq!(level.folders, vec!["a/", "m/", "z/"]);
        let order: Vec<&str> = level.files.iter().map(|f| f.key.as_str()).collect();
        assert_eq!(order, vec!["new.txt", "mid.txt", "old.txt"]);
    }

    #[test]
    fn a_record_body_has_no_priority_and_can_be_proxied() {
        let b = record_body("A", "www.example.com", "1.2.3.4", 1, true, None);
        assert_eq!(b["type"], "A");
        assert_eq!(b["ttl"], 1); // 1 = automatic
        assert_eq!(b["proxied"], true);
        assert!(b.get("priority").is_none());
    }

    #[test]
    fn mx_record_body_carries_priority_and_is_not_proxied() {
        let b = record_body(
            "MX",
            "example.com",
            "mail.example.com",
            3600,
            false,
            Some(10),
        );
        assert_eq!(b["priority"], 10);
        assert!(b.get("proxied").is_none(), "MX is not proxyable");
    }

    #[test]
    fn only_v1_types_are_valid() {
        for t in ["A", "AAAA", "CNAME", "TXT", "NS", "MX"] {
            assert!(valid_record_type(t));
        }
        for t in ["SRV", "CAA", "LOC", "URI", "bogus"] {
            assert!(!valid_record_type(t));
        }
        assert!(proxyable("A") && proxyable("aaaa") && proxyable("CNAME"));
        assert!(!proxyable("TXT") && !proxyable("MX"));
    }

    #[test]
    fn patch_body_only_carries_set_fields() {
        let patch = RecordPatch {
            content: Some("5.6.7.8".into()),
            ..Default::default()
        };
        let b = apply_patch(&patch);
        assert_eq!(b["content"], "5.6.7.8");
        assert!(b.get("proxied").is_none() && b.get("ttl").is_none());
        assert!(!patch.is_empty());
        assert!(
            RecordPatch::default().is_empty(),
            "an all-None patch is empty"
        );
    }

    #[test]
    fn resolve_zone_prefers_name_then_id() {
        let zones = vec![
            Zone {
                id: "id-a".into(),
                name: "example.com".into(),
                status: "active".into(),
            },
            Zone {
                id: "example.com".into(),
                name: "other.com".into(),
                status: "active".into(),
            },
        ];
        assert_eq!(
            resolve_zone(&zones, "example.com").unwrap().id,
            "id-a",
            "name wins over a coincidental id"
        );
        assert_eq!(resolve_zone(&zones, "id-a").unwrap().name, "example.com");
        assert!(resolve_zone(&zones, "nope.com").is_none());
    }

    #[test]
    fn filter_query_uses_operator_keys() {
        let f = RecordFilter {
            kind: Some("A".into()),
            name: Some("api".into()),
            content: None,
        };
        let q = filter_query(&f);
        assert!(q.contains(&("type".into(), "A".into())));
        assert!(q.contains(&("name.contains".into(), "api".into())));
        assert!(
            q.contains(&("match".into(), "all".into())),
            "AND when >1 filter"
        );
        assert!(filter_query(&RecordFilter::default()).is_empty());
    }

    #[test]
    fn select_records_matches_ids_and_wheres() {
        let recs = vec![
            rec("r1", "A", "a.example.com", "1.1.1.1"),
            rec("r2", "A", "b.example.com", "1.1.1.1"),
            rec("r3", "CNAME", "c.example.com", "a.example.com"),
        ];
        let sel = Selector {
            where_content: Some("1.1.1.1".into()),
            ..Default::default()
        };
        assert_eq!(
            select_records(&recs, &sel).len(),
            2,
            "repoint: everything on the old IP"
        );
        let sel = Selector {
            where_content: Some("1.1.1.1".into()),
            where_type: Some("A".into()),
            ..Default::default()
        };
        assert_eq!(
            select_records(&recs, &sel).len(),
            2,
            "type + content intersect"
        );
        let sel = Selector {
            ids: vec!["r3".into()],
            ..Default::default()
        };
        assert_eq!(select_records(&recs, &sel)[0].id, "r3");
        let sel = Selector {
            where_content: Some("9.9.9.9".into()),
            ..Default::default()
        };
        assert!(select_records(&recs, &sel).is_empty(), "no match → empty");
        assert!(
            select_records(&recs, &Selector::default()).is_empty(),
            "empty selector matches nothing"
        );
    }

    #[test]
    fn r2_list_result_is_wrapped_in_a_buckets_object() {
        // The list-buckets `result` is NOT a bare array — it wraps a `buckets` array,
        // and `location` may be omitted. Feed a documented sample envelope and assert
        // the wrapper yields the buckets. result_info.cursor drives the page loop.
        let body = r#"{"success":true,"errors":[],"messages":[],
            "result":{"buckets":[
                {"name":"assets","creation_date":"2024-01-02T03:04:05.000Z",
                 "storage_class":"Standard","jurisdiction":"default"},
                {"name":"backups","creation_date":"2024-02-02T03:04:05.000Z",
                 "location":"weur","storage_class":"InfrequentAccess","jurisdiction":"default"}]},
            "result_info":{"cursor":""}}"#;
        let (res, info): (BucketsResult, ResultInfo) = parse_envelope_paged(body).unwrap();
        assert_eq!(res.buckets.len(), 2);
        assert_eq!(res.buckets[0].name, "assets");
        assert!(res.buckets[0].location.is_none(), "location may be omitted");
        assert_eq!(res.buckets[1].location.as_deref(), Some("weur"));
        assert_eq!(res.buckets[1].storage_class, "InfrequentAccess");
        // An empty cursor means the last page — the loop stops.
        assert!(info.cursor.filter(|c| !c.is_empty()).is_none());
    }

    #[test]
    fn web_analytics_sites_parse_documented_site_info_shape() {
        let body = r#"{"success":true,"errors":[],"messages":[],
            "result":[{
                "auto_install":true,
                "created":"2014-01-01T05:20:00.12345Z",
                "rules":[{
                    "id":"rule-1",
                    "created":"2014-01-01T05:20:00.12345Z",
                    "host":"example.com",
                    "inclusive":true,
                    "is_paused":false,
                    "paths":["*"],
                    "priority":1000
                }],
                "ruleset":{
                    "id":"ruleset-1",
                    "enabled":true,
                    "zone_name":"example.com",
                    "zone_tag":"zone-1"
                },
                "site_tag":"site-1",
                "site_token":"token-1",
                "snippet":"<script></script>"
            }],
            "result_info":{"page":1,"per_page":10,"count":1,"total_count":1,"total_pages":1}}"#;

        let (raw, info): (Vec<WebAnalyticsSiteRaw>, ResultInfo) =
            parse_envelope_paged(body).unwrap();
        let sites: Vec<WebAnalyticsSite> = raw.into_iter().map(WebAnalyticsSite::from).collect();

        assert_eq!(info.total_pages, 1);
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].host, "example.com");
        assert_eq!(sites[0].zone_name, "example.com");
        assert_eq!(sites[0].zone_tag, "zone-1");
        assert_eq!(sites[0].site_tag, "site-1");
        assert_eq!(sites[0].site_token, "token-1");
        assert!(sites[0].auto_install);
        assert!(sites[0].enabled);
        assert_eq!(sites[0].page_views_24h, None);
        assert_eq!(sites[0].visits_24h, None);
    }

    #[test]
    fn r2_auth_error_hints_at_the_workers_r2_storage_permission() {
        let e = anyhow::anyhow!("Cloudflare: Authentication error");
        let hinted = r2_hint(e).to_string();
        assert!(
            hinted.contains("Workers R2 Storage"),
            "an R2 auth failure must name the missing permission: {hinted}"
        );
        // A non-auth error is passed through untouched.
        let other = r2_hint(anyhow::anyhow!(
            "Cloudflare: The bucket you tried to delete is not empty"
        ));
        assert!(!other.to_string().contains("Workers R2 Storage"));
    }

    #[test]
    fn tunnels_parser_accepts_null_connections_and_builds_target() {
        let body = r#"{
            "success":true,
            "errors":[],
            "messages":[],
            "result":[{
                "id":"6b351b2e-1111-2222-3333-444455556666",
                "name":"mrfansi.dev",
                "created_at":"2026-04-27T00:00:00Z",
                "deleted_at":null,
                "tun_type":"cfd_tunnel",
                "status":"degraded",
                "config_src":"cloudflare",
                "conns_active_at":null,
                "conns_inactive_at":"2026-07-24T00:00:00Z",
                "connections":null
            }],
            "result_info":{"page":1,"per_page":100,"count":1,"total_count":1,"total_pages":1}
        }"#;
        let (tunnels, info): (Vec<CloudflareTunnel>, ResultInfo) =
            parse_envelope_paged(body).unwrap();
        assert_eq!(info.total_pages, 1);
        assert_eq!(tunnels.len(), 1);
        assert_eq!(tunnels[0].status_label(), "degraded");
        assert_eq!(
            tunnels[0].target(),
            "6b351b2e-1111-2222-3333-444455556666.cfargotunnel.com"
        );
        assert!(tunnels[0].connections.is_empty());
    }

    #[test]
    fn tunnel_config_rows_include_ingress_and_catch_all() {
        let body = r#"{
            "success":true,
            "errors":[],
            "messages":[],
            "result":{
                "account_id":"acc-1",
                "tunnel_id":"tunnel-1",
                "version":7,
                "source":"cloudflare",
                "created_at":"2026-07-24T00:00:00Z",
                "config":{
                    "ingress":[
                        {"hostname":"app.example.com","service":"http://localhost:3000"},
                        {"service":"http_status:404"}
                    ],
                    "originRequest":{"connectTimeout":30}
                }
            }
        }"#;
        let config: TunnelConfiguration = parse_envelope(body).unwrap();
        let rows = config.rows();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].hostname, "app.example.com");
        assert_eq!(rows[0].service, "http://localhost:3000");
        assert_eq!(rows[1].hostname, "catch-all");
        assert_eq!(rows[1].service, "http_status:404");
        assert_eq!(rows[1].origin, "-");
        assert_eq!(
            config.config.origin_request.as_ref().unwrap()["connectTimeout"],
            json!(30)
        );
    }

    #[test]
    fn tunnel_route_mutations_keep_catch_all_last() {
        let mut config = TunnelConfig {
            ingress: vec![TunnelIngressRule::catch_all()],
            ..Default::default()
        };
        add_tunnel_route(
            &mut config,
            TunnelIngressRule::route(
                "app.example.com",
                "http://localhost:3000",
                "",
                Some(json!({"connectTimeout": 10})),
            ),
        )
        .unwrap();
        add_tunnel_route(
            &mut config,
            TunnelIngressRule::route("ssh.example.com", "ssh://localhost:22", "", None),
        )
        .unwrap();
        assert_eq!(config.ingress[0].hostname, "app.example.com");
        assert_eq!(config.ingress[1].hostname, "ssh.example.com");
        assert!(config.ingress[2].is_catch_all());

        edit_tunnel_route(
            &mut config,
            TunnelRouteChange {
                hostname: "app.example.com".into(),
                service: Some("http://localhost:8080".into()),
                origin_request: Some(None),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(config.ingress[0].service, "http://localhost:8080");
        assert!(config.ingress[0].origin_request.is_none());

        delete_tunnel_route(&mut config, "ssh.example.com", None).unwrap();
        assert_eq!(config.ingress.len(), 2);
        assert_eq!(config.ingress[0].hostname, "app.example.com");
        assert!(config.ingress[1].is_catch_all());
    }

    #[test]
    fn tunnel_route_validation_rejects_unknown_services_and_duplicates() {
        let mut config = TunnelConfig::default();
        let bad = add_tunnel_route(
            &mut config,
            TunnelIngressRule::route("app.example.com", "localhost:3000", "", None),
        )
        .unwrap_err()
        .to_string();
        assert!(bad.contains("Service must start"));

        add_tunnel_route(
            &mut config,
            TunnelIngressRule::route("app.example.com", "https://localhost:3000", "", None),
        )
        .unwrap();
        let duplicate = add_tunnel_route(
            &mut config,
            TunnelIngressRule::route("app.example.com", "https://localhost:8080", "", None),
        )
        .unwrap_err()
        .to_string();
        assert!(duplicate.contains("already exists"));
    }

    #[test]
    fn tunnel_filters_match_names_targets_and_routes() {
        let tunnels = vec![
            CloudflareTunnel {
                id: "abc".into(),
                name: "edge-router".into(),
                config_src: "cloudflare".into(),
                ..Default::default()
            },
            CloudflareTunnel {
                id: "def".into(),
                name: "private".into(),
                ..Default::default()
            },
        ];
        assert_eq!(filter_tunnels(&tunnels, "edge").len(), 1);
        assert_eq!(filter_tunnels(&tunnels, "abc.cfargotunnel").len(), 1);

        let rows = vec![TunnelConfigRow {
            hostname: "app.example.com".into(),
            service: "http://localhost:3000".into(),
            origin: "-".into(),
        }];
        assert_eq!(filter_tunnel_config_rows(&rows, "localhost").len(), 1);
        assert!(filter_tunnel_config_rows(&rows, "missing").is_empty());
    }

    #[test]
    fn encode_object_key_percent_encodes_everything_but_slash_and_unreserved() {
        // Space → %20, colon → %3A; the slash and the unreserved bytes stay literal.
        assert_eq!(encode_object_key("a/b c:d.gz"), "a/b%20c%3Ad.gz");
        // A plain nested key is unchanged.
        assert_eq!(encode_object_key("dir/sub/x.sql.gz"), "dir/sub/x.sql.gz");
    }

    #[test]
    fn object_basename_takes_the_segment_after_the_last_slash() {
        assert_eq!(object_basename("a/b/c.gz"), "c.gz");
        assert_eq!(object_basename("x"), "x");
    }

    #[test]
    fn upload_key_joins_prefix_and_the_local_basename() {
        assert_eq!(upload_key("dir/", "/tmp/dump.sql.gz"), "dir/dump.sql.gz");
        assert_eq!(upload_key("", "x.gz"), "x.gz");
    }

    #[test]
    fn max_rest_object_bytes_is_300_mib() {
        assert_eq!(MAX_REST_OBJECT_BYTES, 300 * 1024 * 1024);
    }

    #[test]
    fn marks_status_uses_easypanels_exact_wording() {
        assert_eq!(
            marks_status("record", 7),
            "7 record(s) marked — [Space] to act on them, [Esc] to clear"
        );
        assert_eq!(
            marks_status("file", 1),
            "1 file(s) marked — [Space] to act on them, [Esc] to clear"
        );
    }

    #[test]
    fn account_analytics_parser_reads_totals_countries_and_breakdowns() {
        let body = r#"{
          "data": {"viewer": {"accounts": [{
            "totals": [{"count": 44120000, "sum": {"edgeResponseBytes": 4606400000000, "visits": 2280000}}],
            "countries": [
              {"count": 17590000, "sum": {"edgeResponseBytes": 2396400000000}, "dimensions": {"clientCountryName": "ID"}},
              {"count": 11280000, "sum": {"edgeResponseBytes": 883179520000}, "dimensions": {"clientCountryName": "Singapore"}}
            ],
            "ssl": [{"count": 39960000, "dimensions": {"clientSSLProtocol": "TLSv1.3"}}],
            "cache": [{"count": 623820, "dimensions": {"cacheStatus": "hit"}}],
            "status": [{"count": 12780000, "dimensions": {"edgeResponseStatus": 404}}],
            "protocols": [{"count": 17200000, "dimensions": {"clientRequestHTTPProtocol": "HTTP/1.1"}}]
          }]}}
        }"#;
        let s = parse_account_analytics(body, 7).unwrap();
        assert_eq!(s.days, 7);
        assert_eq!(s.requests, 44_120_000);
        assert_eq!(s.bandwidth, 4_606_400_000_000);
        assert_eq!(s.visits, 2_280_000);
        assert_eq!(s.countries[0].country, "Indonesia");
        assert_eq!(s.countries[1].country, "Singapore");
        assert_eq!(s.countries[1].bandwidth, 883_179_520_000);
        assert_eq!(s.ssl[0].label, "TLSv1.3");
        assert_eq!(s.cache[0].label, "hit");
        assert_eq!(s.status[0].label, "404");
        assert_eq!(s.protocols[0].label, "HTTP/1.1");
        assert!(s.content_types.is_empty());
    }

    #[test]
    fn account_analytics_parser_surfaces_graphql_errors() {
        let err = parse_account_analytics(
            r#"{"errors":[{"message":"permission denied: Account Analytics Read required"}]}"#,
            7,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("Account Analytics Read required"));
    }

    fn rec(id: &str, kind: &str, name: &str, content: &str) -> Record {
        Record {
            id: id.into(),
            kind: kind.into(),
            name: name.into(),
            content: content.into(),
            ttl: 1,
            proxied: false,
            priority: None,
        }
    }
}
