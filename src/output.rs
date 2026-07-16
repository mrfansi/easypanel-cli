use comfy_table::{presets::UTF8_FULL, Table};
use serde_json::Value;

/// Cetak tabel sederhana ke stdout.
pub fn table(headers: &[&str], rows: Vec<Vec<String>>) {
    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    table.set_header(headers.iter().map(|h| h.to_string()));
    for row in rows {
        table.add_row(row);
    }
    println!("{table}");
}

/// Ambil field JSON via pointer (mis. "/cpuInfo/count") sebagai string; "-" bila kosong.
pub fn field(value: &Value, pointer: &str) -> String {
    match value.pointer(pointer) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(v) if !v.is_null() => v.to_string(),
        _ => "-".to_string(),
    }
}

/// Angka di pointer JSON sebagai f64 (menerima number maupun string numerik).
pub fn num(value: &Value, pointer: &str) -> f64 {
    match value.pointer(pointer) {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0),
        Some(Value::String(s)) => s.parse().unwrap_or(0.0),
        _ => 0.0,
    }
}

/// Nilai satu titik seri metrics: `[unix_ts, "12.34"]`.
fn point_value(p: &Value) -> f64 {
    match p.get(1) {
        Some(Value::String(s)) => s.parse().unwrap_or(0.0),
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0),
        _ => 0.0,
    }
}

/// Nilai terbaru sebuah seri metrics (mis. "cpu").
pub fn series_last(v: &Value, key: &str) -> f64 {
    v.get(key)
        .and_then(Value::as_array)
        .and_then(|a| a.last())
        .map(point_value)
        .unwrap_or(0.0)
}

/// `n` titik terakhir sebuah seri, dinormalisasi ke 0..100 terhadap rentang
/// datanya sendiri (min..max) untuk sparkline.
///
/// Skala absolut tidak berguna di sini: disk yang selalu ~16% akan membuat semua
/// bar penuh (blok solid) bila diskala 0..max, dan CPU 1-5% jadi tak terlihat bila
/// diskala 0..100. Yang ingin dilihat dari sparkline adalah *bentuk perubahannya* —
/// nilai absolutnya sudah tercetak di sebelahnya. Deret datar dirender setengah
/// tinggi supaya terbaca sebagai garis rata, bukan kosong.
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

/// Laju byte per detik jadi bentuk terbaca (mis. "30.4 KB/s").
pub fn format_rate(bytes_per_sec: f64) -> String {
    format!("{}/s", format_bytes(bytes_per_sec))
}

/// Ukuran byte jadi bentuk terbaca (mis. "4.1 GB").
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

/// Timestamp EasyPanel ("2026-07-16 05:55:15", UTC) jadi NaiveDateTime.
pub fn parse_ts(s: &str) -> Option<chrono::NaiveDateTime> {
    chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").ok()
}

/// Detik jadi durasi ringkas berbahasa Indonesia.
pub fn human_duration(secs: i64) -> String {
    match secs {
        s if s < 60 => format!("{s} detik"),
        s if s < 3600 => format!("{} menit", s / 60),
        s if s < 86400 => format!("{} jam", s / 3600),
        s => format!("{} hari", s / 86400),
    }
}

/// Selisih dua timestamp EasyPanel sebagai durasi.
pub fn duration_between(start: &str, end: &str) -> String {
    match (parse_ts(start), parse_ts(end)) {
        (Some(a), Some(b)) => human_duration((b - a).num_seconds().max(0)),
        _ => "-".to_string(),
    }
}

/// Umur sebuah timestamp relatif sekarang (mis. "3 jam lalu").
pub fn age_of(ts: &str) -> String {
    match parse_ts(ts) {
        Some(t) => format!(
            "{} lalu",
            human_duration((chrono::Utc::now().naive_utc() - t).num_seconds().max(0))
        ),
        None => "-".to_string(),
    }
}

/// Baris pertama, dipotong pada `max` karakter.
pub fn first_line(s: &str, max: usize) -> String {
    let line = s.lines().next().unwrap_or("").trim();
    if line.chars().count() <= max {
        return line.to_string();
    }
    let cut: String = line.chars().take(max.saturating_sub(1)).collect();
    format!("{cut}…")
}

/// Boolean di pointer JSON sebagai "ya"/"tidak".
pub fn yes_no(value: &Value, pointer: &str) -> String {
    if value
        .pointer(pointer)
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        "ya".to_string()
    } else {
        "tidak".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn spark_normalises_to_own_range() {
        // CPU 1..5% harus memakai seluruh tinggi, bukan tenggelam di skala 0..100.
        let v = json!({ "cpu": [[1, "1"], [2, "3"], [3, "5"]] });
        assert_eq!(series_spark(&v, "cpu", 10), vec![0, 50, 100]);
    }

    #[test]
    fn spark_renders_flat_series_as_mid_line() {
        // Disk yang konstan: garis rata, bukan blok solid (dan bukan kosong).
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
    fn bytes_and_rates_are_human_readable() {
        assert_eq!(format_bytes(512.0), "512 B");
        assert_eq!(format_bytes(1024.0), "1.0 KB");
        assert_eq!(format_bytes(1_073_741_824.0), "1.0 GB");
        assert_eq!(format_rate(2048.0), "2.0 KB/s");
    }

    #[test]
    fn first_line_trims_and_truncates() {
        assert_eq!(first_line("satu\ndua", 20), "satu");
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
}
