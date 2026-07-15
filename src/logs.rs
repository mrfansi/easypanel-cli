use chrono::{Local, TimeZone};
use serde_json::Value;

/// Ubah respons queryServiceLogs jadi baris siap-tampil, terurut terlama -> terbaru.
pub fn format(result: &Value) -> Vec<String> {
    let mut rows: Vec<(Option<String>, String)> = Vec::new();

    // Bentuk Loki: { entries: [ { values: [ [ns_timestamp, message], ... ] } ] }
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
        // Fallback: list string atau list objek.
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
                    ["1600000060000000000", "baris kedua"],
                    ["1600000000000000000", "baris pertama"],
                ]
            }]
        });

        let lines = format(&result);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].ends_with("baris pertama"));
        assert!(lines[1].ends_with("baris kedua"));
        // Prefix HH:MM:SS.
        let prefix = &lines[0][..8];
        assert_eq!(prefix.chars().filter(|c| *c == ':').count(), 2);
    }

    #[test]
    fn falls_back_to_plain_strings() {
        assert_eq!(format(&json!(["satu", "dua"])), vec!["satu", "dua"]);
    }

    #[test]
    fn empty_for_empty_input() {
        assert!(format(&json!([])).is_empty());
        assert!(format(&json!({})).is_empty());
    }
}
