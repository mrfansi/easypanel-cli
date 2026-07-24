use anyhow::{anyhow, Context, Result};
use dialoguer::{Confirm, Input, Password};
use serde_json::{json, Value};
use std::io::Read;

use crate::client::EasypanelClient;
use crate::cloudflare::CloudflareAccount;
use crate::config::{CloudflareConfig, ServerConfig};
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

// ---------- Cloudflare accounts (config-only; no network) ----------

pub fn cf_account_add(
    cfg: &CloudflareConfig,
    name: String,
    account_id: Option<String>,
    token: Option<String>,
) -> Result<()> {
    if !valid_name(&name) {
        return Err(anyhow!(
            "Cloudflare account names may only contain a-z, 0-9, - and _"
        ));
    }
    let token = match token {
        Some(t) => t,
        None => Password::new()
            .with_prompt("Cloudflare API token")
            .interact()?,
    };
    cfg.add(CloudflareAccount {
        name: name.clone(),
        api_token: token,
        account_id,
        default: false,
    })?;
    let is_default = cfg.default().map(|a| a.name == name).unwrap_or(false);
    println!(
        "Cloudflare account '{}' added.{}",
        name,
        if is_default { " (default)" } else { "" }
    );
    Ok(())
}

pub fn cf_account_list(cfg: &CloudflareConfig) -> Result<()> {
    let accounts = cfg.list();
    if accounts.is_empty() {
        println!("No Cloudflare accounts yet. Run: easypanel cf account add <name>");
        return Ok(());
    }
    let rows = accounts
        .iter()
        .map(|a| {
            vec![
                if a.default { "*".into() } else { String::new() },
                a.name.clone(),
                a.account_id.clone().unwrap_or_default(),
                mask_token(&a.api_token),
            ]
        })
        .collect();
    table(&["Default", "Name", "Account ID", "Token"], rows);
    Ok(())
}

pub fn cf_account_use(cfg: &CloudflareConfig, name: &str) -> Result<()> {
    if cfg.by_name(name).is_none() {
        return Err(anyhow!("Cloudflare account '{name}' not found."));
    }
    cfg.set_default(name)?;
    println!("Default Cloudflare account is now: {name}");
    Ok(())
}

pub fn cf_account_delete(cfg: &CloudflareConfig, name: &str) -> Result<()> {
    if cfg.by_name(name).is_none() {
        return Err(anyhow!("Cloudflare account '{name}' not found."));
    }
    cfg.remove(name)?;
    println!("Cloudflare account '{name}' removed.");
    Ok(())
}

// ---------- Cloudflare zones + records (network) ----------

use crate::cloudflare::{
    apply_patch, object_basename, record_body, resolve_zone, select_records, valid_record_type,
    CloudflareClient, RecordFilter, RecordPatch, Selector, WorkerUploadMode, Zone,
    MAX_REST_OBJECT_BYTES,
};

/// Resolve the account (named or default) and build a client, with a clear message when
/// nothing is configured yet.
fn cf_client(
    cfg: &CloudflareConfig,
    account: Option<&str>,
) -> Result<(CloudflareClient, CloudflareAccount)> {
    let acc = match account {
        Some(name) => cfg.by_name(name).ok_or_else(|| {
            anyhow!(
                "No Cloudflare account called '{name}'. Add it: easypanel cf account add {name}"
            )
        })?,
        None => cfg.default().ok_or_else(|| {
            anyhow!("No Cloudflare account configured. Add one: easypanel cf account add <name>")
        })?,
    };
    Ok((CloudflareClient::new(&acc.api_token), acc))
}

/// Look up the zone id for a name-or-id needle, listing the account's zones first.
fn cf_resolve_zone(
    client: &CloudflareClient,
    acc: &CloudflareAccount,
    needle: &str,
) -> Result<Zone> {
    let zones = client.list_zones(acc.account_id.as_deref())?;
    resolve_zone(&zones, needle)
        .cloned()
        .ok_or_else(|| anyhow!("No zone '{needle}' on this account"))
}

pub fn cf_zone_list(cfg: &CloudflareConfig, account: Option<&str>) -> Result<()> {
    let (client, acc) = cf_client(cfg, account)?;
    let zones = client.list_zones(acc.account_id.as_deref())?;
    if output::json_output() {
        output::print_json(&serde_json::to_value(
            zones
                .iter()
                .map(|z| json!({ "id": z.id, "name": z.name, "status": z.status }))
                .collect::<Vec<_>>(),
        )?);
        return Ok(());
    }
    if zones.is_empty() {
        println!("No zones on this Cloudflare account.");
        return Ok(());
    }
    let rows = zones
        .iter()
        .map(|z| vec![z.name.clone(), z.status.clone(), z.id.clone()])
        .collect();
    table(&["Name", "Status", "ID"], rows);
    Ok(())
}

pub fn cf_zone_add(cfg: &CloudflareConfig, account: Option<&str>, name: &str) -> Result<()> {
    let (client, acc) = cf_client(cfg, account)?;
    let account_id = acc.account_id.clone().ok_or_else(|| {
        anyhow!(
            "This account has no account-id, which Cloudflare needs to create a zone. \
             Re-add it: easypanel cf account add {} --account-id <ID>",
            acc.name
        )
    })?;
    let zone = client.create_zone(name, &account_id)?;
    println!("Zone '{}' created ({}).", zone.name, zone.id);
    Ok(())
}

pub fn cf_zone_delete(
    cfg: &CloudflareConfig,
    account: Option<&str>,
    zone: &str,
    yes: bool,
) -> Result<()> {
    let (client, acc) = cf_client(cfg, account)?;
    let zone = cf_resolve_zone(&client, &acc, zone)?;
    // Deleting a zone removes every DNS record in it and cannot be undone: require the
    // operator to type the zone name, not a bare y/n.
    if !yes {
        let typed: String = Input::new()
            .with_prompt(format!(
                "Delete zone '{}' and ALL its DNS records? Type the zone name to confirm",
                zone.name
            ))
            .interact_text()?;
        if typed != zone.name {
            println!("Name did not match — nothing deleted.");
            return Ok(());
        }
    }
    client.delete_zone(&zone.id)?;
    println!("Zone '{}' deleted.", zone.name);
    Ok(())
}

pub fn cf_record_list(
    cfg: &CloudflareConfig,
    account: Option<&str>,
    zone: &str,
    filter: RecordFilter,
) -> Result<()> {
    let (client, acc) = cf_client(cfg, account)?;
    let zone = cf_resolve_zone(&client, &acc, zone)?;
    let records = client.list_records(&zone.id, &filter)?;
    if output::json_output() {
        output::print_json(&serde_json::to_value(
            records
                .iter()
                .map(|r| {
                    json!({ "id": r.id, "type": r.kind, "name": r.name,
                            "content": r.content, "ttl": r.ttl, "proxied": r.proxied,
                            "priority": r.priority })
                })
                .collect::<Vec<_>>(),
        )?);
        return Ok(());
    }
    if records.is_empty() {
        println!("No DNS records match.");
        return Ok(());
    }
    let rows = records
        .iter()
        .map(|r| {
            vec![
                r.kind.clone(),
                r.name.clone(),
                r.content.clone(),
                // Priority is meaningful for MX/SRV; blank for the rest.
                r.priority.map(|p| p.to_string()).unwrap_or_default(),
                if r.ttl == 1 {
                    "auto".into()
                } else {
                    r.ttl.to_string()
                },
                if r.proxied { "yes".into() } else { "no".into() },
                r.id.clone(),
            ]
        })
        .collect();
    table(
        &[
            "Type", "Name", "Content", "Priority", "TTL", "Proxied", "ID",
        ],
        rows,
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn cf_record_add(
    cfg: &CloudflareConfig,
    account: Option<&str>,
    zone: &str,
    kind: &str,
    name: &str,
    content: &str,
    ttl: u32,
    proxied: bool,
    priority: Option<u16>,
) -> Result<()> {
    if !valid_record_type(kind) {
        return Err(anyhow!(
            "Record type '{kind}' is not supported yet (v1: A, AAAA, CNAME, TXT, NS, MX)"
        ));
    }
    let (client, acc) = cf_client(cfg, account)?;
    let zone = cf_resolve_zone(&client, &acc, zone)?;
    let body = record_body(
        &kind.to_ascii_uppercase(),
        name,
        content,
        ttl,
        proxied,
        priority,
    );
    let rec = client.create_record(&zone.id, &body)?;
    println!(
        "Record {} {} → {} created ({}).",
        rec.kind, rec.name, rec.content, rec.id
    );
    Ok(())
}

/// The per-record outcome of a bulk operation.
struct BulkReport {
    ok: Vec<String>,
    failed: Vec<(String, String)>,
}

impl BulkReport {
    fn print_and_status(&self, verb: &str) -> Result<()> {
        for id in &self.ok {
            println!("  {verb} {id}");
        }
        for (id, err) in &self.failed {
            println!("  FAILED {id}: {err}");
        }
        println!("{} ok, {} failed.", self.ok.len(), self.failed.len());
        if self.failed.is_empty() {
            Ok(())
        } else {
            Err(anyhow!("{} record(s) failed", self.failed.len()))
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn cf_record_set(
    cfg: &CloudflareConfig,
    account: Option<&str>,
    zone: &str,
    selector: Selector,
    patch: RecordPatch,
    yes: bool,
) -> Result<()> {
    if patch.is_empty() {
        return Err(anyhow!(
            "Nothing to change — pass at least one of --content/--proxied/--ttl/--priority"
        ));
    }
    let (client, acc) = cf_client(cfg, account)?;
    let zone = cf_resolve_zone(&client, &acc, zone)?;
    let records = client.list_records(&zone.id, &RecordFilter::default())?;
    let matched: Vec<_> = select_records(&records, &selector)
        .into_iter()
        .cloned()
        .collect();
    if matched.is_empty() {
        println!("0 records matched — nothing changed.");
        return Ok(());
    }
    println!("{} record(s) will change:", matched.len());
    for r in &matched {
        println!("  {} {} → {}", r.kind, r.name, r.content);
    }
    if !yes && !Confirm::new().with_prompt("Apply the change?").interact()? {
        println!("Cancelled.");
        return Ok(());
    }
    let body = apply_patch(&patch);
    let mut report = BulkReport {
        ok: vec![],
        failed: vec![],
    };
    for r in &matched {
        match client.patch_record(&zone.id, &r.id, &body) {
            Ok(_) => report.ok.push(format!("{} {}", r.kind, r.name)),
            Err(e) => report
                .failed
                .push((format!("{} {}", r.kind, r.name), e.to_string())),
        }
    }
    report.print_and_status("changed")
}

pub fn cf_record_delete(
    cfg: &CloudflareConfig,
    account: Option<&str>,
    zone: &str,
    ids: &[String],
) -> Result<()> {
    if ids.is_empty() {
        return Err(anyhow!("Name at least one record id to delete"));
    }
    let (client, acc) = cf_client(cfg, account)?;
    let zone = cf_resolve_zone(&client, &acc, zone)?;
    let mut report = BulkReport {
        ok: vec![],
        failed: vec![],
    };
    for id in ids {
        match client.delete_record(&zone.id, id) {
            Ok(()) => report.ok.push(id.clone()),
            Err(e) => report.failed.push((id.clone(), e.to_string())),
        }
    }
    report.print_and_status("deleted")
}

// ---------- R2 buckets (account-scoped) ----------

/// The active account's account-id, which every R2 call needs (R2 is
/// account-scoped, unlike DNS zones). A friendly error names how to add it.
fn cf_account_id(acc: &CloudflareAccount) -> Result<String> {
    acc.account_id.clone().ok_or_else(|| {
        anyhow!(
            "This account has no account-id, which R2 is scoped by. \
             Re-add it: easypanel cf account add {} --account-id <ID>",
            acc.name
        )
    })
}

pub fn cf_r2_bucket_list(cfg: &CloudflareConfig, account: Option<&str>) -> Result<()> {
    let (client, acc) = cf_client(cfg, account)?;
    let account_id = cf_account_id(&acc)?;
    let buckets = client.list_r2_buckets(&account_id)?;
    if output::json_output() {
        output::print_json(&serde_json::to_value(
            buckets
                .iter()
                .map(|b| {
                    json!({ "name": b.name, "creation_date": b.creation_date,
                            "location": b.location, "storage_class": b.storage_class,
                            "jurisdiction": b.jurisdiction })
                })
                .collect::<Vec<_>>(),
        )?);
        return Ok(());
    }
    if buckets.is_empty() {
        println!("No R2 buckets on this Cloudflare account.");
        return Ok(());
    }
    let rows = buckets
        .iter()
        .map(|b| {
            vec![
                b.name.clone(),
                b.creation_date.clone(),
                b.location.clone().unwrap_or_default(),
                b.storage_class.clone(),
            ]
        })
        .collect();
    table(&["Name", "Created", "Location", "Class"], rows);
    Ok(())
}

pub fn cf_r2_bucket_create(
    cfg: &CloudflareConfig,
    account: Option<&str>,
    name: &str,
) -> Result<()> {
    let (client, acc) = cf_client(cfg, account)?;
    let account_id = cf_account_id(&acc)?;
    let bucket = client.create_r2_bucket(&account_id, name)?;
    println!("Bucket '{}' created.", bucket.name);
    Ok(())
}

pub fn cf_r2_bucket_delete(
    cfg: &CloudflareConfig,
    account: Option<&str>,
    name: &str,
    yes: bool,
) -> Result<()> {
    let (client, acc) = cf_client(cfg, account)?;
    let account_id = cf_account_id(&acc)?;
    // Deleting a bucket is destructive (and Cloudflare requires it be empty): require
    // the operator to type the bucket name, not a bare y/n — the same safeguard as zones.
    if !yes {
        let typed: String = Input::new()
            .with_prompt(format!(
                "Delete R2 bucket '{name}'? It must be empty. Type the bucket name to confirm"
            ))
            .interact_text()?;
        if typed != name {
            println!("Name did not match — nothing deleted.");
            return Ok(());
        }
    }
    client.delete_r2_bucket(&account_id, name)?;
    println!("Bucket '{name}' deleted.");
    Ok(())
}

// ---------- R2 objects (REST API, account-scoped) ----------

/// Resolve the account and print ONE folder level of a bucket (delimiter=/): the
/// subfolders at `--prefix` (default root) with a trailing `/`, then the files here
/// (newest-first). Mirrors the TUI folder view. Uses the SAME Bearer token as buckets (no
/// separate credentials); needs only the account-id. A token missing the Workers R2
/// Storage permission surfaces the same `r2_hint`.
pub fn cf_r2_object_list(
    cfg: &CloudflareConfig,
    account: Option<&str>,
    bucket: &str,
    prefix: Option<&str>,
) -> Result<()> {
    let (client, acc) = cf_client(cfg, account)?;
    let account_id = cf_account_id(&acc)?;
    let level = client.list_r2_level(&account_id, bucket, prefix.unwrap_or(""))?;
    if output::json_output() {
        output::print_json(&json!({
            "folders": level.folders,
            "files": level.files.iter().map(|o| {
                json!({ "key": o.key, "size": o.size,
                        "last_modified": o.last_modified,
                        "storage_class": o.storage_class })
            }).collect::<Vec<_>>(),
        }));
        return Ok(());
    }
    if level.folders.is_empty() && level.files.is_empty() {
        println!("No objects in bucket '{bucket}'.");
        return Ok(());
    }
    // Folders first (full prefix, trailing `/`, no size/date), then files.
    let mut rows: Vec<Vec<String>> = level
        .folders
        .iter()
        .map(|folder| vec![folder.clone(), String::new(), String::new()])
        .collect();
    rows.extend(level.files.iter().map(|o| {
        vec![
            o.key.clone(),
            output::format_bytes(o.size as f64),
            o.last_modified.clone(),
        ]
    }));
    table(&["Key", "Size", "Modified"], rows);
    if level.truncated {
        println!(
            "\nShowing the first {} row(s) at this level — more exist. Narrow with --prefix <path/>.",
            level.folders.len() + level.files.len()
        );
    }
    Ok(())
}

/// Upload a local file to an object key. Rejects files over the 300 MB REST limit up front
/// (those need the S3 API). Reads the file, PUTs the bytes, prints a one-line receipt.
pub fn cf_r2_object_put(
    cfg: &CloudflareConfig,
    account: Option<&str>,
    bucket: &str,
    key: &str,
    file: &str,
) -> Result<()> {
    let (client, acc) = cf_client(cfg, account)?;
    let account_id = cf_account_id(&acc)?;
    let meta = std::fs::metadata(file).with_context(|| format!("cannot read '{file}'"))?;
    if meta.len() > MAX_REST_OBJECT_BYTES {
        anyhow::bail!(
            "'{file}' is {} — over the 300 MB REST upload limit. Larger objects need the \
             S3 API, which this tool uses only for DB dumps.",
            format_bytes(meta.len() as f64)
        );
    }
    let bytes = std::fs::read(file).with_context(|| format!("cannot read '{file}'"))?;
    let size = bytes.len() as f64;
    client.put_object(&account_id, bucket, key, bytes, None)?;
    println!("Uploaded {file} → {bucket}/{key} ({})", format_bytes(size));
    Ok(())
}

/// Download an object to a local file (streamed, never buffered). Destination is `--out`
/// if given, else the key's basename in the CWD. Refuses to overwrite an existing file.
pub fn cf_r2_object_get(
    cfg: &CloudflareConfig,
    account: Option<&str>,
    bucket: &str,
    key: &str,
    out: Option<&str>,
) -> Result<()> {
    let (client, acc) = cf_client(cfg, account)?;
    let account_id = cf_account_id(&acc)?;
    let dest = out.unwrap_or_else(|| object_basename(key));
    if std::path::Path::new(dest).exists() {
        anyhow::bail!("refusing to overwrite {dest} (pass --out to choose)");
    }
    let mut file =
        std::fs::File::create(dest).with_context(|| format!("cannot create '{dest}'"))?;
    let n = client.download_object(&account_id, bucket, key, &mut file)?;
    println!(
        "Downloaded {bucket}/{key} → {dest} ({})",
        format_bytes(n as f64)
    );
    Ok(())
}

/// Delete one or more object keys (bulk). Reports each success, collects failures, and
/// returns an error summarizing them so the process exits nonzero when any key failed.
pub fn cf_r2_object_rm(
    cfg: &CloudflareConfig,
    account: Option<&str>,
    bucket: &str,
    keys: &[String],
) -> Result<()> {
    let (client, acc) = cf_client(cfg, account)?;
    let account_id = cf_account_id(&acc)?;
    let mut failures = Vec::new();
    for key in keys {
        match client.delete_object(&account_id, bucket, key) {
            Ok(()) => println!("Deleted {bucket}/{key}"),
            Err(e) => failures.push(format!("{key}: {e}")),
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        anyhow::bail!(
            "failed to delete {} object(s):\n{}",
            failures.len(),
            failures.join("\n")
        )
    }
}

// ---------- Workers scripts (account-scoped) ----------

fn empty_dash(value: &str) -> String {
    if value.trim().is_empty() {
        "-".into()
    } else {
        value.to_string()
    }
}

pub fn cf_workers_list(cfg: &CloudflareConfig, account: Option<&str>) -> Result<()> {
    let (client, acc) = cf_client(cfg, account)?;
    let account_id = cf_account_id(&acc)?;
    let scripts = client.list_worker_scripts(&account_id)?;
    if output::json_output() {
        output::print_json(&serde_json::to_value(
            scripts
                .iter()
                .map(|w| {
                    json!({
                        "id": w.id,
                        "handlers": w.handlers,
                        "usage_model": w.usage_model,
                        "created_on": w.created_on,
                        "modified_on": w.modified_on,
                        "etag": w.etag,
                    })
                })
                .collect::<Vec<_>>(),
        )?);
        return Ok(());
    }
    if scripts.is_empty() {
        println!("No Workers scripts on this Cloudflare account.");
        return Ok(());
    }
    let rows = scripts
        .iter()
        .map(|w| {
            vec![
                w.id.clone(),
                w.handlers.join(","),
                w.usage_model.clone(),
                w.modified_on
                    .split('T')
                    .next()
                    .unwrap_or(&w.modified_on)
                    .to_string(),
                w.etag.clone(),
            ]
        })
        .collect();
    table(&["Name", "Handlers", "Usage", "Modified", "ETag"], rows);
    Ok(())
}

pub fn cf_workers_get(
    cfg: &CloudflareConfig,
    account: Option<&str>,
    name: &str,
    out: Option<&str>,
) -> Result<()> {
    let (client, acc) = cf_client(cfg, account)?;
    let account_id = cf_account_id(&acc)?;
    let dest = out.unwrap_or(name);
    if std::path::Path::new(dest).exists() {
        anyhow::bail!("refusing to overwrite {dest} (pass --out to choose)");
    }
    let mut bytes = Vec::new();
    let n = client.get_worker_script_content(&account_id, name, &mut bytes)?;
    std::fs::write(dest, &bytes).with_context(|| format!("cannot write '{dest}'"))?;
    println!(
        "Downloaded Worker {name} → {dest} ({})",
        format_bytes(n as f64)
    );
    Ok(())
}

pub fn cf_workers_deployments(
    cfg: &CloudflareConfig,
    account: Option<&str>,
    name: &str,
) -> Result<()> {
    let (client, acc) = cf_client(cfg, account)?;
    let account_id = cf_account_id(&acc)?;
    let deployments = client.list_worker_deployments(&account_id, name)?;
    if output::json_output() {
        output::print_json(&serde_json::to_value(
            deployments
                .iter()
                .map(|d| {
                    json!({
                        "id": d.id,
                        "created_on": d.created_on,
                        "source": d.source,
                        "strategy": d.strategy,
                        "versions": d.versions.iter().map(|v| json!({
                            "version_id": v.version_id,
                            "percentage": v.percentage,
                        })).collect::<Vec<_>>(),
                        "message": d.message(),
                        "triggered_by": d.triggered_by(),
                        "author_email": d.author_email,
                    })
                })
                .collect::<Vec<_>>(),
        )?);
        return Ok(());
    }
    if deployments.is_empty() {
        println!("No deployments found for Worker '{name}'.");
        return Ok(());
    }
    let rows = deployments
        .iter()
        .map(|d| {
            vec![
                d.short_id(),
                d.versions_label(),
                d.created_on
                    .split('T')
                    .next()
                    .unwrap_or(&d.created_on)
                    .to_string(),
                empty_dash(&d.source),
                empty_dash(d.triggered_by()),
                empty_dash(&d.author_email),
                empty_dash(d.message()),
            ]
        })
        .collect();
    table(
        &[
            "Deployment",
            "Versions / traffic",
            "Created",
            "Source",
            "Trigger",
            "Author",
            "Message",
        ],
        rows,
    );
    Ok(())
}

pub fn cf_workers_settings(
    cfg: &CloudflareConfig,
    account: Option<&str>,
    name: &str,
) -> Result<()> {
    let (client, acc) = cf_client(cfg, account)?;
    let account_id = cf_account_id(&acc)?;
    let settings = client.get_worker_settings_bundle(&account_id, name)?;
    let worker = client
        .list_worker_scripts(&account_id)?
        .into_iter()
        .find(|w| w.id == name)
        .unwrap_or_else(|| crate::cloudflare::WorkerScript {
            id: name.into(),
            ..Default::default()
        });
    let rows = settings.rows(&worker);
    if output::json_output() {
        output::print_json(&json!({
            "worker": name,
            "rows": rows.iter().map(|r| json!({
                "section": r.section,
                "name": r.name,
                "value": r.value,
            })).collect::<Vec<_>>(),
            "version": {
                "compatibility_date": settings.version.compatibility_date,
                "compatibility_flags": settings.version.compatibility_flags,
                "usage_model": settings.version.usage_model,
                "bindings": settings.version.bindings,
                "placement": settings.version.placement,
                "cache_options": settings.version.cache_options,
                "limits": settings.version.limits,
            },
            "script": {
                "logpush": settings.script.logpush,
                "observability": settings.script.observability,
                "tags": settings.script.tags,
                "tail_consumers": settings.script.tail_consumers,
            },
            "secrets": settings.secrets,
            "schedules": settings.schedules,
        }));
        return Ok(());
    }
    table(
        &["Section", "Name", "Value"],
        rows.into_iter()
            .map(|r| vec![r.section, r.name, r.value])
            .collect(),
    );
    Ok(())
}

pub fn cf_workers_deploy(
    cfg: &CloudflareConfig,
    account: Option<&str>,
    name: &str,
    file: &str,
    mode: WorkerUploadMode,
) -> Result<()> {
    let (client, acc) = cf_client(cfg, account)?;
    let account_id = cf_account_id(&acc)?;
    let filename = std::path::Path::new(file)
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow!("'{file}' has no filename"))?;
    let bytes = std::fs::read(file).with_context(|| format!("cannot read '{file}'"))?;
    let size = bytes.len() as f64;
    let script = client.put_worker_script_content(&account_id, name, filename, bytes, mode)?;
    println!(
        "Deployed Worker '{}' from {file} ({})",
        if script.id.is_empty() {
            name
        } else {
            &script.id
        },
        format_bytes(size)
    );
    Ok(())
}

pub fn cf_workers_delete(
    cfg: &CloudflareConfig,
    account: Option<&str>,
    name: &str,
    yes: bool,
    force: bool,
) -> Result<()> {
    let (client, acc) = cf_client(cfg, account)?;
    let account_id = cf_account_id(&acc)?;
    if !yes {
        let typed: String = Input::new()
            .with_prompt(format!(
                "Delete Worker '{name}'? Type the Worker name to confirm"
            ))
            .interact_text()?;
        if typed != name {
            println!("Name did not match — nothing deleted.");
            return Ok(());
        }
    }
    client.delete_worker_script(&account_id, name, force)?;
    println!("Worker '{name}' deleted.");
    Ok(())
}

// ---------- Cloudflare Tunnels (account-scoped) ----------

pub fn cf_tunnels_list(cfg: &CloudflareConfig, account: Option<&str>) -> Result<()> {
    let (client, acc) = cf_client(cfg, account)?;
    let account_id = cf_account_id(&acc)?;
    let tunnels = client.list_tunnels(&account_id)?;
    if output::json_output() {
        output::print_json(&serde_json::to_value(&tunnels)?);
        return Ok(());
    }
    if tunnels.is_empty() {
        println!("No Cloudflare Tunnels on this account.");
        return Ok(());
    }
    let rows = tunnels
        .iter()
        .map(|t| {
            vec![
                t.name.clone(),
                t.status_label(),
                empty_dash(&t.config_src),
                t.created_at
                    .split('T')
                    .next()
                    .unwrap_or(&t.created_at)
                    .to_string(),
                t.target(),
                t.id.clone(),
            ]
        })
        .collect();
    table(
        &["Name", "Status", "Config", "Created", "Target", "ID"],
        rows,
    );
    Ok(())
}

pub fn cf_tunnels_config(
    cfg: &CloudflareConfig,
    account: Option<&str>,
    tunnel: &str,
) -> Result<()> {
    let (client, acc) = cf_client(cfg, account)?;
    let account_id = cf_account_id(&acc)?;
    let tunnels = client.list_tunnels(&account_id)?;
    let Some(found) = tunnels.iter().find(|t| t.id == tunnel || t.name == tunnel) else {
        anyhow::bail!("No tunnel named or identified by '{tunnel}'");
    };
    let config = client.get_tunnel_config(&account_id, &found.id)?;
    if output::json_output() {
        output::print_json(&json!({
            "tunnel": found,
            "configuration": config,
            "rows": config.rows(),
        }));
        return Ok(());
    }
    let rows = config
        .rows()
        .into_iter()
        .map(|r| vec![r.hostname, r.service, r.origin])
        .collect::<Vec<_>>();
    if rows.is_empty() {
        println!(
            "No published application routes configured for '{}'.",
            found.name
        );
        return Ok(());
    }
    table(&["Hostname", "Service", "Origin request"], rows);
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

/// Write a project's config to a file (or stdout), secrets redacted.
///
/// The `--json` global flag prints the raw inspectProject instead — this command
/// is the CURATED, git-committable form; that flag is the escape hatch for the
/// raw API shape.
pub fn project_export(client: &EasypanelClient, name: &str, file: Option<String>) -> Result<()> {
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
    let export = crate::services::export_project(name, &services);
    let text = serde_json::to_string_pretty(&export)?;

    match file.as_deref() {
        Some("-") => println!("{text}"),
        other => {
            let path = other
                .map(String::from)
                .unwrap_or_else(|| format!("{name}.easypanel.json"));
            std::fs::write(&path, format!("{text}\n"))?;
            println!(
                "Wrote {} service(s) to {path} — config only, secrets redacted.",
                services.len()
            );
        }
    }
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

pub fn stats_rows(s: &Value) -> Vec<Vec<String>> {
    let pair = |pct: f64, used: &str, total: &str| {
        format!(
            "{pct:.1} % ({} / {})",
            format_bytes(num(s, used)),
            format_bytes(num(s, total))
        )
    };
    vec![
        vec!["CPU".into(), format!("{:.1}%", series_last(s, "cpu"))],
        vec!["Cores".into(), field(s, "/cpuCores")],
        vec!["Load avg".into(), crate::monitor::load_avg(s)],
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
    table(
        &crate::monitor::MONITOR_HEADERS,
        crate::monitor::monitor_rows(&arr),
    );
    Ok(())
}

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
    table(
        &crate::monitor::STORAGE_HEADERS,
        crate::monitor::storage_rows(&arr),
    );
    Ok(())
}

// ---------- Domains (host-wide) ----------
//
// What a domain IS — how its source and destination read — now lives in
// `crate::domains`, the bounded context that owns them. This module keeps only
// the CLI command that prints them.

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
    table(
        &crate::domains::DOMAIN_HEADERS,
        arr.iter().map(crate::domains::domain_row).collect(),
    );
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

    // Pre-flight: EasyPanel restores INTO an existing database, so on a host
    // where `database` was never created the restore dies with a cryptic
    // `[400] … docker exec … exit code 1`. Ask what the service actually holds
    // and refuse plainly, before prompting to overwrite something that can't be.
    // A failure of OUR check (or an empty/unreadable list) is ambiguous — let it
    // through so a restore that might work still can.
    if let Ok(listed) = client.call(
        "databaseBackups",
        "getServiceDatabases",
        json!({ "projectName": project, "serviceName": service }),
    ) {
        if crate::backup::service_lists_database(&listed, database) == Some(false) {
            anyhow::bail!(crate::backup::missing_database_message(service, database));
        }
    }

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

// ---------- Non-locking database dump / restore to object storage ----------
//
// The tool's OWN backup: dump a mysql/mariadb service's databases inside the
// container (non-locking `--single-transaction`), gzip, and push straight to the
// existing remote storage (R2) with a presigned URL. One self-contained file for
// several databases, restorable onto a host where they never existed — the three
// things EasyPanel's own per-database, locking, restore-into-an-existing-db backup
// can't do. See `dump.rs`/`s3.rs`/`container::run_until_done`.

/// A remote (S3/R2) storage provider with the credentials a presigned upload needs.
struct RemoteStore {
    name: String,
    access_key: String,
    secret_key: String,
    bucket: String,
    endpoint: String,
    region: String,
}

/// The engine of a database service, read from the project's service list so we
/// don't have to guess the `services/{type}` path. Only mysql/mariadb dump for now;
/// anything else is a clear error, not a silently wrong command.
fn resolve_db_engine(client: &EasypanelClient, project: &str, service: &str) -> Result<String> {
    let data = client.call(
        "projects",
        "inspectProject",
        json!({ "projectName": project }),
    )?;
    let stype = data
        .get("services")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|s| field(s, "/name") == service)
        .map(|s| field(s, "/type"))
        .ok_or_else(|| anyhow!("{project}/{service} not found"))?;
    match stype.as_str() {
        "mysql" | "mariadb" => Ok(stype),
        other => {
            anyhow::bail!("db dump/restore supports mysql and mariadb; {service} is '{other}'.")
        }
    }
}

/// Pick the remote storage provider: the one named by `want` (id or name), or the
/// only remote one when there is exactly one. Local disk is never a target — a
/// dump you can't reach from another host defeats the whole point.
fn pick_remote_provider(client: &EasypanelClient, want: Option<&str>) -> Result<RemoteStore> {
    let v = client.call("storageProviders/common", "list", Value::Null)?;
    let all = v.as_array().cloned().unwrap_or_default();
    let remote: Vec<Value> = all
        .into_iter()
        .filter(|p| crate::backup::is_remote(&field(p, "/type")))
        .collect();
    let chosen: &Value = match want {
        Some(w) => remote
            .iter()
            .find(|p| field(p, "/id") == w || field(p, "/name").eq_ignore_ascii_case(w))
            .ok_or_else(|| {
                anyhow!("No remote storage provider matches '{w}'. See: easypanel backup providers")
            })?,
        None => match remote.as_slice() {
            [one] => one,
            [] => anyhow::bail!(
                "No remote storage provider is configured (local disk can't be restored elsewhere)."
            ),
            many => anyhow::bail!(
                "There are {} remote providers; choose one with --provider (see: easypanel backup providers).",
                many.len()
            ),
        },
    };
    Ok(RemoteStore {
        name: field(chosen, "/name"),
        access_key: field(chosen, "/accessKeyId"),
        secret_key: field(chosen, "/secretAccessKey"),
        bucket: field(chosen, "/bucket"),
        endpoint: field(chosen, "/endpoint"),
        region: field(chosen, "/region"),
    })
}

/// The databases a dump should cover: `--all` (every non-system schema the service
/// actually holds) or the explicit `--databases` list. Every name is gated to a
/// safe shell token — these are spliced into an in-container command.
fn resolve_dump_databases(
    client: &EasypanelClient,
    project: &str,
    service: &str,
    databases: &[String],
    all: bool,
) -> Result<Vec<String>> {
    let list = if all {
        let v = client.call(
            "databaseBackups",
            "getServiceDatabases",
            json!({ "projectName": project, "serviceName": service }),
        )?;
        let names: Vec<String> = v
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|d| d.as_str().map(String::from))
            .filter(|d| !crate::dump::is_system_db(d))
            .collect();
        if names.is_empty() {
            anyhow::bail!("{service} has no non-system databases to dump.");
        }
        names
    } else if databases.is_empty() {
        anyhow::bail!("Say which databases: --databases a,b  (or --all).");
    } else {
        databases.to_vec()
    };
    for d in &list {
        if !crate::dump::valid_db_name(d) {
            anyhow::bail!(
                "Refusing database name {d:?}: only letters, digits, '_' and '-' allowed."
            );
        }
    }
    Ok(list)
}

/// The root password for a database service, or a clear error. Kept out of any
/// message we print.
fn service_root_password(
    client: &EasypanelClient,
    stype: &str,
    project: &str,
    service: &str,
) -> Result<String> {
    let inspect = client.call(
        &format!("services/{stype}"),
        "inspectService",
        json!({ "projectName": project, "serviceName": service }),
    )?;
    let pw = field(&inspect, "/rootPassword");
    if !crate::backup::is_named(&pw) {
        anyhow::bail!("No root password on record for {service}; cannot reach the database.");
    }
    Ok(pw)
}

/// The result of a completed non-locking dump to object storage.
pub(crate) struct R2Dump {
    pub bucket: String,
    pub key: String,
    pub provider: String,
    pub databases: Vec<String>,
}

/// Run a non-locking dump of a service's databases to the remote storage and
/// return where it landed. Shared by the CLI (`db dump`) and the TUI worker, so
/// both surfaces get the identical behaviour instead of the TUI keeping the old
/// locking path. No I/O of its own — the caller reports progress/results.
pub(crate) fn dump_to_r2(
    client: &EasypanelClient,
    project: &str,
    service: &str,
    databases: &[String],
    all: bool,
    provider: Option<&str>,
) -> Result<R2Dump> {
    let stype = resolve_db_engine(client, project, service)?;
    let root_password = service_root_password(client, &stype, project, service)?;
    let dbs = resolve_dump_databases(client, project, service, databases, all)?;
    let store = pick_remote_provider(client, provider)?;

    let now = chrono::Utc::now();
    let ts = now.format("%Y%m%d-%H%M%S").to_string();
    let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
    let key = format!("{project}/{service}-{ts}.sql.gz");
    let tmp = format!("/tmp/ezp-dump-{ts}.sql.gz");
    let url = crate::s3::presign(
        "PUT",
        &store.endpoint,
        &store.bucket,
        &key,
        &store.access_key,
        &store.secret_key,
        &store.region,
        &amz_date,
        3600,
    );
    let cmd = crate::dump::dump_command(&stype, &root_password, &dbs, &tmp, &url)
        .ok_or_else(|| anyhow!("db dump supports mysql/mariadb only"))?;

    let run = crate::container::run_until_done(
        client,
        project,
        service,
        &cmd,
        std::time::Duration::from_secs(3600),
    )?;
    let redact = |s: &str| {
        s.replace(&url, "<presigned-url>")
            .replace(&root_password, "<redacted>")
            .trim()
            .to_string()
    };
    match run.exit_code {
        Some(0) => Ok(R2Dump {
            bucket: store.bucket,
            key,
            provider: store.name,
            databases: dbs,
        }),
        Some(n) => anyhow::bail!("Dump failed (exit {n}). {}", redact(&run.output)),
        None => anyhow::bail!(
            "Dump still running after 60 min — it continues in the container. \
             Check the service before re-running."
        ),
    }
}

pub fn db_dump(
    client: &EasypanelClient,
    project: &str,
    service: &str,
    databases: &[String],
    all: bool,
    provider: Option<&str>,
) -> Result<()> {
    println!("Dumping {project}/{service} to object storage (non-locking)…");
    let d = dump_to_r2(client, project, service, databases, all, provider)?;
    println!(
        "Done → {}/{} ({}) — {} database(s): {}.",
        d.bucket,
        d.key,
        d.provider,
        d.databases.len(),
        d.databases.join(", ")
    );
    println!(
        "Restore it with:  easypanel db restore {project} {service} --path {}",
        d.key
    );
    Ok(())
}

/// Restore a dump this tool wrote (by its object key) into a service. Shared by
/// the CLI `db restore` and the TUI worker; no confirmation or printing of its own.
pub(crate) fn restore_from_r2(
    client: &EasypanelClient,
    project: &str,
    service: &str,
    path: &str,
    provider: Option<&str>,
) -> Result<()> {
    let stype = resolve_db_engine(client, project, service)?;
    let root_password = service_root_password(client, &stype, project, service)?;
    let store = pick_remote_provider(client, provider)?;

    let now = chrono::Utc::now();
    let ts = now.format("%Y%m%d-%H%M%S").to_string();
    let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
    let tmp = format!("/tmp/ezp-restore-{ts}.sql.gz");
    let url = crate::s3::presign(
        "GET",
        &store.endpoint,
        &store.bucket,
        path,
        &store.access_key,
        &store.secret_key,
        &store.region,
        &amz_date,
        3600,
    );
    let cmd = crate::dump::restore_command(&stype, &root_password, &tmp, &url)
        .ok_or_else(|| anyhow!("db restore supports mysql/mariadb only"))?;

    let run = crate::container::run_until_done(
        client,
        project,
        service,
        &cmd,
        std::time::Duration::from_secs(3600),
    )?;
    let redact = |s: &str| {
        s.replace(&url, "<presigned-url>")
            .replace(&root_password, "<redacted>")
            .trim()
            .to_string()
    };
    match run.exit_code {
        Some(0) => Ok(()),
        Some(n) => anyhow::bail!("Restore failed (exit {n}). {}", redact(&run.output)),
        None => anyhow::bail!(
            "Restore still running after 60 min — it continues in the container. \
             Check the service before re-running; a second restore may clash."
        ),
    }
}

/// The object keys of the dumps this tool has written for a service, newest last.
/// EasyPanel has no endpoint that lists them (they are the tool's own files, not
/// its backup actions), so this signs an S3 `ListObjectsV2` for the
/// `{project}/{service}-` prefix directly. Shared by the CLI `db list` and the TUI.
pub(crate) fn list_r2_dumps(
    client: &EasypanelClient,
    project: &str,
    service: &str,
    provider: Option<&str>,
) -> Result<Vec<String>> {
    let store = pick_remote_provider(client, provider)?;
    let prefix = format!("{project}/{service}-");
    let amz_date = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let (url, auth) = crate::s3::sign_list(
        &store.endpoint,
        &store.bucket,
        &prefix,
        &store.access_key,
        &store.secret_key,
        &store.region,
        &amz_date,
    );
    let body = reqwest::blocking::Client::new()
        .get(&url)
        .header("Authorization", auth)
        .header("x-amz-date", amz_date)
        .header("x-amz-content-sha256", "UNSIGNED-PAYLOAD")
        .send()?
        .error_for_status()?
        .text()?;
    // The XML lists <Key>…</Key> per object; no XML dep needed for one tag.
    let mut keys: Vec<String> = body
        .split("<Key>")
        .skip(1)
        .filter_map(|s| s.split("</Key>").next())
        .filter(|k| k.ends_with(".sql.gz"))
        .map(String::from)
        .collect();
    keys.sort();
    Ok(keys)
}

pub fn db_list(
    client: &EasypanelClient,
    project: &str,
    service: &str,
    provider: Option<&str>,
) -> Result<()> {
    let keys = list_r2_dumps(client, project, service, provider)?;
    if keys.is_empty() {
        println!("No dumps found for {project}/{service}.");
        return Ok(());
    }
    for k in keys {
        println!("{k}");
    }
    Ok(())
}

pub fn db_restore(
    client: &EasypanelClient,
    project: &str,
    service: &str,
    path: &str,
    provider: Option<&str>,
    yes: bool,
) -> Result<()> {
    if !confirm(
        &format!(
            "Restore dump '{path}' into {project}/{service}? It recreates and OVERWRITES \
             the databases the dump contains."
        ),
        yes,
    )? {
        return Ok(());
    }
    println!("Restoring {path} into {project}/{service}…");
    restore_from_r2(client, project, service, path, provider)?;
    println!("Restored {path} into {project}/{service}.");
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
        assert_eq!(rows[0], vec!["CPU", "5.5%"]); // last point
        assert_eq!(rows[1], vec!["Cores", "16"]);
        assert_eq!(rows[2], vec!["Load avg", "0.10, 0.20, 0.30"]);
        assert_eq!(rows[3], vec!["Memory", "25.0 % (1.0 GB / 2.0 GB)"]);
        assert_eq!(rows[4], vec!["Disk", "16.2 % (1.0 GB / 10.0 GB)"]);
        assert_eq!(rows[5], vec!["Network In", "1.0 KB/s"]);
    }
}
