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
