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
