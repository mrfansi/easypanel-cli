use ratatui::crossterm::event::KeyCode;
use ratatui::widgets::TableState;
use serde_json::Value;

use crate::output::{field, format_bytes, format_rate, num};

pub(super) const SERVICE_HEADERS: [&str; 10] = [
    "Project / Service",
    "Type",
    "Status",
    "Repl",
    "Source",
    "Auto",
    "CPU %",
    "Memory",
    "Net In",
    "Net Out",
];

/// One row of the Services table: a project header, or a service beneath it.
///
/// The hierarchy is kept but stays a single table: drill-down forces opening
/// projects one by one and can't be searched, while a flat list with no headers
/// makes empty projects vanish entirely — invisible, unselectable, undeletable.
pub(super) enum Line2<'a> {
    Project {
        name: &'a str,
        services: Vec<&'a Value>,
    },
    Service(&'a Value),
}

/// One row of the flat service table.
///
/// `source` is summarized from the inspectService bundled in
/// listProjectsAndServices, so repo and branch show without opening anything.
pub(super) fn service_row(
    s: &Value,
    running: Option<bool>,
    replicas: Option<(i64, i64)>,
) -> Vec<String> {
    let source = match field(s, "/source/type").as_str() {
        "github" => format!(
            "{}/{}#{}",
            field(s, "/source/owner"),
            field(s, "/source/repo"),
            field(s, "/source/ref")
        ),
        "git" => format!("{}#{}", field(s, "/source/repo"), field(s, "/source/ref")),
        "image" => field(s, "/source/image"),
        _ => "-".to_string(),
    };
    vec![
        field(s, "/projectName"),
        field(s, "/name"),
        field(s, "/type"),
        service_status(s, running, replicas).into(),
        replicas_cell(s, replicas),
        source,
        auto_deploy_cell(s).into(),
    ]
}

/// Whether a service is up or down.
///
/// `enabled` from the API only means "not disabled by the user", NOT "container
/// alive" — a crashed service stays enabled.
///
/// The best ground truth is the swarm replicas (`replicas` = actual/desired from
/// getDockerTaskStats): swarm itself knows how many should be running and how
/// many actually are. When present, it wins over the metric guess:
///
/// - "down": desired>0 but actual<desired — the container is dead/crash-looping
///   and swarm hasn't managed to bring it back up. This used to look identical to
///   "stopped" (deliberately stopped) when it actually means "broken right now".
/// - "stopped": desired=0 — deliberately scaled to zero / stopped by the user.
/// - "active": actual>=desired.
///
/// Without replicas (not yet loaded), fall back to the old metric signal:
/// `running` Some(true)=metrics exist, Some(false)=none (yet enabled → really
/// dead), None=not loaded / filter context (don't accuse it of being dead).
///
/// - "disabled": disabled by the user (enabled=false)
pub(super) fn service_status(
    s: &Value,
    running: Option<bool>,
    replicas: Option<(i64, i64)>,
) -> &'static str {
    let enabled = s.get("enabled").and_then(Value::as_bool).unwrap_or(true);
    if !enabled {
        return "disabled";
    }
    if let Some((actual, desired)) = replicas {
        return if desired == 0 {
            "stopped"
        } else if actual < desired {
            "down"
        } else {
            "active"
        };
    }
    match running {
        Some(false) => "stopped",
        _ => "active",
    }
}

/// Repl column: how many replicas this service runs.
///
/// Swarm's live count wins when it is loaded — and while `actual` differs from
/// `desired` it shows both (`0/1`, `2/3`), which is exactly when the number matters:
/// a rollout in progress, or replicas that never came up. Otherwise it is just the
/// count. Falls back to the configured `deploy.replicas`, and "-" for a service with
/// no deploy block at all (databases).
pub(super) fn replicas_cell(s: &Value, replicas: Option<(i64, i64)>) -> String {
    if let Some((actual, desired)) = replicas {
        return if actual == desired {
            desired.to_string()
        } else {
            format!("{actual}/{desired}")
        };
    }
    match s.pointer("/deploy/replicas").and_then(Value::as_i64) {
        Some(n) => n.to_string(),
        None => "-".to_string(),
    }
}

/// Auto column: only a github source has auto deploy.
///
/// Three states, not two. The server doesn't send `autoDeploy` for image sources
/// or databases (checked directly against the API: present on 15 of 16 apps, the
/// odd one out sourced from an image). A "✗" on a MySQL row would mean "not yet"
/// for something that can never be turned on — exactly the kind of number that's
/// confident but wrong.
pub(super) fn auto_deploy_cell(s: &Value) -> &'static str {
    match (
        field(s, "/source/type").as_str(),
        s.pointer("/source/autoDeploy").and_then(Value::as_bool),
    ) {
        ("github", Some(true)) => "✓",
        ("github", Some(false)) => "✗",
        _ => "-",
    }
}

/// Project header row: aggregate of its children's metrics.
///
/// `mets` is already filtered: only services whose metrics exist. If empty,
/// nothing is measured — so "-", not a number. "0.0 %" would claim it was
/// measured and came out zero, and without this guard `Sum` for f64 (whose
/// identity is -0.0) prints "-0.0 %": a convincing-looking negative CPU.
pub(super) fn project_row(name: &str, count: usize, mets: &[&Value]) -> Vec<String> {
    let mut row: Vec<String> = vec![format!("{name} ({count})")];
    // Type / Status / Repl / Source / Auto: a project header aggregates metrics, not
    // per-service state.
    row.extend(["-", "-", "-", "-", "-"].map(String::from));
    if mets.is_empty() {
        row.extend(metric_cols(None));
        return row;
    }
    let sum = |ptr: &str| -> f64 { mets.iter().map(|m| num(m, ptr)).sum() };
    row.extend([
        format!("{:.1} %", sum("/cpu")),
        format_bytes(sum("/memory")),
        format_rate(sum("/networkIn")),
        format_rate(sum("/networkOut")),
    ]);
    row
}

/// Metric columns for a service; "-" when its metrics aren't (yet) available.
///
/// Split out from service_row() so the filter only matches identity
/// (project/service/type/source) — searching for "1" shouldn't match every row
/// just because of its CPU number.
pub(super) fn metric_cols(m: Option<&Value>) -> Vec<String> {
    let Some(m) = m else {
        return vec!["-".into(), "-".into(), "-".into(), "-".into()];
    };
    vec![
        format!("{:.1} %", num(m, "/cpu")),
        format_bytes(num(m, "/memory")),
        format_rate(num(m, "/networkIn")),
        format_rate(num(m, "/networkOut")),
    ]
}

/// Whether a row passes the filter.
///
/// Matched against the DISPLAYED text, not the raw JSON: what the user is looking
/// for is what's on screen.
pub(super) fn keep(row: &[String], filter: &str) -> bool {
    if filter.is_empty() {
        return true;
    }
    let f = filter.to_lowercase();
    row.iter().any(|c| c.to_lowercase().contains(&f))
}

/// Select the first row when the list is non-empty and nothing is selected yet.
pub(super) fn select_first(state: &mut TableState, len: usize) {
    if len == 0 {
        state.select(None);
    } else if state.selected().is_none() {
        state.select(Some(0));
    }
}

/// Table navigation: arrows/jk, PgUp/PgDn, Home/End.
pub(super) fn move_table(state: &mut TableState, code: KeyCode, len: usize) {
    if len == 0 {
        return;
    }
    let delta: isize = match code {
        KeyCode::Down | KeyCode::Char('j') => 1,
        KeyCode::Up | KeyCode::Char('k') => -1,
        KeyCode::PageDown => 10,
        KeyCode::PageUp => -10,
        KeyCode::Home => -(len as isize),
        KeyCode::End => len as isize,
        _ => return,
    };
    let cur = state.selected().unwrap_or(0) as isize;
    state.select(Some(
        cur.saturating_add(delta).clamp(0, len as isize - 1) as usize
    ));
}

pub(super) fn short_reason(err: &str) -> &str {
    if err.contains("403") || err.contains("Forbidden") {
        "GitHub rejected: 403"
    } else if err.contains("401") || err.contains("Unauthorized") {
        "GitHub rejected: invalid token"
    } else {
        "failed"
    }
}
