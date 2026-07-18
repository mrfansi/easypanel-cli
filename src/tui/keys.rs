//! What happens when a key is pressed.
//!
//! A second `impl App`, deliberately in another file: state and selectors live
//! in `app.rs`, while this is purely "key -> action". Merging them would make one
//! file that can't be read in a single sitting.

use std::sync::mpsc::Sender;

use ratatui::crossterm::event::{KeyCode, MouseButton, MouseEvent, MouseEventKind};
use ratatui::widgets::ListState;
use serde_json::json;

use crate::output::field;

use super::app::{App, Confirm, Menu, MonitorView, Screen, ServerAction, TAB_SCREENS};
use super::form::*;
use super::table::*;
use super::worker::{Req, View};

impl App {
    pub(super) fn on_key(&mut self, code: KeyCode, req: &Sender<Req>) {
        // Help closes on any key: the user opened it to read, not to memorize how
        // to get out.
        // Help still closes on any key the user might reach for — except the ones
        // that scroll it. It is longer than a short terminal, and a reader who
        // presses ↓ to see the rest should not have it slam shut instead.
        if self.help {
            match code {
                KeyCode::Up | KeyCode::Char('k') => {
                    self.help_scroll = self.help_scroll.saturating_sub(1)
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.help_scroll = self.help_scroll.saturating_add(1)
                }
                KeyCode::PageUp => self.help_scroll = self.help_scroll.saturating_sub(10),
                KeyCode::PageDown => self.help_scroll = self.help_scroll.saturating_add(10),
                KeyCode::Home => self.help_scroll = 0,
                // Render clamps this to the real end; reaching for End must not
                // slam the help shut.
                KeyCode::End => self.help_scroll = u16::MAX,
                _ => self.help = false,
            }
            return;
        }
        if self.palette.is_some() {
            self.palette_key(code, req);
            return;
        }
        if self.menu.is_some() {
            self.menu_key(code, req);
            return;
        }
        if self.filter_input {
            self.filter_key(code);
            return;
        }
        if self.chooser.is_some() {
            self.chooser_key(code, req);
            return;
        }
        if self.form.is_some() {
            self.form_key(code, req);
            return;
        }
        if self.confirm.is_some() {
            self.confirm_key(code, req);
            return;
        }
        if self.picker.is_some() {
            self.picker_key(code, req);
            return;
        }

        match code {
            // Arrived at Domains via `o` from a service: Esc goes back to Services
            // (not just clearing its scope filter).
            KeyCode::Esc if self.domain_scope.is_some() => self.goto(Screen::Projects, req),
            KeyCode::Esc if !self.filter.is_empty() => self.clear_filter(),
            // Esc does NOT quit the app. Esc means "cancel": it closes a form,
            // dropdown, confirmation, or filter — and when there's nothing to
            // cancel, it does nothing. Closing the TUI on a single reflexive Esc
            // is losing context without warning. Quit: 'q' or Ctrl-C.
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('1') => self.screen = Screen::Dashboard,
            KeyCode::Char('2') => self.goto(Screen::Hosts, req),
            KeyCode::Char('3') => self.goto(Screen::Maintenance, req),
            KeyCode::Char('4') => self.goto(Screen::Actions, req),
            KeyCode::Char('5') => self.goto(Screen::Monitor, req),
            KeyCode::Char('6') => self.goto(Screen::Domains, req),
            KeyCode::Char('7') => self.goto(Screen::Projects, req),
            KeyCode::Tab => self.goto(self.screen.next(), req),
            // ←/→ move between tabs (e.g. Services ↔ Domains). Menus & forms grab
            // the arrows first (above), so this only applies in ordinary table
            // navigation.
            // In the viewer these scroll sideways instead of switching tab. A
            // full-screen reader is not a tab, and pressing → to see the rest of a
            // cut log line used to throw you out of the logs onto another screen —
            // losing your place to reach content that was there all along. Esc and
            // 1-7 still leave, so nothing became unreachable.
            KeyCode::Left | KeyCode::Right if self.screen == Screen::Viewer => {
                const STEP: u16 = 8;
                self.viewer_hscroll = if code == KeyCode::Right {
                    self.viewer_hscroll.saturating_add(STEP)
                } else {
                    self.viewer_hscroll.saturating_sub(STEP)
                };
            }
            KeyCode::Right => self.goto(self.screen.next(), req),
            KeyCode::Left => self.goto(self.screen.prev(), req),
            // Space opens the action menu for the selected row — the keyboard
            // version of a right click. Empty (a screen with no row actions) does
            // nothing.
            KeyCode::Char(' ') => {
                let items = self.context_items();
                // On Services an empty menu means nothing is selected at all — say
                // so rather than swallowing the key. (A project header row has its
                // own menu, so it is NOT empty.) Other screens have rows without
                // actions, where doing nothing is the honest answer.
                if items.is_empty() && self.screen == Screen::Projects {
                    self.status = "Select a row first".into();
                } else {
                    self.open_menu(items);
                }
            }
            // Global search / command palette: jump quickly to a service/tab
            // without menus. A keyboard alternative for those who dislike browsing
            // menus.
            KeyCode::Char(':') => self.open_palette(),
            KeyCode::Char('?') => {
                self.help = true;
                self.help_scroll = 0;
            }
            KeyCode::Char('s') => self.open_picker(),
            KeyCode::Char('r') => self.refresh(req),
            KeyCode::Char('/') if self.filterable() => {
                self.filter_input = true;
                self.filter.clear();
            }
            _ => match self.screen {
                Screen::Projects => self.services_key(code, req),
                Screen::Viewer => self.viewer_key(code),
                Screen::Actions => self.actions_key(code, req),
                Screen::Domains => self.domains_key(code, req),
                Screen::Monitor => self.monitor_key(code, req),
                Screen::Hosts => match code {
                    // Hosts used to be the one screen with no row action at all —
                    // you could see a host was DOWN and had no way to ask why.
                    KeyCode::Enter => self.open_host_detail(),
                    _ => move_table(&mut self.hosts_state, code, self.hosts.len()),
                },
                Screen::Maintenance => self.maint_key(code),
                // Terminal is handled directly in event_loop (encode_key), not
                // here. Dashboard has no dedicated keys.
                Screen::Dashboard | Screen::Terminal => {}
            },
        }
    }

    /// Clicks & scroll. The context menu captures the mouse while open; other
    /// modals (form/picker/confirmation/dropdown/help) swallow it — a click must
    /// never quietly switch tabs behind a dialog. A terminal session ignores it.
    pub(super) fn on_mouse(&mut self, m: MouseEvent, req: &Sender<Req>) {
        // The palette is keyboard-driven; swallow the mouse so a click doesn't
        // fall through to the table.
        if self.palette.is_some() {
            return;
        }
        // The context menu & dropdown capture the mouse themselves (hover/click
        // their items).
        if self.menu.is_some() {
            self.menu_mouse(m, req);
            return;
        }
        if self.chooser.is_some() {
            self.chooser_mouse(m, req);
            return;
        }
        if self.form.is_some() {
            self.form_mouse(m);
            return;
        }
        // Other modals swallow the mouse: a click must not fall through behind the
        // dialog.
        if self.screen == Screen::Terminal
            || self.help
            || self.picker.is_some()
            || self.confirm.is_some()
        {
            return;
        }
        match m.kind {
            // Scroll moves the viewport; the selection stays under the cursor so it
            // doesn't fight the follow-cursor. The viewer scrolls its text.
            MouseEventKind::ScrollDown => self.on_scroll(3),
            MouseEventKind::ScrollUp => self.on_scroll(-3),
            MouseEventKind::Down(MouseButton::Left) => self.on_click(m.column, m.row, req),
            MouseEventKind::Down(MouseButton::Right) => self.on_right_click(m.column, m.row),
            // The highlight follows the cursor: moving the mouse over a row selects
            // it. Outside the table area select_row_at does nothing, so the
            // selection holds.
            MouseEventKind::Moved | MouseEventKind::Drag(_) => {
                self.select_row_at(m.column, m.row);
            }
            _ => {}
        }
    }

    /// Scroll: the viewer scrolls its text; a table moves the viewport AND the
    /// selection together, so the highlight stays on the same screen row (= stays
    /// under the cursor). That's what stops scroll from fighting hover — the next
    /// mouse move selects the same row.
    fn on_scroll(&mut self, delta: isize) {
        if self.screen == Screen::Viewer {
            let step = delta.unsigned_abs() as u16;
            if delta < 0 {
                self.viewer_follow = false;
                self.viewer_scroll = self.viewer_scroll.saturating_sub(step);
            } else {
                self.viewer_scroll = self.viewer_scroll.saturating_add(step);
            }
            return;
        }
        let len = self.visible_table_len() as isize;
        if len == 0 {
            return;
        }
        // Data rows that fit = area height - top/bottom border - header.
        let visible = (self.table_area.height as isize - 3).max(0);
        // Max offset: stop when the last page is full. If the whole list fits
        // (max_off = 0), there's nothing to scroll.
        let max_off = (len - visible).max(0);
        if let Some(state) = self.active_table() {
            let off = state.offset() as isize;
            let new_off = (off + delta).clamp(0, max_off);
            let applied = new_off - off;
            if applied == 0 {
                return; // no room to scroll -> selection does NOT move
            }
            // Shift the selection by however much the offset actually moved: the
            // highlight stays on the same screen row (= stays under the cursor).
            let sel = state.selected().unwrap_or(0) as isize;
            state.select(Some((sel + applied).clamp(0, len - 1) as usize));
            *state.offset_mut() = new_off as usize;
        }
    }

    /// Mouse on a form: clicking a field focuses it, then — for Bool/Choice/Editor
    /// — activates it right away (toggle / open dropdown / open $EDITOR). A text
    /// field is just focused; the user then types.
    fn form_mouse(&mut self, m: MouseEvent) {
        if !matches!(m.kind, MouseEventKind::Down(MouseButton::Left)) {
            return;
        }
        let idx = {
            let Some(form) = self.form.as_ref() else {
                return;
            };
            let r = form.rect;
            if m.column < r.x || m.column >= r.x.saturating_add(r.width) || m.row < r.y {
                return;
            }
            let slot = (m.row - r.y) as usize;
            match form.visible_here().get(slot) {
                Some(&i) => i,
                None => return,
            }
        };
        let Some(form) = self.form.as_mut() else {
            return;
        };
        form.focus = idx;
        match form.fields[idx].kind {
            FieldKind::Bool => {
                form.fields[idx].cycle();
                form.clamp_focus();
            }
            FieldKind::Editor => self.edit_field = Some(idx),
            FieldKind::Choice(_) => self.open_chooser(),
            _ => {}
        }
    }

    /// Mouse on an open dropdown: hover highlights the option under the cursor, a
    /// click selects it (same as Enter), a click outside / scroll navigates.
    fn chooser_mouse(&mut self, m: MouseEvent, req: &Sender<Req>) {
        let Some(ch) = self.chooser.as_mut() else {
            return;
        };
        match m.kind {
            MouseEventKind::Moved | MouseEventKind::Drag(_) => {
                if let Some(i) = ch.item_at(m.column, m.row) {
                    ch.state.select(Some(i));
                }
            }
            MouseEventKind::ScrollUp => {
                let i = ch.state.selected().unwrap_or(0);
                ch.state.select(Some(i.saturating_sub(1)));
            }
            MouseEventKind::ScrollDown => {
                let last = ch.matches().len().saturating_sub(1);
                let i = ch.state.selected().unwrap_or(0);
                ch.state.select(Some((i + 1).min(last)));
            }
            MouseEventKind::Down(_) => match ch.item_at(m.column, m.row) {
                Some(i) => {
                    ch.state.select(Some(i));
                    self.apply_chooser(req);
                }
                None => self.chooser = None,
            },
            _ => {}
        }
    }

    /// Apply the dropdown choice to the form field (same for Enter and click).
    fn apply_chooser(&mut self, req: &Sender<Req>) {
        let Some(ch) = self.chooser.as_ref() else {
            return;
        };
        let picked = ch.selected();
        let (idx, label) = (ch.field, ch.label);
        self.chooser = None;
        if let (Some(value), Some(form)) = (picked, self.form.as_mut()) {
            form.fields[idx].value = value;
            form.clamp_focus();
            match label {
                "Project" => self.load_form_services(req),
                "Repo" => {
                    if let Some(f) = form.fields.iter_mut().find(|f| f.label == "Branch") {
                        f.value.clear();
                    }
                    self.load_form_branches(req);
                }
                _ => {}
            }
        }
    }

    fn on_click(&mut self, col: u16, row: u16, req: &Sender<Req>) {
        // Click a tab -> switch to it (same as pressing its number).
        if row == self.tab_row {
            if let Some(i) = self
                .tab_spans
                .iter()
                .position(|&(a, b)| col >= a && col < b)
            {
                if let Some(&screen) = TAB_SCREENS.get(i) {
                    self.goto(screen, req);
                }
            }
            return;
        }
        self.select_row_at(col, row);
    }

    /// Right-clicking a row selects it, then opens its action menu. With no action
    /// for that row/screen, no menu appears.
    fn on_right_click(&mut self, col: u16, row: u16) {
        if row == self.tab_row {
            return;
        }
        self.select_row_at(col, row);
        let items = self.context_items();
        if !items.is_empty() {
            let mut state = ListState::default();
            state.select(Some(0));
            self.menu = Some(Menu {
                items,
                state,
                parent: None,
                col,
                row,
                rect: ratatui::layout::Rect::default(),
            });
        }
    }

    /// Select the row under (col,row) in the active screen's table. The top two
    /// rows (border + header) aren't data; offset accounts for a scrolled list.
    /// True when a row was actually selected.
    fn select_row_at(&mut self, col: u16, row: u16) -> bool {
        let a = self.table_area;
        let first = a.y.saturating_add(2); // top border + header
        let last = a.y.saturating_add(a.height).saturating_sub(1); // bottom border (exclusive)
        if col < a.x || col >= a.x.saturating_add(a.width) || row < first || row >= last {
            return false;
        }
        let vis = (row - first) as usize;
        let len = self.visible_table_len();
        if let Some(state) = self.active_table() {
            let idx = vis + state.offset();
            if idx < len {
                state.select(Some(idx));
                return true;
            }
        }
        false
    }

    /// Navigate the context menu by keyboard.
    fn menu_key(&mut self, code: KeyCode, req: &Sender<Req>) {
        let Some(menu) = self.menu.as_mut() else {
            return;
        };
        match code {
            KeyCode::Esc => self.menu = None,
            KeyCode::Up | KeyCode::Char('k') => {
                let i = menu.state.selected().unwrap_or(0);
                menu.state.select(Some(i.saturating_sub(1)));
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let i = menu.state.selected().unwrap_or(0);
                let last = menu.items.len().saturating_sub(1);
                menu.state.select(Some((i + 1).min(last)));
            }
            // → enter submenu / run (same as Enter); ← back to the parent menu, or
            // close if already at the top menu.
            KeyCode::Enter | KeyCode::Right => self.activate_menu(req),
            KeyCode::Left => {
                self.menu = self.menu.take().and_then(|m| m.parent).map(|p| *p);
            }
            _ => {}
        }
    }

    /// Mouse while the menu is open: scroll navigates, a click on an item
    /// activates it, a click outside closes the menu.
    fn menu_mouse(&mut self, m: MouseEvent, req: &Sender<Req>) {
        let Some(menu) = self.menu.as_mut() else {
            return;
        };
        match m.kind {
            MouseEventKind::ScrollUp => {
                let i = menu.state.selected().unwrap_or(0);
                menu.state.select(Some(i.saturating_sub(1)));
            }
            MouseEventKind::ScrollDown => {
                let i = menu.state.selected().unwrap_or(0);
                let last = menu.items.len().saturating_sub(1);
                menu.state.select(Some((i + 1).min(last)));
            }
            // The highlight follows the cursor inside the menu.
            MouseEventKind::Moved | MouseEventKind::Drag(_) => {
                if let Some(i) = menu.item_at(m.column, m.row) {
                    menu.state.select(Some(i));
                }
            }
            MouseEventKind::Down(_) => match menu.item_at(m.column, m.row) {
                Some(i) => {
                    menu.state.select(Some(i));
                    self.activate_menu(req);
                }
                // Click outside the menu -> close without acting.
                None => self.menu = None,
            },
            _ => {}
        }
    }

    /// Run the selected menu item (its action function). A ▸ item opens a submenu.
    fn activate_menu(&mut self, req: &Sender<Req>) {
        let run = self.menu.as_ref().and_then(|menu| {
            menu.state
                .selected()
                .and_then(|i| menu.items.get(i))
                .map(|it| it.run)
        });
        // Take the menu now; `run` may open a submenu (a ▸ item) by setting self.menu.
        let parent = self.menu.take();
        if let Some(run) = run {
            run(self, req);
        }
        // If `run` opened a submenu, record the old menu as its parent so `←` can
        // go back. If `run` was a leaf action (menu stayed None), drop the parent.
        if let (Some(child), Some(parent)) = (self.menu.as_mut(), parent) {
            child.parent = Some(Box::new(parent));
        }
    }

    /// Command palette: typing filters, ↑↓ selects (within the filtered list),
    /// Enter jumps, Esc closes, Backspace deletes. Typing resets the highlight to
    /// the top.
    fn palette_key(&mut self, code: KeyCode, req: &Sender<Req>) {
        let Some(pal) = self.palette.as_mut() else {
            return;
        };
        match code {
            KeyCode::Esc => self.palette = None,
            KeyCode::Enter => self.palette_run(req),
            KeyCode::Up => {
                let i = pal.state.selected().unwrap_or(0);
                pal.state.select(Some(i.saturating_sub(1)));
            }
            KeyCode::Down => {
                let last = pal.matches().len().saturating_sub(1);
                let i = pal.state.selected().unwrap_or(0);
                pal.state.select(Some((i + 1).min(last)));
            }
            KeyCode::Backspace => {
                pal.query.pop();
                pal.state.select(Some(0));
            }
            KeyCode::Char(c) => {
                pal.query.push(c);
                pal.state.select(Some(0));
            }
            _ => {}
        }
    }

    /// Actions: Enter (or View in the context menu) opens the action detail —
    /// metadata + deploy/action log — in the viewer. Everything else is table
    /// navigation.
    pub(super) fn actions_key(&mut self, code: KeyCode, req: &Sender<Req>) {
        match code {
            KeyCode::Enter => {
                if let Some(id) = self.selected_action_id() {
                    self.viewer_from = Screen::Actions;
                    // Not a log-tail view: make sure the log poll doesn't latch onto it.
                    self.viewer_ctx = None;
                    self.status = "Loading action detail...".into();
                    let _ = req.send(Req::ActionDetail(id));
                }
            }
            _ => {
                let n = self.visible_actions().len();
                move_table(&mut self.actions_state, code, n);
            }
        }
    }

    pub(super) fn filter_key(&mut self, code: KeyCode) {
        match code {
            // Esc cancels the filter entirely; Enter keeps it and returns to
            // ordinary navigation.
            KeyCode::Esc => self.clear_filter(),
            KeyCode::Enter => self.filter_input = false,
            KeyCode::Backspace => {
                self.filter.pop();
                self.clamp_filtered();
            }
            KeyCode::Char(c) => {
                self.filter.push(c);
                self.clamp_filtered();
            }
            _ => {}
        }
    }

    /// Docker cleanup is destructive and irreversible, so every action goes
    /// through a confirmation — just like deploy/destroy.
    pub(super) fn maint_key(&mut self, code: KeyCode) {
        let (op, label) = match code {
            KeyCode::Char('p') => (
                "systemPrune",
                "Prune the Docker system? Unused containers, networks, images, and build cache will be removed.",
            ),
            KeyCode::Char('i') => (
                "cleanupDockerImages",
                "Remove unused Docker images?",
            ),
            KeyCode::Char('c') => (
                "cleanupDockerBuilder",
                "Remove the Docker build cache?",
            ),
            _ => return,
        };
        self.confirm = Some(Confirm {
            action: format!("maint:{op}"),
            project: String::new(),
            service: String::new(),
            stype: String::new(),
            label: label.into(),
        });
    }

    pub(super) fn monitor_key(&mut self, code: KeyCode, req: &Sender<Req>) {
        match code {
            KeyCode::Char('v') => {
                self.monitor_view = match self.monitor_view {
                    MonitorView::Services => MonitorView::Storage,
                    MonitorView::Storage => MonitorView::Services,
                };
                self.monitor_state.select(Some(0));
                if self.monitor_view == MonitorView::Storage && self.storage.is_empty() {
                    let _ = req.send(Req::Storage);
                }
            }
            _ => {
                let len = match self.monitor_view {
                    MonitorView::Services => self.monitor.len(),
                    MonitorView::Storage => self.storage.len(),
                };
                move_table(&mut self.monitor_state, code, len);
            }
        }
    }

    pub(super) fn domains_key(&mut self, code: KeyCode, req: &Sender<Req>) {
        let selected = self
            .domains_state
            .selected()
            .and_then(|i| self.visible_domains().get(i).map(|d| (*d).clone()));

        match code {
            KeyCode::Char('n') => {
                // Arrived from a service (via `o`) -> prefill project+service to
                // that service, so "New domain" doesn't start from a random project.
                let prefill = self.domain_scope.as_ref().map(|(p, s)| {
                    json!({
                        "destinationType": "service",
                        "serviceDestination": {
                            "projectName": p, "serviceName": s, "port": 80,
                            "protocol": "http", "path": "/"
                        }
                    })
                });
                let fields = domain_fields(prefill.as_ref(), &self.projects);
                self.form = Some(Form::new(FormKind::DomainCreate, " New domain ", fields));
                self.load_form_services(req);
            }
            KeyCode::Char('e') => {
                if let Some(d) = selected {
                    self.form = Some(
                        Form::new(
                            FormKind::DomainEdit {
                                id: field(&d, "/id"),
                            },
                            format!(" Edit domain: {} ", field(&d, "/host")),
                            domain_fields(Some(&d), &self.projects),
                        )
                        .with_original(d),
                    );
                    self.load_form_services(req);
                }
            }
            KeyCode::Char('x') => {
                if let Some(d) = selected {
                    self.confirm = Some(Confirm {
                        action: "domain-delete".into(),
                        project: field(&d, "/id"),
                        service: String::new(),
                        stype: String::new(),
                        label: format!("Delete domain '{}'?", field(&d, "/host")),
                    });
                }
            }
            KeyCode::Char('P') => {
                if let Some(d) = selected {
                    let _ = req.send(Req::DomainSetPrimary(field(&d, "/id")));
                }
            }
            _ => {
                let n = self.visible_domains().len();
                move_table(&mut self.domains_state, code, n)
            }
        }
    }

    pub(super) fn chooser_key(&mut self, code: KeyCode, req: &Sender<Req>) {
        let Some(ch) = self.chooser.as_mut() else {
            return;
        };
        match code {
            KeyCode::Esc => self.chooser = None,
            KeyCode::Down => {
                let len = ch.matches().len();
                let i = ch.state.selected().unwrap_or(0);
                if len > 0 {
                    ch.state.select(Some((i + 1).min(len - 1)));
                }
            }
            KeyCode::Up => {
                let i = ch.state.selected().unwrap_or(0);
                ch.state.select(Some(i.saturating_sub(1)));
            }
            KeyCode::Backspace => {
                ch.filter.pop();
                ch.clamp();
            }
            KeyCode::Char(c) => {
                ch.filter.push(c);
                ch.clamp();
            }
            KeyCode::Enter => self.apply_chooser(req),
            _ => {}
        }
    }

    pub(super) fn form_key(&mut self, code: KeyCode, req: &Sender<Req>) {
        let Some(form) = self.form.as_mut() else {
            return;
        };
        let typed = form.fields[form.focus].kind.is_typed();

        match code {
            // Wizard: Esc steps back one, and cancels only on the first step. A
            // single-page form has no previous step, so Esc cancels right away as
            // before.
            KeyCode::Esc => match form.prev_present_step() {
                Some(step) => form.goto_step(step),
                None => {
                    self.form = None;
                    self.status = "Cancelled".into();
                }
            },
            KeyCode::Tab | KeyCode::Down => form.move_focus(1),
            KeyCode::BackTab | KeyCode::Up => form.move_focus(-1),
            // Bool just toggles; Choice opens a searchable dropdown. Enter is
            // deliberately NOT here: if Enter opened the dropdown, a form whose
            // last field is a Choice ("New service", app type) could never be
            // saved — Enter would only open and close the dropdown, forever.
            KeyCode::Char(' ') | KeyCode::Left | KeyCode::Right if !typed => {
                match form.fields[form.focus].kind {
                    FieldKind::Bool => {
                        form.fields[form.focus].cycle();
                        form.clamp_focus();
                    }
                    // Multi-line content moves to $EDITOR. event_loop does it: only
                    // it may release the terminal.
                    FieldKind::Editor => self.edit_field = Some(form.focus),
                    _ => self.open_chooser(),
                }
            }
            KeyCode::Backspace if typed => {
                form.fields[form.focus].value.pop();
            }
            KeyCode::Char(c) if typed => form.fields[form.focus].value.push(c),
            // Wizard: Enter advances one step, and saves only on the last step. A
            // single-page form has no next step, so Enter saves right away as
            // before.
            KeyCode::Enter => match form.next_present_step() {
                Some(step) => form.goto_step(step),
                None => self.submit_form(req),
            },
            _ => {}
        }
    }

    pub(super) fn confirm_key(&mut self, code: KeyCode, req: &Sender<Req>) {
        // The caller already checked is_some(), but taking is more robust than an
        // unwrap that depends on a guard elsewhere: if called with no active
        // confirmation, there's nothing to do.
        let Some(c) = self.confirm.take() else {
            return;
        };
        if !matches!(code, KeyCode::Char('y') | KeyCode::Char('Y')) {
            self.status = "Cancelled".into();
            return;
        }

        // Deleting a project/domain has its own endpoint; the rest are ordinary
        // service actions (deploy/restart/stop/start/destroy ->
        // services/{type}/{action}Service).
        let _ = match c.action.as_str() {
            "destroy-project" => req.send(Req::ProjectDestroy(c.project.clone())),
            "domain-delete" => req.send(Req::DomainDelete(c.project.clone())),
            // The port index is stashed in `stype` (same pattern as a domain id
            // stashed in `project`).
            "port-delete" => req.send(Req::PortDelete {
                project: c.project.clone(),
                service: c.service.clone(),
                index: c.stype.parse().unwrap_or(0),
            }),
            "mount-delete" => req.send(Req::MountDelete {
                project: c.project.clone(),
                service: c.service.clone(),
                index: c.stype.parse().unwrap_or(0),
            }),
            // redirect-delete needs stype (services/{stype}); the index is stashed
            // in `stype` like port/mount, so the real stype is pulled from
            // viewer_ctx (the viewer is still open during the confirmation).
            "redirect-delete" => {
                let stype = self
                    .viewer_ctx
                    .as_ref()
                    .map(|(_, _, _, t)| t.clone())
                    .unwrap_or_default();
                req.send(Req::RedirectDelete {
                    project: c.project.clone(),
                    service: c.service.clone(),
                    stype,
                    index: c.stype.parse().unwrap_or(0),
                })
            }
            // Removing a server: a config change, not an API call.
            "server-remove" => {
                self.server_action = Some(ServerAction::Remove(c.project));
                self.status = "Removing server...".into();
                return;
            }
            "maint:systemPrune" => req.send(Req::MaintAction("systemPrune")),
            "maint:cleanupDockerImages" => req.send(Req::MaintAction("cleanupDockerImages")),
            "maint:cleanupDockerBuilder" => req.send(Req::MaintAction("cleanupDockerBuilder")),
            // Same endpoint as a plain deploy, with the layer cache turned off.
            // Carried as its own action name rather than a flag on Confirm, which
            // a dozen other call sites construct.
            "deploy-force" => req.send(Req::Action {
                project: c.project,
                service: c.service,
                stype: c.stype,
                action: "deploy".to_string(),
                force: true,
            }),
            action => req.send(Req::Action {
                project: c.project,
                service: c.service,
                stype: c.stype,
                action: action.to_string(),
                force: false,
            }),
        };
        self.status = "Sending...".into();
    }

    pub(super) fn picker_key(&mut self, code: KeyCode, _req: &Sender<Req>) {
        let Some(state) = self.picker.as_mut() else {
            return;
        };
        match code {
            KeyCode::Esc | KeyCode::Char('s') => self.picker = None,
            KeyCode::Char('n') => {
                self.picker = None;
                self.form = Some(Form::new(
                    FormKind::ServerAdd,
                    " Add server ",
                    vec![
                        Field::text("Name", ""),
                        Field::text("URL", "https://"),
                        Field::secret("Token"),
                    ],
                ));
            }
            KeyCode::Char('e') => {
                if let Some((name, url)) = self.picker_selected() {
                    self.picker = None;
                    self.form = Some(Form::new(
                        FormKind::ServerEdit { name: name.clone() },
                        format!(" Edit server: {name} "),
                        vec![
                            Field::text("URL", &url),
                            // The token is deliberately not re-filled: there's no
                            // need to put it back on screen. Empty = keep the stored
                            // token.
                            Field::secret("Token (empty = unchanged)"),
                        ],
                    ));
                }
            }
            KeyCode::Char('x') => {
                // Removing a server drops its token too, and the token can't be
                // read back from anywhere — one wrong keystroke and the credential
                // is gone. Every other destructive action here asks for
                // confirmation; this one used not to.
                if let Some((name, url)) = self.picker_selected() {
                    self.picker = None;
                    self.confirm = Some(Confirm {
                        action: "server-remove".into(),
                        project: name.clone(),
                        service: String::new(),
                        stype: String::new(),
                        label: format!(
                            "Delete server '{name}' ({url})? Its token goes with it and can't be recovered."
                        ),
                    });
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let i = (state.selected().unwrap_or(0) + 1).min(self.all_servers.len() - 1);
                state.select(Some(i));
            }
            KeyCode::Up | KeyCode::Char('k') => {
                let i = state.selected().unwrap_or(0).saturating_sub(1);
                state.select(Some(i));
            }
            KeyCode::Enter => {
                if let Some((name, _)) = state
                    .selected()
                    .and_then(|i| self.all_servers.get(i))
                    .cloned()
                {
                    if name != self.server_name {
                        self.switch_to = Some(name);
                    }
                }
                self.picker = None;
            }
            _ => {}
        }
    }

    pub(super) fn services_key(&mut self, code: KeyCode, req: &Sender<Req>) {
        match code {
            KeyCode::Enter => self.open_view(View::Logs, req),
            // These seven letters open a group MENU (not a single action) — the
            // heart of the UX consolidation: related actions no longer scatter into
            // loose keys. Their leaf keys (E/w/./p/P/f/F/H/b/U/B/A/L/M/y/R/S/T/X)
            // still work.
            KeyCode::Char('e') => {
                let m = self.env_menu();
                self.open_service_menu(m);
            }
            KeyCode::Char('o') => {
                let m = self.net_menu();
                self.open_service_menu(m);
            }
            KeyCode::Char('u') => {
                let m = self.build_menu();
                self.open_service_menu(m);
            }
            KeyCode::Char('m') => {
                let m = self.store_menu();
                self.open_service_menu(m);
            }
            KeyCode::Char('p') => self.open_view(View::Ports, req),
            KeyCode::Char('b') => self.open_view(View::Backups, req),
            KeyCode::Char('A') => self.toggle_auto_deploy(req),
            KeyCode::Char('U') => self.open_config_form(false, req),
            KeyCode::Char('B') => self.open_config_form(true, req),
            KeyCode::Char('L') => self.open_resource_form(req),
            KeyCode::Char('M') => self.open_mount_form(),
            KeyCode::Char('f') => self.open_view(View::Redirects, req),
            KeyCode::Char('F') => self.open_redirect_form(),
            KeyCode::Char('H') => self.open_basic_auth_form(req),
            KeyCode::Char('c') => self.open_clone_form(),
            KeyCode::Char('E') => self.start_env_edit(),
            KeyCode::Char('w') => self.start_env_replace(),
            // Turn the .env file (dotEnvPath) on/off. App services only — that's
            // the only place EasyPanel writes env as a file. Read state & flip it
            // in the worker.
            KeyCode::Char('.') => match self.selected_row() {
                Some((project, service, stype)) if stype == "app" => {
                    let _ = req.send(Req::EnvFileToggle {
                        project,
                        service,
                        stype,
                    });
                    self.status = "Toggle .env file...".into();
                }
                Some((_, _, stype)) => {
                    self.status = format!(".env file is only for app services (this is {stype})");
                }
                None => self.status = "Select a service first".into(),
            },
            KeyCode::Char('t') => {
                let m = self.shell_menu();
                self.open_service_menu(m);
            }
            // DB shell (auto login) keeps its direct key; also available in the
            // Shell menu (`t`). A feature the web dashboard doesn't have.
            KeyCode::Char('y') => self.start_db_shell(),
            KeyCode::Char('g') => {
                self.form = Some(Form::new(
                    FormKind::LogSearch,
                    " Search logs across ALL services ",
                    vec![Field::text("Keyword", "")],
                ));
            }
            KeyCode::Char('P') => {
                if let Some((project, service, _)) = self.selected_row() {
                    self.form = Some(Form::new(
                        FormKind::PortCreate { project, service },
                        " New port ",
                        port_fields(),
                    ));
                } else {
                    self.status = "Select a service first".into();
                }
            }
            KeyCode::Char('n') => self.new_service_form(req),
            KeyCode::Char('x') => {
                let m = self.danger_menu();
                self.open_service_menu(m);
            }
            // The Projects panel is gone, but projects still need to be
            // creatable/deletable from the TUI.
            KeyCode::Char('N') => {
                self.form = Some(Form::new(
                    FormKind::ProjectCreate,
                    " New project ",
                    vec![Field::text("Name", "")],
                ));
            }
            KeyCode::Char('X') => {
                if let Some(p) = self.selected_project() {
                    self.confirm = Some(Confirm {
                        action: "destroy-project".into(),
                        project: p.clone(),
                        service: String::new(),
                        stype: String::new(),
                        label: format!("Delete project '{p}' AND ALL its services?"),
                    });
                }
            }
            KeyCode::Char('d') => {
                let m = self.life_menu();
                self.open_service_menu(m);
            }
            KeyCode::Char('R') => self.ask_action("restart"),
            KeyCode::Char('S') => self.ask_action("stop"),
            KeyCode::Char('T') => self.ask_action("start"),
            _ => {
                let n = self.visible_rows().len();
                move_table(&mut self.services_table, code, n)
            }
        }
    }

    pub(super) fn viewer_key(&mut self, code: KeyCode) {
        match code {
            // Esc returns to the viewer's origin screen (Services for
            // logs/env/etc., Actions for an action detail).
            KeyCode::Esc => self.screen = self.viewer_from,
            // Scrolling up releases the follow: otherwise a newly arriving log line
            // would drag the view back to the bottom right as the user is reading
            // something above.
            KeyCode::Up | KeyCode::Char('k') | KeyCode::PageUp | KeyCode::Home => {
                self.viewer_follow = false;
                let step = if code == KeyCode::PageUp { 10 } else { 1 };
                self.viewer_scroll = match code {
                    KeyCode::Home => {
                        // Home means "back to the start" — both ways, or a line
                        // scrolled right still hides its own beginning.
                        self.viewer_hscroll = 0;
                        0
                    }
                    _ => self.viewer_scroll.saturating_sub(step),
                };
            }
            // End re-sticks to the last line and resumes following.
            KeyCode::End => self.viewer_follow = true,
            KeyCode::Down | KeyCode::Char('j') => {
                self.viewer_scroll = self.viewer_scroll.saturating_add(1)
            }
            KeyCode::PageDown => self.viewer_scroll = self.viewer_scroll.saturating_add(10),
            // In the Ports/Mounts viewer, a digit selects row [idx] to delete
            // (deletePort/deleteMount by index). 0-9 is enough: there's rarely >10.
            // Only if row [idx] actually exists, so a random digit does nothing.
            KeyCode::Char(c) if c.is_ascii_digit() => {
                let kind = match self.viewer_ctx.as_ref().map(|(v, ..)| *v) {
                    Some(View::Ports) => Some(("port-delete", "port")),
                    Some(View::Mounts) => Some(("mount-delete", "mount")),
                    Some(View::Redirects) => Some(("redirect-delete", "redirect")),
                    _ => None,
                };
                if let (Some((action, noun)), Some((_, project, service, _))) =
                    (kind, self.viewer_ctx.clone())
                {
                    let idx = (c as u8 - b'0') as usize;
                    let exists = self
                        .viewer_lines
                        .iter()
                        .any(|l| l.starts_with(&format!("[{idx}]")));
                    if exists {
                        let label = format!("Delete {noun} [{idx}] from {service}?");
                        self.confirm = Some(Confirm {
                            action: action.into(),
                            project,
                            service,
                            stype: idx.to_string(),
                            label,
                        });
                    }
                }
            }
            _ => {}
        }
    }
}
