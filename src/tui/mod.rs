//! TUI EasyPanel.
//!
//! Dipecah mengikuti aliran datanya, bukan tipenya: `worker` bicara ke jaringan
//! di thread lain dan hanya mengenal Req/Resp; `app` memegang state dan tombol;
//! `render` menggambar dan tak pernah memutuskan apa pun; `form` dan `table`
//! adalah bahasa bersama di antaranya. `mod.rs` hanya menyatukan: event loop,
//! penyerahan ke $EDITOR, dan perubahan daftar server — satu-satunya tempat yang
//! memegang ServerConfig.

mod app;
mod form;
mod keys;
mod render;
mod table;
mod terminal;
mod worker;

#[cfg(test)]
mod tests;

use std::sync::mpsc::{Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
};
use serde_json::json;

use crate::client::EasypanelClient;
use crate::config::ServerConfig;

const REFRESH: Duration = Duration::from_secs(2);
/// Batas baris yang ditahan viewer saat tail log berjalan.
const LOG_BUFFER: usize = 5_000;

use app::{App, HostRow, HostState, Screen, ServerAction};
use render::ui;
use worker::{spawn_workers, Req, Resp, View};

/// Buka TUI untuk server default (atau --server yang sudah di-resolve).
pub fn run(cfg: &ServerConfig, client: EasypanelClient, server_name: String) -> Result<()> {
    if cfg.all().is_empty() {
        println!("Belum ada server. Jalankan: easypanel server add");
        return Ok(());
    }

    let names: Vec<(String, String)> = cfg.all().into_iter().map(|s| (s.name, s.url)).collect();
    let mut app = App::new(server_name, names);

    let mut terminal = ratatui::init();
    enable_mouse();
    let result = event_loop(&mut terminal, &mut app, cfg, client);
    disable_mouse();
    ratatui::restore();
    result
}

/// Tangkap event mouse (klik tab/baris, scroll). Efek samping: seleksi teks bawaan
/// terminal jadi tak aktif — pakai Shift+drag di kebanyakan terminal untuk menyalin.
fn enable_mouse() {
    let _ = ratatui::crossterm::execute!(std::io::stdout(), EnableMouseCapture);
}

fn disable_mouse() {
    let _ = ratatui::crossterm::execute!(std::io::stdout(), DisableMouseCapture);
}

fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    cfg: &ServerConfig,
    client: EasypanelClient,
) -> Result<()> {
    let mut w = spawn_workers(client);
    send_initial(&w.user);
    let mut last_stats = Instant::now();
    // Status memudar: dilacak di sini, bukan di tiap `self.status = …` yang
    // tersebar. Timer reset saat pesannya berubah; kalau diam ≥ IDLE detik dan
    // bukan "Siap", kembalikan ke "Siap" supaya notifikasi sementara (mis.
    // "Deploy dimulai") tak menetap selamanya.
    const STATUS_IDLE: Duration = Duration::from_secs(6);
    let mut last_status = app.status.clone();
    let mut status_since = Instant::now();

    loop {
        while let Ok(resp) = w.resp.try_recv() {
            app.handle(resp, &w.user);
        }

        if app.status != last_status {
            last_status = app.status.clone();
            status_since = Instant::now();
        } else if app.status != "Siap" && status_since.elapsed() >= STATUS_IDLE {
            app.status = "Siap".into();
            last_status = app.status.clone();
        }

        app.tick_anim();
        terminal.draw(|f| ui(f, app))?;

        // Metrik jalan di lajur poll. Guard in-flight menjaga agar ronde tak
        // menumpuk saat server lebih lambat dari interval refresh.
        if last_stats.elapsed() >= REFRESH && !app.refresh_inflight {
            let _ = w.poll.send(Req::Stats);
            // Metrik per service ikut live, tapi hanya di layar yang menampilkannya.
            if matches!(app.screen, Screen::Monitor | Screen::Projects) {
                let _ = w.poll.send(Req::MonitorData);
            }
            // Status "turun" ikut live di tabel Services.
            if app.screen == Screen::Projects {
                let _ = w.poll.send(Req::TaskStats);
            }
            // Log ikut hidup selama viewer-nya terbuka. Di lajur poll, bukan
            // lajur user: tail tiap dua detik tak boleh mengantre di belakang
            // (atau di depan) aksi yang ditekan user.
            if let (Screen::Viewer, Some((View::Logs, project, service, _))) =
                (app.screen, &app.viewer_ctx)
            {
                let _ = w.poll.send(Req::LogTail {
                    project: project.clone(),
                    service: service.clone(),
                    since: app.log_cursor.clone(),
                });
            }
            app.refresh_inflight = true;
            last_stats = Instant::now();
        }

        // Poll lebih rapat saat terminal terbuka (120 ms terasa lag untuk ketikan)
        // atau saat ada animasi berjalan (spinner/denyut/kilat) supaya mulus; idle
        // tanpa animasi tetap 120 ms agar murah.
        let poll = if app.screen == Screen::Terminal {
            15
        } else if app.animating() {
            70
        } else {
            120
        };
        if event::poll(Duration::from_millis(poll))? {
            match event::read()? {
                Event::Mouse(m) => app.on_mouse(m, &w.user),
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if app.screen == Screen::Terminal {
                        // Ctrl-Q menutup sesi; SEMUA tombol lain (termasuk Ctrl-C)
                        // diteruskan ke shell.
                        let ctrl_q = key.code == KeyCode::Char('q')
                            && key.modifiers.contains(KeyModifiers::CONTROL);
                        if ctrl_q {
                            app.close_terminal();
                        } else if let (Some(bytes), Some(tx)) =
                            (terminal::encode_key(key), app.term_input.as_ref())
                        {
                            let _ = tx.send(terminal::TermMsg::Input(
                                String::from_utf8_lossy(&bytes).into_owned(),
                            ));
                        }
                    } else {
                        if key.code == KeyCode::Char('c')
                            && key.modifiers.contains(KeyModifiers::CONTROL)
                        {
                            break;
                        }
                        app.on_key(key.code, &w.user);
                    }
                }
                _ => {}
            }
        }

        // Layar Hosts: satu thread per server. Fan-out ada di sini karena hanya
        // event_loop yang memegang ServerConfig (url + token tiap host).
        if app.load_hosts {
            app.load_hosts = false;
            app.hosts = cfg
                .all()
                .into_iter()
                .map(|s| HostRow {
                    name: s.name,
                    url: s.url,
                    state: HostState::Loading,
                })
                .collect();
            for s in cfg.all() {
                let tx = w.resp_tx.clone();
                thread::spawn(move || {
                    let client = EasypanelClient::new(&s.url, &s.token);
                    let data = client
                        .call("metrics", "getSystemStats", json!({}))
                        .map_err(|e| e.to_string());
                    let _ = tx.send(Resp::HostStat { name: s.name, data });
                });
            }
        }

        // Perubahan daftar server perlu ServerConfig, yang hanya ada di sini.
        if let Some(action) = app.server_action.take() {
            app.status = match apply_server_action(cfg, action) {
                Ok(msg) => msg,
                Err(e) => format!("Error: {e}"),
            };
            app.all_servers = cfg.all().into_iter().map(|s| (s.name, s.url)).collect();
        }

        // Sunting sebuah field form (Dockerfile) di $EDITOR. Beda dengan env:
        // isinya sudah ada di form, jadi tak ada yang perlu diambil dari server.
        if let Some(idx) = app.edit_field.take() {
            let current = app
                .form
                .as_ref()
                .map(|f| f.fields[idx].value.clone())
                .unwrap_or_default();
            let name = app
                .form
                .as_ref()
                .map(|f| f.fields[idx].label.to_lowercase())
                .unwrap_or_else(|| "teks".into());
            // Namanya menentukan syntax highlighting editor: vim mengenali
            // *.dockerfile, tapi tidak "easypanel-tmp".
            match edit_text_in_editor(terminal, &format!("easypanel-form.{name}"), &current) {
                Ok(Some(edited)) => {
                    if let Some(form) = app.form.as_mut() {
                        form.fields[idx].value = edited;
                    }
                    app.status = "Diperbarui — Enter untuk menyimpan".into();
                }
                Ok(None) => app.status = "Tidak berubah".into(),
                Err(e) => app.status = format!("Error: {e}"),
            }
        }

        // Terminal container: resolve URL WebSocket (butuh ServerConfig, hanya di
        // sini), lalu jalankan sesi di thread. Output → Resp::TermOutput ke parser
        // vt100; keystroke dikirim balik lewat channel. Tabs & status tetap tampil.
        if let Some((project, service, db)) = app.terminal_req.take() {
            match cfg.get(&app.server_name) {
                Some(server) => {
                    let client = EasypanelClient::new(&server.url, &server.token);
                    // Shell DB: ambil rootPassword + nama database dari inspectService,
                    // bangun perintah klien mysql. Shell biasa: "sh".
                    let command = match &db {
                        Some(stype) => {
                            match client.call(
                                &format!("services/{stype}"),
                                "inspectService",
                                json!({ "projectName": project, "serviceName": service }),
                            ) {
                                Ok(v) => match terminal::db_command(stype, &v) {
                                    Some(cmd) => cmd,
                                    None => {
                                        app.status = format!("Shell DB tak didukung untuk {stype}");
                                        continue;
                                    }
                                },
                                Err(e) => {
                                    app.status = format!("Shell DB gagal: {e}");
                                    continue;
                                }
                            }
                        }
                        None => "sh".to_string(),
                    };
                    match terminal::ws_url(&client, &project, &service, &command) {
                        Ok(url) => {
                            let (cols, rows) =
                                ratatui::crossterm::terminal::size().unwrap_or((80, 24));
                            // Pane konten kira-kira ukuran layar minus tabs+status;
                            // render menyetel ulang persisnya.
                            let (tcols, trows) = (cols, rows.saturating_sub(5).max(1));
                            let (tx, rx) = std::sync::mpsc::channel();
                            app.term_parser = Some(vt100::Parser::new(trows, tcols, 0));
                            app.term_input = Some(tx);
                            let label =
                                db.as_deref().map(|s| format!(" ({s})")).unwrap_or_default();
                            app.term_title = format!("{project}/{service}{label}");
                            terminal::spawn_session(url, w.resp_tx.clone(), rx, tcols, trows);
                            app.screen = Screen::Terminal;
                            app.status = "Terminal — ketik `exit` atau Ctrl-Q untuk keluar".into();
                        }
                        Err(e) => app.status = format!("Error: {e}"),
                    }
                }
                None => app.status = "Server aktif tak ditemukan".into(),
            }
        }

        // Edit env: lepas terminal, buka $EDITOR, lalu ambil alih lagi.
        if let Some((project, service, stype, replace)) = app.edit_env.take() {
            // Ganti-cepat (replace): editor kosong, tanpa fetch. Simpan hanya kalau
            // user mengetik sesuatu — kosong berarti batal, BUKAN menghapus env.
            let edited = if replace {
                edit_text_in_editor(terminal, &format!("easypanel-{project}-{service}.env"), "")
            } else {
                edit_env_in_editor(&w.user, &w.resp, terminal, &project, &service, &stype)
            };
            match edited {
                Ok(Some(env)) => {
                    let _ = w.user.send(Req::EnvSave {
                        project,
                        service,
                        stype,
                        env,
                    });
                    app.status = "Menyimpan env...".into();
                }
                Ok(None) if replace => app.status = "Kosong — env tidak diubah".into(),
                Ok(None) => app.status = "Env tidak berubah".into(),
                Err(e) => app.status = format!("Error: {e}"),
            }
        }

        // Ganti server: bangun worker baru (yang lama berhenti saat sender-nya di-drop).
        if let Some(name) = app.switch_to.take() {
            if let Some(server) = cfg.get(&name) {
                w = spawn_workers(EasypanelClient::new(&server.url, &server.token));
                app.reset_for_server(name);
                send_initial(&w.user);
                // Muat data layar yang sedang dibuka (reset mengosongkannya), bukan
                // hanya global — kalau tidak, tetap di Services tapi tabelnya kosong.
                let screen = app.screen;
                app.goto(screen, &w.user);
                last_stats = Instant::now();
            }
        }

        if app.should_quit {
            break;
        }
    }
    Ok(())
}

fn apply_server_action(cfg: &ServerConfig, action: ServerAction) -> Result<String> {
    match action {
        ServerAction::Save { name, url, token } => {
            // Token tak pernah ditampilkan kembali ke layar; membiarkannya kosong
            // saat edit berarti "pakai yang lama", bukan "kosongkan".
            let token = match token {
                Some(t) => t,
                None => cfg
                    .get(&name)
                    .map(|s| s.token)
                    .ok_or_else(|| anyhow::anyhow!("server '{name}' tak ditemukan"))?,
            };
            cfg.add(&name, &url, &token)?;
            Ok(format!("Server '{name}' disimpan"))
        }
        ServerAction::Remove(name) => {
            cfg.remove(&name)?;
            Ok(format!("Server '{name}' dihapus"))
        }
    }
}

/// Ambil env service, buka di `$EDITOR`, kembalikan isinya bila berubah.
///
/// Memakai editor milik user (pola `kubectl edit`) alih-alih menulis textarea
/// sendiri di ratatui: jauh lebih sedikit kode dan sudah familier. Terminal
/// dilepas selama editor jalan, lalu diambil alih kembali.
fn edit_env_in_editor(
    req: &Sender<Req>,
    resp: &Receiver<Resp>,
    terminal: &mut ratatui::DefaultTerminal,
    project: &str,
    service: &str,
    stype: &str,
) -> Result<Option<String>> {
    // Ambil env terkini lebih dulu (blocking; user memang sedang menunggu).
    req.send(Req::Fetch {
        view: View::Env,
        project: project.to_string(),
        service: service.to_string(),
        stype: stype.to_string(),
    })?;

    let deadline = Instant::now() + Duration::from_secs(30);
    let current = loop {
        match resp.recv_timeout(Duration::from_millis(200)) {
            Ok(Resp::Viewer(_, lines)) => break lines.join("\n"),
            Ok(Resp::Err(e)) => return Err(anyhow::anyhow!(e)),
            Ok(_) => {}
            Err(_) if Instant::now() > deadline => {
                return Err(anyhow::anyhow!("timeout mengambil env"))
            }
            Err(_) => {}
        }
    };

    edit_text_in_editor(
        terminal,
        &format!("easypanel-{project}-{service}.env"),
        &current,
    )
}

/// Suntingkan teks di `$EDITOR`; None bila tak berubah.
///
/// Terminal dilepas selama editor jalan, lalu diambil alih kembali — termasuk
/// bila editornya gagal, kalau tidak TUI-nya tak pernah kembali.
fn edit_text_in_editor(
    terminal: &mut ratatui::DefaultTerminal,
    filename: &str,
    current: &str,
) -> Result<Option<String>> {
    let path = std::env::temp_dir().join(filename);
    std::fs::write(&path, current)?;

    disable_mouse();
    ratatui::restore();
    let opened = open_in_editor(&path);
    *terminal = ratatui::init();
    enable_mouse();
    terminal.clear()?;
    opened?;

    let edited = std::fs::read_to_string(&path)?;
    let _ = std::fs::remove_file(&path);

    Ok((edited.trim_end() != current.trim_end()).then_some(edited))
}

/// Kandidat editor: pilihan user dulu, lalu cadangan yang pasti ada di Unix.
///
/// Tiap entri dipecah jadi program + argumen, supaya `EDITOR="code -w"` bekerja
/// dan tidak dicari sebagai satu biner bernama "code -w".
fn editor_candidates() -> Vec<Vec<String>> {
    let mut out: Vec<Vec<String>> = ["VISUAL", "EDITOR"]
        .iter()
        .filter_map(|k| std::env::var(k).ok())
        .map(|v| v.split_whitespace().map(String::from).collect::<Vec<_>>())
        .filter(|v| !v.is_empty())
        .collect();
    out.push(vec!["vi".into()]);
    out.push(vec!["nano".into()]);
    out
}

/// Buka file di editor pertama yang benar-benar ada.
///
/// $EDITOR yang menunjuk editor tak terpasang (mis. `nvim` yang belum dipasang)
/// dulu gagal dengan "No such file or directory (os error 2)" — pesan yang
/// terbaca seolah file env-nya yang hilang, bukan editornya. Sekarang kandidat
/// yang hilang dilewati, dan kalau semuanya hilang pesannya menyebut nama-namanya.
fn open_in_editor(path: &std::path::Path) -> Result<()> {
    let mut missing = Vec::new();
    for cand in editor_candidates() {
        let (prog, args) = cand.split_first().expect("kandidat tak pernah kosong");
        match std::process::Command::new(prog)
            .args(args)
            .arg(path)
            .status()
        {
            Ok(status) if status.success() => return Ok(()),
            Ok(status) => anyhow::bail!("editor '{prog}' keluar dengan {status}"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => missing.push(prog.clone()),
            Err(e) => return Err(e.into()),
        }
    }
    anyhow::bail!(
        "tak ada editor yang bisa dipakai (dicoba: {}). Set $EDITOR ke editor yang terpasang.",
        missing.join(", ")
    )
}

fn send_initial(req_tx: &Sender<Req>) {
    let _ = req_tx.send(Req::Stats);
    let _ = req_tx.send(Req::Nodes);
    let _ = req_tx.send(Req::Projects);
}
