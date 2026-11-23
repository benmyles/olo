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
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use olo_rex_link::quic_wire as wire;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Notify;

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

async fn serve(dial: Vec<String>) -> Result<i32, String> {
    let mut input = BufReader::new(tokio::io::stdin());
    let mut line = String::new();
    input.read_line(&mut line).await.map_err(|e| e.to_string())?;
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
    // The SSH channel's end: read to its end, then said.
    let held = life.clone();
    tokio::spawn(async move {
        let mut rest = Vec::new();
        let _ = input.read_to_end(&mut rest).await;
        held.held.store(false, Ordering::Relaxed);
        held.changed.notify_one();
    });
    let (token, dial) = (Arc::new(token), Arc::new(dial));
    loop {
        tokio::select! {
            incoming = endpoint.accept() => {
                let Some(incoming) = incoming else { break };
                let (life, token, dial) = (life.clone(), token.clone(), dial.clone());
                tokio::spawn(async move {
                    let Ok(link) = incoming.await else { return };
                    life.links.fetch_add(1, Ordering::Relaxed);
                    while let Ok((send, recv)) = link.accept_bi().await {
                        tokio::spawn(stream(link.clone(), send, recv, token.clone(), dial.clone()));
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

/// One stream of the app's: its token and kind, then joined to what it
/// asked for until either side ends.
async fn stream(link: quinn::Connection, mut send: quinn::SendStream, mut recv: quinn::RecvStream, token: Arc<Vec<u8>>, dial: Arc<Vec<String>>) {
    let mut head = [0u8; wire::TOKEN + 1];
    if recv.read_exact(&mut head).await.is_err() {
        return;
    }
    if !same(&head[..wire::TOKEN], &token) {
        // (Not the app: the whole link goes.)
        link.close(1u32.into(), b"");
        return;
    }
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
            let Ok(me) = std::env::current_exe() else { return };
            let mut command = tokio::process::Command::new(me);
            command.arg("--").args(dial.iter());
            command
        }
        _ => return,
    };
    let Ok(mut child) = command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true).spawn() else {
        let _ = send.reset(2u32.into());
        return;
    };
    let (Some(mut to_server), Some(mut from_server)) = (child.stdin.take(), child.stdout.take()) else { return };
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
