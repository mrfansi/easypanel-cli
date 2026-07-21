//! Comparing two services, field by field.
//!
//! "Staging works and production doesn't — what is actually different?" is a
//! question an operator asks constantly and today answers by opening two screens
//! and reading them side by side. The tool already fetches everything needed to
//! answer it: `inspectService` is what clone and migrate read. This turns that
//! into a diff.
//!
//! Two rules the answer has to respect, or it is worse than no answer:
//!
//! - **Environment is compared by KEY, never by value.** An env blob is the
//!   densest collection of secrets a service has — connection strings, API keys,
//!   signing secrets. Printing "DATABASE_URL: postgres://user:pw@… → …" to
//!   answer "are they the same?" would leak exactly what v0.66.0 and v0.67.0
//!   worked to contain. So env reports which keys differ, appear on one side, or
//!   agree — the value never leaves this module.
//! - **Order-independence.** Env lines and the domain list arrive in whatever
//!   order the API happened to return them; a diff that called two identical
//!   configs different because a line moved would train the reader to ignore it.
//!
//! No I/O here: the caller fetches both services, this decides what differs.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::output::field;

/// One way two services differ.
#[derive(Debug, Clone, PartialEq)]
pub struct Difference {
    /// What is being compared, in words: "deploy.replicas", "env DATABASE_URL".
    pub what: String,
    /// How it reads on the left service, or `None` when it is absent there.
    pub left: Option<String>,
    pub right: Option<String>,
}

/// The scalar fields worth comparing, as `(label, json pointer)`.
///
/// Deliberately NOT `token` (a credential) or `env` (compared separately, by
/// key). `resources` is compared as its own sub-fields below.
const SCALARS: &[(&str, &str)] = &[
    ("type", "/type"),
    ("enabled", "/enabled"),
    ("source.type", "/source/type"),
    ("source.repo", "/source/repo"),
    ("source.owner", "/source/owner"),
    ("source.ref", "/source/ref"),
    ("source.path", "/source/path"),
    ("source.image", "/source/image"),
    ("source.autoDeploy", "/source/autoDeploy"),
    ("build.type", "/build/type"),
    ("deploy.replicas", "/deploy/replicas"),
    ("deploy.command", "/deploy/command"),
    ("deploy.zeroDowntime", "/deploy/zeroDowntime"),
    (
        "resources.memoryReservation",
        "/resources/memoryReservation",
    ),
    ("resources.memoryLimit", "/resources/memoryLimit"),
    ("resources.cpuReservation", "/resources/cpuReservation"),
    ("resources.cpuLimit", "/resources/cpuLimit"),
];

/// Everything that differs between two `inspectService` results.
///
/// Empty means the two are configured identically in every dimension this
/// compares — which is itself the answer to "why does one behave differently?":
/// look outside the config, at data or the host.
pub fn diff(a: &Value, b: &Value) -> Vec<Difference> {
    let mut out = Vec::new();

    for (label, ptr) in SCALARS {
        // `field` renders an absent value as "-", so a field neither side sets
        // reads the same on both and is skipped — no noise for what nobody set.
        let (l, r) = (present(&field(a, ptr)), present(&field(b, ptr)));
        if l != r {
            out.push(Difference {
                what: label.to_string(),
                left: l,
                right: r,
            });
        }
    }

    diff_env(a, b, &mut out);
    diff_counts(a, b, &mut out);
    out
}

/// A real value, or `None` for one that is absent OR empty.
///
/// `field` renders an absent value as "-". An EMPTY value is folded into the
/// same None on purpose: a `deploy.command` of "" on one service and absent on
/// another both mean "no command", and showing them as a difference is noise
/// that trains the reader to skip the list.
fn present(s: &str) -> Option<String> {
    (s != "-" && !s.is_empty()).then(|| s.to_string())
}

/// Compare the two env blocks by key. Values never appear in the output.
fn diff_env(a: &Value, b: &Value, out: &mut Vec<Difference>) {
    let (ea, eb) = (env_map(a), env_map(b));
    let mut keys: Vec<&String> = ea.keys().chain(eb.keys()).collect();
    keys.sort();
    keys.dedup();
    for key in keys {
        let what = format!("env {key}");
        match (ea.get(key), eb.get(key)) {
            (Some(x), Some(y)) if x == y => {}
            // The VALUES are secrets; the reader only needs to know they differ.
            (Some(_), Some(_)) => out.push(Difference {
                what,
                left: Some("set".into()),
                right: Some("set (differs)".into()),
            }),
            (Some(_), None) => out.push(Difference {
                what,
                left: Some("set".into()),
                right: None,
            }),
            (None, Some(_)) => out.push(Difference {
                what,
                left: None,
                right: Some("set".into()),
            }),
            (None, None) => {}
        }
    }
}

/// Env text ("KEY=value" lines) as a map, so comparison is by key and
/// order-independent. A line with no `=` is a key with an empty value.
fn env_map(v: &Value) -> BTreeMap<String, String> {
    v.get("env")
        .and_then(Value::as_str)
        .unwrap_or("")
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
        .map(|l| match l.split_once('=') {
            Some((k, val)) => (k.trim().to_string(), val.to_string()),
            None => (l.trim().to_string(), String::new()),
        })
        .collect()
}

/// Collections (domains, mounts, ports) are compared by COUNT, not contents.
///
/// A full item-by-item diff of a domain list is a feature of its own, and the
/// count already answers the common question — "this one has a mount and that
/// one doesn't" — without pretending to more precision than it has.
fn diff_counts(a: &Value, b: &Value, out: &mut Vec<Difference>) {
    for (label, key) in [
        ("domains", "domains"),
        ("mounts", "mounts"),
        ("ports", "ports"),
    ] {
        let (na, nb) = (arr_len(a, key), arr_len(b, key));
        if na != nb {
            out.push(Difference {
                what: format!("{label} (count)"),
                left: Some(na.to_string()),
                right: Some(nb.to_string()),
            });
        }
    }
}

fn arr_len(v: &Value, key: &str) -> usize {
    v.get(key).and_then(Value::as_array).map_or(0, |a| a.len())
}

/// The diff as the viewer shows it: a header naming the two sides, then one
/// aligned row per difference.
///
/// `a` and `b` are the labels for each side ("project/service"). An empty diff
/// gets a sentence rather than a blank pane — "these two are identical" is a
/// real, useful answer and must not read as "nothing loaded".
pub fn diff_lines(diffs: &[Difference], a: &str, b: &str) -> Vec<String> {
    if diffs.is_empty() {
        return vec![
            format!("{a} and {b} are configured identically."),
            String::new(),
            "Every field this compares agrees — if they behave differently, the".into(),
            "cause is outside the config: data, the host, or something not deployed.".into(),
        ];
    }
    // Width of the widest field name, so the arrows line up into a column the eye
    // can run down.
    let w = diffs
        .iter()
        .map(|d| d.what.chars().count())
        .max()
        .unwrap_or(0);
    let side = |v: &Option<String>| v.clone().unwrap_or_else(|| "—".into());
    let mut out = vec![
        format!("{a}   vs   {b}"),
        format!("{} differences", diffs.len()),
        String::new(),
    ];
    out.extend(diffs.iter().map(|d| {
        format!(
            "{:<w$}   {}  →  {}",
            d.what,
            side(&d.left),
            side(&d.right),
            w = w
        )
    }));
    out
}

/// How two versions of a whole project compare, service by service.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectDiff {
    /// Services present only on the left host.
    pub only_left: Vec<String>,
    /// Services present only on the right host.
    pub only_right: Vec<String>,
    /// Services on both, but differing — with how many fields differ.
    pub differing: Vec<(String, usize)>,
    /// Services on both and identically configured.
    pub identical: Vec<String>,
}

/// Compare every service of a project across two hosts.
///
/// `a` and `b` are the `services` arrays from `inspectProject` on each host —
/// which already carry each service's full config, so this needs no per-service
/// fetch. Matched by NAME: a service missing on one side is the drift a
/// service-by-service compare can never show you ("staging is missing the worker
/// prod has"), and it is the first thing this surfaces.
pub fn project_diff(a: &[Value], b: &[Value]) -> ProjectDiff {
    use std::collections::BTreeMap;
    let by_name = |list: &[Value]| -> BTreeMap<String, Value> {
        list.iter()
            .map(|s| (field(s, "/name"), s.clone()))
            .collect()
    };
    let (ma, mb) = (by_name(a), by_name(b));
    let mut d = ProjectDiff {
        only_left: Vec::new(),
        only_right: Vec::new(),
        differing: Vec::new(),
        identical: Vec::new(),
    };
    for (name, sa) in &ma {
        match mb.get(name) {
            None => d.only_left.push(name.clone()),
            Some(sb) => {
                let n = diff(sa, sb).len();
                if n == 0 {
                    d.identical.push(name.clone());
                } else {
                    d.differing.push((name.clone(), n));
                }
            }
        }
    }
    for name in mb.keys() {
        if !ma.contains_key(name) {
            d.only_right.push(name.clone());
        }
    }
    // Most differences first: the service that has drifted furthest is the one to
    // look at, and a long identical list should not bury it.
    d.differing
        .sort_by(|x, y| y.1.cmp(&x.1).then(x.0.cmp(&y.0)));
    d
}

/// The project diff as the viewer shows it. `a` and `b` label the two hosts.
pub fn project_diff_lines(d: &ProjectDiff, a: &str, b: &str) -> Vec<String> {
    let total = d.only_left.len() + d.only_right.len() + d.differing.len() + d.identical.len();
    if d.only_left.is_empty() && d.only_right.is_empty() && d.differing.is_empty() {
        return vec![
            format!("{a} and {b} match across all {total} service(s)."),
            String::new(),
            "Same services, same config on each. Any difference in behaviour is".into(),
            "outside the config — data, or the host itself.".into(),
        ];
    }
    let mut out = vec![format!("{a}   vs   {b}"), String::new()];
    // Existence drift leads: a missing service is a bigger finding than a changed
    // field, and it is invisible on a per-service compare.
    if !d.only_left.is_empty() {
        out.push(format!("Only on {a}:  {}", d.only_left.join(", ")));
    }
    if !d.only_right.is_empty() {
        out.push(format!("Only on {b}:  {}", d.only_right.join(", ")));
    }
    if !d.differing.is_empty() {
        out.push(String::new());
        out.push("Differ (open one with \"Compare with another host\" to see how):".into());
        out.extend(
            d.differing
                .iter()
                .map(|(name, n)| format!("  {name}  —  {n} field(s) differ")),
        );
    }
    if !d.identical.is_empty() {
        out.push(String::new());
        out.push(format!(
            "Identical ({}): {}",
            d.identical.len(),
            d.identical.join(", ")
        ));
    }
    out
}

/// A project's config as a stable, redacted, git-committable record.
///
/// EasyPanel has no export and no import, so an operator who wants their config
/// in git — to review a change, keep a record, or diff two points in time — has
/// nowhere to get it. This produces one: every service's source, build, deploy,
/// resources, env KEYS, domains, mounts and ports.
///
/// Two things it deliberately drops, so the file is safe to commit and stable to
/// diff:
///
/// - **Every secret.** Env is reduced to its KEYS (never a value — an env is the
///   densest pile of secrets a service has), the deploy `token` is gone, and any
///   secret-named field in `source` (a registry `password`) is masked. The same
///   rule the on-screen views enforce; a config file that leaks a token into git
///   is worse than no export.
/// - **Volatile, environment-specific noise** — the last commit hash, the
///   deployment URL, the primary-domain cuid. They change on every deploy and
///   would make a diff scream about things that are not configuration.
pub fn export_project(project: &str, services: &[Value]) -> Value {
    let svcs: Vec<Value> = services.iter().map(export_service).collect();
    serde_json::json!({
        "project": project,
        "note": "easypanel-cli export — config only, secrets redacted, no data",
        "services": svcs,
    })
}

fn export_service(s: &Value) -> Value {
    let mut env_keys: Vec<String> = env_map(s).into_keys().collect();
    env_keys.sort();
    let domains: Vec<String> = s
        .get("domains")
        .and_then(Value::as_array)
        .map(|a| a.iter().map(crate::domains::domain_source).collect())
        .unwrap_or_default();
    serde_json::json!({
        "name": field(s, "/name"),
        "type": field(s, "/type"),
        "enabled": s.get("enabled").and_then(Value::as_bool).unwrap_or(true),
        "source": s.get("source").map(redact_source),
        "build": s.get("build").cloned().unwrap_or(Value::Null),
        "deploy": s.get("deploy").cloned().unwrap_or(Value::Null),
        "resources": s.get("resources").cloned().unwrap_or(Value::Null),
        "env": env_keys,
        "domains": domains,
        "mounts": s.get("mounts").cloned().unwrap_or_else(|| serde_json::json!([])),
        "ports": s.get("ports").cloned().unwrap_or_else(|| serde_json::json!([])),
    })
}

/// The `source` object with any secret-named field masked (a private registry
/// keeps a `password` here). Matched by NAME so a secret field a future EasyPanel
/// adds arrives masked rather than leaked — the same reason the Source view does.
fn redact_source(source: &Value) -> Value {
    let Some(obj) = source.as_object() else {
        return source.clone();
    };
    let mut out = serde_json::Map::new();
    for (k, v) in obj {
        // The deploy token is never config; drop it entirely.
        if k == "token" {
            continue;
        }
        if is_secret_key(k) {
            out.insert(k.clone(), Value::String("••••••••".into()));
        } else {
            out.insert(k.clone(), v.clone());
        }
    }
    Value::Object(out)
}

/// Does this field name a credential? A local copy of the on-screen rule (which
/// lives in the TUI module and is not reachable from here); a three-word check is
/// cheaper to duplicate than to hoist, and both point at the same list.
fn is_secret_key(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    [
        "password",
        "token",
        "secret",
        "credential",
        "apikey",
        "privatekey",
    ]
    .iter()
    .any(|needle| k.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn svc() -> Value {
        json!({
            "type": "app", "enabled": true,
            "source": { "type": "github", "owner": "acme", "repo": "web", "ref": "main" },
            "build": { "type": "nixpacks" },
            "deploy": { "replicas": 1, "command": "", "zeroDowntime": true },
            "env": "DATABASE_URL=postgres://a\nREDIS_HOST=r1\nLOG_LEVEL=info",
            "domains": [{}, {}], "mounts": [], "ports": [],
            "token": "secret-deploy-token"
        })
    }

    #[test]
    fn identical_services_get_a_sentence_not_a_blank_pane() {
        let lines = diff_lines(&[], "prod/api", "staging/api");
        assert!(lines[0].contains("identically"));
        // An empty result must never read as a failure to load.
        assert!(lines.iter().any(|l| l.contains("outside the config")));
    }

    #[test]
    fn a_diff_line_names_the_field_and_both_sides() {
        let diffs = vec![Difference {
            what: "deploy.replicas".into(),
            left: Some("1".into()),
            right: Some("3".into()),
        }];
        let lines = diff_lines(&diffs, "prod/api", "staging/api");
        assert!(lines[0].contains("prod/api") && lines[0].contains("staging/api"));
        let row = lines
            .iter()
            .find(|l| l.contains("deploy.replicas"))
            .unwrap();
        assert!(row.contains("1") && row.contains("3") && row.contains("→"));
        // A side that is absent reads as a dash, not an empty gap.
        let one = vec![Difference {
            what: "mounts".into(),
            left: None,
            right: Some("1".into()),
        }];
        assert!(diff_lines(&one, "a", "b")
            .iter()
            .any(|l| l.contains("—  →  1")));
    }

    fn named(name: &str) -> Value {
        let mut v = svc();
        v["name"] = json!(name);
        v
    }

    #[test]
    fn a_service_missing_on_one_host_is_the_headline() {
        let a = vec![named("api"), named("worker")];
        let b = vec![named("api")]; // worker missing on the right
        let d = project_diff(&a, &b);
        assert_eq!(d.only_left, vec!["worker"]);
        assert!(d.only_right.is_empty());
        assert_eq!(d.identical, vec!["api"]);
        // The missing service is named first in the rendered form.
        let lines = project_diff_lines(&d, "prod", "staging");
        assert!(
            lines.iter().any(|l| l.contains("Only on prod:  worker")),
            "{lines:?}"
        );
    }

    #[test]
    fn differing_services_are_sorted_most_changed_first() {
        let mut b_api = named("api");
        b_api["deploy"]["replicas"] = json!(3); // 1 diff
        let mut b_db = named("db");
        b_db["source"]["ref"] = json!("old");
        b_db["deploy"]["replicas"] = json!(9);
        b_db["env"] = json!("X=1"); // several diffs
        let a = vec![named("api"), named("db")];
        let b = vec![b_api, b_db];
        let d = project_diff(&a, &b);
        // db drifted further, so it comes first.
        assert_eq!(d.differing[0].0, "db");
        assert!(d.differing[0].1 > d.differing[1].1);
        assert_eq!(d.differing[1].0, "api");
    }

    #[test]
    fn two_matching_projects_say_so_rather_than_listing_nothing() {
        let a = vec![named("api"), named("web")];
        let d = project_diff(&a, &a.clone());
        assert!(d.differing.is_empty() && d.only_left.is_empty() && d.only_right.is_empty());
        let lines = project_diff_lines(&d, "prod", "staging");
        assert!(lines[0].contains("match across all 2"), "{lines:?}");
    }

    #[test]
    fn an_export_redacts_secrets_and_reduces_env_to_keys() {
        let s = json!({
            "name": "api", "type": "app", "enabled": true,
            "source": { "type": "image", "image": "ghcr.io/acme/api:latest",
                        "username": "bot", "password": "ghp_realtoken" },
            "build": { "type": "nixpacks" },
            "deploy": { "replicas": 2 },
            "env": "DATABASE_URL=postgres://user:pw@host/db\nREDIS_HOST=r1",
            "domains": [{ "https": true, "host": "api.test", "path": "/" }],
            "mounts": [], "ports": [],
            "token": "deploy-secret"
        });
        let out = export_project("shop", &[s]);
        let dump = serde_json::to_string(&out).unwrap();

        // No secret value survives, anywhere.
        assert!(
            !dump.contains("ghp_realtoken"),
            "registry password leaked: {dump}"
        );
        assert!(!dump.contains("postgres://"), "env value leaked: {dump}");
        assert!(
            !dump.contains("deploy-secret"),
            "deploy token leaked: {dump}"
        );

        let svc = &out["services"][0];
        // env is KEYS only, sorted.
        assert_eq!(svc["env"], json!(["DATABASE_URL", "REDIS_HOST"]));
        // A registry password is masked but its presence is still recorded.
        assert_eq!(svc["source"]["password"], json!("••••••••"));
        assert_eq!(svc["source"]["username"], json!("bot"));
        // The domain reads as its URL.
        assert_eq!(svc["domains"], json!(["https://api.test/"]));
        // The deploy token is dropped entirely, not even masked.
        assert!(svc["source"].get("token").is_none());
        assert_eq!(svc["name"], json!("api"));
        assert_eq!(svc["deploy"]["replicas"], json!(2));
    }

    #[test]
    fn the_export_is_stable_regardless_of_env_line_order() {
        // A record you diff across time must not change because the API returned
        // the env lines in a different order.
        let mk = |env: &str| {
            json!({ "name": "a", "type": "app", "env": env,
                    "source": {}, "build": {}, "deploy": {}, "mounts": [], "ports": [], "domains": [] })
        };
        let a = export_project("p", &[mk("B=2\nA=1")]);
        let b = export_project("p", &[mk("A=1\nB=2")]);
        assert_eq!(a, b);
    }

    #[test]
    fn two_identical_services_have_no_differences() {
        assert!(diff(&svc(), &svc()).is_empty());
    }

    #[test]
    fn a_scalar_difference_shows_both_sides() {
        let mut b = svc();
        b["source"]["ref"] = json!("develop");
        b["deploy"]["replicas"] = json!(3);
        let d = diff(&svc(), &b);
        assert!(d.contains(&Difference {
            what: "source.ref".into(),
            left: Some("main".into()),
            right: Some("develop".into()),
        }));
        assert!(d.contains(&Difference {
            what: "deploy.replicas".into(),
            left: Some("1".into()),
            right: Some("3".into()),
        }));
    }

    #[test]
    fn env_is_compared_by_key_and_never_leaks_a_value() {
        let mut b = svc();
        // Same keys, different secret values; a key only on the left; a key only
        // on the right.
        b["env"] = json!("DATABASE_URL=postgres://DIFFERENT\nLOG_LEVEL=info\nAPI_KEY=xyz");
        let d = diff(&svc(), &b);

        let url = d.iter().find(|x| x.what == "env DATABASE_URL").unwrap();
        assert_eq!(url.right.as_deref(), Some("set (differs)"));
        // The secret itself must never appear anywhere in the diff.
        let all = format!("{d:?}");
        assert!(!all.contains("postgres://"), "a value leaked: {all}");
        assert!(!all.contains("xyz"), "a value leaked: {all}");

        // REDIS_HOST is only on the left, API_KEY only on the right.
        assert_eq!(
            d.iter().find(|x| x.what == "env REDIS_HOST").unwrap().right,
            None
        );
        assert_eq!(
            d.iter().find(|x| x.what == "env API_KEY").unwrap().left,
            None
        );
        // LOG_LEVEL agrees on both sides, so it is not a difference.
        assert!(!d.iter().any(|x| x.what == "env LOG_LEVEL"));
    }

    #[test]
    fn env_comparison_ignores_line_order() {
        let mut b = svc();
        b["env"] = json!("LOG_LEVEL=info\nDATABASE_URL=postgres://a\nREDIS_HOST=r1");
        assert!(
            !diff(&svc(), &b).iter().any(|x| x.what.starts_with("env ")),
            "reordering the env lines must not read as a change"
        );
    }

    #[test]
    fn an_empty_value_and_an_absent_one_are_not_a_difference() {
        // "" and missing both mean "not set"; a spurious "  →  —" line is the
        // noise that makes a reader stop trusting the diff.
        let mut a = svc();
        a["deploy"]["command"] = json!(""); // set, but empty
        let mut b = svc();
        b["deploy"].as_object_mut().unwrap().remove("command"); // absent
        assert!(!diff(&a, &b).iter().any(|x| x.what == "deploy.command"));
    }

    #[test]
    fn collections_are_compared_by_count() {
        let mut b = svc();
        b["domains"] = json!([{}]); // one instead of two
        b["mounts"] = json!([{}]); // one instead of zero
        let d = diff(&svc(), &b);
        assert!(d.contains(&Difference {
            what: "domains (count)".into(),
            left: Some("2".into()),
            right: Some("1".into()),
        }));
        assert!(d.iter().any(|x| x.what == "mounts (count)"));
    }

    #[test]
    fn the_deploy_token_is_never_compared() {
        let mut b = svc();
        b["token"] = json!("a-completely-different-token");
        // The token differs, but it is a credential and not a configuration
        // difference anyone is asking about — it must not appear.
        assert!(!diff(&svc(), &b).iter().any(|x| x.what.contains("token")));
    }
}
