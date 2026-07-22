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
    Key("1-8 / Tab / ←→", "switch tab"),
    Key("W", "switch workspace (EasyPanel / Cloudflare)"),
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
        Screen::Uptime => &[
            Key("r", "check them all now"),
            Key("e", "edit this check — method, body, headers"),
            Key("x", "stop watching"),
            Key("↑↓", "select"),
        ],
        Screen::Maintenance => &[
            Key("p", "prune Docker system"),
            Key("i", "remove unused images"),
            Key("c", "remove build cache"),
        ],
        Screen::Actions => &[
            Key("/", "search"),
            Key("f", "failures only"),
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
            Key("E", "bulk edit shown"),
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
        Screen::Credentials => &[
            Key("↑↓", "select a field"),
            Key("v", "reveal / hide the password and connection URL"),
            Key("c / y / Enter", "copy the selected value to the clipboard"),
            Key("Esc", "back to Services"),
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

/// The "Anywhere" keys that actually act in the Cloudflare workspace. The product
/// tab bar (DNS · R2 …) switches with 1..=N / Tab / ←→ — the CF mirror of the
/// EasyPanel tab keys — so the help documents it (the header shows the tabs; nothing
/// else told the reader how to reach them). `:` opens the CF command palette (the
/// mirror of EasyPanel's `:`). The `s` server list stays inert here (the CF analogue
/// is `a`, a per-screen key), so listing it would be help that lies. The `1-2` upper
/// bound is pinned to CF_PRODUCTS by `the_cf_product_tab_hint_names_every_product`.
pub(super) const CF_GLOBAL_KEYS: &[Key] = &[
    Key("W", "switch workspace (EasyPanel / Cloudflare)"),
    Key("?", "this help"),
    Key("1-2 / Tab / ←→", "switch product tab"),
    Key(
        ":",
        "command palette (jump to a product / account / zone / bucket)",
    ),
    Key("r", "refresh"),
    Key("Esc", "go back a step / close a filter or menu"),
    Key("q / Ctrl-C", "quit"),
];

/// The mouse actions available in the Cloudflare workspace (no tab-click with one
/// product tab; right-click opens the row menu on the Zones screen).
pub(super) const CF_MOUSE_KEYS: &[Key] = &[
    Key("Click row", "select the row"),
    Key("Right click", "action menu for the selected row"),
    Key("Scroll", "scroll the list"),
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

    // ISOLATION: the Cloudflare workspace draws its OWN header and screen; no
    // EasyPanel tab bar or pane renders behind it (and vice-versa).
    match app.workspace {
        Workspace::Cloudflare => render_cloudflare(f, chunks[0], chunks[1], app),
        Workspace::Easypanel => {
            render_tabs(f, chunks[0], app);
            match app.screen {
                Screen::Dashboard => render_dashboard(f, chunks[1], app),
                Screen::Hosts => render_hosts(f, chunks[1], app),
                Screen::Maintenance => render_maintenance(f, chunks[1], app),
                Screen::Actions => render_actions(f, chunks[1], app),
                Screen::Monitor => render_monitor(f, chunks[1], app),
                Screen::Domains => render_domains(f, chunks[1], app),
                Screen::Projects => render_projects(f, chunks[1], app),
                Screen::Uptime => render_uptime(f, chunks[1], app),
                Screen::Viewer => render_viewer(f, chunks[1], app),
                Screen::Terminal => render_terminal(f, chunks[1], app),
                Screen::Credentials => render_credentials(f, chunks[1], app),
            }
        }
    }
    render_status(f, chunks[2], app);

    if let Some(c) = &app.confirm {
        render_confirm(f, c, &app.server_name);
    }
    if app.picker.is_some() {
        render_picker(f, app);
    }
    if app.cf_picker.is_some() {
        render_cf_picker(f, app);
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

/// The keys the help overlay lists for a Cloudflare screen — the CF analogue of
/// `screen_keys`, so `?` in the CF workspace documents the CF keys instead of the
/// stale EasyPanel screen's. (Terse status-bar hints live in `cf_status_hints`.)
pub(super) fn cf_screen_keys(screen: CfScreen) -> &'static [Key] {
    match screen {
        CfScreen::Zones => &[
            Key(
                "a",
                "switch Cloudflare account (a picker, like `s` switches servers)",
            ),
            Key("Enter", "open the selected zone's DNS records"),
            Key("Space", "action menu for the selected zone"),
            Key("n", "add a zone"),
            Key("x", "delete a zone (type its name to confirm)"),
            Key("/", "filter the list"),
            Key("r", "refresh"),
            Key("Esc", "back to EasyPanel"),
        ],
        CfScreen::Records => &[
            Key("n", "add a DNS record"),
            Key("e", "edit the selected record"),
            Key("x", "delete the selected record"),
            Key("v / V", "mark one / all shown"),
            Key("Space", "bulk menu for the marked records"),
            Key("/", "filter the list"),
            Key("r", "refresh"),
            Key("Esc", "back to zones"),
        ],
        // Objects is an R2 screen (render_help routes R2 to cf_objects_keys); it shares
        // this fn only to keep the match exhaustive.
        CfScreen::Objects => cf_objects_keys(),
    }
}

/// The R2 Buckets screen's keys (product-selected in `render_help`, since R2 has
/// no `CfScreen` of its own). Enter drills into the bucket's objects.
pub(super) fn cf_buckets_keys() -> &'static [Key] {
    &[
        Key(
            "a",
            "switch Cloudflare account (a picker, like `s` switches servers)",
        ),
        Key("Enter", "browse the selected bucket's objects"),
        Key("Space", "action menu for the selected bucket"),
        Key("n", "add a bucket"),
        Key("x", "delete a bucket (type its name to confirm)"),
        Key("/", "filter the list"),
        Key("r", "refresh"),
        Key("Esc", "back to EasyPanel"),
    ]
}

/// The R2 Objects drill-in keys. Browse-only for now — add/delete of objects is a
/// later slice, so only filter/refresh and Esc back to the buckets are listed.
pub(super) fn cf_objects_keys() -> &'static [Key] {
    &[
        Key("/", "filter the list"),
        Key("r", "refresh"),
        Key("Esc", "back to buckets"),
    ]
}

pub(super) fn render_help(f: &mut Frame, app: &mut App) {
    // In the Cloudflare workspace the "this screen" section documents the CF screen's
    // keys, not the (stale) EasyPanel Screen's — the two workspaces are isolated.
    let cf = app.workspace == Workspace::Cloudflare;
    let rows = if cf {
        match app.cf.product {
            CfProduct::R2 => match app.cf.screen {
                CfScreen::Objects => cf_objects_keys(),
                _ => cf_buckets_keys(),
            },
            CfProduct::Dns => cf_screen_keys(app.cf.screen),
        }
    } else {
        screen_keys(app.screen)
    };
    // The "Anywhere" and "Mouse" sections are workspace-specific too: the CF workspace
    // has its own product-tab switch keys, no `:` palette / `s` server keys, and no
    // tab-click (its own right-click, added in CF_MOUSE_KEYS).
    let globals = if cf { CF_GLOBAL_KEYS } else { GLOBAL_KEYS };
    let mouse = if cf { CF_MOUSE_KEYS } else { MOUSE_KEYS };
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
        .chain(globals)
        .chain(OVERLAY_KEYS)
        .chain(mouse)
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

    let screen_label = if cf {
        match app.cf.product {
            CfProduct::R2 => match app.cf.screen {
                CfScreen::Objects => "Cloudflare · R2 · objects",
                _ => "Cloudflare · R2",
            },
            CfProduct::Dns => match app.cf.screen {
                CfScreen::Zones => "Cloudflare · DNS",
                CfScreen::Records | CfScreen::Objects => "Cloudflare · DNS · records",
            },
        }
    } else {
        TABS[app.screen.index()]
    };
    let mut lines = vec![head(&format!("{screen_label} — this screen"))];
    if rows.is_empty() {
        lines.push(Line::from("   (no dedicated keys)"));
    }
    lines.extend(rows.iter().flat_map(&row));
    lines.push(Line::from(""));
    lines.push(head("Anywhere"));
    lines.extend(globals.iter().flat_map(&row));
    lines.push(Line::from(""));
    lines.push(head("Inside forms & dropdowns"));
    lines.extend(OVERLAY_KEYS.iter().flat_map(&row));
    lines.push(Line::from(""));
    lines.push(head("Mouse"));
    lines.extend(mouse.iter().flat_map(&row));

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

/// A colour that belongs to one server, derived from its name.
///
/// With several hosts in play, the thing that stops you working on the wrong one
/// is not reading a label — it is noticing that the screen looks different. The
/// colour is a pure function of the name, so a server looks the same every time
/// you open it and different from its neighbours.
///
/// The palette avoids the indices that already carry meaning here: red for down,
/// green for active, and the pink used for errors.
pub(super) fn server_colour(name: &str) -> Color {
    const PALETTE: &[u8] = &[
        33,  // blue
        135, // purple
        208, // orange
        37,  // teal
        170, // magenta
        142, // olive
        69,  // slate blue
        173, // tan
    ];
    // FNV-1a: tiny, stable across runs and platforms. `DefaultHasher` is
    // explicitly not guaranteed to be, and a colour that changed between
    // versions would defeat the point.
    let mut h: u32 = 2_166_136_261;
    for b in name.as_bytes() {
        h ^= *b as u32;
        h = h.wrapping_mul(16_777_619);
    }
    Color::Indexed(PALETTE[(h as usize) % PALETTE.len()])
}

/// A content pane's frame, in the current server's colour.
///
/// Every box that holds the SERVER's data wears it — the tables, the dashboard,
/// the log viewer, the embedded terminal. Only the tab strip did at first, which
/// left the one small box at the top coloured and the large one filling the
/// screen still grey; the signal has to be the thing you cannot miss, not the
/// thing you have to look for.
///
/// Popups keep their own colours: a confirmation is yellow and a form is cyan
/// because those say something different, and the confirmation names the server
/// in words anyway.
pub(super) fn pane(title: impl Into<Line<'static>>, tint: Color) -> Block<'static> {
    Block::bordered()
        .title(title)
        .border_style(Style::default().fg(tint))
}

pub(super) fn render_tabs(f: &mut Frame, area: Rect, app: &mut App) {
    // Drawn by hand (not the Tabs widget) so each tab has a definite column hitbox
    // for mouse clicks, and the active tab can briefly "flash" when it changes.
    // The frame carries the server's colour, and its name is bold inside it: the
    // one thing on screen that answers "which machine am I about to change?"
    let tint = server_colour(&app.server_name);
    let block = Block::bordered()
        .border_style(Style::default().fg(tint))
        .title(Line::from(vec![
            Span::styled(" EasyPanel — ", Style::default().fg(tint)),
            Span::styled(
                format!("{} ", app.server_name),
                Style::default().fg(tint).add_modifier(Modifier::BOLD),
            ),
        ]));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let active = app.screen.index();
    let tab_fresh = app.tab_at.elapsed().as_millis() < 300;
    let mut spans = Vec::new();
    let mut hits = Vec::new();
    let mut x = inner.x;
    for (i, title) in super::app::tabs_for(area.width).iter().enumerate() {
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

/// Cloudflare orange (#F38020) — a deliberately distinct accent so it is
/// unmistakable you have left EasyPanel.
const CF_ORANGE: Color = Color::Rgb(243, 128, 32);

/// The Cloudflare empty-state copy (no account configured). One source, so the
/// render and its test cannot drift.
pub(super) const CF_EMPTY_HINT: &str = "No Cloudflare account yet — press a to add one";

/// The isolated Cloudflare workspace. Home is the Zones list; Records is a drill-in
/// from a zone. Reads only from `app.cf` — no EasyPanel state appears here.
pub(super) fn render_cloudflare(f: &mut Frame, header: Rect, body: Rect, app: &mut App) {
    // Dispatch on the PRODUCT first (DNS vs R2); DNS then splits on its screen
    // (Zones home / Records drill-in). R2 has a single buckets screen for now.
    match app.cf.product {
        CfProduct::Dns => match app.cf.screen {
            CfScreen::Zones => render_cf_zones(f, header, body, app),
            CfScreen::Records | CfScreen::Objects => render_cf_records(f, header, body, app),
        },
        CfProduct::R2 => match app.cf.screen {
            CfScreen::Objects => render_cf_objects(f, header, body, app),
            _ => render_cf_buckets(f, header, body, app),
        },
    }
}

/// The per-screen Cloudflare key hints. They live in the STATUS BAR (the header
/// now carries the product tab bar), mirroring how EasyPanel surfaces per-screen
/// keys as a hint line. One source, so the render and its test cannot drift.
pub(super) fn cf_status_hints(screen: CfScreen) -> &'static str {
    match screen {
        CfScreen::Records => {
            "n add · e edit · x delete · v/V mark · Space bulk · / filter · r refresh · Esc zones"
        }
        // The Zones home. R2's Objects drill-in never routes here (R2 uses the CF_*_HINTS
        // consts), so it shares this arm only to keep the match exhaustive.
        CfScreen::Zones | CfScreen::Objects => {
            "a account · Enter records · n add zone · x delete · / filter · r refresh · Esc EasyPanel"
        }
    }
}

/// The R2 Buckets status-bar hint. Product-selected in the status bar (the DNS
/// hints come from `cf_status_hints`), so the two can't drift from the keys.
pub(super) const CF_BUCKETS_HINTS: &str =
    "a account · Enter objects · n add bucket · x delete · Space menu · / filter · r refresh · Esc EasyPanel";

/// The R2 Objects folder-browser status-bar hint. No add/delete yet — object mutation is
/// a later slice. Esc goes up a folder inside the tree, or out to the buckets at the root.
pub(super) const CF_OBJECTS_HINTS: &str = "Enter open · / filter · r refresh · Esc up/buckets";

/// The orange workspace header: the bordered title + the PRODUCT tab bar (DNS
/// today; D1/R2/KV/Workers/Connectors slot in later). Drawn exactly like the
/// EasyPanel `render_tabs` — `│` gray separators, gray inactive tabs, the active
/// tab bold with a brief "reversed flash" on change — but in CF orange. The
/// per-screen key hints now live in the status bar, not here.
fn cf_header(f: &mut Frame, header: Rect, title: &str, product: CfProduct, fresh: bool) {
    let block = Block::bordered()
        .border_style(Style::default().fg(CF_ORANGE))
        .title(Span::styled(
            format!(" {title} "),
            Style::default().fg(CF_ORANGE).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(header);
    f.render_widget(block, header);

    let active = product.index();
    let mut spans = Vec::new();
    for (i, (label, _)) in CF_PRODUCTS.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("│", Style::default().fg(Color::DarkGray)));
        }
        let style = if i == active {
            let base = Style::default().fg(CF_ORANGE).add_modifier(Modifier::BOLD);
            if fresh {
                base.add_modifier(Modifier::REVERSED)
            } else {
                base
            }
        } else {
            Style::default().fg(Color::Gray)
        };
        spans.push(Span::styled(format!(" {label} "), style));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), inner);
}

/// Draw the loading / error / empty placeholder for a CF list, or return false so
/// the caller draws the table. Keeps the empty-vs-failed distinction in one place.
fn cf_placeholder(f: &mut Frame, body: Rect, title: &str, state: &CfListState, err: Option<&str>) {
    let (text, colour) = match state {
        CfListState::Loading => (format!("  Loading {title}…"), Color::DarkGray),
        CfListState::Error => (
            format!("  ⚠ {}", err.unwrap_or("failed to load")),
            Color::Indexed(210),
        ),
        CfListState::Empty => (format!("  No {title}"), Color::DarkGray),
        CfListState::Ready => return,
    };
    f.render_widget(
        Paragraph::new(text)
            .style(Style::default().fg(colour))
            .block(pane(title.to_string(), CF_ORANGE)),
        body,
    );
}

/// The account picker overlay — the mirror of the server `s` picker, in CF orange.
/// Lists the stored accounts (active one marked) with masked tokens; add/delete
/// live here too so accounts are fully managed without a standalone screen.
pub(super) fn render_cf_picker(f: &mut Frame, app: &mut App) {
    let full = f.area();
    let w = 72.min(full.width.saturating_sub(4)).max(30);
    let h = (app.cf.accounts.len() as u16 + 3).clamp(5, full.height.saturating_sub(2));
    let area = centered_abs_w(w, h, full);
    f.render_widget(Clear, area);

    let active = app.cf.active.as_ref().map(|a| a.name.clone());
    let items: Vec<ListItem> = if app.cf.accounts.is_empty() {
        vec![ListItem::new(Line::from(Span::styled(
            format!("  {CF_EMPTY_HINT}"),
            Style::default().fg(Color::DarkGray),
        )))]
    } else {
        app.cf
            .accounts
            .iter()
            .map(|a| {
                let mark = if active.as_deref() == Some(a.name.as_str()) {
                    " (active)"
                } else {
                    ""
                };
                ListItem::new(Line::from(vec![
                    Span::raw(format!("{}{mark}  ", a.name)),
                    // A FIXED run of bullets — the token's length must not leak.
                    Span::styled("••••••••••••", Style::default().fg(Color::DarkGray)),
                ]))
            })
            .collect()
    };
    let list = List::new(items)
        .block(
            Block::bordered()
                .title(" Cloudflare accounts ")
                .title_bottom(fit_hints(
                    &[
                        "Enter select".into(),
                        "n new".into(),
                        "x delete".into(),
                        "Esc close".into(),
                    ],
                    w.saturating_sub(2),
                ))
                .border_style(Style::default().fg(CF_ORANGE)),
        )
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("› ");
    if let Some(state) = app.cf_picker.as_mut() {
        f.render_stateful_widget(list, area, state);
    } else {
        f.render_widget(list, area);
    }
}

/// An orange breadcrumb: "Cloudflare" joined by " · " with each present segment,
/// then " — <tail>". A missing account/zone is dropped rather than shown as "—",
/// so an empty workspace reads "Cloudflare — zones", not "Cloudflare · — — zones".
fn cf_breadcrumb(segments: &[&str], tail: &str) -> String {
    let mut crumb = String::from("Cloudflare");
    for s in segments {
        crumb.push_str(" · ");
        crumb.push_str(s);
    }
    format!("{crumb} — {tail}")
}

/// The Zones home: Name / Status / ID, filterable, with loading/empty/error. With
/// no account configured it shows the empty state (press `a`), never a dead end.
fn render_cf_zones(f: &mut Frame, header: Rect, body: Rect, app: &mut App) {
    let acct = app.cf.active.as_ref().map(|a| a.name.clone());
    // The home screen names the active account exactly like EasyPanel's header names the
    // active server ("EasyPanel — <server>" → "Cloudflare — <account>"), switched with the
    // `a` account picker the way `s` switches servers. The `— zones` breadcrumb is dropped
    // here (this IS the zones home); Records keeps the full breadcrumb as a drill-in.
    let title = match &acct {
        Some(name) => format!("Cloudflare — {name}"),
        None => "Cloudflare".to_string(),
    };
    cf_header(
        f,
        header,
        &title,
        app.cf.product,
        app.cf_product_at.elapsed().as_millis() < 300,
    );

    // No account at all: nothing to load — invite adding one (the `a` picker).
    if app.cf_empty() {
        f.render_widget(
            Paragraph::new(format!("  {CF_EMPTY_HINT}"))
                .style(Style::default().fg(Color::DarkGray))
                .block(pane("Zones".to_string(), CF_ORANGE)),
            body,
        );
        return;
    }

    let state = cf_list_state(
        app.busy() > 0,
        app.cf.error.is_some(),
        app.cf.zones.is_empty(),
    );
    if state != CfListState::Ready {
        cf_placeholder(f, body, "zones", &state, app.cf.error.as_deref());
        return;
    }

    let shown = app.cf_zones_shown();
    let title = format!(
        "Zones ({} of {}){}",
        shown.len(),
        app.cf.zones.len(),
        if app.cf.filter.is_empty() {
            String::new()
        } else {
            format!(" · /{}", app.cf.filter)
        }
    );
    let rows: Vec<Vec<String>> = shown
        .iter()
        .map(|z| vec![z.name.clone(), z.status.clone(), z.id.clone()])
        .collect();
    let headers = ["Name", "Status", "ID"];
    let widths = [
        Constraint::Min(20),
        Constraint::Length(12),
        Constraint::Length(34),
    ];
    // Record the table's Rect so the shared mouse layer can map a click/hover to a
    // row, exactly as the EasyPanel render paths do.
    app.table_area = body;
    render_table(
        f,
        body,
        title,
        &headers,
        &widths,
        rows,
        &mut app.cf.zones_row,
        CF_ORANGE,
        |_, _| None,
    );
}

/// The Records screen: Type / Name / Content / Priority / TTL / Proxied / ID,
/// filterable, marks shown with a leading ✓, with loading/empty/error.
fn render_cf_records(f: &mut Frame, header: Rect, body: Rect, app: &mut App) {
    let acct = app.cf.active.as_ref().map(|a| a.name.clone());
    let zone = app.cf.current_zone.as_ref().map(|z| z.name.clone());
    let segs: Vec<&str> = [acct.as_deref(), zone.as_deref()]
        .into_iter()
        .flatten()
        .collect();
    cf_header(
        f,
        header,
        &cf_breadcrumb(&segs, "records"),
        app.cf.product,
        app.cf_product_at.elapsed().as_millis() < 300,
    );

    let state = cf_list_state(
        app.busy() > 0,
        app.cf.error.is_some(),
        app.cf.records.is_empty(),
    );
    if state != CfListState::Ready {
        cf_placeholder(f, body, "DNS records", &state, app.cf.error.as_deref());
        return;
    }

    let shown = app.cf_records_shown();
    let marked = &app.cf.marked;
    let title = format!(
        "DNS records ({} of {}){}{}",
        shown.len(),
        app.cf.records.len(),
        if marked.is_empty() {
            String::new()
        } else {
            format!(" · {} marked", marked.len())
        },
        if app.cf.filter.is_empty() {
            String::new()
        } else {
            format!(" · /{}", app.cf.filter)
        }
    );
    let rows: Vec<Vec<String>> = shown
        .iter()
        .map(|r| {
            let mark = if marked.contains(&r.id) { "✓ " } else { "" };
            vec![
                format!("{mark}{}", r.kind),
                r.name.clone(),
                r.content.clone(),
                r.priority.map(|p| p.to_string()).unwrap_or_default(),
                if r.ttl == 1 {
                    "auto".into()
                } else {
                    r.ttl.to_string()
                },
                if r.proxied { "yes".into() } else { "no".into() },
                r.id.clone(),
            ]
        })
        .collect();
    let headers = [
        "Type", "Name", "Content", "Priority", "TTL", "Proxied", "ID",
    ];
    let widths = [
        Constraint::Length(9),
        Constraint::Length(24),
        Constraint::Min(20),
        Constraint::Length(8),
        Constraint::Length(6),
        Constraint::Length(7),
        Constraint::Length(34),
    ];
    // Record the table's Rect so the shared mouse layer can map a click/hover to a
    // row, exactly as the EasyPanel render paths do.
    app.table_area = body;
    render_table(
        f,
        body,
        title,
        &headers,
        &widths,
        rows,
        &mut app.cf.records_row,
        CF_ORANGE,
        |_, _| None,
    );
}

/// The R2 Buckets home: Name / Created / Location / Class, filterable, with
/// loading/empty/error. Mirrors `render_cf_zones` — the same header, placeholder,
/// and table machinery, reading only `app.cf`. Object browsing is a later slice.
fn render_cf_buckets(f: &mut Frame, header: Rect, body: Rect, app: &mut App) {
    let acct = app.cf.active.as_ref().map(|a| a.name.clone());
    let title = match &acct {
        Some(name) => format!("Cloudflare — {name}"),
        None => "Cloudflare".to_string(),
    };
    cf_header(
        f,
        header,
        &title,
        app.cf.product,
        app.cf_product_at.elapsed().as_millis() < 300,
    );

    // No account at all: nothing to load — invite adding one (the `a` picker).
    if app.cf_empty() {
        f.render_widget(
            Paragraph::new(format!("  {CF_EMPTY_HINT}"))
                .style(Style::default().fg(Color::DarkGray))
                .block(pane("Buckets".to_string(), CF_ORANGE)),
            body,
        );
        return;
    }

    let state = cf_list_state(
        app.busy() > 0,
        app.cf.error.is_some(),
        app.cf.r2_buckets.is_empty(),
    );
    if state != CfListState::Ready {
        cf_placeholder(f, body, "buckets", &state, app.cf.error.as_deref());
        return;
    }

    let shown = app.cf_buckets_shown();
    let title = format!(
        "Buckets ({} of {}){}",
        shown.len(),
        app.cf.r2_buckets.len(),
        if app.cf.filter.is_empty() {
            String::new()
        } else {
            format!(" · /{}", app.cf.filter)
        }
    );
    let rows: Vec<Vec<String>> = shown
        .iter()
        .map(|b| {
            vec![
                b.name.clone(),
                // The creation date is an ISO-8601 timestamp; show just the date part.
                b.creation_date
                    .split('T')
                    .next()
                    .unwrap_or(&b.creation_date)
                    .to_string(),
                b.location.clone().unwrap_or_default(),
                b.storage_class.clone(),
            ]
        })
        .collect();
    let headers = ["Name", "Created", "Location", "Class"];
    let widths = [
        Constraint::Min(20),
        Constraint::Length(12),
        Constraint::Length(12),
        Constraint::Length(18),
    ];
    // Record the table's Rect so the shared mouse layer can map a click/hover to a
    // row, exactly as the DNS render paths do.
    app.table_area = body;
    render_table(
        f,
        body,
        title,
        &headers,
        &widths,
        rows,
        &mut app.cf.r2_row,
        CF_ORANGE,
        |_, _| None,
    );
}

/// The R2 Objects FOLDER browser: subfolders first (a `▸ name/` marker, no size/date),
/// then the files at this level (basename / Size / Modified), filterable, with loading/
/// empty/error. `/`-delimited keys browse as a tree — Enter descends, Esc goes up. A
/// token lacking the R2 permission lands in the error state with the "Workers R2 Storage"
/// hint.
fn render_cf_objects(f: &mut Frame, header: Rect, body: Rect, app: &mut App) {
    let acct = app.cf.active.as_ref().map(|a| a.name.clone());
    // The breadcrumb tail is the bucket PLUS the path inside it, e.g. `assets/css/`.
    let loc = app.cf.current_bucket.as_ref().map(|b| {
        if app.cf.current_prefix.is_empty() {
            b.clone()
        } else {
            format!("{b}/{}", app.cf.current_prefix)
        }
    });
    let segs: Vec<&str> = [acct.as_deref(), loc.as_deref()]
        .into_iter()
        .flatten()
        .collect();
    cf_header(
        f,
        header,
        &cf_breadcrumb(&segs, "objects"),
        app.cf.product,
        app.cf_product_at.elapsed().as_millis() < 300,
    );

    // Empty means no subfolders AND no files at this level.
    let state = cf_list_state(
        app.busy() > 0,
        app.cf.error.is_some(),
        app.cf.r2_folders.is_empty() && app.cf.r2_objects.is_empty(),
    );
    if state != CfListState::Ready {
        cf_placeholder(f, body, "objects", &state, app.cf.error.as_deref());
        return;
    }

    let prefix = app.cf.current_prefix.clone();
    // Build the combined row list (folders first, then files) as owned data so the
    // immutable borrows of `app` end before the mutable `render_table` borrow below.
    let (rows, shown_count) = {
        let folders = app.cf_folders_shown();
        let files = app.cf_objects_shown();
        let mut rows: Vec<Vec<String>> = folders
            .iter()
            .map(|folder| {
                // Show only the next path segment; the marker makes a folder read as a
                // folder (and `cell_style` tints it), with no size/date.
                let seg = folder.strip_prefix(&prefix).unwrap_or(folder);
                vec![format!("▸ {seg}"), String::new(), String::new()]
            })
            .collect();
        rows.extend(files.iter().map(|o| {
            // Strip the current prefix so the file reads as its basename at this level.
            let name = o.key.strip_prefix(&prefix).unwrap_or(&o.key);
            vec![
                name.to_string(),
                format_bytes(o.size as f64),
                // LastModified is an ISO-8601 timestamp; drop the sub-second tail.
                o.last_modified
                    .split('.')
                    .next()
                    .unwrap_or(&o.last_modified)
                    .to_string(),
            ]
        }));
        (rows, folders.len() + files.len())
    };
    let title = format!(
        "Objects ({} of {}){}{}",
        shown_count,
        app.cf.r2_folders.len() + app.cf.r2_objects.len(),
        // A big level loads only its first page; say so rather than implying it's whole.
        if app.cf.r2_truncated {
            " · first page, more exist — narrow with /"
        } else {
            ""
        },
        if app.cf.filter.is_empty() {
            String::new()
        } else {
            format!(" · /{}", app.cf.filter)
        }
    );
    let headers = ["Name", "Size", "Modified"];
    let widths = [
        Constraint::Min(30),
        Constraint::Length(12),
        Constraint::Length(22),
    ];
    // Record the table's Rect so the shared mouse layer can map a click/hover to a row.
    app.table_area = body;
    render_table(
        f,
        body,
        title,
        &headers,
        &widths,
        rows,
        &mut app.cf.r2_objects_row,
        CF_ORANGE,
        // Paint the folder rows (the `▸ ` marker in the Name column) in bold CF orange so
        // a folder reads as a folder, not a file.
        |col, text| {
            (col == 0 && text.starts_with("▸ "))
                .then(|| Style::default().fg(CF_ORANGE).add_modifier(Modifier::BOLD))
        },
    );
}

pub(super) fn render_dashboard(f: &mut Frame, area: Rect, app: &App) {
    // A FAILED stats load with nothing cached must not draw 0.0% gauges — numbers
    // that read as real, at once alarming ("disk empty?!") and falsely reassuring
    // ("CPU idle"). Say what happened instead. A refresh failure keeps last-good
    // stats (still Some), so this only fires when the first load never arrived.
    if app.stats.is_none() {
        if let Some(err) = &app.stats_error {
            f.render_widget(
                Paragraph::new(format!("  ⚠ Couldn't load stats — {err}. Press r to retry"))
                    .style(Style::default().fg(Color::DarkGray))
                    .block(pane(
                        "Dashboard".to_string(),
                        server_colour(&app.server_name),
                    )),
                area,
            );
            return;
        }
    }
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
    render_gauge(
        f,
        left[0],
        "CPU",
        series_last(&stats, "cpu"),
        server_colour(&app.server_name),
    );
    render_gauge(
        f,
        left[1],
        "Memory",
        series_last(&stats, "memory"),
        server_colour(&app.server_name),
    );
    render_gauge(
        f,
        left[2],
        "Disk",
        series_last(&stats, "disk"),
        server_colour(&app.server_name),
    );
    f.render_widget(
        Paragraph::new(format!(
            " {} cores — load {}",
            field(&stats, "/cpuCores"),
            crate::monitor::load_avg(&stats)
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
        .block(pane(
            format!(" CPU History (0–{ceiling}%) "),
            server_colour(&app.server_name),
        ))
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

pub(super) fn render_gauge(f: &mut Frame, area: Rect, label: &str, pct: f64, tint: Color) {
    let g = Gauge::default()
        .block(pane(format!(" {label} "), tint))
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
        let row = Row::new([
            field(n, "/Description/Hostname"),
            field(n, "/Spec/Role"),
            field(n, "/Status/State"),
            field(n, "/Spec/Availability"),
            field(n, "/Status/Addr"),
        ]);
        // Colour carries state everywhere else in this app — an unreachable host
        // is red, a crashed service pulses red — but a swarm node that has gone
        // `down` was painted exactly like a healthy one, on the FIRST screen the
        // user sees. A node leaving the cluster is why services vanish.
        match field(n, "/Status/State").as_str() {
            "ready" => row,
            _ => row.style(
                Style::default()
                    .fg(Color::Indexed(196))
                    .add_modifier(Modifier::BOLD),
            ),
        }
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
    .block(pane(" Nodes ", server_colour(&app.server_name)));
    f.render_widget(table, area);
}

pub(super) fn render_projects(f: &mut Frame, area: Rect, app: &mut App) {
    // No services at all: distinguish a genuinely empty host from a failed load.
    // On a host with hundreds of services a 502 that drew a bare table read as
    // "this host has nothing" — the Domains bug, on the biggest screen.
    if app.all_services.is_empty() {
        let msg = if let Some(err) = &app.services_error {
            format!("  ⚠ Couldn't load services — {err}. Press r to retry")
        } else {
            "  No services on this host yet — press n to add one".to_string()
        };
        f.render_widget(
            Paragraph::new(msg)
                .style(Style::default().fg(Color::DarkGray))
                .block(pane(
                    "Services".to_string(),
                    server_colour(&app.server_name),
                )),
            area,
        );
        return;
    }
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
    // (cells, is_down, is_header): the header flag drives the bold-cyan project
    // name below. It comes from the Line2 variant, NOT from testing the indent —
    // a marked service reads "✓ name" rather than "  name", so an indent test
    // would mistake it for a header.
    let rows: Vec<(Vec<String>, bool, bool)> = lines
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
                (project_row(name, services.len(), &mets), false, true)
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
                (out, is_down, false)
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
    // Name and Source are both `Min`, so ratatui splits the leftover between them
    // and neither cell knows how wide it ended up. That is how a repo came to be
    // cut at the column edge with no ellipsis: "harisenincom/edukasistudio" read
    // as "harisenincom/edu" — a name that looks complete and is a different repo.
    // Worse, seven services of one project all rendered as "harisenincom/har",
    // telling the reader nothing while appearing to. So the Source width is
    // worked out HERE and the cell cut to it, with the ellipsis every other table
    // in this file already uses.
    const FIXED: [u16; SERVICE_HEADERS.len()] = [0, 8, 11, 5, 0, 5, 8, 10, 11, 11];
    let fixed: u16 = FIXED[..cols].iter().sum();
    let gaps = cols.saturating_sub(1) as u16;
    let spare = area.width.saturating_sub(4 + fixed + gaps);
    // Both minimums, then ratatui shares what is left evenly between the two.
    let source_w = (16 + spare.saturating_sub(26 + 16) / 2) as usize;
    let header = Row::new(SERVICE_HEADERS[..cols].to_vec()).style(
        Style::default()
            .fg(Color::Black)
            .bg(Color::Green)
            .add_modifier(Modifier::BOLD),
    );
    // "down" rows pulse (bright red <-> salmon) so the eye is drawn to what's
    // broken; this is an incident state, so pulling attention to it is apt.
    let down_style = pulse_red(app.anim.elapsed().as_millis());
    // The status dot (column 2) & the Auto mark (column 5) get their own per-cell
    // color: the state reads at a glance. "down" rows are left to inherit the red
    // pulse.
    let body = rows.into_iter().map(|(mut cells, is_down, is_header)| {
        cells.truncate(cols);
        let cells: Vec<Cell> = cells
            .into_iter()
            .enumerate()
            .map(|(i, c)| match i {
                // A project header's name in bold cyan, so the grouping reads at a
                // glance — the same cue the Monitor tab uses. Indexed, not named.
                0 if is_header => Cell::from(c).style(
                    Style::default()
                        .fg(Color::Indexed(14))
                        .add_modifier(Modifier::BOLD),
                ),
                4 => Cell::from(crate::output::first_line(&c, source_w)),
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
        .block(pane(title, server_colour(&app.server_name)))
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

/// What an action's status MEANS, in the colour this app already uses for state.
///
/// The Actions screen is the history of what happened, and it drew every row in
/// the same grey: a killed deploy and a failed one looked exactly like a
/// successful one. On the owner's own panel 19 of the last 200 actions were
/// `killed` or `error` — a tenth of the screen was findings nobody could see.
///
/// The three states are ground truth from a live panel (`done`, `killed`,
/// `error`); anything else EasyPanel adds later is left unpainted rather than
/// guessed at.
pub(super) fn action_status_colour(status: &str) -> Option<Color> {
    match status {
        "done" => Some(Color::Indexed(2)),
        // Deliberately halted — the same yellow "stopped" gets on Services,
        // because it is the same idea: someone chose this.
        "killed" => Some(Color::Indexed(3)),
        // It failed on its own. Red and bold, like an unreachable host.
        "error" => Some(Color::Indexed(196)),
        // In flight — the cyan "deploying" already uses.
        "running" => Some(Color::Indexed(6)),
        _ => None,
    }
}

/// The width the single flexible column of a table actually gets.
///
/// `None` when the table has no `Min` column, or more than one — then the split
/// is ratatui's business and no single answer exists.
///
/// This arithmetic has been written by hand five times in this file, and a
/// column was cut with no ellipsis every time it was forgotten: a repo name, an
/// action description, a domain, a failure reason, and a storage path where the
/// cut landed exactly on the character telling `mysql-r1` from `mysql-r2`. It
/// lives here now so a new table cannot be added without it.
pub(super) fn flex_width(widths: &[Constraint], area_width: u16, selected: bool) -> Option<usize> {
    let mut flex = None;
    let mut fixed = 0u16;
    for (i, c) in widths.iter().enumerate() {
        match c {
            Constraint::Length(n) => fixed += n,
            Constraint::Min(_) if flex.is_none() => flex = Some(i),
            // Two flexible columns: ratatui shares the slack between them and
            // this cannot say how.
            _ => return None,
        }
    }
    flex?;
    // One space between each pair of columns, the two borders, and — only when a
    // row is selected — the two columns of the highlight symbol. Counting the
    // symbol unconditionally would cut two characters that were never needed.
    let gaps = widths.len().saturating_sub(1) as u16;
    let symbol = if selected { 2 } else { 0 };
    Some(area_width.saturating_sub(2 + symbol + fixed + gaps).max(8) as usize)
}

/// Which column is the flexible one, if exactly one is.
fn flex_column(widths: &[Constraint]) -> Option<usize> {
    let mut flex = None;
    for (i, c) in widths.iter().enumerate() {
        match c {
            Constraint::Length(_) => {}
            Constraint::Min(_) if flex.is_none() => flex = Some(i),
            _ => return None,
        }
    }
    flex
}

#[allow(clippy::too_many_arguments)]
pub(super) fn render_table(
    f: &mut Frame,
    area: Rect,
    title: String,
    headers: &[&str],
    widths: &[Constraint],
    rows: Vec<Vec<String>>,
    state: &mut TableState,
    tint: Color,
    // Colour for one cell, by column index and text. Lets a table paint state
    // without every table having to rebuild what this function does.
    cell_style: fn(usize, &str) -> Option<Style>,
) {
    let header = Row::new(headers.to_vec()).style(
        Style::default()
            .fg(Color::Black)
            .bg(Color::Green)
            .add_modifier(Modifier::BOLD),
    );
    // The flexible column is the one that overflows, and ratatui clips it at the
    // pane edge with no mark — so a cut path or name reads as a complete, shorter
    // one. Cut it here instead, with the ellipsis every other table uses.
    let rows = match (
        flex_column(widths),
        flex_width(widths, area.width, state.selected().is_some()),
    ) {
        (Some(col), Some(w)) => rows
            .into_iter()
            .map(|mut cells| {
                if let Some(cell) = cells.get_mut(col) {
                    *cell = crate::output::first_line(cell, w);
                }
                cells
            })
            .collect(),
        _ => rows,
    };
    let table = Table::new(
        rows.into_iter().map(|cells| {
            Row::new(
                cells
                    .into_iter()
                    .enumerate()
                    .map(|(i, c)| match cell_style(i, &c) {
                        Some(st) => Cell::from(c).style(st),
                        None => Cell::from(c),
                    })
                    .collect::<Vec<_>>(),
            )
        }),
        widths.to_vec(),
    )
    .header(header)
    .block(pane(title, tint))
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
            .block(pane(" Maintenance ", server_colour(&app.server_name))),
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
    // The Server column is sized to the longest name actually configured, not to
    // a fixed 14 — "angelia-machine" is fifteen characters and was rendered as
    // "angelia-machin", cutting the one column whose entire job is telling the
    // hosts apart. Names are user-chosen and now user-editable, so the width
    // cannot be a constant; it is clamped so one very long name cannot push the
    // metrics off the screen.
    let name_w = app
        .hosts
        .iter()
        .map(|h| h.name.chars().count())
        .max()
        .unwrap_or(6)
        .clamp(6, 24) as u16;
    // The thresholds move WITH the Server column. They were written when it was a
    // fixed 14, so a fifteen-character name made every one of them one column
    // optimistic: the last column was kept at a width where it no longer fits, and
    // Status — the `Min` — silently absorbed the whole deficit. Status is the
    // column the comment above promises is never dropped BECAUSE it carries the
    // failure reason, so squeezing it is the one thing this must not do.
    let shift = name_w as i32 - 14;
    let at = |t: i32| (t + shift).max(0) as u16;
    let host_cols: &[(u16, Constraint)] = &[
        (0, Constraint::Length(name_w)),   // Server
        (0, Constraint::Min(16)),          // Status — carries the failure reason
        (0, Constraint::Length(7)),        // CPU
        (0, Constraint::Length(19)),       // Memory
        (at(83), Constraint::Length(19)),  // Disk
        (at(102), Constraint::Length(18)), // Load
        (at(133), Constraint::Length(30)), // URL
    ];
    let cols = columns_that_fit(
        &host_cols.iter().map(|(m, _)| *m).collect::<Vec<_>>(),
        area.width,
    )
    .len();

    // What Status will actually get, so a dead host's reason is cut with an
    // ellipsis rather than at the column edge. The old cut at 40 characters was
    // wider than this column is ever given, so it never fired and the reason was
    // clipped silently — reading as the whole cause when it is the first half.
    let fixed_after: u16 = [7u16, 19, 19, 18, 30][..cols.saturating_sub(2)]
        .iter()
        .sum();
    let status_w = area
        .width
        .saturating_sub(4 + name_w + fixed_after + cols.saturating_sub(1) as u16)
        .max(16) as usize;

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
                        format!(
                            "DOWN — {}",
                            crate::output::first_line(e, status_w.saturating_sub(7))
                        ),
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
                            crate::monitor::load_avg(v),
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
        host_cols
            .iter()
            .take(cols)
            .map(|(_, c)| *c)
            .collect::<Vec<_>>(),
    )
    .header(header)
    .block(pane(
        format!(" Hosts ({}) ", app.hosts.len()),
        server_colour(&app.server_name),
    ))
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

    // The width each column actually gets, so a value too long for it is cut
    // HERE — with an ellipsis — instead of silently at the edge. Description was
    // trimmed at 200 characters, a limit the column never reaches: at 80 columns
    // it is about 23 wide, so a commit message stopped dead ("Deploy service:
    // feat: u") and read as the whole of it. The same rule the Domains screen
    // already follows.
    let fixed: u16 = idx
        .iter()
        .filter(|i| **i != 2)
        .map(|i| match i {
            0 => 8,
            1 => 28,
            3 => 10,
            _ => 14,
        })
        .sum();
    let gaps = idx.len().saturating_sub(1) as u16;
    let desc_w = area.width.saturating_sub(4 + fixed + gaps).max(20) as usize;
    let widths_px = [8usize, 28, desc_w, 10, 14];

    let rows: Vec<Vec<String>> = app
        .visible_actions()
        .iter()
        .map(|a| {
            let cells = commands::action_row(a, commands::ACTION_DESC_TUI);
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
        .filter_map(|i| commands::ACTION_HEADERS.get(*i).copied())
        .collect();
    let widths: Vec<Constraint> = idx.iter().map(|i| ACTION_COLS[*i].1).collect();

    let mut title = count_title("Actions", rows.len(), app.actions.len(), app);
    if app.actions_failures_only {
        // A filtered list that does not announce the filter reads as missing
        // data — the mistake this project keeps having to un-make. And the count
        // must be the SHOWN count, not the raw total: "(50)" above four rows is
        // itself the missing-data lie. count_title only switches to shown/total
        // for a TEXT filter, so this one is spelled out.
        if app.filter.is_empty() && !app.filter_input {
            title = format!(" Actions ({}/{}) ", rows.len(), app.actions.len());
        }
        title.push_str("· failures only ");
    }
    app.table_area = area;
    render_table(
        f,
        area,
        title,
        &headers,
        &widths,
        rows,
        &mut app.actions_state,
        server_colour(&app.server_name),
        |col, text| {
            // Only the Status column, and only when the status is one this app
            // knows: an unfamiliar state stays unpainted rather than miscoloured.
            (col == 0)
                .then(|| action_status_colour(text))
                .flatten()
                .map(|c| {
                    let st = Style::default().fg(c);
                    if text == "error" {
                        st.add_modifier(Modifier::BOLD)
                    } else {
                        st
                    }
                })
        },
    );
}

/// A database service's connection identity: user, password, host, port, URL.
/// Secrets are masked with bullets until `v` reveals them; the selected row is
/// copied verbatim (the real value, even while masked) by `c`/`y`/Enter.
pub(super) fn render_credentials(f: &mut Frame, area: Rect, app: &mut App) {
    let colour = server_colour(&app.server_name);
    if app.creds.items.is_empty() {
        f.render_widget(
            Paragraph::new("  Reading credentials…")
                .style(Style::default().fg(Color::DarkGray))
                .block(pane(app.creds.title.clone(), colour)),
            area,
        );
        return;
    }
    let revealed = app.creds.revealed;
    let rows: Vec<Vec<String>> = app
        .creds
        .items
        .iter()
        .map(|c| {
            let value = if c.secret && !revealed {
                // A FIXED run of bullets, never the value's real length — the bullet
                // count must not leak how long the password is.
                "•".repeat(12)
            } else {
                c.value.clone()
            };
            vec![c.label.clone(), value]
        })
        .collect();
    let headers = ["Field", "Value"];
    let widths = [Constraint::Length(16), Constraint::Min(30)];
    let hint = if revealed { "v hide" } else { "v reveal" };
    let title = format!("{} · {hint} · c copy ", app.creds.title);
    render_table(
        f,
        area,
        title,
        &headers,
        &widths,
        rows,
        &mut app.creds.row,
        colour,
        // A masked secret is dimmed so it reads as "hidden", not as a value.
        |col, text| {
            (col == 1 && text.starts_with('•')).then(|| Style::default().fg(Color::DarkGray))
        },
    );
}

/// What marks a domain whose destination service no longer exists.
const ORPHAN_MARK: &str = "✗";

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
    const DEST_MIN: u16 = 34;
    const SRC_MIN: u16 = 30;
    let mins = [0u16, 69, 96];
    let idx = columns_that_fit(&mins, area.width);

    // Source AND Destination share the spare width; only the ID is fixed.
    //
    // Destination used to be pinned at 34 while Source absorbed everything left
    // over, so on a wide terminal Source had far more room than a hostname needs
    // while destinations were still cut — "http://harisenin-com-miniapp-gopa…"
    // beside a 25-character cuid at full width. The opaque id is the one thing
    // here nobody reads or types, so it is the one thing that does not grow.
    let gaps = idx.len().saturating_sub(1) as u16;
    let id_shown = idx.contains(&2);
    let spare = area
        .width
        .saturating_sub(4 + gaps + if id_shown { ID_W } else { 0 });
    // Halved rather than given to one of them: both hold text of about the same
    // length, and either being the only one cut is the bug this fixes.
    let dest_w = if idx.contains(&1) {
        (spare / 2).max(DEST_MIN)
    } else {
        0
    };
    let src_w = spare.saturating_sub(dest_w).max(SRC_MIN);
    let domain_cols = [
        Constraint::Min(SRC_MIN),
        Constraint::Length(dest_w),
        Constraint::Length(ID_W),
    ];

    // The width each column will actually get, so a value too long for its column
    // is cut HERE — with an ellipsis — instead of silently at the edge.
    let widths_px = [src_w as usize, dest_w as usize, ID_W as usize];

    // A domain whose service no longer exists is dead routing, and it is
    // invisible among hundreds of live ones. The mark goes at the FRONT of the
    // destination so it survives the truncation below, and it reads on its own
    // even where colour does not carry (a pipe, a screenshot in mono).
    let live = app.live_services();
    let mut gone = 0usize;
    let rows: Vec<Vec<String>> = app
        .visible_domains()
        .iter()
        .map(|d| {
            let mut cells = crate::domains::domain_row(d);
            if crate::domains::is_orphan(d, live.as_ref()) {
                gone += 1;
                if let Some(dest) = cells.get_mut(1) {
                    *dest = format!("{ORPHAN_MARK} {dest}");
                }
            }
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
        .filter_map(|i| crate::domains::DOMAIN_HEADERS.get(*i).copied())
        .collect();
    let widths: Vec<Constraint> = idx.iter().map(|i| domain_cols[*i]).collect();

    let mut title = count_title("Domains", rows.len(), app.domains.len(), app);
    if gone > 0 {
        // Only when there is something to say. A permanent "0 gone" is noise that
        // trains the eye to skip the whole title.
        title.push_str(&format!(
            "· {ORPHAN_MARK} {gone} pointing at a service that is gone "
        ));
    }
    app.table_area = area;
    if rows.is_empty() {
        // A bare bordered box cannot say whether the fetch FAILED, the filter
        // excluded everything, or there is genuinely nothing here — three states
        // that read identically as "empty" but need different actions. On a host
        // with hundreds of domains, a 502 that drew "No domains yet" is alarming
        // and wrong; say what actually happened.
        let msg = if let Some(err) = &app.domains_error {
            format!("  ⚠ Couldn't load domains — {err}. Press r to retry")
        } else if app.filter.is_empty() {
            "  No domains yet — press n to add one".to_string()
        } else {
            format!("  Nothing matches '{}' — Esc clears the filter", app.filter)
        };
        f.render_widget(
            Paragraph::new(msg)
                .style(Style::default().fg(Color::DarkGray))
                .block(pane(title, server_colour(&app.server_name))),
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
        server_colour(&app.server_name),
        // The destination column, and only the rows carrying the mark.
        |col, text| {
            (col == 1 && text.starts_with(ORPHAN_MARK))
                .then(|| Style::default().fg(Color::Indexed(196)))
        },
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
                &crate::monitor::MONITOR_HEADERS,
                &[
                    Constraint::Min(20),
                    Constraint::Length(9),
                    Constraint::Length(11),
                    Constraint::Length(11),
                    Constraint::Length(11),
                ],
                data,
                &mut app.monitor_state,
                server_colour(&app.server_name),
                // A project-header row's name is NOT indented; its services read
                // "  name". Bold + cyan makes the grouping visible at a glance —
                // without it, in a list of a hundred rows, the 2-space indent is
                // the only cue and it vanishes, so header and service look alike.
                // (Indexed, not named: named colors have rendered unreadable under
                // some terminal themes here before.)
                |col, text| {
                    (col == 0 && !text.starts_with("  ")).then(|| {
                        Style::default()
                            .fg(Color::Indexed(14))
                            .add_modifier(Modifier::BOLD)
                    })
                },
            );
        }
        MonitorView::Storage => {
            let data = app.visible_storage_rows();
            let total = crate::monitor::storage_rows(&app.storage).len();
            render_table(
                f,
                rows[1],
                format!(
                    "{}· [v] Services ",
                    count_title("Storage", data.len(), total, app)
                ),
                &crate::monitor::STORAGE_HEADERS,
                &[
                    Constraint::Length(20),
                    Constraint::Length(18),
                    Constraint::Length(11),
                    Constraint::Min(20),
                ],
                data,
                &mut app.monitor_state,
                server_colour(&app.server_name),
                |_, _| None,
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

/// The uptime watchlist: what is enrolled, and what it last answered.
pub(super) fn render_uptime(f: &mut Frame, area: Rect, app: &mut App) {
    const NOTE_W: usize = 22;
    let tint = server_colour(&app.server_name);
    let rows_data = crate::uptime::ranked(&app.watch, &app.probes);

    if rows_data.is_empty() {
        // An empty screen that explains itself. The watchlist is deliberately
        // empty until you fill it, so "nothing here" must say what to do rather
        // than look like a failure to load.
        let msg = Paragraph::new(vec![
            Line::from(""),
            Line::from("  Nothing is being watched yet."),
            Line::from(""),
            Line::from("  Go to Domains [6], put the cursor on a domain and press [w]."),
            Line::from("  Only what you enrol is checked — never the whole list."),
        ])
        .block(pane(" Uptime ", tint));
        f.render_widget(msg, area);
        return;
    }

    let median = crate::uptime::median_head(&app.probes);
    // "TTFB" rather than "Server": everywhere else in this app a server is an
    // EasyPanel host, and a column of milliseconds under that word reads as
    // something about the host rather than about the request.
    let headers = ["", "URL", "Code", "TTFB", "Total", "Note"];
    // The URL is the identity column, so it takes the slack; the rest are numbers
    // of known width.
    let widths = [
        Constraint::Length(2),
        Constraint::Min(24),
        Constraint::Length(5),
        Constraint::Length(9),
        Constraint::Length(9),
        // Wide enough for a reason. The first draft gave this 10 columns and put
        // failures in the 9-column TTFB cell, so "could not connect" rendered as
        // "could no" and "1.8× slower" as "1.8× slowe" — cutting the one cell
        // that explains the row.
        Constraint::Length(NOTE_W as u16),
    ];
    let url_w = area
        .width
        .saturating_sub(2 + 2 + 5 + 9 + 9 + NOTE_W as u16 + 5 + 4)
        .max(20) as usize;

    let rows: Vec<Row> = rows_data
        .iter()
        .map(|(check, probe)| {
            let (mark, style) = match probe {
                None => ("·", Style::default().fg(Color::DarkGray)),
                Some(p) => match p.verdict(check) {
                    crate::uptime::Verdict::Working => {
                        ("●", Style::default().fg(Color::Indexed(2)))
                    }
                    // Answered, but wrongly: a different problem from silence, so
                    // a different colour rather than the same red.
                    crate::uptime::Verdict::Unexpected => {
                        ("●", Style::default().fg(Color::Indexed(214)))
                    }
                    crate::uptime::Verdict::Unreachable => (
                        "●",
                        Style::default()
                            .fg(Color::Indexed(196))
                            .add_modifier(Modifier::BOLD),
                    ),
                },
            };
            // The method is part of the identity when it is not a GET, so it goes
            // in FRONT of the url — and the whole thing is cut together. Cutting
            // the url first and prefixing afterwards made the row longer than its
            // column, so ratatui clipped it silently: the exact bug this table
            // was written to avoid.
            let label = match check.method.as_str() {
                "GET" => check.url.clone(),
                m => format!("{m} {}", check.url),
            };
            let url = crate::output::first_line(&label, url_w);
            let cells = match probe {
                None => vec![
                    mark.to_string(),
                    url,
                    "-".into(),
                    "-".into(),
                    "-".into(),
                    "not checked".into(),
                ],
                Some(p) => match &p.outcome {
                    // No numbers at all, because there are none: showing a time
                    // next to a domain that never answered would invent one.
                    crate::uptime::Outcome::Failed(why) => vec![
                        mark.to_string(),
                        url,
                        "-".into(),
                        "-".into(),
                        "-".into(),
                        crate::output::first_line(why, NOTE_W),
                    ],
                    crate::uptime::Outcome::Answered {
                        status,
                        head,
                        total,
                    } => vec![
                        mark.to_string(),
                        url,
                        status.to_string(),
                        crate::uptime::human(*head),
                        crate::uptime::human(*total),
                        match crate::uptime::slowness(p, median) {
                            // A number nobody can calibrate becomes one anybody
                            // can: how this domain compares with its peers, right
                            // now, over the same network.
                            Some(x) if x >= 1.5 => format!("{x:.1}× slower"),
                            Some(_) => "—".into(),
                            None => "-".into(),
                        },
                    ],
                },
            };
            Row::new(cells).style(style)
        })
        .collect();

    let title = match (app.checking, median) {
        (true, _) => format!(" Uptime ({}) · checking… ", app.watch.len()),
        (false, Some(m)) => format!(
            " Uptime ({}) · median {} · [r] check ",
            app.watch.len(),
            crate::uptime::human(m)
        ),
        (false, None) => format!(" Uptime ({}) · [r] check ", app.watch.len()),
    };
    let table = Table::new(rows, widths)
        .header(
            Row::new(headers).style(
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
        )
        .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("› ")
        .block(pane(title, tint));
    f.render_stateful_widget(table, area, &mut app.uptime_state);
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
                    crate::monitor::load_avg(&s)
                ),
                format!("load {}", crate::monitor::load_avg(&s)),
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
        f.render_widget(
            pane(format!(" {label} "), server_colour(&app.server_name)),
            cols[i],
        );
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
    let block = pane(
        format!(" Terminal · {} · Ctrl-Q exit ", app.term.title),
        server_colour(&app.server_name),
    );
    let inner = block.inner(area);
    f.render_widget(block, area);

    let (cols, rows) = (inner.width.max(1), inner.height.max(1));
    let Some(parser) = app.term.parser.as_mut() else {
        return;
    };
    // Keep the shell's size aligned with the pane. vt100 uses (rows, cols).
    if parser.screen().size() != (rows, cols) {
        parser.set_size(rows, cols);
        if let Some(tx) = app.term.input.as_ref() {
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
    let max_scroll = (app.viewer.lines.len() as u16).saturating_sub(rows);
    app.viewer.scroll = if app.viewer.follow {
        max_scroll
    } else {
        // Clamped on EVERY path, not just while following: Down and PageDown add
        // without an upper bound, so holding either used to scroll past the last
        // line into a blank bordered box that looks like an empty log.
        app.viewer.scroll.min(max_scroll)
    };
    let block = |app: &App| {
        pane(String::new(), server_colour(&app.server_name))
            .title(format!(
                " {}{} ",
                app.viewer.title,
                // Say so if it's really live. Without this, a quiet log can't be
                // told apart from a dead tail.
                match (app.viewer.log_cursor.is_some(), app.viewer.follow) {
                    (true, true) => " · live",
                    (true, false) => " · live (paused — End to follow again)",
                    _ => "",
                }
            ))
            .title_bottom(viewer_actions(app))
            .title_bottom(if app.viewer.hscroll > 0 {
                // Say where you are once scrolled: otherwise a view missing its
                // left edge looks like the content simply starts there.
                format!(" ← col {} · Home to return ", app.viewer.hscroll + 1)
            } else {
                String::new()
            })
            // Discoverability: a diff or a bulk-result line wider than the pane
            // is cut at the edge, and until v0.73 nothing said it could scroll —
            // the ← indicator only appeared AFTER you had already scrolled, which
            // you had no reason to try. Announce it while it is still relevant.
            .title_bottom(
                Line::from(
                    if app.viewer.hscroll == 0 && viewer_overflows(app, area.width) {
                        " →  more · ←→ scroll ".to_string()
                    } else {
                        String::new()
                    },
                )
                .right_aligned(),
            )
    };

    // A collection is a LIST with a highlighted row; everything else is prose.
    // Selecting a line in a log would mean nothing, but selecting a port is the
    // whole point — it is what `x` deletes, without the ten-row ceiling the old
    // "press the digit on the line" had.
    // An empty collection has a PLACEHOLDER line, not a row. Highlighting it made
    // "No ports yet" look like something you had selected and could delete.
    let has_rows = app.viewer.lines.iter().any(|l| is_row(l));
    if has_rows && app.viewer_is_collection() {
        // A one-column Table rather than a List, so the selection moves with the
        // SAME helper every other table here uses — ↑↓, PageUp/PageDown, Home/End
        // all behave as they do elsewhere instead of being a second scheme.
        let rows: Vec<Row> = app
            .viewer
            .lines
            .iter()
            .map(|l| Row::new(vec![l.clone()]))
            .collect();
        if app.viewer.row.selected().is_none() && !rows.is_empty() {
            app.viewer.row.select(Some(0));
        }
        let table = Table::new(rows, [Constraint::Min(10)])
            .block(block(app))
            .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED))
            .highlight_symbol("› ");
        f.render_stateful_widget(table, area, &mut app.viewer.row);
        return;
    }

    f.render_widget(
        Paragraph::new(app.viewer.lines.join("\n"))
            .block(block(app))
            .scroll((app.viewer.scroll, app.viewer.hscroll)),
        area,
    );
}

/// What you can DO to what this viewer is showing.
///
/// Each collection is one screen — see it, add to it, delete from it — so the
/// screen says which keys do that. These were separate menu entries: findable,
/// but disconnected from the thing they act on.
/// Is any viewer line wider than the pane? Then content is cut at the right edge
/// and horizontal scroll is worth advertising.
///
/// Counted in characters against the inner width (the two borders removed). The
/// hscroll offset is added back so the answer stays true as you scroll: a line
/// long enough to still overflow at column 40 must keep saying there is more.
pub(super) fn viewer_overflows(app: &App, area_width: u16) -> bool {
    let inner = area_width.saturating_sub(2) as usize;
    let reach = inner + app.viewer.hscroll as usize;
    app.viewer.lines.iter().any(|l| l.chars().count() > reach)
}

pub(super) fn viewer_actions(app: &App) -> String {
    use super::worker::View;
    match app.viewer.ctx.as_ref().map(|(v, ..)| *v) {
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
                Span::styled(
                    "  ↑↓ select · Enter apply · Esc cancel",
                    bar.fg(Color::Indexed(244)),
                ),
            ]))
            .style(bar),
            area,
        );
        return;
    }

    // The Cloudflare workspace surfaces its per-screen keys HERE — the header now
    // carries the product tab bar instead. While typing the CF-local filter, show
    // how to apply/cancel it, matching the EasyPanel filter line above.
    if app.workspace == Workspace::Cloudflare {
        let text = if app.cf.filter_input {
            format!(" filter: {}▏  Enter apply · Esc cancel", app.cf.filter)
        } else {
            let hints = match app.cf.product {
                CfProduct::R2 => match app.cf.screen {
                    CfScreen::Objects => CF_OBJECTS_HINTS,
                    _ => CF_BUCKETS_HINTS,
                },
                CfProduct::Dns => cf_status_hints(app.cf.screen),
            };
            format!(" {hints}")
        };
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(text, bar.fg(Color::Indexed(244))))).style(bar),
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
    // Wide enough for what it has to SAY. 64 columns is the comfortable default,
    // but a fixed width silently cut a form's own explanation in half — "only
    // ones on shared remote stora" — and a sentence that stops mid-word is worse
    // than no sentence, because the reader cannot tell what was withheld.
    // Counted in characters: the titles carry ·, — and ⌄.
    let widest = title
        .chars()
        .count()
        .max(form.note.as_deref().map_or(0, |n| n.chars().count()))
        .max(form.error.as_deref().map_or(0, |e| e.chars().count()));
    // +4: the two borders and a space either side of the text.
    let width = (widest as u16 + 4).clamp(64, f.area().width);
    // `_w` — COLUMNS. This measured itself carefully and then handed the number
    // to `centered_abs`, whose first parameter is a percentage, so at 80 columns
    // a form that needed 68 was drawn 54 wide: the measurement was doing its job
    // and the result was thrown away. Two helpers one character apart.
    let area = centered_abs_w(width, height, f.area());
    f.render_widget(Clear, area);

    let mut block = Block::bordered()
        .title(title.clone())
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

pub(super) fn render_confirm(f: &mut Frame, c: &Confirm, server: &str) {
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
    // blank + label + blank + server + target + blank + keys, plus the borders.
    let h = (label_lines + 8).min(full.height);
    // COLUMNS, like `w` and `inner` above. As a percentage the box came out
    // narrower than the width the wrap was calculated with, so the label ran to
    // more lines than the height allowed for and the line naming the keys could
    // fall out of the bottom — on the dialog for irreversible actions.
    let area = centered_abs_w(w, h, full);
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
    // WHICH MACHINE. With several hosts configured, the answer to "am I about to
    // change the right one?" was only in the frame's title behind this very
    // dialog. It is the last thing read before pressing y, so it is on its own
    // line, in that server's colour, above the target it applies to.
    let tint = server_colour(server);
    let body = Paragraph::new(vec![
        Line::from(""),
        Line::from(c.label.clone()),
        Line::from(""),
        Line::from(Span::styled(
            format!("on {server}"),
            Style::default().fg(tint).add_modifier(Modifier::BOLD),
        )),
        Line::from(target),
        Line::from(""),
        Line::from("[y] Yes      [n] Cancel"),
    ]);
    f.render_widget(
        body.alignment(Alignment::Center)
            // Wrap, never truncate: a half-read question about deleting things is
            // worse than a taller dialog.
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

/// Centre a box `pct_x` PERCENT of the width. For a box measured in columns use
/// `centered_abs_w` — passing columns here silently reads them as a percentage,
/// which is how two dialogs came to be sized by a number that meant something
/// else entirely.
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
