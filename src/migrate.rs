//! Copying a service's CONFIGURATION from one place to another — a different
//! project, or a different EasyPanel host entirely.
//!
//! EasyPanel has no migrate (and no clone) endpoint, so this is a composition of
//! the endpoints it does have:
//!
//!   inspectService → createService (minus the source) → updateSource*
//!                  → updateAdvanced (db configFile) → createDomain (optional)
//!
//! The source is applied SEPARATELY rather than inline, because an inline source
//! makes createService block on a ~100-second deploy.
//!
//! # What this does NOT move
//!
//! Data. Volume contents and database rows live on the origin host's disk and are
//! not exposed by the API at all, so nothing here can reach them. Callers must say
//! so plainly rather than letting a user assume their database came along.
//!
//! Cloning inside one host is the same operation with the same client on both
//! sides — it lives here so the copy rule has exactly one definition. It used to
//! have two, and they had already drifted apart (the registry credentials were
//! lost on one path but not the other).

use crate::client::EasypanelClient;
use anyhow::{Context, Result};
use serde_json::{json, Value};

/// Config directives that stop a database from INITIALISING itself on first boot.
///
/// A MySQL/MariaDB container with an empty data directory must WRITE before it can
/// serve anything: the entrypoint creates the root user, sets its password, and
/// creates the database. `super_read_only` (and `read_only`) refuse exactly those
/// writes. Verified against a live panel by cloning a replica: the entrypoint died
/// with
///
/// ```text
/// ERROR 1290 (HY000) at line 1: The MySQL server is running with the
/// --super-read-only option so it cannot execute this statement
/// ```
///
/// and the clone was left with no root password, no user and no database — while
/// the panel still displayed the credentials it believed it had set.
///
/// The damage does not stop there: a database only initialises ONCE, when the
/// directory is empty. The failed boot leaves it non-empty, so fixing the config
/// afterwards does NOT repair the service — it has to be destroyed and remade.
/// That is why this is caught BEFORE the config is ever applied.
///
/// A replica's read-only flags are meaningless until the data is seeded anyway, so
/// the honest order is: create → deploy (initialise) → apply the config → deploy.
///
/// Commented-out lines don't count; `ON`/`1`/`TRUE` all do.
pub fn first_boot_blockers(config: &str) -> Vec<&'static str> {
    let mut found = Vec::new();
    for line in config.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let on = matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "on" | "1" | "true"
        );
        if !on {
            continue;
        }
        match key.trim().to_ascii_lowercase().as_str() {
            "super_read_only" => found.push("super_read_only"),
            "read_only" => found.push("read_only"),
            _ => {}
        }
    }
    found
}

/// Fields that identify a service to its ORIGIN and must never be carried over:
/// the target mints its own. `source` and `configFile` are excluded because they
/// are applied in their own later steps, not inline.
const BLOCK: &[&str] = &[
    "name",
    "serviceName",
    "projectName",
    "type",
    "enabled",
    "token",
    "primaryDomainId",
    "deploymentUrl",
    "commit",
    "dbGateDomain",
    "phpMyAdminDomain",
    "source",
    "configFile",
];

/// Where a service is going: which host, which project, under which name.
pub struct Target<'a> {
    pub client: &'a EasypanelClient,
    pub project: &'a str,
    pub service: &'a str,
}

/// The createService body: the inspected config minus the origin-owned fields,
/// pointed at the target.
pub fn service_body(inspect: &Value, project: &str, name: &str) -> Value {
    let mut body = inspect.clone();
    if let Some(obj) = body.as_object_mut() {
        for k in BLOCK {
            obj.remove(*k);
        }
        obj.insert("projectName".into(), json!(project));
        obj.insert("serviceName".into(), json!(name));
    }
    body
}

/// The domains pointing at `project/service`, ready to be recreated elsewhere.
///
/// `listDomains` returns every domain on the host, so the caller's service has to
/// be picked out of it. `id` is dropped because the target mints its own — and
/// `createDomain` requires the field to be present, so a placeholder goes in.
pub fn domains_for(all: &Value, project: &str, service: &str) -> Vec<Value> {
    all.as_array()
        .map(|ds| {
            ds.iter()
                .filter(|d| {
                    let dest = d.get("serviceDestination");
                    let p = dest
                        .and_then(|x| x.get("projectName"))
                        .and_then(Value::as_str);
                    let s = dest
                        .and_then(|x| x.get("serviceName"))
                        .and_then(Value::as_str);
                    p == Some(project) && s == Some(service)
                })
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

/// Repoint a domain at the migrated service. The host stays as it was: DNS is the
/// user's to move, and silently rewriting a hostname would be worse than leaving
/// it obviously pointing where the user put it.
fn retarget_domain(domain: &Value, project: &str, service: &str) -> Value {
    let mut d = domain.clone();
    if let Some(obj) = d.as_object_mut() {
        // createDomain validates `id` as present but ignores the value.
        obj.insert("id".into(), json!("new"));
        // Required by the schema; absent from some older domain records.
        obj.entry("middlewares").or_insert_with(|| json!([]));
    }
    if let Some(dest) = d
        .get_mut("serviceDestination")
        .and_then(Value::as_object_mut)
    {
        dest.insert("projectName".into(), json!(project));
        dest.insert("serviceName".into(), json!(service));
        // Required by the schema; older records predate it.
        dest.entry("protocol").or_insert_with(|| json!("http"));
    }
    d
}

/// Create the project on the target host unless it is already there.
///
/// Migrating to a fresh host almost always means the project doesn't exist yet,
/// and createProject on an existing name is an error rather than a no-op — so the
/// list is checked first instead of swallowing the failure, which would also
/// swallow real ones (a bad token, an unreachable host).
pub fn ensure_project(client: &EasypanelClient, name: &str) -> Result<bool> {
    let existing = client
        .call("projects", "listProjects", Value::Null)
        .context("couldn't list projects on the target")?;
    let present = existing.as_array().is_some_and(|ps| {
        ps.iter()
            .any(|p| p.get("name").and_then(Value::as_str) == Some(name))
    });
    if present {
        return Ok(false);
    }
    client
        .call("projects", "createProject", json!({ "name": name }))
        .with_context(|| format!("couldn't create project '{name}' on the target"))?;
    Ok(true)
}

/// Copy one service's config to `dst`, optionally bringing its domains.
///
/// Returns the notes worth telling the user about — steps that were skipped or
/// that failed WITHOUT invalidating the service itself (a domain that collides,
/// say). A failure that does invalidate it is an `Err`: a half-created service the
/// user believes is complete is worse than an obvious failure.
pub fn migrate_service(
    src: &EasypanelClient,
    src_project: &str,
    src_service: &str,
    stype: &str,
    dst: &Target,
    copy_domains: bool,
) -> Result<Vec<String>> {
    let grp = format!("services/{stype}");
    let ident = json!({ "projectName": src_project, "serviceName": src_service });
    let mut notes = Vec::new();

    let inspect = src
        .call(&grp, "inspectService", ident.clone())
        .with_context(|| format!("couldn't read '{src_project}/{src_service}'"))?;

    // 1) The service itself, config inline EXCEPT the source.
    dst.client
        .call(
            &grp,
            "createService",
            service_body(&inspect, dst.project, dst.service),
        )
        .with_context(|| format!("couldn't create '{}/{}'", dst.project, dst.service))?;

    // 2) The source, separately — inline it would trigger a deploy.
    if let Some(src_def) = inspect.get("source").filter(|s| s.get("type").is_some()) {
        if let Some((op, mut body)) = crate::source::source_call(src_def) {
            body["projectName"] = json!(dst.project);
            body["serviceName"] = json!(dst.service);
            dst.client
                .call(&grp, op, body)
                .context("the service was created, but its source failed to apply")?;
            // updateSourceGithub resets auto-deploy, so it goes back on after.
            if src_def.get("type").and_then(Value::as_str) == Some("github")
                && src_def
                    .get("autoDeploy")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            {
                let ps = json!({ "projectName": dst.project, "serviceName": dst.service });
                if dst.client.call(&grp, "enableGithubDeploy", ps).is_err() {
                    notes.push("auto-deploy could not be re-enabled".into());
                }
            }
        }
    }

    // 3) configFile (databases): carries the advanced config, e.g. MySQL
    //    replication. updateAdvanced rejects null image/command, hence the
    //    defaults rather than passing them straight through.
    //    A config that would stop the new database from initialising is HELD
    //    BACK rather than copied: applying it here guarantees a service that can
    //    never boot, and the failure is unrepairable afterwards (see
    //    first_boot_blockers). The user is told, and applies it once the database
    //    has come up — which is also when a replica's read-only flags start to
    //    mean anything.
    let config = inspect
        .get("configFile")
        .and_then(Value::as_str)
        .unwrap_or("");
    let blockers = first_boot_blockers(config);
    if !config.is_empty() && !blockers.is_empty() {
        notes.push(format!(
            "⚠ config file NOT applied — {} would stop the new database initialising \
             (no root password, no user, no database, and unrepairable afterwards). \
             Deploy it first so the database initialises, then copy the config from \
             '{src_service}' — it takes effect the next time the service restarts",
            blockers.join(" + ")
        ));
    } else if !config.is_empty() {
        let adv = json!({
            "projectName": dst.project,
            "serviceName": dst.service,
            "command": inspect.get("command").cloned().unwrap_or(Value::Null),
            "configFile": inspect.get("configFile").cloned().unwrap_or(Value::Null),
            "env": inspect.get("env").cloned().unwrap_or(Value::Null),
            "image": inspect.get("image").cloned().unwrap_or(Value::Null),
        });
        dst.client
            .call(&grp, "updateAdvanced", adv)
            .context("the service was created, but its config file failed to apply")?;
    }

    // 4) Domains. Non-fatal: a domain that already exists on the target must not
    //    discard a service that is otherwise fully migrated.
    if copy_domains {
        match src.call("domains", "listDomains", json!({})) {
            Ok(all) => {
                for d in domains_for(&all, src_project, src_service) {
                    let host = d
                        .get("host")
                        .and_then(Value::as_str)
                        .unwrap_or("?")
                        .to_string();
                    let body = retarget_domain(&d, dst.project, dst.service);
                    if let Err(e) = dst.client.call("domains", "createDomain", body) {
                        notes.push(format!("domain {host}: {e}"));
                    }
                }
            }
            Err(e) => notes.push(format!("domains could not be read: {e}")),
        }
    }

    Ok(notes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_body_drops_origin_identity_and_repoints() {
        let inspect = json!({
            "token": "t", "primaryDomainId": "d1", "enabled": true,
            "name": "web", "projectName": "old", "type": "app",
            "env": "FOO=bar", "mounts": [{"type": "volume"}],
            "source": {"type": "image", "image": "nginx"},
        });
        let body = service_body(&inspect, "new-proj", "web");
        for k in ["token", "primaryDomainId", "enabled", "type", "source"] {
            assert!(body.get(k).is_none(), "{k} must not be carried over");
        }
        assert_eq!(body["projectName"], json!("new-proj"));
        assert_eq!(body["serviceName"], json!("web"));
        // The config the user cares about survives.
        assert_eq!(body["env"], json!("FOO=bar"));
        assert_eq!(body["mounts"], json!([{"type": "volume"}]));
    }

    #[test]
    fn domains_for_picks_only_the_named_service() {
        let all = json!([
            {"host": "a.com", "serviceDestination": {"projectName": "p", "serviceName": "web"}},
            {"host": "b.com", "serviceDestination": {"projectName": "p", "serviceName": "api"}},
            {"host": "c.com", "serviceDestination": {"projectName": "q", "serviceName": "web"}},
            {"host": "d.com"},
        ]);
        let got = domains_for(&all, "p", "web");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0]["host"], json!("a.com"));
    }

    #[test]
    fn retarget_domain_repoints_and_fills_required_fields() {
        // An older record: no middlewares, no protocol — both required by
        // createDomain, which is what made the first live attempt fail.
        let d = json!({
            "id": "old-cuid", "host": "app.example.com", "path": "/",
            "serviceDestination": {"projectName": "old", "serviceName": "web", "port": 80},
        });
        let out = retarget_domain(&d, "new", "web2");
        assert_eq!(out["id"], json!("new"));
        assert_eq!(out["middlewares"], json!([]));
        assert_eq!(out["serviceDestination"]["projectName"], json!("new"));
        assert_eq!(out["serviceDestination"]["serviceName"], json!("web2"));
        assert_eq!(out["serviceDestination"]["protocol"], json!("http"));
        // The port the user configured is preserved, and so is the hostname:
        // DNS is theirs to move.
        assert_eq!(out["serviceDestination"]["port"], json!(80));
        assert_eq!(out["host"], json!("app.example.com"));
    }

    #[test]
    fn a_replica_config_is_recognised_as_blocking_first_boot() {
        // The real config from a live MySQL replica. Cloning it produced
        // "ERROR 1290 … --super-read-only option so it cannot execute this
        // statement" and left a database with no root password, no user and no
        // schema — while the panel still showed the credentials it thought it set.
        let replica = "[mysqld]\n\
             server-id                  = 2\n\
             relay_log                  = relay-bin\n\
             read_only                  = ON\n\
             super_read_only            = ON\n";
        assert_eq!(
            first_boot_blockers(replica),
            vec!["read_only", "super_read_only"]
        );
    }

    #[test]
    fn an_ordinary_config_is_left_alone() {
        // A primary's config, and the same directives turned OFF or commented
        // out: none of these stop a database initialising, so none may be
        // treated as a reason to hold the config back.
        let primary = "[mysqld]\n\
             server-id       = 1\n\
             log_bin         = mysql-bin\n\
             max_connections = 500\n\
             # super_read_only = ON\n\
             ; read_only = ON\n\
             read_only       = OFF\n";
        assert!(first_boot_blockers(primary).is_empty());
        assert!(first_boot_blockers("").is_empty());
        // ON/1/true are the same thing, whatever the case.
        assert_eq!(first_boot_blockers("read_only=1"), vec!["read_only"]);
        assert_eq!(
            first_boot_blockers("SUPER_READ_ONLY = True"),
            vec!["super_read_only"]
        );
    }

    #[test]
    fn retarget_domain_keeps_an_existing_protocol() {
        let d = json!({
            "host": "a.com", "middlewares": [{"name": "auth"}],
            "serviceDestination": {"projectName": "p", "serviceName": "s", "protocol": "https"},
        });
        let out = retarget_domain(&d, "q", "s");
        assert_eq!(out["serviceDestination"]["protocol"], json!("https"));
        assert_eq!(out["middlewares"], json!([{"name": "auth"}]));
    }
}
