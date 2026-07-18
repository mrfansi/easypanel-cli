use chrono::{Local, TimeZone};
use serde_json::Value;

/// Turn a queryServiceLogs response into display-ready lines, sorted oldest -> newest.
pub fn format(result: &Value) -> Vec<String> {
    let mut rows: Vec<(Option<String>, String)> = Vec::new();

    // Loki shape: { entries: [ { values: [ [ns_timestamp, message], ... ] } ] }
    if let Some(entries) = result.get("entries").and_then(Value::as_array) {
        for entry in entries {
            let Some(values) = entry.get("values").and_then(Value::as_array) else {
                continue;
            };
            for pair in values {
                let Some(arr) = pair.as_array() else { continue };
                let ts = arr.first().and_then(Value::as_str).map(str::to_string);
                let msg = arr.get(1).and_then(Value::as_str).unwrap_or("").to_string();
                rows.push((ts, msg));
            }
        }
    } else if let Some(arr) = result.as_array() {
        // Fallback: a list of strings or a list of objects.
        for line in arr {
            let msg = if let Some(s) = line.as_str() {
                s.to_string()
            } else {
                line.get("message")
                    .and_then(Value::as_str)
                    .or_else(|| line.get("line").and_then(Value::as_str))
                    .map(str::to_string)
                    .unwrap_or_else(|| line.to_string())
            };
            rows.push((None, msg));
        }
    }

    rows.sort_by(|a, b| a.0.cmp(&b.0));

    rows.into_iter()
        .map(|(ts, msg)| match ts {
            Some(ts) => format!("{} {}", format_time(&ts), msg),
            None => msg,
        })
        .collect()
}

/// Timestamp (nanoseconds) of the newest line, to continue tailing from here.
///
/// queryServiceLogs accepts `start`, so tailing just needs to ask for anything
/// newer than this — instead of re-pulling 200 lines every two seconds. `start`
/// MUST be a string; sending a number is rejected with "Input validation
/// failed" (verified against the server).
pub fn newest_ts(result: &Value) -> Option<String> {
    result
        .get("entries")?
        .as_array()?
        .iter()
        .filter_map(|e| e.get("values")?.as_array())
        .flatten()
        .filter_map(|p| p.as_array()?.first()?.as_str())
        // Length first, then lexicographic: "9" > "10" when compared as text,
        // and the nanosecond count can change digit length.
        .max_by(|a, b| a.len().cmp(&b.len()).then_with(|| a.cmp(b)))
        .map(str::to_string)
}

/// The next not-yet-seen timestamp, to use as `start`.
///
/// Known ceiling: two lines at the EXACT same nanosecond means the second one
/// is skipped. Colliding nanoseconds practically never happen, and the
/// alternative (inclusive start + dedupe) re-pulls lines every round.
pub fn after(ts: &str) -> String {
    match ts.parse::<u64>() {
        Ok(n) => (n + 1).to_string(),
        // Not a number: ask from here again and let one line repeat, rather
        // than skipping over logs that haven't been seen yet.
        Err(_) => ts.to_string(),
    }
}

fn format_time(ns: &str) -> String {
    let secs = ns.parse::<i64>().unwrap_or(0) / 1_000_000_000;
    Local
        .timestamp_opt(secs, 0)
        .single()
        .map(|dt| dt.format("%H:%M:%S").to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn flattens_loki_entries_oldest_first_with_time_prefix() {
        let result = json!({
            "entries": [{
                "values": [
                    ["1600000060000000000", "second line"],
                    ["1600000000000000000", "first line"],
                ]
            }]
        });

        let lines = format(&result);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].ends_with("first line"));
        assert!(lines[1].ends_with("second line"));
        // Prefix HH:MM:SS.
        let prefix = &lines[0][..8];
        assert_eq!(prefix.chars().filter(|c| *c == ':').count(), 2);
    }

    #[test]
    fn falls_back_to_plain_strings() {
        assert_eq!(format(&json!(["one", "two"])), vec!["one", "two"]);
    }

    #[test]
    fn newest_ts_survives_a_digit_change() {
        // Nanoseconds compared as text would say "9..." > "10...". A cursor
        // that drifts backward repeats logs; one that drifts forward skips
        // them SILENTLY.
        let v = json!({ "entries": [{ "values": [
            ["9999999999999999999", "old, fewer digits"],
            ["10000000000000000000", "new"],
        ]}]});
        assert_eq!(newest_ts(&v).as_deref(), Some("10000000000000000000"));
    }

    #[test]
    fn newest_ts_reads_across_every_entry_group() {
        // queryServiceLogs groups by stream label; the newest one can be in any
        // group.
        let v = json!({ "entries": [
            { "values": [["1600000000000000000", "a"]] },
            { "values": [["1600000060000000000", "b"]] },
        ]});
        assert_eq!(newest_ts(&v).as_deref(), Some("1600000060000000000"));
        assert_eq!(newest_ts(&json!({})), None);
        assert_eq!(newest_ts(&json!({ "entries": [] })), None);
    }

    #[test]
    fn after_asks_for_the_next_nanosecond_only() {
        // start is inclusive: asking again for the same ts sends back a line
        // that's already been shown.
        assert_eq!(after("1600000000000000000"), "1600000000000000001");
        // Not a number -> don't skip ahead. One repeated line beats one silently
        // lost line.
        assert_eq!(after("not-a-number"), "not-a-number");
    }

    #[test]
    fn empty_for_empty_input() {
        assert!(format(&json!([])).is_empty());
        assert!(format(&json!({})).is_empty());
    }
}
