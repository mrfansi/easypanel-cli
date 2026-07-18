use ratatui::prelude::*;
use ratatui::widgets::{
    Block, Clear, Gauge, List, ListItem, Paragraph, Row, Sparkline, Table, TableState, Tabs,
};
use serde_json::Value;

use crate::commands;
use crate::output::{field, format_bytes, format_rate, num, series_last, series_spark};

use super::app::*;
use super::form::*;
use super::table::*;

// ---------- Keybinding (satu sumber untuk baris status dan overlay bantuan) ----------

/// Satu keybinding: tombol + artinya.
pub(super) struct Key(pub(super) &'static str, pub(super) &'static str);

/// Tombol yang berlaku di layar mana pun.
pub(super) const GLOBAL_KEYS: &[Key] = &[
    Key("1-7 / Tab", "pindah tab"),
    Key("?", "bantuan ini"),
    Key("s", "daftar server (pilih/tambah/edit/hapus)"),
    Key("r", "refresh"),
    Key("Esc", "batal: tutup form/dropdown/konfirmasi/filter"),
    Key("q / Ctrl-C", "keluar"),
];

/// Tombol khusus sebuah layar.
///
/// Baris status memakai beberapa entri PERTAMA dari daftar yang sama, jadi ia
/// tak bisa menyimpang dari bantuan: dua daftar terpisah pasti akan berbeda
/// seiring waktu, dan bantuan yang berbohong lebih buruk daripada tak ada.
pub(super) fn screen_keys(screen: Screen) -> &'static [Key] {
    match screen {
        Screen::Dashboard => &[],
        Screen::Hosts => &[Key("↑↓", "pilih host")],
        Screen::Maintenance => &[
            Key("p", "prune sistem Docker"),
            Key("i", "hapus image tak terpakai"),
            Key("c", "hapus build cache"),
        ],
        Screen::Actions => &[
            Key("/", "cari"),
            Key("↑↓", "pilih"),
            Key("PgUp/PgDn", "lompat"),
        ],
        Screen::Monitor => &[
            Key("/", "cari"),
            Key("v", "ganti Services / Storage"),
            Key("↑↓", "pilih"),
        ],
        Screen::Domains => &[
            Key("/", "cari"),
            Key("n", "domain baru"),
            Key("e", "edit domain"),
            Key("x", "hapus domain"),
            Key("P", "jadikan primary"),
            Key("↑↓", "pilih"),
        ],
        Screen::Projects => &[
            Key("/", "cari service"),
            Key("g", "cari kata di log SEMUA service"),
            Key("t", "terminal ke dalam container"),
            Key("Enter", "logs"),
            Key("n", "service baru"),
            Key("x", "hapus service"),
            Key("d", "deploy"),
            Key("R", "restart"),
            Key("S", "stop"),
            Key("T", "start"),
            Key("e", "lihat env"),
            Key("p", "lihat ports"),
            Key("m", "lihat mounts"),
            Key("o", "lihat domains"),
            Key("b", "lihat backups"),
            Key("u", "lihat source & build"),
            Key("E", "edit env di $EDITOR"),
            Key("A", "auto deploy on/off (source GitHub)"),
            Key("P", "tambah port"),
            Key("U", "atur source (service app)"),
            Key("B", "atur build (service app)"),
            Key("L", "atur limit resource (CPU/memory)"),
            Key("N", "project baru"),
            Key("X", "hapus project"),
        ],
        Screen::Viewer => &[
            Key("↑↓ / PgUp/PgDn", "scroll (melepas ikut-baris-terakhir)"),
            Key("End", "ikuti baris terakhir lagi (log)"),
            Key("[0-9]", "hapus port itu (di tampilan Ports)"),
            Key("Esc", "kembali ke Services"),
        ],
        Screen::Terminal => &[Key("Ctrl-Q", "keluar terminal (atau ketik `exit`)")],
    }
}

/// Tombol di dalam overlay; berlaku di form dan dropdown mana pun.
pub(super) const OVERLAY_KEYS: &[Key] = &[
    Key("Tab / ↑↓", "pindah field"),
    Key("Enter", "simpan"),
    Key(
        "Spasi / ←→",
        "buka dropdown, ubah field ya/tidak, atau buka $EDITOR",
    ),
    Key("ketik", "saring isi dropdown"),
    Key("Esc", "batal"),
];

// ---------- Render ----------

pub(super) fn ui(f: &mut Frame, app: &mut App) {
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(0),
        // Dua baris: pesan status di baris sendiri (biar error panjang terbaca
        // utuh), keybindings di bawahnya — bukan berdesak di satu baris.
        Constraint::Length(2),
    ])
    .split(f.area());

    render_tabs(f, chunks[0], app);
    match app.screen {
        Screen::Dashboard => render_dashboard(f, chunks[1], app),
        Screen::Hosts => render_hosts(f, chunks[1], app),
        Screen::Maintenance => render_maintenance(f, chunks[1], app),
        Screen::Actions => render_actions(f, chunks[1], app),
        Screen::Monitor => render_monitor(f, chunks[1], app),
        Screen::Domains => render_domains(f, chunks[1], app),
        Screen::Projects => render_projects(f, chunks[1], app),
        Screen::Viewer => render_viewer(f, chunks[1], app),
        Screen::Terminal => render_terminal(f, chunks[1], app),
    }
    render_status(f, chunks[2], app);

    if let Some(c) = &app.confirm {
        render_confirm(f, c);
    }
    if app.picker.is_some() {
        render_picker(f, app);
    }
    if let Some(form) = &app.form {
        render_form(f, form);
    }
    if let Some(ch) = app.chooser.as_mut() {
        render_chooser(f, ch);
    }
    if app.help {
        render_help(f, app);
    }
}

/// Overlay bantuan: tombol global, tombol layar aktif, dan tombol di dalam form.
pub(super) fn render_help(f: &mut Frame, app: &App) {
    let rows = screen_keys(app.screen);
    let area = centered(66, 92, f.area());
    f.render_widget(Clear, area);

    let head = |t: &str| {
        Line::from(Span::styled(
            format!(" {t}"),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ))
    };
    let row = |Key(k, d): &Key| {
        Line::from(vec![
            Span::styled(
                format!("   {k:<12}"),
                Style::default().fg(Color::Indexed(252)),
            ),
            Span::styled((*d).to_string(), Style::default().fg(Color::Gray)),
        ])
    };

    let mut lines = vec![head(&format!("{} — layar ini", TABS[app.screen.index()]))];
    if rows.is_empty() {
        lines.push(Line::from("   (tak ada tombol khusus)"));
    }
    lines.extend(rows.iter().map(row));
    lines.push(Line::from(""));
    lines.push(head("Di mana saja"));
    lines.extend(GLOBAL_KEYS.iter().map(row));
    lines.push(Line::from(""));
    lines.push(head("Di dalam form & dropdown"));
    lines.extend(OVERLAY_KEYS.iter().map(row));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "   tekan tombol apa saja untuk menutup",
        Style::default().fg(Color::DarkGray),
    )));

    f.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .title(" Bantuan ")
                .border_style(Style::default().fg(Color::Yellow)),
        ),
        area,
    );
}

pub(super) fn render_tabs(f: &mut Frame, area: Rect, app: &App) {
    let tabs = Tabs::new(TABS.to_vec())
        .select(app.screen.index())
        .block(Block::bordered().title(format!(" EasyPanel — {} ", app.server_name)))
        .highlight_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        );
    f.render_widget(tabs, area);
}

pub(super) fn render_dashboard(f: &mut Frame, area: Rect, app: &App) {
    let stats = app.stats.clone().unwrap_or(Value::Null);

    let rows = Layout::vertical([Constraint::Length(11), Constraint::Min(0)]).split(area);
    let top =
        Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)]).split(rows[0]);

    let left = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Length(2),
    ])
    .split(top[0]);
    render_gauge(f, left[0], "CPU", series_last(&stats, "cpu"));
    render_gauge(f, left[1], "Memory", series_last(&stats, "memory"));
    render_gauge(f, left[2], "Disk", series_last(&stats, "disk"));
    f.render_widget(
        Paragraph::new(format!(
            " {} cores — load {}",
            field(&stats, "/cpuCores"),
            commands::load_avg(&stats)
        )),
        left[3],
    );

    let spark = Sparkline::default()
        .block(Block::bordered().title(" CPU History (%) "))
        .data(series_spark(&stats, "cpu", 120))
        .max(100)
        .style(Style::default().fg(Color::Cyan));
    f.render_widget(spark, top[1]);

    render_nodes(f, rows[1], app);
}

pub(super) fn render_gauge(f: &mut Frame, area: Rect, label: &str, pct: f64) {
    let g = Gauge::default()
        .block(Block::bordered().title(format!(" {label} ")))
        .gauge_style(Style::default().fg(gauge_color(pct)))
        .ratio((pct / 100.0).clamp(0.0, 1.0))
        .label(format!("{pct:.1}%"));
    f.render_widget(g, area);
}

pub(super) fn render_nodes(f: &mut Frame, area: Rect, app: &App) {
    let header = Row::new(["Hostname", "Role", "State", "Availability", "Addr"])
        .style(Style::default().add_modifier(Modifier::BOLD));
    let rows = app.nodes.iter().map(|n| {
        Row::new([
            field(n, "/Description/Hostname"),
            field(n, "/Spec/Role"),
            field(n, "/Status/State"),
            field(n, "/Spec/Availability"),
            field(n, "/Status/Addr"),
        ])
    });
    let table = Table::new(
        rows,
        [
            Constraint::Percentage(30),
            Constraint::Percentage(15),
            Constraint::Percentage(15),
            Constraint::Percentage(20),
            Constraint::Percentage(20),
        ],
    )
    .header(header)
    .block(Block::bordered().title(" Nodes "));
    f.render_widget(table, area);
}

pub(super) fn render_projects(f: &mut Frame, area: Rect, app: &mut App) {
    // (cells, is_down): baris turun diwarnai merah agar "apa yang rusak" langsung
    // terlihat. Indexed(9), bukan Color::Red bernama: tema terminal pernah membuat
    // warna bernama tak terbaca di proyek ini (lihat AGENT_BRIEF).
    let rows: Vec<(Vec<String>, bool)> = app
        .visible_rows()
        .iter()
        .map(|r| match r {
            // Header project: agregat anak-anaknya, seperti tab Monitor. Hitungan
            // (n) membuat project kosong terlihat sebagai (0), bukan hilang.
            Line2::Project { name, services } => {
                let mets: Vec<&Value> = services
                    .iter()
                    .filter_map(|s| app.metric_for(&field(s, "/projectName"), &field(s, "/name")))
                    .collect();
                (project_row(name, services.len(), &mets), false)
            }
            Line2::Service(s) => {
                let (project, service) = (field(s, "/projectName"), field(s, "/name"));
                // Status jalan/mati dari metrik: ada metrik = jalan. Tapi jangan
                // menuduh "berhenti" sebelum metrik pertama dimuat (monitor kosong)
                // — saat itu jatuh ke enabled saja (None).
                let running = if app.monitor.is_empty() {
                    None
                } else {
                    Some(app.metric_for(&project, &service).is_some())
                };
                let replicas = app.replicas(&project, &service);
                let is_down = matches!(replicas, Some((a, d)) if d > 0 && a < d);
                let mut row = service_row(s, running, replicas);
                // Kolom Project dilebur ke header; service cukup menjorok di bawahnya.
                let name = format!("  {}", row.remove(1));
                row.remove(0);
                let mut out = vec![name];
                out.extend(row);
                out.extend(metric_cols(app.metric_for(&project, &service)));
                (out, is_down)
            }
        })
        .collect();
    let total = app.all_services.len();
    let shown = rows.iter().filter(|(r, _)| r[0].starts_with("  ")).count();
    let mut title = count_title("Services", shown, total, app);
    let down = app.down_count();
    if down > 0 {
        title.push_str(&format!(" · ⚠ {down} turun"));
    }

    let widths = [
        Constraint::Min(26),
        Constraint::Length(8),
        // 9, bukan 7: "berhenti" (8 huruf) harus muat, tak terpotong.
        Constraint::Length(9),
        Constraint::Min(16),
        Constraint::Length(5),
        Constraint::Length(8),
        Constraint::Length(10),
        Constraint::Length(11),
        Constraint::Length(11),
    ];
    let header = Row::new(SERVICE_HEADERS.to_vec()).style(
        Style::default()
            .fg(Color::Black)
            .bg(Color::Green)
            .add_modifier(Modifier::BOLD),
    );
    let down_style = Style::default()
        .fg(Color::Indexed(9))
        .add_modifier(Modifier::BOLD);
    let body = rows.into_iter().map(|(cells, is_down)| {
        let row = Row::new(cells);
        if is_down {
            row.style(down_style)
        } else {
            row
        }
    });
    let table = Table::new(body, widths)
        .header(header)
        .block(Block::bordered().title(title))
        .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("› ");
    f.render_stateful_widget(table, area, &mut app.services_table);
}

pub(super) fn render_table(
    f: &mut Frame,
    area: Rect,
    title: String,
    headers: &[&str],
    widths: &[Constraint],
    rows: Vec<Vec<String>>,
    state: &mut TableState,
) {
    let header = Row::new(headers.to_vec()).style(
        Style::default()
            .fg(Color::Black)
            .bg(Color::Green)
            .add_modifier(Modifier::BOLD),
    );
    let table = Table::new(rows.into_iter().map(Row::new), widths.to_vec())
        .header(header)
        .block(Block::bordered().title(title))
        .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("› ");
    f.render_stateful_widget(table, area, state);
}

/// Info server + pembersihan Docker. Aksinya destruktif dan tak bisa dibatalkan,
/// jadi tombolnya ditulis apa adanya beserta akibatnya, bukan disamarkan.
pub(super) fn render_maintenance(f: &mut Frame, area: Rect, app: &App) {
    let mut lines: Vec<Line> = vec![
        Line::from(Span::styled(
            format!(" Server aktif: {}", app.server_name),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];
    if app.maint.is_empty() {
        lines.push(Line::from("  memuat…"));
    }
    for (k, v) in &app.maint {
        lines.push(Line::from(vec![
            Span::styled(format!("  {k:<24}"), Style::default().fg(Color::DarkGray)),
            Span::raw(v.clone()),
        ]));
    }
    lines.extend([
        Line::from(""),
        Line::from(Span::styled(
            "  Pembersihan (tak bisa dibatalkan, minta konfirmasi dulu)",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from("    [p] prune sistem — container, network, image, build cache tak terpakai"),
        Line::from("    [i] hapus image Docker tak terpakai"),
        Line::from("    [c] hapus build cache Docker"),
    ]);
    f.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(" Maintenance ")),
        area,
    );
}

/// Judul tabel: sebutkan filter yang sedang aktif beserta berapa yang tersaring.
/// Filter yang tak terlihat lebih buruk daripada tak ada filter — user akan
/// mengira baris yang hilang itu memang tak ada.
pub(super) fn count_title(name: &str, shown: usize, total: usize, app: &App) -> String {
    if app.filter.is_empty() && !app.filter_input {
        return format!(" {name} ({total}) ");
    }
    let cursor = if app.filter_input { "▏" } else { "" };
    format!(" {name} ({shown}/{total})  /{}{cursor} ", app.filter)
}

/// Semua host sekaligus. Baris diwarnai per status karena inti layar ini adalah
/// menemukan host bermasalah sekilas — error yang tampil sewarna teks biasa
/// justru terlewat.
pub(super) fn render_hosts(f: &mut Frame, area: Rect, app: &mut App) {
    let rows: Vec<Row> = app
        .hosts
        .iter()
        .map(|h| {
            let (cells, style) = match &h.state {
                HostState::Loading => (
                    vec![
                        h.name.clone(),
                        "memuat…".into(),
                        String::new(),
                        String::new(),
                        String::new(),
                        String::new(),
                        h.url.clone(),
                    ],
                    Style::default().fg(Color::DarkGray),
                ),
                HostState::Err(e) => (
                    vec![
                        h.name.clone(),
                        format!("MATI — {}", crate::output::first_line(e, 40)),
                        "-".into(),
                        "-".into(),
                        "-".into(),
                        "-".into(),
                        h.url.clone(),
                    ],
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                ),
                HostState::Ok(v) => {
                    let pair = |used: &str, total: &str| {
                        format!(
                            "{} / {}",
                            format_bytes(num(v, used)),
                            format_bytes(num(v, total))
                        )
                    };
                    let cpu = series_last(v, "cpu");
                    (
                        vec![
                            h.name.clone(),
                            "ok".into(),
                            format!("{cpu:.1}%"),
                            pair("/memoryUsedBytes", "/memoryTotalBytes"),
                            pair("/diskUsedBytes", "/diskTotalBytes"),
                            // loadAvg bukan deret berstempel-waktu seperti cpu/memory:
                            // isinya tiga string rata-rata 1/5/15 menit. series_last()
                            // mencari p[1] di tiap titik, tak menemukannya, lalu
                            // mengembalikan 0.00 — angka salah yang tampak meyakinkan.
                            commands::load_avg(v),
                            h.url.clone(),
                        ],
                        // Host sehat tak perlu menarik perhatian.
                        Style::default(),
                    )
                }
            };
            Row::new(cells).style(style)
        })
        .collect();

    let header = Row::new(vec![
        "Server", "Status", "CPU", "Memory", "Disk", "Load", "URL",
    ])
    .style(
        Style::default()
            .fg(Color::Black)
            .bg(Color::Green)
            .add_modifier(Modifier::BOLD),
    );
    let table = Table::new(
        rows,
        vec![
            Constraint::Length(14),
            Constraint::Min(16),
            Constraint::Length(7),
            Constraint::Length(19),
            Constraint::Length(19),
            Constraint::Length(18),
            Constraint::Length(30),
        ],
    )
    .header(header)
    .block(Block::bordered().title(format!(" Hosts ({}) ", app.hosts.len())))
    .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED))
    .highlight_symbol("› ");
    f.render_stateful_widget(table, area, &mut app.hosts_state);
}

pub(super) fn render_actions(f: &mut Frame, area: Rect, app: &mut App) {
    let rows: Vec<Vec<String>> = app
        .visible_actions()
        .iter()
        .map(|a| commands::action_row(a, commands::ACTION_DESC_TUI))
        .collect();
    let title = count_title("Actions", rows.len(), app.actions.len(), app);
    render_table(
        f,
        area,
        title,
        &commands::ACTION_HEADERS,
        &[
            Constraint::Length(8),
            Constraint::Length(28),
            Constraint::Min(20),
            Constraint::Length(10),
            Constraint::Length(14),
        ],
        rows,
        &mut app.actions_state,
    );
}

pub(super) fn render_domains(f: &mut Frame, area: Rect, app: &mut App) {
    let rows: Vec<Vec<String>> = app
        .visible_domains()
        .iter()
        .map(|d| commands::domain_row(d))
        .collect();
    let title = count_title("Domains", rows.len(), app.domains.len(), app);
    render_table(
        f,
        area,
        title,
        &commands::DOMAIN_HEADERS,
        &[
            Constraint::Percentage(45),
            Constraint::Percentage(37),
            Constraint::Percentage(18),
        ],
        rows,
        &mut app.domains_state,
    );
}

pub(super) fn render_monitor(f: &mut Frame, area: Rect, app: &mut App) {
    let rows = Layout::vertical([Constraint::Length(8), Constraint::Min(0)]).split(area);
    render_tiles(f, rows[0], app);

    match app.monitor_view {
        MonitorView::Services => {
            let data = app.visible_monitor_rows();
            let total = commands::monitor_rows(app.monitor.clone()).len();
            let title = format!(
                "{}· [v] Storage ",
                count_title("Services", data.len(), total, app)
            );
            render_table(
                f,
                rows[1],
                title,
                &commands::MONITOR_HEADERS,
                &[
                    Constraint::Min(20),
                    Constraint::Length(9),
                    Constraint::Length(11),
                    Constraint::Length(11),
                    Constraint::Length(11),
                ],
                data,
                &mut app.monitor_state,
            );
        }
        MonitorView::Storage => {
            let data = commands::storage_rows(app.storage.clone());
            render_table(
                f,
                rows[1],
                format!(" Storage ({}) · [v] Services ", app.storage.len()),
                &commands::STORAGE_HEADERS,
                &[
                    Constraint::Length(20),
                    Constraint::Length(18),
                    Constraint::Length(11),
                    Constraint::Min(20),
                ],
                data,
                &mut app.monitor_state,
            );
        }
    }
}

/// Lima tile metrik dengan histori (CPU, Memory, Disk, Net In, Net Out).
pub(super) fn render_tiles(f: &mut Frame, area: Rect, app: &App) {
    let s = app.stats.clone().unwrap_or(Value::Null);
    let pair = |used: &str, total: &str| {
        format!(
            "{} / {}",
            format_bytes(num(&s, used)),
            format_bytes(num(&s, total))
        )
    };

    let tiles = [
        (
            "CPU",
            format!("{:.1}%", series_last(&s, "cpu")),
            format!(
                "{} cores — load {}",
                field(&s, "/cpuCores"),
                commands::load_avg(&s)
            ),
            series_spark(&s, "cpu", 60),
            Color::Yellow,
        ),
        (
            "Memory",
            format!("{:.1}%", series_last(&s, "memory")),
            pair("/memoryUsedBytes", "/memoryTotalBytes"),
            series_spark(&s, "memory", 60),
            Color::Blue,
        ),
        (
            "Disk",
            format!("{:.1}%", series_last(&s, "disk")),
            pair("/diskUsedBytes", "/diskTotalBytes"),
            series_spark(&s, "disk", 60),
            Color::Green,
        ),
        (
            "Network In",
            format_rate(series_last(&s, "networkIn")),
            String::new(),
            series_spark(&s, "networkIn", 60),
            Color::Cyan,
        ),
        (
            "Network Out",
            format_rate(series_last(&s, "networkOut")),
            String::new(),
            series_spark(&s, "networkOut", 60),
            Color::Magenta,
        ),
    ];

    let cols = Layout::horizontal([Constraint::Ratio(1, 5); 5]).split(area);
    for (i, (label, value, sub, data, color)) in tiles.into_iter().enumerate() {
        let inner = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(cols[i].inner(Margin::new(1, 1)));
        f.render_widget(Block::bordered().title(format!(" {label} ")), cols[i]);
        f.render_widget(
            Paragraph::new(value).style(Style::default().add_modifier(Modifier::BOLD)),
            inner[0],
        );
        f.render_widget(
            Paragraph::new(sub).style(Style::default().fg(Color::DarkGray)),
            inner[1],
        );
        f.render_widget(
            Sparkline::default()
                .data(data)
                .max(100)
                .style(Style::default().fg(color)),
            inner[2],
        );
    }
}

/// Terminal container tertanam: gambar grid emulator vt100 di pane, dan jaga
/// ukuran shell mengikuti ukuran pane (resize dua arah).
pub(super) fn render_terminal(f: &mut Frame, area: Rect, app: &mut App) {
    let block = Block::bordered().title(format!(" Terminal · {} · Ctrl-Q keluar ", app.term_title));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let (cols, rows) = (inner.width.max(1), inner.height.max(1));
    let Some(parser) = app.term_parser.as_mut() else {
        return;
    };
    // Selaraskan ukuran shell dengan pane. vt100 pakai (rows, cols).
    if parser.screen().size() != (rows, cols) {
        parser.set_size(rows, cols);
        if let Some(tx) = app.term_input.as_ref() {
            let _ = tx.send(super::terminal::TermMsg::Resize(cols, rows));
        }
    }

    let screen = parser.screen();
    let buf = f.buffer_mut();
    for r in 0..rows {
        for c in 0..cols {
            let Some(cell) = screen.cell(r, c) else {
                continue;
            };
            let x = inner.x + c;
            let y = inner.y + r;
            let contents = cell.contents();
            let ch = if contents.is_empty() { " " } else { &contents };
            let mut style = Style::default()
                .fg(vt_color(cell.fgcolor()))
                .bg(vt_color(cell.bgcolor()));
            if cell.bold() {
                style = style.add_modifier(Modifier::BOLD);
            }
            if cell.italic() {
                style = style.add_modifier(Modifier::ITALIC);
            }
            if cell.underline() {
                style = style.add_modifier(Modifier::UNDERLINED);
            }
            if cell.inverse() {
                style = style.add_modifier(Modifier::REVERSED);
            }
            if let Some(target) = buf.cell_mut((x, y)) {
                target.set_symbol(ch).set_style(style);
            }
        }
    }

    // Kursor shell (kalau tak disembunyikan).
    if !screen.hide_cursor() {
        let (cr, cc) = screen.cursor_position();
        if cr < rows && cc < cols {
            f.set_cursor_position((inner.x + cc, inner.y + cr));
        }
    }
}

/// vt100 → warna ratatui.
fn vt_color(c: vt100::Color) -> Color {
    match c {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(i) => Color::Indexed(i),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

pub(super) fn render_viewer(f: &mut Frame, area: Rect, app: &mut App) {
    // Tinggi baru diketahui saat render, jadi posisi "menempel di bawah" dihitung
    // di sini — bukan di handler, yang tak tahu sebesar apa layarnya.
    if app.viewer_follow {
        let rows = area.height.saturating_sub(2);
        app.viewer_scroll = (app.viewer_lines.len() as u16).saturating_sub(rows);
    }
    f.render_widget(
        Paragraph::new(app.viewer_lines.join("\n"))
            .block(Block::bordered().title(format!(
                " {}{} ",
                app.viewer_title,
                // Katakan kalau memang hidup. Tanpa ini, log yang diam tak bisa
                // dibedakan dari tail yang mati.
                match (app.log_cursor.is_some(), app.viewer_follow) {
                    (true, true) => " · live",
                    (true, false) => " · live (dijeda — End untuk ikut lagi)",
                    _ => "",
                }
            )))
            .scroll((app.viewer_scroll, 0)),
        area,
    );
}

/// Susun baris tombol yang MUAT di lebar `width`: sebanyak mungkin tombol layar,
/// lalu selalu "? bantuan · q keluar" di ujung. Menyerah di batas "·", tak pernah
/// memotong tengah kata, dan tak pernah melebihi lebar — jadi baris keybinding tak
/// bisa meluber walau terminal disempitkan.
pub(super) fn fit_status_keys(screen: &[Key], width: u16) -> String {
    let tail = ["? bantuan", "q keluar"];
    let tail_w = tail.join(" · ").chars().count() + 3; // " · " sebelum tail
    let head_w = 1; // spasi pembuka " " di baris keybinding
    let avail = (width as usize).saturating_sub(head_w + tail_w);

    let mut shown: Vec<String> = Vec::new();
    let mut used = 0;
    for Key(k, d) in screen {
        let seg = format!("{k} {d}");
        let add = seg.chars().count() + if shown.is_empty() { 0 } else { 3 };
        if used + add > avail {
            break;
        }
        used += add;
        shown.push(seg);
    }
    shown.extend(tail.iter().map(|s| s.to_string()));
    shown.join(" · ")
}

pub(super) fn render_status(f: &mut Frame, area: Rect, app: &App) {
    // Warna bernama (Color::Blue) ditafsirkan tema terminal dan bisa jadi biru
    // terang, sehingga teks putih di atasnya nyaris tak terbaca. Indeks palet
    // memberi abu-abu gelap yang pasti.
    let bar = Style::default().bg(Color::Indexed(238)).fg(Color::White);

    if app.filter_input {
        // Saat mengetik filter, tombol layar tak berlaku — jangan tampilkan yang
        // tidak akan bekerja. Baris kedua dibiarkan kosong (tetap berwarna bar).
        f.render_widget(
            Paragraph::new(vec![
                Line::from(vec![
                    Span::styled(" filter: ", bar.fg(Color::Indexed(252))),
                    Span::styled(format!("{}▏", app.filter), bar.add_modifier(Modifier::BOLD)),
                    Span::styled("  Enter pakai · Esc batal", bar.fg(Color::Indexed(244))),
                ]),
                Line::from(""),
            ])
            .style(bar),
            area,
        );
        return;
    }

    // Dua baris: pesan status di baris SENDIRI supaya error/hasil yang panjang
    // terbaca utuh — dulu ia berbagi baris dengan tombol dan terpotong di tepi.
    // Keybinding di baris kedua, muat-lebar (yang tak muat pindah ke overlay "?").
    let is_error = app.status.starts_with("Error") || app.status.contains("gagal");
    let status_style = if is_error {
        // Merah muda palet: kontras di atas abu-abu, tak bergantung tema.
        bar.fg(Color::Indexed(210)).add_modifier(Modifier::BOLD)
    } else {
        bar.add_modifier(Modifier::BOLD)
    };
    let keys = fit_status_keys(screen_keys(app.screen), area.width);
    f.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(format!(" {} ", app.status), status_style)),
            Line::from(Span::styled(
                format!(" {keys}"),
                bar.fg(Color::Indexed(252)),
            )),
        ])
        .style(bar),
        area,
    );
}

pub(super) fn render_form(f: &mut Frame, form: &Form) {
    // Hanya field langkah sekarang yang tampil (satu-halaman = semuanya).
    let visible = form.visible_here();
    let height = (visible.len() as u16 + 5).min(f.area().height);
    let area = centered_abs(64, height, f.area());
    f.render_widget(Clear, area);
    // Judul menyebut langkah agar wizard tak terasa seperti form yang terpotong.
    let steps = form.steps_present();
    let title = if form.is_wizard() {
        let at = steps
            .iter()
            .position(|&s| s as usize == form.step)
            .unwrap_or(0);
        let label = match form.step {
            0 => "Dasar",
            1 => "Source",
            2 => "Build",
            3 => "Environment",
            _ => "Domains",
        };
        format!(
            "{} — {}/{} {}",
            form.title.trim(),
            at + 1,
            steps.len(),
            label
        )
    } else {
        form.title.clone()
    };
    f.render_widget(
        Block::bordered()
            .title(title)
            .border_style(Style::default().fg(Color::Cyan)),
        area,
    );

    let inner = area.inner(Margin::new(2, 1));
    let mut rows = vec![Constraint::Length(1); visible.len()];
    rows.push(Constraint::Min(1));
    let slots = Layout::vertical(rows).split(inner);

    for (slot, &idx) in visible.iter().enumerate() {
        let field = &form.fields[idx];
        let focused = idx == form.focus;
        let hint = match (focused, &field.kind) {
            (true, FieldKind::Bool) => "  ⌄ Spasi untuk ubah",
            (true, FieldKind::Choice(_)) => "  ⌄ Spasi untuk pilih",
            (true, FieldKind::Editor) => "  ⌄ Spasi untuk buka di $EDITOR",
            _ => "",
        };
        let line = Line::from(vec![
            Span::styled(
                format!("{:<14}", field.label),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(
                format!("{}{}", field.shown(), if focused { "▏" } else { "" }),
                if focused {
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                },
            ),
            Span::styled(hint, Style::default().fg(Color::DarkGray)),
        ]);
        f.render_widget(Paragraph::new(line), slots[slot]);
    }

    // Footer menyesuaikan langkah: Enter "lanjut" sampai langkah terakhir, dan
    // Esc "kembali" sampai langkah pertama.
    let footer = if form.is_wizard() {
        let enter = if form.next_present_step().is_some() {
            "lanjut →"
        } else {
            "buat service"
        };
        let esc = if form.prev_present_step().is_some() {
            "← kembali"
        } else {
            "batal"
        };
        format!("[Enter] {enter}   [Esc] {esc}   [Tab] pindah field   [Spasi] pilih")
    } else {
        "[Spasi] pilih   [Enter] simpan   [Tab] pindah field   [Esc] batal".to_string()
    };
    f.render_widget(
        Paragraph::new(footer).style(Style::default().fg(Color::DarkGray)),
        slots[visible.len()],
    );
}

pub(super) fn render_chooser(f: &mut Frame, ch: &mut Chooser) {
    let items = ch.matches();
    let height = (items.len() as u16 + 4).clamp(5, 16);
    let area = centered_abs(48, height, f.area());
    f.render_widget(Clear, area);

    let title = if ch.filter.is_empty() {
        format!(" {} — ketik untuk mencari ", ch.label)
    } else {
        format!(" {} — cari: {} ", ch.label, ch.filter)
    };
    let list = List::new(items.into_iter().map(ListItem::new).collect::<Vec<_>>())
        .block(
            Block::bordered()
                .title(title)
                .border_style(Style::default().fg(Color::Cyan)),
        )
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("› ");
    f.render_stateful_widget(list, area, &mut ch.state);
}

pub(super) fn render_confirm(f: &mut Frame, c: &Confirm) {
    let area = centered(52, 22, f.area());
    f.render_widget(Clear, area);
    // Sebutkan target sebenarnya. Kalimat "Memengaruhi service nyata" dulu
    // dipasang untuk semua konfirmasi — keliru untuk aksi maintenance, yang
    // justru mengenai seluruh host, bukan satu service.
    let target = match (c.project.as_str(), c.service.as_str()) {
        ("", _) => "Memengaruhi SELURUH host.".to_string(),
        (p, "") => format!("Target: {p}"),
        (p, s) => format!("Target: {p}/{s}"),
    };
    f.render_widget(
        Paragraph::new(format!(
            "\n{}\n\n{target}\n\n[y] Ya      [n] Batal",
            c.label
        ))
        .alignment(Alignment::Center)
        .block(
            Block::bordered()
                .title(" Konfirmasi ")
                .border_style(Style::default().fg(Color::Yellow)),
        ),
        area,
    );
}

pub(super) fn render_picker(f: &mut Frame, app: &mut App) {
    let area = centered(46, 50, f.area());
    f.render_widget(Clear, area);
    let items: Vec<ListItem> = app
        .all_servers
        .iter()
        .map(|(n, url)| {
            let mark = if n == &app.server_name {
                " (aktif)"
            } else {
                ""
            };
            // URL ikut ditampilkan: nama saja tak cukup untuk memastikan host mana
            // yang akan diedit atau dihapus.
            ListItem::new(Line::from(vec![
                Span::raw(format!("{n}{mark}  ")),
                Span::styled(url.clone(), Style::default().fg(Color::DarkGray)),
            ]))
        })
        .collect();
    let list = List::new(items)
        .block(
            Block::bordered()
                .title(" Server: Enter pilih · n baru · e edit · x hapus ")
                .border_style(Style::default().fg(Color::Yellow)),
        )
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("› ");
    // Dipanggil hanya saat picker Some (lihat ui()), tapi total menghindari
    // panic kalau urutan itu berubah: tanpa state, cukup gambar list tanpa sorot.
    if let Some(state) = app.picker.as_mut() {
        f.render_stateful_widget(list, area, state);
    } else {
        f.render_widget(list, area);
    }
}

// ---------- Helpers ----------

pub(super) fn gauge_color(pct: f64) -> Color {
    if pct < 70.0 {
        Color::Green
    } else if pct < 90.0 {
        Color::Yellow
    } else {
        Color::Red
    }
}

pub(super) fn cap(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(first) => first.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// Overlay dengan lebar persen dan tinggi baris tetap.
pub(super) fn centered_abs(pct_x: u16, height: u16, r: Rect) -> Rect {
    let pad = r.height.saturating_sub(height) / 2;
    let v = Layout::vertical([
        Constraint::Length(pad),
        Constraint::Length(height),
        Constraint::Min(0),
    ])
    .split(r);
    Layout::horizontal([
        Constraint::Percentage((100 - pct_x) / 2),
        Constraint::Percentage(pct_x),
        Constraint::Percentage((100 - pct_x) / 2),
    ])
    .split(v[1])[1]
}

pub(super) fn centered(pct_x: u16, pct_y: u16, r: Rect) -> Rect {
    let v = Layout::vertical([
        Constraint::Percentage((100 - pct_y) / 2),
        Constraint::Percentage(pct_y),
        Constraint::Percentage((100 - pct_y) / 2),
    ])
    .split(r);
    Layout::horizontal([
        Constraint::Percentage((100 - pct_x) / 2),
        Constraint::Percentage(pct_x),
        Constraint::Percentage((100 - pct_x) / 2),
    ])
    .split(v[1])[1]
}
