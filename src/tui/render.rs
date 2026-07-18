use ratatui::prelude::*;
use ratatui::widgets::{
    Block, Cell, Clear, Gauge, List, ListItem, Paragraph, Row, Sparkline, Table, TableState,
};
use serde_json::Value;

use crate::commands;
use crate::output::{field, format_bytes, format_rate, num, series_last, series_spark};

use super::app::*;
use super::form::*;
use super::table::*;

// ---------- Keybindings (one source for the status bar and the help overlay) ----------

/// One keybinding: the key + what it means.
pub(super) struct Key(pub(super) &'static str, pub(super) &'static str);

/// Keys that apply on any screen.
pub(super) const GLOBAL_KEYS: &[Key] = &[
    Key("1-7 / Tab / ←→", "switch tab"),
    Key("?", "this help"),
    Key(
        ":",
        "global search: jump to a service/tab; quick actions (deploy/restart/logs/…) for the selected service",
    ),
    Key("s", "server list (select/add/edit/delete)"),
    Key("r", "refresh"),
    Key("Esc", "cancel: close form/dropdown/confirmation/filter"),
    Key("q / Ctrl-C", "quit"),
];

/// Keys specific to a screen.
///
/// The status bar uses the FIRST few entries of this same list, so it can't drift
/// from the help: two separate lists would inevitably diverge over time, and help
/// that lies is worse than no help.
pub(super) fn screen_keys(screen: Screen) -> &'static [Key] {
    match screen {
        Screen::Dashboard => &[],
        Screen::Hosts => &[Key("↑↓", "select host")],
        Screen::Maintenance => &[
            Key("p", "prune Docker system"),
            Key("i", "remove unused images"),
            Key("c", "remove build cache"),
        ],
        Screen::Actions => &[
            Key("/", "search"),
            Key("↑↓", "select"),
            Key("PgUp/PgDn", "jump"),
        ],
        Screen::Monitor => &[
            Key("/", "search"),
            Key("v", "switch Services / Storage"),
            Key("↑↓", "select"),
        ],
        Screen::Domains => &[
            Key("/", "search"),
            Key("n", "new domain"),
            Key("e", "edit domain"),
            Key("x", "delete domain"),
            Key("P", "set primary"),
            Key("↑↓", "select"),
        ],
        // Related actions are grouped into MENUS (one key → a list), not 25 loose
        // keys. Inside a menu: ↑↓ select, Enter run, Esc close. The old leaf keys
        // (E/w/./P/f/F/H/U/B/A/L/M/R/S/T/X) still work once you know them.
        Screen::Projects => &[
            Key("/", "search services"),
            Key("Enter", "logs"),
            Key("e", "Env menu — view / edit / replace / .env file"),
            Key("o", "Networking menu — domain / port / redirect / auth"),
            Key(
                "u",
                "Build & source menu — source / build / auto / resource",
            ),
            Key("m", "Storage menu — mounts / backups"),
            Key("d", "Lifecycle menu — deploy / restart / stop / start"),
            Key("t", "Shell menu — terminal / DB shell"),
            Key("x", "Danger menu — delete service / project"),
            Key("p", "view ports"),
            Key("b", "view backups"),
            Key("y", "DB shell (auto login)"),
            Key("c", "clone service (config, not data)"),
            Key("g", "search a word in the logs of ALL services"),
            Key("n", "new service"),
            Key("N", "new project"),
            Key("↑↓", "select row"),
            Key(
                "Space / right click",
                "open the action menu for the selected row",
            ),
            Key(
                "in a menu: ↑↓ →←",
                "→ enter submenu · ← back · Enter run · Esc close",
            ),
        ],
        Screen::Viewer => &[
            Key("↑↓ / PgUp/PgDn", "scroll (releases follow-last-line)"),
            Key("End", "follow the last line again (logs)"),
            Key("[0-9]", "delete that line (Ports/Mounts/Redirects)"),
            Key("Esc", "back to Services"),
        ],
        Screen::Terminal => &[Key("Ctrl-Q", "exit terminal (or type `exit`)")],
    }
}

/// Mouse actions (shown in the help overlay so they're discoverable).
pub(super) const MOUSE_KEYS: &[Key] = &[
    Key("Click tab", "switch to that tab"),
    Key("Click row", "select the row"),
    Key("Right click", "action menu for that row"),
    Key("Scroll", "scroll the table / viewer"),
];

/// Keys inside the overlay; apply in any form and dropdown.
pub(super) const OVERLAY_KEYS: &[Key] = &[
    Key("Tab / ↑↓", "move between fields"),
    Key("Enter", "save"),
    Key(
        "Space / ←→",
        "open a dropdown, toggle a yes/no field, or open $EDITOR",
    ),
    Key("type", "filter the dropdown contents"),
    Key("Esc", "cancel"),
];

// ---------- Render ----------

pub(super) fn ui(f: &mut Frame, app: &mut App) {
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(0),
        // A single status line: just the message. The full key list is in the "?"
        // overlay.
        Constraint::Length(1),
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
    if let Some(form) = app.form.as_mut() {
        render_form(f, form);
    }
    if let Some(ch) = app.chooser.as_mut() {
        render_chooser(f, ch);
    }
    if app.help {
        render_help(f, app);
    }
    if app.menu.is_some() {
        render_menu(f, app);
    }
    if app.palette.is_some() {
        render_palette(f, app);
    }
}

/// Command palette (global search): a query line + the filtered list.
pub(super) fn render_palette(f: &mut Frame, app: &mut App) {
    let Some(pal) = app.palette.as_mut() else {
        return;
    };
    let area = centered(60, 70, f.area());
    f.render_widget(Clear, area);
    let matches = pal.matches();
    let count = matches.len();
    // The highlight indexes into the FILTERED list; keep it in range.
    if let Some(sel) = pal.state.selected() {
        if sel >= count {
            pal.state.select(Some(count.saturating_sub(1)));
        }
    }
    let items: Vec<ListItem> = matches
        .iter()
        .map(|&i| ListItem::new(format!("  {}", pal.items[i].label)))
        .collect();
    pal.rect = area;
    let list = List::new(items)
        .block(
            Block::bordered()
                .title(format!(" Search: {}▏ ", pal.query))
                .title_bottom(format!(" {count} results · Enter run/jump · Esc close "))
                .border_style(Style::default().fg(Color::Cyan)),
        )
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
    f.render_stateful_widget(list, area, &mut pal.state);
}

/// Right-click context menu: a small popup at the cursor, clamped to stay on screen.
pub(super) fn render_menu(f: &mut Frame, app: &mut App) {
    let full = f.area();
    let Some(menu) = app.menu.as_mut() else {
        return;
    };
    let w = (menu
        .items
        .iter()
        .map(|it| it.label.chars().count())
        .max()
        .unwrap_or(6) as u16
        + 4)
    .min(full.width);
    let h = (menu.items.len() as u16 + 2).min(full.height);
    let x = menu.col.min(full.width.saturating_sub(w));
    let y = menu.row.min(full.height.saturating_sub(h));
    let rect = Rect {
        x,
        y,
        width: w,
        height: h,
    };
    menu.rect = rect;
    let items: Vec<ListItem> = menu
        .items
        .iter()
        .map(|it| ListItem::new(format!(" {}", it.label)))
        .collect();
    let list = List::new(items)
        .block(
            Block::bordered()
                .title(" Actions ")
                .border_style(Style::default().fg(Color::Yellow)),
        )
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
    f.render_widget(Clear, rect);
    f.render_stateful_widget(list, rect, &mut menu.state);
}

/// Help overlay: global keys, the active screen's keys, and the keys inside forms.
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
    // Key column width = the longest key (across all sections) + 2, so the
    // description never touches the key (e.g. "↑↓ / right click").
    let kw = rows
        .iter()
        .chain(GLOBAL_KEYS)
        .chain(OVERLAY_KEYS)
        .chain(MOUSE_KEYS)
        .map(|Key(k, _)| k.chars().count())
        .max()
        .unwrap_or(12)
        + 2;
    let row = |Key(k, d): &Key| {
        Line::from(vec![
            Span::styled(
                format!("   {k:<kw$}", kw = kw),
                Style::default().fg(Color::Indexed(252)),
            ),
            Span::styled((*d).to_string(), Style::default().fg(Color::Gray)),
        ])
    };

    let mut lines = vec![head(&format!("{} — this screen", TABS[app.screen.index()]))];
    if rows.is_empty() {
        lines.push(Line::from("   (no dedicated keys)"));
    }
    lines.extend(rows.iter().map(row));
    lines.push(Line::from(""));
    lines.push(head("Anywhere"));
    lines.extend(GLOBAL_KEYS.iter().map(row));
    lines.push(Line::from(""));
    lines.push(head("Inside forms & dropdowns"));
    lines.extend(OVERLAY_KEYS.iter().map(row));
    lines.push(Line::from(""));
    lines.push(head("Mouse"));
    for m in MOUSE_KEYS {
        lines.push(row(m));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "   press any key to close",
        Style::default().fg(Color::DarkGray),
    )));

    f.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .title(" Help ")
                .border_style(Style::default().fg(Color::Yellow)),
        ),
        area,
    );
}

pub(super) fn render_tabs(f: &mut Frame, area: Rect, app: &mut App) {
    // Drawn by hand (not the Tabs widget) so each tab has a definite column hitbox
    // for mouse clicks, and the active tab can briefly "flash" when it changes.
    let block = Block::bordered().title(format!(" EasyPanel — {} ", app.server_name));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let active = app.screen.index();
    let tab_fresh = app.tab_at.elapsed().as_millis() < 300;
    let mut spans = Vec::new();
    let mut hits = Vec::new();
    let mut x = inner.x;
    for (i, title) in TABS.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("│", Style::default().fg(Color::DarkGray)));
            x += 1;
        }
        let label = format!(" {title} ");
        let w = label.chars().count() as u16;
        let style = if i == active {
            let base = Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD);
            // Flash: a newly selected tab inverts its colors briefly, then settles.
            if tab_fresh {
                base.add_modifier(Modifier::REVERSED)
            } else {
                base
            }
        } else {
            Style::default().fg(Color::Gray)
        };
        spans.push(Span::styled(label, style));
        hits.push((x, x + w));
        x += w;
    }
    f.render_widget(Paragraph::new(Line::from(spans)), inner);
    app.tab_spans = hits;
    app.tab_row = inner.y;
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
    // (cells, is_down): down rows are painted red so "what's broken" is
    // immediately visible. Indexed(9), not the named Color::Red: a terminal theme
    // once made named colors unreadable in this project (see AGENT_BRIEF).
    let rows: Vec<(Vec<String>, bool)> = app
        .visible_rows()
        .iter()
        .map(|r| match r {
            // Project header: an aggregate of its children, like the Monitor tab.
            // The (n) count makes an empty project show as (0), not vanish.
            Line2::Project { name, services } => {
                let mets: Vec<&Value> = services
                    .iter()
                    .filter_map(|s| app.metric_for(&field(s, "/projectName"), &field(s, "/name")))
                    .collect();
                (project_row(name, services.len(), &mets), false)
            }
            Line2::Service(s) => {
                let (project, service) = (field(s, "/projectName"), field(s, "/name"));
                // Up/down from metrics: metrics present = up. But don't accuse it of
                // being "stopped" before the first metrics load (monitor empty) — at
                // that point fall back to enabled alone (None).
                let running = if app.monitor.is_empty() {
                    None
                } else {
                    Some(app.metric_for(&project, &service).is_some())
                };
                let replicas = app.replicas(&project, &service);
                let deploying = app.is_deploying(&project, &service);
                // A running deploy wins over "down": the old container is still
                // there, this is the expected state, not an incident — don't pulse
                // red. The Status col (index 3 in the full row) is overwritten with
                // "deploying".
                let is_down = !deploying && matches!(replicas, Some((a, d)) if d > 0 && a < d);
                let mut row = service_row(s, running, replicas);
                if deploying {
                    row[3] = "deploying".into();
                }
                // The Project column is folded into the header; the service just
                // indents beneath it.
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
        title.push_str(&format!(" · ⚠ {down} down"));
    }
    let deploying = app.deploying_count();
    if deploying > 0 {
        title.push_str(&format!(" · ⚙ {deploying} deploying"));
    }

    let widths = [
        Constraint::Min(26),
        Constraint::Length(8),
        // 11: fits "● stopped" (dot + space + 7 letters) without truncation.
        Constraint::Length(11),
        // 5: fits "12/12"; the header is abbreviated because this table is already
        // wide and the column is a short number.
        Constraint::Length(5),
        Constraint::Min(16),
        Constraint::Length(5),
        Constraint::Length(8),
        Constraint::Length(10),
        Constraint::Length(11),
        Constraint::Length(11),
    ];
    // Under ~120 columns the four metric columns collapse into unreadable slivers
    // ("0.", "77", "2.") and squeeze the identity columns with them — "Status"
    // became "Statu", "● active" became "● act". Drop them instead: who and what
    // state stays here, the numbers already live on the Monitor tab. Only trailing
    // columns go, so the per-cell colour indices below are unaffected.
    let cols = if area.width < 120 {
        SERVICE_HEADERS.len() - 4
    } else {
        SERVICE_HEADERS.len()
    };
    let widths = &widths[..cols];
    let header = Row::new(SERVICE_HEADERS[..cols].to_vec()).style(
        Style::default()
            .fg(Color::Black)
            .bg(Color::Green)
            .add_modifier(Modifier::BOLD),
    );
    // "down" rows pulse (bright red <-> salmon) so the eye is drawn to what's
    // broken; this is an incident state, so pulling attention to it is apt.
    let down_style = pulse_red(app.anim.elapsed().as_millis());
    // The status dot (column 2) & the Auto mark (column 4) get their own per-cell
    // color: the state reads at a glance. "down" rows are left to inherit the red
    // pulse.
    let body = rows.into_iter().map(|(mut cells, is_down)| {
        cells.truncate(cols);
        let cells: Vec<Cell> = cells
            .into_iter()
            .enumerate()
            .map(|(i, c)| match i {
                2 => status_cell(&c, is_down),
                5 => auto_cell(&c, is_down),
                _ => Cell::from(c),
            })
            .collect();
        let row = Row::new(cells);
        if is_down {
            row.style(down_style)
        } else {
            row
        }
    });
    // Selection flash: a newly selected row (click/arrow) is bolded briefly, then
    // settles to plain reversed. A cell grid can't slide between rows, so a
    // "transition" in the terminal is a brief emphasis, not smooth motion.
    let hl = if app.nav_at.elapsed().as_millis() < 220 {
        Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
    } else {
        Style::default().add_modifier(Modifier::REVERSED)
    };
    app.table_area = area;
    let table = Table::new(body, widths)
        .header(header)
        .block(Block::bordered().title(title))
        .row_highlight_style(hl)
        .highlight_symbol("› ");
    f.render_stateful_widget(table, area, &mut app.services_table);
}

/// Status cell with a `●` dot colored per state: active green, stopped yellow,
/// disabled gray, down red (colored by the row pulse). A 1-cell BMP dot,
/// theme-proof. Non-status rows (project headers, "-") get no dot.
fn status_cell(text: &str, is_down: bool) -> Cell<'static> {
    let color = match text {
        "active" => Some(Color::Indexed(2)),
        "stopped" => Some(Color::Indexed(3)),
        "disabled" => Some(Color::Indexed(8)),
        // "deploying": blue-cyan, distinct from up/down states — in progress.
        "deploying" => Some(Color::Indexed(6)),
        _ => None,
    };
    if color.is_none() && text != "down" {
        return Cell::from(text.to_string());
    }
    let cell = Cell::from(format!("● {text}"));
    match color {
        // "down": let the row's red pulse color it (the dot goes red too).
        Some(c) if !is_down => cell.style(Style::default().fg(c)),
        _ => cell,
    }
}

/// Auto cell: ✓ green (on), ✗ gray (off), "-" as-is (not applicable).
fn auto_cell(text: &str, is_down: bool) -> Cell<'static> {
    let color = match text {
        "✓" => Some(Color::Indexed(2)),
        "✗" => Some(Color::Indexed(8)),
        _ => None,
    };
    let cell = Cell::from(text.to_string());
    match color {
        Some(c) if !is_down => cell.style(Style::default().fg(c)),
        _ => cell,
    }
}

/// The pulse color for a "down" row: a 4-step cycle ~1.1 seconds between bright
/// red and salmon. Palette indices, not named colors (theme-proof).
fn pulse_red(ms: u128) -> Style {
    const SHADES: [u8; 4] = [196, 203, 210, 203];
    let c = SHADES[((ms / 280) % 4) as usize];
    Style::default()
        .fg(Color::Indexed(c))
        .add_modifier(Modifier::BOLD)
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

/// Server info + Docker cleanup. The actions are destructive and irreversible, so
/// the keys are written plainly along with their consequences, not disguised.
pub(super) fn render_maintenance(f: &mut Frame, area: Rect, app: &App) {
    let mut lines: Vec<Line> = vec![
        Line::from(Span::styled(
            format!(" Active server: {}", app.server_name),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];
    if app.maint.is_empty() {
        lines.push(Line::from("  loading…"));
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
            "  Cleanup (irreversible, asks for confirmation first)",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from("    [p] prune system — unused containers, networks, images, build cache"),
        Line::from("    [i] remove unused Docker images"),
        Line::from("    [c] remove the Docker build cache"),
    ]);
    f.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(" Maintenance ")),
        area,
    );
}

/// Table title: name the active filter and how many rows it keeps. An invisible
/// filter is worse than no filter — the user would assume the missing rows simply
/// don't exist.
pub(super) fn count_title(name: &str, shown: usize, total: usize, app: &App) -> String {
    if app.filter.is_empty() && !app.filter_input {
        return format!(" {name} ({total}) ");
    }
    let cursor = if app.filter_input { "▏" } else { "" };
    format!(" {name} ({shown}/{total})  /{}{cursor} ", app.filter)
}

/// Every host at once. Rows are colored by status because the point of this screen
/// is spotting a troubled host at a glance — an error shown in the same color as
/// ordinary text gets missed.
pub(super) fn render_hosts(f: &mut Frame, area: Rect, app: &mut App) {
    let rows: Vec<Row> = app
        .hosts
        .iter()
        .map(|h| {
            let (cells, style) = match &h.state {
                HostState::Loading => (
                    vec![
                        h.name.clone(),
                        "loading…".into(),
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
                        format!("DOWN — {}", crate::output::first_line(e, 40)),
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
                            // loadAvg isn't a timestamped series like cpu/memory:
                            // it's three strings, the 1/5/15-minute averages.
                            // series_last() looks for p[1] at each point, doesn't
                            // find it, then returns 0.00 — a convincing wrong number.
                            commands::load_avg(v),
                            h.url.clone(),
                        ],
                        // A healthy host needn't draw attention.
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
    app.table_area = area;
    f.render_stateful_widget(table, area, &mut app.hosts_state);
}

pub(super) fn render_actions(f: &mut Frame, area: Rect, app: &mut App) {
    let rows: Vec<Vec<String>> = app
        .visible_actions()
        .iter()
        .map(|a| commands::action_row(a, commands::ACTION_DESC_TUI))
        .collect();
    let title = count_title("Actions", rows.len(), app.actions.len(), app);
    app.table_area = area;
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
    app.table_area = area;
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
    app.table_area = rows[1];

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

/// Five metric tiles with history (CPU, Memory, Disk, Net In, Net Out).
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

/// Embedded container terminal: draw the vt100 emulator grid in the pane, and keep
/// the shell's size in step with the pane size (two-way resize).
pub(super) fn render_terminal(f: &mut Frame, area: Rect, app: &mut App) {
    let block = Block::bordered().title(format!(" Terminal · {} · Ctrl-Q exit ", app.term_title));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let (cols, rows) = (inner.width.max(1), inner.height.max(1));
    let Some(parser) = app.term_parser.as_mut() else {
        return;
    };
    // Keep the shell's size aligned with the pane. vt100 uses (rows, cols).
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

    // The shell cursor (unless hidden).
    if !screen.hide_cursor() {
        let (cr, cc) = screen.cursor_position();
        if cr < rows && cc < cols {
            f.set_cursor_position((inner.x + cc, inner.y + cr));
        }
    }
}

/// vt100 → ratatui color.
fn vt_color(c: vt100::Color) -> Color {
    match c {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(i) => Color::Indexed(i),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

pub(super) fn render_viewer(f: &mut Frame, area: Rect, app: &mut App) {
    // The height is only known at render, so the "stick to the bottom" position is
    // computed here — not in the handler, which doesn't know how big the screen is.
    if app.viewer_follow {
        let rows = area.height.saturating_sub(2);
        app.viewer_scroll = (app.viewer_lines.len() as u16).saturating_sub(rows);
    }
    f.render_widget(
        Paragraph::new(app.viewer_lines.join("\n"))
            .block(Block::bordered().title(format!(
                " {}{} ",
                app.viewer_title,
                // Say so if it's really live. Without this, a quiet log can't be
                // told apart from a dead tail.
                match (app.log_cursor.is_some(), app.viewer_follow) {
                    (true, true) => " · live",
                    (true, false) => " · live (paused — End to follow again)",
                    _ => "",
                }
            )))
            .scroll((app.viewer_scroll, 0)),
        area,
    );
}

pub(super) fn render_status(f: &mut Frame, area: Rect, app: &App) {
    // A named color (Color::Blue) is interpreted by the terminal theme and can come
    // out bright blue, leaving the white text on top barely readable. A palette
    // index gives a definite dark gray.
    let bar = Style::default().bg(Color::Indexed(238)).fg(Color::White);

    if app.filter_input {
        // While typing a filter, show how to apply/cancel it (contextual, not the
        // full key list — that's in the "?" overlay).
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(" filter: ", bar.fg(Color::Indexed(252))),
                Span::styled(format!("{}▏", app.filter), bar.add_modifier(Modifier::BOLD)),
                Span::styled("  Enter apply · Esc cancel", bar.fg(Color::Indexed(244))),
            ]))
            .style(bar),
            area,
        );
        return;
    }

    // A single line: just the status message. The key list is removed from here —
    // it's fully in the "?" overlay (and that extra line cost one table row).
    let is_error = app.status.starts_with("Error") || app.status.contains("failed");
    let status_style = if is_error {
        // Palette pink: contrasts on top of the gray, theme-independent.
        bar.fg(Color::Indexed(210)).add_modifier(Modifier::BOLD)
    } else {
        bar.add_modifier(Modifier::BOLD)
    };
    // A spinner while an operation is running: signals "working", not frozen.
    let head = match app.spinner() {
        Some(c) => format!(" {c} {} ", app.status),
        None => format!(" {} ", app.status),
    };
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(head, status_style))).style(bar),
        area,
    );
}

pub(super) fn render_form(f: &mut Frame, form: &mut Form) {
    // Only the current step's fields show (single-page = all of them).
    let visible = form.visible_here();
    let height = (visible.len() as u16 + 5).min(f.area().height);
    let area = centered_abs(64, height, f.area());
    f.render_widget(Clear, area);
    // The title names the step so a wizard doesn't feel like a cut-off form.
    let steps = form.steps_present();
    let title = if form.is_wizard() {
        let at = steps
            .iter()
            .position(|&s| s as usize == form.step)
            .unwrap_or(0);
        let label = match form.step {
            0 => "Basics",
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
    form.rect = inner;
    let mut rows = vec![Constraint::Length(1); visible.len()];
    rows.push(Constraint::Min(1));
    let slots = Layout::vertical(rows).split(inner);

    // Label column width = the longest label + 2 spaces, so the value never touches
    // the label (e.g. "Create .env file" / "Install command").
    let lw = visible
        .iter()
        .map(|&i| form.fields[i].label.chars().count())
        .max()
        .unwrap_or(0)
        + 2;
    for (slot, &idx) in visible.iter().enumerate() {
        let field = &form.fields[idx];
        let focused = idx == form.focus;
        let hint = match (focused, &field.kind) {
            (true, FieldKind::Bool) => "  ⌄ Space to toggle",
            (true, FieldKind::Choice(_)) => "  ⌄ Space to choose",
            (true, FieldKind::Editor) => "  ⌄ Space to open in $EDITOR",
            _ => "",
        };
        let line = Line::from(vec![
            Span::styled(
                format!("{:<lw$}", field.label, lw = lw),
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

    // The footer adapts to the step: Enter "next" until the last step, and Esc
    // "back" until the first step.
    let footer = if form.is_wizard() {
        let enter = if form.next_present_step().is_some() {
            "next →"
        } else {
            "create service"
        };
        let esc = if form.prev_present_step().is_some() {
            "← back"
        } else {
            "cancel"
        };
        format!("[Enter] {enter}   [Esc] {esc}   [Tab] move field   [Space] choose")
    } else {
        "[Space] choose   [Enter] save   [Tab] move field   [Esc] cancel".to_string()
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
    ch.rect = area;
    f.render_widget(Clear, area);

    let title = if ch.filter.is_empty() {
        format!(" {} — type to search ", ch.label)
    } else {
        format!(" {} — search: {} ", ch.label, ch.filter)
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
    // Name the actual target. The line "Affects a real service" used to be shown on
    // every confirmation — wrong for a maintenance action, which affects the whole
    // host, not a single service.
    let target = match (c.project.as_str(), c.service.as_str()) {
        ("", _) => "Affects the ENTIRE host.".to_string(),
        (p, "") => format!("Target: {p}"),
        (p, s) => format!("Target: {p}/{s}"),
    };
    f.render_widget(
        Paragraph::new(format!(
            "\n{}\n\n{target}\n\n[y] Yes      [n] Cancel",
            c.label
        ))
        .alignment(Alignment::Center)
        .block(
            Block::bordered()
                .title(" Confirm ")
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
                " (active)"
            } else {
                ""
            };
            // The URL is shown too: the name alone isn't enough to be sure which
            // host is about to be edited or deleted.
            ListItem::new(Line::from(vec![
                Span::raw(format!("{n}{mark}  ")),
                Span::styled(url.clone(), Style::default().fg(Color::DarkGray)),
            ]))
        })
        .collect();
    let list = List::new(items)
        .block(
            Block::bordered()
                .title(" Server: Enter select · n new · e edit · x delete ")
                .border_style(Style::default().fg(Color::Yellow)),
        )
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("› ");
    // Only called while the picker is Some (see ui()), but taking avoids a panic if
    // that order ever changes: without state, just draw the list without a highlight.
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

/// An overlay with a percentage width and a fixed row height.
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
