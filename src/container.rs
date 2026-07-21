//! Running a command inside a service's container, over the panel's WebSocket.
//!
//! EasyPanel exposes no "exec" REST endpoint; the only way into a container is
//! its `wss://{panel}/ws/containerShell` socket. These primitives build that URL,
//! encode the command, and (for a one-shot) run it and collect what it printed —
//! the infrastructure both the embedded TUI terminal (`tui::terminal`) and any
//! CLI command that needs to reach inside a container are built on. They used to
//! live inside `tui::terminal`; they are not TUI-specific, so they moved here
//! where the CLI can reach them too.

use std::time::Duration;

use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::Message;

use crate::client::EasypanelClient;

/// The `wss://…/ws/containerShell` URL that runs `command` in a service's
/// running container. Resolves the container id first (a service can have several
/// tasks; only a running one can be exec'd into).
pub(crate) fn ws_url(
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

/// Why a connection could not be made, WITHOUT the URL it tried.
///
/// The URL carries `?token={api token}` and, for a database shell, the base64 of
/// a command containing the root password. tungstenite's `Error::Url` renders as
/// "Unable to connect to {the whole URI}", and that was formatted straight into
/// the status line — so any panel outage, wrong port or firewalled host printed
/// the API token on screen and left it in the terminal's scrollback, ready to be
/// screenshotted into a bug report.
///
/// Only that one variant carries the URI; `Io`, `Http` and `Protocol` do not, so
/// their messages (which are the useful ones) pass through untouched.
pub(crate) fn connect_failure(e: &tungstenite::Error) -> String {
    match e {
        tungstenite::Error::Url(_) => "could not reach the panel".to_string(),
        other => other.to_string(),
    }
}

/// Standard base64 (with padding). Hand-written — the encoding is trivial, no
/// need to add a dependency. Used for the container WebSocket's `command` parameter.
pub(crate) fn base64(input: &[u8]) -> String {
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
/// The interactive session in `tui::terminal::spawn_session` is a long-lived
/// conversation; this is the opposite — connect, let the command run, collect,
/// close. It exists so the tool can ask a database engine what schemas it holds,
/// which no EasyPanel endpoint will tell you.
///
/// Output stops being collected after `QUIET` with nothing new, so a command that
/// hangs (a database still starting) returns what it managed to say instead of
/// blocking the caller forever.
pub(crate) fn run_once(
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

/// The marker `run_until_done` appends to learn when a command finished and with
/// what exit status. Distinctive enough not to collide with real output.
const DONE_MARK: &str = "__EZP_DONE_";

/// The result of a long-running one-shot: the exit status (None if the marker
/// never arrived — a dropped connection or a timeout) and everything the command
/// printed, with the sentinel line stripped out.
pub(crate) struct Run {
    pub exit_code: Option<i32>,
    pub output: String,
}

/// Run ONE command that may take much longer than [`run_once`]'s 20 s cap — a
/// database dump piped to `gzip` and uploaded to object storage — and wait for it
/// to actually finish.
///
/// `run_once` stops on a quiet socket, which is wrong here: a healthy `mysqldump |
/// gzip | curl` is SILENT for its whole run, then exits. And the socket merely
/// closing can't tell success from failure. So we append `printf '<MARK>%s__' $?`:
/// its appearance means "done", and the number is the pipeline's exit status.
/// Reading continues until the marker, the socket closing, or `cap` — whichever is
/// first. A `docker exec -it -c <cmd>` passes the command as an argument (not typed
/// on stdin), so the command text — including the password and presigned URL — is
/// never echoed back into `output`; only the command's own stdout/stderr is.
pub(crate) fn run_until_done(
    client: &EasypanelClient,
    project: &str,
    service: &str,
    command: &str,
    cap: Duration,
) -> Result<Run> {
    let wrapped = format!("{command}; printf '{DONE_MARK}%s__\\n' \"$?\"");
    let url = ws_url(client, project, service, &wrapped)?;
    let (mut ws, _) = tungstenite::connect(&url)?;
    set_read_timeout(&mut ws, Duration::from_millis(500));
    let start = std::time::Instant::now();
    let mut out = String::new();
    while start.elapsed() < cap {
        match ws.read() {
            Ok(Message::Text(t)) => {
                if let Some(o) = serde_json::from_str::<Value>(&t)
                    .ok()
                    .and_then(|v| v.get("output").and_then(Value::as_str).map(String::from))
                {
                    out.push_str(&o);
                    if out.contains(DONE_MARK) {
                        break;
                    }
                }
            }
            Ok(Message::Close(_)) => break,
            Ok(_) => {}
            Err(tungstenite::Error::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(tungstenite::Error::Io(e)) if e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(_) => break,
        }
    }
    let _ = ws.close(None);
    Ok(split_done_marker(&out))
}

/// Pull the exit status out of the sentinel line and return the output without it.
fn split_done_marker(raw: &str) -> Run {
    if let Some(i) = raw.find(DONE_MARK) {
        let after = &raw[i + DONE_MARK.len()..];
        let code = after
            .split("__")
            .next()
            .and_then(|s| s.trim().parse::<i32>().ok());
        // Everything up to the marker is the real output; drop the sentinel line.
        let mut output = raw[..i].to_string();
        while output.ends_with('\n') || output.ends_with('\r') {
            output.pop();
        }
        Run {
            exit_code: code,
            output,
        }
    } else {
        Run {
            exit_code: None,
            output: raw.to_string(),
        }
    }
}

/// Give a blocking WebSocket read a timeout, so a quiet socket doesn't hang the
/// caller forever. Both TLS and plain streams are handled.
pub(crate) fn set_read_timeout(
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn done_marker_yields_exit_code_and_clean_output() {
        let r = split_done_marker("some warning\n__EZP_DONE_0__\n");
        assert_eq!(r.exit_code, Some(0));
        assert_eq!(r.output, "some warning");

        let r = split_done_marker("curl: (22) 403\n__EZP_DONE_22__\n");
        assert_eq!(r.exit_code, Some(22));
        assert_eq!(r.output, "curl: (22) 403");

        // No marker (dropped connection / timeout) -> unknown status, raw output.
        let r = split_done_marker("half output");
        assert_eq!(r.exit_code, None);
        assert_eq!(r.output, "half output");
    }
}
