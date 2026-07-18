//! Apa yang terjadi saat sebuah tombol ditekan.
//!
//! `impl App` yang kedua, sengaja di berkas lain: state dan selektor ada di
//! `app.rs`, sementara ini murni "tombol -> aksi". Menggabungkannya membuat satu
//! berkas yang tak bisa dibaca sekali duduk.

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
        // Bantuan menutup dengan tombol apa pun: user membukanya untuk membaca,
        // bukan untuk menghafal cara keluar.
        if self.help {
            self.help = false;
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
            // Datang ke Domains lewat `o` dari sebuah service: Esc kembali ke
            // Services (bukan sekadar menghapus filter scope-nya).
            KeyCode::Esc if self.domain_scope.is_some() => self.goto(Screen::Projects, req),
            KeyCode::Esc if !self.filter.is_empty() => self.clear_filter(),
            // Esc TIDAK menutup aplikasi. Esc berarti "batal": ia menutup form,
            // dropdown, konfirmasi, atau filter — dan bila tak ada yang perlu
            // dibatalkan, ia tak melakukan apa-apa. Menutup TUI karena satu
            // ketukan Esc refleks adalah kehilangan konteks tanpa peringatan.
            // Keluar: 'q' atau Ctrl-C.
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('1') => self.screen = Screen::Dashboard,
            KeyCode::Char('2') => self.goto(Screen::Hosts, req),
            KeyCode::Char('3') => self.goto(Screen::Maintenance, req),
            KeyCode::Char('4') => self.goto(Screen::Actions, req),
            KeyCode::Char('5') => self.goto(Screen::Monitor, req),
            KeyCode::Char('6') => self.goto(Screen::Domains, req),
            KeyCode::Char('7') => self.goto(Screen::Projects, req),
            KeyCode::Tab => self.goto(self.screen.next(), req),
            KeyCode::Char('?') => self.help = true,
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
                Screen::Hosts => move_table(&mut self.hosts_state, code, self.hosts.len()),
                Screen::Maintenance => self.maint_key(code),
                // Terminal ditangani langsung di event_loop (encode_key), bukan di
                // sini. Dashboard tak punya tombol khusus.
                Screen::Dashboard | Screen::Terminal => {}
            },
        }
    }

    /// Klik & scroll. Menu konteks menangkap mouse selama terbuka; modal lain
    /// (form/picker/konfirmasi/dropdown/bantuan) menelannya — klik tak boleh
    /// diam-diam mengganti tab di belakang dialog. Sesi terminal mengabaikannya.
    pub(super) fn on_mouse(&mut self, m: MouseEvent, req: &Sender<Req>) {
        // Menu konteks & dropdown menangkap mouse sendiri (hover/klik itemnya).
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
        // Modal lain menelan mouse: klik tak boleh menembus ke belakang dialog.
        if self.screen == Screen::Terminal
            || self.help
            || self.picker.is_some()
            || self.confirm.is_some()
        {
            return;
        }
        match m.kind {
            // Scroll menggulung viewport; seleksi tetap di bawah kursor supaya tak
            // bertabrakan dengan follow-cursor. Viewer menggulung teksnya.
            MouseEventKind::ScrollDown => self.on_scroll(3),
            MouseEventKind::ScrollUp => self.on_scroll(-3),
            MouseEventKind::Down(MouseButton::Left) => self.on_click(m.column, m.row, req),
            MouseEventKind::Down(MouseButton::Right) => self.on_right_click(m.column, m.row),
            // Sorotan mengikuti kursor: gerak mouse di atas baris memilihnya. Di
            // luar area tabel select_row_at tak berbuat apa-apa, jadi seleksi tetap.
            MouseEventKind::Moved | MouseEventKind::Drag(_) => {
                self.select_row_at(m.column, m.row);
            }
            _ => {}
        }
    }

    /// Scroll: viewer menggulung teks; tabel menggeser viewport DAN seleksi
    /// bersamaan, jadi sorotan tetap di baris layar yang sama (= tetap di bawah
    /// kursor). Itu yang membuat scroll tak lagi bertabrakan dengan hover — gerak
    /// mouse berikutnya memilih baris yang sama.
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
        // Baris data yang muat = tinggi area - border atas/bawah - header.
        let visible = (self.table_area.height as isize - 3).max(0);
        // Offset maksimum: berhenti saat halaman terakhir penuh. Kalau seluruh
        // daftar muat (max_off = 0), tak ada yang bisa digulung.
        let max_off = (len - visible).max(0);
        if let Some(state) = self.active_table() {
            let off = state.offset() as isize;
            let new_off = (off + delta).clamp(0, max_off);
            let applied = new_off - off;
            if applied == 0 {
                return; // tak ada ruang gulung -> seleksi TAK bergeser
            }
            // Geser seleksi sebanyak offset benar-benar bergerak: sorotan tetap di
            // baris layar yang sama (= tetap di bawah kursor).
            let sel = state.selected().unwrap_or(0) as isize;
            state.select(Some((sel + applied).clamp(0, len - 1) as usize));
            *state.offset_mut() = new_off as usize;
        }
    }

    /// Mouse pada form: klik sebuah field memfokuskannya, lalu — untuk Bool/Choice/
    /// Editor — langsung mengaktifkannya (toggle / buka dropdown / buka $EDITOR).
    /// Field teks cukup difokus; user lalu mengetik.
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

    /// Mouse pada dropdown terbuka: hover menyorot pilihan di bawah kursor, klik
    /// memilihnya (sama seperti Enter), klik di luar / scroll menavigasi.
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

    /// Terapkan pilihan dropdown ke field form (sama untuk Enter maupun klik).
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
        // Klik tab -> pindah ke tab itu (sama seperti menekan angkanya).
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

    /// Klik kanan sebuah baris memilihnya lalu membuka menu aksinya. Tanpa aksi
    /// untuk baris/layar itu, tak ada menu yang muncul.
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
                col,
                row,
                rect: ratatui::layout::Rect::default(),
            });
        }
    }

    /// Pilih baris di bawah (col,row) pada tabel layar aktif. Dua baris teratas
    /// (border + header) bukan data; offset menampung daftar yang tergulung. True
    /// bila sebuah baris benar-benar terpilih.
    fn select_row_at(&mut self, col: u16, row: u16) -> bool {
        let a = self.table_area;
        let first = a.y.saturating_add(2); // border atas + header
        let last = a.y.saturating_add(a.height).saturating_sub(1); // border bawah (eksklusif)
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

    /// Navigasi menu konteks lewat keyboard.
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
            KeyCode::Enter => self.activate_menu(req),
            _ => {}
        }
    }

    /// Mouse saat menu terbuka: scroll menavigasi, klik pada item mengaktifkannya,
    /// klik di luar menutup menu.
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
            // Sorotan mengikuti kursor di dalam menu.
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
                // Klik di luar menu -> tutup tanpa aksi.
                None => self.menu = None,
            },
            _ => {}
        }
    }

    /// Jalankan item menu terpilih: mengeksekusi tombol yang SAMA seperti keyboard,
    /// jadi tak ada jalur aksi kedua yang bisa menyimpang.
    fn activate_menu(&mut self, req: &Sender<Req>) {
        let key = self.menu.as_ref().and_then(|menu| {
            menu.state
                .selected()
                .and_then(|i| menu.items.get(i))
                .map(|(_, k)| *k)
        });
        self.menu = None;
        if let Some(k) = key {
            self.on_key(k, req);
        }
    }

    /// Actions: Enter (atau View di menu konteks) membuka detail action —
    /// metadata + log deploy/aksi — di viewer. Sisanya navigasi tabel.
    pub(super) fn actions_key(&mut self, code: KeyCode, req: &Sender<Req>) {
        match code {
            KeyCode::Enter => {
                if let Some(id) = self.selected_action_id() {
                    self.viewer_from = Screen::Actions;
                    // Bukan tampilan log-tail: pastikan poll log tak menyambungnya.
                    self.viewer_ctx = None;
                    self.status = "Memuat detail action...".into();
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
            // Esc membatalkan filter sepenuhnya; Enter menyimpannya dan kembali
            // ke navigasi biasa.
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

    /// Pembersihan Docker itu destruktif dan tak bisa dibatalkan, jadi tiap aksi
    /// lewat konfirmasi — sama seperti deploy/destroy.
    pub(super) fn maint_key(&mut self, code: KeyCode) {
        let (op, label) = match code {
            KeyCode::Char('p') => (
                "systemPrune",
                "Prune sistem Docker? Container, network, image, dan build cache yang tak terpakai akan dihapus.",
            ),
            KeyCode::Char('i') => (
                "cleanupDockerImages",
                "Hapus image Docker yang tak terpakai?",
            ),
            KeyCode::Char('c') => (
                "cleanupDockerBuilder",
                "Hapus build cache Docker?",
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
                // Datang dari sebuah service (via `o`) -> prefill project+service
                // ke service itu, supaya "Domain baru" tak mulai dari project acak.
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
                self.form = Some(Form::new(FormKind::DomainCreate, " Domain baru ", fields));
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
                        label: format!("Hapus domain '{}'?", field(&d, "/host")),
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
            // Wizard: Esc mundur satu langkah, dan membatalkan hanya di langkah
            // pertama. Form satu-halaman tak punya langkah sebelumnya, jadi Esc
            // langsung membatalkan seperti dulu.
            KeyCode::Esc => match form.prev_present_step() {
                Some(step) => form.goto_step(step),
                None => {
                    self.form = None;
                    self.status = "Dibatalkan".into();
                }
            },
            KeyCode::Tab | KeyCode::Down => form.move_focus(1),
            KeyCode::BackTab | KeyCode::Up => form.move_focus(-1),
            // Bool cukup di-toggle; Choice membuka dropdown yang bisa dicari.
            // Enter sengaja TIDAK di sini: kalau Enter membuka dropdown, form yang
            // field terakhirnya Choice ("Service baru" tipe app) tak pernah bisa
            // disimpan — Enter cuma buka-tutup dropdown, selamanya.
            KeyCode::Char(' ') | KeyCode::Left | KeyCode::Right if !typed => {
                match form.fields[form.focus].kind {
                    FieldKind::Bool => {
                        form.fields[form.focus].cycle();
                        form.clamp_focus();
                    }
                    // Isi multi-baris berpindah ke $EDITOR. event_loop yang
                    // melakukannya: hanya ia yang boleh melepas terminal.
                    FieldKind::Editor => self.edit_field = Some(form.focus),
                    _ => self.open_chooser(),
                }
            }
            KeyCode::Backspace if typed => {
                form.fields[form.focus].value.pop();
            }
            KeyCode::Char(c) if typed => form.fields[form.focus].value.push(c),
            // Wizard: Enter maju satu langkah, dan menyimpan hanya di langkah
            // terakhir. Form satu-halaman tak punya langkah berikutnya, jadi Enter
            // langsung menyimpan seperti dulu.
            KeyCode::Enter => match form.next_present_step() {
                Some(step) => form.goto_step(step),
                None => self.submit_form(req),
            },
            _ => {}
        }
    }

    pub(super) fn confirm_key(&mut self, code: KeyCode, req: &Sender<Req>) {
        // Pemanggil sudah cek is_some(), tapi total lebih tahan daripada unwrap
        // yang bergantung pada guard di tempat lain: kalau dipanggil tanpa
        // konfirmasi aktif, tak ada yang perlu dilakukan.
        let Some(c) = self.confirm.take() else {
            return;
        };
        if !matches!(code, KeyCode::Char('y') | KeyCode::Char('Y')) {
            self.status = "Dibatalkan".into();
            return;
        }

        // Hapus project/domain punya endpoint sendiri; sisanya aksi service biasa
        // (deploy/restart/stop/start/destroy -> services/{type}/{action}Service).
        let _ = match c.action.as_str() {
            "destroy-project" => req.send(Req::ProjectDestroy(c.project.clone())),
            "domain-delete" => req.send(Req::DomainDelete(c.project.clone())),
            // Indeks port dititipkan di `stype` (pola yang sama seperti id domain
            // dititipkan di `project`).
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
            // Hapus server: perubahan config, bukan panggilan API.
            "server-remove" => {
                self.server_action = Some(ServerAction::Remove(c.project));
                self.status = "Menghapus server...".into();
                return;
            }
            "maint:systemPrune" => req.send(Req::MaintAction("systemPrune")),
            "maint:cleanupDockerImages" => req.send(Req::MaintAction("cleanupDockerImages")),
            "maint:cleanupDockerBuilder" => req.send(Req::MaintAction("cleanupDockerBuilder")),
            action => req.send(Req::Action {
                project: c.project,
                service: c.service,
                stype: c.stype,
                action: action.to_string(),
            }),
        };
        self.status = "Mengirim...".into();
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
                    " Tambah server ",
                    vec![
                        Field::text("Nama", ""),
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
                            // Token sengaja tak diisi ulang: menampilkannya kembali ke
                            // layar tak perlu. Kosong = pakai token yang tersimpan.
                            Field::secret("Token (kosong = tak diubah)"),
                        ],
                    ));
                }
            }
            KeyCode::Char('x') => {
                // Menghapus server ikut membuang tokennya, dan token tak bisa
                // dibaca balik dari mana pun — sekali salah tekan, kredensialnya
                // hilang. Setiap aksi destruktif lain di sini minta konfirmasi;
                // yang ini dulu tidak.
                if let Some((name, url)) = self.picker_selected() {
                    self.picker = None;
                    self.confirm = Some(Confirm {
                        action: "server-remove".into(),
                        project: name.clone(),
                        service: String::new(),
                        stype: String::new(),
                        label: format!(
                            "Hapus server '{name}' ({url})? Tokennya ikut hilang dan tak bisa dikembalikan."
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
            KeyCode::Char('e') => self.open_view(View::Env, req),
            KeyCode::Char('p') => self.open_view(View::Ports, req),
            KeyCode::Char('m') => self.open_view(View::Mounts, req),
            KeyCode::Char('o') => self.open_service_domains(req),
            KeyCode::Char('b') => self.open_view(View::Backups, req),
            KeyCode::Char('u') => self.open_view(View::Source, req),
            KeyCode::Char('A') => self.toggle_auto_deploy(req),
            KeyCode::Char('U') => self.open_config_form(false, req),
            KeyCode::Char('B') => self.open_config_form(true, req),
            KeyCode::Char('L') => self.open_resource_form(req),
            KeyCode::Char('M') => self.open_mount_form(),
            KeyCode::Char('c') => self.open_clone_form(),
            KeyCode::Char('E') => self.start_env_edit(),
            KeyCode::Char('t') => {
                // Terminal ke container: event_loop yang mengerjakannya (ia yang
                // memegang terminal untuk serah-terima raw mode).
                if let Some((project, service, _)) = self.selected_row() {
                    self.terminal_req = Some((project, service));
                } else {
                    self.status = "Pilih sebuah service dulu".into();
                }
            }
            KeyCode::Char('g') => {
                self.form = Some(Form::new(
                    FormKind::LogSearch,
                    " Cari log di SEMUA service ",
                    vec![Field::text("Kata kunci", "")],
                ));
            }
            KeyCode::Char('P') => {
                if let Some((project, service, _)) = self.selected_row() {
                    self.form = Some(Form::new(
                        FormKind::PortCreate { project, service },
                        " Port baru ",
                        port_fields(),
                    ));
                } else {
                    self.status = "Pilih sebuah service dulu".into();
                }
            }
            KeyCode::Char('n') => self.new_service_form(req),
            KeyCode::Char('x') => self.ask_action("destroy"),
            // Panel Projects sudah tak ada, tapi project tetap harus bisa
            // dibuat/dihapus dari TUI.
            KeyCode::Char('N') => {
                self.form = Some(Form::new(
                    FormKind::ProjectCreate,
                    " Project baru ",
                    vec![Field::text("Nama", "")],
                ));
            }
            KeyCode::Char('X') => {
                if let Some(p) = self.selected_project() {
                    self.confirm = Some(Confirm {
                        action: "destroy-project".into(),
                        project: p.clone(),
                        service: String::new(),
                        stype: String::new(),
                        label: format!("Hapus project '{p}' BESERTA SEMUA service di dalamnya?"),
                    });
                }
            }
            KeyCode::Char('d') => self.ask_action("deploy"),
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
            // Esc kembali ke layar asal viewer (Services untuk log/env/dst.,
            // Actions untuk detail action).
            KeyCode::Esc => self.screen = self.viewer_from,
            // Menggulung ke atas melepas tempelan: kalau tidak, baris log yang
            // baru datang akan menyeret layar kembali ke bawah persis saat user
            // sedang membaca sesuatu di atas.
            KeyCode::Up | KeyCode::Char('k') | KeyCode::PageUp | KeyCode::Home => {
                self.viewer_follow = false;
                let step = if code == KeyCode::PageUp { 10 } else { 1 };
                self.viewer_scroll = match code {
                    KeyCode::Home => 0,
                    _ => self.viewer_scroll.saturating_sub(step),
                };
            }
            // End menempel kembali ke baris terakhir dan melanjutkan mengikuti.
            KeyCode::End => self.viewer_follow = true,
            KeyCode::Down | KeyCode::Char('j') => {
                self.viewer_scroll = self.viewer_scroll.saturating_add(1)
            }
            KeyCode::PageDown => self.viewer_scroll = self.viewer_scroll.saturating_add(10),
            // Di viewer Ports/Mounts, angka memilih baris [idx] itu untuk dihapus
            // (deletePort/deleteMount by index). Cukup 0-9: jarang ada >10. Hanya
            // jika baris [idx] memang ada, jadi angka acak tak melakukan apa pun.
            KeyCode::Char(c) if c.is_ascii_digit() => {
                let kind = match self.viewer_ctx.as_ref().map(|(v, ..)| *v) {
                    Some(View::Ports) => Some(("port-delete", "port")),
                    Some(View::Mounts) => Some(("mount-delete", "mount")),
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
                        let label = format!("Hapus {noun} [{idx}] dari {service}?");
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
