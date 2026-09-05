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

/// The SAME marker as it comes back in the shell's ECHO of our own input line: a
/// PTY reflects what we typed, so the `printf` placeholder is still unresolved
/// there. Finding it locates the END of the echo — and the echo is the one part
/// of this stream that carries credentials (the launch line contains
/// `MYSQL_PWD='…'` and a presigned URL), so everything up to it is dropped before
/// a launch failure is put into words.
const FIRED_ECHO: &str = "__EZP_FIRED_%";

/// How long a launch is given to confirm itself. A launch is `nohup … &` plus one
/// `printf` — seconds, not minutes. Named rather than inlined so the failure
/// message cannot drift from the wait it describes.
const FIRE_CAP: Duration = Duration::from_secs(30);

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
///
/// `redact` is the caller's own secret-masking closure, and it is here for one
/// reason: a launch failure has to be able to quote what the container shell
/// said, and that stream contains the shell's ECHO of `command` — which for a
/// dump/restore carries `MYSQL_PWD='…'` and a presigned URL. Only the caller
/// knows what those values are, so only the caller can mask them; passing the
/// closure it ALREADY builds for [`Run::output`] keeps one definition of "what is
/// secret here" instead of two that can drift apart. It is not applied to
/// `Run::output`: the caller does that itself, in the sentence it words around it.
// Every one of these is a distinct thing the caller alone knows (where to run, what
// to run, how long to allow, what to watch, what is secret, where to report); a
// params struct would be ceremony, the same call `s3::presign` makes.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_until_done(
    client: &EasypanelClient,
    project: &str,
    service: &str,
    command: &str,
    cap: Duration,
    watch: &[String],
    redact: &dyn Fn(&str) -> String,
    mut on_progress: impl FnMut(&Sizes),
) -> Result<Run> {
    let rid = chrono::Utc::now().format("%Y%m%d%H%M%S%6f").to_string();
    let done = format!("/tmp/ezp-run-{rid}.done");
    let log = format!("/tmp/ezp-run-{rid}.log");

    fire(
        client,
        project,
        service,
        &fire_line(command, &done, &log),
        redact,
    )?;

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

/// The longest input line the container shell accepts. MEASURED against a live
/// panel rather than assumed: a 4096-byte line (its trailing newline included) runs
/// and answers, 4097 is silently discarded and nothing ever comes back — the tty
/// line discipline's canonical buffer, which a `docker exec -it` shell sits behind.
///
/// A launch line longer than this can therefore only ever time out, so it is
/// refused HERE, where the reason can be named, instead of after a 30 s silence
/// that says nothing. For scale: a five-database dump with a presigned URL is
/// ~1.1 kB, so this is a ceiling on very large runs, not on ordinary ones.
const MAX_INPUT_LINE: usize = 4096;

/// Why a launch line cannot be delivered, or `None` when it fits.
///
/// Pure so the refusal can be tested without a panel, and separate from `fire` so
/// the number and the sentence that explains it live together.
fn too_long(line: &str) -> Option<String> {
    (line.len() > MAX_INPUT_LINE).then(|| {
        format!(
            "the command is too long to launch: {} bytes on one line, and the \
             container shell accepts at most {MAX_INPUT_LINE} — it would be \
             discarded unrun. Fewer databases in one run makes it shorter.",
            line.len()
        )
    })
}

/// What the socket DID while a launch was waited for, apart from the text it
/// carried. Kept as facts and not as a verdict: a socket that attached and then
/// said nothing at all is a different fault from a shell that answered and never
/// reached the marker, and the two send an operator to different places.
#[derive(Clone, Copy, Default)]
struct Heard {
    /// Any frame arrived at all — output, ping, or close.
    frames: bool,
    /// The session was closed before the marker: the exec itself ended.
    closed: bool,
}

/// How much of the shell's own words a launch failure carries: the last few
/// non-empty lines, capped. This lands in a one-line TUI status bar or on one line
/// of stderr, so a paragraph would push everything else off it — and a shell puts
/// what went wrong LAST.
const SAID_LINES: usize = 3;
const SAID_BYTES: usize = 240;

/// A cut-off echo has no trailing marker to find, so it is recognised the only
/// other honest way: by being a PREFIX of the line we sent. A handful of
/// coincidental characters is not an echo, hence a minimum — and below it nothing
/// is dropped, because a fragment the operator can read beats a blank.
const ECHO_PREFIX_MIN: usize = 16;

/// What the shell said, with the echo of our own input line dropped — and whether
/// there was an echo there to drop.
///
/// Dropping it is not cosmetic: the echo is the ONE part of this stream that can
/// carry credentials (`MYSQL_PWD='…'`, a presigned URL), and it can be spotted
/// exactly. The resolved [`FIRED`] marker can only come from the shell having run
/// the line, so the copy carrying the unresolved [`FIRED_ECHO`] is ours, and the
/// echo ends with it — everything up to the end of that line is ours to discard.
fn shell_said(raw: &str, line: &str) -> (String, bool) {
    let clean = raw.replace('\r', "");
    if let Some(at) = clean.rfind(FIRED_ECHO) {
        let end = clean[at..].find('\n').map_or(clean.len(), |i| at + i + 1);
        return (clean[end..].to_string(), true);
    }
    // No marker in the echo means the line did not arrive whole — the marker is the
    // last thing on it — so what came back is a plain prefix of what we sent.
    let shared = common_prefix(&clean, line);
    if shared >= ECHO_PREFIX_MIN {
        return (clean[shared..].to_string(), true);
    }
    (clean, false)
}

/// How many bytes two strings share from the start, never splitting a character.
fn common_prefix(a: &str, b: &str) -> usize {
    let mut n = a.bytes().zip(b.bytes()).take_while(|(x, y)| x == y).count();
    while n > 0 && !a.is_char_boundary(n) {
        n -= 1;
    }
    n
}

/// The tail of some shell output as ONE capped line, fit to sit in a status bar.
fn one_line(text: &str) -> String {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let said = lines[lines.len().saturating_sub(SAID_LINES)..].join(" | ");
    if said.len() <= SAID_BYTES {
        return said;
    }
    let mut end = SAID_BYTES;
    while end > 0 && !said.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &said[..end])
}

/// Why a launch could not be confirmed, in one line, in the shell's words and not
/// ours.
///
/// This message used to be the bare "could not launch the command in the
/// container" with the captured stream thrown away, so every cause reached the
/// operator identical and undiagnosable. The marker's `printf` sits after the `&`
/// in [`fire_line`], so it runs whatever the launched job does — which means its
/// ABSENCE is never about the dump itself. It is one of: the line never arrived or
/// was discarded (see [`MAX_INPUT_LINE`]), the shell is still waiting on an
/// unterminated quote and has executed nothing, the shell or the exec died before
/// parsing it, or nothing it printed reached us inside [`FIRE_CAP`]. Those look
/// very different in the stream, and this is what says which one it was.
///
/// The order here is load-bearing: `redact` runs BEFORE the length cap, because a
/// secret cut in half by the cap would no longer match the caller's replacement and
/// would leak its first half.
fn launch_failure(raw: &str, line: &str, heard: Heard, redact: &dyn Fn(&str) -> String) -> String {
    const HEAD: &str = "could not launch the command in the container";
    let secs = FIRE_CAP.as_secs();
    let (said, echoed) = shell_said(raw, line);
    let said = one_line(&redact(&said));
    if !said.is_empty() {
        return format!("{HEAD} — the shell said: {said}");
    }
    // Nothing to quote. Which KIND of nothing it was is the whole diagnosis, so
    // each is named separately rather than folded into one shrug.
    if !heard.frames {
        format!(
            "{HEAD} — the shell sent nothing back at all in {secs}s; check the service is running"
        )
    } else if heard.closed {
        format!("{HEAD} — the shell printed nothing and the session closed before it started")
    } else if echoed {
        format!("{HEAD} — the shell echoed the command but printed nothing else in {secs}s")
    } else {
        format!("{HEAD} — the shell printed nothing in {secs}s")
    }
}

/// Open a shell, send the detached-launch line, and wait only until the resolved
/// [`FIRED`] marker confirms it started (or [`FIRE_CAP`] — a launch takes seconds).
fn fire(
    client: &EasypanelClient,
    project: &str,
    service: &str,
    line: &str,
    redact: &dyn Fn(&str) -> String,
) -> Result<()> {
    if let Some(why) = too_long(line) {
        anyhow::bail!("{why}");
    }
    let url = ws_url(client, project, service, "sh")?;
    let (mut ws, _) = tungstenite::connect(&url)?;
    set_read_timeout(&mut ws, Duration::from_millis(500));
    ws.send(Message::Text(json!({ "input": line }).to_string()))
        .map_err(|e| anyhow!("failed to send command: {}", connect_failure(&e)))?;

    let start = std::time::Instant::now();
    let mut out = String::new();
    let mut fired = false;
    let mut heard = Heard::default();
    while start.elapsed() < FIRE_CAP {
        match ws.read() {
            Ok(Message::Text(t)) => {
                heard.frames = true;
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
            Ok(Message::Close(_)) => {
                heard.frames = true;
                heard.closed = true;
                break;
            }
            Ok(_) => heard.frames = true,
            Err(tungstenite::Error::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(tungstenite::Error::Io(e)) if e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(_) => break,
        }
    }
    let _ = ws.close(None);
    if fired {
        Ok(())
    } else {
        anyhow::bail!("{}", launch_failure(&out, line, heard, redact));
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

    /// A presigned PUT of the shape `s3::presign` produces, and the password that
    /// travels beside it: the two secrets a dump's launch line carries.
    const FAKE_URL: &str = "https://acct.r2.cloudflarestorage.com/backups/proj/mysql-20260906-051500.sql.gz?X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Credential=AKIAEXAMPLE%2F20260906%2Fauto%2Fs3%2Faws4_request&X-Amz-Date=20260906T051500Z&X-Amz-Expires=3600&X-Amz-SignedHeaders=host&X-Amz-Signature=1f2e3d4c5b6a7988990a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60";
    const FAKE_PW: &str = "s3cret";

    /// The launch line a one-database dump really sends, secrets and all.
    fn dump_launch_line() -> String {
        let cmd = crate::dump::dump_command(
            "mysql",
            FAKE_PW,
            &["shop".to_string()],
            "/tmp/ezp-dump-20260906-051500.sql.gz",
            FAKE_URL,
        )
        .expect("mysql is supported");
        fire_line(&cmd, "/tmp/ezp-run-1.done", "/tmp/ezp-run-1.log")
    }

    /// The caller's own masking, as `commands::dump_to_r2` builds it.
    fn caller_redact(s: &str) -> String {
        s.replace(FAKE_URL, "<presigned-url>")
            .replace(FAKE_PW, "<redacted>")
            .trim()
            .to_string()
    }

    fn heard() -> Heard {
        Heard {
            frames: true,
            closed: false,
        }
    }

    #[test]
    fn a_failed_launch_quotes_the_shell_and_never_the_credentials() {
        let line = dump_launch_line();
        // What a PTY really returns: our own line echoed back (CRLF, `%s` still
        // unresolved), then the shell's own words.
        let raw = format!("{}sh: 1: nohup: not found\r\n", line.replace('\n', "\r\n"));
        let msg = launch_failure(&raw, &line, heard(), &caller_redact);

        assert!(
            msg.contains("sh: 1: nohup: not found"),
            "the shell's complaint is the whole point: {msg}"
        );
        // The echo is where the secrets are, and it is gone — command and all.
        assert!(!msg.contains(FAKE_PW), "the root password leaked: {msg}");
        assert!(!msg.contains("MYSQL_PWD"), "the echoed line leaked: {msg}");
        assert!(
            !msg.contains("X-Amz-Signature") && !msg.contains("r2.cloudflarestorage.com"),
            "the presigned URL leaked: {msg}"
        );
        assert!(!msg.contains("mysqldump"), "our own command leaked: {msg}");
        assert!(
            !msg.contains('\n'),
            "this has to fit one status line: {msg}"
        );

        // And when the SHELL is the one repeating a secret — a `sh` that quotes the
        // offending word back, a `curl` that names the URL it rejected — dropping the
        // echo cannot help: only the caller's masking can, so it is applied to
        // whatever is left.
        let echoed_back = format!(
            "{}curl: (3) URL rejected: {FAKE_URL}\r\nsh: 1: MYSQL_PWD={FAKE_PW}: not found\r\n",
            line.replace('\n', "\r\n")
        );
        let msg = launch_failure(&echoed_back, &line, heard(), &caller_redact);
        assert!(msg.contains("curl: (3) URL rejected"), "{msg}");
        assert!(
            msg.contains("<presigned-url>"),
            "masked, not dropped: {msg}"
        );
        assert!(msg.contains("<redacted>"), "masked, not dropped: {msg}");
        assert!(!msg.contains(FAKE_PW), "the root password leaked: {msg}");
        assert!(
            !msg.contains("X-Amz-Signature"),
            "the presigned URL leaked: {msg}"
        );
    }

    #[test]
    fn only_the_last_few_lines_of_a_talkative_shell_are_quoted() {
        let line = dump_launch_line();
        let raw = format!(
            "{}one\r\ntwo\r\nthree\r\nfour\r\nfive\r\n",
            line.replace('\n', "\r\n")
        );
        let msg = launch_failure(&raw, &line, heard(), &caller_redact);
        assert!(msg.ends_with("three | four | five"), "{msg}");
        assert!(
            !msg.contains("one"),
            "an old banner is not the failure: {msg}"
        );

        // A single very long line is cut, and says it was.
        let long = format!("{}{}\r\n", line.replace('\n', "\r\n"), "e".repeat(600));
        let cut = launch_failure(&long, &line, heard(), &caller_redact);
        assert!(cut.ends_with('…'), "{cut}");
        assert!(
            cut.len() < 400,
            "still one status line: {} bytes",
            cut.len()
        );
    }

    #[test]
    fn a_launch_that_said_nothing_says_which_kind_of_nothing() {
        let line = dump_launch_line();
        let nop = |s: &str| s.trim().to_string();

        // Not one frame: the socket attached and the container never spoke.
        let silent = launch_failure("", &line, Heard::default(), &nop);
        assert!(silent.contains("sent nothing back at all"), "{silent}");

        // Frames arrived carrying nothing — not the same fault as silence.
        let quiet = launch_failure("", &line, heard(), &nop);
        assert!(quiet.contains("printed nothing in 30s"), "{quiet}");

        // The echo came back and nothing else: the shell HAS the line.
        let echoed = launch_failure(&line.replace('\n', "\r\n"), &line, heard(), &nop);
        assert!(
            echoed.contains("echoed the command but printed nothing else"),
            "{echoed}"
        );

        // The session ended before the marker could arrive.
        let closed = launch_failure(
            "",
            &line,
            Heard {
                frames: true,
                closed: true,
            },
            &nop,
        );
        assert!(closed.contains("session closed"), "{closed}");
    }

    #[test]
    fn an_echo_behind_a_prompt_is_dropped_by_its_unresolved_marker() {
        let line = dump_launch_line();
        // The echo is not always byte-identical to what we sent: a shell may print a
        // prompt first, and a PTY wraps a 1 kB line. Neither can be matched as a
        // prefix, so the unresolved marker at the END of the echo is what identifies
        // it. No redaction here either — the drop has to stand on its own.
        let (a, b) = line.trim_end().split_at(400);
        let raw = format!("# {a}\r\n{b}\r\nsh: 1: nohup: not found\r\n");
        let msg = launch_failure(&raw, &line, heard(), &|s: &str| s.trim().to_string());
        assert_eq!(
            msg,
            "could not launch the command in the container — the shell said: sh: 1: nohup: not found"
        );
    }

    #[test]
    fn an_echo_cut_short_is_still_recognised_by_its_prefix() {
        let line = dump_launch_line();
        // A line that never arrived whole has no trailing marker to find, so the
        // fragment is spotted by being a prefix of what we sent. Redaction is NOT
        // used here: this proves the structural drop on its own.
        let fragment = &line[..300];
        assert!(
            fragment.contains(FAKE_PW),
            "the fragment really does carry the password"
        );
        let raw = format!("{fragment}\r\nsh: syntax error: unterminated quoted string\r\n");
        let msg = launch_failure(&raw, &line, heard(), &|s: &str| s.trim().to_string());
        assert!(msg.contains("unterminated quoted string"), "{msg}");
        assert!(!msg.contains(FAKE_PW), "the fragment leaked: {msg}");
        assert!(!msg.contains("mysqldump"), "the fragment leaked: {msg}");
    }

    #[test]
    fn a_line_over_the_shells_input_limit_is_refused_before_it_is_sent() {
        assert_eq!(too_long(&"x".repeat(MAX_INPUT_LINE)), None, "4096 runs");
        let why = too_long(&"x".repeat(MAX_INPUT_LINE + 1)).expect("4097 cannot be delivered");
        assert!(why.contains("4097 bytes"), "names the size: {why}");
        assert!(why.contains("4096"), "names the limit: {why}");
    }

    #[test]
    fn a_realistic_multi_database_launch_line_fits_the_shells_input_limit() {
        // Five schemas plus a presigned URL — the shape of a real copy. ~1.1 kB
        // against a measured 4096-byte ceiling, so the headroom is real; this test
        // is what notices if the line ever grows towards it.
        // Five schema names and a URL of the lengths a real copy has (~90 bytes of
        // names, a ~400-byte presigned URL).
        let dbs: Vec<String> = [
            "acme_db_my_eu1",
            "acme_db_production_eu1",
            "acme_db_staging_eu1",
            "acme_studio_eu1",
            "acme_seating_eu1",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let cmd = crate::dump::dump_command(
            "mysql",
            "S0me-Long-Root-Password-42",
            &dbs,
            "/tmp/ezp-dump-20260906-051500.sql.gz",
            FAKE_URL,
        )
        .expect("mysql is supported");
        let line = fire_line(&cmd, "/tmp/ezp-run-1.done", "/tmp/ezp-run-1.log");
        assert_eq!(too_long(&line), None, "{} bytes", line.len());
        assert!(
            line.len() < MAX_INPUT_LINE / 2,
            "a five-database dump should sit far below the ceiling, not near it: {} bytes",
            line.len()
        );
    }

    /// A launch that CANNOT confirm, against a live container: the failure has to
    /// arrive carrying the shell's own words and none of ours. `&&` after a command
    /// that does not exist is the cheapest way to reach that state on purpose — the
    /// marker's `printf` never runs, which is exactly the shape of the real failure.
    ///
    /// Needs a running service, hence `#[ignore]`: run it with `--ignored`, and
    /// point it somewhere with `EZP_LIVE_PROJECT` / `EZP_LIVE_SERVICE`.
    #[test]
    #[ignore = "live: needs a running service on the default server"]
    fn a_live_launch_failure_reports_what_the_shell_said() {
        let cfg = crate::config::ServerConfig::new(crate::config::ServerConfig::default_path());
        let srv = cfg.default().expect("a default server exists");
        let client = crate::client::EasypanelClient::new(&srv.url, &srv.token);
        let project = std::env::var("EZP_LIVE_PROJECT").unwrap_or_else(|_| "zzz-emb".into());
        let service = std::env::var("EZP_LIVE_SERVICE").unwrap_or_else(|_| "zzz-redis".into());

        let line = "ezp_no_such_launcher_zzz sh -c 'MYSQL_PWD='\\''zzzFAKEzzz'\\'' true' \
                    && printf '__EZP_FIRED_%s__\\n' ok\n";
        let redact = |s: &str| s.replace("zzzFAKEzzz", "<redacted>").trim().to_string();
        let err =
            fire(&client, &project, &service, line, &redact).expect_err("the marker cannot arrive");
        let msg = err.to_string();
        println!("LIVE MESSAGE: {msg}");

        assert!(
            msg.starts_with("could not launch the command in the container — the shell said:"),
            "{msg}"
        );
        assert!(msg.contains("not found"), "the shell's own words: {msg}");
        assert!(!msg.contains("zzzFAKEzzz"), "a secret leaked: {msg}");
        assert!(!msg.contains("MYSQL_PWD"), "the echoed line leaked: {msg}");
        assert!(!msg.contains('\n'), "one status line: {msg}");
    }
}
