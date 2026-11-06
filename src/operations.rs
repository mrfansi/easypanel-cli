//! Noninteractive operations shared by the CLI, worker and native desktop.

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

use crate::client::EasypanelClient;
use crate::output::field;

/// A process-random nonce and monotonic sequence separate concurrent clients,
/// even when their wall clocks (and another machine's PID) coincide.
fn transfer_stamp(now: &chrono::DateTime<chrono::Utc>) -> String {
    use std::hash::BuildHasher;
    use std::sync::{
        atomic::{AtomicU64, Ordering},
        LazyLock,
    };
    static NEXT: AtomicU64 = AtomicU64::new(0);
    static NONCE: LazyLock<(u64, u64)> = LazyLock::new(|| {
        let random = std::collections::hash_map::RandomState::new();
        (
            random.hash_one((std::time::SystemTime::now(), 0u8)),
            random.hash_one((std::time::SystemTime::now(), 1u8)),
        )
    });
    let mut sequence = NEXT.load(Ordering::Relaxed);
    loop {
        let next = sequence
            .checked_add(1)
            .expect("transfer sequence exhausted");
        match NEXT.compare_exchange_weak(sequence, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(current) => sequence = current,
        }
    }
    format!(
        "{}_{:08x}{:016x}{:016x}{sequence:016x}",
        now.format("%Y%m%d-%H%M%S"),
        std::process::id(),
        NONCE.0,
        NONCE.1
    )
}

pub fn valid_name(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// Inspection responses are untyped: retain either observed env representation,
/// but never turn malformed remote content into an empty editor.
pub fn env_content(service: &Value) -> Result<&str> {
    anyhow::ensure!(service.is_object(), "service inspection must be an object");
    match service.get("env") {
        None | Some(Value::Null) => Ok(""),
        Some(Value::String(content)) => Ok(content),
        Some(Value::Object(env)) => match env.get("content") {
            Some(Value::String(content)) => Ok(content),
            None if env.is_empty() => Ok(""),
            _ => Err(anyhow!("service env.content must be a string")),
        },
        _ => Err(anyhow!(
            "service env must be a string or an object with string content"
        )),
    }
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
    anyhow::ensure!(cur.is_object(), "service inspection must be an object");
    let mut body = json!({ "projectName": project, "serviceName": service, "env": env });
    let op = match stype {
        "wordpress" | "box" => {
            body["env"] = json!({ "content": env });
            "updateEnv"
        }
        "app" | "compose" => {
            // These are different file capabilities; neither belongs on WordPress/Box.
            let key = if stype == "app" {
                "dotEnvPath"
            } else {
                "createDotEnv"
            };
            if let Some(value) = cur.get(key).filter(|value| !value.is_null()) {
                anyhow::ensure!(
                    if stype == "app" {
                        value.as_str().is_some_and(|path| !path.is_empty())
                    } else {
                        value.is_boolean()
                    },
                    "service {key} has an invalid value"
                );
                body[key] = value.clone();
            }
            "updateEnv"
        }
        kind if crate::lifecycle::is_database(kind) => {
            // updateAdvanced replaces the block, not just env.
            for key in ["image", "command", "configFile"] {
                match cur.get(key) {
                    Some(Value::String(value)) => body[key] = json!(value),
                    None | Some(Value::Null) if key != "configFile" => body[key] = json!(""),
                    None | Some(Value::Null) => {}
                    _ => return Err(anyhow!("service {key} must be a string")),
                }
            }
            "updateAdvanced"
        }
        _ => return Err(anyhow!("unsupported environment service type: {stype}")),
    };
    client.call(&grp, op, body)?;
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
pub(crate) struct RemoteStore {
    name: String,
    access_key: String,
    secret_key: String,
    bucket: String,
    endpoint: String,
    region: String,
}

/// The `type` of a service, read from its project's service list — so callers do
/// not have to guess which `services/{type}` path a service answers on.
pub(crate) fn service_stype(
    client: &EasypanelClient,
    project: &str,
    service: &str,
) -> Result<String> {
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
    let ts = transfer_stamp(&now);
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
    on_progress: impl FnMut(&str),
) -> Result<()> {
    restore_from_r2_checked(client, project, service, path, provider, None, on_progress)
}

#[allow(clippy::too_many_arguments)]
fn restore_from_r2_checked(
    client: &EasypanelClient,
    project: &str,
    service: &str,
    path: &str,
    provider: Option<&str>,
    expected: Option<&CopyPlan>,
    mut on_progress: impl FnMut(&str),
) -> Result<()> {
    let stype = resolve_db_engine(client, project, service)?;
    let root_password = service_root_password(client, &stype, project, service)?;
    let store = pick_remote_provider(client, provider)?;
    if let Some(plan) = expected {
        if stype != plan.stype
            || store.name != plan.dst_provider
            || crate::dump::store_refusal(
                &plan.endpoint,
                &plan.bucket,
                &store.endpoint,
                &store.bucket,
            )
            .is_some()
            || service_image(client, &stype, project, service) != plan.dst_image
        {
            anyhow::bail!("Target configuration changed before restore; nothing was restored. Review a new copy preflight.");
        }
    }

    let now = chrono::Utc::now();
    let ts = transfer_stamp(&now);
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
#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
    if src.project == dst.project
        && src.service == dst.service
        && src.client.same_configured_target(dst.client)
    {
        anyhow::bail!("Source and target are the same configured panel/service; refusing to overwrite itself.");
    }
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

/// Execute the operator's original reviewed plan, reporting one progress stream.
/// The authoritative database set and nonsecret engine/image/storage facts come
/// from that plan, never a replacement baseline captured after confirmation.
///
/// The phase words the two legs emit are already distinct ("uploading" vs
/// "downloading"), so nothing is prefixed onto them: the line stays exactly the
/// shape `db dump` and `db restore` produce, which is what lets a caller reuse
/// its existing progress closure verbatim.
pub(crate) fn copy_database(
    src: &CopyTarget,
    dst: &CopyTarget,
    confirmed: &CopyPlan,
    provider: Option<&str>,
    mut on_progress: impl FnMut(&str),
) -> Result<R2Dump> {
    let fresh = plan_copy(src, dst, &confirmed.databases, false, provider)?;
    require_unchanged_copy_plan(confirmed, &fresh)?;
    let dump = dump_to_r2(
        src.client,
        src.project,
        src.service,
        &confirmed.databases,
        false,
        provider,
        &mut on_progress,
    )?;
    // The dump survives a failed load, and it is the expensive half — so the
    // error carries the key and the one command that retries only the load.
    // Without it the operator's only obvious move is to dump 25 GB again.
    (|| -> Result<()> {
        let fresh = plan_copy(src, dst, &confirmed.databases, false, provider)?;
        require_unchanged_copy_plan(confirmed, &fresh)?;
        if dump.databases != confirmed.databases || dump.bucket != confirmed.bucket
            || dump.provider != confirmed.src_provider
        {
            anyhow::bail!("The completed dump's database/storage facts changed; nothing was restored. Review a new copy preflight.");
        }
        restore_from_r2_checked(
            dst.client, dst.project, dst.service, &dump.key, provider,
            Some(confirmed), &mut on_progress,
        )
    })()
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

fn require_unchanged_copy_plan(confirmed: &CopyPlan, fresh: &CopyPlan) -> Result<()> {
    if fresh != confirmed {
        anyhow::bail!("Copy preflight changed since confirmation; nothing was restored. No write was started for this leg. Review the new database, engine, images and storage facts before retrying.");
    }
    Ok(())
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
            .send()
            .map_err(reqwest::Error::without_url)?
            .error_for_status()
            .map_err(reqwest::Error::without_url)?
            .text()
            .map_err(reqwest::Error::without_url)?;
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
    let resp = dump_download_response(&url, &key, &store.name)?;
    let total = resp.content_length();
    let bytes = download_to_new_file(resp, &dest, total, &mut on_progress)?;
    Ok((key, dest, bytes))
}

fn dump_download_response(
    url: &str,
    key: &str,
    provider: &str,
) -> Result<reqwest::blocking::Response> {
    reqwest::blocking::Client::new()
        .get(url)
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|error| anyhow!("Cannot read {key} from {provider}: {}", error.without_url()))
}

fn download_to_new_file(
    mut reader: impl std::io::Read,
    dest: &std::path::Path,
    total: Option<u64>,
    mut on_progress: impl FnMut(u64, Option<u64>),
) -> Result<u64> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(dest)?;
    // Copied by hand rather than with `std::io::copy` only so the caller can be told
    // how far a multi-GB download has got. Reported at most twice a second: the
    // reader loop runs thousands of times a second and the TUI would drown in ticks.
    let transfer = (|| -> Result<u64> {
        let mut buf = vec![0u8; 128 * 1024];
        let (mut bytes, mut last) = (0u64, std::time::Instant::now());
        loop {
            let n = reader.read(&mut buf).map_err(storage_read_error)?;
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
        file.flush()?;
        Ok(bytes)
    })();
    drop(file);
    match transfer {
        Ok(bytes) => Ok(bytes),
        Err(error) => {
            if let Err(cleanup) = std::fs::remove_file(dest) {
                anyhow::bail!(
                    "{error}; could not remove partial download {}: {cleanup}",
                    dest.display()
                );
            }
            Err(error)
        }
    }
}

fn storage_read_error(error: std::io::Error) -> std::io::Error {
    let kind = error.kind();
    match error.into_inner() {
        Some(error) => match error.downcast::<reqwest::Error>() {
            Ok(error) => std::io::Error::new(kind, (*error).without_url()),
            Err(error) => std::io::Error::new(kind, error),
        },
        None => std::io::Error::from(kind),
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;

    fn copy_plan() -> CopyPlan {
        CopyPlan {
            stype: "mysql".into(),
            databases: vec!["shop".into()],
            bucket: "shared".into(),
            endpoint: "https://objects.test".into(),
            src_provider: "shared".into(),
            dst_provider: "shared".into(),
            src_image: "mysql:8".into(),
            dst_image: "mysql:8".into(),
        }
    }

    #[test]
    fn every_confirmed_copy_fact_is_revalidated_before_the_restore_leg() {
        let confirmed = copy_plan();
        assert!(require_unchanged_copy_plan(&confirmed, &confirmed).is_ok());
        let mut alternatives = Vec::new();
        let mut changed = confirmed.clone();
        changed.stype = "mariadb".into();
        alternatives.push(changed);
        let mut changed = confirmed.clone();
        changed.databases.push("unexpected".into());
        alternatives.push(changed);
        let mut changed = confirmed.clone();
        changed.bucket = "other".into();
        alternatives.push(changed);
        let mut changed = confirmed.clone();
        changed.endpoint = "https://other.test".into();
        alternatives.push(changed);
        let mut changed = confirmed.clone();
        changed.src_provider = "other".into();
        alternatives.push(changed);
        let mut changed = confirmed.clone();
        changed.dst_provider = "other".into();
        alternatives.push(changed);
        let mut changed = confirmed.clone();
        changed.src_image = "mysql:9".into();
        alternatives.push(changed);
        let mut changed = confirmed.clone();
        changed.dst_image = "mysql:9".into();
        alternatives.push(changed);
        for changed in alternatives {
            let error = require_unchanged_copy_plan(&confirmed, &changed).unwrap_err();
            assert!(error.to_string().contains("nothing was restored"));
        }
    }

    #[test]
    fn copy_entry_rejects_drift_from_the_actual_reviewed_plan_before_starting_a_dump() {
        let source_host = MockServer::start();
        let target_host = MockServer::start();
        for (host, provider_name) in [
            (&source_host, "source-store"),
            (&target_host, "target-store"),
        ] {
            host.mock(|when, then| {
                when.method(POST).path("/api/rpc/projects/inspectProject");
                then.status(200).json_body(json!({"json": {
                    "services": [{"name": "db", "type": "mysql"}]
                }}));
            });
            host.mock(|when, then| {
                when.method(POST)
                    .path("/api/rpc/storageProviders/common/list");
                then.status(200).json_body(json!({"json": [{
                    "name": provider_name, "type": "s3", "bucket": "shared",
                    "endpoint": "https://objects.test", "accessKeyId": "fixture",
                    "secretAccessKey": "fixture", "region": "auto"
                }]}));
            });
        }
        let mut old_source = source_host.mock(|when, then| {
            when.method(POST)
                .path("/api/rpc/services/mysql/inspectService");
            then.status(200)
                .json_body(json!({"json": {"image": "mysql:8"}}));
        });
        target_host.mock(|when, then| {
            when.method(POST)
                .path("/api/rpc/services/mysql/inspectService");
            then.status(200)
                .json_body(json!({"json": {"image": "mysql:8"}}));
        });
        let source_client = EasypanelClient::new(&source_host.base_url(), "fixture");
        let target_client = EasypanelClient::new(&target_host.base_url(), "fixture");
        let source = CopyTarget {
            client: &source_client,
            project: "shop",
            service: "db",
        };
        let target = CopyTarget {
            client: &target_client,
            project: "shop",
            service: "db",
        };
        let reviewed = plan_copy(&source, &target, &["shop".into()], false, None).unwrap();
        assert_eq!(reviewed.src_provider, "source-store");
        assert_eq!(reviewed.dst_provider, "target-store");
        // The GUI carries these typed, nonsecret facts through its task payload.
        let reviewed: CopyPlan =
            serde_json::from_value(serde_json::to_value(reviewed).unwrap()).unwrap();
        old_source.delete();
        let changed_source = source_host.mock(|when, then| {
            when.method(POST)
                .path("/api/rpc/services/mysql/inspectService");
            then.status(200)
                .json_body(json!({"json": {"image": "mysql:9"}}));
        });
        let mut progressed = false;
        let error = copy_database(&source, &target, &reviewed, None, |_| progressed = true)
            .err()
            .expect("entry must reject facts changed after the operator reviewed them");
        assert!(error.to_string().contains("changed since confirmation"));
        assert!(
            !progressed,
            "a rejected reviewed plan must not start dumping"
        );
        // Exactly the image revalidation: no second inspect to fetch dump credentials.
        changed_source.assert_hits(1);
        assert_eq!(reviewed.src_image, "mysql:8");
    }

    #[test]
    fn restore_refuses_newly_resolved_unconfirmed_provider_before_running_a_container_command() {
        let host = MockServer::start();
        host.mock(|when, then| {
            when.method(POST).path("/api/rpc/projects/inspectProject");
            then.status(200).json_body(json!({"json": {
                "services": [{"name": "db", "type": "mysql"}]
            }}));
        });
        host.mock(|when, then| {
            when.method(POST)
                .path("/api/rpc/services/mysql/inspectService");
            then.status(200).json_body(json!({"json": {
                "rootPassword": "fixture-password", "image": "mysql:8"
            }}));
        });
        host.mock(|when, then| {
            when.method(POST)
                .path("/api/rpc/storageProviders/common/list");
            then.status(200).json_body(json!({"json": [{
                "name": "unconfirmed", "type": "s3", "bucket": "shared",
                "endpoint": "https://objects.test", "accessKeyId": "fixture",
                "secretAccessKey": "fixture", "region": "auto"
            }]}));
        });
        let client = EasypanelClient::new(&host.base_url(), "fixture-token");
        let error = restore_from_r2_checked(
            &client,
            "shop",
            "db",
            "kept-dump.sql.gz",
            None,
            Some(&copy_plan()),
            |_| {},
        )
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("Target configuration changed before restore"));
    }

    #[cfg(unix)]
    #[test]
    fn sql_download_is_private_before_the_first_stream_byte() {
        use std::os::unix::fs::PermissionsExt;
        struct CheckPermissions {
            path: std::path::PathBuf,
            read: bool,
        }
        impl std::io::Read for CheckPermissions {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                assert_eq!(
                    std::fs::metadata(&self.path)?.permissions().mode() & 0o777,
                    0o600
                );
                if self.read {
                    return Ok(0);
                }
                self.read = true;
                buffer[0] = b'x';
                Ok(1)
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("private.sql.gz");
        let reader = CheckPermissions {
            path: path.clone(),
            read: false,
        };
        assert_eq!(
            download_to_new_file(reader, &path, Some(1), |_, _| {}).unwrap(),
            1
        );
        assert_eq!(std::fs::read(path).unwrap(), b"x");
    }

    #[test]
    fn concurrent_transfers_at_the_same_clock_have_distinct_files_and_object_keys() {
        use std::collections::HashSet;
        let now = chrono::DateTime::parse_from_rfc3339("2026-09-05T10:10:10Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let start = std::sync::Barrier::new(32);
        let transfers = std::thread::scope(|scope| {
            let mut tasks = Vec::new();
            for _ in 0..32 {
                let start = &start;
                let now = &now;
                tasks.push(scope.spawn(move || {
                    start.wait();
                    let stamp = transfer_stamp(now);
                    let key = crate::dump::dump_key("shop", "mysql-r2", &stamp);
                    (
                        format!("/tmp/ezp-dump-{stamp}.sql.gz"),
                        format!("/tmp/ezp-restore-{stamp}.sql.gz"),
                        key,
                    )
                }));
            }
            tasks
                .into_iter()
                .map(|task| task.join().unwrap())
                .collect::<Vec<_>>()
        });
        assert_eq!(
            transfers
                .iter()
                .map(|(dump, _, _)| dump)
                .collect::<HashSet<_>>()
                .len(),
            32
        );
        assert_eq!(
            transfers
                .iter()
                .map(|(_, restore, _)| restore)
                .collect::<HashSet<_>>()
                .len(),
            32
        );
        assert_eq!(
            transfers
                .iter()
                .map(|(_, _, key)| key)
                .collect::<HashSet<_>>()
                .len(),
            32
        );
        for (dump, restore, key) in transfers {
            assert_ne!(dump, restore);
            let parsed = crate::dump::parse_dump_key(&key).unwrap();
            assert_eq!(parsed.origin(), "shop/mysql-r2");
            assert_eq!(parsed.when(), "2026-09-05 10:10:10");
        }
    }

    #[test]
    fn presigned_download_status_and_connection_errors_do_not_expose_query_credentials() {
        let host = MockServer::start();
        host.mock(|when, then| {
            when.method(GET).path("/dump");
            then.status(403);
        });
        let query = "X-Amz-Credential=fixture-access&X-Amz-Signature=fixture-signature";
        let error = dump_download_response(
            &format!("{}/dump?{query}", host.base_url()),
            "dump.sql.gz",
            "fixture",
        )
        .expect_err("HTTP refusal must fail");
        let message = format!("{error:#}");
        assert!(message.contains("403"));
        assert!(!message.contains("fixture-access"));
        assert!(!message.contains("fixture-signature"));
        assert!(!message.contains("X-Amz"));

        let socket = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = socket.local_addr().unwrap();
        drop(socket);
        let error = dump_download_response(
            &format!("http://{address}/dump?{query}"),
            "dump.sql.gz",
            "fixture",
        )
        .expect_err("closed local socket must fail");
        assert!(!format!("{error:#}").contains("X-Amz"));
    }

    #[test]
    fn truncated_presigned_download_removes_partial_without_exposing_its_url() {
        use std::io::{Read, Write};
        let socket = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = socket.local_addr().unwrap();
        let sender = std::thread::spawn(move || {
            let (mut stream, _) = socket.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut request = [0; 4096];
            let mut received = 0;
            while !request[..received]
                .windows(4)
                .any(|bytes| bytes == b"\r\n\r\n")
            {
                assert!(
                    received < request.len(),
                    "request headers exceed fixture limit"
                );
                let amount = stream.read(&mut request[received..]).unwrap();
                assert!(amount > 0, "request ended before its headers");
                received += amount;
            }
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\npartial",
                )
                .unwrap();
        });
        let url = format!("http://{address}/dump?X-Amz-Signature=fixture-signature");
        let response = dump_download_response(&url, "dump.sql.gz", "fixture").unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dump.sql.gz");
        let error = download_to_new_file(response, &path, Some(100), |_, _| {}).unwrap_err();
        sender.join().unwrap();
        assert!(!path.exists());
        assert!(!format!("{error:#}").contains("fixture-signature"));
        assert!(!format!("{error:#}").contains("X-Amz"));
    }

    #[test]
    fn download_never_overwrites_existing_files_and_removes_its_new_partial() {
        let dir = tempfile::tempdir().unwrap();
        let existing = dir.path().join("existing.sql.gz");
        std::fs::write(&existing, b"original").unwrap();
        assert!(download_to_new_file(&b"replacement"[..], &existing, None, |_, _| {}).is_err());
        assert_eq!(std::fs::read(&existing).unwrap(), b"original");

        struct FailingReader(bool);
        impl std::io::Read for FailingReader {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                if self.0 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::ConnectionReset,
                        "fixture stream failure",
                    ));
                }
                self.0 = true;
                buf[..7].copy_from_slice(b"partial");
                Ok(7)
            }
        }
        let partial = dir.path().join("partial.sql.gz");
        assert!(download_to_new_file(FailingReader(false), &partial, Some(14), |_, _| {}).is_err());
        assert!(!partial.exists());
        let complete = dir.path().join("complete.sql.gz");
        assert_eq!(
            download_to_new_file(&b"complete"[..], &complete, Some(8), |_, _| {}).unwrap(),
            8
        );
        assert_eq!(std::fs::read(complete).unwrap(), b"complete");
    }

    #[test]
    fn copy_preflight_refuses_the_same_configured_service_before_network_io() {
        let source = EasypanelClient::new("https://PANEL.example.test:443/", "first");
        let target = EasypanelClient::new("https://panel.example.test", "rotated");
        let source = CopyTarget {
            client: &source,
            project: "shop",
            service: "db",
        };
        let target = CopyTarget {
            client: &target,
            project: "shop",
            service: "db",
        };
        let error = plan_copy(&source, &target, &["shop".into()], false, None)
            .err()
            .expect("a copy onto itself must be refused");
        assert!(error.to_string().contains("same configured panel/service"));
    }

    #[test]
    fn copy_preflight_keeps_cross_host_same_named_services_valid() {
        let source = MockServer::start();
        let target = MockServer::start();
        for host in [&source, &target] {
            host.mock(|when, then| {
                when.method(POST).path("/api/rpc/projects/inspectProject");
                then.status(200).json_body(json!({"json": {
                    "services": [{"name": "db", "type": "mysql"}]
                }}));
            });
            host.mock(|when, then| {
                when.method(POST)
                    .path("/api/rpc/storageProviders/common/list");
                then.status(200).json_body(json!({"json": [{
                    "name": "shared", "type": "s3", "bucket": "shared",
                    "endpoint": "https://objects.example.test",
                    "accessKeyId": "fixture", "secretAccessKey": "fixture", "region": "auto"
                }]}));
            });
            host.mock(|when, then| {
                when.method(POST)
                    .path("/api/rpc/services/mysql/inspectService");
                then.status(200)
                    .json_body(json!({"json": {"image": "mysql:8"}}));
            });
        }
        let source = EasypanelClient::new(&source.base_url(), "fixture");
        let target = EasypanelClient::new(&target.base_url(), "fixture");
        let source = CopyTarget {
            client: &source,
            project: "shop",
            service: "db",
        };
        let target = CopyTarget {
            client: &target,
            project: "shop",
            service: "db",
        };
        let plan = plan_copy(&source, &target, &["shop".into()], false, Some("shared")).unwrap();
        assert_eq!(plan.databases, ["shop"]);
        assert_eq!(plan.bucket, "shared");
    }

    #[test]
    fn copy_preflight_rejects_different_buckets_before_dumping() {
        let source = MockServer::start();
        let target = MockServer::start();
        for host in [&source, &target] {
            host.mock(|when, then| {
                when.method(POST).path("/api/rpc/projects/inspectProject");
                then.status(200).json_body(json!({"json": {
                    "services": [{"name": "db", "type": "mysql"}]
                }}));
            });
        }
        for (host, bucket) in [(&source, "source"), (&target, "wrong")] {
            host.mock(|when, then| {
                when.method(POST)
                    .path("/api/rpc/storageProviders/common/list");
                then.status(200).json_body(json!({"json": [{
                    "id": "provider", "name": "shared", "type": "s3",
                    "endpoint": "https://objects.example.test", "bucket": bucket,
                    "accessKeyId": "fixture", "secretAccessKey": "fixture", "region": "auto"
                }]}));
            });
        }
        let source_client = EasypanelClient::new(&source.base_url(), "fixture");
        let target_client = EasypanelClient::new(&target.base_url(), "fixture");
        let source = CopyTarget {
            client: &source_client,
            project: "source",
            service: "db",
        };
        let target = CopyTarget {
            client: &target_client,
            project: "target",
            service: "db",
        };
        let error = plan_copy(&source, &target, &["shop".into()], false, Some("shared"))
            .err()
            .expect("different buckets must refuse the copy");
        assert!(error.to_string().to_ascii_lowercase().contains("bucket"));
    }

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

    #[test]
    fn env_kind_contract_reads_inspection_shapes_without_coercing_unknowns() {
        for content in ["A=1\r\nB=\" space \"\n\n", "", "\n"] {
            for service in [json!({"env":content}), json!({"env":{"content":content}})] {
                assert_eq!(env_content(&service).unwrap(), content);
            }
        }
        for service in [json!({}), json!({"env":null}), json!({"env":{}})] {
            assert_eq!(env_content(&service).unwrap(), "");
        }
        for service in [
            json!(null),
            json!([]),
            json!({"env":false}),
            json!({"env":[]}),
            json!({"env":{"content":null}}),
            json!({"env":{"unknown":"private"}}),
            json!({"env":{"content":{"secret":"private"}}}),
        ] {
            let error = env_content(&service).unwrap_err().to_string();
            assert!(!error.contains("private"));
        }
    }

    #[test]
    fn env_kind_contract_save_and_readback_preserves_bytes_and_other_settings() {
        use std::io::{BufRead, Read, Write};
        use std::time::{Duration, Instant};
        for kind in [
            "app",
            "wordpress",
            "box",
            "compose",
            "mysql",
            "mariadb",
            "postgres",
            "mongo",
            "redis",
        ] {
            for content in ["A=1\r\nSECRET=\" space \"\n\n", ""] {
                let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
                listener.set_nonblocking(true).unwrap();
                let url = format!("http://{}", listener.local_addr().unwrap());
                let server = std::thread::spawn(move || {
                    let mut stored = json!({"env":"OLD=keep\n"});
                    let mut expected =
                        json!({"projectName":"p","serviceName":"site","env":content});
                    let operation = if crate::lifecycle::is_database(kind) {
                        stored["image"] = json!("engine:stable");
                        stored["command"] = json!("--keep-command");
                        expected["image"] = stored["image"].clone();
                        expected["command"] = stored["command"].clone();
                        if matches!(kind, "mysql" | "mariadb") {
                            stored["configFile"] = json!("[mysqld]\nmax_connections=200\n");
                            expected["configFile"] = stored["configFile"].clone();
                        }
                        "updateAdvanced"
                    } else {
                        if matches!(kind, "wordpress" | "box") {
                            stored["env"] = json!({"content":"OLD=keep\n"});
                            expected["env"] = json!({"content":content});
                        } else if kind == "app" {
                            stored["dotEnvPath"] = json!("config/.env");
                            expected["dotEnvPath"] = stored["dotEnvPath"].clone();
                        } else {
                            stored["createDotEnv"] = json!(false);
                            expected["createDotEnv"] = json!(false);
                        }
                        "updateEnv"
                    };
                    for action in ["inspectService", operation, "inspectService"] {
                        let deadline = Instant::now() + Duration::from_secs(5);
                        let mut stream = loop {
                            match listener.accept() {
                                Ok((stream, _)) => break stream,
                                Err(error)
                                    if error.kind() == std::io::ErrorKind::WouldBlock
                                        && Instant::now() < deadline =>
                                {
                                    std::thread::sleep(Duration::from_millis(5))
                                }
                                Err(error) => panic!("waiting for {kind}/{action}: {error}"),
                            }
                        };
                        stream
                            .set_read_timeout(Some(Duration::from_secs(3)))
                            .unwrap();
                        let mut reader = std::io::BufReader::new(&mut stream);
                        let mut line = String::new();
                        reader.read_line(&mut line).unwrap();
                        assert!(
                            line.starts_with(&format!("POST /api/rpc/services/{kind}/{action} "))
                        );
                        let mut length = 0;
                        loop {
                            line.clear();
                            assert!(reader.read_line(&mut line).unwrap() > 0);
                            if line == "\r\n" {
                                break;
                            }
                            if let Some((name, value)) = line.split_once(':') {
                                if name.eq_ignore_ascii_case("content-length") {
                                    length = value.trim().parse().unwrap();
                                }
                            }
                        }
                        let mut body = vec![0; length];
                        reader.read_exact(&mut body).unwrap();
                        drop(reader);
                        let request: Value = serde_json::from_slice(&body).unwrap();
                        let response = if action == operation {
                            assert_eq!(request["json"], expected);
                            stored["env"] = request["json"]["env"].clone();
                            Value::Null
                        } else {
                            stored.clone()
                        };
                        let body = json!({"json":response}).to_string();
                        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
                    }
                    stored
                });
                let client = EasypanelClient::new(&url, "fixture");
                save_env(&client, "p", "site", kind, content).unwrap();
                let readback = client
                    .call(
                        &format!("services/{kind}"),
                        "inspectService",
                        json!({"projectName":"p","serviceName":"site"}),
                    )
                    .unwrap();
                assert_eq!(env_content(&readback).unwrap(), content);
                assert_eq!(readback, server.join().unwrap());
            }
        }
    }

    #[test]
    fn env_kind_contract_keeps_server_refusals_as_errors() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.path("/api/rpc/services/wordpress/inspectService");
            then.status(200)
                .json_body(json!({"json":{"env":{"content":"OLD=1"}}}));
        });
        let refusal = server.mock(|when, then| {
            when.path("/api/rpc/services/wordpress/updateEnv")
                .json_body(
                    json!({"json":{"projectName":"p","serviceName":"site","env":{"content":""}}}),
                );
            then.status(400)
                .json_body(json!({"json":{"message":"Environment update refused"}}));
        });
        let client = EasypanelClient::new(&server.base_url(), "fixture");
        let error = save_env(&client, "p", "site", "wordpress", "").unwrap_err();
        assert!(error.to_string().contains("Environment update refused"));
        assert_eq!(
            error
                .downcast_ref::<crate::client::ApiError>()
                .unwrap()
                .status,
            400
        );
        refusal.assert();
    }

    #[test]
    fn env_kind_contract_sends_canonical_payloads_without_changing_content() {
        for kind in ["wordpress", "box", "compose"] {
            for content in ["A=1\r\nSECRET=\" space \"\n\n", ""] {
                let server = MockServer::start();
                let stored = if kind == "compose" {
                    json!(content)
                } else {
                    json!({"content": content})
                };
                server.mock(|when, then| {
                    when.path(format!("/api/rpc/services/{kind}/inspectService"));
                    then.status(200).json_body(json!({"json": {
                        "env": stored, "dotEnvPath": ".env", "createDotEnv": true
                    }}));
                });
                let mut expected = json!({
                    "projectName": "p", "serviceName": "site", "env": stored
                });
                if kind == "compose" {
                    expected["createDotEnv"] = json!(true);
                }
                let save = server.mock(|when, then| {
                    when.path(format!("/api/rpc/services/{kind}/updateEnv"))
                        .json_body(json!({"json": expected}));
                    then.status(200).json_body(json!({"json": null}));
                });
                let client = EasypanelClient::new(&server.base_url(), "fixture");
                save_env(&client, "p", "site", kind, content).unwrap();
                save.assert();
            }
        }
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
}
