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

use super::actions::Menu;
use super::app::{
    parent_prefix, App, CfAction, CfProduct, CfScreen, Confirm, MonitorView, Screen, ServerAction,
    WatchAction, Workspace, CF_PRODUCTS, TAB_SCREENS,
};
use super::form::*;
use super::table::*;
use super::worker::{CfReq, Req, View};

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
        if self.cf_picker.is_some() {
            self.cf_picker_key(code, req);
            return;
        }

        // `W` opens the workspace switch menu from EITHER workspace — it is the one
        // key orthogonal to Screen, so it sits above the isolation gate below.
        if code == KeyCode::Char('W') {
            self.open_workspace_menu();
            return;
        }
        // `?` opens the help overlay from EITHER workspace — like `W`, it is orthogonal
        // to Screen, so the Cloudflare workspace gets the same help EasyPanel has instead
        // of the key being swallowed by the isolation gate below.
        if code == KeyCode::Char('?') {
            self.help = true;
            self.help_scroll = 0;
            return;
        }
        if self.workspace == Workspace::Cloudflare && self.screen == Screen::Viewer {
            if matches!(code, KeyCode::Left | KeyCode::Right) {
                const STEP: u16 = 8;
                self.viewer.hscroll = if code == KeyCode::Right {
                    self.viewer.hscroll.saturating_add(STEP)
                } else {
                    self.viewer.hscroll.saturating_sub(STEP)
                };
            } else {
                self.viewer_key(code, req);
            }
            return;
        }
        // ISOLATION: while in the Cloudflare workspace, none of the EasyPanel keys
        // below (tabs, digits 1-8, Tab, ←/→, the per-screen handlers) may act. The
        // Cloudflare account screen has its own, separate handler.
        if self.workspace == Workspace::Cloudflare {
            self.cloudflare_key(code, req);
            return;
        }

        match code {
            // Arrived at Domains via `o` from a service: Esc goes back to Services
            // (not just clearing its scope filter).
            // An armed bulk rewrite is the most immediate thing on screen, so Esc
            // cancels THAT first. It used to fall through to "clear the filter",
            // which left the rewrite armed behind a screen the user believed they
            // had backed out of.
            KeyCode::Esc if !self.domain_edits.is_empty() => {
                self.domain_edits.clear();
                self.screen = Screen::Domains;
                self.status = "Bulk edit cancelled — nothing was changed".into();
            }
            KeyCode::Esc if self.domain_scope.is_some() => self.goto(Screen::Projects, req),
            // A full-screen sub-view (Viewer, Credentials) OWNS its Esc — it means
            // "back to the list I came from", with my filter and marks intact so I
            // land where I was. Without this exemption Esc there fired these guards
            // instead: it silently cleared the Services filter (invisible on that
            // screen) or DESTROYED the marked set, and the "Esc back" the screen
            // advertises did nothing until a second press.
            KeyCode::Esc if !self.filter.is_empty() && !self.screen_owns_esc() => {
                self.clear_filter()
            }
            // Marks outlive the filter that helped make them, so Esc clears them
            // too — after the filter, since a filtered view is usually the thing
            // the user wants out of first.
            KeyCode::Esc if !self.marked.is_empty() && !self.screen_owns_esc() => {
                self.marked.clear();
                self.status = "Marks cleared".into();
            }
            // Esc does NOT quit the app. Esc means "cancel": it closes a form,
            // dropdown, confirmation, or filter — and when there's nothing to
            // cancel, it does nothing. Closing the TUI on a single reflexive Esc
            // is losing context without warning. Quit: 'q' or Ctrl-C.
            KeyCode::Char('q') => self.should_quit = true,
            // Global on every screen. They were guarded away from the viewer
            // while a digit meant "delete the row with this index"; the viewer
            // now has a selected row and `x`, so nothing there wants a digit and
            // the exception is dead weight.
            KeyCode::Char('1') => self.screen = Screen::Dashboard,
            KeyCode::Char('2') => self.goto(Screen::Hosts, req),
            KeyCode::Char('3') => self.goto(Screen::Maintenance, req),
            KeyCode::Char('4') => self.goto(Screen::Actions, req),
            KeyCode::Char('5') => self.goto(Screen::Monitor, req),
            KeyCode::Char('6') => self.goto(Screen::Domains, req),
            KeyCode::Char('7') => self.goto(Screen::Projects, req),
            KeyCode::Char('8') => self.goto(Screen::Uptime, req),
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
                self.viewer.hscroll = if code == KeyCode::Right {
                    self.viewer.hscroll.saturating_add(STEP)
                } else {
                    self.viewer.hscroll.saturating_sub(STEP)
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
            KeyCode::Char('s') => self.open_picker(),
            KeyCode::Char('r') => self.refresh(req),
            KeyCode::Char('/') if self.filterable() => {
                self.filter_input = true;
                self.filter.clear();
            }
            _ => match self.screen {
                Screen::Projects => self.services_key(code, req),
                Screen::Viewer => self.viewer_key(code, req),
                Screen::Actions => self.actions_key(code, req),
                Screen::Domains => self.domains_key(code, req),
                Screen::Uptime => self.uptime_key(code, req),
                Screen::Monitor => self.monitor_key(code, req),
                Screen::Hosts => match code {
                    // Hosts used to be the one screen with no row action at all —
                    // you could see a host was DOWN and had no way to ask why.
                    KeyCode::Enter => self.open_host_detail(),
                    _ => move_table(&mut self.hosts_state, code, self.hosts.len()),
                },
                Screen::Maintenance => self.maint_key(code),
                Screen::Credentials => self.credentials_key(code),
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
            || self.cf_picker.is_some()
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
        // A collection is a table: the wheel moves the SELECTION, as it does on
        // every other table here. It used to change viewer.scroll, which this
        // view does not read — so the wheel did nothing at all, silently.
        // The viewer branches are EasyPanel-only: `self.screen` is a stale EasyPanel
        // screen while in the Cloudflare workspace, so the wheel must fall through to
        // the CF table below instead of scrolling a hidden viewer.
        if self.workspace == Workspace::Easypanel
            && self.screen == Screen::Viewer
            && self.viewer_is_collection()
        {
            let len = self.viewer.lines.len();
            let key = if delta < 0 {
                KeyCode::Up
            } else {
                KeyCode::Down
            };
            for _ in 0..delta.unsigned_abs() {
                move_table(&mut self.viewer.row, key, len);
            }
            return;
        }
        if self.workspace == Workspace::Easypanel && self.screen == Screen::Viewer {
            let step = delta.unsigned_abs() as u16;
            if delta < 0 {
                self.viewer.follow = false;
                self.viewer.scroll = self.viewer.scroll.saturating_sub(step);
            } else {
                self.viewer.scroll = self.viewer.scroll.saturating_add(step);
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
        // Nothing matched, so there is nothing to apply. Closing anyway looked
        // EXACTLY like a successful pick: the dropdown vanished, the field kept
        // its old value, and nothing was said — so a typo left the user believing
        // they had changed it. The same silent close was already fixed in the
        // palette; this is its sibling. Stay open, and let the box say why.
        let Some(value) = ch.selected() else {
            return;
        };
        let (idx, label) = (ch.field, ch.label);
        self.chooser = None;
        if let Some(form) = self.form.as_mut() {
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
        if self.workspace == Workspace::Cloudflare && row == self.cf_product_row {
            if let Some(i) = self
                .cf_product_spans
                .iter()
                .position(|&(a, b)| col >= a && col < b)
            {
                if let Some(&(_, product)) = CF_PRODUCTS.get(i) {
                    self.cf_set_product(product, req);
                }
            }
            return;
        }
        // Click a tab -> switch to it (same as pressing its number). Only the
        // EasyPanel workspace has this tab bar; in Cloudflare tab_row is stale, so a
        // click there must fall through to row selection, not switch a hidden tab.
        if self.workspace == Workspace::Easypanel && row == self.tab_row {
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
        // In the Cloudflare workspace, right-click opens the row's action menu (the CF
        // mirror of EasyPanel's right-click): select the row under the cursor, then open
        // the menu for whatever screen is active. Records keeps `Space` for its bulk
        // menu (marked rows) and gets a single-record menu here.
        if self.workspace == Workspace::Cloudflare {
            if self.select_row_at(col, row) {
                match (self.cf.product, self.cf.screen) {
                    (CfProduct::Analytics, _) => {}
                    // R2 Objects: a FILE row gets the per-object menu (Download / Delete);
                    // a FOLDER row has no actions, so `open_cf_object_menu` no-ops on it.
                    // NOT the bucket menu — that would offer to delete the very bucket you
                    // are inside.
                    (CfProduct::R2, CfScreen::Objects) => self.open_cf_object_menu(),
                    (CfProduct::R2, _) => self.open_cf_bucket_menu(),
                    (CfProduct::Dns, CfScreen::Zones) => self.open_cf_zone_menu(),
                    (CfProduct::Dns, _) => self.open_cf_record_menu(),
                }
            }
            return;
        }
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
            // Show only what did not finish cleanly. The text filter cannot do
            // this — "error" there also matches a commit message that contains
            // the word — and the status colour (v0.68.0) already decides what
            // counts, so this is the natural companion to it.
            KeyCode::Char('f') => {
                self.actions_failures_only = !self.actions_failures_only;
                // The selection was an index into the OLD, longer list; reset it
                // to the top of the new one rather than leave it pointing at a
                // row that is no longer where it was.
                self.clamp_filtered();
                self.status = if self.actions_failures_only {
                    "Showing only actions that did not finish cleanly — [f] all".into()
                } else {
                    "Showing all actions".into()
                };
            }
            KeyCode::Enter => {
                if let Some(id) = self.selected_action_id() {
                    self.viewer.from = Screen::Actions;
                    // Not a log-tail view: make sure the log poll doesn't latch onto it.
                    self.viewer.ctx = None;
                    // Remembered so `r` can fetch it again — a running deploy's
                    // log is a snapshot, and this screen exists to watch it.
                    self.viewer.action_detail = Some(id.clone());
                    self.status = "Loading action detail...".into();
                    let _ = req.send(Req::ActionDetail(id));
                }
            }
            _ => self.move_selection(code),
        }
    }

    /// Move the selection on whichever screen is showing a table.
    ///
    /// ONE definition, because the filter needs it too now. Every screen used to
    /// carry its own "which table, how many rows" in a fallback arm, and a
    /// seventh copy for the filter is precisely how they drift apart.
    pub(super) fn move_selection(&mut self, code: KeyCode) {
        match self.screen {
            Screen::Projects => {
                let n = self.visible_rows().len();
                move_table(&mut self.services_table, code, n)
            }
            Screen::Actions => {
                let n = self.visible_actions().len();
                move_table(&mut self.actions_state, code, n)
            }
            Screen::Domains => {
                let n = self.visible_domains().len();
                move_table(&mut self.domains_state, code, n)
            }
            Screen::Monitor => {
                let n = self.monitor_rows_shown();
                move_table(&mut self.monitor_state, code, n)
            }
            Screen::Hosts => {
                let n = self.hosts.len();
                move_table(&mut self.hosts_state, code, n)
            }
            _ => {}
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
            // The arrows still move the list WHILE you type. Typing narrows the
            // table in front of you, so reaching for ↓ to pick a row is the
            // obvious next move — and it used to do nothing at all, with no hint
            // that Enter was required first.
            //
            // Deliberately NOT j/k: those are letters, and a filter is text.
            KeyCode::Up
            | KeyCode::Down
            | KeyCode::PageUp
            | KeyCode::PageDown
            | KeyCode::Home
            | KeyCode::End => self.move_selection(code),
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
            // Bounded by what is actually DRAWN. Counting raw metric entries
            // ignored the project header rows the table inserts, so the last rows
            // were unreachable — and it ignored the filter entirely.
            _ => self.move_selection(code),
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
            // Enrol the selected domain for uptime checks — through the form, so
            // the method, body and expected status are CHOSEN at the moment of
            // enrolling rather than defaulted silently and configured on another
            // screen later. Deliberately one domain at a time: a key that swept
            // 713 domains into the watchlist would destroy what makes it useful.
            KeyCode::Char('w') => {
                if let Some(d) = selected {
                    self.open_check_form(&crate::domains::domain_source(&d));
                }
            }
            // Rewrite one part of every domain ON SCREEN — so `/` narrows the set
            // first, using the filter the user already knows, rather than a
            // second way of choosing things that exists only here.
            KeyCode::Char('E') => {
                let n = self.visible_domains().len();
                self.form = Some(Form::new(
                    FormKind::DomainBulkEdit,
                    format!(" Bulk edit: {n} domain(s) on screen "),
                    domain_bulk_fields(),
                ));
            }
            _ => self.move_selection(code),
        }
    }

    pub(super) fn uptime_key(&mut self, code: KeyCode, req: &Sender<Req>) {
        let picked = self
            .uptime_state
            .selected()
            .and_then(|i| self.watched_row(i).map(|c| c.url.clone()));
        match code {
            KeyCode::Char('x') => {
                if let Some(url) = picked {
                    self.watch_action = Some(WatchAction::Remove(url.clone()));
                    self.watch.retain(|c| c.url != url);
                    self.probes.retain(|p| p.url != url);
                    self.status = format!("No longer watching {url}");
                }
            }
            KeyCode::Char('e') => {
                if let Some(url) = picked {
                    self.open_check_form(&url);
                }
            }
            KeyCode::Enter => self.run_checks(req),
            _ => move_table(&mut self.uptime_state, code, self.watch.len()),
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
        // A validation message describes the form as it was when Enter was pressed.
        // Keeping it until the NEXT successful Enter meant it outlived the field it
        // named: switch Source from dockerfile to image and "Dockerfile is still
        // empty" stayed on the border, pointing at a field no longer on screen. Any
        // key dismisses it; a still-failing Enter re-raises it below.
        form.error = None;

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
                Some(step) => match validate_step(form) {
                    Ok(()) => form.goto_step(step),
                    // Stay put. The field at fault is on THIS step, where the
                    // user can see it.
                    Err(e) => form.error = Some(e),
                },
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
            // Deploy exactly the services the confirmation counted.
            "project-env-deploy" => req.send(Req::Bulk {
                targets: self.deployable_in(&c.project),
                action: "deploy".into(),
                force: false,
            }),
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
            // viewer.ctx (the viewer is still open during the confirmation).
            "redirect-delete" => {
                let stype = self
                    .viewer
                    .ctx
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
            // Removing a Cloudflare account: local config only (see CfAction), the
            // event loop holds the file.
            "cf-account-delete" => {
                self.cf_action = Some(CfAction::Remove(c.project));
                self.status = "Removing Cloudflare account...".into();
                return;
            }
            // Delete one DNS record (its id was stashed in `project`). Needs the
            // active token + current zone, resolved here on the main thread.
            "cf-record-delete" => {
                if let (Some(token), Some(zone)) = (self.cf_token(), self.cf.current_zone.clone()) {
                    let _ = req.send(Req::Cf(CfReq::DeleteRecord {
                        token,
                        zone_id: zone.id,
                        id: c.project,
                    }));
                    self.status = "Deleting record...".into();
                }
                return;
            }
            // Delete every marked record — one call per id on the worker.
            "cf-bulk-delete" => {
                let ids: Vec<String> = self.cf.marked.iter().cloned().collect();
                if let (Some(token), Some(zone)) = (self.cf_token(), self.cf.current_zone.clone()) {
                    let _ = req.send(Req::Cf(CfReq::BulkDelete {
                        token,
                        zone_id: zone.id,
                        ids,
                    }));
                    self.status = "Deleting marked records...".into();
                }
                return;
            }
            // Delete one object (its key was stashed in `project`). The worker reports the
            // result and Done re-lists the level.
            "cf-object-delete" => {
                if let (Some(token), Some(account_id), Some(bucket)) = (
                    self.cf_token(),
                    self.cf_account_id(),
                    self.cf.current_bucket.clone(),
                ) {
                    let _ = req.send(Req::Cf(CfReq::R2Delete {
                        token,
                        account_id,
                        bucket,
                        keys: vec![c.project],
                    }));
                    self.status = "Deleting object...".into();
                }
                return;
            }
            // Delete every marked object — one call per key on the worker. Marks are
            // cleared once dispatched (their job is done).
            "cf-object-bulk-delete" => {
                let keys: Vec<String> = self.cf.marked.iter().cloned().collect();
                if let (Some(token), Some(account_id), Some(bucket)) = (
                    self.cf_token(),
                    self.cf_account_id(),
                    self.cf.current_bucket.clone(),
                ) {
                    let _ = req.send(Req::Cf(CfReq::R2Delete {
                        token,
                        account_id,
                        bucket,
                        keys,
                    }));
                    self.cf.marked.clear();
                    self.status = "Deleting marked objects...".into();
                }
                return;
            }
            // The file was chosen in the picker, not typed, so its three parts
            // travel in `pending_restore` rather than being squeezed into Confirm.
            // One request per database: the endpoint takes a single name, and a
            // failure on one must not silently take the others with it.
            "backup" => {
                let names = std::mem::take(&mut self.backups.pending);
                if names.is_empty() {
                    return;
                }
                let path = crate::backup::default_path(&c.project);
                for database in names {
                    let _ = req.send(Req::BackupNow {
                        project: c.project.clone(),
                        service: c.service.clone(),
                        database,
                        provider: c.stype.clone(),
                        path: path.clone(),
                    });
                }
                self.status = "Backing up...".into();
                return;
            }
            // A non-locking dump of ALL the chosen databases into ONE file, straight
            // to object storage — a single request, unlike native backup's one-per-db.
            "r2dump" => {
                let databases = std::mem::take(&mut self.backups.pending);
                if databases.is_empty() {
                    return;
                }
                let _ = req.send(Req::DumpR2 {
                    project: c.project,
                    service: c.service,
                    databases,
                });
                self.status = "Dumping (non-locking) to object storage…".into();
                return;
            }
            "restore" => match self.backups.pending_restore.take() {
                Some((database, provider, path)) => req.send(Req::RestoreBackup {
                    project: c.project,
                    service: c.service,
                    database,
                    provider,
                    path,
                }),
                None => return,
            },
            "r2restore" => match self.backups.pending_r2_restore.take() {
                Some(path) => {
                    self.status = "Restoring from object storage…".into();
                    req.send(Req::RestoreR2 {
                        project: c.project,
                        service: c.service,
                        path,
                    })
                }
                None => return,
            },
            "maint:systemPrune" => req.send(Req::MaintAction("systemPrune")),
            "maint:cleanupDockerImages" => req.send(Req::MaintAction("cleanupDockerImages")),
            "maint:cleanupDockerBuilder" => req.send(Req::MaintAction("cleanupDockerBuilder")),
            // A bulk run reads its targets from the marks at the moment it is
            // CONFIRMED, not when it was offered: the confirmation shows a count
            // the user just agreed to, and re-deriving it here keeps the two from
            // ever disagreeing. `stype` carries the force flag for a bulk rebuild.
            bulk if bulk.starts_with("bulk-") => {
                let targets = self.bulk_targets();
                if targets.is_empty() {
                    self.status = "Nothing marked any more — cancelled".into();
                    return;
                }
                req.send(Req::Bulk {
                    targets,
                    action: bulk.trim_start_matches("bulk-").to_string(),
                    force: c.stype == "force",
                })
            }
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
                            // Editable: a server's name is the label the whole UI
                            // identifies it by — the title bar, the confirmations
                            // and its colour — so a typo in it was permanent, and
                            // the only way out was to delete the server and lose
                            // its token with it.
                            Field::text("Name", &name),
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

    /// The Credentials screen: a database service's connection identity, read-only.
    /// `v` reveals the masked secrets, `c`/`y`/Enter copies the selected value,
    /// Esc returns to Services.
    pub(super) fn credentials_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Esc => self.screen = Screen::Projects,
            KeyCode::Char('v') => {
                self.creds.revealed = !self.creds.revealed;
                // Keep the status in step with the reveal — the border toggles
                // v reveal / v hide, and a stale status must not contradict it.
                self.status = if self.creds.revealed {
                    "Revealed — c copies · v hides again"
                } else {
                    "Hidden — v reveals · c still copies the real value"
                }
                .into();
            }
            KeyCode::Enter | KeyCode::Char('c') | KeyCode::Char('y') => {
                match self
                    .creds
                    .row
                    .selected()
                    .and_then(|i| self.creds.items.get(i))
                {
                    Some(item) => {
                        // The REAL value, even while masked on screen — a copy has to be.
                        self.clipboard = Some(item.value.clone());
                        self.status = format!("{} copied to clipboard", item.label);
                    }
                    None => self.status = "Nothing selected".into(),
                }
            }
            _ => move_table(&mut self.creds.row, code, self.creds.items.len()),
        }
    }

    /// The Cloudflare workspace. Its keys never touch EasyPanel state, and no
    /// EasyPanel key reaches here — the isolation the workspace promises. The home
    /// is the Zones list; Records is a drill-in from a zone; accounts are switched
    /// through the `a` picker overlay. `W` (workspace switch) still works, handled
    /// above this dispatch.
    pub(super) fn cloudflare_key(&mut self, code: KeyCode, req: &Sender<Req>) {
        // The CF-local filter input owns the keyboard while active (its own state,
        // never the EasyPanel filter).
        if self.cf.filter_input {
            self.cf_filter_key(code);
            return;
        }
        // `:` opens the CF command palette (jump to a product / account / zone /
        // bucket) — the mirror of the EasyPanel `:` palette, which the isolation gate
        // otherwise keeps out of this workspace. The palette overlay, once open, is
        // handled above this dispatch, so it works the same in both workspaces.
        if code == KeyCode::Char(':') {
            self.open_cf_palette();
            return;
        }
        // `a` opens the account picker from ANY CF screen — the mirror of EasyPanel's
        // `s` server switcher, which sits above the per-screen dispatch and so works on
        // every screen. It used to be bound only on the Zones/Buckets home handlers, so
        // on the Records/Objects drill-ins `a` was a silent dead key. Switching account
        // returns to the new account's home (`cf_goto_home`), so it is safe from a
        // drill-in.
        if code == KeyCode::Char('a') {
            self.open_cf_picker();
            return;
        }
        // Product tabs (Analytics · Domains · R2; D1/KV/Workers/Connectors later): 1..=N jump,
        // Tab/→ cycle forward, ← cycles back — the CF mirror of the EasyPanel tab
        // keys. Overlays/forms/the filter are handled above this dispatch, so they
        // can't be swallowed here. Switching products loads that product's home.
        match code {
            KeyCode::Char(d @ '1'..='9') => {
                if let Some(&(_, p)) = CF_PRODUCTS.get(d as usize - '1' as usize) {
                    self.cf_set_product(p, req);
                }
                return;
            }
            KeyCode::Tab | KeyCode::Right => {
                self.cf_set_product(self.cf.product.next(), req);
                return;
            }
            KeyCode::Left => {
                self.cf_set_product(self.cf.product.prev(), req);
                return;
            }
            _ => {}
        }
        match self.cf.product {
            CfProduct::Analytics => self.cf_analytics_key(code, req),
            CfProduct::R2 => match self.cf.screen {
                CfScreen::Objects => self.cf_objects_key(code, req),
                _ => self.cf_buckets_key(code, req),
            },
            CfProduct::Dns => match self.cf.screen {
                CfScreen::Zones => self.cf_zones_key(code, req),
                CfScreen::Records | CfScreen::Objects => self.cf_records_key(code, req),
            },
        }
    }

    /// Account-level Analytics home. It is read-only: `r` refreshes, `Esc` leaves the
    /// workspace, and `a`/product switching are handled by the outer CF dispatcher.
    fn cf_analytics_key(&mut self, code: KeyCode, req: &Sender<Req>) {
        match code {
            KeyCode::Esc => self.set_workspace(Workspace::Easypanel),
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('r') => self.cf_goto_analytics(req),
            _ => {}
        }
    }

    /// The R2 buckets home. Mirrors the Zones home: `a` switches account (picker),
    /// Enter drills into the bucket's objects, `n` adds a bucket, `x` deletes
    /// (typed-name confirm), Space opens the row menu, Esc leaves the workspace
    /// (after clearing an active filter first).
    fn cf_buckets_key(&mut self, code: KeyCode, req: &Sender<Req>) {
        match code {
            KeyCode::Esc if !self.cf.filter.is_empty() => {
                self.cf.filter.clear();
                self.cf_clamp_filtered();
            }
            KeyCode::Esc => self.set_workspace(Workspace::Easypanel),
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('/') => {
                self.cf.filter_input = true;
                self.cf.filter.clear();
            }
            KeyCode::Char('r') => self.cf_reload(req),
            KeyCode::Char('n') => self.open_cf_bucket_form(),
            KeyCode::Char('x') => self.open_cf_bucket_delete_form(),
            KeyCode::Char(' ') => self.open_cf_bucket_menu(),
            KeyCode::Enter => self.cf_open_objects(req),
            _ => {
                let len = self.cf_buckets_shown().len();
                move_table(&mut self.cf.r2_row, code, len);
            }
        }
    }

    /// The R2 objects FOLDER browser (the mirror of the DNS Records screen): Enter
    /// descends into a folder or downloads a file, `u` uploads into this level, `x`
    /// deletes the selected file, `v`/`V` mark files, Space is the object/bulk menu, `/`
    /// filters this level, `r` refreshes it. Esc clears an active filter, then marks, then
    /// goes UP one folder while inside the tree, and only backs out to the buckets home at
    /// the root.
    fn cf_objects_key(&mut self, code: KeyCode, req: &Sender<Req>) {
        match code {
            KeyCode::Esc if !self.cf.filter.is_empty() => {
                self.cf.filter.clear();
                self.cf_clamp_filtered();
            }
            KeyCode::Esc if !self.cf.marked.is_empty() => {
                self.cf.marked.clear();
                self.status = "Marks cleared".into();
            }
            KeyCode::Esc if !self.cf.current_prefix.is_empty() => {
                // Inside a folder: go UP one level (reload the parent), not out of objects.
                let parent = parent_prefix(&self.cf.current_prefix);
                self.cf_request_level(parent, req);
            }
            KeyCode::Esc => {
                // At the bucket root: back to the buckets home. R2's home is any non-Objects
                // screen; the buckets stay loaded, so no reload — just drop the drill-in.
                self.cf.screen = CfScreen::Zones;
                self.cf.current_bucket = None;
                self.cf.current_prefix.clear();
                self.cf.marked.clear();
                self.cf.error = None;
            }
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('/') => {
                self.cf.filter_input = true;
                self.cf.filter.clear();
            }
            KeyCode::Char('r') => self.cf_reload(req),
            KeyCode::Char('u') => self.open_cf_upload_form(),
            KeyCode::Char('x') => self.ask_cf_object_delete(),
            KeyCode::Char('v') => self.cf_toggle_object_mark(),
            KeyCode::Char('V') => self.cf_mark_all_objects(),
            // Space: the bulk menu when something is marked, else the single-object menu.
            KeyCode::Char(' ') if !self.cf.marked.is_empty() => self.open_cf_object_bulk_menu(),
            KeyCode::Char(' ') => self.open_cf_object_menu(),
            KeyCode::Enter => self.cf_object_enter(req),
            _ => {
                let len = self.cf_level_len();
                move_table(&mut self.cf.r2_objects_row, code, len);
            }
        }
    }

    /// The Zones home. `a` switches account (picker), Enter drills into a zone's
    /// records, Esc leaves the workspace (after clearing an active filter first).
    fn cf_zones_key(&mut self, code: KeyCode, req: &Sender<Req>) {
        match code {
            KeyCode::Esc if !self.cf.filter.is_empty() => {
                self.cf.filter.clear();
                self.cf_clamp_filtered();
            }
            KeyCode::Esc => self.set_workspace(Workspace::Easypanel),
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('/') => {
                self.cf.filter_input = true;
                self.cf.filter.clear();
            }
            KeyCode::Char('r') => self.cf_reload(req),
            KeyCode::Char('n') => self.open_cf_zone_form(),
            KeyCode::Char('x') => self.open_cf_zone_delete_form(),
            // Space opens the row action menu for the selected zone — the keyboard
            // version of a right click, mirroring EasyPanel's row menu.
            KeyCode::Char(' ') => self.open_cf_zone_menu(),
            KeyCode::Enter => self.cf_open_records(req),
            _ => {
                let len = self.cf_zones_shown().len();
                move_table(&mut self.cf.zones_row, code, len);
            }
        }
    }

    /// The account picker overlay — the mirror of the server `s` picker, plus the
    /// account's own add/delete. Enter activates the account and reloads its zones.
    pub(super) fn cf_picker_key(&mut self, code: KeyCode, req: &Sender<Req>) {
        let Some(state) = self.cf_picker.as_mut() else {
            return;
        };
        match code {
            KeyCode::Esc | KeyCode::Char('a') => self.cf_picker = None,
            // `n` adds an account (the same local form as before); reachable even
            // with no accounts, so the empty state is never a dead end.
            KeyCode::Char('n') => {
                self.cf_picker = None;
                self.open_cf_account_form();
            }
            KeyCode::Char('e') => {
                self.open_cf_account_edit_form();
                self.cf_picker = None;
            }
            KeyCode::Char('x') => {
                if let Some(acc) = self.cf_picker_selected() {
                    self.cf_picker = None;
                    self.confirm = Some(Confirm {
                        action: "cf-account-delete".into(),
                        project: acc.name.clone(),
                        service: String::new(),
                        stype: String::new(),
                        label: format!(
                            "Delete Cloudflare account '{}'? (local config only)",
                            acc.name
                        ),
                    });
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if !self.cf.accounts.is_empty() {
                    let i = (state.selected().unwrap_or(0) + 1).min(self.cf.accounts.len() - 1);
                    state.select(Some(i));
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                let i = state.selected().unwrap_or(0).saturating_sub(1);
                state.select(Some(i));
            }
            KeyCode::Enter => {
                if let Some(acc) = self.cf_picker_selected() {
                    self.cf_picker = None;
                    self.cf_activate_account(acc, req);
                }
            }
            _ => {}
        }
    }

    /// The Records screen. `v`/`V` mark, Space opens the bulk menu, Esc backs out
    /// to Zones (after clearing an active filter or marks first).
    fn cf_records_key(&mut self, code: KeyCode, req: &Sender<Req>) {
        match code {
            KeyCode::Esc if !self.cf.filter.is_empty() => {
                self.cf.filter.clear();
                self.cf_clamp_filtered();
            }
            KeyCode::Esc if !self.cf.marked.is_empty() => {
                self.cf.marked.clear();
                self.status = "Marks cleared".into();
            }
            KeyCode::Esc => self.cf.screen = CfScreen::Zones,
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('/') => {
                self.cf.filter_input = true;
                self.cf.filter.clear();
            }
            KeyCode::Char('r') => self.cf_reload(req),
            KeyCode::Char('n') => self.open_cf_record_form(),
            KeyCode::Char('e') => self.open_cf_record_edit(),
            KeyCode::Char('x') => self.ask_cf_record_delete(),
            KeyCode::Char('v') => self.cf_toggle_mark(),
            KeyCode::Char('V') => self.cf_mark_all_shown(),
            KeyCode::Char(' ') if !self.cf.marked.is_empty() => self.open_cf_bulk_menu(),
            KeyCode::Char(' ') => self.open_cf_record_menu(),
            _ => {
                let len = self.cf_records_shown().len();
                move_table(&mut self.cf.records_row, code, len);
            }
        }
    }

    /// Type into the CF-local filter. Esc cancels it, Enter keeps it and returns to
    /// the list.
    fn cf_filter_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Esc => {
                self.cf.filter.clear();
                self.cf.filter_input = false;
                self.cf_clamp_filtered();
            }
            KeyCode::Enter => self.cf.filter_input = false,
            KeyCode::Backspace => {
                self.cf.filter.pop();
                self.cf_clamp_filtered();
            }
            KeyCode::Char(c) => {
                self.cf.filter.push(c);
                self.cf_clamp_filtered();
            }
            // The arrows move the CF list WHILE you type — the same as EasyPanel's
            // filter (keys.rs `filter_key`). Typing narrows the zones/records/buckets/
            // objects list in front of you, so reaching for ↓ to grab the row you just
            // filtered to is the obvious next move; it used to be inert here, silently
            // forcing an Enter first. `active_table`/`visible_table_len` already resolve
            // to the correct CF row + filtered length. Not j/k: a filter is text.
            KeyCode::Up
            | KeyCode::Down
            | KeyCode::PageUp
            | KeyCode::PageDown
            | KeyCode::Home
            | KeyCode::End => {
                let len = self.visible_table_len();
                if let Some(state) = self.active_table() {
                    move_table(state, code, len);
                }
            }
            _ => {}
        }
    }

    pub(super) fn services_key(&mut self, code: KeyCode, req: &Sender<Req>) {
        match code {
            KeyCode::Enter => self.open_view(View::Logs, req),
            // Marking, the three ways of choosing a set: `v` takes the row (or a
            // whole project from its header), `V` takes everything the filter has
            // left on screen. Space then acts on them — see service_menu().
            KeyCode::Char('v') => self.toggle_mark(),
            KeyCode::Char('V') => self.mark_all_visible(),
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
            KeyCode::Char('p') => {
                if let Some((_, _, t)) = self.selected_row() {
                    if self.allows_mounts_and_ports(&t) {
                        self.open_view(View::Ports, req)
                    }
                }
            }
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
            _ => self.move_selection(code),
        }
    }

    pub(super) fn viewer_key(&mut self, code: KeyCode, req: &Sender<Req>) {
        match code {
            // Esc returns to the viewer's origin screen (Services for
            // logs/env/etc., Actions for an action detail). Leaving the restore
            // picker also drops what it was aimed at, so a later Enter elsewhere
            // cannot fire a restore the user has walked away from.
            KeyCode::Esc => {
                self.backups.close_pickers();
                self.screen = self.viewer.from;
            }
            // The bulk-rewrite preview IS the confirmation — the list of
            // before → after lines is on screen while this key is pressed.
            KeyCode::Enter if !self.domain_edits.is_empty() => self.apply_domain_edits(req),
            // In the restore picker, Enter acts on the selected backup.
            KeyCode::Enter if self.backups.restore_into.is_some() => self.ask_restore(),
            // …in the object-storage dump picker, on the selected dump.
            KeyCode::Enter if self.backups.r2_restore_into.is_some() => self.ask_r2_restore(),
            // …and in the database picker, on the selected database.
            KeyCode::Enter if self.backups.backup_from.is_some() => self.ask_backup(),
            // `v` ticks, exactly as it marks a service in the table.
            KeyCode::Char('v') if self.backups.backup_from.is_some() => self.toggle_backup_mark(),
            // Scrolling up releases the follow: otherwise a newly arriving log line
            // would drag the view back to the bottom right as the user is reading
            // something above.
            // In a collection ↑↓ move the SELECTED row; in prose they scroll.
            KeyCode::Up
            | KeyCode::Down
            | KeyCode::Char('k')
            | KeyCode::Char('j')
            | KeyCode::PageUp
            | KeyCode::PageDown
            | KeyCode::Home
            | KeyCode::End
                if self.viewer_is_collection() =>
            {
                // Every movement key moves the SELECTION here, through the same
                // helper the other tables use — so PageDown and End behave as
                // they do everywhere else rather than scrolling a list whose
                // highlight then sits off screen.
                let len = self.viewer.lines.len();
                move_table(&mut self.viewer.row, code, len);
            }
            KeyCode::Up | KeyCode::Char('k') | KeyCode::PageUp | KeyCode::Home => {
                self.viewer.follow = false;
                let step = if code == KeyCode::PageUp { 10 } else { 1 };
                self.viewer.scroll = match code {
                    KeyCode::Home => {
                        // Home means "back to the start" — both ways, or a line
                        // scrolled right still hides its own beginning.
                        self.viewer.hscroll = 0;
                        0
                    }
                    _ => self.viewer.scroll.saturating_sub(step),
                };
            }
            // End re-sticks to the last line and resumes following.
            KeyCode::End => self.viewer.follow = true,
            KeyCode::Down | KeyCode::Char('j') => {
                self.viewer.scroll = self.viewer.scroll.saturating_add(1)
            }
            KeyCode::PageDown => self.viewer.scroll = self.viewer.scroll.saturating_add(10),
            // A collection lives in ONE screen: this is where you see it, add to
            // it and delete from it. "View X" and "Add X" used to be separate
            // menu entries — two doors into the same room, which made looking at
            // a thing and changing it feel like unrelated features.
            //
            // Routed through services_key so these are the SAME handlers the menu
            // uses; there is no second path that could drift from it.
            // `x` deletes the highlighted row — the same verb Domains and the
            // server picker use, and with no ten-row ceiling. It replaced "press
            // the digit printed on the line", which capped the list at [9] and
            // lost 1-7 to the tab keys.
            KeyCode::Char('x') => {
                let Some((view, project, service, _)) = self.viewer.ctx.clone() else {
                    return;
                };
                let Some((action, noun)) = (match view {
                    View::Ports => Some(("port-delete", "port")),
                    View::Mounts => Some(("mount-delete", "mount")),
                    View::Redirects => Some(("redirect-delete", "redirect")),
                    _ => None,
                }) else {
                    self.status = "Nothing here can be deleted".into();
                    return;
                };
                // The index comes from the marker PRINTED on the row, not from
                // its position: the list can hold lines that are not rows, and
                // the server deletes by the index it gave us.
                let picked = self
                    .viewer
                    .row
                    .selected()
                    .and_then(|i| self.viewer.lines.get(i))
                    .and_then(|l| row_index(l));
                match picked {
                    Some(idx) => {
                        let label = format!("Delete {noun} [{idx}] from {service}?");
                        self.confirm = Some(Confirm {
                            action: action.into(),
                            project,
                            service,
                            stype: idx.to_string(),
                            label,
                        });
                    }
                    _ => self.status = format!("Select a {noun} first"),
                }
            }
            KeyCode::Char('n') | KeyCode::Char('e') | KeyCode::Char('b') => {
                let view = self.viewer.ctx.as_ref().map(|(v, ..)| *v);
                // `e` on a mount EDITS the highlighted one, the same verb Domains
                // uses. It lives in this handler rather than its own arm so `e`
                // keeps one meaning per screen — a second arm shadowed Env's and
                // Source's `e` entirely. Only mounts have an update endpoint;
                // ports and redirects are read-modify-write on a whole array.
                if let (Some(View::Mounts), KeyCode::Char('e')) = (view, code) {
                    let Some((_, project, service, _)) = self.viewer.ctx.clone() else {
                        return;
                    };
                    // The index comes from the marker PRINTED on the row — the
                    // one rule, shared with the pickers and the delete above.
                    match self.picker_row() {
                        Some(index) => {
                            let _ = req.send(Req::MountForm {
                                project,
                                service,
                                index,
                            });
                            self.status = "Loading mount...".into();
                        }
                        None => self.status = "Select a mount first".into(),
                    }
                    return;
                }
                let leaf = match (view, code) {
                    (Some(View::Env), KeyCode::Char('e')) => Some('E'),
                    (Some(View::Ports), KeyCode::Char('n')) => Some('P'),
                    (Some(View::Mounts), KeyCode::Char('n')) => Some('M'),
                    (Some(View::Redirects), KeyCode::Char('n')) => Some('F'),
                    (Some(View::Source), KeyCode::Char('e')) => Some('U'),
                    (Some(View::Source), KeyCode::Char('b')) => Some('B'),
                    _ => None,
                };
                match leaf {
                    Some(k) => self.services_key(KeyCode::Char(k), req),
                    // Not every viewer takes every key. Doing nothing at all left
                    // the user unable to tell "wrong key here" from "the app is
                    // stuck" — so say what THIS screen accepts.
                    None => {
                        let keys = super::render::viewer_actions(self);
                        self.status = if keys.trim().is_empty() {
                            "Nothing to change on this screen".into()
                        } else {
                            format!("Not here —{keys}")
                        };
                    }
                }
            }
            _ => {}
        }
    }
}
