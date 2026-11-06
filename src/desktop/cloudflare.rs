use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use eframe::egui;
use serde_json::{json, Value};

use super::model::{OpenPage, PageContext, PageKind, Payload};
use super::widgets;
use crate::cloudflare::*;
use crate::config::CloudflareConfig;
use crate::form::{self, CfBulkAttr, Field, Form, FormKind};
use crate::worker::{CfReq, CfResp, Resp};

#[derive(Default)]
pub(super) struct State {
    section: String,
    initialized: bool,
    loaded: HashSet<String>,
    errors: HashMap<String, String>,
    status: String,
    zones: Vec<Zone>,
    records: Vec<Record>,
    tunnels: Vec<CloudflareTunnel>,
    buckets: Vec<R2Bucket>,
    workers: Vec<WorkerScript>,
    analytics: Option<AnalyticsSummary>,
    sites: Vec<WebAnalyticsSite>,
    days: u16,
    detailed: bool,
    filter: String,
    record_filter: RecordFilter,
    selected: Option<String>,
    route_filter: String,
    settings_filter: String,
    settings_selected: Option<String>,
    deployments_filter: String,
    zone: Option<Zone>,
    tunnel: Option<CloudflareTunnel>,
    config: Option<TunnelConfiguration>,
    bucket: Option<String>,
    prefix: String,
    folder_input: String,
    folders: Vec<String>,
    objects: Vec<R2Object>,
    truncated: bool,
    worker: Option<String>,
    settings: Option<WorkerSettingsBundle>,
    deployments: Vec<WorkerDeployment>,
    marked: HashSet<String>,
    editor: Option<Form>,
    baseline: Vec<String>,
    confirmation: Option<Review>,
    confirm_entry: String,
    file: Option<PathBuf>,
    token: Option<TunnelToken>,
    token_request: Option<(String, String)>,
    reveal_token: bool,
    failed: Vec<(String, String)>,
}

struct TunnelToken {
    account_id: String,
    tunnel_id: String,
    value: String,
}

struct Review {
    title: String,
    target: String,
    typed: Option<String>,
    details: Vec<String>,
    action: Action,
}

enum Action {
    Cf(CfReq),
    Upload {
        account: CloudflareAccount,
        bucket: String,
        key: String,
        path: PathBuf,
    },
    Deploy {
        account: CloudflareAccount,
        name: String,
        path: PathBuf,
        mode: WorkerUploadMode,
    },
    Download {
        account: CloudflareAccount,
        bucket: Option<String>,
        files: Vec<(String, PathBuf)>,
        overwrite: bool,
    },
    RemoveAccount(String),
    DefaultAccount(String),
}

impl State {
    pub(super) fn is_dirty(&self) -> bool {
        self.file.is_some()
            || self.editor.as_ref().is_some_and(|f| {
                f.fields
                    .iter()
                    .map(|x| x.value.as_str())
                    .ne(self.baseline.iter().map(String::as_str))
            })
    }

    fn accept_token(&mut self, current_account_id: &str, value: String) {
        let Some((account_id, tunnel_id)) = self.token_request.take() else {
            return;
        };
        if account_id == current_account_id
            && self.tunnel.as_ref().is_some_and(|t| t.id == tunnel_id)
        {
            self.token = Some(TunnelToken {
                account_id,
                tunnel_id,
                value,
            });
            self.reveal_token = false;
        }
    }

    fn browse_prefix(&mut self, prefix: String) {
        self.prefix = prefix;
        self.folder_input.clone_from(&self.prefix);
        self.loaded.remove("objects");
        self.selected = None;
        self.marked.clear();
        self.objects.clear();
        self.folders.clear();
        self.truncated = false;
    }

    fn edit(&mut self, form: Form) {
        self.baseline = form.fields.iter().map(|f| f.value.clone()).collect();
        self.editor = Some(form);
        self.file = None;
    }

    fn review(
        &mut self,
        account: &CloudflareAccount,
        title: &str,
        resource: &str,
        typed: bool,
        details: Vec<String>,
        action: Action,
    ) {
        self.confirm_entry.clear();
        self.confirmation = Some(Review {
            title: title.into(),
            target: format!("Cloudflare account {} / {}", account.name, resource),
            typed: typed.then(|| resource.to_owned()),
            details,
            action,
        });
    }

    fn receive(&mut self, cx: &mut PageContext<'_>) {
        for key in [
            "zones",
            "records",
            "tunnels",
            "config",
            "buckets",
            "objects",
            "workers",
            "settings",
            "deployments",
            "analytics",
            "sites",
            "token",
            "mutation",
            "account",
            "transfer",
        ] {
            let Some(result) = cx.take(key) else { continue };
            match result {
                Err(e) => {
                    if key == "token" {
                        self.token_request = None;
                    }
                    self.errors.insert(key.into(), e);
                }
                Ok(Payload::Text(text)) => {
                    self.errors.remove(key);
                    if key == "token" {
                        self.accept_token(
                            cx.target
                                .account
                                .as_ref()
                                .and_then(|a| a.account_id.as_deref())
                                .unwrap_or(""),
                            text,
                        );
                    } else {
                        self.status = text;
                    }
                    if key == "account" {
                        self.editor = None;
                        self.baseline.clear();
                        cx.reload_connections();
                    }
                    if key == "transfer" {
                        self.editor = None;
                        self.file = None;
                        self.baseline.clear();
                        self.loaded.remove("objects");
                        self.loaded.remove("workers");
                        self.loaded.remove("settings");
                        self.loaded.remove("deployments");
                    }
                }
                Ok(Payload::Json(value)) if key == "transfer" => {
                    self.errors.remove(key);
                    let saved = value
                        .get("saved")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    self.failed = serde_json::from_value(
                        value.get("failed").cloned().unwrap_or_else(|| json!([])),
                    )
                    .unwrap_or_default();
                    self.status = format!(
                        "{} files saved; {} failed.\n{}",
                        saved.len(),
                        self.failed.len(),
                        saved
                            .iter()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join("\n")
                    );
                    self.marked = self.failed.iter().map(|(key, _)| key.clone()).collect();
                }
                Ok(Payload::Worker(Resp::Cf(reply))) => {
                    self.errors.remove(key);
                    match reply {
                        CfResp::Zones(v) => self.zones = v,
                        CfResp::Records { zone_id, records } => {
                            if self.zone.as_ref().is_some_and(|z| z.id == zone_id) {
                                self.records = records;
                                self.marked
                                    .retain(|id| self.records.iter().any(|r| &r.id == id));
                            }
                        }
                        CfResp::Tunnels(v) => self.tunnels = v,
                        CfResp::R2Buckets(v) => self.buckets = v,
                        CfResp::Workers(v) => self.workers = v,
                        CfResp::Analytics(v) => self.analytics = Some(v),
                        CfResp::WebAnalyticsSites(v) => self.sites = v,
                        CfResp::TunnelConfig { tunnel_id, config } => {
                            if self.tunnel.as_ref().is_some_and(|t| t.id == tunnel_id) {
                                self.config = Some(*config);
                            }
                        }
                        CfResp::R2Objects {
                            bucket,
                            prefix,
                            folders,
                            objects,
                            truncated,
                        } => {
                            if self.bucket.as_ref() == Some(&bucket) && self.prefix == prefix {
                                self.folders = folders;
                                self.objects = objects;
                                self.truncated = truncated;
                                self.marked
                                    .retain(|key| self.objects.iter().any(|o| &o.key == key));
                            }
                        }
                        CfResp::WorkerSettings { worker, settings } => {
                            if self.worker.as_ref() == Some(&worker) {
                                self.settings = Some(*settings);
                            }
                        }
                        CfResp::WorkerDeployments {
                            worker,
                            deployments,
                        } => {
                            if self.worker.as_ref() == Some(&worker) {
                                self.deployments = deployments;
                            }
                        }
                        CfResp::Done(text) => {
                            self.status = text;
                            self.mutation_finished();
                        }
                        CfResp::BulkDone { ok, failed } => {
                            self.status = format!("{ok} succeeded; {} failed. Failed items remain marked; refresh before retrying.", failed.len());
                            self.marked = failed.iter().map(|(id, _)| id.clone()).collect();
                            self.failed = failed;
                            self.mutation_finished();
                        }
                        CfResp::Status(text) => self.status = text,
                        CfResp::Err(e) | CfResp::WebAnalyticsErr(e) => {
                            self.errors.insert(key.into(), e);
                            if key == "mutation" {
                                self.loaded.remove("config");
                            }
                        }
                        CfResp::Viewer { title: _, lines } => self.status = lines.join("\n"),
                    }
                }
                Ok(_) => {
                    self.errors.insert(
                        key.into(),
                        "Unexpected Cloudflare response; no local state changed".into(),
                    );
                }
            }
        }
    }

    fn mutation_finished(&mut self) {
        self.editor = None;
        self.file = None;
        self.baseline.clear();
        for key in [
            "zones",
            "records",
            "tunnels",
            "config",
            "buckets",
            "objects",
            "workers",
            "settings",
            "deployments",
        ] {
            self.loaded.remove(key);
        }
    }

    fn load(&mut self, cx: &mut PageContext<'_>, account: &CloudflareAccount) {
        let token = account.api_token.clone();
        let id = account.account_id.clone().unwrap_or_default();
        let (key, req) = match self.section.as_str() {
            "analytics" if !id.is_empty() => (
                "analytics",
                CfReq::Analytics {
                    token,
                    account_id: id,
                    days: self.days.max(1),
                },
            ),
            "zones" if self.zone.is_some() => (
                "records",
                CfReq::Records {
                    token,
                    zone_id: self.zone.as_ref().unwrap().id.clone(),
                    filter: self.record_filter.clone(),
                },
            ),
            "zones" => (
                "zones",
                CfReq::Zones {
                    token,
                    account_id: account.account_id.clone(),
                },
            ),
            "tunnels" if !id.is_empty() && self.tunnel.is_some() => (
                "config",
                CfReq::TunnelConfig {
                    token,
                    account_id: id,
                    tunnel_id: self.tunnel.as_ref().unwrap().id.clone(),
                },
            ),
            "tunnels" if !id.is_empty() => (
                "tunnels",
                CfReq::Tunnels {
                    token,
                    account_id: id,
                },
            ),
            "r2" if !id.is_empty() && self.bucket.is_some() => (
                "objects",
                CfReq::R2Objects {
                    token,
                    account_id: id,
                    bucket: self.bucket.clone().unwrap(),
                    prefix: self.prefix.clone(),
                },
            ),
            "r2" if !id.is_empty() => (
                "buckets",
                CfReq::R2Buckets {
                    token,
                    account_id: id,
                },
            ),
            "workers" if !id.is_empty() => (
                "workers",
                CfReq::Workers {
                    token,
                    account_id: id,
                },
            ),
            _ => return,
        };
        if !self.loaded.contains(key) && !cx.busy(key) {
            self.loaded.insert(key.into());
            cx.cloudflare(key, req);
        }
        if self.section == "analytics"
            && account.account_id.is_some()
            && !self.loaded.contains("sites")
            && !cx.busy("sites")
        {
            self.loaded.insert("sites".into());
            cx.cloudflare(
                "sites",
                CfReq::WebAnalyticsSites {
                    token: account.api_token.clone(),
                    account_id: account.account_id.clone().unwrap(),
                },
            );
        }
        if self.section == "workers" {
            if let (Some(name), Some(id)) = (&self.worker, &account.account_id) {
                if !self.loaded.contains("settings") && !cx.busy("settings") {
                    self.loaded.insert("settings".into());
                    cx.cloudflare(
                        "settings",
                        CfReq::WorkerSettings {
                            token: account.api_token.clone(),
                            account_id: id.clone(),
                            name: name.clone(),
                        },
                    );
                }
                if !self.loaded.contains("deployments") && !cx.busy("deployments") {
                    self.loaded.insert("deployments".into());
                    cx.cloudflare(
                        "deployments",
                        CfReq::WorkerDeployments {
                            token: account.api_token.clone(),
                            account_id: id.clone(),
                            name: name.clone(),
                        },
                    );
                }
            }
        }
    }
}

pub(super) fn show(ui: &mut egui::Ui, state: &mut State, cx: &mut PageContext<'_>) {
    if !state.initialized {
        state.section = match cx.target.section.as_str() {
            "analytics" | "tunnels" | "r2" | "workers" | "accounts" => cx.target.section.clone(),
            "records" => {
                state.zone = serde_json::from_value(cx.target.resource.clone()).ok();
                "zones".into()
            }
            "tunnel-routes" => {
                state.tunnel = serde_json::from_value(cx.target.resource.clone()).ok();
                "tunnels".into()
            }
            "objects" => {
                state.bucket = serde_json::from_value::<R2Bucket>(cx.target.resource.clone())
                    .ok()
                    .map(|b| b.name);
                "r2".into()
            }
            "worker-settings" | "worker-deployments" => {
                state.worker = serde_json::from_value::<WorkerScript>(cx.target.resource.clone())
                    .ok()
                    .map(|w| w.id);
                "workers".into()
            }
            _ => "zones".into(),
        };
        state.days = 7;
        state.initialized = true;
    }
    state.receive(cx);
    let account = cx.target.account.clone();
    ui.horizontal_wrapped(|ui| {
        for (section, title) in [("zones", "Zones & DNS"), ("tunnels", "Tunnels"), ("r2", "R2 storage"), ("workers", "Workers"), ("analytics", "Analytics"), ("accounts", "Accounts")] {
            if ui.selectable_label(state.section == section, title).clicked() && state.section != section {
                if state.is_dirty() || state.confirmation.is_some() || cx.busy("mutation") || cx.busy("transfer") || cx.busy("account") { state.status = "Save/discard the current edit, or wait for the scoped operation before changing product.".into(); }
                else { state.section = section.into(); state.editor = None; state.baseline.clear(); state.filter.clear(); state.selected = None; state.marked.clear(); state.token = None; }
            }
        }
    });
    ui.separator();
    if state.section == "accounts" || account.is_none() {
        accounts(ui, state, cx);
    } else if let Some(account) = &account {
        ui.horizontal(|ui| {
            ui.strong(&account.name);
            ui.label(
                account
                    .account_id
                    .as_deref()
                    .unwrap_or("Zone-scoped token • no account ID"),
            );
            if ui
                .add_enabled(!state.is_dirty(), egui::Button::new("Refresh"))
                .clicked()
            {
                state.loaded.clear();
            }
        });
        if state.section != "zones" && account.account_id.as_deref().unwrap_or("").is_empty() {
            widgets::error(ui, "This product requires an account ID. Edit this Cloudflare account in Accounts; EasyPanel server selection is unrelated.");
        } else {
            state.load(cx, account);
            if state.editor.is_none()
                && state.confirmation.is_none()
                && !cx.busy("mutation")
                && !cx.busy("transfer")
            {
                match state.section.as_str() {
                    "analytics" => analytics(ui, state),
                    "tunnels" => tunnels(ui, state, cx, account),
                    "r2" => r2(ui, state, cx, account),
                    "workers" => workers(ui, state, cx, account),
                    _ => dns(ui, state, cx, account),
                }
            }
        }
    }
    if let Some(account) = &account {
        editor(ui, state, cx, account);
    } else if state.editor.is_some() {
        account_editor(ui, state, cx);
    }
    review(ui, state, cx);
    for key in [
        "zones",
        "records",
        "tunnels",
        "config",
        "buckets",
        "objects",
        "workers",
        "settings",
        "deployments",
        "analytics",
        "sites",
        "token",
        "mutation",
        "transfer",
        "account",
    ] {
        if let Some(e) = state.errors.get(key) {
            widgets::error(ui, &format!("{key}: {e}"));
        }
        if cx.busy(key) {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(cx.progress(key).unwrap_or(key));
            });
        }
    }
    if !state.status.is_empty() {
        ui.separator();
        ui.label(&state.status);
    }
    if !state.failed.is_empty() {
        egui::CollapsingHeader::new("Per-item failures")
            .default_open(true)
            .show(ui, |ui| {
                for (name, error) in &state.failed {
                    widgets::error(ui, &format!("{name}: {error}"));
                }
            });
    }
}

fn accounts(ui: &mut egui::Ui, state: &mut State, cx: &mut PageContext<'_>) {
    if state.editor.is_some() || state.confirmation.is_some() || cx.busy("account") {
        return;
    }
    ui.horizontal(|ui| {
        ui.heading("Cloudflare accounts");
        if ui.button("Add account").clicked() {
            state.edit(Form::new(
                FormKind::CfAccountAdd,
                "Add Cloudflare account",
                vec![
                    Field::text("Name", ""),
                    Field::secret_val("API token", ""),
                    Field::text("Account ID", ""),
                ],
            ));
        }
    });
    ui.label("Independent of EasyPanel hosts. Tokens stay masked; changing the selected account does not change the CLI default.");
    let rows = cx
        .accounts
        .iter()
        .map(|a| {
            vec![
                a.name.clone(),
                a.account_id.clone().unwrap_or_default(),
                if a.default {
                    "Default".into()
                } else {
                    String::new()
                },
                "••••••••".into(),
            ]
        })
        .collect::<Vec<_>>();
    let index = resource_table(
        ui,
        "cf-accounts",
        &["Account", "Account ID", "CLI default", "API token"],
        &rows,
        cx.accounts.iter().map(|a| a.name.as_str()),
        &mut state.selected,
    );
    if let Some(account) = index.and_then(|i| cx.accounts.get(i)).cloned() {
        ui.horizontal_wrapped(|ui| {
            if ui.button("Open account").clicked() {
                let mut target = cx.target.clone(); target.server = None; target.account = Some(account.clone()); target.section = "zones".into(); target.resource = Value::Null;
                cx.open(OpenPage { kind: PageKind::Cloudflare, target, title: format!("{} / Cloudflare", account.name) });
            }
            if ui.button("Edit / rename / rotate token").clicked() {
                state.edit(Form::new(FormKind::CfAccountEdit { name: account.name.clone() }, "Edit Cloudflare account", vec![Field::text("Name", &account.name), Field::secret_val("API token", ""), Field::text("Account ID", account.account_id.as_deref().unwrap_or(""))]).with_note("Leave API token blank to preserve it. Existing documents retain their original account context; reopen after rotation."));
            }
            if ui.button("Set CLI default…").clicked() { state.review(&account, "Set CLI default account", &account.name, false, vec!["New CLI invocations will use this account unless explicitly selected.".into()], Action::DefaultAccount(account.name.clone())); }
            if ui.button("Remove account…").clicked() { state.review(&account, "Remove stored account", &account.name, true, vec!["Removes the local stored token, not remote Cloudflare resources.".into()], Action::RemoveAccount(account.name.clone())); }
            if ui.button("Test token").clicked() {
                let account = account.clone();
                let client = cx.cloudflare_client(&account);
                cx.background("account", "Test Cloudflare token", move || Ok(Payload::Text(super::connections::test_cf_account(&account, &client)?)));
            }
        });
    }
    if cx.accounts.is_empty() {
        ui.label("Add a scoped Cloudflare token to manage zones, Tunnels, R2 and Workers.");
    }
}

fn account_editor(ui: &mut egui::Ui, state: &mut State, cx: &mut PageContext<'_>) {
    let Some(form) = state.editor.as_mut() else {
        return;
    };
    ui.heading(&form.title);
    ui.add_enabled_ui(!cx.busy("account"), |ui| widgets::form_fields(ui, form));
    let mut save = false;
    let mut discard = false;
    ui.horizontal(|ui| {
        save = ui
            .add_enabled(!cx.busy("account"), egui::Button::new("Save account"))
            .clicked();
        discard = ui
            .add_enabled(!cx.busy("account"), egui::Button::new("Discard"))
            .clicked();
    });
    if discard {
        state.editor = None;
        state.baseline.clear();
        return;
    }
    if save {
        let form = state.editor.as_ref().unwrap();
        let original = match &form.kind {
            FormKind::CfAccountEdit { name } => Some(name.clone()),
            _ => None,
        };
        let account = CloudflareAccount {
            name: form.by_label("Name"),
            api_token: form.by_label("API token"),
            account_id: nonempty(form.by_label("Account ID")),
            default: false,
        };
        let config_path = cx.cloudflare_config_path().to_owned();
        cx.background("account", "Save Cloudflare account", move || {
            super::connections::save_cf_account(
                &CloudflareConfig::new(config_path),
                original.as_deref(),
                account,
            )?;
            Ok(Payload::Text(
                "Account saved. Reopen its resource document to use changed credentials.".into(),
            ))
        });
    }
}

fn analytics(ui: &mut egui::Ui, state: &mut State) {
    ui.horizontal(|ui| {
        ui.label("Days");
        if ui
            .add(egui::DragValue::new(&mut state.days).range(1..=365))
            .changed()
        {
            state.loaded.remove("analytics");
        }
        ui.checkbox(&mut state.detailed, "Detailed breakdown");
    });
    if let Some(a) = &state.analytics {
        ui.label(format!(
            "Last {} days • {} requests • {} bytes • {} visits",
            a.days, a.requests, a.bandwidth, a.visits
        ));
        let rows = a
            .countries
            .iter()
            .map(|c| {
                vec![
                    c.country.clone(),
                    c.requests.to_string(),
                    c.bandwidth.to_string(),
                ]
            })
            .collect::<Vec<_>>();
        widgets::table(
            ui,
            "cf-countries",
            &["Country", "Requests", "Bytes"],
            &rows,
            &mut None,
        );
        if state.detailed {
            for (name, metrics) in [
                ("TLS", &a.ssl),
                ("Cache", &a.cache),
                ("HTTP status", &a.status),
                ("HTTP protocol", &a.protocols),
                ("Content type", &a.content_types),
            ] {
                egui::CollapsingHeader::new(name)
                    .default_open(true)
                    .show(ui, |ui| {
                        for m in metrics {
                            ui.horizontal(|ui| {
                                ui.label(&m.label);
                                ui.monospace(m.value.to_string());
                            });
                        }
                        if metrics.is_empty() {
                            ui.label("No breakdown returned for this period.");
                        }
                    });
            }
        }
    }
    egui::CollapsingHeader::new("Web Analytics sites").show(ui, |ui| {
        let rows = state.sites.iter().map(|s| vec![s.host.clone(), s.zone_name.clone(), s.enabled.to_string(), s.auto_install.to_string(), s.page_views_24h.map(|v| v.to_string()).unwrap_or_else(|| "Not returned".into()), s.visits_24h.map(|v| v.to_string()).unwrap_or_else(|| "Not returned".into())]).collect::<Vec<_>>();
        widgets::table(ui, "cf-sites", &["Host", "Zone", "Enabled", "Auto install", "Page views / 24h", "Visits / 24h"], &rows, &mut None);
        ui.label("Site metadata is separate from account traffic totals. Missing optional totals are not zero.");
    });
}

fn filter(ui: &mut egui::Ui, value: &mut String) -> bool {
    ui.horizontal(|ui| {
        ui.label("Filter");
        let mut changed = ui
            .add(
                egui::TextEdit::singleline(value).hint_text(
                    "Case-insensitive text or regex; invalid regex uses literal matching",
                ),
            )
            .changed();
        if ui.button("Clear").clicked() {
            changed |= !value.is_empty();
            value.clear();
        }
        changed
    })
    .inner
}

fn selected_row<'a>(
    mut keys: impl Iterator<Item = &'a str>,
    selected: Option<&str>,
) -> Option<usize> {
    selected.and_then(|selected| keys.position(|key| key == selected))
}

fn resource_table<'a>(
    ui: &mut egui::Ui,
    id: &str,
    columns: &[&str],
    rows: &[Vec<String>],
    keys: impl Iterator<Item = &'a str> + Clone,
    selected: &mut Option<String>,
) -> Option<usize> {
    let mut index = selected_row(keys.clone(), selected.as_deref());
    if let Some(clicked) = widgets::table(ui, id, columns, rows, &mut index) {
        *selected = keys.clone().nth(clicked).map(str::to_owned);
    }
    index
}

fn dns(
    ui: &mut egui::Ui,
    state: &mut State,
    cx: &mut PageContext<'_>,
    account: &CloudflareAccount,
) {
    if let Some(zone) = state.zone.clone() {
        ui.horizontal_wrapped(|ui| {
            if ui.button("All zones").clicked() {
                state.zone = None;
                state.selected = None;
                state.marked.clear();
                state.filter.clear();
            }
            ui.strong(&zone.name);
            if ui.button("Add DNS record").clicked() {
                state.edit(record_form(None));
            }
        });
        ui.collapsing("Server-side DNS filters", |ui| {
            let mut kind = state.record_filter.kind.clone().unwrap_or_default();
            let mut name = state.record_filter.name.clone().unwrap_or_default();
            let mut content = state.record_filter.content.clone().unwrap_or_default();
            ui.horizontal(|ui| {
                ui.label("Type");
                ui.text_edit_singleline(&mut kind);
            });
            ui.horizontal(|ui| {
                ui.label("Name contains");
                ui.text_edit_singleline(&mut name);
            });
            ui.horizontal(|ui| {
                ui.label("Content contains");
                ui.text_edit_singleline(&mut content);
            });
            state.record_filter = RecordFilter {
                kind: nonempty(kind),
                name: nonempty(name),
                content: nonempty(content),
            };
            if ui.button("Apply filters").clicked() {
                state.loaded.remove("records");
                state.marked.clear();
            }
        });
        filter(ui, &mut state.filter);
        let shown = filter_records(&state.records, &state.filter);
        ui.horizontal_wrapped(|ui| {
            ui.label(format!(
                "{} shown / {} loaded • {} marked",
                shown.len(),
                state.records.len(),
                state.marked.len()
            ));
            if ui.button("Mark filtered").clicked() {
                state.marked.extend(shown.iter().map(|r| r.id.clone()));
            }
            if ui.button("Clear marks").clicked() {
                state.marked.clear();
            }
        });
        let rows = shown
            .iter()
            .map(|r| {
                vec![
                    if state.marked.contains(&r.id) {
                        "Marked".into()
                    } else {
                        String::new()
                    },
                    r.kind.clone(),
                    r.name.clone(),
                    r.content.clone(),
                    r.ttl.to_string(),
                    r.proxied.to_string(),
                    r.priority.map(|p| p.to_string()).unwrap_or_default(),
                ]
            })
            .collect::<Vec<_>>();
        let index = resource_table(
            ui,
            "cf-records",
            &[
                "Bulk", "Type", "Name", "Content", "TTL", "Proxied", "Priority",
            ],
            &rows,
            shown.iter().map(|r| r.id.as_str()),
            &mut state.selected,
        );
        let selected = index.and_then(|i| shown.get(i)).map(|r| (*r).clone());
        drop(shown);
        if let Some(record) = selected {
            ui.horizontal_wrapped(|ui| {
                if ui.button("Toggle mark").clicked() && !state.marked.remove(&record.id) {
                    state.marked.insert(record.id.clone());
                }
                if valid_record_type(&record.kind) && ui.button("Edit record").clicked() {
                    state.edit(record_form(Some(&record)));
                }
                if ui.button("Delete record…").clicked() {
                    state.review(
                        account,
                        "Delete DNS record",
                        &record.name,
                        false,
                        vec![format!("{} {} • {}", record.kind, record.name, record.id)],
                        Action::Cf(CfReq::DeleteRecord {
                            token: account.api_token.clone(),
                            zone_id: zone.id.clone(),
                            id: record.id.clone(),
                        }),
                    );
                }
            });
        }
        if !state.marked.is_empty() {
            ui.horizontal_wrapped(|ui| {
                for (label, attr) in [("Set content…", CfBulkAttr::Content), ("Set proxy…", CfBulkAttr::Proxied), ("Set TTL…", CfBulkAttr::Ttl), ("Set priority…", CfBulkAttr::Priority)] {
                    if ui.button(label).clicked() {
                        let field = match attr { CfBulkAttr::Content => Field::text("Content", ""), CfBulkAttr::Proxied => Field::boolean("Proxied", true), CfBulkAttr::Ttl => Field::text("TTL", "1"), CfBulkAttr::Priority => Field::text("Priority", "0") };
                        state.edit(Form::new(FormKind::CfBulkSet(attr), "Patch marked DNS records", vec![field]).with_note("Only marked IDs are patched; unchanged attributes stay intact. Batches are capped at 200 records. Priority requires MX records and an integer from 0 to 65535."));
                    }
                }
                if ui.button("Delete marked…").clicked() {
                    let ids = marked_record_ids(state);
                    let details = marked_dns_details(state, None);
                    state.review(account, "Delete marked DNS records", &zone.name, true, details, Action::Cf(CfReq::BulkDelete { token: account.api_token.clone(), zone_id: zone.id.clone(), ids }));
                }
            });
        }
    } else {
        ui.horizontal(|ui| {
            ui.heading("Zones");
            if ui
                .add_enabled(
                    account.account_id.is_some(),
                    egui::Button::new("Create zone"),
                )
                .clicked()
            {
                state.edit(Form::new(
                    FormKind::CfZoneCreate,
                    "Create zone",
                    vec![Field::text("Name", "")],
                ));
            }
        });
        filter(ui, &mut state.filter);
        let shown = filter_zones(&state.zones, &state.filter);
        let rows = shown
            .iter()
            .map(|z| vec![z.name.clone(), z.status.clone(), z.id.clone()])
            .collect::<Vec<_>>();
        let index = resource_table(
            ui,
            "cf-zones",
            &["Zone", "Status", "ID"],
            &rows,
            shown.iter().map(|z| z.id.as_str()),
            &mut state.selected,
        );
        let selected = index.and_then(|i| shown.get(i)).map(|z| (*z).clone());
        if let Some(zone) = selected {
            ui.horizontal(|ui| {
                if ui.button("Open DNS records").clicked() {
                    state.zone = Some(zone.clone());
                    state.loaded.remove("records");
                    state.selected = None;
                    state.records.clear();
                    state.filter.clear();
                    state.record_filter = RecordFilter::default();
                }
                if ui.button("Delete zone…").clicked() {
                    state.review(
                        account,
                        "Delete zone and ALL DNS records",
                        &zone.name,
                        true,
                        vec![format!("Zone ID {} • irreversible", zone.id)],
                        Action::Cf(CfReq::DeleteZone {
                            token: account.api_token.clone(),
                            zone_id: zone.id.clone(),
                        }),
                    );
                }
            });
        }
    }
    let _ = cx;
}

fn record_form(record: Option<&Record>) -> Form {
    if let Some(r) = record {
        let mut fields = vec![
            Field::text("Content", &r.content),
            Field::text("TTL", &r.ttl.to_string()),
        ];
        if proxyable(&r.kind) {
            fields.push(Field::boolean("Proxied", r.proxied));
        }
        if r.kind.eq_ignore_ascii_case("MX") {
            fields.push(Field::text(
                "Priority",
                &r.priority.map(|p| p.to_string()).unwrap_or_default(),
            ));
        }
        Form::new(
            FormKind::CfRecordEdit {
                id: r.id.clone(),
                kind: r.kind.clone(),
            },
            format!("Edit {} {}", r.kind, r.name),
            fields,
        )
        .with_note("PATCH changes only these fields. TTL 1 means automatic.")
    } else {
        Form::new(FormKind::CfRecordCreate, "Add DNS record", vec![Field::choice("Type", &["A", "AAAA", "CNAME", "TXT", "NS", "MX"], "A"), Field::text("Name", ""), Field::text("Content", ""), Field::text("TTL", "1"), Field::boolean("Proxied", false).when("Type", "A,AAAA,CNAME"), Field::text("Priority", "").when("Type", "MX")]).with_note("TTL 1 means automatic. Structured DNS types not supported by the existing client can still be listed/deleted.")
    }
}

fn marked_record_ids(state: &State) -> Vec<String> {
    state
        .records
        .iter()
        .filter(|r| state.marked.contains(&r.id))
        .map(|r| r.id.clone())
        .collect()
}
fn marked_dns_details(state: &State, patch: Option<&Value>) -> Vec<String> {
    state
        .records
        .iter()
        .filter(|r| state.marked.contains(&r.id))
        .map(|r| {
            let before = format!(
                "{} {} [{}]: content={} TTL={} proxied={} priority={}",
                r.kind,
                r.name,
                r.id,
                r.content,
                r.ttl,
                r.proxied,
                r.priority
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| "none".into())
            );
            match patch {
                Some(p) => format!("{before} → {}", p),
                None => before,
            }
        })
        .collect()
}

fn tunnels(
    ui: &mut egui::Ui,
    state: &mut State,
    cx: &mut PageContext<'_>,
    account: &CloudflareAccount,
) {
    let id = account.account_id.clone().unwrap();
    if let Some(tunnel) = state.tunnel.clone() {
        ui.horizontal_wrapped(|ui| {
            if ui.button("All tunnels").clicked() {
                state.tunnel = None;
                state.config = None;
                state.selected = None;
                state.token = None;
            }
            ui.strong(&tunnel.name);
            ui.label(&tunnel.status);
            if ui.button("Add published route").clicked() {
                state.edit(route_form(&tunnel.id, None));
            }
            if ui
                .add_enabled(!cx.busy("token"), egui::Button::new("Fetch install token"))
                .clicked()
            {
                state.token_request = Some((id.clone(), tunnel.id.clone()));
                let account = account.clone();
                let tunnel_id = tunnel.id.clone();
                let client = cx.cloudflare_client(&account);
                cx.background("token", "Retrieve connector secret", move || {
                    Ok(Payload::Text(client.get_tunnel_token(
                        account.account_id.as_deref().unwrap(),
                        &tunnel_id,
                    )?))
                });
            }
        });
        if let Some(scoped) = state
            .token
            .as_ref()
            .filter(|t| t.account_id == id && t.tunnel_id == tunnel.id)
        {
            let token = &scoped.value;
            ui.horizontal(|ui| {
                ui.checkbox(&mut state.reveal_token, "Reveal connector secret");
                if ui.button("Copy install command").clicked() {
                    ui.ctx()
                        .copy_text(format!("sudo cloudflared service install {token}"));
                }
                if ui.button("Copy Docker command").clicked() {
                    ui.ctx().copy_text(format!("docker run -d --name cloudflared --restart unless-stopped cloudflare/cloudflared:latest tunnel --no-autoupdate run --token {token}"));
                }
            });
            ui.label("Install cloudflared on the origin. This application retrieves the token; it does not start a connector.");
            if state.reveal_token {
                ui.add(egui::TextEdit::singleline(&mut token.clone()).interactive(false));
            }
        }
        filter(ui, &mut state.route_filter);
        if let Some(config) = &state.config {
            ui.label(format!(
                "Configuration version {} • source {} • catch-all retained automatically",
                config.version.map(|v| v.to_string()).unwrap_or_default(),
                config.source
            ));
            let matcher = crate::filter::FilterMatcher::new(&state.route_filter);
            let shown = config
                .config
                .ingress
                .iter()
                .filter(|r| {
                    matcher.matches_any([
                        r.hostname_label().as_str(),
                        r.service.as_str(),
                        r.origin_label().as_str(),
                    ])
                })
                .collect::<Vec<_>>();
            let rows = shown
                .iter()
                .map(|r| vec![r.hostname_label(), r.service.clone(), r.origin_label()])
                .collect::<Vec<_>>();
            let keys = shown
                .iter()
                .map(|r| format!("{}\0{}", r.hostname, r.path))
                .collect::<Vec<_>>();
            let index = resource_table(
                ui,
                "cf-routes",
                &["Hostname / path", "Origin service", "Origin options"],
                &rows,
                keys.iter().map(String::as_str),
                &mut state.selected,
            );
            let selected = index.and_then(|i| shown.get(i)).map(|r| (*r).clone());
            if let Some(rule) = selected {
                if rule.is_catch_all() {
                    ui.label(
                        "The catch-all is managed automatically and is preserved by route edits.",
                    );
                } else {
                    ui.horizontal(|ui| {
                    if ui.button("Edit published route").clicked() { state.edit(route_form(&tunnel.id, Some(&rule))); }
                    if ui.button("Delete route…").clicked() { state.edit(Form::new(FormKind::CfTunnelRouteDelete { tunnel_id: tunnel.id.clone(), hostname: rule.hostname.clone(), path: rule.path.clone() }, format!("Delete route {}", rule.hostname_label()), vec![Field::boolean("Delete DNS CNAME", true), Field::text("DNS Zone", "")]).with_note("DNS deletion is optional. A DNS failure can occur after ingress was already changed; refresh before retrying.")); }
                });
                }
            }
        }
    } else {
        ui.horizontal(|ui| {
            ui.heading("Cloudflare Tunnels");
            if ui.button("Create tunnel").clicked() {
                state.edit(Form::new(
                    FormKind::CfTunnelCreate,
                    "Create remotely managed tunnel",
                    vec![Field::text("Tunnel name", "")],
                ));
            }
        });
        filter(ui, &mut state.filter);
        let shown = filter_tunnels(&state.tunnels, &state.filter);
        let rows = shown
            .iter()
            .map(|t| {
                vec![
                    t.name.clone(),
                    t.status.clone(),
                    t.config_src.clone(),
                    t.connections.len().to_string(),
                    t.id.clone(),
                ]
            })
            .collect::<Vec<_>>();
        let index = resource_table(
            ui,
            "cf-tunnels",
            &["Tunnel", "Status", "Configuration", "Connections", "ID"],
            &rows,
            shown.iter().map(|t| t.id.as_str()),
            &mut state.selected,
        );
        let selected = index.and_then(|i| shown.get(i)).map(|t| (*t).clone());
        if let Some(t) = selected {
            ui.horizontal(|ui| {
                if ui.button("Published routes & install").clicked() { state.tunnel = Some(t.clone()); state.config = None; state.loaded.remove("config"); state.selected = None; state.token = None; }
                if ui.button("Delete tunnel…").clicked() { state.review(account, "Delete Cloudflare Tunnel", &t.name, true, vec![format!("Tunnel {}. DNS records and existing routes are not deleted automatically.", t.id)], Action::Cf(CfReq::TunnelDelete { token: account.api_token.clone(), account_id: id, tunnel_id: t.id.clone(), name: t.name.clone() })); }
            });
        }
    }
}

fn route_form(tunnel_id: &str, rule: Option<&TunnelIngressRule>) -> Form {
    let mut fields = if rule.is_some() {
        Vec::new()
    } else {
        vec![Field::text("Hostname", ""), Field::text("Path", "")]
    };
    fields.extend(form::tunnel_service_fields(
        rule.map(|r| r.service.as_str())
            .unwrap_or("http://localhost:3000"),
    ));
    fields.extend([
        Field::boolean("Sync DNS CNAME", true),
        Field::text("DNS Zone", "").when("Sync DNS CNAME", "yes"),
        Field::boolean("DNS Proxied", true).when("Sync DNS CNAME", "yes"),
    ]);
    fields.extend(form::tunnel_origin_fields(
        rule.and_then(|r| r.origin_request.as_ref()),
    ));
    let kind = match rule {
        Some(r) => {
            fields.push(Field::boolean("Clear origin request", false));
            FormKind::CfTunnelRouteEdit {
                tunnel_id: tunnel_id.into(),
                hostname: r.hostname.clone(),
                path: r.path.clone(),
            }
        }
        None => FormKind::CfTunnelRouteCreate {
            tunnel_id: tunnel_id.into(),
        },
    };
    let mut f = Form::new(kind, "Published tunnel route", fields).with_note("Unknown origin options and the catch-all are preserved. DNS sync follows ingress save; errors may mean a partial outcome. Blank DNS zone resolves from the hostname.");
    if let Some(r) = rule {
        f.original = r.origin_request.clone();
    }
    f
}

fn r2(ui: &mut egui::Ui, state: &mut State, cx: &mut PageContext<'_>, account: &CloudflareAccount) {
    if let Some(bucket) = state.bucket.clone() {
        ui.horizontal_wrapped(|ui| {
            if ui.button("All buckets").clicked() { state.bucket = None; state.selected = None; state.marked.clear(); }
            ui.strong(&bucket); ui.label(if state.prefix.is_empty() { "/" } else { &state.prefix });
            if !state.prefix.is_empty() && ui.button("Parent folder").clicked() { state.browse_prefix(parent_prefix(&state.prefix)); }
            if ui.button("Upload file…").clicked() {
                if let Some(path) = rfd::FileDialog::new().set_title("Upload file to R2").pick_file() {
                    let key = format!("{}{}", state.prefix, path.file_name().unwrap_or_default().to_string_lossy());
                    state.edit(Form::new(FormKind::R2Upload, "Upload R2 object", vec![Field::text("Object key", &key)]).with_note("PUT replaces an existing object with this exact key. Maximum REST upload size: 300 MB."));
                    state.file = Some(path);
                }
            }
        });
        ui.horizontal(|ui| {
            ui.label("Folder / key prefix");
            ui.add(
                egui::TextEdit::singleline(&mut state.folder_input)
                    .hint_text("assets/css/ or a narrower object prefix"),
            );
            if ui.button("Browse prefix").clicked() {
                state.browse_prefix(state.folder_input.clone());
            }
        });
        filter(ui, &mut state.filter);
        let matcher = crate::filter::FilterMatcher::new(&state.filter);
        let folders = state
            .folders
            .iter()
            .filter(|f| matcher.matches(f.strip_prefix(&state.prefix).unwrap_or(f)))
            .cloned()
            .collect::<Vec<_>>();
        let objects = state
            .objects
            .iter()
            .filter(|f| matcher.matches(&f.key))
            .cloned()
            .collect::<Vec<_>>();
        if state.truncated {
            widgets::error(ui, "Partial folder listing: more than 1,000 entries. Only the displayed page can be marked. Browse a narrower folder; this client does not fetch further object pages.");
        }
        ui.horizontal_wrapped(|ui| {
            ui.label(format!(
                "{} folders • {} files shown • {} marked",
                folders.len(),
                objects.len(),
                state.marked.len()
            ));
            if ui.button("Mark filtered files").clicked() {
                state.marked.extend(objects.iter().map(|o| o.key.clone()));
            }
            if ui.button("Clear marks").clicked() {
                state.marked.clear();
            }
        });
        let mut rows = folders
            .iter()
            .map(|f| {
                vec![
                    String::new(),
                    "Folder".into(),
                    f.clone(),
                    String::new(),
                    String::new(),
                ]
            })
            .collect::<Vec<_>>();
        rows.extend(objects.iter().map(|o| {
            vec![
                if state.marked.contains(&o.key) {
                    "Marked".into()
                } else {
                    String::new()
                },
                "File".into(),
                o.key.clone(),
                o.size.to_string(),
                o.last_modified.clone(),
            ]
        }));
        let keys = folders
            .iter()
            .map(|f| format!("folder\0{f}"))
            .chain(objects.iter().map(|o| format!("object\0{}", o.key)))
            .collect::<Vec<_>>();
        let index = resource_table(
            ui,
            "cf-objects",
            &["Bulk", "Kind", "Object key", "Bytes", "Modified"],
            &rows,
            keys.iter().map(String::as_str),
            &mut state.selected,
        );
        if let Some(i) = index {
            if let Some(folder) = folders.get(i) {
                if ui.button("Open folder").clicked() {
                    state.browse_prefix(folder.clone());
                }
            } else if let Some(object) = i.checked_sub(folders.len()).and_then(|i| objects.get(i)) {
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Toggle mark").clicked() && !state.marked.remove(&object.key) {
                        state.marked.insert(object.key.clone());
                    }
                    if ui.button("Download file…").clicked() {
                        choose_download(state, account, Some(bucket.clone()), &object.key);
                    }
                    if ui.button("Delete file…").clicked() {
                        state.review(
                            account,
                            "Delete R2 object",
                            &object.key,
                            false,
                            vec![format!("Bucket {bucket} • {} bytes", object.size)],
                            Action::Cf(CfReq::R2Delete {
                                token: account.api_token.clone(),
                                account_id: account.account_id.clone().unwrap(),
                                bucket: bucket.clone(),
                                keys: vec![object.key.clone()],
                            }),
                        );
                    }
                });
            }
        }
        if !state.marked.is_empty() {
            ui.horizontal_wrapped(|ui| {
                if ui.button("Download marked files…").clicked() {
                    if let Some(dir) = rfd::FileDialog::new()
                        .set_title("Download marked R2 files into folder")
                        .pick_folder()
                    {
                        let files = state
                            .objects
                            .iter()
                            .filter(|o| state.marked.contains(&o.key))
                            .map(|o| (o.key.clone(), dir.join(safe_basename(&o.key))))
                            .collect::<Vec<_>>();
                        state.review(
                            account,
                            "Download marked R2 objects",
                            &bucket,
                            false,
                            files
                                .iter()
                                .map(|(k, p)| {
                                    format!("{k} → {} (existing paths are refused)", p.display())
                                })
                                .collect(),
                            Action::Download {
                                account: account.clone(),
                                bucket: Some(bucket.clone()),
                                files,
                                overwrite: false,
                            },
                        );
                    }
                }
                if ui.button("Delete marked files…").clicked() {
                    let keys = state
                        .objects
                        .iter()
                        .filter(|o| state.marked.contains(&o.key))
                        .map(|o| o.key.clone())
                        .collect::<Vec<_>>();
                    state.review(
                        account,
                        "Delete marked R2 objects",
                        &bucket,
                        true,
                        keys.clone(),
                        Action::Cf(CfReq::R2Delete {
                            token: account.api_token.clone(),
                            account_id: account.account_id.clone().unwrap(),
                            bucket: bucket.clone(),
                            keys,
                        }),
                    );
                }
            });
        }
    } else {
        ui.horizontal(|ui| {
            ui.heading("R2 buckets");
            if ui.button("Create bucket").clicked() {
                state.edit(
                    Form::new(
                        FormKind::CfBucketCreate,
                        "Create R2 bucket",
                        vec![Field::text("Bucket name", "")],
                    )
                    .with_note(
                        "Lowercase letters, digits and hyphens; unique within this account.",
                    ),
                );
            }
        });
        filter(ui, &mut state.filter);
        let shown = filter_buckets(&state.buckets, &state.filter);
        let rows = shown
            .iter()
            .map(|b| {
                vec![
                    b.name.clone(),
                    b.storage_class.clone(),
                    b.location.clone().unwrap_or_default(),
                    b.creation_date.clone(),
                ]
            })
            .collect::<Vec<_>>();
        let index = resource_table(
            ui,
            "cf-buckets",
            &["Bucket", "Storage class", "Location", "Created"],
            &rows,
            shown.iter().map(|b| b.name.as_str()),
            &mut state.selected,
        );
        let selected = index.and_then(|i| shown.get(i)).map(|b| (*b).clone());
        if let Some(b) = selected {
            ui.horizontal(|ui| {
                if ui.button("Browse objects").clicked() {
                    state.bucket = Some(b.name.clone());
                    state.browse_prefix(String::new());
                    state.filter.clear();
                }
                if ui.button("Delete bucket…").clicked() {
                    state.review(
                        account,
                        "Delete R2 bucket",
                        &b.name,
                        true,
                        vec!["Bucket must be empty. Deletion is irreversible.".into()],
                        Action::Cf(CfReq::DeleteR2Bucket {
                            token: account.api_token.clone(),
                            account_id: account.account_id.clone().unwrap(),
                            name: b.name.clone(),
                        }),
                    );
                }
            });
        }
    }
    let _ = cx;
}

fn workers(
    ui: &mut egui::Ui,
    state: &mut State,
    _cx: &mut PageContext<'_>,
    account: &CloudflareAccount,
) {
    ui.horizontal_wrapped(|ui| {
        ui.heading("Workers");
        if ui.button("Deploy JavaScript file…").clicked() {
            deploy_form(state, "");
        }
        if state.worker.is_some() && ui.button("All scripts").clicked() {
            state.worker = None;
            state.settings = None;
            state.deployments.clear();
            state.selected = None;
        }
    });
    if let Some(name) = state.worker.clone() {
        ui.strong(&name);
        ui.horizontal_wrapped(|ui| {
            if ui.button("Download script…").clicked() { choose_download(state, account, None, &name); }
            if ui.button("Replace script…").clicked() { deploy_form(state, &name); }
            if ui.button("Delete script…").clicked() { state.edit(Form::new(FormKind::CfWorkerDelete { name: name.clone() }, format!("Delete Worker {name}"), vec![Field::boolean("Force", false)]).with_note("Force deletes even when resources reference this Worker. Typed confirmation follows.")); }
        });
        egui::CollapsingHeader::new("Settings, bindings, secret metadata & schedules").default_open(true).show(ui, |ui| {
            ui.label("Secret values are never returned. This view lists settings and metadata; deploy replaces script content only.");
            filter(ui, &mut state.settings_filter);
            if let Some(settings) = &state.settings {
                let worker = state.workers.iter().find(|w| w.id == name).cloned().unwrap_or_else(|| WorkerScript { id: name.clone(), ..Default::default() });
                let shown = filter_worker_settings_rows(&settings.rows(&worker), &state.settings_filter);
                let rows = shown.iter().map(|r| vec![r.section.clone(), r.name.clone(), r.value.clone()]).collect::<Vec<_>>();
                let keys = shown.iter().map(|r| format!("{}\0{}", r.section, r.name)).collect::<Vec<_>>();
                resource_table(ui, "cf-worker-settings", &["Section", "Name", "Value / metadata"], &rows, keys.iter().map(String::as_str), &mut state.settings_selected);
            }
        });
        egui::CollapsingHeader::new("Deployments & version traffic")
            .default_open(true)
            .show(ui, |ui| {
                filter(ui, &mut state.deployments_filter);
                let active = state.deployments.first().map(|d| d.id.as_str());
                for d in filter_worker_deployments(&state.deployments, &state.deployments_filter) {
                    egui::CollapsingHeader::new(format!(
                        "{} • {} • {}",
                        d.status(active == Some(d.id.as_str())),
                        d.created_on,
                        d.id
                    ))
                    .id_salt(("cf-deployment", &name, &d.id))
                    .show(ui, |ui| {
                        ui.label(format!(
                            "Source {} • strategy {} • author {}",
                            d.source, d.strategy, d.author_email
                        ));
                        ui.label(d.message());
                        ui.label(d.triggered_by());
                        for v in &d.versions {
                            ui.monospace(format!("{} — {}% traffic", v.version_id, v.percentage));
                        }
                    });
                }
                if state.loaded.contains("deployments") && state.deployments.is_empty() {
                    ui.label("No deployment history returned.");
                }
            });
    } else {
        filter(ui, &mut state.filter);
        let shown = filter_workers(&state.workers, &state.filter);
        let rows = shown
            .iter()
            .map(|w| {
                vec![
                    w.id.clone(),
                    w.handlers.join(", "),
                    w.usage_model.clone(),
                    w.modified_on.clone(),
                ]
            })
            .collect::<Vec<_>>();
        let index = resource_table(
            ui,
            "cf-workers",
            &["Script", "Handlers", "Usage model", "Modified"],
            &rows,
            shown.iter().map(|w| w.id.as_str()),
            &mut state.selected,
        );
        let selected = index.and_then(|i| shown.get(i)).map(|w| (*w).clone());
        if let Some(w) = selected {
            ui.horizontal_wrapped(|ui| {
                if ui.button("Settings & deployments").clicked() {
                    state.worker = Some(w.id.clone());
                    state.settings = None;
                    state.deployments.clear();
                    state.loaded.remove("settings");
                    state.loaded.remove("deployments");
                    state.selected = None;
                }
                if ui.button("Download script…").clicked() {
                    choose_download(state, account, None, &w.id);
                }
                if ui.button("Replace script…").clicked() {
                    deploy_form(state, &w.id);
                }
            });
        }
    }
}

fn deploy_form(state: &mut State, name: &str) {
    if let Some(path) = rfd::FileDialog::new()
        .set_title("Select Worker JavaScript file")
        .add_filter("JavaScript", &["js", "mjs"])
        .pick_file()
    {
        state.edit(Form::new(FormKind::CfWorkerDeploy, "Deploy / replace Worker script", vec![Field::text("Worker name", name), Field::choice("Mode", &["module", "service-worker"], "module")]).with_note("Uploads one file. Existing script content with this name is replaced; settings remain separate."));
        state.file = Some(path);
    }
}

fn choose_download(
    state: &mut State,
    account: &CloudflareAccount,
    bucket: Option<String>,
    key: &str,
) {
    let filename = if bucket.is_some() {
        safe_basename(key)
    } else {
        format!("{}.js", safe_basename(key))
    };
    if let Some(path) = rfd::FileDialog::new()
        .set_title("Save downloaded content")
        .set_file_name(&filename)
        .save_file()
    {
        // Always review replacement; existence checks and all filesystem IO run in the task.
        state.review(account, "Save download (replace chosen destination)", key, false, vec![format!("Destination: {}", path.display()), "An existing regular file at this exact destination will be replaced only after a successful download. Symlinks and directories are refused.".into()], Action::Download { account: account.clone(), bucket, files: vec![(key.into(), path)], overwrite: true });
    }
}

fn editor(
    ui: &mut egui::Ui,
    state: &mut State,
    cx: &mut PageContext<'_>,
    account: &CloudflareAccount,
) {
    let Some(form) = state.editor.as_ref() else {
        return;
    };
    if matches!(
        form.kind,
        FormKind::CfAccountAdd | FormKind::CfAccountEdit { .. }
    ) {
        account_editor(ui, state, cx);
        return;
    }
    let Some(form) = state.editor.as_mut() else {
        return;
    };
    ui.heading(&form.title);
    if let Some(path) = &state.file {
        ui.label(format!("Local file: {}", path.display()));
    }
    let changed = ui
        .add_enabled_ui(!cx.busy("mutation") && !cx.busy("transfer"), |ui| {
            widgets::form_fields(ui, form)
        })
        .inner;
    if changed {
        state.confirmation = None;
    }
    let mut apply = false;
    let mut discard = false;
    ui.horizontal(|ui| {
        apply = ui
            .add_enabled(
                state.confirmation.is_none() && !cx.busy("mutation") && !cx.busy("transfer"),
                egui::Button::new("Review changes…"),
            )
            .clicked();
        discard = ui
            .add_enabled(
                !cx.busy("mutation") && !cx.busy("transfer"),
                egui::Button::new("Discard"),
            )
            .clicked();
    });
    if discard {
        state.editor = None;
        state.file = None;
        state.baseline.clear();
        state.confirmation = None;
        return;
    }
    if apply {
        match prepare(state, account) {
            Ok((title, resource, typed, details, action)) => {
                state.review(account, &title, &resource, typed, details, action)
            }
            Err(e) => {
                if let Some(f) = state.editor.as_mut() {
                    f.error = Some(e.to_string());
                }
            }
        }
    }
}

fn prepare(
    state: &State,
    account: &CloudflareAccount,
) -> Result<(String, String, bool, Vec<String>, Action)> {
    let f = state.editor.as_ref().context("No editor")?;
    let get = |label: &str| f.by_label(label);
    let required = |label: &str| -> Result<String> {
        let value = get(label);
        if value.is_empty() {
            bail!("{label} is required");
        }
        Ok(value)
    };
    let id = || {
        account
            .account_id
            .clone()
            .filter(|v| !v.is_empty())
            .context("This operation requires an account ID")
    };
    if matches!(f.kind, FormKind::CfBulkSet(CfBulkAttr::Priority))
        && state
            .records
            .iter()
            .any(|r| state.marked.contains(&r.id) && !r.kind.eq_ignore_ascii_case("MX"))
    {
        bail!("Priority changes require only MX records. Unmark other record types before reviewing this batch.");
    }
    // Native paths stay PathBuf; these transfers also support reviewed replacement.
    match &f.kind {
        FormKind::CfWorkerDeploy => {
            id()?;
            let name = required("Worker name")?;
            let path = state.file.clone().context("Choose a JavaScript file")?;
            let mode = if get("Mode") == "service-worker" {
                WorkerUploadMode::ServiceWorker
            } else {
                WorkerUploadMode::Module
            };
            let details = vec![format!(
                "{} → Worker {name}; mode {}. Existing content is replaced.",
                path.display(),
                get("Mode")
            )];
            return Ok((
                "Deploy / replace Worker".into(),
                name.clone(),
                true,
                details,
                Action::Deploy {
                    account: account.clone(),
                    name,
                    path,
                    mode,
                },
            ));
        }
        FormKind::R2Upload => {
            id()?;
            let key = required("Object key")?;
            let bucket = state.bucket.clone().context("No bucket selected")?;
            let path = state.file.clone().context("Choose a local file")?;
            let details = vec![format!(
                "{} → {bucket}/{key}. Existing object bytes at this exact key are replaced.",
                path.display()
            )];
            return Ok((
                "Upload / replace R2 object".into(),
                key.clone(),
                true,
                details,
                Action::Upload {
                    account: account.clone(),
                    bucket,
                    key,
                    path,
                },
            ));
        }
        FormKind::CfWorkerDelete { name } => {
            let details = vec![format!("Force delete: {}", get("Force"))];
            let req = CfReq::WorkerDelete {
                token: account.api_token.clone(),
                account_id: id()?,
                name: name.clone(),
                force: get("Force") == "yes",
            };
            return Ok((
                "Delete Worker script".into(),
                name.clone(),
                true,
                details,
                Action::Cf(req),
            ));
        }
        FormKind::CfTunnelRouteDelete {
            tunnel_id,
            hostname,
            path,
        } => {
            let details = vec![format!(
                "Remove {hostname}{path}; delete DNS CNAME {}",
                get("Delete DNS CNAME")
            )];
            let req = CfReq::TunnelRouteDelete {
                token: account.api_token.clone(),
                account_id: id()?,
                tunnel_id: tunnel_id.clone(),
                hostname: hostname.clone(),
                path: path.clone(),
                delete_dns: get("Delete DNS CNAME") == "yes",
                zone: nonempty(get("DNS Zone")),
            };
            return Ok((
                "Delete published tunnel route".into(),
                hostname.clone(),
                true,
                details,
                Action::Cf(req),
            ));
        }
        _ => (),
    }
    let ids = marked_record_ids(state);
    let context = form::CloudflareFormContext {
        token: Some(&account.api_token),
        account_id: account.account_id.as_deref().filter(|id| !id.is_empty()),
        zone_id: state.zone.as_ref().map(|z| z.id.as_str()),
        bucket: state.bucket.as_deref(),
        prefix: &state.prefix,
        record_ids: &ids,
    };
    let req = form::cloudflare_form_request(f, &context).map_err(anyhow::Error::msg)?;
    let (title, resource, typed, details) = match &req {
        CfReq::CreateZone { name, .. } => ("Create zone", name.clone(), false, Vec::new()),
        CfReq::CreateR2Bucket { name, .. } => ("Create R2 bucket", name.clone(), false, Vec::new()),
        CfReq::TunnelCreate { name, .. } => ("Create tunnel", name.clone(), false, Vec::new()),
        CfReq::CreateRecord { body, .. } => (
            "Create DNS record",
            get("Name"),
            false,
            vec![format!(
                "{} {} → {}; TTL {}",
                get("Type"),
                get("Name"),
                get("Content"),
                body["ttl"]
            )],
        ),
        CfReq::PatchRecord { id, body, .. } => (
            "Patch DNS record",
            id.clone(),
            false,
            vec![format!("PATCH {body}; all unshown fields preserved")],
        ),
        CfReq::BulkPatch { body, .. } => (
            "Patch marked DNS records (200 per batch)",
            state
                .zone
                .as_ref()
                .context("No zone selected")?
                .name
                .clone(),
            true,
            marked_dns_details(state, Some(body)),
        ),
        CfReq::TunnelRouteAdd {
            hostname,
            path,
            service,
            dns,
            ..
        } => {
            validate_tunnel_route(hostname, service)?;
            (
                "Publish tunnel route",
                hostname.clone(),
                false,
                vec![format!("{hostname}{path} → {service}; DNS sync {dns}")],
            )
        }
        CfReq::TunnelRouteEdit {
            hostname,
            path,
            service,
            dns,
            ..
        } => {
            validate_tunnel_route(hostname, service)?;
            ("Update tunnel route", hostname.clone(), false, vec![format!("{hostname}{path} → {service}; DNS sync {dns}. Unknown origin options retained unless explicitly cleared.")])
        }
        _ => bail!("This editor does not describe a Cloudflare operation"),
    };
    Ok((title.into(), resource, typed, details, Action::Cf(req)))
}

fn review(ui: &mut egui::Ui, state: &mut State, cx: &mut PageContext<'_>) {
    let Some(review) = state.confirmation.as_ref() else {
        return;
    };
    ui.separator();
    egui::ScrollArea::vertical()
        .id_salt("cf-review-items")
        .max_height(180.0)
        .show(ui, |ui| {
            for line in &review.details {
                ui.label(line);
            }
        });
    let approved = widgets::confirm(
        ui,
        &review.title,
        &review.target,
        review.typed.as_deref(),
        &mut state.confirm_entry,
    );
    if ui.button("Back / cancel review").clicked() {
        state.confirmation = None;
        return;
    }
    if !approved || cx.busy("mutation") || cx.busy("transfer") || cx.busy("account") {
        return;
    }
    let review = state.confirmation.take().unwrap();
    state.failed.clear();
    state.errors.remove("mutation");
    state.errors.remove("transfer");
    let config_path = cx.cloudflare_config_path().to_owned();
    match review.action {
        Action::Cf(req) => cx.cloudflare("mutation", req),
        Action::RemoveAccount(name) => {
            cx.background("account", "Remove stored Cloudflare account", move || {
                super::connections::remove_cf_account(&CloudflareConfig::new(config_path), &name)?;
                Ok(Payload::Text(format!("Stored account {name} removed")))
            })
        }
        Action::DefaultAccount(name) => {
            cx.background("account", "Set default Cloudflare account", move || {
                super::connections::default_cf_account(&CloudflareConfig::new(config_path), &name)?;
                Ok(Payload::Text(format!("CLI default is now {name}")))
            })
        }
        Action::Upload {
            account,
            bucket,
            key,
            path,
        } => {
            let client = cx.cloudflare_client(&account);
            cx.background_transfer("transfer", "Upload R2 object", move || {
                let metadata =
                    std::fs::metadata(&path).with_context(|| format!("Read {}", path.display()))?;
                if !metadata.is_file() {
                    bail!("Upload source must be a regular file");
                }
                if metadata.len() > MAX_REST_OBJECT_BYTES {
                    bail!("File exceeds the 300 MB Cloudflare REST limit");
                }
                let bytes = std::fs::read(&path)?;
                if bytes.len() as u64 > MAX_REST_OBJECT_BYTES {
                    bail!("File grew beyond the 300 MB Cloudflare REST limit");
                }
                client.put_object(
                    account
                        .account_id
                        .as_deref()
                        .context("Account ID required")?,
                    &bucket,
                    &key,
                    bytes,
                    None,
                )?;
                Ok(Payload::Text(format!(
                    "Uploaded {} to {bucket}/{key}",
                    path.display()
                )))
            });
        }
        Action::Deploy {
            account,
            name,
            path,
            mode,
        } => {
            let client = cx.cloudflare_client(&account);
            cx.background_transfer("transfer", "Deploy Worker file", move || {
                if !std::fs::metadata(&path)?.is_file() {
                    bail!("Worker source must be a regular file");
                }
                let filename = path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .context("Worker filename is not valid Unicode")?;
                client.put_worker_script_content(
                    account
                        .account_id
                        .as_deref()
                        .context("Account ID required")?,
                    &name,
                    filename,
                    std::fs::read(&path)?,
                    mode,
                )?;
                Ok(Payload::Text(format!(
                    "Worker {name} deployed from {}",
                    path.display()
                )))
            });
        }
        Action::Download {
            account,
            bucket,
            files,
            overwrite,
        } => {
            let client = cx.cloudflare_client(&account);
            cx.background_transfer(
                "transfer",
                "Stream Cloudflare files to chosen paths",
                move || {
                    let id = account
                        .account_id
                        .as_deref()
                        .context("Account ID required")?;
                    let mut saved = Vec::new();
                    let mut failed = Vec::new();
                    for (key, path) in files {
                        let result = safe_download(&path, overwrite, |out| match &bucket {
                            Some(bucket) => client.download_object(id, bucket, &key, out),
                            None => client.get_worker_script_content(id, &key, out),
                        });
                        match result {
                            Ok(bytes) => {
                                saved.push(format!("{key}: {bytes} bytes → {}", path.display()))
                            }
                            Err(e) => failed.push((key, format!("{}: {e:#}", path.display()))),
                        }
                    }
                    Ok(Payload::Json(json!({ "saved": saved, "failed": failed })))
                },
            );
        }
    }
}

fn nonempty(value: String) -> Option<String> {
    (!value.trim().is_empty()).then(|| value.trim().to_owned())
}
fn parent_prefix(prefix: &str) -> String {
    prefix
        .trim_end_matches('/')
        .rsplit_once('/')
        .map(|(parent, _)| format!("{parent}/"))
        .unwrap_or_default()
}
fn safe_basename(key: &str) -> String {
    let name = key.rsplit(['/', '\\']).next().unwrap_or("");
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    let device = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && matches!(stem.as_bytes()[3], b'1'..=b'9'));
    if name.is_empty()
        || name == "."
        || name == ".."
        || device
        || name.ends_with(['.', ' '])
        || name
            .chars()
            .any(|c| c.is_control() || matches!(c, ':' | '<' | '>' | '"' | '|' | '?' | '*'))
    {
        "download".into()
    } else {
        name.to_owned()
    }
}

fn safe_download(
    path: &Path,
    overwrite: bool,
    download: impl FnOnce(&mut dyn std::io::Write) -> Result<u64>,
) -> Result<u64> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.file_type().is_file() => bail!("Destination is not a regular file; refusing to replace it"),
        Ok(_) if !overwrite => bail!("Destination already exists; choose another path or use the reviewed single-file replacement flow"),
        Ok(_) => (),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(e) => return Err(e.into()),
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    let bytes = download(temp.as_file_mut())?;
    temp.as_file_mut().sync_all()?;
    if overwrite {
        temp.persist(path).map_err(|e| e.error)?;
    } else {
        temp.persist_noclobber(path).map_err(|e| e.error)?;
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_stream_preserves_existing_file_and_paths_with_spaces() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("original file.js");
        std::fs::write(&path, b"original").unwrap();
        let result = safe_download(&path, true, |out| {
            out.write_all(b"partial")?;
            bail!("stream failed")
        });
        assert!(result.is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"original");
        safe_download(&path, true, |out| {
            out.write_all(b"complete")?;
            Ok(8)
        })
        .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"complete");
        assert!(safe_download(&path, false, |_| Ok(0)).is_err());
    }

    #[test]
    fn folder_parent_and_untrusted_download_names_stay_local() {
        assert_eq!(parent_prefix("assets/css/"), "assets/");
        assert_eq!(parent_prefix("assets/"), "");
        assert_eq!(safe_basename("../a/file name.txt"), "file name.txt");
        assert_eq!(safe_basename("x\\..\\escape.txt"), "escape.txt");
        assert_eq!(safe_basename(".."), "download");
        assert_eq!(safe_basename("x/C:secret"), "download");
        assert_eq!(safe_basename("folder/CON.js"), "download");
        assert_eq!(safe_basename("folder/lpt9"), "download");
        assert_eq!(safe_basename("folder/trailing."), "download");
    }

    #[test]
    fn dns_selection_ignores_hidden_or_stale_ids_without_selecting_everything() {
        let mut state = State {
            records: vec![Record {
                id: "one".into(),
                kind: "A".into(),
                name: "example.test".into(),
                content: "192.0.2.1".into(),
                ttl: 1,
                proxied: false,
                priority: None,
            }],
            ..Default::default()
        };
        assert!(marked_record_ids(&state).is_empty());
        state.marked.insert("missing".into());
        assert!(marked_record_ids(&state).is_empty());
        state.marked.insert("one".into());
        assert_eq!(marked_record_ids(&state), vec!["one"]);
    }

    #[test]
    fn selected_upload_and_changed_form_protect_document_close() {
        let mut state = State::default();
        state.edit(record_form(None));
        assert!(!state.is_dirty());
        state.editor.as_mut().unwrap().fields[1].value = "example.test".into();
        assert!(state.is_dirty());
        state.editor = None;
        state.file = Some(PathBuf::from("selected file.js"));
        assert!(state.is_dirty());
        state.mutation_finished();
        assert!(!state.is_dirty());
    }

    #[test]
    fn route_review_preserves_unknown_origin_options_and_explicit_dns_intent() {
        let account = CloudflareAccount {
            name: "fixture".into(),
            api_token: "not-a-real-token".into(),
            account_id: Some("fixture-account".into()),
            default: false,
        };
        let rule = TunnelIngressRule::route(
            "app.example.test",
            "https://origin:443",
            "/api",
            Some(json!({
                "originServerName": "origin.example.test", "connectTimeout": "45s",
                "futureOption": { "keep": true }, "access": { "required": true, "teamName": "team", "audTag": ["aud"], "futureAccess": 7 }
            })),
        );
        let mut state = State::default();
        state.edit(route_form("tunnel-id", Some(&rule)));
        let form = state.editor.as_mut().unwrap();
        form.fields
            .iter_mut()
            .find(|f| f.label == "HTTP Host Header")
            .unwrap()
            .value = "internal.example.test".into();
        form.fields
            .iter_mut()
            .find(|f| f.label == "Sync DNS CNAME")
            .unwrap()
            .value = "no".into();
        let (_, resource, _, _, action) = prepare(&state, &account).unwrap();
        assert_eq!(resource, "app.example.test");
        let Action::Cf(CfReq::TunnelRouteEdit {
            hostname,
            path,
            origin_request: Some(Some(origin)),
            dns,
            ..
        }) = action
        else {
            panic!("Expected a complete route edit");
        };
        assert_eq!(hostname, "app.example.test");
        assert_eq!(path, "/api");
        assert!(!dns);
        assert_eq!(origin["futureOption"]["keep"], true);
        assert_eq!(origin["access"]["futureAccess"], 7);
        assert_eq!(origin["connectTimeout"], "45s");
        assert_eq!(origin["httpHostHeader"], "internal.example.test");
    }

    #[test]
    fn browsing_prefix_does_not_relabel_old_rows_or_keep_old_bulk_targets() {
        let mut state = State::default();
        state.objects.push(R2Object {
            key: "old/file.txt".into(),
            size: 9,
            last_modified: String::new(),
            storage_class: String::new(),
        });
        state.marked.insert("old/file.txt".into());
        state.selected = Some("old/file.txt".into());
        state.loaded.insert("objects".into());
        state.truncated = true;
        state.browse_prefix("new prefix/".into());
        assert_eq!(state.prefix, "new prefix/");
        assert_eq!(state.folder_input, "new prefix/");
        assert!(state.objects.is_empty() && state.marked.is_empty());
        assert!(state.selected.is_none() && !state.loaded.contains("objects") && !state.truncated);
    }

    #[test]
    fn late_connector_token_never_crosses_tunnel_or_account_identity() {
        let mut state = State {
            token_request: Some(("account-a".into(), "tunnel-a".into())),
            tunnel: Some(CloudflareTunnel {
                id: "tunnel-b".into(),
                ..Default::default()
            }),
            ..Default::default()
        };
        state.accept_token("account-a", "secret-for-a".into());
        assert!(state.token.is_none());
        assert!(state.token_request.is_none());
        state.token_request = Some(("account-a".into(), "tunnel-b".into()));
        state.accept_token("account-b", "secret-for-other-account".into());
        assert!(state.token.is_none());
        state.token_request = Some(("account-a".into(), "tunnel-b".into()));
        state.accept_token("account-a", "secret-for-b".into());
        let token = state.token.as_ref().unwrap();
        assert_eq!(
            (&*token.account_id, &*token.tunnel_id, &*token.value),
            ("account-a", "tunnel-b", "secret-for-b")
        );
        assert!(!state.reveal_token);
    }

    #[test]
    fn identity_selection_survives_regex_filtering_hidden_rows_and_refresh_reordering() {
        let records = vec![
            Record {
                id: "one".into(),
                kind: "MX".into(),
                name: "mail.example.test".into(),
                content: "mx1.test".into(),
                ttl: 1,
                proxied: false,
                priority: Some(10),
            },
            Record {
                id: "two".into(),
                kind: "TXT".into(),
                name: "literal[.example.test".into(),
                content: "value".into(),
                ttl: 1,
                proxied: false,
                priority: None,
            },
        ];
        let selected = Some("two");
        let shown = filter_records(&records, "(?i)mail");
        assert_eq!(
            selected_row(shown.iter().map(|r| r.id.as_str()), selected),
            None
        );
        let shown = filter_records(&records, "[");
        assert_eq!(shown.len(), 1);
        assert_eq!(
            selected_row(shown.iter().map(|r| r.id.as_str()), selected),
            Some(0)
        );
        let reordered = [records[1].clone(), records[0].clone()];
        assert_eq!(
            selected_row(reordered.iter().map(|r| r.id.as_str()), selected),
            Some(0)
        );
        assert_eq!(
            selected_row(records.iter().map(|r| r.id.as_str()), selected),
            Some(1)
        );
    }

    #[test]
    fn bulk_priority_review_uses_canonical_patch_and_refuses_ineligible_types() {
        let account = CloudflareAccount {
            name: "fixture".into(),
            api_token: "fixture-token".into(),
            account_id: None,
            default: false,
        };
        let mut state = State {
            zone: Some(Zone {
                id: "zone".into(),
                name: "example.test".into(),
                status: "active".into(),
            }),
            ..Default::default()
        };
        state.records.push(Record {
            id: "mx".into(),
            kind: "MX".into(),
            name: "example.test".into(),
            content: "mail.example.test".into(),
            ttl: 3600,
            proxied: false,
            priority: Some(10),
        });
        state.marked.insert("mx".into());
        state.edit(Form::new(
            FormKind::CfBulkSet(CfBulkAttr::Priority),
            "Set MX priority",
            vec![Field::text("Priority", "20")],
        ));
        let (_, target, typed, details, action) = prepare(&state, &account).unwrap();
        assert_eq!(target, "example.test");
        assert!(typed && details[0].contains("priority=10"));
        let Action::Cf(CfReq::BulkPatch { ids, body, .. }) = action else {
            panic!("Expected marked DNS patch");
        };
        assert_eq!(ids, ["mx"]);
        assert_eq!(body, json!({"priority":20}));
        state.editor.as_mut().unwrap().fields[0].value = "65536".into();
        assert!(prepare(&state, &account).is_err());
        state.editor.as_mut().unwrap().fields[0].value = "20".into();
        state.records[0].kind = "A".into();
        assert!(prepare(&state, &account)
            .err()
            .unwrap()
            .to_string()
            .contains("only MX"));
    }
}
