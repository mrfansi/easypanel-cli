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
///
/// They are not dropped — see `defer_first_boot_blockers`, which comments them
/// out so the configuration survives the copy.
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

/// The config with the first-boot blockers COMMENTED OUT, and which they were.
///
/// The config is copied in full — losing a replica's tuning is not acceptable —
/// but the two directives that stop a brand-new database from initialising are
/// left as comments rather than active settings. They are one `#` away from
/// being back on, which is what the user does once the replica has been seeded;
/// dropping them would have thrown away configuration they asked to copy.
///
/// A commented line is left alone: it is already harmless.
pub fn defer_first_boot_blockers(config: &str) -> (String, Vec<&'static str>) {
    let deferred = first_boot_blockers(config);
    if deferred.is_empty() {
        return (config.to_string(), deferred);
    }
    let out = config
        .lines()
        .map(|line| {
            if line_blocks_first_boot(line) {
                // Kept verbatim after the marker, so re-enabling it is deleting
                // the prefix — not retyping the setting from memory.
                format!("# easypanel-cli: enable after the first boot — {line}")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    (out, deferred)
}

/// Does this ONE line turn a first-boot blocker on?
fn line_blocks_first_boot(line: &str) -> bool {
    let t = line.trim();
    if t.starts_with('#') || t.starts_with(';') {
        return false;
    }
    let Some((key, value)) = t.split_once('=') else {
        return false;
    };
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "on" | "1" | "true"
    ) && matches!(
        key.trim().to_ascii_lowercase().as_str(),
        "super_read_only" | "read_only"
    )
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

    anyhow::ensure!(
        inspect.is_object(),
        "couldn't copy service configuration: source inspection must be an object"
    );
    let mut create = service_body(&inspect, dst.project, dst.service);
    for key in ["command", "image", "env"] {
        if key != "env" && !crate::lifecycle::is_database(stype) {
            continue;
        }
        match create.get(key) {
            Some(Value::Null) => {
                create.as_object_mut().unwrap().remove(key);
            }
            Some(value) if key == "env" && matches!(stype, "box" | "wordpress") => {
                anyhow::ensure!(
                    value.is_object(),
                    "couldn't copy service configuration: source env must be an object"
                );
                anyhow::ensure!(
                    value.get("content").is_none_or(Value::is_string),
                    "couldn't copy service configuration: source env.content must be a string"
                );
            }
            Some(value) => anyhow::ensure!(
                value.is_string(),
                "couldn't copy service configuration: source {key} must be a string"
            ),
            None => {}
        }
    }

    let config = match inspect.get("configFile") {
        Some(Value::Null) | None => "",
        Some(value) => value
            .as_str()
            .context("couldn't copy config file: source configFile must be a string")?,
    };
    // Only MySQL/MariaDB expose configFile; box's Advanced block holds scripts.
    anyhow::ensure!(
        config.is_empty() || matches!(stype, "mysql" | "mariadb"),
        "couldn't copy service configuration: {stype} has no configFile API; \
         migrate this configuration through its supported source/settings first"
    );
    let (config, deferred) = defer_first_boot_blockers(config);
    let advanced = if config.is_empty() {
        None
    } else {
        let mut body = json!({
            "projectName": dst.project,
            "serviceName": dst.service,
            "configFile": config,
        });
        // Match the other Advanced writers: absent/null means the image's default.
        for key in ["command", "image", "env"] {
            match inspect.get(key).filter(|value| !value.is_null()) {
                Some(value) => body[key] = value.clone(),
                None if key != "env" => body[key] = json!(""),
                None => {}
            }
        }
        Some(body)
    };

    let source = inspect.get("source").filter(|value| !value.is_null());
    let source_call = if let Some(source) = source {
        let (op, mut body) = if stype == "compose" {
            let (op, keys): (&str, &[&str]) = match source["type"].as_str() {
                Some("inline") => ("updateSourceInline", &["content"]),
                Some("git") => (
                    "updateSourceGit",
                    &["repo", "ref", "rootPath", "composeFile"],
                ),
                _ => {
                    anyhow::bail!("couldn't copy compose source: expected an inline or git source")
                }
            };
            let mut body = json!({});
            for key in keys {
                let value = source.get(*key).and_then(Value::as_str).with_context(|| {
                    format!("couldn't copy compose source: source {key} must be a string")
                })?;
                anyhow::ensure!(
                    match *key {
                        "rootPath" => value.starts_with('/'),
                        "composeFile" => true,
                        _ => !value.is_empty(),
                    },
                    "couldn't copy compose source: source {key} is invalid"
                );
                body[*key] = json!(value);
            }
            (op, body)
        } else {
            anyhow::ensure!(
                stype == "app",
                "couldn't copy service configuration: {stype} has no source API"
            );
            crate::source::source_call(source).context(
                "couldn't copy app source: no configuration-only API for this source; \
                 upload sources require the original files to be uploaded separately",
            )?
        };
        body["projectName"] = json!(dst.project);
        body["serviceName"] = json!(dst.service);
        Some((op, body))
    } else {
        None
    };

    // 1) The service itself, config inline EXCEPT the source.
    dst.client
        .call(&grp, "createService", create)
        .with_context(|| format!("couldn't create '{}/{}'", dst.project, dst.service))?;

    // 2) The source, separately — inline it would trigger a deploy.
    if let Some((op, body)) = source_call {
        dst.client
            .call(&grp, op, body)
            .context("the service was created, but its source failed to apply")?;
        // updateSourceGithub resets auto-deploy, so it goes back on after.
        if source.and_then(|s| s.get("type")).and_then(Value::as_str) == Some("github")
            && source
                .and_then(|s| s.get("autoDeploy"))
                .and_then(Value::as_bool)
                .unwrap_or(false)
        {
            let ps = json!({ "projectName": dst.project, "serviceName": dst.service });
            if dst.client.call(&grp, "enableGithubDeploy", ps).is_err() {
                notes.push("auto-deploy could not be re-enabled".into());
            }
        }
    }

    // 3) Copy configFile, commenting out only first-boot blockers.
    if !deferred.is_empty() {
        notes.push(format!(
            "⚠ config file copied, but {} left COMMENTED OUT — active, they stop a \
             brand-new database initialising (no root password, no user, no database, \
             and unrepairable afterwards). Deploy it, let it come up, then delete the \
             '# easypanel-cli:' prefix on those lines and restart",
            deferred.join(" + ")
        ));
    }
    if let Some(adv) = advanced {
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
    fn config_clone_preserves_wordpress_and_box_object_environments() {
        use httpmock::prelude::*;

        for kind in ["wordpress", "box"] {
            let server = MockServer::start();
            let inspect = server.mock(|when, then| {
                when.method(POST)
                    .path(format!("/api/rpc/services/{kind}/inspectService"));
                then.status(200).json_body(json!({"json":{
                    "name":"web", "projectName":"origin", "type":kind,
                    "env":{"content":"SITE_NAME=Original\nSECRET=keep\n"},
                    "nginx":{"rootDocument":"/code/public","config":"server { listen 80; }"},
                    "git":{"url":"https://example.test/site.git","branch":"production"}
                }}));
            });
            let create = server.mock(|when, then| {
                when.method(POST)
                    .path(format!("/api/rpc/services/{kind}/createService"))
                    .json_body(json!({"json":{
                        "projectName":"target", "serviceName":"clone",
                        "env":{"content":"SITE_NAME=Original\nSECRET=keep\n"},
                        "nginx":{"rootDocument":"/code/public","config":"server { listen 80; }"},
                        "git":{"url":"https://example.test/site.git","branch":"production"}
                    }}));
                then.status(200).json_body(json!({"json":null}));
            });
            let other = server.mock(|when, then| {
                when.matches(|request| {
                    !request.path.ends_with("/inspectService")
                        && !request.path.ends_with("/createService")
                });
                then.status(400);
            });
            let client = EasypanelClient::new(&server.base_url(), "test-only");
            let notes = migrate_service(
                &client,
                "origin",
                "web",
                kind,
                &Target {
                    client: &client,
                    project: "target",
                    service: "clone",
                },
                false,
            )
            .unwrap();
            assert!(notes.is_empty());
            inspect.assert_hits(1);
            create.assert_hits(1);
            other.assert_hits(0);
        }
    }

    #[test]
    fn config_clone_applies_compose_inline_and_git_sources_without_advanced_or_deploy() {
        use httpmock::prelude::*;

        // These are create/source request contracts, not a typed inspect response.
        for (source, operation, source_body) in [
            (
                json!({"type":"inline","content":"services:\n  web:\n    image: nginx:alpine\n"}),
                "updateSourceInline",
                json!({"content":"services:\n  web:\n    image: nginx:alpine\n"}),
            ),
            (
                json!({"type":"git","repo":"https://example.test/stack.git","ref":"release",
                    "rootPath":"/infra","composeFile":"production.yaml"}),
                "updateSourceGit",
                json!({"repo":"https://example.test/stack.git","ref":"release",
                    "rootPath":"/infra","composeFile":"production.yaml"}),
            ),
        ] {
            let server = MockServer::start();
            let inspect = server.mock(|when, then| {
                when.method(POST)
                    .path("/api/rpc/services/compose/inspectService");
                then.status(200).json_body(json!({"json":{
                    "name":"stack","projectName":"origin","type":"compose",
                    "env":"IMAGE_TAG=stable\n", "createDotEnv":true, "source":source
                }}));
            });
            let create = server.mock(|when, then| {
                when.method(POST)
                    .path("/api/rpc/services/compose/createService")
                    .json_body(json!({"json":{
                        "projectName":"target","serviceName":"clone",
                        "env":"IMAGE_TAG=stable\n","createDotEnv":true
                    }}));
                then.status(200).json_body(json!({"json":null}));
            });
            let mut expected_source = source_body;
            expected_source["projectName"] = json!("target");
            expected_source["serviceName"] = json!("clone");
            let update = server.mock(|when, then| {
                when.method(POST)
                    .path(format!("/api/rpc/services/compose/{operation}"))
                    .json_body(json!({"json":expected_source}));
                then.status(200).json_body(json!({"json":null}));
            });
            let other = server.mock(|when, then| {
                when.matches(|request| {
                    !matches!(
                        request.path.as_str(),
                        "/api/rpc/services/compose/inspectService"
                            | "/api/rpc/services/compose/createService"
                            | "/api/rpc/services/compose/updateSourceInline"
                            | "/api/rpc/services/compose/updateSourceGit"
                    )
                });
                then.status(400);
            });
            let client = EasypanelClient::new(&server.base_url(), "test-only");
            assert!(migrate_service(
                &client,
                "origin",
                "stack",
                "compose",
                &Target {
                    client: &client,
                    project: "target",
                    service: "clone"
                },
                false,
            )
            .unwrap()
            .is_empty());
            inspect.assert_hits(1);
            create.assert_hits(1);
            update.assert_hits(1);
            other.assert_hits(0);
        }
    }

    #[test]
    fn config_clone_rejects_malformed_kind_fields_and_unavailable_configuration_before_create() {
        use httpmock::prelude::*;

        for (kind, source, message) in [
            (
                "wordpress",
                json!({"env":"GENERIC_FIXTURE=invalid"}),
                "source env must be an object",
            ),
            (
                "box",
                json!({"env":{"content":false}}),
                "source env.content must be a string",
            ),
            ("compose", json!({"env":{}}), "source env must be a string"),
            (
                "compose",
                json!({"source":{"type":"inline","content":{}}}),
                "source content must be a string",
            ),
            (
                "compose",
                json!({"source":{"type":"git","repo":"https://example.test/stack.git",
                "ref":"main","composeFile":"compose.yaml"}}),
                "source rootPath must be a string",
            ),
            (
                "compose",
                json!({"configFile":"# generic fixture, not a compose field\n"}),
                "compose has no configFile API",
            ),
            (
                "app",
                json!({"source":{"type":"upload"}}),
                "upload sources require the original files",
            ),
        ] {
            let server = MockServer::start();
            let inspect = server.mock(|when, then| {
                when.path(format!("/api/rpc/services/{kind}/inspectService"));
                then.status(200).json_body(json!({"json":source}));
            });
            let mutation = server.mock(|when, then| {
                when.matches(|request| !request.path.ends_with("/inspectService"));
                then.status(200).json_body(json!({"json":null}));
            });
            let client = EasypanelClient::new(&server.base_url(), "test-only");
            let error = migrate_service(
                &client,
                "origin",
                "service",
                kind,
                &Target {
                    client: &client,
                    project: "target",
                    service: "clone",
                },
                false,
            )
            .unwrap_err();
            assert!(format!("{error:#}").contains(message), "{error:#}");
            inspect.assert_hits(1);
            mutation.assert_hits(0);
        }
    }

    #[test]
    fn project_migration_reports_failed_compose_source_and_continues_to_database_without_command() {
        use crate::worker::{handle_req, Emit, Req, Resp};
        use httpmock::prelude::*;

        let server = MockServer::start();
        let projects = server.mock(|when, then| {
            when.path("/api/rpc/projects/listProjects");
            then.status(200)
                .json_body(json!({"json":[{"name":"target"}]}));
        });
        for (kind, source) in [
            ("wordpress", json!({"env":{"content":"WP_DEBUG=false"}})),
            (
                "compose",
                json!({"env":"TAG=stable",
                "source":{"type":"inline","content":"services:\n  web:\n    image: nginx\n"}}),
            ),
            (
                "mariadb",
                json!({"image":"mariadb:11","configFile":"[mysqld]\nmax_connections=321"}),
            ),
        ] {
            server.mock(|when, then| {
                when.path(format!("/api/rpc/services/{kind}/inspectService"));
                then.status(200).json_body(json!({"json":source}));
            });
        }
        let wordpress = server.mock(|when, then| {
            when.path("/api/rpc/services/wordpress/createService")
                .json_body(json!({"json":{"projectName":"target","serviceName":"site",
                    "env":{"content":"WP_DEBUG=false"}}}));
            then.status(200).json_body(json!({"json":null}));
        });
        let compose = server.mock(|when, then| {
            when.path("/api/rpc/services/compose/createService")
                .json_body(json!({"json":{"projectName":"target","serviceName":"stack","env":"TAG=stable"}}));
            then.status(200).json_body(json!({"json":null}));
        });
        let refused_source = server.mock(|when, then| {
            when.path("/api/rpc/services/compose/updateSourceInline")
                .json_body(json!({"json":{"projectName":"target","serviceName":"stack",
                    "content":"services:\n  web:\n    image: nginx\n"}}));
            then.status(503)
                .json_body(json!({"message":"source storage unavailable"}));
        });
        let database = server.mock(|when, then| {
            when.path("/api/rpc/services/mariadb/createService")
                .json_body(json!({"json":{"projectName":"target","serviceName":"db","image":"mariadb:11"}}));
            then.status(200).json_body(json!({"json":null}));
        });
        let advanced = server.mock(|when, then| {
            when.path("/api/rpc/services/mariadb/updateAdvanced")
                .json_body(json!({"json":{"projectName":"target","serviceName":"db",
                    "command":"","image":"mariadb:11","configFile":"[mysqld]\nmax_connections=321"}}));
            then.status(200).json_body(json!({"json":null}));
        });
        let domains = server.mock(|when, then| {
            when.path("/api/rpc/domains/listDomains");
            then.status(200).json_body(json!({"json":[]}));
        });
        let prohibited = server.mock(|when, then| {
            when.matches(|request| {
                request.path.contains("deploy")
                    || request.path.contains("destroy")
                    || request.path == "/api/rpc/services/compose/updateAdvanced"
            });
            then.status(400);
        });
        let client = EasypanelClient::new(&server.base_url(), "test-only");
        let emit: Emit = std::sync::Arc::new(|_| {});
        let result = handle_req(
            &client,
            Req::Migrate {
                target: client.clone(),
                target_name: "destination".into(),
                target_project: "target".into(),
                services: vec![
                    ("origin".into(), "site".into(), "wordpress".into()),
                    ("origin".into(), "stack".into(), "compose".into()),
                    ("origin".into(), "db".into(), "mariadb".into()),
                ],
            },
            &emit,
        );
        let message = match result {
            Resp::Done(message, _) => message,
            _ => panic!("a partial project migration must retain its per-service outcome"),
        };
        assert!(message.contains("Migrated 2/3"), "{message}");
        assert!(message.contains("config only, NO data"), "{message}");
        assert!(
            message
                .contains("failed: stack: the service was created, but its source failed to apply"),
            "{message}"
        );
        projects.assert_hits(1);
        wordpress.assert_hits(1);
        compose.assert_hits(1);
        refused_source.assert_hits(1);
        database.assert_hits(1);
        advanced.assert_hits(1);
        domains.assert_hits(2);
        prohibited.assert_hits(0);
    }

    #[test]
    fn config_clone_rejects_malformed_advanced_fields_before_creation() {
        use httpmock::prelude::*;

        for config in [None, Some(""), Some("# fixture config\n")] {
            for key in ["configFile", "command", "image", "env"] {
                let server = MockServer::start();
                let mut source = json!({
                    "image": "mariadb:11", "command": "",
                });
                if let Some(config) = config {
                    source["configFile"] = json!(config);
                }
                source[key] = json!({"unexpected": "object"});
                let inspect = server.mock(|when, then| {
                    when.path("/api/rpc/services/mariadb/inspectService");
                    then.status(200).json_body(json!({"json": source}));
                });
                let mutation = server.mock(|when, then| {
                    when.matches(|request| {
                        request.path != "/api/rpc/services/mariadb/inspectService"
                    });
                    then.status(200).json_body(json!({"json": null}));
                });
                let client = EasypanelClient::new(&server.base_url(), "test-only");
                let error = migrate_service(
                    &client,
                    "origin",
                    "db",
                    "mariadb",
                    &Target {
                        client: &client,
                        project: "target",
                        service: "clone",
                    },
                    false,
                )
                .unwrap_err();
                assert!(error
                    .to_string()
                    .contains(&format!("source {key} must be a string")));
                inspect.assert_hits(1);
                mutation.assert_hits(0);
            }
        }
    }

    #[test]
    fn config_clone_preserves_destination_defaults_and_explicit_commands() {
        use std::io::{BufRead, Read, Write};
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        };
        use std::time::{Duration, Instant};

        for config in [
            None,
            Some(Value::Null),
            Some(json!("")),
            Some(json!("# fixture config\n")),
        ] {
            for command in [
                None,
                Some(Value::Null),
                Some(json!("")),
                Some(json!("mariadbd --max-connections=321")),
            ] {
                let mut source = json!({
                    "projectName": "origin", "name": "db", "type": "mariadb",
                    "image": "mariadb:11",
                });
                if let Some(config) = &config {
                    source["configFile"] = config.clone();
                }
                if let Some(command) = &command {
                    source["command"] = command.clone();
                    source["env"] = if command.is_null() {
                        Value::Null
                    } else {
                        json!("SOURCE=1")
                    };
                }
                let original = source.clone();
                let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
                listener.set_nonblocking(true).unwrap();
                let url = format!("http://{}", listener.local_addr().unwrap());
                let done = Arc::new(AtomicBool::new(false));
                let stopped = done.clone();
                let server = std::thread::spawn(move || {
                    let mut destination = Value::Null;
                    let mut calls = Vec::new();
                    let deadline = Instant::now() + Duration::from_secs(10);
                    while !stopped.load(Ordering::SeqCst) {
                        let (mut stream, _) = match listener.accept() {
                            Ok(connection) => connection,
                            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                                assert!(Instant::now() < deadline, "clone fixture timed out");
                                std::thread::sleep(Duration::from_millis(5));
                                continue;
                            }
                            Err(error) => panic!("{error}"),
                        };
                        stream
                            .set_read_timeout(Some(Duration::from_secs(3)))
                            .unwrap();
                        let mut reader = std::io::BufReader::new(&mut stream);
                        let mut line = String::new();
                        reader.read_line(&mut line).unwrap();
                        let path = line.split_whitespace().nth(1).unwrap().to_string();
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
                        let mut bytes = vec![0; length];
                        reader.read_exact(&mut bytes).unwrap();
                        drop(reader);
                        let body = serde_json::from_slice::<Value>(&bytes).unwrap()["json"].clone();
                        calls.push(path.clone());
                        let operation = path.strip_prefix("/api/rpc/services/mariadb/");
                        let target =
                            body["projectName"] == "target" && body["serviceName"] == "clone";
                        // Canonical create fields are optional strings; Advanced requires command/image.
                        let strings = ["command", "image", "env", "configFile"]
                            .iter()
                            .all(|key| body.get(*key).is_none_or(Value::is_string));
                        let (status, response) = match operation {
                            Some("inspectService")
                                if body
                                    == json!({
                                        "projectName": "origin", "serviceName": "db"
                                    }) =>
                            {
                                (200, source.clone())
                            }
                            Some("createService") if target && strings && destination.is_null() => {
                                destination = json!({
                                    "projectName": "target", "serviceName": "clone",
                                    "command": "", "image": "mariadb:11",
                                    "env": "DESTINATION_DEFAULT=1", "configFile": "",
                                });
                                destination
                                    .as_object_mut()
                                    .unwrap()
                                    .extend(body.as_object().unwrap().clone());
                                (200, Value::Null)
                            }
                            Some("updateAdvanced")
                                if target
                                    && strings
                                    && body.get("command").is_some_and(Value::is_string)
                                    && body.get("image").is_some_and(Value::is_string)
                                    && !destination.is_null() =>
                            {
                                destination
                                    .as_object_mut()
                                    .unwrap()
                                    .extend(body.as_object().unwrap().clone());
                                (200, Value::Null)
                            }
                            _ => (
                                400,
                                json!({"message": "invalid canonical request or unexpected operation"}),
                            ),
                        };
                        let response = json!({"json": response}).to_string();
                        write!(stream, "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}", response.len()).unwrap();
                    }
                    (source, destination, calls)
                });
                let client = EasypanelClient::new(&url, "test-only");
                let result = migrate_service(
                    &client,
                    "origin",
                    "db",
                    "mariadb",
                    &Target {
                        client: &client,
                        project: "target",
                        service: "clone",
                    },
                    false,
                );
                done.store(true, Ordering::SeqCst);
                let (source, destination, calls) = server.join().unwrap();
                assert!(result.unwrap().is_empty());
                assert_eq!(source, original, "the source must remain untouched");
                let copied_config = config.as_ref().and_then(Value::as_str).unwrap_or("");
                assert_eq!(destination["configFile"], copied_config);
                assert_eq!(
                    destination["command"],
                    command.as_ref().and_then(Value::as_str).unwrap_or("")
                );
                assert_eq!(destination["image"], "mariadb:11");
                assert_eq!(
                    destination["env"],
                    if command.as_ref().is_some_and(Value::is_string) {
                        "SOURCE=1"
                    } else {
                        "DESTINATION_DEFAULT=1"
                    }
                );
                let mut expected = vec!["inspectService", "createService"];
                if !copied_config.is_empty() {
                    expected.push("updateAdvanced");
                }
                assert_eq!(
                    calls,
                    expected
                        .iter()
                        .map(|op| format!("/api/rpc/services/mariadb/{op}"))
                        .collect::<Vec<_>>(),
                    "no source, domain, deploy or data operations are allowed"
                );
            }
        }
    }

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
    fn a_replica_config_is_copied_with_its_blockers_commented_not_dropped() {
        // The owner's point: the config file must be COPIED. It is — every line
        // survives, and the two that would stop a fresh database initialising
        // are one `#` away from being back on.
        let replica = "[mysqld]\n\
             server-id                  = 2\n\
             relay_log                  = relay-bin\n\
             read_only                  = ON\n\
             super_read_only            = ON\n\
             max_connections            = 500\n";
        let (out, deferred) = defer_first_boot_blockers(replica);
        assert_eq!(deferred, vec!["read_only", "super_read_only"]);

        // Nothing was thrown away: the settings are still there, verbatim.
        assert!(out.contains("read_only                  = ON"));
        assert!(out.contains("super_read_only            = ON"));
        assert!(out.contains("server-id                  = 2"));
        assert!(out.contains("max_connections            = 500"));
        assert!(out.contains("relay_log"));

        // …but neither is ACTIVE any more, which is the whole point.
        assert!(
            first_boot_blockers(&out).is_empty(),
            "the copy must be able to boot:\n{out}"
        );
        // Two lines commented, and only those two.
        assert_eq!(
            out.lines().filter(|l| l.contains("easypanel-cli:")).count(),
            2
        );
        assert_eq!(out.lines().count(), replica.lines().count());
    }

    #[test]
    fn a_config_with_nothing_to_defer_is_passed_through_untouched() {
        let primary = "[mysqld]\nserver-id = 1\nlog_bin = mysql-bin\n";
        let (out, deferred) = defer_first_boot_blockers(primary);
        assert!(deferred.is_empty());
        assert_eq!(out, primary, "not even a reflow");
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
