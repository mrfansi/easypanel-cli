//! How host and service metrics READ — the monitoring bounded context.
//!
//! These say what a load average, a per-service metrics table, and a storage
//! listing look like. They lived in `commands.rs` (the CLI layer) and both the
//! CLI tables and the TUI's Monitor screen reached into it — one presentation
//! borrowing another's idea of how a metric reads. They are the monitoring
//! vocabulary itself, so both surfaces now depend on this context rather than on
//! each other. No I/O here: the caller fetches, this decides how it looks.

use serde_json::Value;

use crate::output::{field, format_bytes, format_rate, num};

/// The three load-average figures as "1.02, 1.35, 1.57", or "-" if absent.
pub fn load_avg(s: &Value) -> String {
    s.get("loadAvg")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|v| v.as_str().unwrap_or("-").to_string())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_else(|| "-".to_string())
}

/// Per-service metrics grouped under a project header, heaviest first.
///
/// Source: `metrics/getAllServicesStats` — `networkIn`/`networkOut` are already
/// byte/sec rates, and `serviceName` is correct for compose sub-services.
/// Borrows rather than consumes: this runs on EVERY frame of the Monitor screen,
/// and taking ownership forced the caller to clone the whole dataset each time.
pub fn monitor_rows(services: &[Value]) -> Vec<Vec<String>> {
    let mem = |c: &&Value| num(c, "/memory");
    let mut groups: std::collections::HashMap<String, Vec<&Value>> =
        std::collections::HashMap::new();
    for c in services {
        groups.entry(field(c, "/projectName")).or_default().push(c);
    }
    let mut groups: Vec<(String, Vec<&Value>)> = groups.into_iter().collect();
    let total = |v: &[&Value]| -> f64 { v.iter().map(mem).sum() };
    groups.sort_by(|a, b| total(&b.1).total_cmp(&total(&a.1)));

    let mut rows = Vec::new();
    for (project, mut svcs) in groups {
        svcs.sort_by(|a, b| mem(b).total_cmp(&mem(a)));
        let sum = |ptr: &str| -> f64 { svcs.iter().map(|c| num(c, ptr)).sum() };
        rows.push(vec![
            format!("{project} ({})", svcs.len()),
            format!("{:.1}%", sum("/cpu")),
            format_bytes(sum("/memory")),
            format_rate(sum("/networkIn")),
            format_rate(sum("/networkOut")),
        ]);
        for c in svcs {
            rows.push(vec![
                format!("  {}", field(c, "/serviceName")),
                format!("{:.1}%", num(c, "/cpu")),
                format_bytes(num(c, "/memory")),
                format_rate(num(c, "/networkIn")),
                format_rate(num(c, "/networkOut")),
            ]);
        }
    }
    rows
}

pub const MONITOR_HEADERS: [&str; 5] =
    ["Project / Service", "CPU %", "Memory", "Net In", "Net Out"];

/// Storage table rows, sorted largest first.
pub fn storage_rows(items: &[Value]) -> Vec<Vec<String>> {
    let mut arr: Vec<&Value> = items.iter().collect();
    arr.sort_by(|a, b| num(b, "/size").total_cmp(&num(a, "/size")));
    arr.iter()
        .map(|s| {
            vec![
                field(s, "/projectName"),
                field(s, "/serviceName"),
                format_bytes(num(s, "/size")),
                field(s, "/path"),
            ]
        })
        .collect()
}

pub const STORAGE_HEADERS: [&str; 4] = ["Project", "Service", "Size", "Path"];

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn svc(project: &str, name: &str, mem: f64, cpu: f64) -> Value {
        json!({
            "projectName": project, "serviceName": name,
            "cpu": cpu, "memory": mem, "networkIn": 1024.0, "networkOut": 2048.0
        })
    }

    #[test]
    fn monitor_groups_by_project_and_sorts_by_memory() {
        let rows = monitor_rows(&[
            svc("small", "a", 10.0, 0.1),
            svc("big", "tiny", 1.0, 0.2),
            svc("big", "huge", 1_073_741_824.0, 0.5),
        ]);

        // The project with the largest total memory comes first, then its
        // services sorted by memory.
        let names: Vec<&str> = rows.iter().map(|r| r[0].as_str()).collect();
        assert_eq!(
            names,
            vec!["big (2)", "  huge", "  tiny", "small (1)", "  a"]
        );
        // Project row = the total across its services.
        assert_eq!(rows[0][1], "0.7%");
        assert_eq!(rows[0][4], "4.0 KB/s"); // 2048*2
    }

    #[test]
    fn monitor_formats_memory_and_rates() {
        let rows = monitor_rows(&[svc("p", "s", 1_073_741_824.0, 12.34)]);
        assert_eq!(rows[1][0], "  s");
        assert_eq!(rows[1][1], "12.3%");
        assert_eq!(rows[1][2], "1.0 GB");
        assert_eq!(rows[1][3], "1.0 KB/s");
    }

    #[test]
    fn storage_rows_sorted_by_size_desc() {
        let rows = storage_rows(&[
            json!({ "projectName": "p", "serviceName": "tiny", "size": 1024, "path": "/a" }),
            json!({ "projectName": "p", "serviceName": "huge", "size": 1048576, "path": "/b" }),
        ]);
        assert_eq!(rows[0][1], "huge");
        assert_eq!(rows[0][2], "1.0 MB");
        assert_eq!(rows[1][1], "tiny");
    }
}
