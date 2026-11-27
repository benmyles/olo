//! `olo-rex-link quic -- DIAL…`: the helper's own link over UDP
//! (`olo_rex_link::quic_wire` says what the app and it agree on).
//! Started over SSH by the app, it listens for QUIC and joins each
//! stream the app opens to a `DIAL` of its own (the host's Rex server,
//! as an SSH channel was joined to one), the control connection's
//! through the helper's gate. A terminal's flood then holds up nothing
//! but itself, a lost packet holds up one stream for a round trip
//! instead of every stream for a retransmission's wait, and the link
//! goes on through a change of the network under it.
//!
//! It lives while the SSH channel that started it is open or a link is
//! up, and waits on nothing else: no clock of its own.

use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use olo_rex_link::quic_wire as wire;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
#[cfg(feature = "screens")]
use tokio::sync::Semaphore;
use tokio::sync::{Notify, OwnedSemaphorePermit};

/// Deadline for reading a stream's token/kind header and `SCREEN` selector.
const SELECTOR_TIMEOUT: Duration = Duration::from_secs(5);

#[cfg(feature = "screens")]
struct ScreenLimits {
    streams: Arc<Semaphore>,
    exports: Arc<Semaphore>,
    follows: Arc<Semaphore>,
}

#[cfg(feature = "screens")]
impl ScreenLimits {
    fn from_env() -> Self {
        let cap = |var: &str, default: usize| std::env::var(var).ok().and_then(|v| v.parse::<usize>().ok()).filter(|&n| n > 0).unwrap_or(default);
        Self {
            streams: Arc::new(Semaphore::new(cap("OLO_QUIC_MAX_SCREEN_STREAMS", 8))),
            exports: Arc::new(Semaphore::new(cap("OLO_QUIC_MAX_SCREEN_EXPORTS", 2))),
            follows: Arc::new(Semaphore::new(cap("OLO_QUIC_MAX_SCREEN_FOLLOWS", 16))),
        }
    }

    fn try_acquire(&self, subcommand: &str) -> Option<OwnedSemaphorePermit> {
        let sem = match subcommand {
            "stream" => &self.streams,
            "export" => &self.exports,
            _ => &self.follows,
        };
        sem.clone().try_acquire_owned().ok()
    }
}

#[cfg(not(feature = "screens"))]
struct ScreenLimits;

#[cfg(not(feature = "screens"))]
impl ScreenLimits {
    fn from_env() -> Self {
        Self
    }
}

/// Resolve the currently running helper binary via `/proc/self/exe` on Linux so
/// spawning child modes keeps working even if Rex prunes or replaces the file on
/// disk while the QUIC helper stays up (Finding 20).
fn self_exe() -> Option<PathBuf> {
    let proc_self = Path::new("/proc/self/exe");
    if proc_self.exists() {
        return Some(proc_self.to_path_buf());
    }
    std::env::current_exe().ok()
}

pub fn run(mut args: Vec<String>) -> ! {
    if args.first().is_some_and(|a| a == "--") {
        args.remove(0);
    }
    if args.is_empty() {
        args = ["rex", "__internal", "dial-stdio"].map(str::to_owned).to_vec();
    }
    let code = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime.block_on(serve(args)).unwrap_or_else(|why| {
            eprintln!("olo-rex-link: {why}");
            1
        }),
        Err(e) => {
            eprintln!("olo-rex-link: {e}");
            1
        }
    };
    std::process::exit(code)
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    let text = text.trim();
    (text.len() % 2 == 0).then(|| (0..text.len()).step_by(2).map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok()).collect::<Option<Vec<u8>>>()).flatten()
}

/// Whether two tokens are one, in time that doesn't say where they differ.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

struct Life {
    /// The SSH channel that started the helper is still open.
    held: AtomicBool,
    /// The links that are up.
    links: AtomicUsize,
    changed: Notify,
}

/// Maximum byte length accepted for the initial hex token line on stdin
/// (2 * 32 hex chars + optional CR/LF + generous margin, never unbounded).
const MAX_TOKEN_LINE_BYTES: usize = 256;

async fn read_bounded_token_line<R: tokio::io::AsyncBufRead + Unpin>(input: &mut R) -> Result<String, String> {
    let mut buf = Vec::with_capacity(MAX_TOKEN_LINE_BYTES);
    loop {
        let avail = input.fill_buf().await.map_err(|e| e.to_string())?;
        if avail.is_empty() {
            break;
        }
        if let Some(nl) = avail.iter().position(|&b| b == b'\n') {
            if buf.len() + nl + 1 > MAX_TOKEN_LINE_BYTES {
                return Err("token line too long".into());
            }
            buf.extend_from_slice(&avail[..=nl]);
            input.consume(nl + 1);
            break;
        }
        let n = avail.len();
        if buf.len() + n > MAX_TOKEN_LINE_BYTES {
            return Err("token line too long".into());
        }
        buf.extend_from_slice(avail);
        input.consume(n);
    }
    String::from_utf8(buf).map_err(|_| "token line is not UTF-8".into())
}

async fn serve(dial: Vec<String>) -> Result<i32, String> {
    let mut input = BufReader::new(tokio::io::stdin());
    let line = read_bounded_token_line(&mut input).await?;
    let token = unhex(&line).filter(|t| t.len() == wire::TOKEN).ok_or("no token was given")?;

    let made = rcgen::generate_simple_self_signed(vec![wire::NAME.to_owned()]).map_err(|e| e.to_string())?;
    let certificate = made.cert.der().clone();
    let key = rustls::pki_types::PrivatePkcs8KeyDer::from(made.signing_key.serialize_der());
    let print: String = {
        use sha2::Digest as _;
        sha2::Sha256::digest(certificate.as_ref()).iter().map(|b| format!("{b:02x}")).collect()
    };
    let mut tls = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::aws_lc_rs::default_provider()))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| e.to_string())?
        .with_no_client_auth()
        .with_single_cert(vec![certificate], key.into())
        .map_err(|e| e.to_string())?;
    tls.alpn_protocols = vec![wire::ALPN.to_vec()];
    let mut config = quinn::ServerConfig::with_crypto(Arc::new(quinn::crypto::rustls::QuicServerConfig::try_from(tls).map_err(|e| e.to_string())?));
    let mut transport = quinn::TransportConfig::default();
    transport.max_idle_timeout(Some(wire::IDLE.try_into().map_err(|_| "the idle time")?));
    transport.max_concurrent_bidi_streams(1024u32.into());
    transport.max_concurrent_uni_streams(0u32.into());
    config.transport_config(Arc::new(transport));

    // One of its own ports, on every address of the host (both kinds
    // where the host has both); else any port.
    let endpoint = wire::PORTS
        .chain(std::iter::once(0))
        .find_map(|port| {
            quinn::Endpoint::server(config.clone(), SocketAddr::from((Ipv6Addr::UNSPECIFIED, port)))
                .or_else(|_| quinn::Endpoint::server(config.clone(), SocketAddr::from((Ipv4Addr::UNSPECIFIED, port))))
                .ok()
        })
        .ok_or("no UDP port could be had")?;
    let port = endpoint.local_addr().map_err(|e| e.to_string())?.port();
    let mut out = tokio::io::stdout();
    out.write_all(format!("{} {} {port} {print}\n", wire::ANNOUNCE, wire::VERSION).as_bytes()).await.map_err(|e| e.to_string())?;
    out.flush().await.map_err(|e| e.to_string())?;

    let life = Arc::new(Life { held: AtomicBool::new(true), links: AtomicUsize::new(0), changed: Notify::new() });
    // The SSH channel's end: discard any unexpected trailing bytes in a fixed
    // buffer until EOF so a faulty peer can never grow host memory.
    let held = life.clone();
    tokio::spawn(async move {
        let mut discard = [0u8; 4096];
        loop {
            match input.read(&mut discard).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
        held.held.store(false, Ordering::Relaxed);
        held.changed.notify_one();
    });
    let (token, dial, screen_limits) = (Arc::new(token), Arc::new(dial), Arc::new(ScreenLimits::from_env()));
    loop {
        tokio::select! {
            incoming = endpoint.accept() => {
                let Some(incoming) = incoming else { break };
                let (life, token, dial, screen_limits) = (life.clone(), token.clone(), dial.clone(), screen_limits.clone());
                tokio::spawn(async move {
                    let Ok(link) = incoming.await else { return };
                    life.links.fetch_add(1, Ordering::Relaxed);
                    while let Ok((send, recv)) = link.accept_bi().await {
                        tokio::spawn(stream(link.clone(), send, recv, token.clone(), dial.clone(), screen_limits.clone()));
                    }
                    life.links.fetch_sub(1, Ordering::Relaxed);
                    life.changed.notify_one();
                });
            }
            () = life.changed.notified() => {
                if !life.held.load(Ordering::Relaxed) && life.links.load(Ordering::Relaxed) == 0 {
                    break;
                }
            }
        }
    }
    endpoint.close(0u32.into(), b"");
    Ok(0)
}

#[cfg(feature = "screens")]
async fn send_screen_busy(send: &mut quinn::SendStream, subcommand: &str) {
    use olo_rex_link::screen_wire::{ExportMsg, HistoryMsg, ServerMsg, StreamStatus, encode_export_msg, encode_history_msg, encode_server_msg};
    let framed = match subcommand {
        "stream" => encode_server_msg(&ServerMsg::Status(StreamStatus { message: "Too many live screen streams are open on this host.".into(), reconnecting: false, error: true })).ok(),
        "export" => encode_export_msg(&ExportMsg::Error { message: "Too many screen video exports are running on this host.".into() }).ok(),
        "history" | "history-stream" => encode_history_msg(&HistoryMsg::Error { message: "Too many screen history streams are open on this host.".into() }).ok(),
        _ => None,
    };
    if let Some((tag, payload)) = framed {
        let mut head = [0u8; 5];
        head[0] = tag;
        head[1..5].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        let _ = send.write_all(&head).await;
        let _ = send.write_all(&payload).await;
        let _ = send.finish();
    } else {
        let _ = send.reset(4u32.into());
    }
}

/// Wait up to `timeout` for `wait_fut` (child reaping) to finish; if the child
/// is still un-reaped (e.g. stuck in D-state), detach a background reaper that
/// keeps the admission `permit` held until `wait_fut` actually finishes (Finding 28).
#[cfg(feature = "screens")]
async fn reap_or_retain_permit<F>(wait_fut: F, permit: Option<OwnedSemaphorePermit>, timeout: Duration)
where
    F: std::future::Future<Output = std::io::Result<std::process::ExitStatus>> + Send + 'static,
{
    let mut wait_fut = Box::pin(wait_fut);
    if matches!(tokio::time::timeout(timeout, &mut wait_fut).await, Ok(Ok(_))) {
        return;
    }
    tokio::spawn(async move {
        let _permit = permit;
        let _ = wait_fut.await;
    });
}

/// Gracefully supervise a screen child through stdin EOF, `SIGTERM`, bounded
/// cleanup wait, and `SIGKILL` fallback, ALWAYS holding the admission permit
/// until the child is actually reaped (Findings 7 & 28).
#[cfg(feature = "screens")]
async fn terminate_and_reap_screen_child(mut child: tokio::process::Child, permit: Option<OwnedSemaphorePermit>) {
    // Brief grace for clean stdin-EOF exit first.
    if matches!(tokio::time::timeout(Duration::from_millis(300), child.wait()).await, Ok(Ok(_))) {
        return;
    }
    if let Some(pid) = child.id() {
        // SAFETY: `child` has not been reaped yet, so `pid` still refers to our
        // direct child. `SIGTERM` lets `screen stream` release held inputs/`.control`
        // and `screen export` tear down its entire process group cleanly.
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGTERM);
        }
    }
    if matches!(tokio::time::timeout(Duration::from_secs(2), child.wait()).await, Ok(Ok(_))) {
        return;
    }
    let _ = child.start_kill();
    reap_or_retain_permit(async move { child.wait().await }, permit, Duration::from_secs(1)).await;
}

/// One stream of the app's: its token and kind, then joined to what it
/// asked for until either side ends.
async fn stream(link: quinn::Connection, mut send: quinn::SendStream, mut recv: quinn::RecvStream, token: Arc<Vec<u8>>, dial: Arc<Vec<String>>, screen_limits: Arc<ScreenLimits>) {
    let mut head = [0u8; wire::TOKEN + 1];
    let Ok(Ok(())) = tokio::time::timeout(SELECTOR_TIMEOUT, recv.read_exact(&mut head)).await else {
        let _ = send.reset(2u32.into());
        return;
    };
    if !same(&head[..wire::TOKEN], &token) {
        // (Not the app: the whole link goes.)
        link.close(1u32.into(), b"");
        return;
    }
    let is_screen = head[wire::TOKEN] == wire::SCREEN;
    let mut _screen_permit: Option<OwnedSemaphorePermit> = None;
    let _ = &screen_limits;
    let mut command = match head[wire::TOKEN] {
        wire::HELLO => {
            let _ = send.write_all(&[1]).await;
            let _ = send.finish();
            return;
        }
        wire::PLAIN => {
            let mut command = tokio::process::Command::new(&dial[0]);
            command.args(&dial[1..]);
            command
        }
        // (Through the helper's gate, as over SSH: this program again.)
        wire::CONTROL => {
            let _ = send.set_priority(1);
            let Some(me) = self_exe() else { return };
            let mut command = tokio::process::Command::new(me);
            command.arg("--").args(dial.iter());
            command
        }
        // Host Agent Screens stream (`follow`, `stream NAME`, …), strictly
        // below interactive terminals (0) and control (1) so frame bursts
        // never delay typing or RPCs.
        wire::SCREEN => {
            #[cfg(not(feature = "screens"))]
            {
                let _ = send.reset(3u32.into());
                return;
            }
            #[cfg(feature = "screens")]
            {
                let _ = send.set_priority(-1);
                let mut len_bytes = [0u8; 2];
                let Ok(Ok(())) = tokio::time::timeout(SELECTOR_TIMEOUT, recv.read_exact(&mut len_bytes)).await else {
                    let _ = send.reset(2u32.into());
                    return;
                };
                let len = u16::from_le_bytes(len_bytes);
                if len == 0 || len > olo_rex_link::screen_wire::MAX_QUIC_SCREEN_ARGS_LEN {
                    let _ = send.reset(2u32.into());
                    return;
                }
                let mut body = vec![0u8; len as usize];
                let Ok(Ok(())) = tokio::time::timeout(SELECTOR_TIMEOUT, recv.read_exact(&mut body)).await else {
                    let _ = send.reset(2u32.into());
                    return;
                };
                let Ok(mut args) = olo_rex_link::screen_wire::decode_quic_screen_args(&body) else {
                    let _ = send.reset(2u32.into());
                    return;
                };
                let Some(permit) = screen_limits.try_acquire(&args[0]) else {
                    send_screen_busy(&mut send, &args[0]).await;
                    return;
                };
                _screen_permit = Some(permit);
                // Map QUIC `history NAME` to the explicit binary wire verb
                // `history-stream NAME` (Finding 12).
                if args[0] == "history" {
                    args[0] = "history-stream".to_owned();
                }
                let Some(me) = self_exe() else {
                    let _ = send.reset(2u32.into());
                    return;
                };
                let mut command = tokio::process::Command::new(me);
                command.arg("screen").args(args);
                command
            }
        }
        _ => return,
    };
    let Ok(mut child) = command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true).spawn() else {
        let _ = send.reset(2u32.into());
        return;
    };
    let (Some(mut to_server), Some(mut from_server)) = (child.stdin.take(), child.stdout.take()) else { return };

    #[cfg(feature = "screens")]
    if is_screen {
        // Screen relay: observe peer send-stream reset independently of child
        // output, and bound how long a stalled download may linger after client
        // stdin EOF so cancellation can never hang indefinitely (Finding 7).
        let up = async move {
            let _ = tokio::io::copy(&mut recv, &mut to_server).await;
            drop(to_server);
            // Allow normal final drain after stdin EOF, then force teardown if
            // the client stopped reading the download half.
            tokio::time::sleep(Duration::from_secs(2)).await;
        };
        let down = async {
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                tokio::select! {
                    stopped = send.stopped() => {
                        let _ = stopped;
                        break;
                    }
                    read_res = from_server.read(&mut buf) => {
                        match read_res {
                            Ok(0) | Err(_) => {
                                let _ = send.finish();
                                break;
                            }
                            Ok(n) => {
                                if send.write_all(&buf[..n]).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        };
        tokio::select! {
            () = up => {}
            () = down => {}
        }
        terminate_and_reap_screen_child(child, _screen_permit.take()).await;
        return;
    }

    let _ = is_screen;
    // What the app sends, to the server, until the app ends its side
    // (then the server's input ends, and it ends in its turn).
    let up = async {
        let _ = tokio::io::copy(&mut recv, &mut to_server).await;
        drop(to_server);
        std::future::pending::<()>().await
    };
    // What the server sends, to the app, until the server ends: then
    // the stream does.
    let down = async {
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            match from_server.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if send.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                }
            }
        }
        let _ = send.finish();
    };
    tokio::select! {
        () = up => {}
        () = down => {}
    }
}

#[cfg(all(test, feature = "screens"))]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt as _;

    /// Finding 28: when a child wait outlives the bounded teardown timeout,
    /// the admission permit MUST stay held by a background reaper until the
    /// child is actually reaped!
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn review2_w2_reap_or_retain_holds_permit_until_child_exits() {
        let sem = Arc::new(Semaphore::new(1));
        let permit = sem.clone().try_acquire_owned().unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel::<std::io::Result<std::process::ExitStatus>>();
        reap_or_retain_permit(async move { rx.await.unwrap_or_else(|_| Ok(std::process::ExitStatus::from_raw(0))) }, Some(permit), Duration::from_millis(30)).await;

        // Timeout elapsed, child still un-reaped -> permit MUST still be held!
        assert_eq!(sem.available_permits(), 0, "Finding 28: permit dropped while child still un-reaped");

        // Once the child finishes and is reaped, the background reaper drops the permit.
        let _ = tx.send(Ok(std::process::ExitStatus::from_raw(0)));
        let until = std::time::Instant::now() + Duration::from_secs(2);
        while sem.available_permits() == 0 && std::time::Instant::now() < until {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(sem.available_permits(), 1, "permit should return once background reaper finishes");

        // Also verify real child escalation (SIGTERM-ignoring child -> SIGKILL -> reaped + permit returned).
        let permit2 = sem.clone().try_acquire_owned().unwrap();
        let child =
            tokio::process::Command::new("/bin/sh").args(["-c", "trap '' TERM; exec sleep 30"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).kill_on_drop(true).spawn().unwrap();
        terminate_and_reap_screen_child(child, Some(permit2)).await;
        assert_eq!(sem.available_permits(), 1, "SIGTERM-ignoring child should be reaped and return its permit");
    }
}
