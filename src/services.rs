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
