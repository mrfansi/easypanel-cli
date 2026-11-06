//! Platform-neutral EasyPanel interactive WebSocket transport.

use crate::container::{connect_failure, set_read_timeout};
use serde_json::{json, Value};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Duration;
use std::{
    collections::VecDeque,
    net::{TcpStream, ToSocketAddrs},
    time::Instant,
};
use tungstenite::Message;
use tungstenite::{client::IntoClientRequest, stream::MaybeTlsStream, HandshakeError};

type Socket = tungstenite::WebSocket<MaybeTlsStream<TcpStream>>;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const CONNECT_POLL: Duration = Duration::from_millis(20);
const TCP_CONNECT_SLICE: Duration = Duration::from_secs(1);

pub(crate) enum TermMsg {
    Input(String),
    Resize(u16, u16),
}

pub(crate) enum SessionEvent {
    Output(Vec<u8>),
    Error(String),
    Closed,
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
pub(crate) fn db_command(stype: &str, inspect: &Value) -> Option<String> {
    let f = |k: &str| inspect.get(k).and_then(Value::as_str).unwrap_or("");
    let q = crate::dbms::sh_quote;
    let db = f("databaseName");
    match stype {
        "mysql" | "mariadb" => {
            let d = if db.is_empty() {
                String::new()
            } else {
                format!(" --database={}", q(db))
            };
            Some(format!(
                "MYSQL_PWD={} mysql -uroot{d}",
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
                format!(" -d {}", q(db))
            };
            Some(format!(
                "PGPASSWORD={} psql -U {}{d}",
                q(f("password")),
                q(user)
            ))
        }
        "mongo" => Some(format!(
            "mongosh -u {} -p {} --authenticationDatabase admin",
            q(f("user")),
            q(f("password"))
        )),
        "redis" => Some(format!("REDISCLI_AUTH={} redis-cli", q(f("password")))),
        _ => None,
    }
}

/// Save early keys/resizes while checking whether the owning document still exists.
fn connecting_input(
    input: &Receiver<TermMsg>,
    pending: &mut VecDeque<TermMsg>,
    size: &mut (u16, u16),
) -> bool {
    loop {
        match input.try_recv() {
            Ok(TermMsg::Resize(cols, rows)) => *size = (cols.max(1), rows.max(1)),
            Ok(message) => pending.push_back(message),
            Err(TryRecvError::Empty) => return true,
            Err(TryRecvError::Disconnected) => return false,
        }
    }
}

/// Nonblocking TLS + HTTP upgrade keeps its handshake state across WouldBlock.
/// SSH/container lookup has already completed; this deadline does not shorten it.
fn connect_session(
    url: &str,
    input: &Receiver<TermMsg>,
    pending: &mut VecDeque<TermMsg>,
    size: &mut (u16, u16),
    timeout: Duration,
) -> Result<Option<Socket>, Box<tungstenite::Error>> {
    let deadline = Instant::now() + timeout;
    let timed_out = || {
        Box::new(tungstenite::Error::Io(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "Timed out connecting to the panel terminal",
        )))
    };
    let mut request = url.into_client_request()?;
    'redirect: for redirect in 0..=3 {
        if !connecting_input(input, pending, size) {
            return Ok(None);
        }
        if Instant::now() >= deadline {
            return Err(timed_out());
        }
        let mode = tungstenite::client::uri_mode(request.uri())?;
        let host = request.uri().host().ok_or(tungstenite::Error::Url(
            tungstenite::error::UrlError::NoHostName,
        ))?;
        let host = host
            .strip_prefix('[')
            .and_then(|host| host.strip_suffix(']'))
            .unwrap_or(host)
            .to_owned();
        let port = request.uri().port_u16().unwrap_or(match mode {
            tungstenite::stream::Mode::Plain => 80,
            tungstenite::stream::Mode::Tls => 443,
        });
        let addresses = if let Ok(ip) = host.parse::<std::net::IpAddr>() {
            vec![std::net::SocketAddr::new(ip, port)]
        } else {
            // A cancelled resolver never starts TCP. std's resolver itself cannot
            // be interrupted; its bounded result channel is disposable.
            let (resolved_tx, resolved_rx) = std::sync::mpsc::sync_channel(1);
            std::thread::spawn(move || {
                let addresses = (host.as_str(), port)
                    .to_socket_addrs()
                    .map(|addresses| addresses.collect::<Vec<_>>());
                let _ = resolved_tx.send(addresses);
            });
            loop {
                if !connecting_input(input, pending, size) {
                    return Ok(None);
                }
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(timed_out());
                }
                match resolved_rx.recv_timeout(CONNECT_POLL.min(remaining)) {
                    Ok(addresses) => break addresses.map_err(tungstenite::Error::from)?,
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                        return Err(tungstenite::Error::Io(std::io::Error::other(
                            "Terminal address resolution stopped",
                        ))
                        .into())
                    }
                }
            }
        };
        let mut stream = None;
        let mut last_error = std::io::Error::new(
            std::io::ErrorKind::AddrNotAvailable,
            "No panel address was resolved",
        );
        for address in addresses {
            if !connecting_input(input, pending, size) {
                return Ok(None);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(timed_out());
            }
            match TcpStream::connect_timeout(&address, TCP_CONNECT_SLICE.min(remaining)) {
                Ok(tcp) => {
                    stream = Some(tcp);
                    break;
                }
                Err(error) => last_error = error,
            }
        }
        if !connecting_input(input, pending, size) {
            return Ok(None);
        }
        let stream = stream.ok_or(tungstenite::Error::Io(last_error))?;
        stream.set_nodelay(true).map_err(tungstenite::Error::from)?;
        stream
            .set_nonblocking(true)
            .map_err(tungstenite::Error::from)?;
        let mut handshake = tungstenite::client_tls(request, stream);
        loop {
            // Dropping either MidHandshake or the ready socket closes this
            // session's TCP/TLS stream, including a peer withholding its upgrade.
            if !connecting_input(input, pending, size) {
                return Ok(None);
            }
            if Instant::now() >= deadline {
                return Err(timed_out());
            }
            match handshake {
                Ok((ws, _)) => {
                    let tcp = match ws.get_ref() {
                        MaybeTlsStream::Plain(tcp) => tcp,
                        MaybeTlsStream::Rustls(tls) => tls.get_ref(),
                        _ => {
                            return Err(tungstenite::Error::Io(std::io::Error::other(
                                "Unsupported terminal TLS stream",
                            ))
                            .into())
                        }
                    };
                    tcp.set_nonblocking(false)
                        .map_err(tungstenite::Error::from)?;
                    if !connecting_input(input, pending, size) {
                        return Ok(None);
                    }
                    return Ok(Some(ws));
                }
                Err(HandshakeError::Interrupted(mid)) => {
                    std::thread::sleep(
                        CONNECT_POLL.min(deadline.saturating_duration_since(Instant::now())),
                    );
                    handshake = mid.handshake();
                }
                Err(HandshakeError::Failure(tungstenite::Error::Http(response)))
                    if response.status().is_redirection() && redirect < 3 =>
                {
                    if let Some(location) =
                        response.headers().get(tungstenite::http::header::LOCATION)
                    {
                        request = location
                            .to_str()
                            .map_err(tungstenite::Error::from)?
                            .into_client_request()?;
                        continue 'redirect;
                    }
                    return Err(tungstenite::Error::Http(response).into());
                }
                Err(HandshakeError::Failure(error)) => return Err(error.into()),
            }
        }
    }
    unreachable!("every final redirect returns a socket or error")
}

/// One socket per session; dropping the input sender closes the remote PTY.
/// Initial resize waits for output because EasyPanel drops it during setup.
pub(crate) fn spawn_session(
    url: String,
    emit: impl Fn(SessionEvent) + Send + 'static,
    input_rx: Receiver<TermMsg>,
    cols: u16,
    rows: u16,
    auth_hint: &'static str,
) {
    std::thread::spawn(move || {
        let mut pending = VecDeque::new();
        let mut size = (cols.max(1), rows.max(1));
        let mut ws =
            match connect_session(&url, &input_rx, &mut pending, &mut size, CONNECT_TIMEOUT) {
                Ok(Some(ws)) => ws,
                Ok(None) => {
                    emit(SessionEvent::Closed);
                    return;
                }
                Err(e) => {
                    let hint = if crate::container::is_auth_rejection(&e) {
                        auth_hint
                    } else {
                        ""
                    };
                    emit(SessionEvent::Error(format!(
                        "Terminal failed to connect: {}{hint}",
                        connect_failure(&e)
                    )));
                    emit(SessionEvent::Closed);
                    return;
                }
            };
        set_read_timeout(&mut ws, Duration::from_millis(15));
        let mut sized = false;
        let result: Result<(), Box<tungstenite::Error>> = (|| {
            loop {
                // Bound output draining so a busy process cannot starve input/close.
                for _ in 0..128 {
                    match ws.read() {
                        Ok(Message::Text(t)) => {
                            if let Some(out) =
                                serde_json::from_str::<Value>(&t).ok().and_then(|v| {
                                    v.get("output").and_then(Value::as_str).map(str::to_owned)
                                })
                            {
                                emit(SessionEvent::Output(out.into_bytes()));
                                if !sized {
                                    ws.send(resize_msg(size.0, size.1))?;
                                    sized = true;
                                }
                            }
                        }
                        Ok(Message::Close(_)) => return Ok(()),
                        Ok(_) => {}
                        Err(tungstenite::Error::Io(e))
                            if matches!(
                                e.kind(),
                                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                            ) =>
                        {
                            break
                        }
                        Err(
                            tungstenite::Error::ConnectionClosed
                            | tungstenite::Error::AlreadyClosed,
                        ) => return Ok(()),
                        Err(e) => return Err(e.into()),
                    }
                }
                loop {
                    match pending.pop_front().map_or_else(|| input_rx.try_recv(), Ok) {
                        Ok(TermMsg::Input(text)) => {
                            // Preserve the TUI's first-key readiness resize.
                            if !sized {
                                ws.send(resize_msg(size.0, size.1))?;
                                sized = true;
                            }
                            ws.send(Message::Text(json!({ "input": text }).to_string()))?;
                        }
                        Ok(TermMsg::Resize(cols, rows)) => {
                            size = (cols.max(1), rows.max(1));
                            if sized {
                                ws.send(resize_msg(size.0, size.1))?;
                            }
                        }
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => {
                            let _ = ws.close(None);
                            return Ok(());
                        }
                    }
                }
                std::thread::sleep(Duration::from_millis(4));
            }
        })();
        if let Err(e) = result {
            emit(SessionEvent::Error(format!(
                "Terminal connection lost: {}",
                connect_failure(&e)
            )));
        }
        let _ = ws.close(None);
        emit(SessionEvent::Closed);
    });
}

fn resize_msg(cols: u16, rows: u16) -> Message {
    Message::Text(json!({ "resize": [cols.max(1), rows.max(1)] }).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    #[test]
    fn socket_roundtrip_delays_initial_resize_and_closes_remote_session() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (observed_tx, observed_rx) = channel();
        let server = std::thread::spawn(move || {
            let (tcp, _) = listener.accept().unwrap();
            tcp.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            let mut ws = tungstenite::accept(tcp).unwrap();
            ws.get_ref()
                .set_read_timeout(Some(Duration::from_millis(60)))
                .unwrap();
            assert!(
                matches!(ws.read(),Err(tungstenite::Error::Io(error)) if matches!(error.kind(),std::io::ErrorKind::WouldBlock|std::io::ErrorKind::TimedOut))
            );
            ws.get_ref()
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            ws.send(Message::Text(
                json!({"output":"\u{1b}[31mready"}).to_string(),
            ))
            .unwrap();
            for _ in 0..3 {
                let Message::Text(text) = ws.read().unwrap() else {
                    panic!("expected protocol text")
                };
                observed_tx
                    .send(serde_json::from_str::<Value>(&text).unwrap())
                    .unwrap();
            }
            ws.send(Message::Text(json!({"output":"\r\nhi"}).to_string()))
                .unwrap();
            assert!(matches!(ws.read().unwrap(), Message::Close(_)));
        });
        let (events_tx, events_rx) = channel();
        let (input, rx) = channel();
        spawn_session(
            format!("ws://{address}"),
            move |e| {
                let _ = events_tx.send(e);
            },
            rx,
            91,
            27,
            "",
        );
        let SessionEvent::Output(bytes) = events_rx.recv_timeout(Duration::from_secs(3)).unwrap()
        else {
            panic!("expected initial output")
        };
        let mut parser = vt100::Parser::new(27, 91, 100);
        parser.process(&bytes);
        assert_eq!(
            parser.screen().cell(0, 0).unwrap().fgcolor(),
            vt100::Color::Idx(1)
        );
        assert_eq!(
            observed_rx.recv_timeout(Duration::from_secs(3)).unwrap(),
            json!({"resize":[91,27]})
        );
        input.send(TermMsg::Resize(117, 32)).unwrap();
        input.send(TermMsg::Input("printf hi\r".into())).unwrap();
        assert_eq!(
            observed_rx.recv_timeout(Duration::from_secs(3)).unwrap(),
            json!({"resize":[117,32]})
        );
        assert_eq!(
            observed_rx.recv_timeout(Duration::from_secs(3)).unwrap(),
            json!({"input":"printf hi\r"})
        );
        let SessionEvent::Output(bytes) = events_rx.recv_timeout(Duration::from_secs(3)).unwrap()
        else {
            panic!("expected command output")
        };
        parser.process(&bytes);
        assert!(parser.screen().contents().ends_with("hi"));
        drop(input);
        server.join().unwrap();
    }

    #[test]
    fn empty_geometry_cannot_send_invalid_pty_dimensions() {
        let Message::Text(text) = resize_msg(0, 0) else {
            unreachable!()
        };
        assert_eq!(
            serde_json::from_str::<Value>(&text).unwrap(),
            json!({"resize":[1,1]})
        );
    }

    #[test]
    fn closed_document_never_opens_a_late_socket() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let (input, rx) = channel();
        drop(input);
        let (events, output) = channel();
        spawn_session(
            format!("ws://{}", listener.local_addr().unwrap()),
            move |event| {
                let _ = events.send(event);
            },
            rx,
            80,
            24,
            "",
        );
        assert!(matches!(
            output.recv_timeout(Duration::from_secs(3)).unwrap(),
            SessionEvent::Closed
        ));
        assert!(
            matches!(listener.accept(),Err(error) if error.kind()==std::io::ErrorKind::WouldBlock)
        );
    }

    #[cfg(unix)]
    #[test]
    fn database_login_shell_preserves_opaque_arguments_without_injection() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let stub=b"#!/bin/sh\nprintf '%s\\000' \"$MYSQL_PWD\" \"$PGPASSWORD\" \"$REDISCLI_AUTH\" \"$@\"\n";
        for binary in ["mysql", "psql", "mongosh", "redis-cli"] {
            let path = dir.path().join(binary);
            std::fs::write(&path, stub).unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let database = "--help space; printf INJECTED >&2; : > injected_db #";
        let user = "--user name; printf INJECTED >&2; : > injected_user #";
        let password = "pa'ss $(printf INJECTED >&2); : > injected_password";
        let inspect = json!({"databaseName":database,"user":user,"password":password,"rootPassword":password});
        for engine in ["mysql", "mariadb", "postgres", "mongo", "redis"] {
            let output = std::process::Command::new("/bin/sh")
                .arg("-c")
                .arg(db_command(engine, &inspect).unwrap())
                .current_dir(dir.path())
                .env("PATH", dir.path())
                .env_remove("MYSQL_PWD")
                .env_remove("PGPASSWORD")
                .env_remove("REDISCLI_AUTH")
                .output()
                .unwrap();
            assert!(output.status.success(), "{engine} stub failed");
            assert!(
                output.stderr.is_empty(),
                "{engine} executed shell syntax instead of preserving a literal"
            );
            let actual = String::from_utf8(output.stdout)
                .unwrap()
                .split_terminator('\0')
                .map(str::to_owned)
                .collect::<Vec<_>>();
            let mut expected = vec![String::new(); 3];
            let args = match engine {
                "mysql" | "mariadb" => {
                    expected[0] = password.into();
                    vec!["-uroot".to_owned(), format!("--database={database}")]
                }
                "postgres" => {
                    expected[1] = password.into();
                    vec!["-U".into(), user.into(), "-d".into(), database.into()]
                }
                "mongo" => vec![
                    "-u".into(),
                    user.into(),
                    "-p".into(),
                    password.into(),
                    "--authenticationDatabase".into(),
                    "admin".into(),
                ],
                "redis" => {
                    expected[2] = password.into();
                    Vec::new()
                }
                _ => unreachable!(),
            };
            expected.extend(args);
            assert_eq!(actual, expected, "{engine} changed argument boundaries");
            for side_effect in ["injected_db", "injected_user", "injected_password"] {
                assert!(
                    !dir.path().join(side_effect).exists(),
                    "{engine} executed injected shell redirection"
                );
            }
        }
        let output = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(db_command("postgres", &json!({"password":password})).unwrap())
            .current_dir(dir.path())
            .env("PATH", dir.path())
            .env_remove("MYSQL_PWD")
            .env_remove("PGPASSWORD")
            .env_remove("REDISCLI_AUTH")
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
        let actual = String::from_utf8(output.stdout)
            .unwrap()
            .split_terminator('\0')
            .map(str::to_owned)
            .collect::<Vec<_>>();
        assert_eq!(actual, vec!["", password, "", "-U", "postgres"]);
    }

    fn accept_stalled_client(listener: &std::net::TcpListener) -> TcpStream {
        listener.set_nonblocking(true).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            match listener.accept() {
                Ok((tcp, _)) => {
                    // macOS can inherit O_NONBLOCK from the listener. These
                    // peers must wait for EOF or their real read deadline.
                    tcp.set_nonblocking(false).unwrap();
                    return tcp;
                }
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(2))
                }
                Err(error) => {
                    panic!("terminal client did not reach the handshake fixture: {error}")
                }
            }
        }
    }

    fn cancelled_handshake_closes_socket(tls: bool) {
        use std::io::Read;
        for _ in 0..3 {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let (ready_tx, ready_rx) = channel();
            let server = std::thread::spawn(move || {
                let mut tcp = accept_stalled_client(&listener);
                tcp.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
                let mut byte = [0];
                tcp.read_exact(&mut byte).unwrap(); // HTTP request or TLS ClientHello has started.
                ready_tx.send(()).unwrap();
                let mut bytes = [0; 1024];
                loop {
                    match tcp.read(&mut bytes) {
                        Ok(0) => break,
                        Ok(_) => {}
                        Err(error)
                            if matches!(
                                error.kind(),
                                std::io::ErrorKind::ConnectionReset
                                    | std::io::ErrorKind::ConnectionAborted
                            ) =>
                        {
                            break
                        }
                        Err(error) => panic!("disconnect retained a connecting socket: {error}"),
                    }
                }
            });
            let (input, rx) = channel();
            let (events, output) = channel();
            spawn_session(
                format!("{}://{address}", if tls { "wss" } else { "ws" }),
                move |event| {
                    let _ = events.send(event);
                },
                rx,
                80,
                24,
                "",
            );
            ready_rx.recv_timeout(Duration::from_secs(3)).unwrap();
            drop(input);
            assert!(matches!(
                output.recv_timeout(Duration::from_secs(2)).unwrap(),
                SessionEvent::Closed
            ));
            server.join().unwrap();
        }
    }

    #[test]
    fn disconnect_during_http_upgrade_does_not_leak_reconnecting_sockets() {
        cancelled_handshake_closes_socket(false);
    }

    #[test]
    fn disconnect_during_tls_handshake_does_not_leak_reconnecting_sockets() {
        cancelled_handshake_closes_socket(true);
    }

    #[test]
    fn an_unanswered_upgrade_has_a_real_connection_deadline() {
        use std::io::Read;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (started_tx, started_rx) = channel();
        let server = std::thread::spawn(move || {
            let mut tcp = accept_stalled_client(&listener);
            tcp.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            let mut first = [0];
            tcp.read_exact(&mut first).unwrap();
            started_tx.send(()).unwrap();
            let mut bytes = [0; 1024];
            loop {
                match tcp.read(&mut bytes) {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(error)
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::ConnectionReset
                                | std::io::ErrorKind::ConnectionAborted
                        ) =>
                    {
                        break
                    }
                    Err(error) => panic!("connection deadline retained its socket: {error}"),
                }
            }
        });
        let (_input, rx) = channel();
        let result = connect_session(
            &format!("ws://{address}"),
            &rx,
            &mut VecDeque::new(),
            &mut (80, 24),
            Duration::from_millis(200),
        );
        assert!(
            matches!(result, Err(error) if matches!(error.as_ref(), tungstenite::Error::Io(error) if error.kind() == std::io::ErrorKind::TimedOut))
        );
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        server.join().unwrap();
    }
}
