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

/// The markers that FRAME a captured one-shot. Both are printed by `printf` with
/// a `%s` placeholder, so the RESOLVED form (`__EZP_OUT_ok__`,
/// `__EZP_RC_0__`) can only come from the command actually running: a PTY echoes
/// the input line back, and that echo carries the literal `%s`. Same
/// echo-discrimination the detached launcher relies on ([`FIRED`]).
const OUT_MARK: &str = "__EZP_OUT_ok__";
const RC_PREFIX: &str = "__EZP_RC_";

/// How much captured text is kept. A `SELECT *` on a large table would otherwise
/// stream until the cap elapsed and hold all of it in memory; past this the read
/// stops and the result says it was cut, which is the honest thing to show above
/// a grid.
const MAX_CAPTURE_BYTES: usize = 256 * 1024;

/// What a framed one-shot printed, whether it succeeded, and whether it fitted.
///
/// Kept apart from [`Run`]: that one describes a DETACHED job (its `output` is
/// "what it printed WHEN IT FAILED", and it has no notion of a result being cut
/// short), while this is the whole point here — a query's rows, its exit status,
/// and whether there were more of them than we were willing to hold.
pub(crate) struct Capture {
    /// Everything the command printed between the two markers, `\r` stripped.
    pub output: String,
    /// The command's exit status. `None` = the closing marker never arrived
    /// within `cap`, so the command may still be running and `output` may be a
    /// fragment — NOT the same thing as "it finished and printed nothing".
    pub exit_code: Option<i32>,
    /// The read stopped at [`MAX_CAPTURE_BYTES`]; there was more.
    pub truncated: bool,
}

/// The input line that runs `command` between two resolved markers and reports
/// its exit status.
///
/// `command` is NOT re-quoted: it is sent as a shell line exactly as built, the
/// same contract [`run_once`] has. The markers each start on a fresh line, so
/// neither can be split by the PTY wrapping a long line of output.
fn capture_line(command: &str) -> String {
    format!("printf '\\n__EZP_OUT_%s__\\n' ok ; {command} ; printf '\\n__EZP_RC_%s__\\n' $?\n")
}

/// The exit code carried by the closing marker, and where in the text it starts.
///
/// Only a marker with DIGITS counts: `__EZP_RC_%s__` is the shell's echo of what
/// we typed, `__EZP_RC_0__` is the shell answering.
fn find_rc(text: &str) -> Option<(i32, usize)> {
    let mut from = 0;
    while let Some(rel) = text[from..].find(RC_PREFIX) {
        let at = from + rel;
        let rest = &text[at + RC_PREFIX.len()..];
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if !digits.is_empty() && rest[digits.len()..].starts_with("__") {
            if let Ok(code) = digits.parse::<i32>() {
                return Some((code, at));
            }
        }
        from = at + RC_PREFIX.len();
    }
    None
}

/// Split a raw captured stream into what the command printed and how it ended.
///
/// The opening marker is what drops the shell's echo of our own input line: the
/// echo cannot contain the resolved marker, so everything up to and including it
/// is noise (the echo, any login banner) and everything after it is output. With
/// no opening marker at all (a shell that never ran the line) nothing is thrown
/// away — a fragment the caller can read beats a blank.
fn parse_capture(raw: &str, truncated: bool) -> Capture {
    let clean = raw.replace('\r', "");
    let (exit_code, end) = match find_rc(&clean) {
        Some((code, at)) => (Some(code), at),
        None => (None, clean.len()),
    };
    let body = &clean[..end];
    let payload = match body.find(OUT_MARK) {
        Some(i) => &body[i + OUT_MARK.len()..],
        None => body,
    };
    Capture {
        output: payload.trim_matches('\n').to_string(),
        exit_code,
        truncated,
    }
}

/// Run ONE command and capture what it printed, framed by markers so the result
/// is COMPLETE rather than merely quiet.
///
/// [`run_once`] stops after 1.2 s with nothing new, which is right for a poll but
/// wrong for a query: a statement that thinks for two seconds before printing
/// anything would come back empty, indistinguishable from a result set with no
/// rows. Here the read continues until the closing marker arrives (the command
/// has exited), the socket closes, `cap` elapses, or the byte budget is spent —
/// and each of those is reported for what it is.
pub(crate) fn run_capture(
    client: &EasypanelClient,
    project: &str,
    service: &str,
    command: &str,
    cap: Duration,
) -> Result<Capture> {
    let url = ws_url(client, project, service, "sh")?;
    let (mut ws, _) = tungstenite::connect(&url)?;
    set_read_timeout(&mut ws, Duration::from_millis(200));
    ws.send(Message::Text(
        json!({ "input": capture_line(command) }).to_string(),
    ))
    .map_err(|e| anyhow!("failed to send command: {}", connect_failure(&e)))?;

    let start = std::time::Instant::now();
    let mut out = String::new();
    let mut truncated = false;
    while start.elapsed() < cap {
        match ws.read() {
            Ok(Message::Text(t)) => {
                if let Some(o) = serde_json::from_str::<Value>(&t)
                    .ok()
                    .and_then(|v| v.get("output").and_then(Value::as_str).map(String::from))
                {
                    out.push_str(&o);
                    if find_rc(&out).is_some() {
                        break;
                    }
                    if out.len() > MAX_CAPTURE_BYTES {
                        truncated = true;
                        break;
                    }
                }
            }
            Ok(Message::Close(_)) => break,
            Ok(_) => {}
            // A read timeout is the normal quiet case, not a failure: the command
            // is still working.
            Err(tungstenite::Error::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(tungstenite::Error::Io(e)) if e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(_) => break,
        }
    }
    let _ = ws.close(None);
    Ok(parse_capture(&out, truncated))
}

/// A resolved marker (`printf '…%s…' ok`) that appears ONLY in the shell's output,
/// never in the echoed input line (a PTY echoes what we type, and the echo carries
/// the literal `%s`, not `ok`) — so seeing it means the launch actually ran.
const FIRED: &str = "__EZP_FIRED_ok__";

/// The result of a long-running one-shot: the exit status (None if the sentinel
/// never appeared within `cap` — the job is likely still running) and, on failure,
/// what the command printed.
pub(crate) struct Run {
    pub exit_code: Option<i32>,
    pub output: String,
}

/// The input line that launches `command` DETACHED. A DB dump or restore can run
/// far longer than one WebSocket can be trusted to stay open, and the job keeps
/// running in the container after our socket closes — so we do not hold a socket
/// open waiting for it. Instead we fire it under `nohup` (so closing our socket
/// can't SIGHUP it), redirect its output to `log`, record its exit status in
/// `done`, and confirm the launch with a resolved marker. `command` is embedded in
/// a single-quoted `sh -c`, so every `'` in it is escaped `'\''`.
fn fire_line(command: &str, done: &str, log: &str) -> String {
    let inner = format!("{command} ; echo $? > '{done}'");
    let esc = inner.replace('\'', "'\\''");
    format!("nohup sh -c '{esc}' > '{log}' 2>&1 & printf '__EZP_FIRED_%s__\\n' ok\n")
}

/// The exit code written to the sentinel file, or None if it isn't there yet. The
/// file holds just the number; we take the last line that is purely an integer, so
/// a stray prompt or blank line can't fool us.
fn parse_done(cat_output: &str) -> Option<i32> {
    cat_output
        .lines()
        .rev()
        .find_map(|l| l.trim().parse::<i32>().ok())
}

/// The bytes each watched file holds right now — only the ones that exist. A
/// detached job says nothing while it runs, so the size of what it is writing is
/// the only honest progress signal there is.
pub(crate) type Sizes = Vec<(String, u64)>;

/// The `wc -c` lines of a poll, as `(path, bytes)` for the watched files only —
/// which also drops `wc`'s own `total` line, since "total" is never a watched path.
fn parse_sizes(out: &str, watch: &[String]) -> Sizes {
    out.lines()
        .filter_map(|l| {
            let mut f = l.split_whitespace();
            let bytes = f.next()?.parse::<u64>().ok()?;
            let path = f.next()?;
            watch
                .iter()
                .find(|w| *w == path)
                .map(|w| (w.clone(), bytes))
        })
        .collect()
}

/// Run ONE command that may take much longer than [`run_once`]'s 20 s cap — a
/// database dump/restore gzipped through object storage — WITHOUT holding a single
/// socket open for its whole duration.
///
/// FIRE the command detached (it writes its exit code to a sentinel file when it
/// finishes), then POLL that file over short, fresh connections until it appears or
/// `cap` elapses. A socket that drops mid-run no longer aborts anything — the next
/// poll just reconnects. Returns `exit_code: None` only if the sentinel never shows
/// within `cap`, in which case the job is most likely still running.
///
/// Each poll also sizes the `watch` files in the SAME exec (the sentinel `cat` was
/// a round trip already; `wc -c` rides along for free) and hands them to
/// `on_progress`, so a caller can say how far a 25 GB dump has got instead of
/// spinning silently for twenty minutes. Pass `&[]` to want none of it.
pub(crate) fn run_until_done(
    client: &EasypanelClient,
    project: &str,
    service: &str,
    command: &str,
    cap: Duration,
    watch: &[String],
    mut on_progress: impl FnMut(&Sizes),
) -> Result<Run> {
    let rid = chrono::Utc::now().format("%Y%m%d%H%M%S%6f").to_string();
    let done = format!("/tmp/ezp-run-{rid}.done");
    let log = format!("/tmp/ezp-run-{rid}.log");

    fire(client, project, service, &fire_line(command, &done, &log))?;

    let sizes = if watch.is_empty() {
        String::new()
    } else {
        let files = watch
            .iter()
            .map(|f| format!("'{f}'"))
            .collect::<Vec<_>>()
            .join(" ");
        format!("; wc -c {files} 2>/dev/null")
    };
    let poll = format!("cat '{done}' 2>/dev/null{sizes}");

    let start = std::time::Instant::now();
    loop {
        let seen = run_once(client, project, service, &poll)?;
        if let Some(code) = parse_done(&seen) {
            let output = run_once(
                client,
                project,
                service,
                &format!("cat '{log}' 2>/dev/null"),
            )
            .unwrap_or_default();
            let _ = run_once(client, project, service, &format!("rm -f '{done}' '{log}'"));
            return Ok(Run {
                exit_code: Some(code),
                output: output.trim().to_string(),
            });
        }
        if !watch.is_empty() {
            on_progress(&parse_sizes(&seen, watch));
        }
        if start.elapsed() >= cap {
            // Sentinel never showed: the job is most likely still running. Leave the
            // files be and let the caller warn against a blind re-run.
            return Ok(Run {
                exit_code: None,
                output: String::new(),
            });
        }
        std::thread::sleep(Duration::from_secs(5));
    }
}

/// Open a shell, send the detached-launch line, and wait only until the resolved
/// [`FIRED`] marker confirms it started (or a short cap — a launch takes seconds).
fn fire(client: &EasypanelClient, project: &str, service: &str, line: &str) -> Result<()> {
    let url = ws_url(client, project, service, "sh")?;
    let (mut ws, _) = tungstenite::connect(&url)?;
    set_read_timeout(&mut ws, Duration::from_millis(500));
    ws.send(Message::Text(json!({ "input": line }).to_string()))
        .map_err(|e| anyhow!("failed to send command: {}", connect_failure(&e)))?;

    let start = std::time::Instant::now();
    let mut out = String::new();
    let mut fired = false;
    while start.elapsed() < Duration::from_secs(30) {
        match ws.read() {
            Ok(Message::Text(t)) => {
                if let Some(o) = serde_json::from_str::<Value>(&t)
                    .ok()
                    .and_then(|v| v.get("output").and_then(Value::as_str).map(String::from))
                {
                    out.push_str(&o);
                    if out.contains(FIRED) {
                        fired = true;
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
    if fired {
        Ok(())
    } else {
        anyhow::bail!("could not launch the command in the container")
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
    fn parse_done_reads_the_exit_code_or_nothing_yet() {
        assert_eq!(parse_done("0\n"), Some(0));
        assert_eq!(parse_done("22\n"), Some(22));
        // Sentinel not written yet -> still running.
        assert_eq!(parse_done(""), None);
        assert_eq!(parse_done("\n"), None);
        // A stray prompt line before the number must not fool it.
        assert_eq!(parse_done("$ \n3\n"), Some(3));
    }

    #[test]
    fn poll_sizes_only_the_watched_files() {
        let watch = vec!["/tmp/d.sql".to_string(), "/tmp/d.sql.gz".to_string()];
        let out = "0\n 25298331739 /tmp/d.sql\n  3084386304 /tmp/d.sql.gz\n 28382718043 total\n";
        assert_eq!(
            parse_sizes(out, &watch),
            vec![
                ("/tmp/d.sql".to_string(), 25298331739),
                ("/tmp/d.sql.gz".to_string(), 3084386304),
            ],
            "wc's own 'total' line is not a watched path"
        );
        // A file gzip has already removed simply isn't there — that absence is what
        // tells the caller which phase the job is in.
        assert_eq!(
            parse_sizes(" 3901300000 /tmp/d.sql.gz\n", &watch),
            vec![("/tmp/d.sql.gz".to_string(), 3901300000)]
        );
        assert!(parse_sizes("", &watch).is_empty());
        // The sentinel's exit code shares the output and must not read as a size.
        assert!(parse_sizes("0\n", &watch).is_empty());
    }

    #[test]
    fn fire_line_detaches_escapes_and_confirms() {
        // Command carries its own single quotes (password/URL do in real use).
        let line = fire_line("mysql -uroot < '/tmp/x.sql'", "/tmp/r.done", "/tmp/r.log");
        assert!(line.starts_with("nohup sh -c '"), "detached under nohup");
        assert!(
            line.contains("> '/tmp/r.log' 2>&1 &"),
            "output to log, backgrounded"
        );
        assert!(
            line.contains("printf '__EZP_FIRED_%s__"),
            "confirms launch with a resolved marker (echo carries %s, not the token)"
        );
        // Every single quote from the command is escaped for the outer sh -c '...'.
        assert!(line.contains(r"'\''/tmp/x.sql'\''"), "inner quotes escaped");
        assert!(
            line.contains(r"echo $? > '\''/tmp/r.done'\''"),
            "captures the command's exit code into the sentinel"
        );
    }

    #[test]
    fn a_capture_drops_the_shells_echo_and_reads_the_exit_code() {
        // What a PTY really returns: our input line echoed back (carrying the
        // literal %s), then the resolved opening marker, the rows, the resolved
        // closing marker.
        let raw = concat!(
            "printf '\\n__EZP_OUT_%s__\\n' ok ; mysql -e 'SELECT 1' ; printf '\\n__EZP_RC_%s__\\n' $?\r\n",
            "\r\n__EZP_OUT_ok__\r\n",
            "1\t2\r\n",
            "\r\n__EZP_RC_0__\r\n"
        );
        let cap = parse_capture(raw, false);
        assert_eq!(cap.output, "1\t2", "only what the command printed");
        assert_eq!(cap.exit_code, Some(0));
        assert!(!cap.truncated);

        // A failing command: its message is the payload, its status is carried.
        let failed = parse_capture(
            "\n__EZP_OUT_ok__\nERROR 1064 (42000): bad syntax\n\n__EZP_RC_1__\n",
            false,
        );
        assert_eq!(failed.output, "ERROR 1064 (42000): bad syntax");
        assert_eq!(failed.exit_code, Some(1));
    }

    #[test]
    fn no_closing_marker_means_incomplete_not_empty() {
        // The echo alone must not be mistaken for a finished run: it carries
        // `%s`, never a number, so there is no exit code to read.
        let echo_only =
            "printf '\\n__EZP_OUT_%s__\\n' ok ; sleep 60 ; printf '\\n__EZP_RC_%s__\\n' $?\n";
        let cap = parse_capture(echo_only, false);
        assert_eq!(cap.exit_code, None, "an echoed marker is not an answer");

        // Output started arriving but the command had not finished: the rows are
        // kept AND the caller is told the status is unknown.
        let partial = parse_capture("\n__EZP_OUT_ok__\nid\tname\n1\tAda\n", false);
        assert_eq!(partial.output, "id\tname\n1\tAda");
        assert_eq!(partial.exit_code, None);

        // Truncation is carried through, so a cut result can say so.
        assert!(parse_capture("\n__EZP_OUT_ok__\nx\n", true).truncated);
    }

    #[test]
    fn capture_line_frames_the_command_with_resolved_markers() {
        let line = capture_line("mysql -e 'SELECT 1'");
        assert!(
            line.starts_with("printf '\\n__EZP_OUT_%s__\\n' ok ; "),
            "{line}"
        );
        assert!(
            line.trim_end().ends_with("printf '\\n__EZP_RC_%s__\\n' $?"),
            "the status is reported by the shell, not guessed: {line}"
        );
        // The command is passed through untouched — it is already a shell line.
        assert!(line.contains("mysql -e 'SELECT 1'"), "{line}");
        assert!(line.ends_with('\n'), "the line must be submitted");
    }
}
