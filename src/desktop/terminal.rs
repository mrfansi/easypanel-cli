//! Target-bound interactive PTYs and live service logs, shared by dock and document.
use super::model::{OpenPage, PageContext, PageKind, Payload, Target};
use crate::{
    client::EasypanelClient,
    session::{self, SessionEvent, TermMsg},
};
use eframe::egui::{self, Color32, FontId, Key, Modifiers, Rect, Sense, Vec2};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, VecDeque},
    sync::mpsc::{self, Receiver, Sender},
    time::{Duration, Instant},
};

const HISTORY: usize = 10_000;

pub(super) struct State {
    target: Option<Target>,
    input: Option<Sender<TermMsg>>,
    events: Option<Receiver<SessionEvent>>,
    parser: vt100::Parser,
    size: (u16, u16),
    selection: Option<Selection>,
    connected: bool,
    opened: bool,
    error: String,
    review_host: bool,
    host_entry: String,
    review_close: bool,
    font_size: f32,
    scroll_remainder: f32,
    logs: LogState,
}
impl Default for State {
    fn default() -> Self {
        Self {
            target: None,
            input: None,
            events: None,
            parser: vt100::Parser::new(24, 80, HISTORY),
            size: (80, 24),
            selection: None,
            connected: false,
            opened: false,
            error: String::new(),
            review_host: false,
            host_entry: String::new(),
            review_close: false,
            font_size: 13.0,
            scroll_remainder: 0.0,
            logs: LogState::default(),
        }
    }
}
impl State {
    pub(super) fn is_dirty(&self) -> bool {
        self.logs.criteria_changed()
    }
    pub(super) fn has_running_session(&self) -> bool {
        self.input.is_some()
    }
    pub(super) fn tick(&mut self, cx: &mut PageContext<'_>) {
        if self
            .target
            .as_ref()
            .is_some_and(|target| !target.same_resource(cx.target))
        {
            return;
        }
        self.pump();
        if let Some(Err(error)) = cx.take("terminal-open") {
            self.error = error;
            self.disconnect();
        }
        if matches!(cx.target.section.as_str(), "logs" | "log-search") {
            tick_logs(&mut self.logs, cx);
        }
    }
    fn disconnect(&mut self) {
        self.input = None;
        self.events = None;
        self.connected = false;
        self.review_close = false;
        self.selection = None;
        self.scroll_remainder = 0.0;
    }
    fn send(&mut self, text: String) {
        if let Some(input) = &self.input {
            if input.send(TermMsg::Input(text)).is_err() {
                self.error = "The terminal connection ended. Reconnect to open a new shell.".into();
                self.disconnect();
            } else {
                self.parser.set_scrollback(0);
                self.selection = None;
                self.scroll_remainder = 0.0;
            }
        }
    }
    pub(super) fn pump(&mut self) {
        for _ in 0..512 {
            let event = self.events.as_ref().and_then(|rx| rx.try_recv().ok());
            match event {
                Some(SessionEvent::Output(bytes)) => {
                    self.connected = true;
                    self.parser.process(&bytes);
                }
                Some(SessionEvent::Error(error)) => self.error = error,
                Some(SessionEvent::Closed) => {
                    self.input = None;
                    self.connected = false;
                    break;
                }
                None => break,
            }
        }
    }
}

pub(super) fn show(ui: &mut egui::Ui, state: &mut State, cx: &mut PageContext<'_>) {
    show_mode(ui, state, cx, false);
}

pub(super) fn show_utility(ui: &mut egui::Ui, state: &mut State, cx: &mut PageContext<'_>) {
    show_mode(ui, state, cx, true);
}

fn show_mode(ui: &mut egui::Ui, state: &mut State, cx: &mut PageContext<'_>, compact: bool) {
    if let Some(target) = &state.target {
        if !target.same_resource(cx.target) {
            super::widgets::error(ui, "This session belongs to another target. Open a new document instead of retargeting a running session.");
            return;
        }
    } else {
        state.target = Some(cx.target.clone());
    }
    if matches!(cx.target.section.as_str(), "logs" | "log-search") {
        show_logs(ui, &mut state.logs, cx, compact);
        return;
    }
    state.tick(cx);
    if compact {
        ui.horizontal(|ui| terminal_toolbar(ui, state, cx, true));
    } else {
        ui.horizontal_wrapped(|ui| terminal_toolbar(ui, state, cx, false));
    }
    if state.review_host {
        if compact {
            egui::Window::new("Open privileged host shell")
                .id(ui.scope_id().with("host-shell-review"))
                .collapsible(false)
                .resizable(false)
                .default_width(440.0)
                .show(ui.ctx(), |ui| host_shell_review(ui, state, cx));
        } else {
            host_shell_review(ui, state, cx);
        }
    }
    if state.review_close {
        if compact {
            egui::Window::new("Disconnect terminal")
                .id(ui.scope_id().with("terminal-disconnect-review"))
                .collapsible(false)
                .resizable(false)
                .default_width(400.0)
                .show(ui.ctx(), |ui| {
                    ui.label(cx.target.label());
                    disconnect_review(ui, state);
                });
        } else {
            disconnect_review(ui, state);
        }
    }
    if !state.error.is_empty() {
        let error = redact_log(&state.error, cx.target);
        if compact {
            ui.add(
                egui::Label::new(egui::RichText::new(&error).color(ui.visuals().error_fg_color))
                    .truncate(),
            )
            .on_hover_text(error);
        } else {
            super::widgets::error(ui, &error);
        }
    }
    if !compact {
        if state.parser.screen().scrollback() > 0 {
            ui.label(format!(
                "Scrollback: {} lines above live output",
                state.parser.screen().scrollback()
            ));
        }
        ui.small("Click terminal to type · Ctrl+C interrupts · Cmd+C / Ctrl+Shift+C copies selection · paste supports Unicode and multiline text · Shift+PageUp browses history");
    }
    terminal_canvas(ui, state);
    if state.input.is_some() {
        ui.ctx().request_repaint_after(Duration::from_millis(33));
    }
}

fn terminal_toolbar(ui: &mut egui::Ui, state: &mut State, cx: &mut PageContext<'_>, compact: bool) {
    if !compact {
        ui.strong(if cx.target.section == "host" {
            "Host root shell"
        } else if cx.target.section == "db" {
            "Database shell"
        } else {
            "Container shell"
        });
        ui.label(cx.target.label());
        ui.separator();
    }
    ui.label(if state.connected {
        "Connected"
    } else if state.input.is_some() {
        "Connecting…"
    } else {
        "Disconnected"
    });
    if state.input.is_some() {
        if ui.button("Disconnect").clicked() {
            state.review_close = true;
        }
    } else if ui
        .add_enabled(
            !cx.busy("terminal-open"),
            egui::Button::new(if state.opened { "Reconnect" } else { "Connect" }),
        )
        .clicked()
    {
        if cx.target.section == "host" {
            state.review_host = true;
            state.host_entry.clear();
        } else {
            open_session(ui.ctx(), state, cx);
        }
    }
    if ui.button("Copy selection").clicked() {
        copy_selection(ui, state);
    }
    if ui.button("Live output").clicked() {
        state.parser.set_scrollback(0);
        state.selection = None;
        state.scroll_remainder = 0.0;
    }
    if compact {
        ui.menu_button("More", |ui| {
            ui.set_max_width(320.0);
            if ui.button("Copy screen").clicked() {
                ui.ctx().copy_text(state.parser.screen().contents());
            }
            ui.add(egui::Slider::new(&mut state.font_size, 10.0..=20.0).text("Font"));
            ui.label(format!("{} lines above live output", state.parser.screen().scrollback()));
            ui.small("Click to type. Ctrl+C interrupts; Cmd+C / Ctrl+Shift+C copies selection. Drag to select; wheel or Shift+PageUp browses history.");
        });
    } else {
        if ui.button("Copy screen").clicked() {
            ui.ctx().copy_text(state.parser.screen().contents());
        }
        ui.add(
            egui::Slider::new(&mut state.font_size, 10.0..=20.0)
                .text("Font")
                .show_value(false),
        );
    }
}

fn host_shell_review(ui: &mut egui::Ui, state: &mut State, cx: &mut PageContext<'_>) {
    ui.label("This opens a privileged root shell on the real host filesystem and namespaces. Commands can destroy host data. An administrator token is required.");
    let host = cx
        .target
        .server
        .as_ref()
        .map(|s| s.name.as_str())
        .unwrap_or("");
    if super::widgets::confirm(
        ui,
        "Open privileged host shell",
        host,
        Some(host),
        &mut state.host_entry,
    ) {
        state.review_host = false;
        open_session(ui.ctx(), state, cx);
    }
    if ui.button("Cancel host shell").clicked() {
        state.review_host = false;
    }
}

fn disconnect_review(ui: &mut egui::Ui, state: &mut State) {
    ui.label("Disconnect this session? Closing the socket terminates its remote shell.");
    ui.horizontal(|ui| {
        if ui.button("Disconnect session").clicked() {
            state.disconnect();
        }
        if ui.button("Keep connected").clicked() {
            state.review_close = false;
        }
    });
}

fn open_session(ctx: &egui::Context, state: &mut State, cx: &mut PageContext<'_>) {
    let Some(server) = cx.target.server.clone() else {
        state.error = "Choose an EasyPanel server first.".into();
        return;
    };
    let target = cx.target.clone();
    let (input, input_rx) = mpsc::channel();
    let (event_tx, events) = mpsc::sync_channel(256);
    let wake = ctx.clone();
    state.disconnect();
    state.opened = true;
    state.parser = vt100::Parser::new(state.size.1, state.size.0, HISTORY);
    state.input = Some(input);
    state.events = Some(events);
    state.error.clear();
    let (cols, rows) = state.size;
    cx.background("terminal-open", "Open interactive terminal", move || {
        let client = EasypanelClient::for_server(&server);
        let host = target.section == "host";
        let url = if host {
            crate::container::host_ws_url(&client)?
        } else {
            anyhow::ensure!(
                !target.project.is_empty() && !target.service.is_empty(),
                "Choose a running service for the container terminal."
            );
            let command = if target.section == "db" {
                let inspect = client.call(
                    &format!("services/{}", target.service_type),
                    "inspectService",
                    json!({"projectName":target.project,"serviceName":target.service}),
                )?;
                session::db_command(&target.service_type, &inspect)
                    .ok_or_else(|| anyhow::anyhow!("This service type has no database shell."))?
            } else {
                crate::container::INTERACTIVE_SHELL.to_owned()
            };
            crate::container::ws_url(&client, &target.project, &target.service, &command)?
        };
        session::spawn_session(
            url,
            move |event| {
                let _ = event_tx.send(event);
                wake.request_repaint();
            },
            input_rx,
            cols,
            rows,
            if host {
                " — this route requires an administrator API token"
            } else {
                " — check this server's API token"
            },
        );
        Ok(Payload::Text("Terminal transport started".into()))
    });
}

#[derive(Clone, Copy)]
struct Selection {
    anchor: (i32, u16),
    end: (i32, u16),
    scrollback: usize,
}
impl Selection {
    fn new(row: u16, col: u16, scrollback: usize) -> Self {
        Self {
            anchor: (row.into(), col),
            end: (row.into(), col),
            scrollback,
        }
    }
    fn extend(&mut self, row: u16, col: u16, sb: usize) {
        self.end = (i32::from(row) + self.scrollback as i32 - sb as i32, col);
    }
    fn ordered(self) -> ((i32, u16), (i32, u16)) {
        if self.anchor <= self.end {
            (self.anchor, self.end)
        } else {
            (self.end, self.anchor)
        }
    }
    fn contains(self, row: u16, col: u16, sb: usize) -> bool {
        let p = (i32::from(row) + self.scrollback as i32 - sb as i32, col);
        let (start, end) = self.ordered();
        self.anchor != self.end && p >= start && p <= end
    }
    fn text(self, screen: &vt100::Screen) -> String {
        if self.anchor == self.end {
            return String::new();
        }
        let (rows, cols) = screen.size();
        let (a, b) = self.ordered();
        let top = a.0 - self.scrollback as i32 + screen.scrollback() as i32;
        let bottom = b.0 - self.scrollback as i32 + screen.scrollback() as i32;
        if bottom < 0 || top >= i32::from(rows) {
            return String::new();
        }
        screen
            .contents_between(
                top.max(0) as u16,
                if top < 0 { 0 } else { a.1.min(cols - 1) },
                bottom.min(i32::from(rows) - 1) as u16,
                if bottom >= i32::from(rows) {
                    cols
                } else {
                    b.1.saturating_add(1).min(cols)
                },
            )
            .split('\n')
            .map(str::trim_end)
            .collect::<Vec<_>>()
            .join("\n")
    }
}
fn copy_selection(ui: &egui::Ui, state: &State) {
    if let Some(selection) = state.selection {
        let text = selection.text(state.parser.screen());
        if !text.is_empty() {
            ui.ctx().copy_text(text);
        }
    }
}

fn grid_size(area: Vec2, cell: Vec2) -> (u16, u16) {
    (
        (area.x / cell.x.max(1.0))
            .floor()
            .clamp(1.0, u16::MAX as f32) as u16,
        (area.y / cell.y.max(1.0))
            .floor()
            .clamp(1.0, u16::MAX as f32) as u16,
    )
}
fn scroll_rows(remainder: &mut f32, points: f32, row_height: f32) -> i32 {
    *remainder += points / row_height.max(1.0);
    let rows = remainder.trunc() as i32;
    *remainder -= rows as f32;
    rows
}

fn terminal_canvas(ui: &mut egui::Ui, state: &mut State) {
    let font = FontId::monospace(state.font_size);
    let cell = ui.fonts_mut(|f| {
        Vec2::new(
            f.glyph_width(&font, 'M').max(1.0),
            f.row_height(&font).max(1.0),
        )
    });
    let available = ui.available_size().max(Vec2::new(cell.x, cell.y));
    let (rect, response) = ui.allocate_exact_size(available, Sense::click_and_drag());
    let inner = rect.shrink(6.0);
    let size = grid_size(inner.size(), cell);
    if size != state.size {
        state.size = size;
        state.parser.set_size(size.1, size.0);
        state.selection = None;
        if let Some(input) = &state.input {
            let _ = input.send(TermMsg::Resize(size.0, size.1));
        }
    }
    if response.clicked() || response.drag_started() {
        response.request_focus();
    }
    if ui
        .ctx()
        .data_mut(|data| {
            data.remove_temp::<bool>(egui::Id::unique("desktop-terminal-focus-request"))
        })
        .unwrap_or(false)
    {
        response.request_focus();
    }
    let has_focus = response.has_focus();
    ui.ctx()
        .data_mut(|data| data.insert_temp(egui::Id::unique("desktop-terminal-focus"), has_focus));
    if has_focus {
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(
                response.id,
                egui::EventFilter {
                    tab: true,
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    escape: true,
                },
            )
        });
    }
    let sb = state.parser.screen().scrollback();
    if let Some(pos) = response.interact_pointer_pos() {
        let col = ((pos.x - inner.left()) / cell.x)
            .floor()
            .clamp(0.0, f32::from(size.0 - 1)) as u16;
        let row = ((pos.y - inner.top()) / cell.y)
            .floor()
            .clamp(0.0, f32::from(size.1 - 1)) as u16;
        if response.drag_started() {
            let origin = ui.input(|i| i.pointer.press_origin()).unwrap_or(pos);
            let col = ((origin.x - inner.left()) / cell.x)
                .floor()
                .clamp(0.0, f32::from(size.0 - 1)) as u16;
            let row = ((origin.y - inner.top()) / cell.y)
                .floor()
                .clamp(0.0, f32::from(size.1 - 1)) as u16;
            state.selection = Some(Selection::new(row, col, sb));
        }
        if response.dragged() {
            if let Some(selection) = &mut state.selection {
                selection.extend(row, col, sb);
            }
        }
    }
    if response.hovered() {
        // egui normalizes wheel/trackpad units to points. Consume the vertical
        // delta so enclosing scroll areas do not also scroll this gesture.
        let points = ui.input_mut(|input| std::mem::take(&mut input.smooth_scroll_delta.y));
        let rows = scroll_rows(&mut state.scroll_remainder, points, cell.y);
        if rows != 0 {
            state.parser.set_scrollback(if rows > 0 {
                sb.saturating_add(rows as usize)
            } else {
                sb.saturating_sub(rows.unsigned_abs() as usize)
            });
        }
    }
    if response.has_focus() {
        let events = ui.input_mut(|i| {
            let mut captured = Vec::new();
            i.events.retain(|event| {
                let take = matches!(
                    event,
                    egui::Event::Text(_)
                        | egui::Event::Paste(_)
                        | egui::Event::Copy
                        | egui::Event::Cut
                        | egui::Event::Key { .. }
                        | egui::Event::Ime(_)
                );
                if take {
                    captured.push(event.clone());
                }
                !take
            });
            captured
        });
        let modifiers = ui.input(|i| i.modifiers);
        let has_paste = events.iter().any(|e| matches!(e, egui::Event::Paste(_)));
        let has_text = events.iter().any(|e| matches!(e, egui::Event::Text(_)));
        let has_copy = events.iter().any(|e| matches!(e, egui::Event::Copy));
        let has_cut = events.iter().any(|e| matches!(e, egui::Event::Cut));
        for event in &events {
            if matches!(event, egui::Event::Copy | egui::Event::Cut) {
                if let Some(control) =
                    clipboard_control(event, modifiers, cfg!(target_os = "macos"))
                {
                    state.send(control.into());
                } else {
                    copy_selection(ui, state);
                }
                continue;
            }
            if let egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } = event
            {
                if modifiers.ctrl && ((has_copy && *key == Key::C) || (has_cut && *key == Key::X)) {
                    continue;
                }
            }
            if let egui::Event::Key {
                key: Key::PageUp | Key::PageDown,
                pressed: true,
                modifiers,
                ..
            } = event
            {
                if modifiers.shift {
                    let sb = state.parser.screen().scrollback();
                    state.parser.set_scrollback(
                        if matches!(
                            event,
                            egui::Event::Key {
                                key: Key::PageUp,
                                ..
                            }
                        ) {
                            sb.saturating_add(usize::from(size.1))
                        } else {
                            sb.saturating_sub(usize::from(size.1))
                        },
                    );
                    continue;
                }
            }
            if let egui::Event::Key {
                key: Key::C,
                pressed: true,
                modifiers,
                ..
            } = event
            {
                if modifiers.ctrl && modifiers.shift {
                    copy_selection(ui, state);
                    continue;
                }
            }
            if let egui::Event::Text(text) = event {
                if modifiers.alt && !modifiers.ctrl && !cfg!(target_os = "macos") {
                    state.send(format!("\u{1b}{text}"));
                    continue;
                }
            }
            if let Some(text) = terminal_input(
                event,
                state.parser.screen().application_cursor(),
                state.parser.screen().bracketed_paste(),
                has_paste,
                has_text,
            ) {
                state.send(text);
            }
        }
    }
    let bg = Color32::from_rgb(20, 23, 28);
    let fg = Color32::from_rgb(224, 229, 237);
    let painter = ui.painter().with_clip_rect(rect);
    painter.rect_filled(rect, 2.0, bg);
    let screen = state.parser.screen();
    let sb = screen.scrollback();
    for row in 0..size.1 {
        for col in 0..size.0 {
            let Some(c) = screen.cell(row, col) else {
                continue;
            };
            if c.is_wide_continuation() {
                continue;
            }
            let pos = inner.min + Vec2::new(f32::from(col) * cell.x, f32::from(row) * cell.y);
            let cell_rect = Rect::from_min_size(
                pos,
                Vec2::new(cell.x * if c.is_wide() { 2.0 } else { 1.0 }, cell.y),
            );
            let mut foreground = terminal_color(c.fgcolor(), fg, c.bold());
            let mut background = terminal_color(c.bgcolor(), bg, false);
            if c.inverse() {
                std::mem::swap(&mut foreground, &mut background);
            }
            if state.selection.is_some_and(|s| s.contains(row, col, sb)) {
                background = Color32::from_rgb(52, 81, 121);
            }
            if background != bg {
                painter.rect_filled(cell_rect, 0.0, background);
            }
            let text = c.contents();
            if !text.is_empty() {
                let mut job = egui::text::LayoutJob::default();
                job.append(
                    &text,
                    0.0,
                    egui::TextFormat {
                        font_id: font.clone(),
                        color: foreground,
                        italics: c.italic(),
                        underline: if c.underline() {
                            egui::Stroke::new(1.0, foreground)
                        } else {
                            egui::Stroke::NONE
                        },
                        ..Default::default()
                    },
                );
                painter.galley(pos, ui.fonts_mut(|f| f.layout_job(job)), foreground);
            }
        }
    }
    if response.has_focus() && !screen.hide_cursor() && sb == 0 {
        let (row, col) = screen.cursor_position();
        let pos = inner.min + Vec2::new(f32::from(col) * cell.x, f32::from(row) * cell.y);
        painter.rect_stroke(
            Rect::from_min_size(pos, cell),
            0.0,
            egui::Stroke::new(1.5, fg),
            egui::StrokeKind::Inside,
        );
    }
    response.widget_info(|| {
        let mut info = egui::WidgetInfo::labeled(
            egui::Role::Terminal,
            true,
            "Interactive terminal; click to focus, type to send input, drag to select text",
        );
        info.current_text_value = Some(screen.contents());
        info
    });
}

fn terminal_color(color: vt100::Color, default: Color32, bold: bool) -> Color32 {
    const ANSI: [[u8; 3]; 16] = [
        [0, 0, 0],
        [205, 49, 49],
        [13, 188, 121],
        [229, 229, 16],
        [36, 114, 200],
        [188, 63, 188],
        [17, 168, 205],
        [229, 229, 229],
        [102, 102, 102],
        [241, 76, 76],
        [35, 209, 139],
        [245, 245, 67],
        [59, 142, 234],
        [214, 112, 214],
        [41, 184, 219],
        [255, 255, 255],
    ];
    match color {
        vt100::Color::Default => default,
        vt100::Color::Rgb(r, g, b) => Color32::from_rgb(r, g, b),
        vt100::Color::Idx(mut i) => {
            if bold && i < 8 {
                i += 8;
            }
            let [r, g, b] = if i < 16 {
                ANSI[usize::from(i)]
            } else if i < 232 {
                let n = i - 16;
                let component = |c| if c == 0 { 0 } else { 55 + 40 * c };
                [component(n / 36), component(n / 6 % 6), component(n % 6)]
            } else {
                let v = 8 + (i - 232) * 10;
                [v, v, v]
            };
            Color32::from_rgb(r, g, b)
        }
    }
}

fn clipboard_control(
    event: &egui::Event,
    modifiers: Modifiers,
    macos: bool,
) -> Option<&'static str> {
    if macos || !modifiers.ctrl || modifiers.shift {
        return None;
    }
    match event {
        egui::Event::Copy => Some("\u{3}"),
        egui::Event::Cut => Some("\u{18}"),
        _ => None,
    }
}

fn terminal_input(
    event: &egui::Event,
    application_cursor: bool,
    bracketed_paste: bool,
    has_paste: bool,
    has_text: bool,
) -> Option<String> {
    match event {
        egui::Event::Text(text) => Some(text.clone()),
        egui::Event::Ime(egui::ImeEvent::Commit(text)) if !has_text => Some(text.clone()),
        egui::Event::Paste(text) => Some(if bracketed_paste {
            format!("\u{1b}[200~{text}\u{1b}[201~")
        } else {
            text.replace("\r\n", "\n").replace('\n', "\r")
        }),
        egui::Event::Key {
            key,
            pressed: true,
            modifiers: m,
            ..
        } => {
            if m.mac_cmd || (has_paste && matches!(key, Key::V | Key::Paste)) {
                return None;
            }
            if m.ctrl && m.alt && has_text {
                return None;
            } // AltGr produces Unicode text, not a control chord.
            let modifier = 1 + u8::from(m.shift) + 2 * u8::from(m.alt) + 4 * u8::from(m.ctrl);
            let arrow = match key {
                Key::ArrowUp => Some('A'),
                Key::ArrowDown => Some('B'),
                Key::ArrowRight => Some('C'),
                Key::ArrowLeft => Some('D'),
                Key::Home => Some('H'),
                Key::End => Some('F'),
                _ => None,
            };
            if let Some(code) = arrow {
                return Some(if modifier > 1 {
                    format!("\u{1b}[1;{modifier}{code}")
                } else {
                    format!("\u{1b}{}{code}", if application_cursor { 'O' } else { '[' })
                });
            }
            let function = match key {
                Key::F1 => Some('P'),
                Key::F2 => Some('Q'),
                Key::F3 => Some('R'),
                Key::F4 => Some('S'),
                _ => None,
            };
            if let Some(code) = function {
                return Some(if modifier > 1 {
                    format!("\u{1b}[1;{modifier}{code}")
                } else {
                    format!("\u{1b}O{code}")
                });
            }
            let numbered = match key {
                Key::Insert => Some(2),
                Key::Delete => Some(3),
                Key::PageUp => Some(5),
                Key::PageDown => Some(6),
                Key::F5 => Some(15),
                Key::F6 => Some(17),
                Key::F7 => Some(18),
                Key::F8 => Some(19),
                Key::F9 => Some(20),
                Key::F10 => Some(21),
                Key::F11 => Some(23),
                Key::F12 => Some(24),
                _ => None,
            };
            if let Some(code) = numbered {
                return Some(if modifier > 1 {
                    format!("\u{1b}[{code};{modifier}~")
                } else {
                    format!("\u{1b}[{code}~")
                });
            }
            let sequence = match key {
                Key::Enter => "\r",
                Key::Backspace => {
                    if m.ctrl {
                        "\u{8}"
                    } else {
                        "\u{7f}"
                    }
                }
                Key::Tab => {
                    if m.shift {
                        "\u{1b}[Z"
                    } else {
                        "\t"
                    }
                }
                Key::Escape => "\u{1b}",
                _ => "",
            };
            if !sequence.is_empty() {
                return Some(if m.alt && !sequence.starts_with('\u{1b}') {
                    format!("\u{1b}{sequence}")
                } else {
                    sequence.to_owned()
                });
            }
            let letter = match key {
                Key::A => 'a',
                Key::B => 'b',
                Key::C => 'c',
                Key::D => 'd',
                Key::E => 'e',
                Key::F => 'f',
                Key::G => 'g',
                Key::H => 'h',
                Key::I => 'i',
                Key::J => 'j',
                Key::K => 'k',
                Key::L => 'l',
                Key::M => 'm',
                Key::N => 'n',
                Key::O => 'o',
                Key::P => 'p',
                Key::Q => 'q',
                Key::R => 'r',
                Key::S => 's',
                Key::T => 't',
                Key::U => 'u',
                Key::V => 'v',
                Key::W => 'w',
                Key::X => 'x',
                Key::Y => 'y',
                Key::Z => 'z',
                Key::Space => ' ',
                Key::OpenBracket => '[',
                Key::Backslash => '\\',
                Key::CloseBracket => ']',
                Key::Num6 => '^',
                Key::Minus => '_',
                _ => return None,
            };
            if m.ctrl {
                return Some(
                    char::from(if letter == ' ' {
                        0
                    } else {
                        (letter.to_ascii_uppercase() as u8) & 0x1f
                    })
                    .to_string(),
                );
            }
            if m.alt && !has_text {
                return Some(format!("\u{1b}{letter}"));
            }
            None // Printable characters come only from Text/IME, never twice from Key.
        }
        _ => None,
    }
}

#[derive(Default)]
struct StreamCursor {
    timestamp: Option<String>,
    fallback: Vec<String>,
}
struct LogLine {
    project: String,
    service: String,
    text: String,
}
struct LogState {
    query: String,
    applied_query: String,
    start: String,
    applied_start: String,
    end: String,
    applied_end: String,
    stream: String,
    applied_stream: String,
    limit: u16,
    applied_limit: u16,
    filter: String,
    rendered: String,
    lines: VecDeque<LogLine>,
    cursors: HashMap<(String, String), StreamCursor>,
    errors: Vec<String>,
    follow: bool,
    started: bool,
    next_poll: Instant,
    refreshed: Option<Instant>,
    capped: bool,
    selected_service: Option<(String, String)>,
}
impl Default for LogState {
    fn default() -> Self {
        Self {
            query: String::new(),
            applied_query: String::new(),
            start: String::new(),
            applied_start: String::new(),
            end: String::new(),
            applied_end: String::new(),
            stream: String::new(),
            applied_stream: String::new(),
            limit: 200,
            applied_limit: 200,
            filter: String::new(),
            rendered: String::new(),
            lines: VecDeque::new(),
            cursors: HashMap::new(),
            errors: Vec::new(),
            follow: true,
            started: false,
            next_poll: Instant::now(),
            refreshed: None,
            capped: false,
            selected_service: None,
        }
    }
}

fn tick_logs(state: &mut LogState, cx: &mut PageContext<'_>) {
    let search_all = cx.target.section == "log-search";
    if let Some(result) = cx.take("log-poll") {
        match result {
            Ok(Payload::Json(batches)) => {
                state.errors.clear();
                if let Some(batches) = batches.as_array() {
                    for batch in batches {
                        let project = batch["project"].as_str().unwrap_or("");
                        let service = batch["service"].as_str().unwrap_or("");
                        if let Some(error) = batch["error"].as_str() {
                            state.errors.push(format!("{project}/{service}: {error}"));
                            continue;
                        }
                        let key = (project.to_owned(), service.to_owned());
                        let cursor = state.cursors.entry(key.clone()).or_default();
                        let lines = append_batch(cursor, &batch["result"]);
                        if lines.len() >= usize::from(state.applied_limit) {
                            state.capped = true;
                        }
                        for line in lines {
                            state.lines.push_back(LogLine {
                                project: key.0.clone(),
                                service: key.1.clone(),
                                text: redact_log(&line, cx.target),
                            });
                        }
                    }
                    while state.lines.len() > HISTORY {
                        state.lines.pop_front();
                        state.capped = true;
                    }
                    state.render(search_all);
                    state.refreshed = Some(Instant::now());
                } else {
                    state
                        .errors
                        .push("The panel returned an invalid log batch.".into());
                }
            }
            Err(error) => state.errors = vec![error],
            _ => state.errors = vec!["The log request returned an unexpected response.".into()],
        }
        state.next_poll = Instant::now() + Duration::from_secs(2);
    }
    if state.started && state.follow && state.next_poll <= Instant::now() && !cx.busy("log-poll") {
        poll_logs(state, cx, search_all);
    }
}

fn compact_log_toolbar(
    ui: &mut egui::Ui,
    state: &mut LogState,
    cx: &mut PageContext<'_>,
    search_all: bool,
) {
    let search_width = (ui.available_width() - 360.0).clamp(96.0, 220.0);
    ui.horizontal(|ui| {
        ui.checkbox(&mut state.follow, "Follow");
        ui.add(
            egui::TextEdit::singleline(&mut state.query)
                .desired_width(search_width)
                .hint_text("Search service logs"),
        );
        if ui
            .add_enabled(!cx.busy("log-poll"), egui::Button::new("Search"))
            .clicked()
        {
            if search_all && state.query.trim().is_empty() {
                state.errors = vec!["Enter a keyword to search across services.".into()];
            } else if state.valid_bounds() {
                state.apply_criteria();
                poll_logs(state, cx, search_all);
            }
        }
        if ui
            .button("Copy")
            .on_hover_text("Copy visible log lines")
            .clicked()
        {
            ui.ctx().copy_text(state.rendered.clone());
        }
        ui.menu_button("More", |ui| {
            ui.set_max_width(340.0);
            egui::ScrollArea::vertical()
                .max_height(320.0)
                .min_scrolled_height(0.0)
                .show(ui, |ui| {
                    ui.strong("Fetch criteria");
                    ui.horizontal(|ui| {
                        ui.label("Stream");
                        egui::ComboBox::from_id_salt("log-stream")
                            .selected_text(if state.stream.is_empty() {
                                "Both"
                            } else {
                                &state.stream
                            })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut state.stream, String::new(), "Both");
                                ui.selectable_value(&mut state.stream, "stdout".into(), "stdout");
                                ui.selectable_value(&mut state.stream, "stderr".into(), "stderr");
                            });
                    });
                    ui.label("From (Unix ns)");
                    ui.add(
                        egui::TextEdit::singleline(&mut state.start)
                            .desired_width(230.0)
                            .hint_text("Latest available"),
                    );
                    ui.label("Until (Unix ns)");
                    ui.add(
                        egui::TextEdit::singleline(&mut state.end)
                            .desired_width(230.0)
                            .hint_text("Now"),
                    );
                    ui.small("Search applies changed criteria and restarts the cursor.");
                    ui.separator();
                    ui.label("Filter visible lines");
                    if ui
                        .add(egui::TextEdit::singleline(&mut state.filter).desired_width(230.0))
                        .changed()
                    {
                        state.render(search_all);
                    }
                    if ui
                        .add_enabled(
                            state.started && !cx.busy("log-poll"),
                            egui::Button::new("Fetch next"),
                        )
                        .clicked()
                    {
                        state.started = true;
                        poll_logs(state, cx, search_all);
                    }
                    if ui.button("Clear view").clicked() {
                        state.lines.clear();
                        state.rendered.clear();
                    }
                    if let Some(refreshed) = state.refreshed {
                        ui.small(format!(
                            "Updated {}s ago; up to {} lines per service/request",
                            refreshed.elapsed().as_secs(),
                            state.applied_limit
                        ));
                    }
                    if let Some(cursor) = state
                        .cursors
                        .get(&(cx.target.project.clone(), cx.target.service.clone()))
                        .and_then(|cursor| cursor.timestamp.as_deref())
                    {
                        ui.small(format!("Cursor {cursor}"));
                    }
                    if state
                        .cursors
                        .values()
                        .any(|cursor| !cursor.fallback.is_empty())
                    {
                        ui.small(
                            "Panel omitted timestamps; overlapping snapshots are deduplicated.",
                        );
                    }
                    if state.capped {
                        ui.colored_label(
                            ui.visuals().warn_fg_color,
                            "Limit reached: history may be incomplete. Narrow the time range.",
                        );
                    }
                    for error in &state.errors {
                        super::widgets::error(ui, &redact_log(error, cx.target));
                    }
                    if search_all {
                        ui.separator();
                        log_service_picker(ui, state, cx);
                    }
                });
        });
        if cx.busy("log-poll") {
            ui.spinner();
        }
    });
    ui.horizontal(|ui| {
        ui.label("Rows per fetch");
        ui.add(egui::DragValue::new(&mut state.limit).range(1..=1000))
            .on_hover_text(format!(
                "Search applies this limit; current fetch uses {}.",
                state.applied_limit
            ));
        ui.weak(format!("{} lines", state.lines.len()));
        if state.criteria_changed() {
            ui.weak("Unapplied changes");
        }
        if let Some(error) = state.errors.first() {
            let error = redact_log(error, cx.target);
            ui.add(
                egui::Label::new(egui::RichText::new(&error).color(ui.visuals().error_fg_color))
                    .truncate(),
            )
            .on_hover_text(error);
        } else if state.capped {
            ui.add(
                egui::Label::new(
                    egui::RichText::new("History may be incomplete")
                        .color(ui.visuals().warn_fg_color),
                )
                .truncate(),
            )
            .on_hover_text("Fetch or history limit reached. Narrow the time range in More.");
        } else if !state.filter.is_empty() {
            ui.add(egui::Label::new(format!("Filter: {}", state.filter)).truncate())
                .on_hover_text(&state.filter);
        }
    });
}

fn log_service_picker(ui: &mut egui::Ui, state: &mut LogState, cx: &mut PageContext<'_>) {
    let mut services: Vec<_> = state.cursors.keys().cloned().collect();
    services.sort();
    ui.horizontal_wrapped(|ui| {
        egui::ComboBox::from_id_salt("log-hit-service")
            .selected_text(
                state
                    .selected_service
                    .as_ref()
                    .map(|(p, s)| format!("{p}/{s}"))
                    .unwrap_or_else(|| "Open a matching service".into()),
            )
            .show_ui(ui, |ui| {
                for service in &services {
                    ui.selectable_value(
                        &mut state.selected_service,
                        Some(service.clone()),
                        format!("{}/{}", service.0, service.1),
                    );
                }
            });
        if ui.button("Open service logs").clicked() {
            if let Some((project, service)) = &state.selected_service {
                let mut target = cx.target.clone();
                target.project = project.clone();
                target.service = service.clone();
                target.section = "logs".into();
                cx.open(OpenPage {
                    kind: PageKind::Terminal,
                    target,
                    title: format!("Logs · {project}/{service}"),
                });
            }
        }
    });
}

fn show_logs(ui: &mut egui::Ui, state: &mut LogState, cx: &mut PageContext<'_>, compact: bool) {
    let search_all = cx.target.section == "log-search";
    tick_logs(state, cx);
    if compact {
        compact_log_toolbar(ui, state, cx, search_all);
    } else {
        ui.horizontal_wrapped(|ui| {
            ui.strong(if search_all {
                "Search all service logs"
            } else {
                "Service logs"
            });
            ui.label(cx.target.label());
            if ui
                .add_enabled(
                    !cx.busy("log-poll"),
                    egui::Button::new(if search_all { "Search" } else { "Apply search" }),
                )
                .clicked()
            {
                if search_all && state.query.trim().is_empty() {
                    state.errors = vec!["Enter a keyword to search across services.".into()];
                } else if state.valid_bounds() {
                    state.apply_criteria();
                    poll_logs(state, cx, search_all);
                }
            }
            ui.checkbox(&mut state.follow, "Follow");
            if ui
                .add_enabled(
                    state.started && !cx.busy("log-poll"),
                    egui::Button::new("Fetch next"),
                )
                .clicked()
            {
                state.started = true;
                poll_logs(state, cx, search_all);
            }
            if ui.button("Copy visible").clicked() {
                ui.ctx().copy_text(state.rendered.clone());
            }
            if ui.button("Clear view").clicked() {
                state.lines.clear();
                state.rendered.clear();
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.label("Server search");
            ui.add(
                egui::TextEdit::singleline(&mut state.query)
                    .desired_width(220.0)
                    .hint_text("Keyword matched by the panel"),
            );
            ui.label("Stream");
            egui::ComboBox::from_id_salt("log-stream")
                .selected_text(if state.stream.is_empty() {
                    "Both"
                } else {
                    &state.stream
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut state.stream, String::new(), "Both");
                    ui.selectable_value(&mut state.stream, "stdout".into(), "stdout");
                    ui.selectable_value(&mut state.stream, "stderr".into(), "stderr");
                });
            ui.label("Rows per fetch");
            ui.add(egui::DragValue::new(&mut state.limit).range(1..=1000))
                .on_hover_text(
                    "Apply search to restart the query with this row limit and a fresh cursor.",
                );
            ui.label("Filter visible");
            if ui
                .add(
                    egui::TextEdit::singleline(&mut state.filter)
                        .desired_width(180.0)
                        .hint_text("Local text filter"),
                )
                .changed()
            {
                state.render(search_all);
            }
        });
        egui::CollapsingHeader::new("Time range")
            .default_open(ui.available_height() > 400.0)
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label("From (Unix ns)");
                    ui.add(
                        egui::TextEdit::singleline(&mut state.start)
                            .desired_width(165.0)
                            .hint_text("Latest available"),
                    );
                    ui.label("Until (Unix ns)");
                    ui.add(
                        egui::TextEdit::singleline(&mut state.end)
                            .desired_width(165.0)
                            .hint_text("Now"),
                    );
                    ui.small(
                        "Apply search to change server-side bounds; pausing keeps the cursor.",
                    );
                });
            });
        for error in &state.errors {
            super::widgets::error(ui, &redact_log(error, cx.target));
        }
        ui.horizontal_wrapped(|ui| {
            ui.small(format!(
                "{} buffered lines · {} · up to {} lines per service/request",
                state.lines.len(),
                if state.follow { "Following" } else { "Paused" },
                state.applied_limit
            ));
            if let Some(refreshed) = state.refreshed {
                ui.small(format!("Updated {}s ago", refreshed.elapsed().as_secs()));
            }
            if cx.busy("log-poll") {
                ui.spinner();
                ui.small("Fetching logs…");
            }
            if state.capped {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    "Limit reached: history may be incomplete. Narrow the time range.",
                );
            }
            if !search_all {
                if let Some(cursor) = state
                    .cursors
                    .get(&(cx.target.project.clone(), cx.target.service.clone()))
                    .and_then(|c| c.timestamp.as_deref())
                {
                    ui.small(format!("Cursor {cursor}"));
                }
            }
            if state.cursors.values().any(|c| !c.fallback.is_empty()) {
                ui.small("Panel omitted timestamps; overlapping snapshots are deduplicated.");
            }
        });
        if search_all {
            log_service_picker(ui, state, cx);
        }
    }
    if !state.started && !search_all {
        state.started = true;
        poll_logs(state, cx, false);
    }
    if !compact && !state.started && search_all {
        ui.label("Search across every service on this server. Per-service failures remain visible alongside matches.");
    } else if !compact && state.lines.is_empty() && !cx.busy("log-poll") && state.errors.is_empty()
    {
        ui.label("No log lines match this search and time range.");
    }
    egui::ScrollArea::both()
        .max_width(ui.available_width().max(0.0))
        .max_height(ui.available_height().max(0.0))
        .min_scrolled_width(0.0)
        .min_scrolled_height(0.0)
        .id_salt("log-output")
        .stick_to_bottom(state.follow)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if compact && state.rendered.is_empty() {
                ui.weak(if cx.busy("log-poll") {
                    "Fetching logs…"
                } else if !state.started && search_all {
                    "Enter a keyword and select Search."
                } else {
                    "No matching log lines."
                });
            }
            let mut text = state.rendered.as_str();
            ui.add(
                egui::TextEdit::multiline(&mut text)
                    .font(egui::TextStyle::Monospace)
                    .desired_width(f32::INFINITY)
                    .desired_rows(if compact { 1 } else { 8 })
                    .code_editor(),
            );
        });
    if state.started && state.follow {
        ui.ctx().request_repaint_after(Duration::from_millis(500));
    }
}

impl LogState {
    fn criteria_changed(&self) -> bool {
        self.query != self.applied_query
            || self.start != self.applied_start
            || self.end != self.applied_end
            || self.stream != self.applied_stream
            || self.limit != self.applied_limit
    }
    fn apply_criteria(&mut self) {
        self.applied_query = self.query.clone();
        self.applied_start = self.start.clone();
        self.applied_end = self.end.clone();
        self.applied_stream = self.stream.clone();
        self.applied_limit = self.limit;
        self.lines.clear();
        self.cursors.clear();
        self.rendered.clear();
        self.errors.clear();
        self.selected_service = None;
        self.capped = false;
        self.started = true;
        self.next_poll = Instant::now();
    }
    fn valid_bounds(&mut self) -> bool {
        self.errors.clear();
        if !(1..=1000).contains(&self.limit) {
            self.errors
                .push("Rows per fetch must be between 1 and 1,000.".into());
        }
        for (label, value) in [("From", &self.start), ("Until", &self.end)] {
            if !value.is_empty() && value.parse::<u64>().is_err() {
                self.errors
                    .push(format!("{label} must be a Unix timestamp in nanoseconds."));
            }
        }
        if let (Ok(start), Ok(end)) = (self.start.parse::<u64>(), self.end.parse::<u64>()) {
            if start > end {
                self.errors
                    .push("From must not be later than Until.".into());
            }
        }
        self.errors.is_empty()
    }
    fn render(&mut self, search_all: bool) {
        self.rendered.clear();
        for line in &self.lines {
            if !self.filter.is_empty()
                && !line.text.contains(&self.filter)
                && !line.project.contains(&self.filter)
                && !line.service.contains(&self.filter)
            {
                continue;
            }
            if !self.rendered.is_empty() {
                self.rendered.push('\n');
            }
            if search_all {
                self.rendered.push_str(&line.project);
                self.rendered.push('/');
                self.rendered.push_str(&line.service);
                self.rendered.push_str("  ");
            }
            self.rendered.push_str(&line.text);
        }
    }
}

fn poll_logs(state: &mut LogState, cx: &mut PageContext<'_>, search_all: bool) {
    if cx.busy("log-poll") {
        return;
    }
    if search_all && state.applied_query.trim().is_empty() {
        state.errors = vec!["Enter a keyword and run Search first.".into()];
        state.follow = false;
        return;
    }
    let Some(server) = cx.target.server.clone() else {
        state.errors = vec!["Choose an EasyPanel server for logs.".into()];
        state.follow = false;
        return;
    };
    if !search_all && (cx.target.project.is_empty() || cx.target.service.is_empty()) {
        state.errors =
            vec!["Choose a service for live logs, or use Search all service logs.".into()];
        state.follow = false;
        return;
    }
    let scope = (cx.target.project.clone(), cx.target.service.clone());
    let query = state.applied_query.clone();
    let start = state.applied_start.clone();
    let end = state.applied_end.clone();
    let stream = state.applied_stream.clone();
    let limit = state.applied_limit;
    let cursors: HashMap<_, _> = state
        .cursors
        .iter()
        .filter_map(|(key, cursor)| {
            cursor
                .timestamp
                .as_ref()
                .map(|ts| (key.clone(), ts.clone()))
        })
        .collect();
    state.next_poll = Instant::now() + Duration::from_secs(2);
    cx.background_poll("log-poll", if search_all { "Search service logs" } else { "Follow service logs" }, move || {
        let client = EasypanelClient::for_server(&server);
        let services = if search_all {
            let inventory = client.call("projects","listProjectsAndServices",Value::Null)?;
            let services = inventory["services"].as_array().ok_or_else(||anyhow::anyhow!("The panel returned no services collection."))?;
            services.iter().filter_map(|v|Some((v["projectName"].as_str()?.to_owned(),v["name"].as_str()?.to_owned()))).collect::<Vec<_>>()
        } else { vec![scope] };
        // The same per-service endpoint as the CLI; bounded parallel batches retain partial errors.
        let mut batches = Vec::with_capacity(services.len());
        for chunk in services.chunks(8) {
            let results = std::thread::scope(|threads| {
                let jobs:Vec<_> = chunk.iter().map(|(project,service)| {
                    let client = &client; let query=&query; let start=&start; let end=&end; let stream=&stream;
                    let cursor=cursors.get(&(project.clone(),service.clone()));
                    threads.spawn(move || {
                        let mut input=json!({"projectName":project,"serviceName":service,"limit":limit});
                        if !query.is_empty() { input["search"]=json!(query); }
                        let next = cursor.and_then(|ts|ts.parse::<u64>().ok()).and_then(|n|n.checked_add(1)).map(|n|n.to_string());
                        if let Some(next)=next { input["start"]=json!(next); } else if !start.is_empty() { input["start"]=json!(start); }
                        if !end.is_empty() { input["end"]=json!(end); }
                        if !stream.is_empty() { input["stream"]=json!(stream); }
                        match client.call("logs","queryServiceLogs",input) {
                            Ok(result)=>json!({"project":project,"service":service,"result":result}),
                            Err(error)=>json!({"project":project,"service":service,"error":error.to_string()}),
                        }
                    })
                }).collect();
                jobs.into_iter().map(|job|job.join().unwrap_or_else(|_|json!({"error":"A log query worker stopped unexpectedly."}))).collect::<Vec<_>>()
            });
            batches.extend(results);
        }
        Ok(Payload::Json(Value::Array(batches)))
    });
}

fn timestamp_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    a.len().cmp(&b.len()).then_with(|| a.cmp(b))
}

fn append_batch(cursor: &mut StreamCursor, result: &Value) -> Vec<String> {
    if let Some(entries) = result.get("entries").and_then(Value::as_array) {
        let mut filtered = Vec::with_capacity(entries.len());
        for entry in entries {
            let Some(values) = entry["values"].as_array() else {
                continue;
            };
            let values: Vec<_> = values
                .iter()
                .filter(|pair| {
                    pair[0].as_str().is_some_and(|ts| {
                        cursor
                            .timestamp
                            .as_ref()
                            .is_none_or(|previous| timestamp_cmp(ts, previous).is_gt())
                    })
                })
                .cloned()
                .collect();
            filtered.push(json!({"values":values}));
        }
        let filtered = json!({"entries":filtered});
        if let Some(newest) = crate::logs::newest_ts(&filtered) {
            cursor.timestamp = Some(newest);
        }
        crate::logs::format(&filtered)
    } else {
        let lines = crate::logs::format(result);
        // Older panels omit timestamps: keep only the non-overlapping tail of their snapshots.
        let overlap = (0..=cursor.fallback.len().min(lines.len()))
            .rev()
            .find(|&n| cursor.fallback[cursor.fallback.len() - n..] == lines[..n])
            .unwrap_or(0);
        let fresh = lines[overlap..].to_vec();
        cursor.fallback = lines;
        fresh
    }
}
fn redact_log(text: &str, target: &Target) -> String {
    let mut text = text.to_owned();
    if let Some(server) = &target.server {
        if !server.token.is_empty() {
            text = text.replace(&server.token, "[redacted]");
        }
        if let Some(secret) = server
            .ssh
            .as_ref()
            .and_then(|s| s.secret.as_ref())
            .filter(|s| !s.is_empty())
        {
            text = text.replace(secret, "[redacted]");
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    fn key(key: Key, modifiers: Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }
    fn input(event: egui::Event) -> Option<String> {
        terminal_input(&event, false, false, false, false)
    }

    #[test]
    fn unconnected_terminal_paints_at_dock_sizes_before_and_after_focus() {
        for size in [Vec2::new(900.0, 220.0), Vec2::new(640.0, 130.0)] {
            let ctx = egui::Context::default();
            let mut state = State {
                target: Some(Target {
                    project: "offline-project".into(),
                    service: "offline-service".into(),
                    section: "terminal".into(),
                    ..Default::default()
                }),
                ..Default::default()
            };
            for focused in [false, true] {
                if focused {
                    ctx.data_mut(|data| {
                        data.insert_temp(egui::Id::unique("desktop-terminal-focus-request"), true);
                    });
                }
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, size)),
                        ..Default::default()
                    },
                    |ui| terminal_canvas(ui, &mut state),
                );
                // Headless frames have no renderer to consume texture uploads.
                output.textures_delta.clear();
                assert!(!output.shapes.is_empty(), "terminal must paint a frame");
                assert_eq!(
                    ctx.data(|data| {
                        data.get_temp::<bool>(egui::Id::unique("desktop-terminal-focus"))
                    }),
                    Some(focused),
                );
                assert!(state.size.0 > 0 && state.size.1 > 0);
                assert!(!state.connected && state.input.is_none() && state.events.is_none());
            }
        }
    }

    #[test]
    fn terminal_controls_repeats_and_printable_events_do_not_double_send() {
        let ctrl = Modifiers {
            ctrl: true,
            ..Default::default()
        };
        assert_eq!(input(key(Key::C, ctrl)), Some("\u{3}".into()));
        assert_eq!(input(key(Key::P, ctrl)), Some("\u{10}".into()));
        assert_eq!(input(key(Key::K, ctrl)), Some("\u{b}".into()));
        assert_eq!(input(key(Key::A, Modifiers::default())), None);
        assert_eq!(input(egui::Event::Text("é中".into())), Some("é中".into()));
        let mut repeat = key(Key::ArrowUp, Modifiers::default());
        if let egui::Event::Key { repeat, .. } = &mut repeat {
            *repeat = true;
        }
        assert_eq!(input(repeat), Some("\u{1b}[A".into()));
        assert_eq!(
            terminal_input(&key(Key::V, ctrl), false, false, true, false),
            None
        );
        assert_eq!(
            input(key(
                Key::C,
                Modifiers {
                    mac_cmd: true,
                    command: true,
                    ..Default::default()
                }
            )),
            None
        );
    }
    #[test]
    fn platform_clipboard_shortcuts_preserve_terminal_control_keys() {
        let ctrl = Modifiers {
            ctrl: true,
            command: true,
            ..Default::default()
        };
        assert_eq!(
            clipboard_control(&egui::Event::Copy, ctrl, false),
            Some("\u{3}")
        );
        assert_eq!(
            clipboard_control(&egui::Event::Cut, ctrl, false),
            Some("\u{18}")
        );
        assert_eq!(clipboard_control(&egui::Event::Copy, ctrl, true), None);
        assert_eq!(
            clipboard_control(
                &egui::Event::Copy,
                Modifiers {
                    shift: true,
                    ..ctrl
                },
                false
            ),
            None
        );
        assert_eq!(
            clipboard_control(&egui::Event::Copy, Modifiers::default(), false),
            None
        );
    }
    #[test]
    fn unicode_multiline_paste_and_application_cursor_modes() {
        let paste = egui::Event::Paste("é中\r\nsecond\n".into());
        assert_eq!(
            terminal_input(&paste, false, true, true, false),
            Some("\u{1b}[200~é中\r\nsecond\n\u{1b}[201~".into())
        );
        assert_eq!(
            terminal_input(&paste, false, false, true, false),
            Some("é中\rsecond\r".into())
        );
        assert_eq!(
            terminal_input(
                &key(Key::ArrowUp, Modifiers::default()),
                true,
                false,
                false,
                false
            ),
            Some("\u{1b}OA".into())
        );
        assert_eq!(
            terminal_input(
                &key(
                    Key::Q,
                    Modifiers {
                        ctrl: true,
                        alt: true,
                        ..Default::default()
                    }
                ),
                false,
                false,
                false,
                true
            ),
            None
        );
        assert_eq!(
            terminal_input(
                &egui::Event::Ime(egui::ImeEvent::Commit("中".into())),
                false,
                false,
                false,
                true
            ),
            None
        );
    }
    #[test]
    fn modified_function_and_navigation_keys_keep_xterm_modifiers() {
        let shift = Modifiers {
            shift: true,
            ..Default::default()
        };
        let ctrl = Modifiers {
            ctrl: true,
            ..Default::default()
        };
        let alt = Modifiers {
            alt: true,
            ..Default::default()
        };
        assert_eq!(input(key(Key::F1, shift)), Some("\u{1b}[1;2P".into()));
        assert_eq!(input(key(Key::F4, ctrl)), Some("\u{1b}[1;5S".into()));
        assert_eq!(input(key(Key::F5, shift)), Some("\u{1b}[15;2~".into()));
        assert_eq!(input(key(Key::F12, alt)), Some("\u{1b}[24;3~".into()));
        assert_eq!(input(key(Key::Insert, shift)), Some("\u{1b}[2;2~".into()));
        assert_eq!(input(key(Key::Delete, ctrl)), Some("\u{1b}[3;5~".into()));
        assert_eq!(input(key(Key::PageUp, ctrl)), Some("\u{1b}[5;5~".into()));
        assert_eq!(input(key(Key::PageDown, alt)), Some("\u{1b}[6;3~".into()));
        assert_eq!(
            input(key(Key::F2, Modifiers::default())),
            Some("\u{1b}OQ".into())
        );
        assert_eq!(
            input(key(Key::F6, Modifiers::default())),
            Some("\u{1b}[17~".into())
        );
    }
    #[test]
    fn measured_geometry_tracks_font_and_tiny_panes() {
        assert_eq!(
            grid_size(Vec2::new(800.0, 480.0), Vec2::new(8.0, 16.0)),
            (100, 30)
        );
        assert_eq!(
            grid_size(Vec2::new(800.0, 480.0), Vec2::new(10.0, 20.0)),
            (80, 24)
        );
        assert_eq!(grid_size(Vec2::ZERO, Vec2::new(8.0, 16.0)), (1, 1));
    }
    #[test]
    fn paused_log_cursor_resumes_without_repeated_lines_or_scope_collisions() {
        let mut a = StreamCursor::default();
        let mut b = StreamCursor::default();
        let first = json!({"entries":[{"values":[["9","old"],["10","new"]]}]});
        assert_eq!(append_batch(&mut a, &first).len(), 2);
        assert!(append_batch(&mut a, &first).is_empty());
        assert_eq!(append_batch(&mut b, &first).len(), 2);
        let resumed = json!({"entries":[{"values":[["10","new"],["11","after pause"]]}]});
        let rows = append_batch(&mut a, &resumed);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].ends_with("after pause"));
        assert_eq!(a.timestamp.as_deref(), Some("11"));
        assert_eq!(b.timestamp.as_deref(), Some("10"));
    }
    #[test]
    fn timestamp_less_panel_snapshots_only_append_new_suffix() {
        let mut cursor = StreamCursor::default();
        assert_eq!(
            append_batch(&mut cursor, &json!(["a", "b"])),
            vec!["a", "b"]
        );
        assert_eq!(
            append_batch(&mut cursor, &json!(["a", "b"])),
            Vec::<String>::new()
        );
        assert_eq!(append_batch(&mut cursor, &json!(["b", "c"])), vec!["c"]);
    }
    #[test]
    fn selection_is_stable_while_scrolling_and_respects_wrapped_text() {
        let mut parser = vt100::Parser::new(3, 5, 20);
        parser.process(b"abcdefghi\r\nnext");
        let mut selection = Selection::new(0, 0, 0);
        selection.extend(1, 3, 0);
        assert_eq!(selection.text(parser.screen()), "abcdefghi");
        assert!(selection.contains(0, 2, 0));
        assert!(selection.contains(1, 2, 1));
    }
    #[test]
    fn closing_one_session_does_not_disconnect_another() {
        let (a, a_rx) = mpsc::channel();
        let (b, b_rx) = mpsc::channel();
        let mut state = State {
            input: Some(a),
            ..Default::default()
        };
        state.disconnect();
        assert!(matches!(
            a_rx.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
        b.send(TermMsg::Input("still here".into())).unwrap();
        assert!(matches!(b_rx.try_recv(), Ok(TermMsg::Input(_))));
    }
    #[test]
    fn document_close_guard_tracks_unapplied_log_search_edits() {
        let mut state = State::default();
        assert!(!state.is_dirty());
        state.logs.query = "connection refused".into();
        assert!(state.is_dirty());
        state.logs.applied_query = state.logs.query.clone();
        assert!(!state.is_dirty());
        state.logs.start = "200".into();
        state.logs.end = "100".into();
        assert!(!state.logs.valid_bounds());
        assert!(state.is_dirty());
        state.logs.start = "not a timestamp".into();
        assert!(!state.logs.valid_bounds());
    }

    #[test]
    fn smooth_wheel_points_accumulate_without_rounding_each_frame_to_a_row() {
        let mut remainder = 0.0;
        assert_eq!(scroll_rows(&mut remainder, 4.0, 16.0), 0);
        assert_eq!(scroll_rows(&mut remainder, 4.0, 16.0), 0);
        assert_eq!(scroll_rows(&mut remainder, 8.0, 16.0), 1);
        assert_eq!(remainder, 0.0);
        assert_eq!(scroll_rows(&mut remainder, -40.0, 16.0), -2);
        assert_eq!(remainder, -0.5);
        assert_eq!(scroll_rows(&mut remainder, 8.0, 16.0), 0);
        assert_eq!(remainder, 0.0);
        assert_eq!(scroll_rows(&mut remainder, 40.0, 20.0), 2);
    }

    #[test]
    fn applying_changed_fetch_criteria_restarts_history_and_cursor_at_chosen_limit() {
        let mut state = State::default();
        state.logs.cursors.insert(
            ("project".into(), "service".into()),
            StreamCursor {
                timestamp: Some("11".into()),
                fallback: Vec::new(),
            },
        );
        state.logs.lines.push_back(LogLine {
            project: "project".into(),
            service: "service".into(),
            text: "old result".into(),
        });
        state.logs.render(false);
        state.logs.capped = true;
        state.logs.selected_service = Some(("project".into(), "service".into()));
        state.logs.limit = 25;
        state.logs.query = "refused".into();
        state.logs.start = "12".into();
        state.logs.end = "30".into();
        state.logs.stream = "stderr".into();
        assert!(state.is_dirty());
        assert_eq!(state.logs.applied_limit, 200); // An in-flight fetch retains its applied criteria.
        assert!(state.logs.valid_bounds());
        state.logs.apply_criteria();
        assert!(!state.is_dirty());
        assert_eq!(state.logs.applied_limit, 25);
        assert_eq!(state.logs.applied_query, "refused");
        assert_eq!(state.logs.applied_start, "12");
        assert_eq!(state.logs.applied_end, "30");
        assert_eq!(state.logs.applied_stream, "stderr");
        assert!(state.logs.cursors.is_empty());
        assert!(state.logs.lines.is_empty());
        assert!(state.logs.rendered.is_empty());
        assert!(state.logs.selected_service.is_none());
        assert!(!state.logs.capped);
        assert!(state.logs.started);
        for limit in [0, 1001] {
            state.logs.limit = limit;
            assert!(!state.logs.valid_bounds());
        }
        for limit in [1, 1000] {
            state.logs.limit = limit;
            assert!(state.logs.valid_bounds());
        }
    }
}
