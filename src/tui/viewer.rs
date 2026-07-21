//! The full-screen viewer: its STATE and what each view READS AS.
//!
//! State (`ViewerUi`) and formatting (the `*_lines` functions) live together
//! here. The formatting was pulled out of `fetch_view` first — "talk to the
//! network" and "decide how this looks" had been one function — and its state
//! was the last cluster still spread across `App` as ten loose fields. It joins
//! its formatting here, the same move `BackupUi` made for the backup screen.
//!
//! The `*_lines` functions take what the API returned and give back the lines
//! the viewer shows. No I/O, so each is testable on a literal.

use ratatui::widgets::TableState;
use serde_json::Value;

use crate::output::field;

use super::app::Screen;
use super::table::row_marker;
use super::worker::View;

/// Everything the full-screen viewer is currently showing.
///
/// Ten fields that had accumulated on `App` — the text, where it scrolls, what
/// it was opened FROM and ABOUT, the live-log cursor. They belong together, and
/// keeping them here stops the next viewer tweak from reaching across a 77-field
/// struct.
pub(super) struct ViewerUi {
    /// The screen Esc returns to — the viewer opens from Services or Actions.
    pub(super) from: Screen,
    pub(super) title: String,
    pub(super) lines: Vec<String>,
    pub(super) scroll: u16,
    /// How far right it is scrolled, in columns. The viewer neither wraps nor
    /// reflows, so a line longer than the pane would be unreachable without this
    /// — and this is the screen logs open in.
    pub(super) hscroll: u16,
    /// The service view this is (ports/env/…), so `r` knows what to re-fetch.
    pub(super) ctx: Option<(View, String, String, String)>,
    /// The highlighted row, for the views that ARE rows (ports, mounts,
    /// redirects) — what `x` deletes, without the ten-row ceiling the old
    /// "press the digit on the line" had.
    pub(super) row: TableState,
    /// The action whose detail is showing, if any. An action detail has no
    /// `ctx` (it is not a service view), so this is how `r` re-fetches it — a
    /// running deploy's log must not freeze at first fetch.
    pub(super) action_detail: Option<String>,
    /// The newest log timestamp already shown; the resume marker for the tail.
    /// Some = the tail is active (only for `View::Logs`).
    pub(super) log_cursor: Option<String>,
    /// Stick to the last line. Logs grow from the bottom, so without this a new
    /// line arrives off-screen and the tail looks dead.
    pub(super) follow: bool,
}

impl Default for ViewerUi {
    fn default() -> Self {
        Self {
            // Screen has no Default; the viewer is opened from Services by default.
            from: Screen::Projects,
            title: "Viewer".into(),
            lines: Vec::new(),
            scroll: 0,
            hscroll: 0,
            ctx: None,
            row: TableState::default(),
            action_detail: None,
            log_cursor: None,
            follow: false,
        }
    }
}

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

/// Does this field name a credential?
///
/// The Source view excluded the `token` and `env` KEYS of the service, which was
/// the right instinct applied to an incomplete list: the `source` object of a
/// private registry carries a `password`, and it was printed in full — a real
/// GitHub token, in the clear, on screen. Matching on the NAME catches the ones
/// nobody has thought of yet, which is the point: a new secret field added by a
/// future EasyPanel arrives hidden rather than exposed.
pub(super) fn is_secret(key: &str) -> bool {
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

/// Source, build, deploy and resources from `inspectService`.
///
/// Credentials are never printed: the `token` and `env` keys are skipped
/// entirely (env has its own view), and any field whose name reads like a secret
/// is masked rather than shown.
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
                    _ if is_secret(k) => format!("  {k}: ••••••••"),
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
