//! A terminal inside the container — embedded in a TUI pane (not a full-screen
//! takeover).
//!
//! The protocol was verified with a live WebSocket round trip to the server:
//! `wss://{panel}/ws/containerShell?container={id}&command={b64}&token={apiToken}`,
//! exchanging JSON `{"input"}` / `{"resize":[cols,rows]}` (to the server) and
//! `{"output"}` (from the server). Auth uses the API token this tool already
//! has stored.
//!
//! The WebSocket runs on its own thread: its output is sent as `Resp::TermOutput`
//! to be fed into the vt100 parser (which becomes the on-screen grid), and
//! keystrokes from the event loop are sent back over a channel. So tabs & the
//! status bar stay visible — the terminal lives inside the content pane,
//! seamlessly.

use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::Duration;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::{json, Value};
use tungstenite::Message;

use crate::container::{connect_failure, set_read_timeout};

use super::worker::Resp;

/// Message from the event loop to the WebSocket thread.
pub(super) enum TermMsg {
    Input(String),
    Resize(u16, u16),
}

/// The active container-terminal session's state, grouped out of `App` the same
/// way `ViewerUi`/`BackupUi`/`CredsUi` were — one sub-view, one struct, next to
/// the code that drives it.
///
/// `parser` and `input` are the live session (dropping `input` closes it);
/// `title` is the pane label and is deliberately NOT cleared alongside them —
/// the "Terminal {title} closed" message reads it after the session ends.
#[derive(Default)]
pub(super) struct TermUi {
    /// The screen emulator: a vt100 parser fed by WebSocket output.
    pub(super) parser: Option<vt100::Parser>,
    /// Keystrokes/resizes to the WebSocket thread. Dropping it = close the session.
    pub(super) input: Option<Sender<TermMsg>>,
    /// The terminal pane title (project/service).
    pub(super) title: String,
    /// The text selection being dragged (or held after release, until the next
    /// keystroke). None = nothing selected.
    pub(super) sel: Option<TermSel>,
}

/// A text selection over the terminal grid — what a mouse drag marks and its
/// release copies to the clipboard.
///
/// Rows are stored in the coordinate frame of `scrollback`: the offset in force
/// when the anchor was set. A line of output that sits at screen row `r` with
/// scrollback `s` sits at `r + 1` with scrollback `s + 1`, so `row - scrollback`
/// names the content rather than the viewport. Keeping the frame is what makes a
/// selection stay on the text it marked while the user scrolls (and while new
/// output arrives, which vt100 absorbs by bumping the offset).
///
/// The selection is linewise (reading order), not a block: whole rows between
/// the two ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TermSel {
    /// Where the drag started, as (row, col) in this selection's frame.
    pub(super) anchor: (i32, u16),
    /// Where the pointer is now, same frame.
    pub(super) cursor: (i32, u16),
    /// The scrollback offset the rows above are measured against.
    pub(super) scrollback: usize,
}

impl TermSel {
    /// A selection that starts and ends on one cell (a click that has not been
    /// dragged yet), anchored to what is on screen at scrollback `sb`.
    pub(super) fn new(row: u16, col: u16, sb: usize) -> Self {
        let at = (i32::from(row), col);
        Self {
            anchor: at,
            cursor: at,
            scrollback: sb,
        }
    }

    /// Move the loose end to a screen cell seen at scrollback `sb`.
    pub(super) fn extend(&mut self, row: u16, col: u16, sb: usize) {
        self.cursor = (self.frame_row(row, sb), col);
    }

    /// A click with no drag selects one cell — which is not something anyone
    /// means to copy, so it counts as nothing selected.
    pub(super) fn is_empty(&self) -> bool {
        self.anchor == self.cursor
    }

    /// Is the cell at screen (row, col), seen at scrollback `sb`, selected?
    pub(super) fn contains(&self, row: u16, col: u16, sb: usize) -> bool {
        let r = self.frame_row(row, sb);
        let (start, end) = self.ordered();
        r >= start.0
            && r <= end.0
            && !(r == start.0 && col < start.1)
            && !(r == end.0 && col > end.1)
    }

    /// A screen row seen at scrollback `sb`, in this selection's frame.
    fn frame_row(&self, row: u16, sb: usize) -> i32 {
        i32::from(row) + self.scrollback as i32 - sb as i32
    }

    /// The inverse: a frame row as a screen row at scrollback `sb`. May fall
    /// outside the grid — the content has been scrolled away from.
    fn screen_row(&self, row: i32, sb: usize) -> i32 {
        row - self.scrollback as i32 + sb as i32
    }

    /// The two ends in reading order; a drag may run backwards or upwards.
    fn ordered(&self) -> ((i32, u16), (i32, u16)) {
        if self.anchor <= self.cursor {
            (self.anchor, self.cursor)
        } else {
            (self.cursor, self.anchor)
        }
    }
}

/// The selected text, read off the screen as it stands.
///
/// vt100's own `contents_between` is the clipboard primitive here: it skips
/// wide-character continuation cells, pads gaps inside a line, and suppresses
/// the newline on a row the shell *wrapped* — so a copied long command line
/// pastes back as one line. Rows are joined with `\n`.
///
/// Blanks the shell actually printed (column padding in `ls -l`, a cleared
/// prompt line) do survive that, and a pasted line ending in invisible junk is
/// nobody's intent, so each row is trimmed at its tail.
///
/// Only the viewport can be read back, so a selection whose ends have been
/// scrolled out of sight contributes the part still visible.
pub(super) fn selection_text(screen: &vt100::Screen, sel: &TermSel) -> String {
    let (rows, cols) = screen.size();
    if rows == 0 || cols == 0 || sel.is_empty() {
        return String::new();
    }
    let sb = screen.scrollback();
    let (start, end) = sel.ordered();
    let (top, bottom) = (sel.screen_row(start.0, sb), sel.screen_row(end.0, sb));
    let last = i32::from(rows) - 1;
    if bottom < 0 || top > last {
        return String::new();
    }
    // A clamped end loses its column bound: the visible fragment runs to the
    // edge of the grid.
    let start_col = if top < 0 { 0 } else { start.1.min(cols - 1) };
    let end_col = if bottom > last {
        cols
    } else {
        end.1.min(cols - 1) + 1
    };
    let raw = screen.contents_between(
        top.max(0) as u16,
        start_col,
        bottom.min(last) as u16,
        end_col,
    );
    let mut out = String::with_capacity(raw.len());
    for (i, line) in raw.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(line.trim_end());
    }
    out
}

/// The shell command that opens a service's database client using its already
/// stored credentials (root/superuser). The password is passed via an env var
/// (doesn't show up in `ps`, doesn't trigger an "insecure" warning), safely
/// quoted for sh. None for non-database types. Each command's shape was
/// verified live against the server.
///
/// - mysql/mariadb: `mysql -uroot` (the `mysql` client is present in both images)
/// - postgres: `psql -U <user>`
/// - mongo: `mongosh` with auth (authSource admin, EasyPanel's default)
/// - redis: `redis-cli` (REDISCLI_AUTH)
pub(super) fn db_command(stype: &str, inspect: &Value) -> Option<String> {
    let f = |k: &str| inspect.get(k).and_then(Value::as_str).unwrap_or("");
    // Single-quote for sh: ' -> '\'' so any credential is safe.
    let q = |s: &str| s.replace('\'', "'\\''");
    let db = f("databaseName");
    match stype {
        "mysql" | "mariadb" => {
            let d = if db.is_empty() {
                String::new()
            } else {
                format!(" {db}")
            };
            Some(format!(
                "MYSQL_PWD='{}' mysql -uroot{d}",
                q(f("rootPassword"))
            ))
        }
        "postgres" => {
            let user = if f("user").is_empty() {
                "postgres"
            } else {
                f("user")
            };
            let d = if db.is_empty() {
                String::new()
            } else {
                format!(" -d {db}")
            };
            Some(format!(
                "PGPASSWORD='{}' psql -U {user}{d}",
                q(f("password"))
            ))
        }
        "mongo" => Some(format!(
            "mongosh -u '{}' -p '{}' --authenticationDatabase admin",
            q(f("user")),
            q(f("password"))
        )),
        "redis" => Some(format!("REDISCLI_AUTH='{}' redis-cli", q(f("password")))),
        _ => None,
    }
}

/// Run the WebSocket session on its own thread. Returns a sender for keystrokes
/// & resize events; output is sent as `Resp::TermOutput`, session end as `Resp::TermClosed`.
///
/// `auth_hint` is appended when the panel REFUSES the token at the handshake (and
/// only then). It exists because the two routes fail that way for different
/// reasons: a container shell needs a valid token, the host shell needs an ADMIN
/// one, and "terminal failed to connect: HTTP error: 403 Forbidden" sends an
/// operator hunting a network problem they do not have. Empty = nothing to add.
pub(super) fn spawn_session(
    url: String,
    resp_tx: Sender<Resp>,
    input_rx: Receiver<TermMsg>,
    cols: u16,
    rows: u16,
    auth_hint: &'static str,
) {
    std::thread::spawn(move || {
        let mut ws = match tungstenite::connect(&url) {
            Ok((ws, _)) => ws,
            Err(e) => {
                let hint = if crate::container::is_auth_rejection(&e) {
                    auth_hint
                } else {
                    ""
                };
                let _ = resp_tx.send(Resp::Err(format!(
                    "terminal failed to connect: {}{hint}",
                    connect_failure(&e)
                )));
                let _ = resp_tx.send(Resp::TermClosed);
                return;
            }
        };
        // A small read timeout so the loop also gets a chance to handle input & resize.
        set_read_timeout(&mut ws, Duration::from_millis(15));
        let _ = ws.send(resize_msg(cols, rows));

        loop {
            // Drain any pending output.
            loop {
                match ws.read() {
                    Ok(Message::Text(t)) => {
                        if let Some(out) = serde_json::from_str::<Value>(&t).ok().and_then(|v| {
                            v.get("output").and_then(Value::as_str).map(str::to_string)
                        }) {
                            if resp_tx.send(Resp::TermOutput(out.into_bytes())).is_err() {
                                return;
                            }
                        }
                    }
                    Ok(Message::Close(_)) => {
                        let _ = resp_tx.send(Resp::TermClosed);
                        return;
                    }
                    Ok(_) => {}
                    Err(tungstenite::Error::Io(e))
                        if matches!(
                            e.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                        ) =>
                    {
                        break
                    }
                    Err(_) => {
                        let _ = resp_tx.send(Resp::TermClosed);
                        return;
                    }
                }
            }

            // Forward any pending input/resize. If input_rx disconnects (the app
            // closed the terminal, e.g. Ctrl-Q) → end the session.
            loop {
                match input_rx.try_recv() {
                    Ok(msg) => {
                        let out = match msg {
                            TermMsg::Input(s) => {
                                ws.send(Message::Text(json!({ "input": s }).to_string()))
                            }
                            TermMsg::Resize(c, r) => ws.send(resize_msg(c, r)),
                        };
                        if out.is_err() {
                            let _ = resp_tx.send(Resp::TermClosed);
                            return;
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        let _ = ws.close(None);
                        return;
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(4));
        }
    });
}

fn resize_msg(cols: u16, rows: u16) -> Message {
    Message::Text(json!({ "resize": [cols, rows] }).to_string())
}

/// Encode a KeyEvent into bytes to send to the shell (xterm encoding).
/// None = not sent (e.g. an unmapped key).
pub(super) fn encode_key(key: KeyEvent) -> Option<Vec<u8>> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let bytes = match key.code {
        KeyCode::Char(c) => {
            if ctrl {
                // Ctrl-A..Z → 0x01..0x1a; matches a real terminal.
                let b = (c.to_ascii_uppercase() as u8).wrapping_sub(b'@') & 0x7f;
                vec![b]
            } else if alt {
                let mut v = vec![0x1b];
                v.extend(c.to_string().into_bytes());
                v
            } else {
                c.to_string().into_bytes()
            }
        }
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::BackTab => vec![0x1b, b'[', b'Z'],
        KeyCode::Esc => vec![0x1b],
        KeyCode::Left => vec![0x1b, b'[', b'D'],
        KeyCode::Right => vec![0x1b, b'[', b'C'],
        KeyCode::Up => vec![0x1b, b'[', b'A'],
        KeyCode::Down => vec![0x1b, b'[', b'B'],
        KeyCode::Home => vec![0x1b, b'[', b'H'],
        KeyCode::End => vec![0x1b, b'[', b'F'],
        KeyCode::Delete => vec![0x1b, b'[', b'3', b'~'],
        KeyCode::Insert => vec![0x1b, b'[', b'2', b'~'],
        KeyCode::PageUp => vec![0x1b, b'[', b'5', b'~'],
        KeyCode::PageDown => vec![0x1b, b'[', b'6', b'~'],
        _ => return None,
    };
    Some(bytes)
}
