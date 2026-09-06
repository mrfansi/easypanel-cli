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

/// The client for a server named EXPLICITLY, rather than the active one.
///
/// `resolve_client` answers "the host this command is aimed at"; a copy needs a
/// SECOND host as well, and only ever by name — the token is read here and goes
/// no further, the same split the TUI keeps by resolving names in its event loop
/// so the UI state never holds one.
pub fn resolve_client_named(cfg: &ServerConfig, name: &str) -> Result<EasypanelClient> {
    let s = cfg
        .get(name)
        .ok_or_else(|| anyhow!("Server '{name}' not found. See: easypanel server list"))?;
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
    apply_patch, object_basename, parse_tunnel_origin_request, record_body, resolve_zone,
    select_records, valid_record_type, CloudflareClient, CloudflareTunnel, RecordFilter,
    RecordPatch, Selector, TunnelIngressRule, TunnelRouteChange, WorkerUploadMode, Zone,
    DNS_BATCH_LIMIT, MAX_REST_OBJECT_BYTES,
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
    for chunk in matched.chunks(DNS_BATCH_LIMIT) {
        let ids: Vec<String> = chunk.iter().map(|r| r.id.clone()).collect();
        match client.batch_patch_records(&zone.id, &ids, &body) {
            Ok(()) => report
                .ok
                .extend(chunk.iter().map(|r| format!("{} {}", r.kind, r.name))),
            Err(e) => {
                let msg = e.to_string();
                report.failed.extend(
                    chunk
                        .iter()
                        .map(|r| (format!("{} {}", r.kind, r.name), msg.clone())),
                );
            }
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
    for chunk in ids.chunks(DNS_BATCH_LIMIT) {
        match client.batch_delete_records(&zone.id, chunk) {
            Ok(()) => report.ok.extend(chunk.iter().cloned()),
            Err(e) => {
                let msg = e.to_string();
                report
                    .failed
                    .extend(chunk.iter().cloned().map(|id| (id, msg.clone())));
            }
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
                .enumerate()
                .map(|(i, d)| {
                    json!({
                        "id": d.id,
                        "status": d.status(i == 0),
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
        .enumerate()
        .map(|(i, d)| {
            vec![
                d.short_id(),
                // Cloudflare returns the active deployment first.
                d.status(i == 0).to_string(),
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
            "Status",
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

pub fn cf_tunnels_create(cfg: &CloudflareConfig, account: Option<&str>, name: &str) -> Result<()> {
    let (client, acc) = cf_client(cfg, account)?;
    let account_id = cf_account_id(&acc)?;
    let tunnel = client.create_tunnel(&account_id, name)?;
    if output::json_output() {
        output::print_json(&serde_json::to_value(&tunnel)?);
        return Ok(());
    }
    println!(
        "Tunnel '{}' created. Run `easypanel cf tunnels install {}` on the origin machine setup path.",
        tunnel.name, tunnel.name
    );
    Ok(())
}

pub fn cf_tunnels_install(
    cfg: &CloudflareConfig,
    account: Option<&str>,
    tunnel: &str,
) -> Result<()> {
    let (client, acc) = cf_client(cfg, account)?;
    let account_id = cf_account_id(&acc)?;
    let found = cf_resolve_tunnel(&client, &account_id, tunnel)?;
    let token = client.get_tunnel_token(&account_id, &found.id)?;
    let linux = format!("sudo cloudflared service install {token}");
    let docker = format!(
        "docker run -d --name cloudflared --restart unless-stopped cloudflare/cloudflared:latest tunnel --no-autoupdate run --token {token}"
    );
    if output::json_output() {
        output::print_json(&json!({
            "tunnel": found,
            "token": token,
            "commands": {
                "linux_service": linux,
                "docker": docker,
            }
        }));
        return Ok(());
    }
    println!("Install cloudflared for tunnel '{}':", found.name);
    println!();
    println!("Linux service:");
    println!("{linux}");
    println!();
    println!("Docker:");
    println!("{docker}");
    println!();
    println!("Token:");
    println!("{token}");
    Ok(())
}

pub fn cf_tunnels_delete(
    cfg: &CloudflareConfig,
    account: Option<&str>,
    tunnel: &str,
    yes: bool,
) -> Result<()> {
    let (client, acc) = cf_client(cfg, account)?;
    let account_id = cf_account_id(&acc)?;
    let found = cf_resolve_tunnel(&client, &account_id, tunnel)?;
    if !yes {
        let typed: String = Input::new()
            .with_prompt(format!(
                "Delete tunnel '{}'? Type the tunnel name to confirm",
                found.name
            ))
            .interact_text()?;
        if typed != found.name {
            println!("Tunnel name did not match — nothing deleted.");
            return Ok(());
        }
    }
    client.delete_tunnel(&account_id, &found.id)?;
    if output::json_output() {
        output::print_json(&json!({
            "deleted": true,
            "tunnel": found,
        }));
        return Ok(());
    }
    println!("Tunnel '{}' deleted.", found.name);
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

pub struct TunnelRouteAddOpts<'a> {
    pub account: Option<&'a str>,
    pub tunnel: &'a str,
    pub hostname: &'a str,
    pub service: &'a str,
    pub path: Option<&'a str>,
    pub origin_request_json: Option<&'a str>,
    pub dns: bool,
    pub zone: Option<&'a str>,
    pub proxied: bool,
}

pub struct TunnelRouteEditOpts<'a> {
    pub account: Option<&'a str>,
    pub tunnel: &'a str,
    pub hostname: &'a str,
    pub path: Option<&'a str>,
    pub service: Option<&'a str>,
    pub origin_request_json: Option<&'a str>,
    pub clear_origin_request: bool,
    pub dns: bool,
    pub zone: Option<&'a str>,
    pub proxied: bool,
}

pub struct TunnelRouteDeleteOpts<'a> {
    pub account: Option<&'a str>,
    pub tunnel: &'a str,
    pub hostname: &'a str,
    pub path: Option<&'a str>,
    pub delete_dns: bool,
    pub zone: Option<&'a str>,
}

pub fn cf_tunnels_route_add(cfg: &CloudflareConfig, opts: TunnelRouteAddOpts<'_>) -> Result<()> {
    let (client, acc) = cf_client(cfg, opts.account)?;
    let account_id = cf_account_id(&acc)?;
    let found = cf_resolve_tunnel(&client, &account_id, opts.tunnel)?;
    let origin_request = parse_tunnel_origin_request(opts.origin_request_json.unwrap_or(""))?;
    let rule = TunnelIngressRule::route(
        opts.hostname,
        opts.service,
        opts.path.unwrap_or(""),
        origin_request,
    );
    client.add_tunnel_route(&account_id, &found.id, rule)?;
    let mut suffix = String::new();
    if opts.dns {
        let record = upsert_tunnel_dns(
            &client,
            &acc,
            opts.hostname,
            &found,
            opts.zone,
            opts.proxied,
        )?;
        suffix = format!(
            " DNS CNAME '{}' -> {} updated.",
            record.name,
            found.target()
        );
    }
    println!(
        "Route '{}' -> '{}' added to tunnel '{}'.{}",
        opts.hostname.trim(),
        opts.service.trim(),
        found.name,
        suffix
    );
    Ok(())
}

pub fn cf_tunnels_route_edit(cfg: &CloudflareConfig, opts: TunnelRouteEditOpts<'_>) -> Result<()> {
    if opts.service.is_none()
        && opts.origin_request_json.is_none()
        && !opts.clear_origin_request
        && !opts.dns
    {
        anyhow::bail!("Nothing to change; pass --service, --origin-request-json, --clear-origin-request, or --dns");
    }
    if opts.origin_request_json.is_some() && opts.clear_origin_request {
        anyhow::bail!("Use either --origin-request-json or --clear-origin-request, not both");
    }
    let (client, acc) = cf_client(cfg, opts.account)?;
    let account_id = cf_account_id(&acc)?;
    let found = cf_resolve_tunnel(&client, &account_id, opts.tunnel)?;
    if opts.service.is_some() || opts.origin_request_json.is_some() || opts.clear_origin_request {
        let origin_request = if opts.clear_origin_request {
            Some(None)
        } else if let Some(raw) = opts.origin_request_json {
            Some(parse_tunnel_origin_request(raw)?)
        } else {
            None
        };
        client.edit_tunnel_route(
            &account_id,
            &found.id,
            TunnelRouteChange {
                hostname: opts.hostname.trim().into(),
                service: opts.service.map(|s| s.trim().into()),
                path: opts.path.map(|p| p.trim().into()),
                origin_request,
            },
        )?;
    }
    let mut suffix = String::new();
    if opts.dns {
        let record = upsert_tunnel_dns(
            &client,
            &acc,
            opts.hostname,
            &found,
            opts.zone,
            opts.proxied,
        )?;
        suffix = format!(
            " DNS CNAME '{}' -> {} updated.",
            record.name,
            found.target()
        );
    }
    println!(
        "Route '{}' on tunnel '{}' updated.{}",
        opts.hostname.trim(),
        found.name,
        suffix
    );
    Ok(())
}

pub fn cf_tunnels_route_delete(
    cfg: &CloudflareConfig,
    opts: TunnelRouteDeleteOpts<'_>,
) -> Result<()> {
    let (client, acc) = cf_client(cfg, opts.account)?;
    let account_id = cf_account_id(&acc)?;
    let found = cf_resolve_tunnel(&client, &account_id, opts.tunnel)?;
    client.delete_tunnel_route(&account_id, &found.id, opts.hostname, opts.path)?;
    let mut suffix = String::new();
    if opts.delete_dns {
        let deleted = delete_tunnel_dns(&client, &acc, opts.hostname, &found, opts.zone)?;
        suffix = format!(" Deleted {deleted} matching DNS CNAME record(s).");
    }
    println!(
        "Route '{}' removed from tunnel '{}'.{}",
        opts.hostname.trim(),
        found.name,
        suffix
    );
    Ok(())
}

fn cf_resolve_tunnel(
    client: &CloudflareClient,
    account_id: &str,
    needle: &str,
) -> Result<CloudflareTunnel> {
    client
        .list_tunnels(account_id)?
        .into_iter()
        .find(|t| t.id == needle || t.name == needle)
        .ok_or_else(|| anyhow!("No tunnel named or identified by '{needle}'"))
}

fn zone_for_hostname(
    client: &CloudflareClient,
    acc: &CloudflareAccount,
    hostname: &str,
    zone: Option<&str>,
) -> Result<Zone> {
    if let Some(zone) = zone {
        return cf_resolve_zone(client, acc, zone);
    }
    let zones = client.list_zones(acc.account_id.as_deref())?;
    zones
        .into_iter()
        .filter(|z| hostname == z.name || hostname.ends_with(&format!(".{}", z.name)))
        .max_by_key(|z| z.name.len())
        .ok_or_else(|| {
            anyhow!("Cannot infer a zone for '{hostname}'. Pass --zone <zone-name-or-id>.")
        })
}

fn upsert_tunnel_dns(
    client: &CloudflareClient,
    acc: &CloudflareAccount,
    hostname: &str,
    tunnel: &CloudflareTunnel,
    zone: Option<&str>,
    proxied: bool,
) -> Result<crate::cloudflare::Record> {
    let zone = zone_for_hostname(client, acc, hostname.trim(), zone)?;
    let target = tunnel.target();
    let body = record_body("CNAME", hostname.trim(), &target, 1, proxied, None);
    let matches = client.list_records(
        &zone.id,
        &RecordFilter {
            kind: Some("CNAME".into()),
            name: Some(hostname.trim().into()),
            content: None,
        },
    )?;
    if let Some(existing) = matches
        .into_iter()
        .find(|r| r.name.eq_ignore_ascii_case(hostname.trim()))
    {
        client.patch_record(&zone.id, &existing.id, &body)
    } else {
        client.create_record(&zone.id, &body)
    }
}

fn delete_tunnel_dns(
    client: &CloudflareClient,
    acc: &CloudflareAccount,
    hostname: &str,
    tunnel: &CloudflareTunnel,
    zone: Option<&str>,
) -> Result<usize> {
    let zone = zone_for_hostname(client, acc, hostname.trim(), zone)?;
    let target = tunnel.target();
    let records = client.list_records(
        &zone.id,
        &RecordFilter {
            kind: Some("CNAME".into()),
            name: Some(hostname.trim().into()),
            content: Some(target.clone()),
        },
    )?;
    let mut deleted = 0;
    for record in records {
        if record.name.eq_ignore_ascii_case(hostname.trim()) && record.content == target {
            client.delete_record(&zone.id, &record.id)?;
            deleted += 1;
        }
    }
    Ok(deleted)
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

/// `easypanel host shell` — an interactive shell on the HOST behind the panel.
///
/// Deliberately the SAME code path as the TUI's Hosts ▸ `t`: it opens the TUI
/// straight onto its terminal pane. There is no second implementation of the
/// session, the vt100 emulation or the key encoding — one pane, one protocol
/// handler, so a fix to either reaches both. Leaving the shell leaves you in the
/// TUI, which is the point: the host is where you were already working.
///
/// Gated like the destructive Docker cleanups above, and for a stronger reason:
/// EasyPanel's handler answers this route with
/// `docker run --privileged --net=host --pid=host --ipc=host -v /:/host … chroot /host`
/// (its own source, `/app/backend.js`, 2.32.2) — root on the real machine, not a
/// container. `--yes` skips the prompt for scripted use.
pub fn host_shell(
    cfg: &ServerConfig,
    client: EasypanelClient,
    server: &str,
    yes: bool,
) -> Result<()> {
    if !confirm(
        &format!(
            "Open a shell on the HOST behind '{server}'? This is a privileged root shell on \
             the host filesystem — every project on it, not one container."
        ),
        yes,
    )? {
        return Ok(());
    }
    crate::tui::run_host_shell(cfg, client, server.to_string())
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

/// The `type` of a service, read from its project's service list — so callers do
/// not have to guess which `services/{type}` path a service answers on.
fn service_stype(client: &EasypanelClient, project: &str, service: &str) -> Result<String> {
    let data = client.call(
        "projects",
        "inspectProject",
        json!({ "projectName": project }),
    )?;
    data.get("services")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|s| field(s, "/name") == service)
        .map(|s| field(s, "/type"))
        .ok_or_else(|| anyhow!("{project}/{service} not found"))
}

/// The engine of a database service. Only mysql/mariadb dump for now; anything
/// else is a clear error, not a silently wrong command.
fn resolve_db_engine(client: &EasypanelClient, project: &str, service: &str) -> Result<String> {
    let stype = service_stype(client, project, service)?;
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

/// The image a service runs, for a pre-flight to SHOW.
///
/// Never compared. A version skew is not predictable from a tag — `mysql:8`,
/// `mysql:8.0.36` and a private mirror's own name all describe servers whose
/// collations may or may not agree — so both ends are printed and the operator
/// decides. An unreadable image must not block a copy either, so this answers a
/// string rather than a `Result`.
fn service_image(client: &EasypanelClient, stype: &str, project: &str, service: &str) -> String {
    match client.call(
        &format!("services/{stype}"),
        "inspectService",
        json!({ "projectName": project, "serviceName": service }),
    ) {
        Ok(v) => field(&v, "/image"),
        Err(_) => "unknown".to_string(),
    }
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
    mut on_progress: impl FnMut(&str),
) -> Result<R2Dump> {
    let stype = resolve_db_engine(client, project, service)?;
    let root_password = service_root_password(client, &stype, project, service)?;
    let dbs = resolve_dump_databases(client, project, service, databases, all)?;
    let store = pick_remote_provider(client, provider)?;

    let now = chrono::Utc::now();
    let ts = now.format("%Y%m%d-%H%M%S").to_string();
    let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
    let key = crate::dump::dump_key(project, service, &ts);
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

    // The dump writes `{tmp}` minus its `.gz` first; watching both files is what
    // turns a silent twenty-minute job into a phase and a byte count.
    let sql = tmp.strip_suffix(".gz").unwrap_or(&tmp).to_string();
    let watch = [sql.clone(), tmp.clone()];
    // Built BEFORE the run, not after it: `run_until_done` needs it too, because a
    // launch that never confirms quotes what the container shell said and that
    // stream contains the shell's echo of `cmd` — password and presigned URL and
    // all. One definition of "what is secret here", used by both.
    let redact = |s: &str| {
        s.replace(&url, "<presigned-url>")
            .replace(&root_password, "<redacted>")
            .trim()
            .to_string()
    };
    let run = crate::container::run_until_done(
        client,
        project,
        service,
        &cmd,
        std::time::Duration::from_secs(3600),
        &watch,
        &redact,
        |sizes| {
            let of = |p: &str| sizes.iter().find(|(f, _)| f == p).map(|(_, b)| *b);
            on_progress(&crate::dump::dump_phase(of(&sql), of(&tmp)));
        },
    )?;
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

/// Repaint one status line in place on a terminal, and stay silent when stderr is
/// redirected — a log file full of `\r` fragments helps nobody. Progress belongs on
/// stderr so `easypanel db dump … > out` keeps its stdout clean.
fn progress_line(text: &str) {
    use std::io::{IsTerminal, Write};
    if std::io::stderr().is_terminal() {
        // Pad to overwrite a longer previous line, which would otherwise leave its
        // tail behind ("uploading" under "compressing — 2.9 GB of 23.6 GB").
        eprint!("\r  {text:<60}");
        let _ = std::io::stderr().flush();
    }
}

/// Clear the in-place progress line before printing anything final over it.
fn progress_done() {
    use std::io::{IsTerminal, Write};
    if std::io::stderr().is_terminal() {
        eprint!("\r{:<64}\r", "");
        let _ = std::io::stderr().flush();
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
    let started = std::time::Instant::now();
    let d = dump_to_r2(client, project, service, databases, all, provider, |p| {
        progress_line(&format!(
            "{p}  [{}]",
            crate::output::human_duration(started.elapsed().as_secs() as i64)
        ));
    });
    progress_done();
    let d = d?;
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
/// the CLI `db restore`, the second leg of [`copy_database`], and the TUI worker;
/// no confirmation or printing of its own.
///
/// `on_progress` receives the same kind of phase line the dump reports. It used
/// to watch nothing at all, which left the load half of a transfer silent for up
/// to an hour — the one part of the job where the operator most wants to know
/// whether anything is happening.
pub(crate) fn restore_from_r2(
    client: &EasypanelClient,
    project: &str,
    service: &str,
    path: &str,
    provider: Option<&str>,
    mut on_progress: impl FnMut(&str),
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

    // Same two files the dump watches, read in the other direction — see
    // `dump::restore_phase`. Reaching the `.sql`-alone phase is also the only
    // signal available that the CLIENT has started reading, i.e. that a failure
    // from here on can have written rows.
    let sql = tmp.strip_suffix(".gz").unwrap_or(&tmp).to_string();
    let watch = [sql.clone(), tmp.clone()];
    let mut loading = false;
    // Built BEFORE the run, for the same reason the dump builds it early: a launch
    // that never confirms quotes the container shell, whose stream contains the
    // echo of `cmd` — the presigned GET URL and the root password with it.
    let redact = |s: &str| {
        s.replace(&url, "<presigned-url>")
            .replace(&root_password, "<redacted>")
            .trim()
            .to_string()
    };
    let run = crate::container::run_until_done(
        client,
        project,
        service,
        &cmd,
        std::time::Duration::from_secs(3600),
        &watch,
        &redact,
        |sizes| {
            let of = |p: &str| sizes.iter().find(|(f, _)| f == p).map(|(_, b)| *b);
            let (s, g) = (of(&sql), of(&tmp));
            if s.is_some() && g.is_none() {
                loading = true;
            }
            on_progress(&crate::dump::restore_phase(s, g));
        },
    )?;
    match run.exit_code {
        Some(0) => Ok(()),
        // A load that had started can have replaced some tables and not others:
        // there is no transaction around DDL, so "it failed" does NOT mean "it
        // changed nothing". Say so rather than letting the operator assume.
        Some(n) if loading => anyhow::bail!(
            "Restore failed (exit {n}) while loading — {project}/{service} MAY BE \
             PARTIALLY WRITTEN: some of the dump's tables will have been replaced \
             and some not. Check it before re-running. {}",
            redact(&run.output)
        ),
        // The download or the decompression failed, so probably nothing was
        // written — but the sentinel is polled every 5 s and a very short load
        // could have come and gone between two polls, so this is not a promise.
        Some(n) => anyhow::bail!(
            "Restore failed (exit {n}) before the load was seen to start. \
             {project}/{service} is probably untouched, though a load shorter than \
             the 5 s poll could have been missed — check it before re-running. {}",
            redact(&run.output)
        ),
        None => anyhow::bail!(
            "Restore still running after 60 min — it continues in the container, so \
             {project}/{service} may be partially written until it finishes. Check \
             the service before re-running; a second restore may clash."
        ),
    }
}

// ---------- Copying a database from one service to another ----------
//
// Composed from the two halves above rather than given a data path of its own:
// `dump_to_r2` already pushes a non-locking, self-contained `.sql.gz` from the
// source container straight to shared storage, and `restore_from_r2` already
// pulls one into a target container and recreates the schemas. A copy is those
// two, in order, with the checks that only make sense when you can see BOTH ends.
//
// Same-host is not a special case: the two legs never talk to each other, only to
// the bucket, so "the same host" is simply the same client passed twice.

/// One end of a copy. The shape `migrate::Target` already uses for "where a thing
/// is going", so the two cross-host features describe a destination the same way.
pub(crate) struct CopyTarget<'a> {
    pub client: &'a EasypanelClient,
    pub project: &'a str,
    pub service: &'a str,
}

impl CopyTarget<'_> {
    fn name(&self) -> String {
        format!("{}/{}", self.project, self.service)
    }
}

/// What a copy is about to do, resolved from BOTH panels before anything is
/// dumped. Everything here is either a refusal that already passed or a fact the
/// operator is shown before being asked to confirm.
pub(crate) struct CopyPlan {
    /// The engine both ends share — a mismatch never gets this far.
    pub stype: String,
    pub databases: Vec<String>,
    pub bucket: String,
    pub endpoint: String,
    pub src_provider: String,
    pub dst_provider: String,
    pub src_image: String,
    pub dst_image: String,
}

/// Everything that must hold BEFORE a byte is dumped, checked against both
/// panels. Nothing here writes.
///
/// Kept apart from [`copy_database`] for two reasons. A 25 GB dump that lands in
/// a bucket the target cannot read is an hour wasted for a comparison that costs
/// one API call. And the operator can only weigh a version skew — which nothing
/// can predict, so both images are simply shown — while there is still nothing
/// overwritten to regret.
pub(crate) fn plan_copy(
    src: &CopyTarget,
    dst: &CopyTarget,
    databases: &[String],
    all: bool,
    provider: Option<&str>,
) -> Result<CopyPlan> {
    let src_stype = service_stype(src.client, src.project, src.service)?;
    let dst_stype = service_stype(dst.client, dst.project, dst.service)?;
    if let Some(reason) =
        crate::dump::copy_refusal(&src_stype, &src.name(), &dst_stype, &dst.name())
    {
        anyhow::bail!(reason);
    }

    // `--provider` is matched against each panel separately, by id OR name. Ids
    // are per-panel, so a name is the only value that can select on both ends.
    let src_store = pick_remote_provider(src.client, provider)?;
    let dst_store = pick_remote_provider(dst.client, provider)?;
    if let Some(reason) = crate::dump::store_refusal(
        &src_store.endpoint,
        &src_store.bucket,
        &dst_store.endpoint,
        &dst_store.bucket,
    ) {
        anyhow::bail!(reason);
    }

    let databases = resolve_dump_databases(src.client, src.project, src.service, databases, all)?;
    Ok(CopyPlan {
        src_image: service_image(src.client, &src_stype, src.project, src.service),
        dst_image: service_image(dst.client, &dst_stype, dst.project, dst.service),
        stype: src_stype,
        databases,
        bucket: src_store.bucket,
        endpoint: src_store.endpoint,
        src_provider: src_store.name,
        dst_provider: dst_store.name,
    })
}

/// Dump `databases` out of `src` and load them into `dst`, reporting through one
/// progress closure for the whole run. Shared by the CLI and the TUI, so both
/// surfaces get the identical behaviour.
///
/// `databases` must be the list a [`plan_copy`] already resolved and the operator
/// already agreed to — it is passed through as an explicit list so no second
/// resolution can quietly widen what was confirmed.
///
/// The phase words the two legs emit are already distinct ("uploading" vs
/// "downloading"), so nothing is prefixed onto them: the line stays exactly the
/// shape `db dump` and `db restore` produce, which is what lets a caller reuse
/// its existing progress closure verbatim.
pub(crate) fn copy_database(
    src: &CopyTarget,
    dst: &CopyTarget,
    databases: &[String],
    provider: Option<&str>,
    mut on_progress: impl FnMut(&str),
) -> Result<R2Dump> {
    let dump = dump_to_r2(
        src.client,
        src.project,
        src.service,
        databases,
        false,
        provider,
        &mut on_progress,
    )?;
    // The dump survives a failed load, and it is the expensive half — so the
    // error carries the key and the one command that retries only the load.
    // Without it the operator's only obvious move is to dump 25 GB again.
    restore_from_r2(
        dst.client,
        dst.project,
        dst.service,
        &dump.key,
        provider,
        &mut on_progress,
    )
    .map_err(|e| {
        anyhow!(
            "{e}\nThe dump itself succeeded and is kept at {}/{}. Retry just the \
             load with:  easypanel db restore {} {} --path {}",
            dump.bucket,
            dump.key,
            dst.project,
            dst.service,
            dump.key
        )
    })?;
    Ok(dump)
}

/// The `.sql.gz` object keys under `prefix`, sorted (so a key's trailing
/// `%Y%m%d-%H%M%S` stamp puts the newest last).
///
/// EVERY page is read, not just the first. `ListObjectsV2` answers at most 1000
/// keys and reports `IsTruncated` with a `NextContinuationToken`; reading one
/// page silently returned "no dumps" against a bucket this tool shares with
/// other things (the user's held ~250 KB of rotated app logs whose keys sort
/// ahead of every dump), which is a lie that looks exactly like an empty
/// history. `MAX_PAGES` bounds it so a pathological bucket cannot spin forever;
/// hitting the bound is an error rather than a short answer, for the same reason.
fn list_r2_keys(store: &RemoteStore, prefix: &str) -> Result<Vec<String>> {
    const MAX_PAGES: usize = 50;
    let http = reqwest::blocking::Client::new();
    let mut keys: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let amz_date = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
        let (url, auth) = crate::s3::sign_list(
            &store.endpoint,
            &store.bucket,
            prefix,
            token.as_deref(),
            &store.access_key,
            &store.secret_key,
            &store.region,
            &amz_date,
        );
        let body = http
            .get(&url)
            .header("Authorization", auth)
            .header("x-amz-date", amz_date)
            .header("x-amz-content-sha256", "UNSIGNED-PAYLOAD")
            .send()?
            .error_for_status()?
            .text()?;
        let (page, next) = crate::dump::parse_key_page(&body);
        keys.extend(page);
        match next {
            Some(t) => token = Some(t),
            None => {
                keys.sort();
                return Ok(keys);
            }
        }
    }
    anyhow::bail!(
        "the storage listing did not finish after {MAX_PAGES} pages of 1000 keys; \
         narrow it down with a per-service listing"
    )
}

/// The object keys of the dumps this tool has written for a service, newest last.
/// EasyPanel has no endpoint that lists them (they are the tool's own files, not
/// its backup actions), so this signs an S3 `ListObjectsV2` for the
/// `{project}/{service}-` prefix directly. Shared by the CLI `db list` and the TUI.
///
/// The store is resolved HERE and passed down: a fan-out over several prefixes
/// (the host-wide listing) would otherwise re-ask the panel which provider to
/// use once per service, and the answer cannot change mid-listing.
pub(crate) fn list_r2_dumps(
    client: &EasypanelClient,
    project: &str,
    service: &str,
    provider: Option<&str>,
) -> Result<Vec<String>> {
    let store = pick_remote_provider(client, provider)?;
    list_r2_keys(&store, &crate::dump::dump_prefix(project, service))
}

/// Every dump this tool has written on the host, whatever service it came from,
/// NEWEST FIRST — each row carrying its origin.
///
/// The same move `backup::history_all` makes, for the same reason: restoring
/// ACROSS services is a legitimate thing to want (`db restore --path` has always
/// accepted any key), and filtering the list by the destination's own prefix
/// showed an empty list and explained nothing. Newest first because a wide list
/// is long and the dump you just took is the one you came for; the per-service
/// listing keeps its newest-LAST order, which `db_restore` relies on to pick a
/// default.
///
/// Anything in the bucket that is not one of our keys (EasyPanel's own backups,
/// unrelated objects) is skipped by `parse_dump_key`.
pub(crate) fn list_all_r2_dumps(
    client: &EasypanelClient,
    provider: Option<&str>,
) -> Result<Vec<crate::dump::DumpKey>> {
    // Ask per SERVICE rather than scanning the whole bucket. The bucket may be
    // shared with hundreds of thousands of objects that are none of our
    // business, so a full scan is both slow and — before pagination — wrong;
    // `{project}/{service}-` filters server-side, and the panel already knows
    // every service it has. Only the engines this path can dump are asked for:
    // no other service has ever written one of these keys.
    //
    // The provider is resolved ONCE for the whole fan-out: it cannot change
    // between prefixes, and asking per service was an extra panel round-trip per
    // database service.
    let store = pick_remote_provider(client, provider)?;
    let all = client.call("projects", "listProjectsAndServices", Value::Null)?;
    let mut dumps: Vec<crate::dump::DumpKey> = Vec::new();
    for s in all
        .get("services")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let (project, service, stype) = (
            field(s, "/projectName"),
            field(s, "/name"),
            field(s, "/type"),
        );
        if !crate::dump::can_dump(&stype) {
            continue;
        }
        let keys = list_r2_keys(&store, &crate::dump::dump_prefix(&project, &service))?;
        dumps.extend(keys.iter().filter_map(|k| crate::dump::parse_dump_key(k)));
    }
    // Newest first across every service: the stamp sorts lexically, so one sort
    // on the key orders the whole set.
    dumps.sort_by(|a, b| b.ts.cmp(&a.ts).then_with(|| a.key.cmp(&b.key)));
    Ok(dumps)
}

pub fn db_list(
    client: &EasypanelClient,
    project: &str,
    service: &str,
    provider: Option<&str>,
    all_services: bool,
) -> Result<()> {
    // `db restore <project> <service> --path <key>` accepts ANY key, so the CLI
    // must be able to show the keys that are not this service's own — otherwise
    // the only restorable dumps a user can NAME are the ones already listed.
    if all_services {
        let dumps = list_all_r2_dumps(client, provider)?;
        if dumps.is_empty() {
            println!("No dumps found on this host's object storage.");
            return Ok(());
        }
        for d in dumps {
            println!("{}  {:<28}{}", d.when(), d.origin(), d.key);
        }
        return Ok(());
    }
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

/// Everything a browse/query step needs: which engine, and how to log into it.
///
/// The CLI and the TUI ask the same questions of the same module
/// (`crate::dbms`), so a query typed at a prompt and one typed in the TUI's box
/// build the identical command.
fn dbms_target(
    client: &EasypanelClient,
    project: &str,
    service: &str,
) -> Result<(crate::dbms::Engine, crate::dbms::Creds)> {
    let stype = service_stype(client, project, service)?;
    let engine = crate::dbms::Engine::from_service_type(&stype).ok_or_else(|| {
        anyhow!(crate::dbms::unsupported_reason(&stype)
            .unwrap_or_else(|| format!("{service} is '{stype}'")))
    })?;
    let inspect = client.call(
        &format!("services/{stype}"),
        "inspectService",
        json!({ "projectName": project, "serviceName": service }),
    )?;
    Ok((engine, crate::dbms::creds(engine, &inspect)))
}

/// Run one command in the database's container and hand back what it printed,
/// turning the engine's own complaint into this command's error.
fn dbms_capture(
    client: &EasypanelClient,
    project: &str,
    service: &str,
    command: &str,
) -> Result<String> {
    let cap = crate::container::run_capture(
        client,
        project,
        service,
        command,
        std::time::Duration::from_secs(60),
    )?;
    if let Some(msg) = crate::dbms::failure(&cap.output, cap.exit_code) {
        anyhow::bail!(msg);
    }
    if cap.truncated {
        eprintln!("warning: the output was cut — there was more than this.");
    }
    Ok(cap.output)
}

/// The database to work in: the one asked for, or the one EasyPanel recorded.
fn dbms_database(want: Option<&str>, creds: &crate::dbms::Creds) -> Result<String> {
    match want {
        Some(d) => Ok(d.to_string()),
        None if !creds.database.is_empty() => Ok(creds.database.clone()),
        None => anyhow::bail!(
            "This service has no database on record; name one with --database (see: db databases)."
        ),
    }
}

pub fn db_databases(client: &EasypanelClient, project: &str, service: &str) -> Result<()> {
    let (engine, creds) = dbms_target(client, project, service)?;
    let out = dbms_capture(
        client,
        project,
        service,
        &crate::dbms::list_databases_cmd(engine, &creds),
    )?;
    for name in crate::dbms::parse_names(&out) {
        println!("{name}");
    }
    Ok(())
}

pub fn db_tables(
    client: &EasypanelClient,
    project: &str,
    service: &str,
    database: Option<&str>,
) -> Result<()> {
    let (engine, creds) = dbms_target(client, project, service)?;
    let database = dbms_database(database, &creds)?;
    let out = dbms_capture(
        client,
        project,
        service,
        &crate::dbms::list_tables_cmd(engine, &creds, &database),
    )?;
    for name in crate::dbms::parse_names(&out) {
        println!("{name}");
    }
    Ok(())
}

/// Run a statement and print the result as a table. The statement is sent as
/// typed — no LIMIT is added, so an unbounded SELECT on a huge table is the
/// caller's decision, the same as it would be in the shell.
pub fn db_query(
    client: &EasypanelClient,
    project: &str,
    service: &str,
    database: Option<&str>,
    query: &str,
) -> Result<()> {
    let (engine, creds) = dbms_target(client, project, service)?;
    // A query MAY be server-wide (`SHOW DATABASES`, an admin command), so an
    // absent database is not an error here the way it is for a table listing.
    let database = database
        .map(str::to_string)
        .unwrap_or_else(|| creds.database.clone());
    let out = dbms_capture(
        client,
        project,
        service,
        &crate::dbms::query_cmd(engine, &creds, &database, query),
    )?;
    let grid = crate::dbms::parse_grid(engine, &out);
    if grid.columns.is_empty() {
        println!("The statement ran and returned nothing.");
        return Ok(());
    }
    let headers: Vec<&str> = grid.columns.iter().map(String::as_str).collect();
    crate::output::table(&headers, grid.rows);
    Ok(())
}

/// Where a downloaded dump lands: the `--out` path as given, the key's file name
/// inside it when it names an existing DIRECTORY, or that file name in the current
/// directory when no `--out` was passed.
fn dump_dest(out: Option<&str>, key: &str) -> std::path::PathBuf {
    let name = key.rsplit('/').next().unwrap_or("dump.sql.gz");
    match out {
        Some(o) if std::path::Path::new(o).is_dir() => std::path::Path::new(o).join(name),
        Some(o) => std::path::PathBuf::from(o),
        None => std::path::PathBuf::from(name),
    }
}

/// Download a dump this tool wrote to a local file. The container is not involved:
/// the object already sits in the remote storage, so a presigned GET streamed
/// straight to disk is the whole job (a multi-GB dump never enters memory).
/// `path` defaults to the NEWEST dump of the service. Shared by the CLI
/// `db download` and the TUI. Returns (object key, local path, bytes).
pub(crate) fn download_r2_dump(
    client: &EasypanelClient,
    project: &str,
    service: &str,
    path: Option<&str>,
    out: Option<&str>,
    provider: Option<&str>,
    mut on_progress: impl FnMut(u64, Option<u64>),
) -> Result<(String, std::path::PathBuf, u64)> {
    // `list_r2_dumps` sorts by key, and the key ends in a `%Y%m%d-%H%M%S` stamp,
    // so the last one is the newest.
    let key = match path {
        Some(p) => p.to_string(),
        None => list_r2_dumps(client, project, service, provider)?
            .pop()
            .ok_or_else(|| {
                anyhow!(
                    "No dumps found for {project}/{service}. \
                     Make one with: easypanel db dump {project} {service} --all"
                )
            })?,
    };
    let dest = dump_dest(out, &key);
    // Never write over a file that is already there: the caller may have edited or
    // already restored from it, and `--out` makes choosing another path trivial.
    if dest.exists() {
        anyhow::bail!(
            "{} already exists; pass --out to write elsewhere.",
            dest.display()
        );
    }
    let store = pick_remote_provider(client, provider)?;
    let amz_date = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let url = crate::s3::presign(
        "GET",
        &store.endpoint,
        &store.bucket,
        &key,
        &store.access_key,
        &store.secret_key,
        &store.region,
        &amz_date,
        3600,
    );
    let mut resp = reqwest::blocking::Client::new()
        .get(&url)
        .send()?
        .error_for_status()
        .map_err(|e| anyhow!("Cannot read {key} from {}: {e}", store.name))?;
    let total = resp.content_length();
    let mut file = std::fs::File::create(&dest)?;
    // Copied by hand rather than with `std::io::copy` only so the caller can be told
    // how far a multi-GB download has got. Reported at most twice a second: the
    // reader loop runs thousands of times a second and the TUI would drown in ticks.
    use std::io::{Read, Write};
    let mut buf = vec![0u8; 128 * 1024];
    let (mut bytes, mut last) = (0u64, std::time::Instant::now());
    loop {
        let n = resp.read(&mut buf)?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        bytes += n as u64;
        if last.elapsed() >= std::time::Duration::from_millis(500) {
            on_progress(bytes, total);
            last = std::time::Instant::now();
        }
    }
    Ok((key, dest, bytes))
}

/// "1.2 GB of 3.9 GB (31%)", or just the bytes when the server sent no length.
pub(crate) fn download_progress(bytes: u64, total: Option<u64>) -> String {
    let done = crate::output::format_bytes(bytes as f64);
    match total.filter(|t| *t > 0) {
        Some(t) => format!(
            "{done} of {} ({}%)",
            crate::output::format_bytes(t as f64),
            bytes * 100 / t
        ),
        None => done,
    }
}

pub fn db_download(
    client: &EasypanelClient,
    project: &str,
    service: &str,
    path: Option<&str>,
    out: Option<&str>,
    provider: Option<&str>,
) -> Result<()> {
    let got = download_r2_dump(client, project, service, path, out, provider, |b, t| {
        progress_line(&download_progress(b, t));
    });
    progress_done();
    let (key, dest, bytes) = got?;
    println!(
        "Downloaded {key} → {} ({}).",
        dest.display(),
        crate::output::format_bytes(bytes as f64)
    );
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
    let started = std::time::Instant::now();
    let done = restore_from_r2(client, project, service, path, provider, |p| {
        progress_line(&format!(
            "{p}  [{}]",
            crate::output::human_duration(started.elapsed().as_secs() as i64)
        ));
    });
    progress_done();
    done?;
    println!("Restored {path} into {project}/{service}.");
    Ok(())
}

/// Everything `db copy` needs. A struct rather than a dozen arguments, the shape
/// `TunnelRouteAddOpts` already uses here.
pub struct DbCopyOpts<'a> {
    pub src: &'a EasypanelClient,
    pub src_server: &'a str,
    pub src_project: &'a str,
    pub src_service: &'a str,
    pub dst: &'a EasypanelClient,
    pub dst_server: &'a str,
    pub dst_project: &'a str,
    pub dst_service: &'a str,
    pub databases: &'a [String],
    pub all: bool,
    pub provider: Option<&'a str>,
    pub yes: bool,
}

/// Copy databases from one service into another — on this host, or on another
/// one with `--to-server`.
///
/// The pre-flight is printed BEFORE the confirmation deliberately: the two images
/// are the only version-skew signal there is, and they are worth nothing after
/// the target has been overwritten.
pub fn db_copy(opts: DbCopyOpts<'_>) -> Result<()> {
    let src = CopyTarget {
        client: opts.src,
        project: opts.src_project,
        service: opts.src_service,
    };
    let dst = CopyTarget {
        client: opts.dst,
        project: opts.dst_project,
        service: opts.dst_service,
    };
    let plan = plan_copy(&src, &dst, opts.databases, opts.all, opts.provider)?;

    println!(
        "Copy {} database(s): {}:{}/{} → {}:{}/{}",
        plan.databases.len(),
        opts.src_server,
        opts.src_project,
        opts.src_service,
        opts.dst_server,
        opts.dst_project,
        opts.dst_service,
    );
    println!("  engine        {}", plan.stype);
    println!("  databases     {}", plan.databases.join(", "));
    // Shown, never compared — a tag cannot tell you whether a load will fit.
    println!("  source image  {}", plan.src_image);
    println!("  target image  {}", plan.dst_image);
    println!(
        "  via storage   {}/{} (provider '{}' on {}, '{}' on {})",
        plan.endpoint,
        plan.bucket,
        plan.src_provider,
        opts.src_server,
        plan.dst_provider,
        opts.dst_server,
    );

    if !confirm(
        &format!(
            "Load {} into {}/{} on {}? Each of those databases there is DROPPED and \
             recreated from this dump — anything in it the dump does not contain \
             is gone and cannot be recovered.",
            plan.databases.join(", "),
            opts.dst_project,
            opts.dst_service,
            opts.dst_server
        ),
        opts.yes,
    )? {
        return Ok(());
    }

    let started = std::time::Instant::now();
    let copied = copy_database(&src, &dst, &plan.databases, opts.provider, |p| {
        progress_line(&format!(
            "{p}  [{}]",
            crate::output::human_duration(started.elapsed().as_secs() as i64)
        ));
    });
    progress_done();
    let copied = copied?;
    println!(
        "Copied {} database(s) into {}/{} on {}: {}.",
        copied.databases.len(),
        opts.dst_project,
        opts.dst_service,
        opts.dst_server,
        copied.databases.join(", ")
    );
    // Filed under the SOURCE service (see `dump::dump_key`), so it will NOT show
    // up under the target — which makes printing the whole key the only way the
    // operator finds it again from this side.
    println!(
        "Dump kept at {}/{} on {} (provider '{}'), listed by:  easypanel db list {} {}",
        copied.bucket,
        copied.key,
        opts.src_server,
        copied.provider,
        opts.src_project,
        opts.src_service
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;

    /// A downloaded dump keeps its own name unless a file path is given, and an
    /// `--out` that is a DIRECTORY must not be written to as if it were a file.
    #[test]
    fn a_download_lands_on_the_dumps_own_name_unless_told_otherwise() {
        use std::path::Path;
        let key = "shop/db-20260101-101010.sql.gz";
        assert_eq!(dump_dest(None, key), Path::new("db-20260101-101010.sql.gz"));
        assert_eq!(
            dump_dest(Some("/tmp/mine.gz"), key),
            Path::new("/tmp/mine.gz")
        );
        // An existing directory keeps the dump's file name inside it.
        let dir = std::env::temp_dir();
        assert_eq!(
            dump_dest(Some(dir.to_str().unwrap()), key),
            dir.join("db-20260101-101010.sql.gz")
        );
    }

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
