//! What each viewer READS AS.
//!
//! These used to sit inside `fetch_view`, interleaved with the API calls that
//! produce their input — so "talk to the network" and "decide how this looks"
//! were the same function, and none of the formatting could be exercised without
//! standing up an HTTP server. The worker now fetches and hands the JSON here.
//!
//! Every function takes what the API returned and gives back the lines the
//! viewer shows. No I/O, so each of these is testable on a literal.

use serde_json::Value;

use crate::output::field;

use super::table::row_marker;

/// Rows from a JSON array, or a single line saying there are none.
///
/// The empty case is a sentence rather than a blank pane: a viewer that opens
/// empty must say whether there is nothing to show or something went wrong.
pub(super) fn list_or_empty(
    v: &Value,
    empty: &str,
    f: impl Fn(usize, &Value) -> String,
) -> Vec<String> {
    let arr = v.as_array().cloned().unwrap_or_default();
    if arr.is_empty() {
        return vec![empty.to_string()];
    }
    arr.iter().enumerate().map(|(i, x)| f(i, x)).collect()
}

/// A newline-separated string field (env, config file) as lines.
pub(super) fn text_lines(v: &Value, pointer: &str) -> Vec<String> {
    v.pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or("")
        .lines()
        .map(String::from)
        .collect()
}

pub(super) fn ports_lines(v: &Value) -> Vec<String> {
    list_or_empty(v, "No ports yet — press n to add one", |i, p| {
        format!(
            "{} {} {}->{}",
            row_marker(i),
            field(p, "/protocol"),
            field(p, "/published"),
            field(p, "/target")
        )
    })
}

pub(super) fn mounts_lines(v: &Value) -> Vec<String> {
    list_or_empty(v, "No mounts yet — press n to add one", |i, m| {
        let detail = match field(m, "/type").as_str() {
            "bind" => format!("{} -> {}", field(m, "/hostPath"), field(m, "/mountPath")),
            "volume" => format!("{} -> {}", field(m, "/name"), field(m, "/mountPath")),
            _ => field(m, "/mountPath"),
        };
        format!("{} {}  {detail}", row_marker(i), field(m, "/type"))
    })
}

/// Redirects live inside `inspectService`, not in a list endpoint of their own.
pub(super) fn redirects_lines(v: &Value) -> Vec<String> {
    let arr = v
        .get("redirects")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if arr.is_empty() {
        return vec!["No redirects".into()];
    }
    arr.iter()
        .enumerate()
        .map(|(i, r)| {
            let kind = if r.get("permanent").and_then(Value::as_bool).unwrap_or(false) {
                "301"
            } else {
                "302"
            };
            let on = if r.get("enabled").and_then(Value::as_bool).unwrap_or(true) {
                "on"
            } else {
                "off"
            };
            format!(
                "{} {} -> {}  ({kind}, {on})",
                row_marker(i),
                field(r, "/regex"),
                field(r, "/replacement")
            )
        })
        .collect()
}

/// Source, build, deploy and resources from `inspectService`.
///
/// Deliberately NOT showing `token` (the deploy token) or `env`: both are
/// credentials, and env has its own view.
pub(super) fn source_lines(v: &Value) -> Vec<String> {
    let mut out = Vec::new();
    for (title, key) in [
        ("Source", "source"),
        ("Build", "build"),
        ("Deploy", "deploy"),
        ("Resources", "resources"),
    ] {
        out.push(format!("── {title}"));
        match v.get(key) {
            // pointer "" = the value itself, so a string shows without quotes.
            Some(Value::Object(o)) if !o.is_empty() => out.extend(o.iter().map(|(k, val)| {
                match val {
                    // A flag reads as a word, not as JSON. `autoDeploy` is the
                    // SAME field the Services table shows as ✓/✗ and the Backups
                    // view shows as on/off — three renderings of one boolean in
                    // one app, and this was the raw one.
                    Value::Bool(b) => format!("  {k}: {}", if *b { "yes" } else { "no" }),
                    _ => format!("  {k}: {}", field(val, "")),
                }
            })),
            _ => out.push("  (not set)".into()),
        }
        out.push(String::new());
    }
    out
}

/// Database backup schedules.
///
/// The id used to lead every row and nothing here could use it: this view has no
/// run and no delete, and it is not a collection, so there is no selection
/// either. Twenty-five characters of cuid pushed the only thing that tells two
/// rows apart — the database name — off to the right.
pub(super) fn backups_lines(v: &Value) -> Vec<String> {
    let rows = list_or_empty(v, "No database backups", |_, b| {
        let state = if b.get("enabled").and_then(Value::as_bool).unwrap_or(false) {
            "on"
        } else {
            "off"
        };
        format!(
            "{:<18}{:<16}{state}",
            field(b, "/databaseName"),
            field(b, "/schedule")
        )
    });
    // The header only earns its line when there is something under it.
    if v.as_array().is_some_and(|a| !a.is_empty()) {
        let mut out = vec!["Database          Schedule        Enabled".to_string()];
        out.extend(rows);
        out
    } else {
        rows
    }
}
