//! Client for Rex's local screens Unix socket (`rex_screens_socket()`),
//! implementing [`StreamOpener`] so [`crate::StreamScreenSource`] talks to any
//! remote Rex host through Rex with zero SSH/QUIC code in `apps/screen`.
//!
//! Zero GUI dependencies. Every connect uses [`rex_sock::connect_verified`]
//! (verifying `SO_PEERCRED` UID before a single byte is sent, bound strictly to
//! the configured display endpoint rather than an untrusted path from disk),
//! enforces one monotonic deadline across connect + request + handshake/reply,
//! honors cancellation promptly even during `Ready`, shuts down the write half
//! when dropped, and tears down both halves via `Shutdown::Both` on cancel so
//! the host helper releases `Take Control` and stops capture/export immediately.

use std::{
    io::{self, Read, Write},
    net::Shutdown,
    os::unix::net::UnixStream,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use crate::{
    paths,
    rex_sock::{self, MAX_REQUEST_LINE_BYTES, MAX_RESPONSE_LINE_BYTES, RexScreen, RexScreensState, RexSockRequest, RexSockResponse},
    stream::{CommandOutput, LiveStream, StreamOpener},
};

/// Timeout for one-shot RPCs (start, stop, clear, rename, archive, delete).
const RPC_TIMEOUT: Duration = Duration::from_secs(60);
/// Timeout for fast handshake / metadata ops (`list`, `protocol`, stream `Ready`).
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);

/// Result of asking Rex to bring forward a host window (`Show in Rex`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenHostError {
    /// Rex's local screens socket is not listening / unreachable (caller may
    /// fall back to launching `olo-rex --focus-host <key>` detached).
    Unavailable(String),
    /// Rex answered and explicitly refused the request (surface its words; do
    /// not spawn a duplicate Rex process).
    Refused(String),
}

impl std::fmt::Display for OpenHostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpenHostError::Unavailable(m) | OpenHostError::Refused(m) => f.write_str(m),
        }
    }
}

/// Write half wrapper that explicitly signals EOF (`shutdown(Shutdown::Write)`)
/// on drop even while the reader or cancel closure still owns a duplicated fd
/// of the same Unix socket (ensures `open_export` write-half drop cancels the
/// host export immediately).
struct ShutdownWrite(UnixStream);

impl Write for ShutdownWrite {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

impl Drop for ShutdownWrite {
    fn drop(&mut self) {
        let _ = self.0.shutdown(Shutdown::Write);
    }
}

/// Client speaking [`rex_sock`] to Rex's screens Unix socket.
#[derive(Clone, Debug)]
pub struct RexSockClient {
    /// Explicit socket path (resolved from `paths::rex_screens_socket()` when
    /// `None`; never reads an untrusted socket path from the disk state file).
    socket: Option<PathBuf>,
}

impl Default for RexSockClient {
    fn default() -> Self {
        Self::new()
    }
}

impl RexSockClient {
    pub fn new() -> Self {
        RexSockClient { socket: None }
    }

    pub fn with_socket(path: impl Into<PathBuf>) -> Self {
        RexSockClient { socket: Some(path.into()) }
    }

    /// Configured display endpoint (`OLO_REX_SCREENS_SOCKET` or
    /// `<rex_screens_dir>/rex-<WAYLAND_DISPLAY>.sock`).
    pub fn socket_path(&self) -> PathBuf {
        self.socket.clone().unwrap_or_else(paths::rex_screens_socket)
    }

    fn connect_io(&self, deadline: Instant, cancelled: Option<&AtomicBool>) -> io::Result<UnixStream> {
        let path = self.socket_path();
        rex_sock::connect_verified(&path, deadline, None, cancelled)
    }

    fn rpc_with_cancel(&self, req: &RexSockRequest, timeout: Duration, cancelled: Option<&AtomicBool>) -> Result<RexSockResponse, String> {
        rex_sock::validate_request(req)?;
        let deadline = Instant::now() + timeout;
        let stream = self.connect_io(deadline, cancelled).map_err(|e| format!("Rex screens service isn’t reachable: {e}"))?;
        let encoded = rex_sock::encode_bounded_json_line(req, MAX_REQUEST_LINE_BYTES, "RexSockRequest").map_err(|e| format!("invalid request: {e}"))?;
        rex_sock::write_all_deadline(&stream, &encoded, deadline, None, cancelled).map_err(|e| format!("couldn’t talk to Rex: {e}"))?;
        match rex_sock::read_line_deadline(&stream, MAX_RESPONSE_LINE_BYTES, deadline, None, cancelled).map_err(|e| format!("invalid answer from Rex: {e}"))? {
            Some((line, _tail)) => {
                let resp: RexSockResponse = serde_json::from_slice(&line).map_err(|e| format!("invalid answer from Rex: {e}"))?;
                rex_sock::validate_response(&resp).map_err(|e| format!("invalid answer from Rex: {e}"))?;
                Ok(resp)
            }
            None => Err("Rex closed the connection without answering.".into()),
        }
    }

    fn rpc(&self, req: &RexSockRequest, timeout: Duration) -> Result<RexSockResponse, String> {
        self.rpc_with_cancel(req, timeout, None)
    }

    /// Fetch the full current snapshot via `{"op":"list"}`.
    pub fn list(&self) -> Result<RexScreensState, String> {
        self.list_with_cancel(None)
    }

    fn list_with_cancel(&self, cancelled: Option<&AtomicBool>) -> Result<RexScreensState, String> {
        match self.rpc_with_cancel(&RexSockRequest::List, HANDSHAKE_TIMEOUT, cancelled)? {
            RexSockResponse::Snapshot { state } => Ok(state),
            RexSockResponse::Error { message } => Err(message),
            other => Err(format!("unexpected answer to list: {other:?}")),
        }
    }

    /// Fetch cached `olo-rex-link screen --protocol` capabilities for `host`.
    pub fn protocol(&self, host: &str) -> Result<(u32, Vec<String>, Vec<String>), String> {
        match self.rpc(&RexSockRequest::Protocol { host: host.to_owned() }, HANDSHAKE_TIMEOUT)? {
            RexSockResponse::Protocol { proto, commands, capabilities } => Ok((proto, commands, capabilities)),
            RexSockResponse::Error { message } => Err(message),
            other => Err(format!("unexpected answer to protocol: {other:?}")),
        }
    }

    /// Start a new screen (or restart a stopped one) on `host`.
    pub fn start_screen(&self, host: &str, name: Option<&str>, keep: bool, size: Option<(u32, u32)>, scale: Option<f64>, sound: bool) -> Result<RexScreen, String> {
        let req = RexSockRequest::Start { host: host.to_owned(), name: name.map(str::to_owned), keep, size, scale, sound };
        match self.rpc(&req, RPC_TIMEOUT)? {
            RexSockResponse::Started { screen } => {
                if let Some(expected_name) = name
                    && screen.name != expected_name
                {
                    return Err(format!("Rex returned screen {:?} instead of requested {:?}", screen.name, expected_name));
                }
                Ok(*screen)
            }
            RexSockResponse::Error { message } => Err(message),
            other => Err(format!("unexpected answer to start: {other:?}")),
        }
    }

    /// Stop `name` on `host`, checking `expected_started` and/or
    /// `expected_last_used` under the host's lifecycle lock.
    pub fn stop_screen(&self, host: &str, name: &str, expected_started: Option<u64>, expected_last_used: Option<u64>) -> Result<(), String> {
        let req = RexSockRequest::Stop { host: host.to_owned(), name: name.to_owned(), expected_started, expected_last_used };
        expect_ok(self.rpc(&req, RPC_TIMEOUT)?)
    }

    /// Clear history of stopped `name` on `host`, checking `expected_started`
    /// and/or `expected_last_used` (`Some(0)` = observed empty) under the
    /// host's lifecycle lock.
    pub fn clear_history(&self, host: &str, name: &str, expected_started: Option<u64>, expected_last_used: Option<u64>) -> Result<(), String> {
        let req = RexSockRequest::ClearHistory { host: host.to_owned(), name: name.to_owned(), expected_started, expected_last_used };
        expect_ok(self.rpc(&req, RPC_TIMEOUT)?)
    }

    /// Rename `name` on `host`.
    pub fn rename(&self, host: &str, name: &str, title: Option<&str>) -> Result<(), String> {
        let req = RexSockRequest::Rename { host: host.to_owned(), name: name.to_owned(), title: title.map(str::to_owned) };
        expect_ok(self.rpc(&req, RPC_TIMEOUT)?)
    }

    /// Archive or unarchive `name` on `host`.
    pub fn archive(&self, host: &str, name: &str, archived: bool) -> Result<(), String> {
        let req = RexSockRequest::Archive { host: host.to_owned(), name: name.to_owned(), archived };
        expect_ok(self.rpc(&req, RPC_TIMEOUT)?)
    }

    /// Delete stopped `name` on `host`, checking `expected_started` and/or
    /// `expected_last_used` (`Some(0)` = observed empty) under the host's
    /// lifecycle lock.
    pub fn delete(&self, host: &str, name: &str, expected_started: Option<u64>, expected_last_used: Option<u64>) -> Result<(), String> {
        let req = RexSockRequest::Delete { host: host.to_owned(), name: name.to_owned(), expected_started, expected_last_used };
        expect_ok(self.rpc(&req, RPC_TIMEOUT)?)
    }

    /// Ask Rex to bring forward its window for `host`, distinguishing an
    /// unavailable Rex service (eligible for detached `olo-rex` fallback) from
    /// an explicit Rex refusal.
    pub fn open_host(&self, host: &str) -> Result<(), OpenHostError> {
        let req = RexSockRequest::OpenHost { host: host.to_owned() };
        rex_sock::validate_request(&req).map_err(OpenHostError::Refused)?;
        let deadline = Instant::now() + HANDSHAKE_TIMEOUT;
        let stream = match self.connect_io(deadline, None) {
            Ok(s) => s,
            Err(e) if matches!(e.kind(), io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused) => {
                return Err(OpenHostError::Unavailable(format!("Rex isn’t reachable: {e}")));
            }
            Err(e) => return Err(OpenHostError::Refused(format!("couldn’t reach Rex: {e}"))),
        };
        let encoded = rex_sock::encode_bounded_json_line(&req, MAX_REQUEST_LINE_BYTES, "RexSockRequest").map_err(|e| OpenHostError::Refused(e.to_string()))?;
        rex_sock::write_all_deadline(&stream, &encoded, deadline, None, None).map_err(|e| OpenHostError::Unavailable(format!("couldn’t talk to Rex: {e}")))?;
        match rex_sock::read_line_deadline(&stream, MAX_RESPONSE_LINE_BYTES, deadline, None, None).map_err(|e| OpenHostError::Refused(format!("invalid answer from Rex: {e}")))? {
            Some((line, _)) => {
                let resp: RexSockResponse = serde_json::from_slice(&line).map_err(|e| OpenHostError::Refused(format!("invalid answer from Rex: {e}")))?;
                rex_sock::validate_response(&resp).map_err(|e| OpenHostError::Refused(format!("invalid answer from Rex: {e}")))?;
                match resp {
                    RexSockResponse::Ok => Ok(()),
                    RexSockResponse::Error { message } => Err(OpenHostError::Refused(message)),
                    other => Err(OpenHostError::Refused(format!("unexpected answer from Rex: {other:?}"))),
                }
            }
            None => Err(OpenHostError::Unavailable("Rex closed the connection without answering.".into())),
        }
    }

    /// Open a raw binary relay (`stream`, `history`, or `export`) over the Rex
    /// screens socket. Enforces one monotonic deadline across connect, write,
    /// and `Ready`, honors `cancelled` promptly during `Ready`, and keeps any
    /// tail bytes read past the `Ready` newline attached to the returned reader.
    fn open_relay(&self, req: &RexSockRequest, cancelled: &Arc<AtomicBool>) -> Result<LiveStream, String> {
        if cancelled.load(Ordering::Acquire) {
            return Err("cancelled".into());
        }
        rex_sock::validate_request(req)?;
        let deadline = Instant::now() + HANDSHAKE_TIMEOUT;
        let stream =
            self.connect_io(deadline, Some(cancelled)).map_err(|e| if cancelled.load(Ordering::Acquire) { "cancelled".into() } else { format!("Rex screens service isn’t reachable: {e}") })?;
        let encoded = rex_sock::encode_bounded_json_line(req, MAX_REQUEST_LINE_BYTES, "RexSockRequest").map_err(|e| format!("invalid request: {e}"))?;
        rex_sock::write_all_deadline(&stream, &encoded, deadline, None, Some(cancelled)).map_err(|e| {
            let _ = stream.shutdown(Shutdown::Both);
            if cancelled.load(Ordering::Acquire) { "cancelled".into() } else { format!("couldn’t talk to Rex: {e}") }
        })?;
        let (line, buffered_tail) = match rex_sock::read_line_deadline(&stream, MAX_RESPONSE_LINE_BYTES, deadline, None, Some(cancelled)) {
            Ok(Some(v)) => v,
            Ok(None) => {
                let _ = stream.shutdown(Shutdown::Both);
                return Err("Rex closed the connection before the stream was ready.".into());
            }
            Err(e) => {
                let _ = stream.shutdown(Shutdown::Both);
                return Err(if cancelled.load(Ordering::Acquire) { "cancelled".into() } else { format!("invalid answer from Rex: {e}") });
            }
        };
        let resp: RexSockResponse = serde_json::from_slice(&line).map_err(|e| {
            let _ = stream.shutdown(Shutdown::Both);
            format!("invalid answer from Rex: {e}")
        })?;
        rex_sock::validate_response(&resp).map_err(|e| {
            let _ = stream.shutdown(Shutdown::Both);
            format!("invalid answer from Rex: {e}")
        })?;
        match resp {
            RexSockResponse::Ready { .. } => {}
            RexSockResponse::Error { message } => {
                let _ = stream.shutdown(Shutdown::Both);
                return Err(message);
            }
            other => {
                let _ = stream.shutdown(Shutdown::Both);
                return Err(format!("unexpected answer from Rex: {other:?}"));
            }
        }
        if cancelled.load(Ordering::Acquire) {
            let _ = stream.shutdown(Shutdown::Both);
            return Err("cancelled".into());
        }
        // Switch back to blocking mode for long-lived binary relay.
        rex_sock::set_stream_blocking(&stream).map_err(|e| format!("couldn’t configure Rex socket: {e}"))?;
        let writer = stream.try_clone().map_err(|e| format!("couldn’t clone Rex socket: {e}"))?;
        let canceller = stream.try_clone().map_err(|e| format!("couldn’t clone Rex socket: {e}"))?;
        let chained: Box<dyn Read + Send> = if buffered_tail.is_empty() { Box::new(stream) } else { Box::new(std::io::Cursor::new(buffered_tail).chain(stream)) };
        let cancel = move || {
            let _ = canceller.shutdown(Shutdown::Both);
        };
        Ok(LiveStream { reader: chained, writer: Box::new(ShutdownWrite(writer)), cancel: Some(Box::new(cancel)) })
    }
}

fn expect_ok(resp: RexSockResponse) -> Result<(), String> {
    match resp {
        RexSockResponse::Ok => Ok(()),
        RexSockResponse::Error { message } => Err(message),
        other => Err(format!("unexpected answer from Rex: {other:?}")),
    }
}

/// [`StreamOpener`] backed by [`RexSockClient`] for one remote Rex host.
///
/// Maps `StreamScreenSource`'s binary streams (`open_live`, `open_history`,
/// `open_export`) onto Rex's raw-relay socket requests and maps one-shot helper
/// commands (`run_command`) onto Rex's typed JSON RPCs, forwarding explicit
/// `--expected-started` and `--expected-last-used` values captured when the
/// user confirmed the action.
pub struct RexSockOpener {
    client: RexSockClient,
    host: String,
    /// Optional running session `started` generation (seconds since Unix epoch)
    /// bound to this opener so `open_live` never attaches to a replacement
    /// session under the same screen name (Final pass B.1 / Astra #5).
    expected_started: Option<u64>,
}

impl RexSockOpener {
    pub fn new(client: RexSockClient, host: impl Into<String>) -> Self {
        RexSockOpener { client, host: host.into(), expected_started: None }
    }

    /// Bind `open_live` to one running session's `started` timestamp so retries
    /// refuse a replacement screen under the same name with `ScreenGone`.
    pub fn with_expected_started(mut self, expected_started: Option<u64>) -> Self {
        self.expected_started = expected_started;
        self
    }

    fn verify_expected_started(&self, name: &str, cancelled: &Arc<AtomicBool>) -> Result<(), String> {
        let Some(expected) = self.expected_started else {
            return Ok(());
        };
        let state = self.client.list_with_cancel(Some(cancelled))?;
        let Some(host) = state.hosts.iter().find(|h| h.key == self.host) else {
            return Err(format!("{}Host “{}” is no longer available.", crate::stream::OPEN_LIVE_SCREEN_GONE_PREFIX, self.host));
        };
        if !host.state.is_connected() {
            return Err(format!("“{}” is disconnected.", host.label));
        }
        let matches = host.screens.iter().find(|s| s.name == name).is_some_and(|s| s.is_running() && s.started == Some(expected));
        if !matches {
            return Err(format!("{}Screen “{name}” was stopped or replaced.", crate::stream::OPEN_LIVE_SCREEN_GONE_PREFIX));
        }
        Ok(())
    }
}

impl StreamOpener for RexSockOpener {
    fn open_live(&self, name: &str, cancelled: Arc<AtomicBool>) -> Result<LiveStream, String> {
        // B.1 (#5): verify running `(host, name, started)` before opening the
        // relay and again right after `Ready` so a replacement session under
        // the same name is rejected terminally as `ScreenGone`.
        self.verify_expected_started(name, &cancelled)?;
        let live = self.client.open_relay(&RexSockRequest::Stream { host: self.host.clone(), name: name.to_owned() }, &cancelled)?;
        if let Err(err) = self.verify_expected_started(name, &cancelled) {
            if let Some(cancel) = live.cancel {
                cancel();
            }
            drop(live.writer);
            drop(live.reader);
            return Err(err);
        }
        Ok(live)
    }

    fn open_history(&self, name: &str, cancelled: Arc<AtomicBool>) -> Result<LiveStream, String> {
        self.client.open_relay(&RexSockRequest::History { host: self.host.clone(), name: name.to_owned() }, &cancelled)
    }

    fn open_export(&self, name: &str, args: &[String], cancelled: Arc<AtomicBool>) -> Result<LiveStream, String> {
        self.client.open_relay(&RexSockRequest::Export { host: self.host.clone(), name: name.to_owned(), args: args.to_vec() }, &cancelled)
    }

    fn run_command(&self, args: &[String], cancelled: Arc<AtomicBool>) -> Result<CommandOutput, String> {
        if cancelled.load(Ordering::Acquire) {
            return Err("cancelled".into());
        }
        let Some(cmd) = args.first().map(String::as_str) else {
            return Err("missing host screen command".into());
        };
        let to_out = |res: Result<String, String>| match res {
            Ok(stdout) => Ok(CommandOutput { status: 0, stdout, stderr: String::new() }),
            Err(stderr) => Ok(CommandOutput { status: 1, stdout: String::new(), stderr }),
        };
        match cmd {
            "--protocol" => {
                if args.len() != 1 {
                    return Err(format!("unexpected arguments to --protocol: {:?}", &args[1..]));
                }
                let (proto, commands, capabilities) = self.client.protocol(&self.host)?;
                let body = serde_json::json!({ "proto": proto, "commands": commands, "capabilities": capabilities }).to_string();
                Ok(CommandOutput { status: 0, stdout: body, stderr: String::new() })
            }
            "start" => {
                let name = args.get(1).filter(|a| !a.starts_with("--")).map(String::as_str);
                let keep = args.iter().any(|a| a == "--keep");
                let sound = args.iter().any(|a| a == "--sound");
                let size = flag_val(args, "--size").and_then(|s| s.split_once('x')).and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)));
                let scale = flag_val(args, "--scale").and_then(|s| s.parse().ok());
                to_out(self.client.start_screen(&self.host, name, keep, size, scale, sound).map(|sc| serde_json::to_string(&sc).unwrap_or_default()))
            }
            "stop" => {
                let name = args.get(1).ok_or("stop requires screen name")?;
                // B.3 (#2–#4): strict guard parser rejects malformed/duplicate/unknown flags
                // rather than silently mapping invalid values to `None`.
                let (_, expected_started, expected_last_used) = crate::screens::parse_expected_flags_strict(&args[2..], false)?;
                to_out(self.client.stop_screen(&self.host, name, expected_started, expected_last_used).map(|()| String::new()))
            }
            "rename" => {
                let name = args.get(1).ok_or("rename requires screen name")?;
                let title = args.get(2).map(String::as_str);
                to_out(self.client.rename(&self.host, name, title).map(|()| String::new()))
            }
            "history" => {
                let name = args.get(1).ok_or("history requires screen name")?;
                let (saw_clear, expected_started, expected_last_used) = crate::screens::parse_expected_flags_strict(&args[2..], true)?;
                if !saw_clear {
                    return Err(format!("unsupported history command over Rex socket: {args:?}"));
                }
                to_out(self.client.clear_history(&self.host, name, expected_started, expected_last_used).map(|()| String::new()))
            }
            "archive" => {
                let name = args.get(1).ok_or("archive requires screen name")?;
                to_out(self.client.archive(&self.host, name, true).map(|()| String::new()))
            }
            "unarchive" => {
                let name = args.get(1).ok_or("unarchive requires screen name")?;
                to_out(self.client.archive(&self.host, name, false).map(|()| String::new()))
            }
            "delete" => {
                let name = args.get(1).ok_or("delete requires screen name")?;
                let (_, expected_started, expected_last_used) = crate::screens::parse_expected_flags_strict(&args[2..], false)?;
                to_out(self.client.delete(&self.host, name, expected_started, expected_last_used).map(|()| String::new()))
            }
            other => Err(format!("unsupported host screen command over Rex socket: {other}")),
        }
    }
}

fn flag_val<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.windows(2).find(|w| w[0] == flag).map(|w| w[1].as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufReader, Write},
        os::unix::{fs::PermissionsExt as _, net::UnixListener},
    };

    #[test]
    fn rex_sock_opener_relays_binary_and_maps_lifecycle_rpcs() {
        let dir = PathBuf::from(format!("/var/tmp/olo-rex-client-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let sock = dir.join("rex-test.sock");
        let listener = UnixListener::bind(&sock).unwrap();

        let (live_eof_tx, live_eof_rx) = std::sync::mpsc::channel();
        let srv = std::thread::spawn(move || {
            for _ in 0..6 {
                let (mut conn, _) = listener.accept().unwrap();
                rex_sock::verify_peer_uid(&conn).unwrap();
                let mut reader = BufReader::new(conn.try_clone().unwrap());
                let req = rex_sock::read_request(&mut reader).unwrap().unwrap();
                match req {
                    RexSockRequest::Protocol { host } => {
                        assert_eq!(host, "dev-box");
                        rex_sock::write_response(
                            &mut conn,
                            &RexSockResponse::Protocol {
                                proto: 1,
                                commands: vec!["stream".into(), "history-stream".into()],
                                capabilities: vec!["control-ack".into(), "stop-expected".into(), "stop-expected-started".into()],
                            },
                        )
                        .unwrap();
                    }
                    RexSockRequest::Stream { host, name } => {
                        assert_eq!((host.as_str(), name.as_str()), ("dev-box", "review"));
                        // Send Ready + immediate binary tail in the same write to test tail preservation!
                        let mut combined = serde_json::to_vec(&RexSockResponse::Ready { width: Some(1440), height: Some(900), scale: Some(1.0) }).unwrap();
                        combined.push(b'\n');
                        combined.extend_from_slice(b"BIN-TAIL!");
                        conn.write_all(&combined).unwrap();
                        let mut echo = [0u8; 4];
                        reader.read_exact(&mut echo).unwrap();
                        assert_eq!(&echo, b"PING");
                        conn.write_all(b"PONG").unwrap();
                        // Final pass B.13 (#38): remain blocked on read until client's cancel hook
                        // fires while BOTH `reader` and `writer` are still alive, and signal EOF!
                        let mut drain = [0u8; 16];
                        let n = reader.read(&mut drain).unwrap();
                        let _ = live_eof_tx.send(n);
                    }
                    RexSockRequest::Export { host, name, .. } => {
                        assert_eq!((host.as_str(), name.as_str()), ("dev-box", "review"));
                        rex_sock::write_response(&mut conn, &RexSockResponse::Ready { width: None, height: None, scale: None }).unwrap();
                        // Finding 35: dropping ONLY the write half must deliver EOF to the peer!
                        let mut drain = [0u8; 16];
                        assert_eq!(reader.read(&mut drain).unwrap(), 0, "dropping write half must deliver EOF");
                    }
                    RexSockRequest::Stop { host, name, expected_started, expected_last_used } => {
                        assert_eq!((host.as_str(), name.as_str(), expected_started, expected_last_used), ("dev-box", "review", Some(12345), Some(99_000)));
                        rex_sock::write_response(&mut conn, &RexSockResponse::Ok).unwrap();
                    }
                    RexSockRequest::ClearHistory { host, name, expected_started, expected_last_used } => {
                        assert_eq!((host.as_str(), name.as_str(), expected_started, expected_last_used), ("dev-box", "review", Some(0), Some(99_000)));
                        rex_sock::write_response(&mut conn, &RexSockResponse::Error { message: "newer activity".into() }).unwrap();
                    }
                    RexSockRequest::OpenHost { host } => {
                        assert_eq!(host, "build-farm");
                        rex_sock::write_response(&mut conn, &RexSockResponse::Ok).unwrap();
                    }
                    other => panic!("unexpected request: {other:?}"),
                }
            }
        });

        let client = RexSockClient::with_socket(&sock);
        let opener = RexSockOpener::new(client.clone(), "dev-box");
        let no_cancel = Arc::new(AtomicBool::new(false));

        // 1. Protocol probe
        let proto = opener.run_command(&["--protocol".into()], no_cancel.clone()).unwrap();
        assert!(proto.success());
        assert!(proto.stdout.contains("control-ack"));

        // 2. Binary relay preserving tail bytes + B.13 (#38): invoke `cancel()` while BOTH
        //    `reader` and `writer` stay alive and verify server observes EOF BEFORE dropping them!
        let mut live = opener.open_live("review", no_cancel.clone()).unwrap();
        let mut tail = [0u8; 9];
        live.reader.read_exact(&mut tail).unwrap();
        assert_eq!(&tail, b"BIN-TAIL!");
        live.writer.write_all(b"PING").unwrap();
        let mut pong = [0u8; 4];
        live.reader.read_exact(&mut pong).unwrap();
        assert_eq!(&pong, b"PONG");
        if let Some(cancel) = live.cancel.take() {
            cancel();
        }
        let eof_n = live_eof_rx.recv_timeout(Duration::from_secs(2)).expect("cancel hook must deliver EOF while reader and writer are still alive");
        assert_eq!(eof_n, 0, "server must observe 0-byte EOF from cancel hook before live halves drop");
        drop(live);

        // 3. Export write-half drop cancels peer even while reader & cancel remain alive (Finding 35)
        let exp = opener.open_export("review", &["--zoom".into(), "1".into()], no_cancel.clone()).unwrap();
        let LiveStream { reader: _keep_reader, writer, cancel: _keep_cancel } = exp;
        drop(writer);

        // 4. Stop forwards both --expected-started and --expected-last-used
        let stop = opener.run_command(&["stop".into(), "review".into(), "--expected-started".into(), "12345".into(), "--expected-last-used".into(), "99000".into()], no_cancel.clone()).unwrap();
        assert!(stop.success());

        // 5. ClearHistory forwards explicit expected tokens and surfaces helper refusal words
        let clear = opener
            .run_command(&["history".into(), "review".into(), "--clear".into(), "--expected-started".into(), "0".into(), "--expected-last-used".into(), "99000".into()], no_cancel.clone())
            .unwrap();
        assert!(!clear.success());
        assert_eq!(clear.error_words(), "newer activity");

        // 6. OpenHost
        client.open_host("build-farm").unwrap();

        srv.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn round3_f12_f36_cancel_during_ready_is_prompt_and_open_host_distinguishes_errors() {
        let dir = PathBuf::from(format!("/var/tmp/olo-rex-client-r3-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();

        // Finding 36: missing socket -> Unavailable; explicit server Error -> Refused.
        let missing_client = RexSockClient::with_socket(dir.join("absent.sock"));
        assert!(matches!(missing_client.open_host("dev-box"), Err(OpenHostError::Unavailable(_))));

        let sock = dir.join("rex-r3.sock");
        let listener = UnixListener::bind(&sock).unwrap();
        // B.13 (#12/#38): synchronize cancellation strictly on server receipt of the
        // `Stream` request rather than fixed sleeps so scheduler delays never race.
        let (req_seen_tx, req_seen_rx) = std::sync::mpsc::channel::<()>();
        let (release_conn1_tx, release_conn1_rx) = std::sync::mpsc::channel::<()>();
        let srv = std::thread::spawn(move || {
            // Connection 1: read the Stream request, notify test, then hold Ready silent until test finishes.
            let (conn1, _) = listener.accept().unwrap();
            let mut r1 = BufReader::new(conn1.try_clone().unwrap());
            let req1 = rex_sock::read_request(&mut r1).unwrap().unwrap();
            assert!(matches!(req1, RexSockRequest::Stream { .. }));
            let _ = req_seen_tx.send(());
            let _ = release_conn1_rx.recv_timeout(Duration::from_secs(5));
            drop(conn1);
            // Connection 2: OpenHost returns explicit refusal.
            let (mut conn2, _) = listener.accept().unwrap();
            let mut r = BufReader::new(conn2.try_clone().unwrap());
            let req2 = rex_sock::read_request(&mut r).unwrap().unwrap();
            assert!(matches!(req2, RexSockRequest::OpenHost { .. }));
            rex_sock::write_response(&mut conn2, &RexSockResponse::Error { message: "unknown host".into() }).unwrap();
        });

        let client = RexSockClient::with_socket(&sock);
        let opener = RexSockOpener::new(client.clone(), "dev-box");
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_bg = cancel.clone();
        let trigger_t = std::thread::spawn(move || {
            req_seen_rx.recv_timeout(Duration::from_secs(3)).expect("server must receive Stream request before cancelling");
            let fired_at = Instant::now();
            cancel_bg.store(true, Ordering::Release);
            fired_at
        });
        let err = opener.open_live("qa", cancel).err().unwrap();
        let fired_at = trigger_t.join().unwrap();
        assert_eq!(err, "cancelled");
        assert!(fired_at.elapsed() < Duration::from_secs(1), "cancel after server received Stream took {:?}", fired_at.elapsed());
        let _ = release_conn1_tx.send(());

        assert_eq!(client.open_host("ghost"), Err(OpenHostError::Refused("unknown host".into())));

        srv.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn final_pass_b1_b3_open_live_expected_started_and_strict_flags() {
        use crate::rex_sock::{HostScreenState, RexHost, RexHostConn, STATE_VERSION};
        let dir = PathBuf::from(format!("/var/tmp/olo-rex-client-b1-b3-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let sock = dir.join("rex-b1.sock");
        let listener = UnixListener::bind(&sock).unwrap();

        let make_state = |started: u64| RexScreensState {
            version: STATE_VERSION,
            rev: 1,
            pid: std::process::id(),
            socket: String::new(),
            hosts: vec![RexHost {
                key: "dev-box".into(),
                label: "dev-box".into(),
                state: RexHostConn::Connected,
                error: None,
                disconnected_at: None,
                installed: Some("1.0".into()),
                committed: None,
                default_on_host: true,
                not_set_up_reason: None,
                can_control: Some(true),
                control_disabled_reason: None,
                capabilities: vec!["control-ack".into(), "stop-expected".into()],
                screens: vec![RexScreen {
                    name: "qa".into(),
                    title: None,
                    state: HostScreenState::Running,
                    width: 1280,
                    height: 800,
                    scale: 1.0,
                    sound: false,
                    pid: Some(42),
                    started: Some(started),
                    last_used: None,
                    actions: Some(0),
                    controlled: false,
                    controlled_by: None,
                    owner: None,
                    started_by: None,
                }],
            }],
        };

        let srv = std::thread::spawn(move || {
            // 1. Pre-open List sees replacement started=2000 (no Stream opened).
            let (mut c1, _) = listener.accept().unwrap();
            let mut r1 = BufReader::new(c1.try_clone().unwrap());
            assert!(matches!(rex_sock::read_request(&mut r1).unwrap().unwrap(), RexSockRequest::List));
            rex_sock::write_response(&mut c1, &RexSockResponse::Snapshot { state: make_state(2000) }).unwrap();

            // 2. Post-Ready race: pre-open List sees started=1000, Stream opens Ready,
            //    post-Ready List sees replacement started=2000 -> opener cancels relay!
            let (mut c2, _) = listener.accept().unwrap();
            let mut r2 = BufReader::new(c2.try_clone().unwrap());
            assert!(matches!(rex_sock::read_request(&mut r2).unwrap().unwrap(), RexSockRequest::List));
            rex_sock::write_response(&mut c2, &RexSockResponse::Snapshot { state: make_state(1000) }).unwrap();

            let (mut c3, _) = listener.accept().unwrap();
            let mut r3 = BufReader::new(c3.try_clone().unwrap());
            assert!(matches!(rex_sock::read_request(&mut r3).unwrap().unwrap(), RexSockRequest::Stream { .. }));
            rex_sock::write_response(&mut c3, &RexSockResponse::Ready { width: Some(1280), height: Some(800), scale: Some(1.0) }).unwrap();

            let (mut c4, _) = listener.accept().unwrap();
            let mut r4 = BufReader::new(c4.try_clone().unwrap());
            assert!(matches!(rex_sock::read_request(&mut r4).unwrap().unwrap(), RexSockRequest::List));
            rex_sock::write_response(&mut c4, &RexSockResponse::Snapshot { state: make_state(2000) }).unwrap();

            // Stream c3 must receive immediate EOF because opener cancelled it on mismatch.
            let mut buf = [0u8; 8];
            assert_eq!(r3.read(&mut buf).unwrap(), 0, "mismatched post-Ready relay must be closed immediately");
        });

        let client = RexSockClient::with_socket(&sock);
        let opener = RexSockOpener::new(client, "dev-box").with_expected_started(Some(1000));
        let no_cancel = Arc::new(AtomicBool::new(false));

        // B.1: pre-open mismatch -> ScreenGone sentinel.
        let err1 = opener.open_live("qa", no_cancel.clone()).err().unwrap();
        assert!(err1.starts_with(crate::stream::OPEN_LIVE_SCREEN_GONE_PREFIX), "expected ScreenGone sentinel, got: {err1}");

        // B.1: post-Ready race mismatch -> ScreenGone sentinel + relay closed.
        let err2 = opener.open_live("qa", no_cancel.clone()).err().unwrap();
        assert!(err2.starts_with(crate::stream::OPEN_LIVE_SCREEN_GONE_PREFIX), "expected ScreenGone sentinel on post-Ready mismatch, got: {err2}");

        // B.3: malformed guard flags fail closed BEFORE touching the socket!
        let s = |xs: &[&str]| xs.iter().map(|v| (*v).to_owned()).collect::<Vec<_>>();
        assert!(opener.run_command(&s(&["delete", "qa", "--expected-started=0"]), no_cancel.clone()).is_err());
        assert!(opener.run_command(&s(&["stop", "qa", "--expected-started", "notnum"]), no_cancel.clone()).is_err());
        assert!(opener.run_command(&s(&["stop", "qa", "--expected-started", "1", "--expected-started", "2"]), no_cancel.clone()).is_err());
        assert!(opener.run_command(&s(&["history", "qa", "--clear", "--bogus"]), no_cancel.clone()).is_err());
        assert!(opener.run_command(&s(&["history", "qa", "--expected-started", "0"]), no_cancel.clone()).is_err());

        srv.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn round3_f35_dropping_writer_alone_signals_eof_while_reader_and_cancel_stay_alive() {
        let dir = PathBuf::from(format!("/var/tmp/olo-rex-client-f35-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let sock = dir.join("rex-f35.sock");
        let listener = UnixListener::bind(&sock).unwrap();

        let (eof_tx, eof_rx) = std::sync::mpsc::channel();
        let srv = std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let _ = conn.set_read_timeout(Some(Duration::from_secs(3)));
            let mut r = BufReader::new(conn.try_clone().unwrap());
            let _ = rex_sock::read_request(&mut r).unwrap();
            rex_sock::write_response(&mut conn, &RexSockResponse::Ready { width: Some(1280), height: Some(800), scale: Some(1.0) }).unwrap();
            let mut buf = [0u8; 16];
            let n = r.read(&mut buf).unwrap_or(99);
            let _ = eof_tx.send(n);
        });

        let opener = RexSockOpener::new(RexSockClient::with_socket(&sock), "dev-box");
        let live = opener.open_export("qa", &["--zoom".into(), "1".into()], Arc::new(AtomicBool::new(false))).unwrap();
        let crate::stream::LiveStream { reader, writer, cancel } = live;
        // Keep `reader` and `cancel` alive; drop ONLY `writer`. Server MUST observe immediate EOF!
        drop(writer);
        let got = eof_rx.recv_timeout(Duration::from_secs(2)).expect("server must observe EOF on writer drop while reader & cancel stay alive");
        assert_eq!(got, 0, "expected 0-byte EOF after dropping writer half");
        drop(reader);
        drop(cancel);
        srv.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn agy_r1_f10_verify_expected_started_fails_closed_when_host_missing_or_disconnected() {
        use crate::rex_sock::{HostScreenState, RexHost, RexHostConn, RexScreen, RexScreensState, RexSockResponse, STATE_VERSION};
        use std::{collections::VecDeque, io::BufRead as _, os::unix::fs::PermissionsExt as _, os::unix::net::UnixListener};

        let dir = PathBuf::from("/var/tmp").join(format!("olo-rex-client-f10-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let sock = dir.join("screens.sock");
        let listener = UnixListener::bind(&sock).unwrap();

        let mk_host = |state: RexHostConn, started: Option<u64>| RexHost {
            key: "h1".into(),
            label: "Lab Box".into(),
            state,
            error: None,
            disconnected_at: None,
            installed: Some("1.0".into()),
            committed: None,
            default_on_host: true,
            not_set_up_reason: None,
            can_control: Some(true),
            control_disabled_reason: None,
            capabilities: Vec::new(),
            screens: vec![RexScreen {
                name: "qa".into(),
                title: None,
                state: HostScreenState::Running,
                width: 1280,
                height: 800,
                scale: 1.0,
                sound: false,
                pid: Some(42),
                started,
                last_used: None,
                actions: None,
                controlled: false,
                owner: None,
                started_by: None,
                controlled_by: None,
            }],
        };

        let mk_state = |rev: u64, hosts: Vec<RexHost>| RexScreensState { version: STATE_VERSION, rev, pid: std::process::id(), socket: String::new(), hosts };
        let mut snapshots = VecDeque::from([
            // 1. Host disappeared completely -> terminal ScreenGone sentinel error.
            mk_state(1, Vec::new()),
            // 2. Host disconnected -> disconnect error (must NOT fail open).
            mk_state(2, vec![mk_host(RexHostConn::Disconnected, Some(1000))]),
            // 3. Host reconnecting -> disconnect error (must NOT fail open).
            mk_state(3, vec![mk_host(RexHostConn::Reconnecting, Some(1000))]),
            // 4. Connected with mismatched started -> terminal ScreenGone sentinel error.
            mk_state(4, vec![mk_host(RexHostConn::Connected, Some(2000))]),
            // 5. Connected with matching started -> Ok.
            mk_state(5, vec![mk_host(RexHostConn::Connected, Some(1000))]),
        ]);

        let srv = std::thread::spawn(move || {
            while let Some(state) = snapshots.pop_front() {
                let (mut conn, _) = listener.accept().unwrap();
                let mut req = String::new();
                std::io::BufReader::new(conn.try_clone().unwrap()).read_line(&mut req).unwrap();
                let line = serde_json::to_string(&RexSockResponse::Snapshot { state }).unwrap();
                std::io::Write::write_all(&mut conn, format!("{line}\n").as_bytes()).unwrap();
            }
        });

        let opener = RexSockOpener::new(RexSockClient::with_socket(sock), "h1").with_expected_started(Some(1000));
        let cancel = Arc::new(AtomicBool::new(false));

        let e1 = opener.verify_expected_started("qa", &cancel).unwrap_err();
        assert!(e1.starts_with(crate::stream::OPEN_LIVE_SCREEN_GONE_PREFIX), "missing host must fail closed with ScreenGone sentinel, got: {e1}");

        let e2 = opener.verify_expected_started("qa", &cancel).unwrap_err();
        assert!(e2.contains("disconnected") && !e2.starts_with(crate::stream::OPEN_LIVE_SCREEN_GONE_PREFIX), "disconnected host must fail closed without terminal ScreenGone, got: {e2}");

        let e3 = opener.verify_expected_started("qa", &cancel).unwrap_err();
        assert!(e3.contains("disconnected"), "reconnecting host must fail closed, got: {e3}");

        let e4 = opener.verify_expected_started("qa", &cancel).unwrap_err();
        assert!(e4.starts_with(crate::stream::OPEN_LIVE_SCREEN_GONE_PREFIX), "replaced session must fail closed with ScreenGone sentinel, got: {e4}");

        opener.verify_expected_started("qa", &cancel).expect("connected matching generation must succeed");

        srv.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Finding 38: invoked by E2E scripts (`--ignored`) to verify the REAL desktop
    /// `RexSockOpener` + `StreamScreenSource::export_video_cancellable` file-writing
    /// path against a live Rex / stand-in socket + real `olo-rex-link` helper.
    #[test]
    #[ignore]
    fn e2e_export_via_rex_sock_opener_env() {
        use crate::{
            ScreenSource as _,
            source::{ExportCancel, PaneCapabilities, ScreenId, ScreenInfo, ScreenState},
            stream::StreamScreenSource,
        };
        let host = std::env::var("OLO_TEST_EXPORT_HOST").expect("OLO_TEST_EXPORT_HOST");
        let name = std::env::var("OLO_TEST_EXPORT_NAME").expect("OLO_TEST_EXPORT_NAME");
        let out = PathBuf::from(std::env::var("OLO_TEST_EXPORT_OUT").expect("OLO_TEST_EXPORT_OUT"));
        let client = RexSockClient::new();
        let opener = Arc::new(RexSockOpener::new(client, &host));
        let info = ScreenInfo {
            id: ScreenId::host(&host, &host, &name),
            title: None,
            state: ScreenState::Running,
            size: Some((1280, 800)),
            scale: 1.0,
            sound: false,
            started_ms: None,
            last_used_ms: None,
            disconnected_at_ms: None,
            actions: None,
            controlled: false,
            capabilities: PaneCapabilities::remote_default(),
        };
        let src = StreamScreenSource::new(info, opener);
        let cancel = ExportCancel::new();
        let made = src.export_video_cancellable(None, 1, &out, &cancel, &mut |_| true).expect("StreamScreenSource export via RexSockOpener");
        let data = std::fs::read(&out).expect("exported MP4 file exists");
        assert!(data.len() > 512 && data[..64].windows(4).any(|w| w == b"ftyp"), "invalid MP4 ({} bytes)", data.len());
        assert!(made.frames > 0, "expected >=1 frame in exported MP4");
    }
}
