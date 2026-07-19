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

use anyhow::{anyhow, Result};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::{json, Value};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::Message;

use crate::client::EasypanelClient;

use super::worker::Resp;

/// Message from the event loop to the WebSocket thread.
pub(super) enum TermMsg {
    Input(String),
    Resize(u16, u16),
}

/// Resolve the running container ID for a service (swarm name
/// "{project}_{service}"), then build its terminal WebSocket URL. `command` is
/// what to run inside the container (e.g. "sh" for a shell, or a mysql command
/// for a database shell) — the server wraps it with `docker exec -it … /bin/sh -c`.
pub(super) fn ws_url(
    client: &EasypanelClient,
    project: &str,
    service: &str,
    command: &str,
) -> Result<String> {
    let containers = client.call(
        "projects",
        "getDockerContainers",
        json!({ "service": format!("{project}_{service}") }),
    )?;
    let cid = containers
        .as_array()
        .and_then(|a| {
            a.iter()
                .find(|c| c.get("State").and_then(Value::as_str) == Some("running"))
        })
        .and_then(|c| c.get("Id").and_then(Value::as_str))
        .ok_or_else(|| anyhow!("No running container for {project}/{service}"))?;
    let wss = client
        .url()
        .trim_end_matches('/')
        .replacen("https://", "wss://", 1)
        .replacen("http://", "ws://", 1);
    Ok(format!(
        "{wss}/ws/containerShell?container={cid}&command={}&token={}",
        base64(command.as_bytes()),
        client.token()
    ))
}

/// Standard base64 (with padding). Hand-written — the encoding is trivial, no
/// need to add a dependency. Used for the terminal WebSocket's `command` parameter.
pub(super) fn base64(input: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let n = ((chunk[0] as u32) << 16)
            | ((*chunk.get(1).unwrap_or(&0) as u32) << 8)
            | (*chunk.get(2).unwrap_or(&0) as u32);
        out.push(T[((n >> 18) & 63) as usize] as char);
        out.push(T[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            T[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// Run ONE command in a container and return what it printed.
///
/// The interactive session in `spawn_session` is a long-lived conversation; this
/// is the opposite — connect, let the command run, collect, close. It exists so
/// the tool can ask a database engine what schemas it holds, which no EasyPanel
/// endpoint will tell you.
///
/// Output stops being collected after `quiet` with nothing new, so a command that
/// hangs (a database still starting) returns what it managed to say instead of
/// blocking the worker forever.
pub(super) fn run_once(
    client: &EasypanelClient,
    project: &str,
    service: &str,
    command: &str,
) -> Result<String> {
    const QUIET: Duration = Duration::from_millis(1200);
    const CAP: Duration = Duration::from_secs(20);

    let url = ws_url(client, project, service, command)?;
    let (mut ws, _) = tungstenite::connect(&url)?;
    set_read_timeout(&mut ws, Duration::from_millis(200));
    let (start, mut last) = (std::time::Instant::now(), std::time::Instant::now());
    let mut out = String::new();
    while start.elapsed() < CAP && last.elapsed() < QUIET {
        match ws.read() {
            Ok(Message::Text(t)) => {
                if let Some(o) = serde_json::from_str::<Value>(&t)
                    .ok()
                    .and_then(|v| v.get("output").and_then(Value::as_str).map(String::from))
                {
                    out.push_str(&o);
                    last = std::time::Instant::now();
                }
            }
            Ok(Message::Close(_)) => break,
            Ok(_) => {}
            // A read timeout is the normal quiet case, not a failure.
            Err(tungstenite::Error::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(tungstenite::Error::Io(e)) if e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(_) => break,
        }
    }
    let _ = ws.close(None);
    Ok(out)
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
pub(super) fn spawn_session(
    url: String,
    resp_tx: Sender<Resp>,
    input_rx: Receiver<TermMsg>,
    cols: u16,
    rows: u16,
) {
    std::thread::spawn(move || {
        let mut ws = match tungstenite::connect(&url) {
            Ok((ws, _)) => ws,
            Err(e) => {
                let _ = resp_tx.send(Resp::Err(format!("terminal failed to connect: {e}")));
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

fn set_read_timeout(
    ws: &mut tungstenite::WebSocket<MaybeTlsStream<std::net::TcpStream>>,
    dur: Duration,
) {
    let stream = match ws.get_ref() {
        MaybeTlsStream::Plain(s) => Some(s),
        MaybeTlsStream::Rustls(s) => Some(s.get_ref()),
        _ => None,
    };
    if let Some(s) = stream {
        let _ = s.set_read_timeout(Some(dur));
    }
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
