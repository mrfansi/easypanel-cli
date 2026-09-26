//! Reaching a panel through an SSH local forward.
//!
//! A server stored with `ssh` set is not dialled directly: its `url` is the
//! panel's address AS SEEN FROM THE SSH HOST (usually `http://localhost:3000`,
//! which EasyPanel publishes on the machine itself), and every request goes to a
//! local port that `ssh -N -L` forwards there. So a panel whose port 3000 is
//! firewalled off the internet is still reachable by anyone who can SSH in.
//!
//! The system `ssh` binary is used rather than an SSH library, deliberately: it
//! already knows the user's `~/.ssh/config` (aliases, `IdentityFile`, `ProxyJump`,
//! ports), their agent and their `known_hosts`. A library would have to
//! re-implement all of that and would still disagree with `ssh host` about which
//! key to offer.
//!
//! One tunnel per (SSH settings, remote) for the life of the process, opened on
//! first use and reopened if `ssh` has exited. Process-wide rather than owned by a
//! client, because a WebSocket session outlives the client that produced its URL:
//! the terminal pane keeps only the URL string, so a tunnel dropped with the
//! client would cut the shell the moment it opened.

use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{ErrorKind, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, LazyLock, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// How long the probe waits for the panel to answer through a fresh tunnel.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
/// How much of `ssh`'s stderr is kept for the error message.
const STDERR_KEEP: usize = 2048;

/// The URL a tunnelled panel is written as when none is given.
pub const DEFAULT_REMOTE_URL: &str = "http://localhost:3000";

struct Tunnel {
    child: Child,
    port: u16,
    stderr: Arc<Mutex<String>>,
    reader: Option<JoinHandle<()>>,
}

impl Tunnel {
    /// The last thing `ssh` said, for an error message ("" when it said nothing).
    fn reason(&mut self) -> String {
        // After an exit the reader ends at EOF, so wait for it to catch the last
        // line — briefly rather than join, since a ProxyCommand grandchild can
        // hold the pipe open. A live ssh has nothing more coming: don't wait.
        if !self.alive() {
            if let Some(h) = &self.reader {
                let until = Instant::now() + Duration::from_secs(1);
                while !h.is_finished() && Instant::now() < until {
                    thread::sleep(Duration::from_millis(20));
                }
            }
        }
        let err = self.stderr.lock().unwrap_or_else(PoisonError::into_inner);
        let tail = err
            .lines()
            .map(str::trim)
            .rfind(|l| !l.is_empty())
            .unwrap_or("");
        if tail.is_empty() {
            return String::new();
        }
        let hint = if tail.contains("Host key verification failed") {
            " — connect once with plain `ssh` to accept the host key"
        } else if tail.contains("Permission denied") {
            " — check the SSH user and the login method (agent / key / password)"
        } else {
            ""
        };
        format!(": {tail}{hint}")
    }

    fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

type Slot = Arc<Mutex<Option<Tunnel>>>;

static REGISTRY: LazyLock<Mutex<HashMap<String, Slot>>> = LazyLock::new(Default::default);

/// Is this usable as the host (or user) argument of `ssh`?
///
/// A leading `-` would be read as an OPTION — `-oProxyCommand=…` runs an
/// arbitrary local command — so it is refused outright rather than escaped.
pub fn valid_destination(dest: &str) -> bool {
    !dest.is_empty()
        && !dest.starts_with('-')
        && !dest.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// How `ssh` proves who you are.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SshAuth {
    /// Whatever plain `ssh` would do: the agent, `IdentityFile` from
    /// `~/.ssh/config`, the default keys.
    #[default]
    Agent,
    /// One private key file (`-i`), optionally with a passphrase.
    Key,
    /// A password.
    Password,
}

impl SshAuth {
    /// The names used on the command line, in servers.json and in the TUI form.
    pub const NAMES: [&'static str; 3] = ["agent", "key", "password"];

    pub fn as_str(self) -> &'static str {
        match self {
            SshAuth::Agent => "agent",
            SshAuth::Key => "key",
            SshAuth::Password => "password",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "agent" => Some(SshAuth::Agent),
            "key" => Some(SshAuth::Key),
            "password" => Some(SshAuth::Password),
            _ => None,
        }
    }
}

/// One SSH hop in front of a panel: the fields `ssh` itself would take.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SshTunnel {
    /// Hostname, IP, or a `Host` alias from `~/.ssh/config`.
    pub host: String,
    /// None = whatever `ssh` would use (22, or the alias's `Port`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// None = whatever `ssh` would use (the alias's `User`, or yours).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(default)]
    pub auth: SshAuth,
    /// The private key, for `auth = key`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_path: Option<String>,
    /// The key's passphrase (`key`) or the password (`password`). Stored like
    /// the API token next to it: servers.json is `0600`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
    /// Seconds `ssh` gets to connect. None = [`DEFAULT_TIMEOUT`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
}

/// Connect timeout when none is set.
pub const DEFAULT_TIMEOUT: u64 = 10;

impl SshTunnel {
    /// `user@host:port`, leaving out what was left to `ssh` to decide.
    pub fn describe(&self) -> String {
        let mut s = String::new();
        if let Some(u) = &self.user {
            s.push_str(u);
            s.push('@');
        }
        s.push_str(&self.host);
        if let Some(p) = self.port {
            s.push_str(&format!(":{p}"));
        }
        s
    }

    fn timeout(&self) -> u64 {
        self.timeout.unwrap_or(DEFAULT_TIMEOUT)
    }

    /// The key path with a leading `~/` expanded, so the check here and the file
    /// `ssh` opens are the same file.
    fn key_file(&self) -> Option<String> {
        let path = self.key_path.as_deref()?;
        Some(match path.strip_prefix("~/") {
            Some(rest) => match std::env::var("HOME") {
                Ok(home) => format!("{home}/{rest}"),
                Err(_) => path.to_string(),
            },
            None => path.to_string(),
        })
    }

    /// Check everything but the secret, which an edit may leave blank to keep.
    pub fn validate(&self) -> Result<()> {
        if !valid_destination(&self.host) {
            bail!("SSH host must be a hostname, an IP or a ~/.ssh/config alias (no spaces, no leading '-')");
        }
        if let Some(u) = &self.user {
            if !valid_destination(u) || u.contains('@') {
                bail!("SSH user must be a plain user name (no spaces, no '@', no leading '-')");
            }
        }
        if self.port == Some(0) {
            bail!("SSH port must be 1-65535");
        }
        if !(1..=300).contains(&self.timeout()) {
            bail!("SSH timeout must be 1-300 seconds");
        }
        if self.auth == SshAuth::Key {
            let Some(file) = self.key_file() else {
                bail!("Key login needs a key path");
            };
            if !std::path::Path::new(&file).is_file() {
                bail!("SSH key not found: {file}");
            }
        }
        Ok(())
    }

    /// Does `ssh` need a secret typed for it? Then it is asked through
    /// [`answer_askpass`] rather than a terminal the TUI owns.
    fn secret_to_type(&self) -> Result<Option<&str>> {
        match (self.auth, self.secret.as_deref()) {
            (SshAuth::Password, None | Some("")) => {
                bail!("SSH login to {} needs a password", self.describe())
            }
            (SshAuth::Agent, _) | (SshAuth::Key, None | Some("")) => Ok(None),
            (_, Some(s)) => Ok(Some(s)),
        }
    }

    /// What identifies one tunnel: everything that changes where or as whom
    /// `ssh` connects. The secret is not part of it.
    fn identity(&self, remote: &str) -> String {
        format!(
            "{}\n{:?}\n{:?}\n{}\n{:?}\n{remote}",
            self.host,
            self.port,
            self.user,
            self.auth.as_str(),
            self.key_path
        )
    }

    fn command(&self, local: u16, remote: &str) -> Result<Command> {
        let secret = self.secret_to_type()?;
        let mut cmd = Command::new("ssh");
        cmd.args(["-N", "-o", "ExitOnForwardFailure=yes"])
            .args([
                "-o",
                "ServerAliveInterval=15",
                "-o",
                "ServerAliveCountMax=3",
            ])
            .args(["-o", "LogLevel=ERROR"])
            .arg("-o")
            .arg(format!("ConnectTimeout={}", self.timeout()));
        match secret {
            // No prompts at all: the TUI owns the terminal, and a prompt would
            // scribble over it and wait forever for input it never gets.
            None => {
                cmd.args(["-o", "BatchMode=yes"]);
            }
            // The one prompt allowed goes to this binary as SSH_ASKPASS, never to
            // the terminal; one attempt, so a wrong secret fails instead of looping.
            Some(secret) => {
                let exe = std::env::current_exe()
                    .map_err(|e| anyhow!("cannot locate this program for SSH_ASKPASS: {e}"))?;
                cmd.args(["-o", "BatchMode=no", "-o", "NumberOfPasswordPrompts=1"])
                    .env("SSH_ASKPASS", exe)
                    .env("SSH_ASKPASS_REQUIRE", "force")
                    .env(ASKPASS_SECRET_ENV, secret);
            }
        }
        if let Some(p) = self.port {
            cmd.arg("-p").arg(p.to_string());
        }
        if let Some(u) = &self.user {
            cmd.arg("-l").arg(u);
        }
        match self.auth {
            SshAuth::Agent => {}
            SshAuth::Key => {
                let file = self.key_file().unwrap_or_default();
                cmd.arg("-i").arg(file).args(["-o", "IdentitiesOnly=yes"]);
            }
            SshAuth::Password => {
                cmd.args([
                    "-o",
                    "PreferredAuthentications=keyboard-interactive,password",
                    "-o",
                    "PubkeyAuthentication=no",
                ]);
            }
        }
        cmd.arg("-L")
            .arg(format!("127.0.0.1:{local}:{remote}"))
            .arg(&self.host)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        Ok(cmd)
    }
}

/// The env var carrying the secret to this binary when `ssh` runs it as askpass.
const ASKPASS_SECRET_ENV: &str = "EASYPANEL_SSH_ASKPASS_SECRET";

/// If `ssh` started this process as its askpass helper, answer and return true.
///
/// Must run before argument parsing: `ssh` calls the helper with its prompt as
/// the only argument, which is not a command this CLI knows.
pub fn answer_askpass() -> bool {
    let Some(secret) = std::env::var_os(ASKPASS_SECRET_ENV) else {
        return false;
    };
    let prompt = std::env::args().nth(1).unwrap_or_default();
    // An unknown host key is asked through the same helper. Answering it with
    // the password would be wrong in every way; refusing makes ssh fail with
    // "Host key verification failed", which carries its own hint.
    if prompt.contains("(yes/no") {
        println!("no");
    } else {
        println!("{}", secret.to_string_lossy());
    }
    true
}

/// Where a tunnelled `url` points on the SSH host: (`host:port`, path).
///
/// Only `http`: the request arrives at `127.0.0.1:<local port>`, so an https
/// certificate for the panel's name could never match — and EasyPanel's own
/// port 3000 is plain http on the host anyway.
pub fn forward_target(url: &str) -> Result<(String, String)> {
    let u = reqwest::Url::parse(url).map_err(|e| anyhow!("invalid URL '{url}': {e}"))?;
    if u.scheme() != "http" {
        bail!(
            "through an SSH tunnel the URL must be http:// as seen from the SSH host \
             (e.g. {DEFAULT_REMOTE_URL}), not {}://",
            u.scheme()
        );
    }
    let host = u
        .host_str()
        .ok_or_else(|| anyhow!("URL '{url}' has no host"))?;
    let port = u.port_or_known_default().unwrap_or(80);
    Ok((
        format!("{host}:{port}"),
        u.path().trim_end_matches('/').to_string(),
    ))
}

/// Check a server's tunnel settings before they are saved.
pub fn validate(tunnel: &SshTunnel, url: &str) -> Result<()> {
    tunnel.validate()?;
    forward_target(url).map(|_| ())
}

/// The base URL to send requests to for a panel behind `tunnel`, opening (or
/// reopening) the tunnel if needed.
pub fn base_url(tunnel: &SshTunnel, url: &str) -> Result<String> {
    let (remote, path) = forward_target(url)?;
    let port = local_port(tunnel, &remote)?;
    Ok(format!("http://127.0.0.1:{port}{path}"))
}

/// Open a throwaway tunnel and say what happened — the form's and `server
/// test`'s "Test SSH tunnel". Never touches the shared tunnels.
pub fn test(tunnel: &SshTunnel, url: &str) -> Result<String> {
    validate(tunnel, url)?;
    let (remote, _) = forward_target(url)?;
    let (_t, probe) = open(tunnel, &remote)?;
    let who = tunnel.describe();
    Ok(match probe {
        Probe::Answered => format!("SSH to {who} OK — the panel answered at {remote}"),
        _ => format!(
            "SSH to {who} OK, but nothing answered at {remote} within {}s",
            PROBE_TIMEOUT.as_secs()
        ),
    })
}

fn local_port(tunnel: &SshTunnel, remote: &str) -> Result<u16> {
    let slot = REGISTRY
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .entry(tunnel.identity(remote))
        .or_default()
        .clone();
    // Per-tunnel lock: two hosts open in parallel, two users of one host wait for
    // the same tunnel instead of each starting their own.
    let mut slot = slot.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(t) = slot.as_mut() {
        if t.alive() {
            return Ok(t.port);
        }
    }
    // Dead (the connection dropped, the laptop slept): reap it and dial again.
    *slot = None;
    let (t, _) = open(tunnel, remote)?;
    let port = t.port;
    *slot = Some(t);
    Ok(port)
}

fn open(tunnel: &SshTunnel, remote: &str) -> Result<(Tunnel, Probe)> {
    tunnel.validate()?;
    let who = tunnel.describe();
    // Bind-and-release to find a free port. ExitOnForwardFailure turns the rare
    // race (someone else takes it in between) into an error rather than a tunnel
    // that forwards nothing.
    let port = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?
        .local_addr()?
        .port();
    let mut child = tunnel
        .command(port, remote)?
        .spawn()
        .map_err(|e| anyhow!("cannot run ssh for the tunnel to {who}: {e}"))?;

    // Drained continuously: ssh logs every failed forward, and a full pipe would
    // block it — freezing every request behind the tunnel.
    let stderr = Arc::new(Mutex::new(String::new()));
    let reader = child.stderr.take().map(|mut pipe| {
        let sink = stderr.clone();
        thread::spawn(move || {
            let mut buf = [0u8; 1024];
            while let Ok(n @ 1..) = pipe.read(&mut buf) {
                let mut s = sink.lock().unwrap_or_else(PoisonError::into_inner);
                s.push_str(&String::from_utf8_lossy(&buf[..n]));
                if s.len() > STDERR_KEEP {
                    let mut cut = s.len() - STDERR_KEEP;
                    while !s.is_char_boundary(cut) {
                        cut += 1;
                    }
                    s.drain(..cut);
                }
            }
        })
    });
    let mut t = Tunnel {
        child,
        port,
        stderr,
        reader,
    };

    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    // Connecting is bounded by ConnectTimeout; authenticating on top of it is not,
    // so allow some more before calling it stuck.
    let limit = Duration::from_secs(tunnel.timeout() + 15);
    let deadline = Instant::now() + limit;
    loop {
        if let Ok(Some(status)) = t.child.try_wait() {
            bail!("SSH tunnel to {who} failed ({status}){}", t.reason());
        }
        // ssh only listens once it has authenticated, so a connect is the signal.
        if let Ok(stream) = TcpStream::connect_timeout(&addr, Duration::from_millis(200)) {
            return match probe(stream) {
                Probe::Closed => bail!(
                    "SSH to {who} works, but it could not reach the panel at {remote} from there{}",
                    t.reason()
                ),
                p => Ok((t, p)),
            };
        }
        if Instant::now() >= deadline {
            bail!(
                "SSH tunnel to {who} did not come up within {}s{}",
                limit.as_secs(),
                t.reason()
            );
        }
        thread::sleep(Duration::from_millis(100));
    }
}

enum Probe {
    /// The panel sent bytes back: the whole path works.
    Answered,
    /// ssh accepted, then hung up: it could not open the far side.
    Closed,
    /// Nothing within the timeout — left to the real request to report.
    Silent,
}

/// Ask the far side something, so a wrong remote port fails HERE with ssh's
/// reason, instead of later as an opaque "connection closed" from the client.
fn probe(mut stream: TcpStream) -> Probe {
    let _ = stream.set_read_timeout(Some(PROBE_TIMEOUT));
    if stream
        .write_all(b"HEAD / HTTP/1.0\r\nHost: localhost\r\n\r\n")
        .is_err()
    {
        return Probe::Closed;
    }
    let mut byte = [0u8; 1];
    match stream.read(&mut byte) {
        Ok(0) => Probe::Closed,
        Ok(_) => Probe::Answered,
        Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => Probe::Silent,
        Err(_) => Probe::Closed,
    }
}

/// Stop every tunnel. Called on the way out; a tunnel left behind would be an
/// `ssh` process holding a port open with no one to use it.
pub fn close_all() {
    let slots: Vec<Slot> = REGISTRY
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .drain()
        .map(|(_, s)| s)
        .collect();
    for slot in slots {
        slot.lock().unwrap_or_else(PoisonError::into_inner).take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tunnel(host: &str) -> SshTunnel {
        SshTunnel {
            host: host.into(),
            ..Default::default()
        }
    }

    #[test]
    fn a_host_or_user_that_ssh_would_read_as_an_option_is_refused() {
        assert!(tunnel("203.0.113.7").validate().is_ok());
        assert!(tunnel("viding-idc").validate().is_ok());
        assert!(tunnel("-oProxyCommand=touch /tmp/pwned")
            .validate()
            .is_err());
        assert!(tunnel("host -p 22").validate().is_err());
        assert!(tunnel("").validate().is_err());
        let user = |u: &str| SshTunnel {
            user: Some(u.into()),
            ..tunnel("h")
        };
        assert!(user("root").validate().is_ok());
        assert!(user("-oProxyCommand=id").validate().is_err());
        assert!(user("root@evil").validate().is_err());
        assert!(SshTunnel {
            port: Some(0),
            ..tunnel("h")
        }
        .validate()
        .is_err());
        assert!(SshTunnel {
            timeout: Some(0),
            ..tunnel("h")
        }
        .validate()
        .is_err());
    }

    #[test]
    fn key_login_needs_a_key_file_that_exists() {
        let dir = tempfile::tempdir().unwrap();
        let key = dir.path().join("id_test");
        let with = |p: Option<&std::path::Path>| SshTunnel {
            auth: SshAuth::Key,
            key_path: p.map(|p| p.display().to_string()),
            ..tunnel("h")
        };
        assert!(with(None).validate().is_err());
        assert!(
            with(Some(&key)).validate().is_err(),
            "missing file accepted"
        );
        std::fs::write(&key, "k").unwrap();
        assert!(with(Some(&key)).validate().is_ok());
    }

    #[test]
    fn a_secret_goes_to_askpass_and_never_onto_the_command_line() {
        let args = |t: &SshTunnel| -> (Vec<String>, bool) {
            let cmd = t.command(4000, "localhost:3000").unwrap();
            let args = cmd
                .get_args()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            let askpass = cmd
                .get_envs()
                .any(|(k, v)| k == ASKPASS_SECRET_ENV && v.is_some());
            (args, askpass)
        };
        // No secret: prompts are impossible, so nothing can land in the TUI.
        let (a, askpass) = args(&tunnel("h"));
        assert!(a.contains(&"BatchMode=yes".to_string()) && !askpass);

        let pw = SshTunnel {
            auth: SshAuth::Password,
            secret: Some("hunter2".into()),
            ..tunnel("h")
        };
        let (a, askpass) = args(&pw);
        assert!(askpass, "the password must reach ssh through askpass");
        // argv is readable by every user on the machine (`ps`); the env is not.
        assert!(!a.iter().any(|x| x.contains("hunter2")), "{a:?}");
        assert_eq!(a.last().map(String::as_str), Some("h"));

        // A password login without a password fails before ssh is started.
        let empty = SshTunnel { secret: None, ..pw };
        assert!(empty.command(4000, "localhost:3000").is_err());
    }

    #[test]
    fn the_forward_target_is_the_url_as_seen_from_the_ssh_host() {
        assert_eq!(
            forward_target("http://localhost:3000").unwrap(),
            ("localhost:3000".into(), String::new())
        );
        assert_eq!(
            forward_target("http://10.0.0.5/panel/").unwrap(),
            ("10.0.0.5:80".into(), "/panel".into())
        );
        assert_eq!(forward_target("http://[::1]:3000").unwrap().0, "[::1]:3000");
        // https can never verify against 127.0.0.1, so it is rejected up front.
        assert!(forward_target("https://panel.example.com").is_err());
    }
}
