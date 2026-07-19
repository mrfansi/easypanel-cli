//! The EasyPanel TUI.
//!
//! Split along its data flow, not by type: `worker` talks to the network on
//! another thread and only knows Req/Resp; `app` holds the state and the keys;
//! `render` draws and never decides anything; `form` and `table` are the shared
//! vocabulary between them. `mod.rs` only ties it together: the event loop, the
//! handoff to $EDITOR, and server-list changes — the only place that holds the
//! ServerConfig.

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
/// The cap on lines the viewer holds while a log tail is running.
const LOG_BUFFER: usize = 5_000;
/// Lines of terminal history kept for scrolling back.
///
/// It used to be ZERO — the parser was told to keep no scrollback at all, so
/// output that left the screen was discarded rather than merely out of reach.
/// No key could have brought it back.
pub(super) const TERM_SCROLLBACK: usize = 5_000;
/// How far Shift+PageUp/PageDown move through that history.
const TERM_PAGE: isize = 10;

use app::{App, HostRow, HostState, Screen, ServerAction};
use render::ui;
use worker::{spawn_workers, Req, Resp, View};

/// Open the TUI for the default server (or the resolved --server).
pub fn run(cfg: &ServerConfig, client: EasypanelClient, server_name: String) -> Result<()> {
    if cfg.all().is_empty() {
        println!("No servers yet. Run: easypanel server add");
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

/// Capture mouse events (tab/row clicks, scroll). Side effect: the terminal's
/// built-in text selection is disabled — use Shift+drag in most terminals to copy.
fn enable_mouse() {
    let _ = ratatui::crossterm::execute!(std::io::stdout(), EnableMouseCapture);
}

fn disable_mouse() {
    let _ = ratatui::crossterm::execute!(std::io::stdout(), DisableMouseCapture);
}

/// How long a transient notice stays before the status line returns to "Ready".
const STATUS_IDLE: Duration = Duration::from_secs(6);

/// Should the status line revert to "Ready" now?
///
/// Tracked centrally rather than in every scattered `self.status = …`, so a
/// notice like "Deploy started" doesn't linger forever. Two things are never
/// faded:
///
/// - "Ready" itself — there is nothing to revert to.
/// - A failure — it is the ONLY copy the user gets. There is no log and no
///   history to scroll back to, so erasing it after six seconds loses the message
///   for good AND replaces it with a claim that everything is fine.
pub(super) fn status_should_fade(status: &str, idle: Duration, busy: usize) -> bool {
    status != "Ready" && !app::status_is_error(status) && busy == 0 && idle >= STATUS_IDLE
}

fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    cfg: &ServerConfig,
    client: EasypanelClient,
) -> Result<()> {
    let mut w = spawn_workers(client);
    // The App decides what to draw; the worker knows what is running. One shared
    // counter joins them.
    app.busy = w.busy.clone();
    send_initial(&w.user);
    let mut last_stats = Instant::now();
    let mut last_status = app.status.clone();
    let mut status_since = Instant::now();

    loop {
        while let Ok(resp) = w.resp.try_recv() {
            app.handle(resp, &w.user);
        }

        if app.status != last_status {
            last_status = app.status.clone();
            status_since = Instant::now();
        } else if status_should_fade(&app.status, status_since.elapsed(), app.busy()) {
            app.status = "Ready".into();
            last_status = app.status.clone();
        }

        app.tick_anim();
        terminal.draw(|f| ui(f, app))?;

        // Metrics run on the poll lane. The in-flight guard keeps rounds from
        // stacking up when the server is slower than the refresh interval.
        if last_stats.elapsed() >= REFRESH && !app.refresh_inflight {
            let _ = w.poll.send(Req::Stats);
            // Per-service metrics go live too, but only on the screen that shows them.
            if matches!(app.screen, Screen::Monitor | Screen::Projects) {
                let _ = w.poll.send(Req::MonitorData);
            }
            // The "down" status goes live in the Services table.
            if app.screen == Screen::Projects {
                let _ = w.poll.send(Req::TaskStats);
            }
            // In-progress deploys stay live in the Status column (Projects) and the
            // Actions tab stays fresh (it used to be frozen until `r`). One
            // listActions call.
            if matches!(app.screen, Screen::Projects | Screen::Actions) {
                let _ = w.poll.send(Req::Actions);
            }
            // Logs stay live while their viewer is open. On the poll lane, not the
            // user lane: a tail every two seconds must not queue behind (or ahead
            // of) an action the user pressed.
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

        // Poll more tightly while the terminal is open (120 ms feels laggy for
        // typing) or while an animation is running (spinner/pulse/flash) so it's
        // smooth; idle with no animation stays at 120 ms to keep it cheap.
        let poll = if app.screen == Screen::Terminal {
            15
        } else if app.animating() {
            70
        } else {
            120
        };
        if event::poll(Duration::from_millis(poll))? {
            match event::read()? {
                // The wheel scrolls the terminal's own history; everywhere else
                // it belongs to the tables.
                Event::Mouse(m) if app.screen == Screen::Terminal => match m.kind {
                    event::MouseEventKind::ScrollUp => app.term_scroll(3),
                    event::MouseEventKind::ScrollDown => app.term_scroll(-3),
                    _ => {}
                },
                Event::Mouse(m) => app.on_mouse(m, &w.user),
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if app.screen == Screen::Terminal {
                        // Ctrl-Q closes the session; EVERY other key (including
                        // Ctrl-C) is forwarded to the shell.
                        let ctrl_q = key.code == KeyCode::Char('q')
                            && key.modifiers.contains(KeyModifiers::CONTROL);
                        // Shift+PageUp/PageDown walk the scrollback, the binding
                        // every terminal emulator uses for exactly this. Held by
                        // the UI rather than forwarded: a shell has no idea what
                        // scrolled off ITS output, so nothing downstream can serve
                        // this.
                        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
                        if ctrl_q {
                            app.close_terminal();
                        } else if shift && key.code == KeyCode::PageUp {
                            app.term_scroll(TERM_PAGE);
                        } else if shift && key.code == KeyCode::PageDown {
                            app.term_scroll(-TERM_PAGE);
                        } else if let (Some(bytes), Some(tx)) =
                            (terminal::encode_key(key), app.term_input.as_ref())
                        {
                            let _ = tx.send(terminal::TermMsg::Input(
                                String::from_utf8_lossy(&bytes).into_owned(),
                            ));
                            // Typing returns to the live view: otherwise the keys
                            // go to a shell you cannot see answering them.
                            app.term_scroll(isize::MIN / 2);
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

        // Hosts screen: one thread per server. The fan-out is here because only
        // event_loop holds the ServerConfig (each host's url + token).
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

        // A server-list change needs the ServerConfig, which only lives here.
        if let Some(action) = app.server_action.take() {
            app.status = match apply_server_action(cfg, action) {
                Ok(msg) => msg,
                Err(e) => format!("Error: {e}"),
            };
            app.all_servers = cfg.all().into_iter().map(|s| (s.name, s.url)).collect();
        }

        // Migration: the App knows the destination by name, but its token lives in
        // the ServerConfig, which only exists here.
        if let Some(m) = app.migrate_req.take() {
            match cfg.get(&m.target_server) {
                Some(server) => {
                    app.status = format!(
                        "Migrating {} service(s) to {}/{}…",
                        m.services.len(),
                        m.target_server,
                        m.target_project
                    );
                    let _ = w.user.send(Req::Migrate {
                        target_url: server.url,
                        target_token: server.token,
                        target_name: m.target_server,
                        target_project: m.target_project,
                        services: m.services,
                    });
                }
                None => {
                    app.status = format!("Server '{}' is no longer configured", m.target_server)
                }
            }
        }

        // Edit a form field (Dockerfile) in $EDITOR. Unlike env: the contents are
        // already in the form, so there's nothing to fetch from the server.
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
                .unwrap_or_else(|| "text".into());
            // The name drives the editor's syntax highlighting: vim recognizes
            // *.dockerfile, but not "easypanel-tmp".
            match edit_text_in_editor(terminal, &format!("easypanel-form.{name}"), &current) {
                Ok(Some(edited)) => {
                    if let Some(form) = app.form.as_mut() {
                        form.fields[idx].value = edited;
                    }
                    app.status = "Updated — Enter to save".into();
                }
                Ok(None) => app.status = "Unchanged".into(),
                Err(e) => app.status = format!("Error: {e}"),
            }
        }

        // Container terminal: resolve the WebSocket URL (needs ServerConfig, only
        // here), then run the session on a thread. Output → Resp::TermOutput to the
        // vt100 parser; keystrokes go back via a channel. Tabs & status stay visible.
        if let Some((project, service, db)) = app.terminal_req.take() {
            match cfg.get(&app.server_name) {
                Some(server) => {
                    let client = EasypanelClient::new(&server.url, &server.token);
                    // DB shell: take rootPassword + the database name from
                    // inspectService, build the mysql client command. Plain shell: "sh".
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
                                        app.status = format!("DB shell not supported for {stype}");
                                        continue;
                                    }
                                },
                                Err(e) => {
                                    app.status = format!("DB shell failed: {e}");
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
                            // The content pane is roughly the screen minus
                            // tabs+status; render sets the exact size.
                            let (tcols, trows) = (cols, rows.saturating_sub(5).max(1));
                            let (tx, rx) = std::sync::mpsc::channel();
                            app.term_parser =
                                Some(vt100::Parser::new(trows, tcols, TERM_SCROLLBACK));
                            app.term_input = Some(tx);
                            let label =
                                db.as_deref().map(|s| format!(" ({s})")).unwrap_or_default();
                            app.term_title = format!("{project}/{service}{label}");
                            terminal::spawn_session(url, w.resp_tx.clone(), rx, tcols, trows);
                            app.screen = Screen::Terminal;
                            app.status = "Terminal — type `exit` or Ctrl-Q to leave".into();
                        }
                        Err(e) => app.status = format!("Error: {e}"),
                    }
                }
                None => app.status = "Active server not found".into(),
            }
        }

        // Edit env: release the terminal, open $EDITOR, then take it back.
        //
        // There used to be a second mode that opened a BLANK editor ("replace
        // entire env"). Saving sends the whole string either way, so it was the
        // same operation starting from an empty buffer — something you do inside
        // your editor, not a separate feature with its own menu entry and key.
        if let Some((project, service, stype)) = app.edit_env.take() {
            match edit_env_in_editor(&w.user, &w.resp, terminal, &project, &service, &stype) {
                Ok(Some(env)) => {
                    let _ = w.user.send(Req::EnvSave {
                        project,
                        service,
                        stype,
                        env,
                    });
                    app.status = "Saving env...".into();
                }
                Ok(None) => app.status = "Env unchanged".into(),
                Err(e) => app.status = format!("Error: {e}"),
            }
        }

        // Edit the Config File (Advanced db) in $EDITOR, then save via updateAdvanced.
        if let Some((project, service, stype)) = app.edit_config.take() {
            match edit_config_in_editor(&w.user, &w.resp, terminal, &project, &service, &stype) {
                Ok(Some(config)) => {
                    let _ = w.user.send(Req::ConfigFileSave {
                        project,
                        service,
                        stype,
                        config,
                    });
                    app.status = "Saving config file...".into();
                }
                Ok(None) => app.status = "Config file unchanged".into(),
                Err(e) => app.status = format!("Error: {e}"),
            }
        }

        // Switch server: build a new worker (the old one stops when its sender is dropped).
        if let Some(name) = app.switch_to.take() {
            if let Some(server) = cfg.get(&name) {
                w = spawn_workers(EasypanelClient::new(&server.url, &server.token));
                app.reset_for_server(name);
                send_initial(&w.user);
                // Load the currently open screen's data (reset cleared it), not just
                // the global stuff — otherwise we stay on Services with an empty table.
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
            // The token is never shown back on screen; leaving it empty on edit
            // means "keep the old one", not "clear it".
            let token = match token {
                Some(t) => t,
                None => cfg
                    .get(&name)
                    .map(|s| s.token)
                    .ok_or_else(|| anyhow::anyhow!("server '{name}' not found"))?,
            };
            cfg.add(&name, &url, &token)?;
            Ok(format!("Server '{name}' saved"))
        }
        ServerAction::Remove(name) => {
            cfg.remove(&name)?;
            Ok(format!("Server '{name}' deleted"))
        }
    }
}

/// Fetch a service's env, open it in `$EDITOR`, return the contents if changed.
///
/// Uses the user's editor (the `kubectl edit` pattern) instead of writing our own
/// textarea in ratatui: far less code and already familiar. The terminal is
/// released while the editor runs, then taken back.
fn edit_env_in_editor(
    req: &Sender<Req>,
    resp: &Receiver<Resp>,
    terminal: &mut ratatui::DefaultTerminal,
    project: &str,
    service: &str,
    stype: &str,
) -> Result<Option<String>> {
    // Fetch the current env first (blocking; the user is waiting on it anyway).
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
                return Err(anyhow::anyhow!("timed out fetching env"))
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

/// Fetch a service's Config File (Advanced), open it in `$EDITOR`, return it if
/// changed. Like edit_env_in_editor but loads `configFile`.
fn edit_config_in_editor(
    req: &Sender<Req>,
    resp: &Receiver<Resp>,
    terminal: &mut ratatui::DefaultTerminal,
    project: &str,
    service: &str,
    stype: &str,
) -> Result<Option<String>> {
    req.send(Req::Fetch {
        view: View::ConfigFile,
        project: project.to_string(),
        service: service.to_string(),
        stype: stype.to_string(),
    })?;

    let deadline = Instant::now() + Duration::from_secs(30);
    let current = loop {
        match resp.recv_timeout(Duration::from_millis(200)) {
            // fetch_view returns "(empty)" for an empty config — don't load that as
            // the editor's initial contents.
            Ok(Resp::Viewer(_, lines)) => {
                break if lines == ["(empty)"] {
                    String::new()
                } else {
                    lines.join("\n")
                }
            }
            Ok(Resp::Err(e)) => return Err(anyhow::anyhow!(e)),
            Ok(_) => {}
            Err(_) if Instant::now() > deadline => {
                return Err(anyhow::anyhow!("timed out fetching config file"))
            }
            Err(_) => {}
        }
    };

    edit_text_in_editor(
        terminal,
        &format!("easypanel-{project}-{service}.conf"),
        &current,
    )
}

/// Edit text in `$EDITOR`; None if unchanged.
///
/// The terminal is released while the editor runs, then taken back — including if
/// the editor fails, otherwise the TUI never comes back.
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

/// GUI editors and the flag that makes them BLOCK until the file is closed.
///
/// Without it they hand the file to an already-running window and exit at once.
/// The TUI would then read the temp file before a single keystroke was typed,
/// find it unchanged, and delete it — the user's edit silently thrown away, with
/// the UI reporting "Unchanged". Terminal editors (vi, nano, nvim, emacs, helix,
/// micro) already block, so they aren't listed.
const EDITOR_WAIT_FLAGS: &[(&str, &str)] = &[
    ("code", "--wait"),
    ("code-insiders", "--wait"),
    ("codium", "--wait"),
    ("vscodium", "--wait"),
    ("cursor", "--wait"),
    ("windsurf", "--wait"),
    ("positron", "--wait"),
    ("zed", "--wait"),
    ("subl", "--wait"),
    ("sublime_text", "--wait"),
    ("mate", "--wait"),
    ("atom", "--wait"),
    ("gvim", "-f"),
    ("kate", "--block"),
    // JetBrains launchers all take the same flag.
    ("idea", "--wait"),
    ("webstorm", "--wait"),
    ("pycharm", "--wait"),
    ("phpstorm", "--wait"),
    ("goland", "--wait"),
    ("rustrover", "--wait"),
    ("clion", "--wait"),
    ("rubymine", "--wait"),
];

/// The wait flag for `prog`, if it's a GUI editor that needs one.
///
/// Matched on the file name so a full path (`/usr/local/bin/code`) and a Windows
/// launcher (`code.cmd`) resolve the same as a bare name.
fn editor_wait_flag(prog: &str) -> Option<&'static str> {
    let name = std::path::Path::new(prog)
        .file_stem()?
        .to_str()?
        .to_ascii_lowercase();
    EDITOR_WAIT_FLAGS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, f)| *f)
}

/// Make a GUI editor wait, unless the user already said so.
///
/// Returns the command to run and whether it's a GUI editor — the caller says
/// what it's waiting for, because the TUI is torn down at that point and an
/// otherwise-blank terminal looks like a hang.
pub(super) fn with_editor_wait(cmd: &[String]) -> (Vec<String>, bool) {
    let Some((prog, args)) = cmd.split_first() else {
        return (cmd.to_vec(), false);
    };
    let Some(flag) = editor_wait_flag(prog) else {
        return (cmd.to_vec(), false);
    };
    // `EDITOR="code -w"` is already correct; don't pass the flag twice.
    let already = args
        .iter()
        .any(|a| matches!(a.as_str(), "-w" | "--wait" | "-f" | "--block"));
    if already {
        return (cmd.to_vec(), true);
    }
    let mut out = cmd.to_vec();
    out.push(flag.to_string());
    (out, true)
}

/// Editor candidates: the user's choice first, then fallbacks that are sure to
/// exist on Unix.
///
/// Each entry is split into program + arguments, so `EDITOR="code -w"` works and
/// isn't looked up as a single binary named "code -w". `EASYPANEL_EDITOR` wins
/// over the global $EDITOR, so a terminal editor can be used here without
/// changing the editor everything else on the machine uses.
fn editor_candidates() -> Vec<Vec<String>> {
    let mut out: Vec<Vec<String>> = ["EASYPANEL_EDITOR", "VISUAL", "EDITOR"]
        .iter()
        .filter_map(|k| std::env::var(k).ok())
        .map(|v| v.split_whitespace().map(String::from).collect::<Vec<_>>())
        .filter(|v| !v.is_empty())
        .map(|v| with_editor_wait(&v).0)
        .collect();
    out.push(vec!["vi".into()]);
    out.push(vec!["nano".into()]);
    out
}

/// Open the file in the first editor that actually exists.
///
/// A $EDITOR pointing at an uninstalled editor (e.g. `nvim` that isn't installed)
/// used to fail with "No such file or directory (os error 2)" — a message that
/// reads as if the env file were missing, not the editor. Now a missing candidate
/// is skipped, and if they're all missing the message names them.
fn open_in_editor(path: &std::path::Path) -> Result<()> {
    let mut missing = Vec::new();
    for cand in editor_candidates() {
        let (prog, args) = cand.split_first().expect("candidate is never empty");
        // The TUI is torn down while the editor runs. A terminal editor fills that
        // blank screen itself; a GUI one leaves it empty, which reads as a hang —
        // so say what we're waiting for.
        if editor_wait_flag(prog).is_some() {
            println!("Waiting for {prog} — save and close the file there to come back.");
        }
        match std::process::Command::new(prog)
            .args(args)
            .arg(path)
            .status()
        {
            Ok(status) if status.success() => return Ok(()),
            Ok(status) => anyhow::bail!("editor '{prog}' exited with {status}"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => missing.push(prog.clone()),
            Err(e) => return Err(e.into()),
        }
    }
    anyhow::bail!(
        "no usable editor found (tried: {}). Set $EDITOR to an installed editor.",
        missing.join(", ")
    )
}

fn send_initial(req_tx: &Sender<Req>) {
    let _ = req_tx.send(Req::Stats);
    let _ = req_tx.send(Req::Nodes);
    let _ = req_tx.send(Req::Projects);
}
