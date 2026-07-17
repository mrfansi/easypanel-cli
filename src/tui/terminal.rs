//! Terminal ke dalam container — tertanam di pane TUI (bukan alih layar).
//!
//! Protokol diverifikasi dengan round-trip WebSocket hidup ke server:
//! `wss://{panel}/ws/containerShell?container={id}&command={b64}&token={apiToken}`,
//! bertukar JSON `{"input"}` / `{"resize":[cols,rows]}` (ke server) dan
//! `{"output"}` (dari server). Auth memakai token API yang tool ini sudah simpan.
//!
//! WebSocket berjalan di thread sendiri: output-nya dikirim sebagai `Resp::TermOutput`
//! untuk diumpankan ke parser vt100 (yang jadi grid di layar), dan keystroke dari
//! event loop dikirim balik lewat channel. Jadi tabs & status bar tetap tampil —
//! terminal hidup di dalam pane konten, seamless.

use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::Duration;

use anyhow::{anyhow, Result};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::{json, Value};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::Message;

use crate::client::EasypanelClient;

use super::worker::Resp;

/// Pesan dari event loop ke thread WebSocket.
pub(super) enum TermMsg {
    Input(String),
    Resize(u16, u16),
}

/// Resolve ID container yang berjalan untuk sebuah service (nama swarm
/// "{project}_{service}"), lalu bangun URL WebSocket terminalnya.
pub(super) fn ws_url(client: &EasypanelClient, project: &str, service: &str) -> Result<String> {
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
        .ok_or_else(|| anyhow!("Tak ada container berjalan untuk {project}/{service}"))?;
    // command = base64("sh") = "c2g="; `-it` memberi TTY → shell interaktif.
    let wss = client
        .url()
        .trim_end_matches('/')
        .replacen("https://", "wss://", 1)
        .replacen("http://", "ws://", 1);
    Ok(format!(
        "{wss}/ws/containerShell?container={cid}&command=c2g=&token={}",
        client.token()
    ))
}

/// Jalankan sesi WebSocket di thread sendiri. Kembalikan pengirim untuk keystroke
/// & resize; output dikirim sebagai `Resp::TermOutput`, akhir sesi `Resp::TermClosed`.
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
                let _ = resp_tx.send(Resp::Err(format!("terminal gagal tersambung: {e}")));
                let _ = resp_tx.send(Resp::TermClosed);
                return;
            }
        };
        // Read timeout kecil supaya loop juga sempat mengurus input & resize.
        set_read_timeout(&mut ws, Duration::from_millis(15));
        let _ = ws.send(resize_msg(cols, rows));

        loop {
            // Kuras output yang ada.
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

            // Teruskan input/resize yang tertunda. input_rx putus (app menutup
            // terminal, mis. Ctrl-Q) → akhiri sesi.
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

/// Encode sebuah KeyEvent jadi byte yang dikirim ke shell (encoding xterm).
/// None = tak dikirim (mis. tombol yang tak dipetakan).
pub(super) fn encode_key(key: KeyEvent) -> Option<Vec<u8>> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let bytes = match key.code {
        KeyCode::Char(c) => {
            if ctrl {
                // Ctrl-A..Z → 0x01..0x1a; mengikuti terminal sungguhan.
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
