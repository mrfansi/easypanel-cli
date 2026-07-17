use ratatui::crossterm::event::KeyCode;
use ratatui::widgets::TableState;
use serde_json::Value;

use crate::output::{field, format_bytes, format_rate, num};

pub(super) const SERVICE_HEADERS: [&str; 9] = [
    "Project / Service",
    "Type",
    "Status",
    "Source",
    "Auto",
    "CPU %",
    "Memory",
    "Net In",
    "Net Out",
];

/// Satu baris tabel Services: header project, atau service di bawahnya.
///
/// Hirarki dipertahankan tapi tetap satu tabel: drill-down memaksa membuka
/// project satu per satu dan tak bisa dicari, sementara daftar datar tanpa
/// header membuat project kosong hilang sama sekali — tak terlihat, tak bisa
/// dipilih, tak bisa dihapus.
pub(super) enum Line2<'a> {
    Project {
        name: &'a str,
        services: Vec<&'a Value>,
    },
    Service(&'a Value),
}

/// Satu baris tabel service datar.
///
/// `source` diringkas dari inspectService-nya listProjectsAndServices, jadi repo
/// dan branch terlihat tanpa membuka apa pun.
pub(super) fn service_row(s: &Value, running: Option<bool>) -> Vec<String> {
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
        service_status(s, running).into(),
        source,
        auto_deploy_cell(s).into(),
    ]
}

/// Status jalan/mati sebuah service.
///
/// `enabled` dari API cuma berarti "tidak di-disable user", BUKAN "container
/// hidup" — service yang crash tetap enabled. Sinyal jalan sebenarnya adalah
/// apakah ia punya metrik (getAllServicesStats hanya memuat yang berjalan). Jadi
/// `running`: Some(true)=ada metrik, Some(false)=tak ada (padahal enabled → mati
/// beneran), None=metrik belum dimuat / konteks filter (jangan menuduh mati).
///
/// - "mati": di-disable user (enabled=false)
/// - "berhenti": enabled tapi tak jalan (crash/stop) — inilah yang dulu bohong
///   menampilkan "aktif"
/// - "aktif": jalan, atau belum bisa dipastikan (metrik belum ada)
pub(super) fn service_status(s: &Value, running: Option<bool>) -> &'static str {
    let enabled = s.get("enabled").and_then(Value::as_bool).unwrap_or(true);
    match (enabled, running) {
        (false, _) => "mati",
        (true, Some(false)) => "berhenti",
        (true, _) => "aktif",
    }
}

/// Kolom Auto: hanya source github yang punya auto deploy.
///
/// Tiga keadaan, bukan dua. Server tak mengirim `autoDeploy` untuk source image
/// maupun database (diperiksa langsung ke API: hadir pada 15 dari 16 app, yang
/// satu itu bersumber image). "✗" di baris MySQL akan berarti "belum" untuk
/// sesuatu yang tak pernah bisa dinyalakan — persis jenis angka yang percaya
/// diri tapi salah.
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

/// Baris header project: agregat metrik anak-anaknya.
///
/// `mets` sudah disaring: hanya service yang metriknya ada. Kalau kosong, tak
/// ada yang diukur — jadi "-", bukan angka. "0.0 %" akan mengklaim sudah diukur
/// dan hasilnya nol, dan tanpa penjaga ini `Sum` untuk f64 (identitasnya -0.0)
/// mencetak "-0.0 %": CPU negatif yang tampak meyakinkan.
pub(super) fn project_row(name: &str, count: usize, mets: &[&Value]) -> Vec<String> {
    let mut row: Vec<String> = vec![format!("{name} ({count})")];
    row.extend(["-", "-", "-", "-"].map(String::from));
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

/// Kolom metrik untuk sebuah service; "-" bila metriknya belum/tak ada.
///
/// Dipisah dari service_row() supaya filter hanya mencocokkan identitas
/// (project/service/tipe/source) — mencari "1" tak seharusnya cocok ke setiap
/// baris hanya karena angka CPU-nya.
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

/// Apakah sebuah baris lolos filter.
///
/// Dicocokkan ke teks yang DITAMPILKAN, bukan ke JSON mentahnya: yang dicari user
/// adalah yang terlihat di layar.
pub(super) fn keep(row: &[String], filter: &str) -> bool {
    if filter.is_empty() {
        return true;
    }
    let f = filter.to_lowercase();
    row.iter().any(|c| c.to_lowercase().contains(&f))
}

/// Pilih baris pertama bila daftar terisi dan belum ada yang dipilih.
pub(super) fn select_first(state: &mut TableState, len: usize) {
    if len == 0 {
        state.select(None);
    } else if state.selected().is_none() {
        state.select(Some(0));
    }
}

/// Navigasi tabel: panah/jk, PgUp/PgDn, Home/End.
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
        "GitHub menolak: 403"
    } else if err.contains("401") || err.contains("Unauthorized") {
        "GitHub menolak: token tidak valid"
    } else {
        "gagal"
    }
}
