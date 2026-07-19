use std::sync::atomic::{AtomicBool, Ordering};

use comfy_table::{presets::UTF8_FULL, Table};
use serde_json::Value;

// The output mode is chosen once from the --json flag and then read by many
// read-only commands. A process-wide flag (like a logger's verbosity) avoids
// threading a `json: bool` through ~16 signatures + call sites — it's
// configuration, not a per-function argument.
// ponytail: global flag; if two output modes are ever needed at once, make it
// a parameter — but this CLI is one process per command, so not needed yet.
static JSON_OUTPUT: AtomicBool = AtomicBool::new(false);

/// Enable raw JSON output for read-only commands (called once in main).
pub fn set_json_output(on: bool) {
    JSON_OUTPUT.store(on, Ordering::Relaxed);
}

/// Whether read-only commands should print the raw API JSON instead of a table.
pub fn json_output() -> bool {
    JSON_OUTPUT.load(Ordering::Relaxed)
}

/// Print the raw API JSON (pretty-printed). Its shape belongs to the server, not
/// to us: scripts get exactly what EasyPanel sent, including an empty `[]`
/// instead of a "No X." message that isn't valid JSON.
pub fn print_json(value: &Value) {
    println!("{}", json_string(value));
}

/// Pretty JSON for a Value; falls back to the compact form if pretty
/// serialization fails (practically never happens for an already-valid Value).
pub fn json_string(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

/// Print a simple table to stdout.
pub fn table(headers: &[&str], rows: Vec<Vec<String>>) {
    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    table.set_header(headers.iter().map(|h| h.to_string()));
    for row in rows {
        table.add_row(row);
    }
    println!("{table}");
}

/// Get a JSON field via pointer (e.g. "/cpuInfo/count") as a string; "-" if empty.
pub fn field(value: &Value, pointer: &str) -> String {
    match value.pointer(pointer) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(v) if !v.is_null() => v.to_string(),
        _ => "-".to_string(),
    }
}

/// Number at a JSON pointer as f64 (accepts both a number and a numeric string).
pub fn num(value: &Value, pointer: &str) -> f64 {
    match value.pointer(pointer) {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0),
        Some(Value::String(s)) => s.parse().unwrap_or(0.0),
        _ => 0.0,
    }
}

/// Value of a single metrics series point: `[unix_ts, "12.34"]`.
fn point_value(p: &Value) -> f64 {
    match p.get(1) {
        Some(Value::String(s)) => s.parse().unwrap_or(0.0),
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0),
        _ => 0.0,
    }
}

/// Latest value of a metrics series (e.g. "cpu").
pub fn series_last(v: &Value, key: &str) -> f64 {
    v.get(key)
        .and_then(Value::as_array)
        .and_then(|a| a.last())
        .map(point_value)
        .unwrap_or(0.0)
}

/// A percentage series drawn at its TRUE height, for a sparkline with `.max(100)`.
///
/// `series_spark` rescales to the window's own min..max, which is right for a
/// series with no ceiling but a lie for a percentage: measured against a live
/// host, CPU moving between 7.8% and 19.4% was drawn from an empty bar to a FULL
/// one, under a panel titled "CPU History (%)". The chart said the machine was
/// pegged; it was idling. The lowest sample in any window was always empty and
/// the highest always full, whatever the real numbers were.
///
/// This does trade something away, and the trade is deliberate. Window-relative
/// scaling shows the SHAPE of small movements; a true scale draws an idle machine
/// as a low flat band, which looks boring. That is the correct answer: the panel
/// is titled "(%)" and the question it exists to answer is "is this host under
/// load?". A real spike still stands out, because it really is taller.
pub fn series_percent(v: &Value, key: &str, n: usize) -> Vec<u64> {
    v.get(key)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .rev()
                .take(n)
                .rev()
                .map(|p| point_value(p).clamp(0.0, 100.0).round() as u64)
                .collect()
        })
        .unwrap_or_default()
}

/// A series rescaled to its own window, for values with NO fixed ceiling — a
/// network rate, where "what counts as full" only makes sense relative to what
/// else happened. Never use it for a percentage: see `series_percent`.
pub fn series_spark(v: &Value, key: &str, n: usize) -> Vec<u64> {
    let vals: Vec<f64> = v
        .get(key)
        .and_then(Value::as_array)
        .map(|a| a.iter().rev().take(n).rev().map(point_value).collect())
        .unwrap_or_default();

    if vals.is_empty() {
        return Vec::new();
    }
    let min = vals.iter().copied().fold(f64::INFINITY, f64::min);
    let max = vals.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let range = max - min;

    vals.iter()
        .map(|v| {
            if range <= f64::EPSILON {
                50
            } else {
                (((v - min) / range) * 100.0).round().clamp(0.0, 100.0) as u64
            }
        })
        .collect()
}

/// Bytes per second as a human-readable string (e.g. "30.4 KB/s").
pub fn format_rate(bytes_per_sec: f64) -> String {
    format!("{}/s", format_bytes(bytes_per_sec))
}

/// Byte size as a human-readable string (e.g. "4.1 GB").
pub fn format_bytes(bytes: f64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes.max(0.0);
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{v:.0} {}", UNITS[i])
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

/// EasyPanel timestamp ("2026-07-16 05:55:15", UTC) as a NaiveDateTime.
pub fn parse_ts(s: &str) -> Option<chrono::NaiveDateTime> {
    chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").ok()
}

/// Seconds as a concise human-readable duration.
pub fn human_duration(secs: i64) -> String {
    match secs {
        s if s < 60 => format!("{s} seconds"),
        s if s < 3600 => format!("{} minutes", s / 60),
        s if s < 86400 => format!("{} hours", s / 3600),
        s => format!("{} days", s / 86400),
    }
}

/// Difference between two EasyPanel timestamps, as a duration.
pub fn duration_between(start: &str, end: &str) -> String {
    match (parse_ts(start), parse_ts(end)) {
        (Some(a), Some(b)) => human_duration((b - a).num_seconds().max(0)),
        _ => "-".to_string(),
    }
}

/// Age of a timestamp relative to now (e.g. "3 hours ago").
pub fn age_of(ts: &str) -> String {
    match parse_ts(ts) {
        Some(t) => format!(
            "{} ago",
            human_duration((chrono::Utc::now().naive_utc() - t).num_seconds().max(0))
        ),
        None => "-".to_string(),
    }
}

/// First line, truncated at `max` characters.
pub fn first_line(s: &str, max: usize) -> String {
    let line = s.lines().next().unwrap_or("").trim();
    if line.chars().count() <= max {
        return line.to_string();
    }
    let cut: String = line.chars().take(max.saturating_sub(1)).collect();
    format!("{cut}…")
}

/// Boolean at a JSON pointer as "yes"/"no".
pub fn yes_no(value: &Value, pointer: &str) -> String {
    if value
        .pointer(pointer)
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        "yes".to_string()
    } else {
        "no".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn spark_normalises_to_own_range() {
        // CPU at 1..5% should use the full height, not get lost on a 0..100 scale.
        let v = json!({ "cpu": [[1, "1"], [2, "3"], [3, "5"]] });
        assert_eq!(series_spark(&v, "cpu", 10), vec![0, 50, 100]);
    }

    #[test]
    fn spark_renders_flat_series_as_mid_line() {
        // Constant disk usage: a level line, not a solid block (and not empty).
        let v = json!({ "disk": [[1, "16.2"], [2, "16.2"]] });
        assert_eq!(series_spark(&v, "disk", 10), vec![50, 50]);
    }

    #[test]
    fn spark_takes_only_last_n_points() {
        let v = json!({ "cpu": [[1, "0"], [2, "0"], [3, "0"], [4, "10"]] });
        assert_eq!(series_spark(&v, "cpu", 2).len(), 2);
        assert_eq!(series_spark(&v, "nope", 5), Vec::<u64>::new());
    }

    #[test]
    fn series_last_reads_final_point() {
        let v = json!({ "cpu": [[1, "1.0"], [2, "5.5"]] });
        assert_eq!(series_last(&v, "cpu"), 5.5);
        assert_eq!(series_last(&v, "nope"), 0.0);
    }

    #[test]
    fn series_last_returns_zero_for_flat_arrays() {
        // Trap: `loadAvg` from metrics is NOT a [ts, value] series like cpu —
        // it's three strings holding the 1/5/15-minute averages. series_last()
        // looks for p[1] at each point and returns 0.0, which reads as "load
        // zero" when the actual load is 0.58. Use commands::load_avg() for this.
        let v = json!({ "loadAvg": ["0.58", "0.62", "0.7"] });
        assert_eq!(series_last(&v, "loadAvg"), 0.0);
    }

    #[test]
    fn bytes_and_rates_are_human_readable() {
        assert_eq!(format_bytes(512.0), "512 B");
        assert_eq!(format_bytes(1024.0), "1.0 KB");
        assert_eq!(format_bytes(1_073_741_824.0), "1.0 GB");
        assert_eq!(format_rate(2048.0), "2.0 KB/s");
    }

    #[test]
    fn first_line_trims_and_truncates() {
        assert_eq!(first_line("one\ntwo", 20), "one");
        assert_eq!(first_line("abcdef", 4), "abc…");
        assert_eq!(first_line("abcd", 4), "abcd");
    }

    #[test]
    fn num_accepts_numbers_and_numeric_strings() {
        let v = json!({ "a": 5, "b": "842787864576", "c": "x" });
        assert_eq!(num(&v, "/a"), 5.0);
        assert_eq!(num(&v, "/b"), 842_787_864_576.0);
        assert_eq!(num(&v, "/c"), 0.0);
    }

    #[test]
    fn format_bytes_survives_extreme_and_invalid_input() {
        // Bytes come from the server: could be 0, negative (if the server is
        // wrong), or NaN. All of these must become "0 B", not "-5 B", "NaN B", or
        // a panic. This behavior relies on `.max(0.0)`; a refactor that silently
        // drops it would break this, so it's pinned here.
        assert_eq!(format_bytes(-5.0), "0 B");
        assert_eq!(format_bytes(f64::NAN), "0 B");
        // Exact unit boundary.
        assert_eq!(format_bytes(1023.0), "1023 B");
        assert_eq!(format_bytes(1024.0), "1.0 KB");
        // Never goes past TB, no matter how large — and doesn't overflow/panic.
        assert!(format_bytes(1024f64.powi(6)).ends_with(" TB"));
        assert!(format_bytes(f64::MAX).ends_with(" TB"));
        assert_eq!(format_bytes(f64::INFINITY), "inf TB");
    }

    #[test]
    fn a_percentage_series_is_drawn_at_its_real_height() {
        // Measured against a live host: CPU between 7.8% and 19.4% was drawn from
        // an EMPTY bar to a FULL one under a panel titled "CPU History (%)" —
        // the chart said the machine was pegged while it idled.
        let v = json!({ "cpu": [
            ["1", "7.8"], ["2", "8.7"], ["3", "19.4"]
        ]});
        assert_eq!(series_percent(&v, "cpu", 10), vec![8, 9, 19]);
        // The same data through the window-relative helper is what the bug looked
        // like: the low sample empty, the high one full.
        assert_eq!(series_spark(&v, "cpu", 10), vec![0, 8, 100]);

        // Out-of-range values are clamped rather than blowing past the ceiling.
        let odd = json!({ "cpu": [["1", "-5"], ["2", "140"]] });
        assert_eq!(series_percent(&odd, "cpu", 10), vec![0, 100]);
        // Empty and missing behave like the other helpers: nothing, not a fake 0.
        assert!(series_percent(&json!({ "cpu": [] }), "cpu", 10).is_empty());
        assert!(series_percent(&json!({}), "cpu", 10).is_empty());
        // The window keeps the LAST n points.
        let many = json!({ "cpu": [["1","1"],["2","2"],["3","3"]] });
        assert_eq!(series_percent(&many, "cpu", 2), vec![2, 3]);
    }

    #[test]
    fn series_helpers_are_empty_safe() {
        // An empty series / missing key must not panic or make up a number:
        // "-" is more honest than a fake 0 (see the loadAvg bug history).
        let empty = json!({ "cpu": [] });
        assert_eq!(series_last(&empty, "cpu"), 0.0);
        assert_eq!(series_last(&empty, "missing"), 0.0);
        assert!(series_spark(&empty, "cpu", 30).is_empty());
        assert!(series_spark(&json!({}), "cpu", 30).is_empty());
        // One point: no range, so a mid-height line (50), not a divide-by-zero.
        let one = json!({ "cpu": [["1700000000000", "42"]] });
        assert_eq!(series_spark(&one, "cpu", 30), vec![50]);
    }

    #[test]
    fn json_string_is_pretty_and_preserves_the_api_shape() {
        // --json must print what the server sent, not our own schema — so what
        // matters is: valid, indented, and no fields lost or added.
        let v = json!({ "name": "web", "members": [1, 2] });
        let s = json_string(&v);
        assert!(s.contains("\n  "), "must be pretty (indented): {s}");
        assert_eq!(serde_json::from_str::<Value>(&s).unwrap(), v);

        // An empty array prints as `[]`, not a human message — that's the heart
        // of script-friendliness: `[]` can be read by `jq`, "No X." can't.
        assert_eq!(json_string(&json!([])), "[]");
    }
}
