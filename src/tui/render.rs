use ratatui::prelude::*;
use ratatui::widgets::{
    Block, Cell, Clear, Gauge, List, ListItem, Paragraph, Row, Sparkline, Table, TableState, Wrap,
};
use serde_json::Value;

use crate::commands;
use crate::output::{
    field, format_bytes, format_rate, num, series_last, series_percent, series_spark,
};

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
        Screen::Hosts => &[
            Key("↑↓", "select host"),
            Key(
                "Enter",
                "host detail — the full reason a host is unreachable",
            ),
        ],
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
            Key(
                "v",
                "mark this service for a bulk action (on a project header: all of its services)",
            ),
            Key("V", "mark every service the filter shows"),
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
            Key(
                "n / e / b / x",
                "act on what is shown — the keys for THIS view are listed on its bottom border",
            ),
            Key(
                "↑↓",
                "select a row (ports/mounts/redirects) · scroll (logs, env, source)",
            ),
            Key("PgUp/PgDn", "scroll (releases follow-last-line)"),
            Key("←→", "scroll sideways — long lines are not wrapped"),
            Key("Home", "back to the first line and the left edge"),
            Key("End", "follow the last line again (logs)"),
            Key("Esc", "back to Services"),
        ],
        Screen::Terminal => &[
            Key("Ctrl-Q", "exit terminal (or type `exit`)"),
            Key(
                "Shift+PgUp/PgDn · wheel",
                "scroll back through this session's output",
            ),
        ],
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
                .title(match &pal.context {
                    // Named once here, not on every row.
                    Some(c) => format!(" Search: {}▏  ·  actions for {c} ", pal.query),
                    None => format!(" Search: {}▏ ", pal.query),
                })
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
    // Clear one column to the RIGHT of the popup. Without it the row underneath
    // continues hard against the border — "┐d", "│dio-db (5)" — which reads as
    // corrupted text rather than a menu floating over the table. Only the right:
    // the column to the left carries the "›" marker for the row this menu acts on,
    // and blanking that would drop the very context the menu belongs to.
    let gutter = Rect {
        x: rect.x,
        y: rect.y,
        width: (rect.width + 1).min(full.width.saturating_sub(rect.x)),
        height: rect.height,
    };
    f.render_widget(Clear, gutter);
    f.render_stateful_widget(list, rect, &mut menu.state);
}

/// Help overlay: global keys, the active screen's keys, and the keys inside forms.
/// Break `text` into lines of at most `width` columns, never mid-word.
///
/// The help is a two-column table, so wrapping has to happen here rather than via
/// `Paragraph::wrap`: that would restart the continuation at column 0 and lose the
/// alignment that makes the list scannable.
pub(super) fn wrap_words(text: &str, width: usize) -> Vec<String> {
    let width = width.max(8);
    let mut out = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        // A single token longer than the pane (a URL, a stack frame) has no space
        // to break at, so it is cut into pieces rather than left to overflow the
        // edge — which is the whole failure this wrapping exists to avoid.
        if word.chars().count() > width {
            if !line.is_empty() {
                out.push(std::mem::take(&mut line));
            }
            let mut chars = word.chars().peekable();
            while chars.peek().is_some() {
                out.push(chars.by_ref().take(width).collect());
            }
            continue;
        }
        if line.is_empty() {
            line = word.to_string();
        } else if line.chars().count() + 1 + word.chars().count() <= width {
            line.push(' ');
            line.push_str(word);
        } else {
            out.push(std::mem::take(&mut line));
            line = word.to_string();
        }
    }
    if !line.is_empty() {
        out.push(line);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

pub(super) fn render_help(f: &mut Frame, app: &mut App) {
    let rows = screen_keys(app.screen);
    let area = centered(72, 92, f.area());
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
        // Capped: one long key ("Space / right click") used to push every
        // description into a ~26-column gutter where they all truncated.
        .min(16)
        + 2;
    let desc_w = (area.width as usize).saturating_sub(3 + kw + 2).max(12);
    let row = move |Key(k, d): &Key| -> Vec<Line<'static>> {
        wrap_words(d, desc_w)
            .into_iter()
            .enumerate()
            .map(|(i, part)| {
                // Continuation lines sit under the description, not under the key.
                let head = if i == 0 {
                    format!("   {k:<kw$}", kw = kw)
                } else {
                    " ".repeat(3 + kw)
                };
                Line::from(vec![
                    Span::styled(head, Style::default().fg(Color::Indexed(252))),
                    Span::styled(part, Style::default().fg(Color::Gray)),
                ])
            })
            .collect()
    };

    let mut lines = vec![head(&format!("{} — this screen", TABS[app.screen.index()]))];
    if rows.is_empty() {
        lines.push(Line::from("   (no dedicated keys)"));
    }
    lines.extend(rows.iter().flat_map(&row));
    lines.push(Line::from(""));
    lines.push(head("Anywhere"));
    lines.extend(GLOBAL_KEYS.iter().flat_map(&row));
    lines.push(Line::from(""));
    lines.push(head("Inside forms & dropdowns"));
    lines.extend(OVERLAY_KEYS.iter().flat_map(&row));
    lines.push(Line::from(""));
    lines.push(head("Mouse"));
    lines.extend(MOUSE_KEYS.iter().flat_map(&row));

    // The help is taller than a short terminal. It used to simply stop at the
    // bottom border — the Anywhere, form and Mouse sections were invisible at 80x24
    // with nothing to say so. Scroll instead, and keep the how-to-leave line in the
    // border where it can't scroll away.
    let inner_h = area.height.saturating_sub(2);
    let max_scroll = (lines.len() as u16).saturating_sub(inner_h);
    app.help_scroll = app.help_scroll.min(max_scroll);
    let hint = if max_scroll > 0 {
        format!(
            " ↑↓ scroll · {}/{} · any other key closes ",
            app.help_scroll + 1,
            max_scroll + 1
        )
    } else {
        " press any key to close ".to_string()
    };

    f.render_widget(
        Paragraph::new(lines).scroll((app.help_scroll, 0)).block(
            Block::bordered()
                .title(" Help ")
                .title_bottom(hint)
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

    // True percentages, on an axis that SAYS what it is. Drawn against a fixed
    // 0-100 an idle host is a flat sliver — truthful but unreadable; drawn
    // against the window's own range (what this used to do) 8% reached the top
    // of a panel titled "(%)". An adaptive ceiling, named in the title, keeps the
    // shape visible without the chart claiming a load that isn't there.
    let cpu = series_percent(&stats, "cpu", 120);
    let ceiling = axis_ceiling(cpu.iter().copied().max().unwrap_or(0));
    let spark = Sparkline::default()
        .block(Block::bordered().title(format!(" CPU History (0–{ceiling}%) ")))
        .data(cpu)
        .max(ceiling)
        .style(Style::default().fg(Color::Cyan));
    f.render_widget(spark, top[1]);

    render_nodes(f, rows[1], app);
}

/// The smallest sensible top-of-axis for a percentage chart peaking at `max`.
///
/// Steps rather than "exactly the peak" so the axis does not jitter with every
/// sample, and so the number in the title stays round enough to read.
pub(super) fn axis_ceiling(max: u64) -> u64 {
    [10, 25, 50, 100]
        .into_iter()
        .find(|c| max <= *c)
        .unwrap_or(100)
}

pub(super) fn render_gauge(f: &mut Frame, area: Rect, label: &str, pct: f64) {
    let g = Gauge::default()
        .block(Block::bordered().title(format!(" {label} ")))
        // The bg matters: ratatui swaps fg/bg for the part of the label that sits
        // ON the filled bar, so leaving bg unset made that half render as the
        // terminal's DEFAULT foreground on green — light on light, unreadable at
        // exactly the moment the number is worth reading.
        .gauge_style(
            Style::default()
                .fg(gauge_color(pct))
                .bg(Color::Indexed(235)),
        )
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
    // Built once for this frame. Looked up per row, these were linear scans:
    // metrics two or three times per service, actions once per service.
    let metrics = app.metric_index();
    let deploying_set = app.deploying_index();
    let lines = app.visible_rows();
    // Counted from the ROWS, not from their text. It used to be derived by
    // testing row[0] for the two-space indent, which quietly made the indent
    // load-bearing: the mark below replaces it with "✓ " on marked services and
    // the header count would have silently dropped every one of them.
    let shown = lines
        .iter()
        .filter(|l| matches!(l, Line2::Service(_)))
        .count();
    let rows: Vec<(Vec<String>, bool)> = lines
        .iter()
        .map(|r| match r {
            // Project header: an aggregate of its children, like the Monitor tab.
            // The (n) count makes an empty project show as (0), not vanish.
            Line2::Project { name, services } => {
                let mets: Vec<&Value> = services
                    .iter()
                    .filter_map(|s| {
                        let p = s.get("projectName").and_then(Value::as_str)?;
                        let n = s.get("name").and_then(Value::as_str)?;
                        metrics.get(&(p, n)).copied()
                    })
                    .collect();
                (project_row(name, services.len(), &mets), false)
            }
            Line2::Service(s) => {
                let (project, service) = (field(s, "/projectName"), field(s, "/name"));
                let metric = metrics.get(&(project.as_str(), service.as_str())).copied();
                // Up/down from metrics: metrics present = up. But don't accuse it of
                // being "stopped" before the first metrics load (monitor empty) — at
                // that point fall back to enabled alone (None).
                let running = if app.monitor.is_empty() {
                    None
                } else {
                    Some(metric.is_some())
                };
                let replicas = app.replicas(&project, &service);
                let deploying = deploying_set.contains(&(project.as_str(), service.as_str()));
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
                // The mark replaces the indent rather than widening the column:
                // a ✓ that shifted every name two columns would make the table
                // jump sideways each time one was marked.
                let indent = if app.is_marked(&project, &service) {
                    "✓ "
                } else {
                    "  "
                };
                let name = format!("{indent}{}", row.remove(1));
                row.remove(0);
                let mut out = vec![name];
                out.extend(row);
                out.extend(metric_cols(metric));
                (out, is_down)
            }
        })
        .collect();
    let total = app.all_services.len();
    let mut title = count_title("Services", shown, total, app);
    // Marks are the one piece of state a user builds up deliberately and can
    // scroll away from; the title is the only place it stays visible.
    if !app.marked.is_empty() {
        title.push_str(&format!(" · ✓ {} marked", app.marked.len()));
    }
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
    const SERVICE_MINS: [u16; SERVICE_HEADERS.len()] = [0, 0, 0, 0, 0, 0, 120, 120, 120, 120];
    let cols = columns_that_fit(&SERVICE_MINS, area.width).len();
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
    /// Width of the label column ("  {k:<24}" minus its leading two spaces).
    const LABEL_W: usize = 24;
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
        // A row that FAILED to load must not read like a value that loaded. It
        // used to render in the terminal's ordinary text colour, identical to a
        // real Docker version sitting directly above it — and this screen offers
        // three irreversible host-wide actions.
        let label = Span::styled(format!("  {k:<24}"), Style::default().fg(Color::DarkGray));
        match v {
            Ok(value) => lines.push(Line::from(vec![label, Span::raw(value.clone())])),
            Err(e) => {
                let style = Style::default()
                    .fg(Color::Indexed(210))
                    .add_modifier(Modifier::BOLD);
                // Wrapped with a hanging indent so the reason stays in the value
                // column. Letting the paragraph wrap it sent the continuation back
                // to column 0, which reads as a new row rather than the rest of
                // this one — on a screen you are looking at BECAUSE it is broken.
                // Two borders + the 26-column label ("  " + 24). Getting this
                // wrong by one made the paragraph re-wrap the indented line and
                // spill a single character onto a row of its own.
                let avail = (area.width as usize).saturating_sub(LABEL_W + 4).max(20);
                let text = format!("could not load — {e}");
                for (i, part) in wrap_words(&text, avail).into_iter().enumerate() {
                    lines.push(if i == 0 {
                        Line::from(vec![label.clone(), Span::styled(part, style)])
                    } else {
                        Line::from(Span::styled(
                            format!("{:<w$}{part}", "", w = LABEL_W + 2),
                            style,
                        ))
                    });
                }
            }
        }
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
        // Wrapped: a transport error and the "[p] prune system — …" consequence
        // line both run past 74 columns, and a destructive key whose stated
        // consequence has been cut off is worse than no help at all.
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(Block::bordered().title(" Maintenance ")),
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
    // The full set needs 123 columns plus the highlight symbol. Squeezed below
    // that, ratatui shrinks every column proportionally — which turned
    // "29.8 GB / 59.0 GB" into "29.8 GB", a figure that reads as complete and is
    // not. Whole columns are dropped instead, least useful first: the URL is
    // something you configured and already know, and Load is the least urgent
    // metric. Status is never dropped — it carries the failure reason.
    // Each threshold is the total area width the column needs, counting what is
    // easy to forget: one space BETWEEN each pair of columns, the two-column
    // highlight symbol, and the two border columns. Guessing these (the first
    // attempt used round numbers) left Disk rendering as "194.7 GB / 784.9".
    const HOST_COLS: &[(u16, Constraint)] = &[
        (0, Constraint::Length(14)),   // Server
        (0, Constraint::Min(16)),      // Status — carries the failure reason
        (0, Constraint::Length(7)),    // CPU
        (0, Constraint::Length(19)),   // Memory
        (83, Constraint::Length(19)),  // Disk
        (102, Constraint::Length(18)), // Load
        (133, Constraint::Length(30)), // URL
    ];
    let cols = columns_that_fit(
        &HOST_COLS.iter().map(|(m, _)| *m).collect::<Vec<_>>(),
        area.width,
    )
    .len();

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
            let mut cells = cells;
            cells.truncate(cols);
            Row::new(cells).style(style)
        })
        .collect();

    let header =
        Row::new(["Server", "Status", "CPU", "Memory", "Disk", "Load", "URL"][..cols].to_vec())
            .style(
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            );
    let table = Table::new(
        rows,
        HOST_COLS
            .iter()
            .take(cols)
            .map(|(_, c)| *c)
            .collect::<Vec<_>>(),
    )
    .header(header)
    .block(Block::bordered().title(format!(" Hosts ({}) ", app.hosts.len())))
    .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED))
    .highlight_symbol("› ");
    app.table_area = area;
    f.render_stateful_widget(table, area, &mut app.hosts_state);
}

pub(super) fn render_actions(f: &mut Frame, area: Rect, app: &mut App) {
    // Status, Target, Description, Duration, Age. The full set needs 88 columns
    // once the spacing between them, the highlight symbol and the borders are
    // counted; below that "Target" was squeezed from 28 to 20 and the service an
    // action happened to became unidentifiable ("harisenin-net-db/php").
    //
    // Duration is the first to go — not Age, which is dropped only when four
    // columns no longer fit. A history screen that cannot say WHEN has lost the
    // point of itself, and how long it took is one keypress away in the detail.
    const ACTION_COLS: &[(u16, Constraint)] = &[
        (0, Constraint::Length(8)),   // Status
        (0, Constraint::Length(28)),  // Target
        (0, Constraint::Min(20)),     // Description
        (88, Constraint::Length(10)), // Duration
        (77, Constraint::Length(14)), // Age
    ];
    let mins: Vec<u16> = ACTION_COLS.iter().map(|(m, _)| *m).collect();
    let idx = columns_that_fit(&mins, area.width);

    let rows: Vec<Vec<String>> = app
        .visible_actions()
        .iter()
        .map(|a| {
            let cells = commands::action_row(a, commands::ACTION_DESC_TUI);
            idx.iter().filter_map(|i| cells.get(*i).cloned()).collect()
        })
        .collect();
    let headers: Vec<&str> = idx
        .iter()
        .filter_map(|i| commands::ACTION_HEADERS.get(*i).copied())
        .collect();
    let widths: Vec<Constraint> = idx.iter().map(|i| ACTION_COLS[*i].1).collect();

    let title = count_title("Actions", rows.len(), app.actions.len(), app);
    app.table_area = area;
    render_table(
        f,
        area,
        title,
        &headers,
        &widths,
        rows,
        &mut app.actions_state,
    );
}

pub(super) fn render_domains(f: &mut Frame, area: Rect, app: &mut App) {
    // Source, Destination, ID. Percentages used to shrink all three together, so
    // a hostname was cut mid-word with nothing to show for it —
    // "https://dashboard.internal.example.com/v1" rendered as
    // "https://dashboard.internal.exampl", which reads as a complete and
    // DIFFERENT host. On the screen that also carries `x delete`, that is the
    // worst possible place to guess.
    //
    // ID goes first when space runs short: it is an opaque cuid nobody types, and
    // at 18% it was too narrow to even show in full. Source is never dropped —
    // it is the whole point of the row.
    const ID_W: u16 = 26;
    const DEST_W: u16 = 34;
    const SRC_MIN: u16 = 30;
    const DOMAIN_COLS: &[(u16, Constraint)] = &[
        (0, Constraint::Min(SRC_MIN)),
        (69, Constraint::Length(DEST_W)),
        (96, Constraint::Length(ID_W)),
    ];
    let mins: Vec<u16> = DOMAIN_COLS.iter().map(|(m, _)| *m).collect();
    let idx = columns_that_fit(&mins, area.width);

    // The width each column will actually get, so a value too long for its column
    // is cut HERE — with an ellipsis — instead of silently at the edge.
    let fixed: u16 = idx
        .iter()
        .skip(1)
        .map(|i| if *i == 1 { DEST_W } else { ID_W })
        .sum();
    let gaps = idx.len().saturating_sub(1) as u16;
    let src_w = area.width.saturating_sub(4 + fixed + gaps).max(SRC_MIN) as usize;
    let widths_px = [src_w, DEST_W as usize, ID_W as usize];

    let rows: Vec<Vec<String>> = app
        .visible_domains()
        .iter()
        .map(|d| {
            let cells = commands::domain_row(d);
            idx.iter()
                .filter_map(|i| {
                    cells
                        .get(*i)
                        .map(|c| crate::output::first_line(c, widths_px[*i]))
                })
                .collect()
        })
        .collect();
    let headers: Vec<&str> = idx
        .iter()
        .filter_map(|i| commands::DOMAIN_HEADERS.get(*i).copied())
        .collect();
    let widths: Vec<Constraint> = idx.iter().map(|i| DOMAIN_COLS[*i].1).collect();

    let title = count_title("Domains", rows.len(), app.domains.len(), app);
    app.table_area = area;
    if rows.is_empty() {
        // A bare bordered box cannot say whether the filter excluded everything
        // or there is genuinely nothing here — and those need different actions.
        let msg = if app.filter.is_empty() {
            "  No domains yet — press n to add one".to_string()
        } else {
            format!("  Nothing matches '{}' — Esc clears the filter", app.filter)
        };
        f.render_widget(
            Paragraph::new(msg)
                .style(Style::default().fg(Color::DarkGray))
                .block(Block::bordered().title(title)),
            area,
        );
        return;
    }
    render_table(
        f,
        area,
        title,
        &headers,
        &widths,
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
            // Built once, and by the SAME rule the rest of the app uses: the rows
            // and the count come out of one pass, so what is drawn here cannot
            // drift from what navigation and the title believe.
            let (data, total) = app.monitor_table();
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
            let data = app.visible_storage_rows();
            let total = commands::storage_rows(&app.storage).len();
            render_table(
                f,
                rows[1],
                format!(
                    "{}· [v] Services ",
                    count_title("Storage", data.len(), total, app)
                ),
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

/// The widest form of a tile's sub-line that actually FITS.
///
/// Truncating it is not cosmetic: five tiles across an 80-column terminal leave
/// 14 usable columns each, and "199.9 GB / 784.9 GB" cut to that reads
/// "199.9 GB / 784" — a total with no unit, off by three orders of magnitude. A
/// shorter TRUE form beats a longer cut one, and nothing beats a wrong number.
pub(super) fn fit_sub(forms: &[String], width: usize) -> String {
    forms
        .iter()
        .find(|s| s.chars().count() <= width)
        .cloned()
        .unwrap_or_default()
}

/// "31.0 GB / 59.0 GB" shortened to "31.0/59.0 GB" when both sides share a unit.
///
/// Keeps BOTH numbers, which is the point — a lone "31.0 GB" reads as a complete
/// figure and hides that it is a half.
pub(super) fn compact_pair(used: &str, total: &str) -> String {
    match (used.rsplit_once(' '), total.rsplit_once(' ')) {
        (Some((un, uu)), Some((tn, tu))) if uu == tu => format!("{un}/{tn} {uu}"),
        _ => format!("{used}/{total}"),
    }
}

/// Five metric tiles with history (CPU, Memory, Disk, Net In, Net Out).
pub(super) fn render_tiles(f: &mut Frame, area: Rect, app: &App) {
    let s = app.stats.clone().unwrap_or(Value::Null);
    // Each pair tile offers a full form and a compact one; the renderer picks
    // whichever fits the tile it ends up with.
    let pair = |used: &str, total: &str| {
        let (u, t) = (format_bytes(num(&s, used)), format_bytes(num(&s, total)));
        vec![format!("{u} / {t}"), compact_pair(&u, &t)]
    };

    let tiles = [
        (
            "CPU",
            format!("{:.1}%", series_last(&s, "cpu")),
            vec![
                format!(
                    "{} cores — load {}",
                    field(&s, "/cpuCores"),
                    commands::load_avg(&s)
                ),
                format!("load {}", commands::load_avg(&s)),
                format!("{} cores", field(&s, "/cpuCores")),
            ],
            series_percent(&s, "cpu", 60),
            Color::Yellow,
        ),
        (
            "Memory",
            format!("{:.1}%", series_last(&s, "memory")),
            pair("/memoryUsedBytes", "/memoryTotalBytes"),
            series_percent(&s, "memory", 60),
            Color::Blue,
        ),
        (
            "Disk",
            format!("{:.1}%", series_last(&s, "disk")),
            pair("/diskUsedBytes", "/diskTotalBytes"),
            series_percent(&s, "disk", 60),
            Color::Green,
        ),
        (
            "Network In",
            format_rate(series_last(&s, "networkIn")),
            Vec::new(),
            series_spark(&s, "networkIn", 60),
            Color::Cyan,
        ),
        (
            "Network Out",
            format_rate(series_last(&s, "networkOut")),
            Vec::new(),
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
            Paragraph::new(fit_sub(&sub, inner[1].width as usize))
                .style(Style::default().fg(Color::DarkGray)),
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
    let rows = area.height.saturating_sub(2);
    let max_scroll = (app.viewer_lines.len() as u16).saturating_sub(rows);
    app.viewer_scroll = if app.viewer_follow {
        max_scroll
    } else {
        // Clamped on EVERY path, not just while following: Down and PageDown add
        // without an upper bound, so holding either used to scroll past the last
        // line into a blank bordered box that looks like an empty log.
        app.viewer_scroll.min(max_scroll)
    };
    let block = |app: &App| {
        Block::bordered()
            .title(format!(
                " {}{} ",
                app.viewer_title,
                // Say so if it's really live. Without this, a quiet log can't be
                // told apart from a dead tail.
                match (app.log_cursor.is_some(), app.viewer_follow) {
                    (true, true) => " · live",
                    (true, false) => " · live (paused — End to follow again)",
                    _ => "",
                }
            ))
            .title_bottom(viewer_actions(app))
            .title_bottom(if app.viewer_hscroll > 0 {
                // Say where you are once scrolled: otherwise a view missing its
                // left edge looks like the content simply starts there.
                format!(" ← col {} · Home to return ", app.viewer_hscroll + 1)
            } else {
                String::new()
            })
    };

    // A collection is a LIST with a highlighted row; everything else is prose.
    // Selecting a line in a log would mean nothing, but selecting a port is the
    // whole point — it is what `x` deletes, without the ten-row ceiling the old
    // "press the digit on the line" had.
    // An empty collection has a PLACEHOLDER line, not a row. Highlighting it made
    // "No ports yet" look like something you had selected and could delete.
    let has_rows = app.viewer_lines.iter().any(|l| is_row(l));
    if has_rows
        && app
            .viewer_ctx
            .as_ref()
            .is_some_and(|(v, ..)| v.is_collection())
    {
        // A one-column Table rather than a List, so the selection moves with the
        // SAME helper every other table here uses — ↑↓, PageUp/PageDown, Home/End
        // all behave as they do elsewhere instead of being a second scheme.
        let rows: Vec<Row> = app
            .viewer_lines
            .iter()
            .map(|l| Row::new(vec![l.clone()]))
            .collect();
        if app.viewer_row.selected().is_none() && !rows.is_empty() {
            app.viewer_row.select(Some(0));
        }
        let table = Table::new(rows, [Constraint::Min(10)])
            .block(block(app))
            .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED))
            .highlight_symbol("› ");
        f.render_stateful_widget(table, area, &mut app.viewer_row);
        return;
    }

    f.render_widget(
        Paragraph::new(app.viewer_lines.join("\n"))
            .block(block(app))
            .scroll((app.viewer_scroll, app.viewer_hscroll)),
        area,
    );
}

/// What you can DO to what this viewer is showing.
///
/// Each collection is one screen — see it, add to it, delete from it — so the
/// screen says which keys do that. These were separate menu entries: findable,
/// but disconnected from the thing they act on.
pub(super) fn viewer_actions(app: &App) -> String {
    use super::worker::View;
    match app.viewer_ctx.as_ref().map(|(v, ..)| *v) {
        Some(View::Env) => " e edit ".into(),
        Some(View::Ports) | Some(View::Mounts) | Some(View::Redirects) => {
            " ↑↓ select · n add · x delete ".into()
        }
        Some(View::Source) => " e set source · b set build ".into(),
        _ => String::new(),
    }
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
    let status_style = if app.status_is_error() {
        // Palette pink: contrasts on top of the gray, theme-independent.
        bar.fg(Color::Indexed(210)).add_modifier(Modifier::BOLD)
    } else {
        bar.add_modifier(Modifier::BOLD)
    };
    // A spinner while an operation is running: signals "working", not frozen.
    let head = match app.spinner() {
        Some(c) => format!(" {c} {} ", app.status_line()),
        None => format!(" {} ", app.status_line()),
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
    let mut block = Block::bordered()
        .title(title)
        .border_style(Style::default().fg(Color::Cyan));
    // A refusal replaces the guidance while it stands: it is the more urgent of
    // the two, and it names the field the user must fix to move on.
    if let Some(err) = &form.error {
        block = block.title_bottom(Line::from(Span::styled(
            format!(" {err} "),
            Style::default()
                .fg(Color::Indexed(210))
                .add_modifier(Modifier::BOLD),
        )));
    } else if let Some(note) = &form.note {
        block = block.title_bottom(format!(" {note} "));
    }
    f.render_widget(block, area);

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
    // The form is a PERCENTAGE of the terminal, so the footer has to earn its
    // width rather than assume it: on an 80-column terminal the full hint list
    // doesn't fit, and a hint cut mid-word ("[Esc] can") reads as a broken UI.
    let (enter, esc) = if form.is_wizard() {
        (
            if form.next_present_step().is_some() {
                "next →"
            } else {
                "create service"
            },
            if form.prev_present_step().is_some() {
                "← back"
            } else {
                "cancel"
            },
        )
    } else {
        ("save", "cancel")
    };
    let slot = slots[visible.len()];
    let footer = fit_hints(
        &[
            format!("[Enter] {enter}"),
            format!("[Esc] {esc}"),
            "[Tab] move field".into(),
            "[Space] choose".into(),
        ],
        slot.width,
    );
    f.render_widget(
        Paragraph::new(footer).style(Style::default().fg(Color::DarkGray)),
        slot,
    );
}

pub(super) fn render_chooser(f: &mut Frame, ch: &mut Chooser) {
    let items = ch.matches();
    // An empty result used to draw a blank box: no explanation, no way to tell
    // that the search had excluded everything rather than that the list was
    // simply empty.
    let empty = items.is_empty();
    let height = (items.len() as u16 + 4).clamp(5, 16);
    let area = centered_abs(48, height, f.area());
    ch.rect = area;
    f.render_widget(Clear, area);

    let title = if ch.filter.is_empty() {
        format!(" {} — type to search ", ch.label)
    } else {
        format!(" {} — search: {} ", ch.label, ch.filter)
    };
    let rows: Vec<ListItem> = if empty {
        vec![ListItem::new(Line::from(Span::styled(
            "  nothing matches — Backspace to widen",
            Style::default().fg(Color::Indexed(210)),
        )))]
    } else {
        items.into_iter().map(ListItem::new).collect()
    };
    let list = List::new(rows)
        .block(
            Block::bordered()
                .title(title)
                // The keys were nowhere on this widget before.
                .title_bottom(" Enter select · Esc cancel ")
                .border_style(Style::default().fg(Color::Cyan)),
        )
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("› ");
    f.render_stateful_widget(list, area, &mut ch.state);
}

pub(super) fn render_confirm(f: &mut Frame, c: &Confirm) {
    // Sized from the content, not from a percentage of the screen. At 80x24 the old
    // 52%x22% box was 41x5 for six lines of text: the question was cut mid-word and
    // the "[y] Yes [n] Cancel" line fell off the bottom entirely — the operator was
    // asked to approve an irreversible, host-wide action without being able to read
    // it or see which key confirms.
    let full = f.area();
    let w = 60.min(full.width.saturating_sub(4)).max(24);
    let inner = w.saturating_sub(2).max(1) as usize;
    // Word wrapping can break earlier than a hard division, so round up and add a
    // line: a dialog one row too tall is harmless, one row too short hides the keys.
    let label_lines = c.label.chars().count().div_ceil(inner) as u16 + 1;
    // blank + label + blank + target + blank + keys, plus the two borders.
    let h = (label_lines + 7).min(full.height);
    let area = centered_abs(w, h, full);
    f.render_widget(Clear, area);
    // Name the actual target. The line "Affects a real service" used to be shown on
    // every confirmation — wrong for a maintenance action, which affects the whole
    // host, not a single service.
    // A bulk run names its services in the label and has no single target, so it
    // must be matched BEFORE the empty-project case — which reads an empty
    // project as "maintenance" and would otherwise warn that restarting three
    // marked services affects the whole host.
    let target = match (c.project.as_str(), c.service.as_str()) {
        _ if c.action.starts_with("bulk-") => "Affects only the marked services.".to_string(),
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
        // Wrap, never truncate: a half-read question about deleting things is worse
        // than a taller dialog.
        .wrap(Wrap { trim: false })
        .block(
            Block::bordered()
                .title(" Confirm ")
                .border_style(Style::default().fg(Color::Yellow)),
        ),
        area,
    );
}

pub(super) fn render_picker(f: &mut Frame, app: &mut App) {
    // Sized from the content, not as a percentage of the screen. At 46% of an
    // 80-column terminal this box was 36 wide: its own title lost "x delete", and
    // every URL was cut without an ellipsis — "https://panel.internal.exa" reads
    // as a complete, different host. The URL is here precisely so you can tell
    // which server you are about to edit or DELETE, so it is the last thing that
    // should be guessed at.
    let full = f.area();
    let w = 72.min(full.width.saturating_sub(4)).max(30);
    let h = (app.all_servers.len() as u16 + 3).clamp(5, full.height.saturating_sub(2));
    let area = centered_abs_w(w, h, full);
    f.render_widget(Clear, area);

    let inner = w.saturating_sub(4) as usize;
    let items: Vec<ListItem> = app
        .all_servers
        .iter()
        .map(|(n, url)| {
            let mark = if n == &app.server_name {
                " (active)"
            } else {
                ""
            };
            let head = format!("{n}{mark}  ");
            // Whatever is left after the name — and if the URL still does not
            // fit, it ends in "…" rather than looking like a shorter host.
            let room = inner.saturating_sub(head.chars().count()).max(8);
            ListItem::new(Line::from(vec![
                Span::raw(head),
                Span::styled(
                    crate::output::first_line(url, room),
                    Style::default().fg(Color::DarkGray),
                ),
            ]))
        })
        .collect();
    let list = List::new(items)
        .block(
            Block::bordered()
                .title(" Servers ")
                // Dropped whole rather than cut mid-word, the same rule the form
                // footers use.
                .title_bottom(fit_hints(
                    &[
                        "Enter select".into(),
                        "n new".into(),
                        "e edit".into(),
                        "x delete".into(),
                        "Esc close".into(),
                    ],
                    w.saturating_sub(2),
                ))
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
/// Join the hints that FIT, most important first, and drop the rest.
///
/// Dropping a whole hint is honest; truncating one is not — "[Esc] can" looks
/// like a rendering fault, and the key it names is the one a stuck user needs.
/// Counted in chars, not bytes: the wizard's arrows are multi-byte.
pub(super) fn fit_hints(parts: &[String], width: u16) -> String {
    let mut out = String::new();
    let mut used = 0usize;
    for p in parts {
        let len = p.chars().count();
        let sep = if out.is_empty() { 0 } else { 2 };
        if used + sep + len > width as usize {
            break;
        }
        if sep > 0 {
            out.push_str("  ");
        }
        out.push_str(p);
        used += sep + len;
    }
    out
}

/// Centre a box of an ABSOLUTE width and height.
///
/// `centered_abs` takes a PERCENTAGE for its width despite the name, which is
/// how the server picker ended up 36 columns wide on an 80-column terminal.
pub(super) fn centered_abs_w(width: u16, height: u16, r: Rect) -> Rect {
    let x = r.x + (r.width.saturating_sub(width)) / 2;
    let y = r.y + (r.height.saturating_sub(height)) / 2;
    Rect {
        x,
        y,
        width: width.min(r.width),
        height: height.min(r.height),
    }
}

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
