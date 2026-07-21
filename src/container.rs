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

/// The sentinel `run_until_done` prints to learn when the command finished and with
/// what status: `<PREFIX><exit code><SUFFIX>`. The command travels to the shell as
/// typed INPUT, which a PTY ECHOES — so the format string `printf '<PREFIX>%s<SUFFIX>'`
/// appears in the output too. We therefore wait for a marker whose middle is DIGITS
/// (the resolved `$?`); the echoed `%s` can never match, so the echo is ignored.
const DONE_PREFIX: &str = "__EZP_DONE_";
const DONE_SUFFIX: &str = "__";

/// The result of a long-running one-shot: the exit status (None if the marker never
/// arrived — a dropped connection or a timeout) and what the command printed.
pub(crate) struct Run {
    pub exit_code: Option<i32>,
    pub output: String,
}

/// Run ONE command that may take much longer than [`run_once`]'s 20 s cap — a
/// database dump gzipped and uploaded to object storage — and wait for it to finish.
///
/// The command is sent to a plain `sh` as **input over the WebSocket**, NOT baked
/// into the connection URL. Putting it in the URL is what [`run_once`] does, and it
/// works only for short commands: a multi-database dump command (four schema names
/// plus a ~380-char presigned URL) overran the URL, arrived truncated, and left the
/// shell hanging on an unterminated command — so the marker never came and the caller
/// waited out the whole cap ("did not report completion"). Input has no such limit.
///
/// A healthy dump is SILENT for its whole run, then prints the marker, so we read
/// until the marker (resolved digits), the socket closing, or `cap`.
pub(crate) fn run_until_done(
    client: &EasypanelClient,
    project: &str,
    service: &str,
    command: &str,
    cap: Duration,
) -> Result<Run> {
    // A short, fixed URL command — the real work travels as input below.
    let url = ws_url(client, project, service, "sh")?;
    let (mut ws, _) = tungstenite::connect(&url)?;
    set_read_timeout(&mut ws, Duration::from_millis(500));

    let line = format!("{command}; printf '{DONE_PREFIX}%s{DONE_SUFFIX}\\n' \"$?\"\n");
    ws.send(Message::Text(json!({ "input": line }).to_string()))
        .map_err(|e| anyhow!("failed to send command: {}", connect_failure(&e)))?;

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
                    if find_done(&out).is_some() {
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
    Ok(done_result(&out))
}

/// The exit code from the first RESOLVED marker (`<PREFIX><digits><SUFFIX>`) in the
/// output, or None if none has arrived. Skips the echoed `printf '<PREFIX>%s…'`,
/// whose middle is `%s`, not digits.
fn find_done(s: &str) -> Option<i32> {
    for begin in s
        .match_indices(DONE_PREFIX)
        .map(|(i, _)| i + DONE_PREFIX.len())
    {
        let rest = &s[begin..];
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if !digits.is_empty() && rest[digits.len()..].starts_with(DONE_SUFFIX) {
            return digits.parse().ok();
        }
    }
    None
}

/// Split the collected output at the resolved marker: the exit code, and everything
/// before it (which the caller redacts before showing — it includes the echoed
/// command, so the password and presigned URL are in it).
fn done_result(raw: &str) -> Run {
    match find_done(raw) {
        Some(code) => {
            let marker = format!("{DONE_PREFIX}{code}{DONE_SUFFIX}");
            let cut = raw.find(&marker).unwrap_or(raw.len());
            Run {
                exit_code: Some(code),
                output: raw[..cut].trim_end().to_string(),
            }
        }
        None => Run {
            exit_code: None,
            output: raw.to_string(),
        },
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
        let r = done_result("some warning\n__EZP_DONE_0__\n");
        assert_eq!(r.exit_code, Some(0));
        assert_eq!(r.output, "some warning");

        let r = done_result("curl: (22) 403\n__EZP_DONE_22__\n");
        assert_eq!(r.exit_code, Some(22));
        assert_eq!(r.output, "curl: (22) 403");

        // No marker (dropped connection / timeout) -> unknown status, raw output.
        let r = done_result("half output");
        assert_eq!(r.exit_code, None);
        assert_eq!(r.output, "half output");
    }

    #[test]
    fn the_echoed_printf_is_not_mistaken_for_the_marker() {
        // The PTY echoes the command we send, so its `printf '<PREFIX>%s<SUFFIX>'`
        // appears BEFORE the real output. Only the resolved (digits) marker counts.
        let echoed = "mysqldump …; printf '__EZP_DONE_%s__\\n' \"$?\"\n";
        let raw = format!("{echoed}some real output\n__EZP_DONE_0__\n");
        let r = done_result(&raw);
        assert_eq!(
            r.exit_code,
            Some(0),
            "must skip the echoed %s and find the 0"
        );
        assert!(r.output.contains("some real output"));
        assert!(
            !r.output.contains("__EZP_DONE_0__"),
            "the sentinel is trimmed off"
        );
        // Before the real marker arrives, the echo alone yields no completion.
        assert_eq!(find_done(echoed), None);
    }
}
