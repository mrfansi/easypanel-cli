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

/// Timestamp (nanodetik) baris terbaru, untuk melanjutkan tail dari sini.
///
/// queryServiceLogs menerima `start`, jadi tail cukup meminta yang lebih baru
/// dari ini — bukan menarik ulang 200 baris tiap dua detik. `start` WAJIB string;
/// mengirim angka ditolak dengan "Input validation failed" (diuji ke server).
pub fn newest_ts(result: &Value) -> Option<String> {
    result
        .get("entries")?
        .as_array()?
        .iter()
        .filter_map(|e| e.get("values")?.as_array())
        .flatten()
        .filter_map(|p| p.as_array()?.first()?.as_str())
        // Panjang dulu, baru leksikografis: "9" > "10" kalau dibandingkan
        // sebagai teks, dan nanodetik bisa berganti jumlah digit.
        .max_by(|a, b| a.len().cmp(&b.len()).then_with(|| a.cmp(b)))
        .map(str::to_string)
}

/// Timestamp berikutnya yang belum terlihat, untuk dipakai sebagai `start`.
///
/// Ceiling yang diketahui: dua baris pada nanodetik yang SAMA persis membuat
/// yang kedua terlewat. Nanodetik bertabrakan praktis tak terjadi, dan
/// alternatifnya (start inklusif + dedupe) menarik ulang baris tiap ronde.
pub fn after(ts: &str) -> String {
    match ts.parse::<u64>() {
        Ok(n) => (n + 1).to_string(),
        // Bukan angka: minta dari sini lagi dan biarkan satu baris terulang,
        // ketimbang melompati log yang belum terlihat.
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
    fn newest_ts_survives_a_digit_change() {
        // Nanodetik dibandingkan sebagai teks akan bilang "9..." > "10...".
        // Kursor yang meleset ke belakang mengulang log; yang meleset ke depan
        // MELEWATKANNYA diam-diam.
        let v = json!({ "entries": [{ "values": [
            ["9999999999999999999", "lama, digitnya kurang"],
            ["10000000000000000000", "baru"],
        ]}]});
        assert_eq!(newest_ts(&v).as_deref(), Some("10000000000000000000"));
    }

    #[test]
    fn newest_ts_reads_across_every_entry_group() {
        // queryServiceLogs mengelompokkan per label stream; yang terbaru bisa
        // ada di grup mana pun.
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
        // start bersifat inklusif: meminta ulang ts yang sama akan mengirim
        // balik baris yang sudah tampil.
        assert_eq!(after("1600000000000000000"), "1600000000000000001");
        // Bukan angka -> jangan melompat. Satu baris terulang jauh lebih baik
        // daripada satu baris hilang tanpa jejak.
        assert_eq!(after("bukan-angka"), "bukan-angka");
    }

    #[test]
    fn empty_for_empty_input() {
        assert!(format(&json!([])).is_empty());
        assert!(format(&json!({})).is_empty());
    }
}
