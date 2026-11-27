//! The helper's own link over UDP, from the app's side (`olo_rex_link::
//! quic_wire` says what the two agree on; the helper's `quic` mode is
//! the other end). SSH signs in and starts the helper, hands it a
//! token made for this link and takes back the port it listens on and
//! its certificate's print; from then on the streams to the host's Rex
//! server (the control connection, each terminal's) are streams of one
//! QUIC connection, not channels of the SSH connection:
//! - a terminal's flood holds up only itself (each stream has its own
//!   window; over SSH one TCP connection carries all, in order);
//! - a lost packet holds up the stream it was of for a round trip, not
//!   every stream for a retransmission's wait;
//! - the link goes on through a change of the network under it.
//!
//! Where UDP doesn't get through (a firewall, a proxy between), the
//! link isn't made and everything goes over SSH as before: the app's
//! streams never wait on it for longer than [`WITHIN`], once.

use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use olo_rex_link::quic_wire as wire;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use tokio::io::{AsyncRead, ReadBuf};

use crate::transport::AsyncReadWrite;

/// How long the link is given to come up (its handshake): past it,
/// SSH.
pub const WITHIN: Duration = Duration::from_millis(2500);

/// Maximum time to acquire a QUIC bidirectional stream, write its selector,
/// and (for screen streams) receive the host helper's initial reply byte
/// (Findings 12 & 13).
pub const STREAM_OPEN_TIMEOUT: Duration = Duration::from_secs(10);

/// Typed failure returned by [`QuicLink::open_screen_checked`] so
/// [`crate::ssh::SshSessionPool`] can distinguish argument/stream-level
/// refusals, pre-dispatch credit timeouts, and post-dispatch startup timeouts
/// from a dead QUIC connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum QuicScreenError {
    InvalidArgs(String),
    /// Helper built without `screens` reset the `SCREEN` stream with code 3.
    Unsupported,
    /// Helper reset the `SCREEN` stream with another application code.
    Refused(u64),
    /// Helper closed stdout (`FIN`) before writing a single reply byte (e.g.
    /// pre-SCREEN helper or startup failure whose stderr was discarded).
    EmptyStartup,
    /// Timed out BEFORE the screen request could be written (e.g. QUIC bidi
    /// stream credits exhausted): nothing was dispatched on the host yet, so
    /// falling back to SSH cannot run the job twice.
    OpenTimeout,
    /// Timed out AFTER the screen request was written while waiting for the
    /// helper's first reply byte: the host job may already be running, so the
    /// caller must NOT blindly re-dispatch it over SSH.
    StartupTimeout,
    /// Underlying QUIC transport / connection error.
    Transport(String),
}

impl std::fmt::Display for QuicScreenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidArgs(msg) | Self::Transport(msg) => f.write_str(msg),
            Self::Unsupported => f.write_str("remote QUIC helper does not support screens"),
            Self::Refused(code) => write!(f, "remote screen stream refused (code {code})"),
            Self::EmptyStartup => f.write_str("remote screen helper ended before replying"),
            Self::OpenTimeout => f.write_str("timed out opening QUIC screen stream"),
            Self::StartupTimeout => f.write_str("timed out waiting for remote screen helper startup"),
        }
    }
}

/// `quinn::RecvStream` preceded by the non-empty initial startup chunk already
/// read to confirm the host screen child started cleanly (Finding 12).
pub(crate) struct PrefixedRecv {
    prefix: Option<(Bytes, usize)>,
    recv: quinn::RecvStream,
}

impl PrefixedRecv {
    fn new(first: Bytes, recv: quinn::RecvStream) -> Self {
        Self { prefix: Some((first, 0)), recv }
    }
}

impl AsyncRead for PrefixedRecv {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        if let Some((data, idx)) = this.prefix.as_mut() {
            let n = buf.remaining().min(data.len() - *idx);
            buf.put_slice(&data[*idx..*idx + n]);
            *idx += n;
            if *idx == data.len() {
                this.prefix = None;
            }
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut this.recv).poll_read(cx, buf)
    }
}

/// The helper's certificate is the one whose print SSH brought, and no
/// other: nobody else is asked.
#[derive(Debug)]
struct Pinned {
    print: String,
    provider: Arc<rustls::crypto::CryptoProvider>,
}

pub fn print_of(certificate: &[u8]) -> String {
    use sha2::Digest as _;
    sha2::Sha256::digest(certificate).iter().map(|b| format!("{b:02x}")).collect()
}

impl rustls::client::danger::ServerCertVerifier for Pinned {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        if print_of(end_entity.as_ref()) == self.print { Ok(rustls::client::danger::ServerCertVerified::assertion()) } else { Err(rustls::Error::General("not the helper's certificate".into())) }
    }

    fn verify_tls12_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &rustls::DigitallySignedStruct) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &rustls::DigitallySignedStruct) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

/// A link that is up.
pub struct QuicLink {
    connection: quinn::Connection,
    token: [u8; wire::TOKEN],
    /// Local counter of `open_bi()` streams allocated on this connection
    /// (for tests verifying locally refused calls allocate zero streams).
    opened_streams: AtomicU64,
    /// (Its socket lives as long as the link.)
    _endpoint: quinn::Endpoint,
}

impl std::fmt::Debug for QuicLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QuicLink").field("to", &self.connection.remote_address()).finish_non_exhaustive()
    }
}

/// A token for one link: what the helper is given over SSH, and what
/// every stream then begins with.
pub fn new_token() -> Result<[u8; wire::TOKEN], String> {
    let mut token = [0u8; wire::TOKEN];
    rustls::crypto::aws_lc_rs::default_provider().secure_random.fill(&mut token).map_err(|_| "no randomness".to_owned())?;
    Ok(token)
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// What the helper said as it began to listen ([`wire::ANNOUNCE`]):
/// its port, and its certificate's print.
pub fn announced(line: &str) -> Option<(u16, String)> {
    let mut words = line.split_whitespace();
    (words.next()? == wire::ANNOUNCE && words.next()?.parse::<u32>().ok()? == wire::VERSION).then_some(())?;
    let port = words.next()?.parse().ok()?;
    let print = words.next()?;
    (print.len() == 64 && print.bytes().all(|b| b.is_ascii_hexdigit())).then(|| (port, print.to_owned()))
}

impl QuicLink {
    /// The link to the helper at `to`, whose certificate has `print`:
    /// made (its handshake, which needs UDP both ways) within
    /// [`WITHIN`].
    pub async fn connect(to: SocketAddr, print: &str, token: [u8; wire::TOKEN]) -> Result<QuicLink, String> {
        let up = async {
            let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
            let mut tls = rustls::ClientConfig::builder_with_provider(provider.clone())
                .with_protocol_versions(&[&rustls::version::TLS13])
                .map_err(|e| e.to_string())?
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(Pinned { print: print.to_owned(), provider }))
                .with_no_client_auth();
            tls.alpn_protocols = vec![wire::ALPN.to_vec()];
            let mut config = quinn::ClientConfig::new(Arc::new(quinn::crypto::rustls::QuicClientConfig::try_from(tls).map_err(|e| e.to_string())?));
            let mut transport = quinn::TransportConfig::default();
            transport.max_idle_timeout(Some(wire::IDLE.try_into().map_err(|_| "the idle time")?));
            transport.keep_alive_interval(Some(wire::KEEP_ALIVE));
            config.transport_config(Arc::new(transport));
            let from: SocketAddr = if to.is_ipv6() { (std::net::Ipv6Addr::UNSPECIFIED, 0).into() } else { (std::net::Ipv4Addr::UNSPECIFIED, 0).into() };
            let endpoint = quinn::Endpoint::client(from).map_err(|e| e.to_string())?;
            let connection = endpoint.connect_with(config, to, wire::NAME).map_err(|e| e.to_string())?.await.map_err(|e| e.to_string())?;
            // (The handshake has gone both ways over UDP: the path
            // carries it. No word more is waited for: a round trip
            // saved at every connection.)
            Ok::<_, String>(QuicLink { connection, token, opened_streams: AtomicU64::new(0), _endpoint: endpoint })
        };
        tokio::time::timeout(WITHIN, up).await.map_err(|_| format!("no answer over UDP from {to} in {} ms", WITHIN.as_millis()))?
    }

    async fn begin(&self, kind: u8) -> Result<(quinn::SendStream, quinn::RecvStream), String> {
        let up = async {
            let (mut send, mut recv) = self.connection.open_bi().await.map_err(|e| e.to_string())?;
            self.opened_streams.fetch_add(1, Ordering::Relaxed);
            let mut head = [0u8; wire::TOKEN + 1];
            head[..wire::TOKEN].copy_from_slice(&self.token);
            head[wire::TOKEN] = kind;
            if let Err(e) = send.write_all(&head).await {
                let _ = send.reset(0u32.into());
                let _ = recv.stop(0u32.into());
                return Err(e.to_string());
            }
            Ok((send, recv))
        };
        tokio::time::timeout(STREAM_OPEN_TIMEOUT, up).await.map_err(|_| format!("timed out opening QUIC stream in {} ms", STREAM_OPEN_TIMEOUT.as_millis()))?
    }

    async fn open_halves(&self, control: bool) -> Result<(quinn::RecvStream, quinn::SendStream), String> {
        let (send, recv) = self.begin(if control { wire::CONTROL } else { wire::PLAIN }).await?;
        if control {
            let _ = send.set_priority(1);
        }
        Ok((recv, send))
    }

    /// A stream to the host's Rex server: through the helper's gate
    /// (`control`: the control connection, sent ahead of the others'
    /// bytes where both wait), or straight.
    pub async fn open(&self, control: bool) -> Result<Pin<Box<dyn AsyncReadWrite + Send>>, String> {
        let (recv, send) = self.open_halves(control).await?;
        Ok(Box::pin(tokio::io::join(recv, send)))
    }

    /// A host Agent Screens stream (`olo-rex-link screen ARGS…`, e.g.
    /// `follow`, `stream NAME`, `history-stream NAME`, `export NAME …`).
    ///
    /// Arguments are validated and encoded with
    /// [`olo_rex_link::screen_wire::encode_quic_screen_args`] before any
    /// QUIC stream is opened or byte is sent: disallowed arguments are
    /// refused here with the validator's own words. Its send priority is
    /// set to `-1` (strictly below interactive terminals at `0` and the
    /// control connection at `1`), matching the helper's side.
    ///
    /// Separates pre-dispatch stream credit/write timeout (`OpenTimeout`, safe
    /// to fall back to SSH) from post-dispatch first-byte startup timeout
    /// (`StartupTimeout`, where the host job may already be running and must
    /// not be blindly re-dispatched).
    ///
    /// Dropping the returned stream stops receiving (`STOP_SENDING`) and
    /// finishes sending (`FIN`, or resets if already stopped) via
    /// `quinn::RecvStream::drop` and `quinn::SendStream::drop`, ending the
    /// helper's screen child process promptly on the host.
    async fn open_screen_halves_within(&self, args: &[String], within: Duration) -> Result<(PrefixedRecv, quinn::SendStream), QuicScreenError> {
        let selector = olo_rex_link::screen_wire::encode_quic_screen_args(args).map_err(|e| QuicScreenError::InvalidArgs(e.to_string()))?;
        let deadline = tokio::time::Instant::now() + within;

        // Guard ensuring cancellation or timeout during setup immediately
        // sends RESET_STREAM + STOP_SENDING rather than leaving an orphan job.
        struct SetupGuard(Option<(quinn::SendStream, quinn::RecvStream)>);
        impl Drop for SetupGuard {
            fn drop(&mut self) {
                if let Some((mut send, mut recv)) = self.0.take() {
                    let _ = send.reset(0u32.into());
                    let _ = recv.stop(0u32.into());
                }
            }
        }

        // Phase 1 (pre-dispatch): acquire bidi stream credit and write header.
        let mut guard = tokio::time::timeout_at(deadline, async {
            let (send, recv) = self.connection.open_bi().await.map_err(|e| QuicScreenError::Transport(e.to_string()))?;
            self.opened_streams.fetch_add(1, Ordering::Relaxed);
            let _ = send.set_priority(-1);
            let mut guard = SetupGuard(Some((send, recv)));
            let mut head = Vec::with_capacity(wire::TOKEN + 1 + selector.len());
            head.extend_from_slice(&self.token);
            head.push(wire::SCREEN);
            head.extend_from_slice(&selector);
            if let Err(e) = guard.0.as_mut().expect("streams").0.write_all(&head).await {
                return Err(QuicScreenError::Transport(e.to_string()));
            }
            Ok(guard)
        })
        .await
        .map_err(|_| QuicScreenError::OpenTimeout)??;

        // Phase 2 (post-dispatch): confirm the host helper started and emitted
        // at least one byte (or surfaced Reset(3) / empty FIN).
        let first_chunk = tokio::time::timeout_at(deadline, async {
            let mut first = [0u8; 4096];
            loop {
                match guard.0.as_mut().expect("streams").1.read(&mut first).await {
                    Ok(Some(0)) => continue,
                    Ok(Some(n)) => return Ok(Bytes::copy_from_slice(&first[..n])),
                    Ok(None) => return Err(QuicScreenError::EmptyStartup),
                    Err(quinn::ReadError::Reset(code)) => {
                        let raw = code.into_inner();
                        return Err(if raw == 3 { QuicScreenError::Unsupported } else { QuicScreenError::Refused(raw) });
                    }
                    Err(e) => return Err(QuicScreenError::Transport(e.to_string())),
                }
            }
        })
        .await
        .map_err(|_| QuicScreenError::StartupTimeout)??;

        let (send, recv) = guard.0.take().expect("streams");
        Ok((PrefixedRecv::new(first_chunk, recv), send))
    }

    pub(crate) async fn open_screen_checked(&self, args: &[String], within: Duration) -> Result<Pin<Box<dyn AsyncReadWrite + Send>>, QuicScreenError> {
        let (recv, send) = self.open_screen_halves_within(args, within).await?;
        Ok(Box::pin(tokio::io::join(recv, send)))
    }

    pub async fn open_screen(&self, args: &[String]) -> Result<Pin<Box<dyn AsyncReadWrite + Send>>, String> {
        self.open_screen_checked(args, STREAM_OPEN_TIMEOUT).await.map_err(|e| e.to_string())
    }

    /// Open a screen stream with a custom setup timeout (for tests exercising
    /// bounded setup waits under stream-credit exhaustion, Finding 13).
    #[doc(hidden)]
    pub async fn open_screen_within(&self, args: &[String], within: Duration) -> Result<Pin<Box<dyn AsyncReadWrite + Send>>, String> {
        self.open_screen_checked(args, within).await.map_err(|e| e.to_string())
    }

    /// Count of QUIC STREAM frames transmitted on this connection (for tests
    /// verifying zero wire frames on locally refused requests).
    #[doc(hidden)]
    pub fn sent_stream_frames(&self) -> u64 {
        self.connection.stats().frame_tx.stream
    }

    /// Local count of bidirectional streams allocated via `open_bi()` on this
    /// connection (catches allocations even before any STREAM frame is flushed).
    #[doc(hidden)]
    pub fn opened_streams(&self) -> u64 {
        self.opened_streams.load(Ordering::Relaxed)
    }

    /// Close the underlying QUIC connection (for tests verifying link discard).
    #[doc(hidden)]
    pub fn close_now(&self) {
        self.connection.close(0u32.into(), b"");
    }

    /// A word asked of the helper and had back: the helper takes this
    /// link's token (one that doesn't closes the link).
    pub async fn hello(&self) -> Result<(), String> {
        let (mut send, mut recv) = self.begin(wire::HELLO).await?;
        let _ = send.finish();
        let mut word = [0u8; 1];
        recv.read_exact(&mut word).await.map_err(|e| e.to_string())
    }

    /// The link's round trip, as its own packets have measured it.
    pub fn round_trip(&self) -> Duration {
        self.connection.rtt()
    }

    /// Whether it is still up.
    pub fn alive(&self) -> bool {
        self.connection.close_reason().is_none()
    }
}

/// Locate the Cargo profile directory (`<target>/<profile>`) owning `exe`,
/// without ever climbing past the target directory into shared directories
/// like `/var/tmp` (Finding 4).
#[doc(hidden)]
pub fn cargo_profile_dir(exe: &std::path::Path) -> Option<std::path::PathBuf> {
    // Normal Cargo: `<target>/<profile>/deps/<bin>`
    // Jato build-dir: `<target>/<profile>/build/<pkg>/<hash>/out/<bin>`
    for anc in exe.ancestors().skip(1).take(5) {
        let Some(name) = anc.file_name().and_then(|n| n.to_str()) else { continue };
        if name == "deps" || name == "build" {
            let profile = anc.parent()?;
            let pname = profile.file_name().and_then(|n| n.to_str())?;
            if matches!(pname, "debug" | "release") {
                return Some(profile.to_path_buf());
            }
            return None;
        }
    }
    None
}

/// Locate the freshly built `olo-rex-link` binary from either explicit
/// `OLO_REX_LINK_BIN` or `exe`'s own Cargo profile artifact directory
/// (`<target>/<profile>/olo-rex-link`), never executing binaries outside the
/// build profile (Finding 4).
#[doc(hidden)]
pub fn find_test_helper(exe: &std::path::Path) -> Option<std::path::PathBuf> {
    let explicit = std::env::var_os("OLO_REX_LINK_BIN").map(std::path::PathBuf::from);
    let required = std::env::var_os("OLO_REX_LINK_REQUIRED").is_some();
    find_test_helper_with(exe, explicit.as_deref(), required)
}

#[doc(hidden)]
pub fn find_test_helper_with(exe: &std::path::Path, explicit: Option<&std::path::Path>, required: bool) -> Option<std::path::PathBuf> {
    use std::process::{Command, Stdio};

    let has_quic_and_screens = |bin: &std::path::Path| -> bool {
        let Ok(proto) = Command::new(bin).args(["screen", "--protocol"]).env("PATH", "/dev/null").stdin(Stdio::null()).output() else { return false };
        let stdout = String::from_utf8_lossy(&proto.stdout);
        let has_screens = proto.status.success() && stdout.contains("\"proto\":") && stdout.contains("\"follow\"");
        let Ok(quic) = Command::new(bin).arg("quic").env("PATH", "/dev/null").stdin(Stdio::null()).output() else { return false };
        let has_quic = quic.status.code() != Some(126) && !String::from_utf8_lossy(&quic.stderr).contains("built without");
        has_screens && has_quic
    };

    if let Some(explicit) = explicit {
        assert!(explicit.is_file(), "OLO_REX_LINK_BIN={} does not exist", explicit.display());
        assert!(has_quic_and_screens(explicit), "OLO_REX_LINK_BIN={} lacks quic/screens features", explicit.display());
        return Some(explicit.to_path_buf());
    }
    let candidate = cargo_profile_dir(exe).map(|d| d.join("olo-rex-link")).filter(|p| p.is_file());
    if let Some(bin) = candidate
        && has_quic_and_screens(&bin)
    {
        return Some(bin);
    }
    assert!(
        !required,
        "OLO_REX_LINK_REQUIRED is set but no quic+screens `olo-rex-link` exists in {}",
        cargo_profile_dir(exe).map_or_else(|| "<unknown profile>".to_owned(), |d| d.display().to_string())
    );
    eprintln!("SKIP: olo-rex-link (quic,screens) not built in current Cargo profile (run `jato build -p olo-rex-link --features quic,screens` or set OLO_REX_LINK_BIN)");
    None
}

/// Generate a self-signed Ed25519 X.509 v3 DER certificate and PKCS#8 v1 DER
/// private key entirely in-process (zero external `openssl` dependency) for
/// deterministic QUIC peer tests, returning `(cert_der, pkcs8_key_der, sha256_print)`.
#[doc(hidden)]
pub fn make_test_quic_cert() -> (Vec<u8>, Vec<u8>, String) {
    fn der_tlv(tag: u8, body: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(4 + body.len());
        out.push(tag);
        if body.len() < 128 {
            out.push(body.len() as u8);
        } else if body.len() < 256 {
            out.push(0x81);
            out.push(body.len() as u8);
        } else {
            out.push(0x82);
            out.push((body.len() >> 8) as u8);
            out.push((body.len() & 0xff) as u8);
        }
        out.extend_from_slice(body);
        out
    }

    let seed = new_token().expect("secure random for test Ed25519 key");
    let kp = russh::keys::ssh_key::private::Ed25519Keypair::from_seed(&seed);
    let pub_bytes = kp.public.as_ref();

    // RFC 8410 Ed25519 AlgorithmIdentifier: SEQUENCE { OID 1.3.101.112 }
    let ed25519_alg = [0x30u8, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70];
    // RFC 8410 PKCS#8 v1 PrivateKeyInfo for Ed25519 (48 bytes).
    let mut pkcs8 = Vec::with_capacity(48);
    pkcs8.extend_from_slice(&[0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20]);
    pkcs8.extend_from_slice(&seed);

    let cn_name = [0x30u8, 0x14, 0x31, 0x12, 0x30, 0x10, 0x06, 0x03, 0x55, 0x04, 0x03, 0x0c, 0x09, b'l', b'o', b'c', b'a', b'l', b'h', b'o', b's', b't'];
    let mut validity_body = Vec::new();
    validity_body.extend_from_slice(&der_tlv(0x17, b"200101000000Z"));
    validity_body.extend_from_slice(&der_tlv(0x17, b"491231235959Z"));
    let validity = der_tlv(0x30, &validity_body);

    let mut spki_body = Vec::new();
    spki_body.extend_from_slice(&ed25519_alg);
    let mut pub_bits = Vec::with_capacity(33);
    pub_bits.push(0x00); // 0 unused bits
    pub_bits.extend_from_slice(pub_bytes);
    spki_body.extend_from_slice(&der_tlv(0x03, &pub_bits));
    let spki = der_tlv(0x30, &spki_body);

    let mut tbs_body = Vec::new();
    tbs_body.extend_from_slice(&[0xa0, 0x03, 0x02, 0x01, 0x02]); // version v3
    tbs_body.extend_from_slice(&[0x02, 0x01, 0x01]); // serialNumber = 1
    tbs_body.extend_from_slice(&ed25519_alg); // signature alg
    tbs_body.extend_from_slice(&cn_name); // issuer
    tbs_body.extend_from_slice(&validity); // validity
    tbs_body.extend_from_slice(&cn_name); // subject
    tbs_body.extend_from_slice(&spki); // subjectPublicKeyInfo
    let tbs = der_tlv(0x30, &tbs_body);

    let provider = rustls::crypto::aws_lc_rs::default_provider();
    let signing_key = provider.key_provider.load_private_key(rustls::pki_types::PrivateKeyDer::Pkcs8(pkcs8.clone().into())).expect("load Ed25519 PKCS#8");
    let signer = signing_key.choose_scheme(&[rustls::SignatureScheme::ED25519]).expect("Ed25519 scheme");
    let sig = signer.sign(&tbs).expect("sign TBSCertificate");

    let mut cert_body = Vec::with_capacity(tbs.len() + ed25519_alg.len() + sig.len() + 4);
    cert_body.extend_from_slice(&tbs);
    cert_body.extend_from_slice(&ed25519_alg);
    let mut sig_bits = Vec::with_capacity(1 + sig.len());
    sig_bits.push(0x00);
    sig_bits.extend_from_slice(&sig);
    cert_body.extend_from_slice(&der_tlv(0x03, &sig_bits));
    let cert_der = der_tlv(0x30, &cert_body);
    let print = print_of(&cert_der);
    (cert_der, pkcs8, print)
}

/// Create an exclusively owned temporary directory under `/var/tmp` via
/// `mkdtemp(3)` so concurrent runs / PID namespaces can never collide or
/// delete each other's fixtures, cleaned up automatically on drop.
#[doc(hidden)]
pub struct TestTempDir(std::path::PathBuf);

impl TestTempDir {
    pub fn new(prefix: &str) -> Self {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStringExt as _;
        let template = format!("/var/tmp/{prefix}-XXXXXX");
        let mut bytes = CString::new(template).expect("NUL in temp prefix").into_bytes_with_nul();
        // SAFETY: `bytes` is a NUL-terminated mutable buffer ending in 6 `X`s.
        let ptr = unsafe { libc::mkdtemp(bytes.as_mut_ptr().cast()) };
        assert!(!ptr.is_null(), "mkdtemp({prefix}) failed: {}", io::Error::last_os_error());
        bytes.pop(); // strip trailing NUL
        Self(std::path::PathBuf::from(std::ffi::OsString::from_vec(bytes)))
    }

    pub fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TestTempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_helpers_word_is_read() {
        let print = "ab".repeat(32);
        assert_eq!(announced(&format!("olo-rex-quic 1 61100 {print}\n")), Some((61100, print.clone())));
        // Another version of the wire, a print that isn't one, another word: no link.
        assert_eq!(announced(&format!("olo-rex-quic 2 61100 {print}")), None);
        assert_eq!(announced("olo-rex-quic 1 61100 abc"), None);
        assert_eq!(announced("rex: command not found"), None);
        assert_eq!(hex(&[0, 255, 16]), "00ff10");
        assert_ne!(new_token().unwrap(), new_token().unwrap());
    }

    /// Finding 4: helper discovery stays strictly inside the Cargo profile
    /// directory and NEVER executes an unrelated `olo-rex-link` in a shared
    /// ancestor directory like `/var/tmp`.
    #[test]
    fn find_test_helper_never_executes_ancestor_binary() {
        use std::os::unix::fs::PermissionsExt as _;

        let root = TestTempDir::new("olo-f4");
        let marker = root.path().join("decoy-ran");
        let decoy = root.path().join("olo-rex-link");
        std::fs::write(&decoy, format!("#!/bin/sh\necho ran > {}\nexit 0\n", marker.display())).unwrap();
        std::fs::set_permissions(&decoy, std::fs::Permissions::from_mode(0o755)).unwrap();

        let fake_exe = root.path().join("target/debug/build/olo-rex-client/abc123/out/test_bin");
        std::fs::create_dir_all(fake_exe.parent().unwrap()).unwrap();
        assert_eq!(cargo_profile_dir(&fake_exe), Some(root.path().join("target/debug")));
        // Exercise discovery with explicit inputs independent of environment
        // variables so this assertion ALWAYS runs (even when OLO_REX_LINK_REQUIRED=1).
        assert!(find_test_helper_with(&fake_exe, None, false).is_none());
        assert!(!marker.exists(), "helper finder executed an unrelated binary outside Cargo profile dir!");
    }

    /// Verify production priority-setting paths: `open_halves(true)` sets `1`,
    /// `open_halves(false)` leaves `0`, and `open_screen_halves_within` sets `-1`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn open_screen_sets_priority_minus_one() {
        use std::process::Stdio;
        use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};

        let Some(bin) = find_test_helper(&std::env::current_exe().unwrap()) else { return };

        let scratch = TestTempDir::new("olo-qp");
        for sub in ["home", "run", "state", "desktop"] {
            std::fs::create_dir_all(scratch.path().join(sub)).unwrap();
        }
        let token = new_token().unwrap();
        let mut child = tokio::process::Command::new(&bin)
            .args(["quic", "--", "cat"])
            .env("HOME", scratch.path().join("home"))
            .env("XDG_RUNTIME_DIR", scratch.path().join("run"))
            .env("XDG_STATE_HOME", scratch.path().join("state"))
            .env("OLO_HOST_DESKTOP", scratch.path().join("desktop"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(format!("{}\n", hex(&token)).as_bytes()).await.unwrap();
        let mut line = String::new();
        tokio::time::timeout(Duration::from_secs(10), BufReader::new(child.stdout.take().unwrap()).read_line(&mut line)).await.unwrap().unwrap();
        let (port, print) = announced(&line).unwrap();

        let link = QuicLink::connect((std::net::Ipv4Addr::LOCALHOST, port).into(), &print, token).await.unwrap();
        let (_plain_recv, plain_send) = link.open_halves(false).await.unwrap();
        let (_ctrl_recv, ctrl_send) = link.open_halves(true).await.unwrap();
        let (_screen_recv, screen_send) = link.open_screen_halves_within(&["follow".to_owned()], STREAM_OPEN_TIMEOUT).await.unwrap();

        assert_eq!(ctrl_send.priority(), Ok(1), "production open(true) must set priority 1");
        assert_eq!(plain_send.priority(), Ok(0), "production open(false) must keep priority 0");
        assert_eq!(screen_send.priority(), Ok(-1), "screen stream priority must be -1");

        drop((screen_send, _screen_recv, plain_send, _plain_recv, ctrl_send, _ctrl_recv, link, stdin));
        let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
    }

    /// Findings 12, 13 & Test Findings 1, 4: in-process self-signed Ed25519
    /// Quinn server (no external `openssl`) verifying:
    /// - 0-byte FIN -> `QuicScreenError::EmptyStartup`
    /// - Reset(3) -> `QuicScreenError::Unsupported`
    /// - post-dispatch stall (accepted stream, never replies) -> `QuicScreenError::StartupTimeout`
    /// - pre-dispatch stream-credit exhaustion on a 1-credit peer -> `QuicScreenError::OpenTimeout`
    ///   and immediate recovery on the same connection once credit is released.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn open_screen_rejects_empty_startup_and_classifies_reset_3_unsupported() {
        let (cert_der, key_der, print) = make_test_quic_cert();

        let make_server = |max_bidi: u32| {
            let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
            let mut tls = rustls::ServerConfig::builder_with_provider(provider)
                .with_protocol_versions(&[&rustls::version::TLS13])
                .unwrap()
                .with_no_client_auth()
                .with_single_cert(vec![CertificateDer::from(cert_der.clone())], rustls::pki_types::PrivateKeyDer::Pkcs8(key_der.clone().into()))
                .unwrap();
            tls.alpn_protocols = vec![wire::ALPN.to_vec()];
            let mut server_cfg = quinn::ServerConfig::with_crypto(Arc::new(quinn::crypto::rustls::QuicServerConfig::try_from(tls).unwrap()));
            let mut transport = quinn::TransportConfig::default();
            transport.max_concurrent_bidi_streams(max_bidi.into());
            server_cfg.transport_config(Arc::new(transport));
            quinn::Endpoint::server(server_cfg, (std::net::Ipv4Addr::LOCALHOST, 0).into()).unwrap()
        };

        // --- Server A: multi-stream peer testing EmptyStartup, Reset(3), StartupTimeout, OK ---
        let server = make_server(16);
        let addr = server.local_addr().unwrap();
        let srv = tokio::spawn(async move {
            let Some(inc) = server.accept().await else { return };
            let Ok(conn) = inc.await else { return };
            while let Ok((mut send, mut recv)) = conn.accept_bi().await {
                tokio::spawn(async move {
                    let mut head = [0u8; wire::TOKEN + 1];
                    if recv.read_exact(&mut head).await.is_err() {
                        return;
                    }
                    match head[wire::TOKEN] {
                        wire::HELLO => {
                            let _ = send.write_all(&[1]).await;
                            let _ = send.finish();
                        }
                        wire::SCREEN => {
                            let mut lb = [0u8; 2];
                            if recv.read_exact(&mut lb).await.is_err() {
                                return;
                            }
                            let mut body = vec![0u8; u16::from_le_bytes(lb) as usize];
                            if recv.read_exact(&mut body).await.is_err() {
                                return;
                            }
                            let args = olo_rex_link::screen_wire::decode_quic_screen_args(&body).unwrap();
                            if args[0] == "follow" {
                                let _ = send.finish();
                            } else if args[0] == "stream" && args[1] == "no-screens" {
                                let _ = send.reset(3u32.into());
                            } else if args[0] == "stream" && args[1] == "hang-startup" {
                                // Accepted and dispatched, then never emits a reply byte.
                                let _ = send.stopped().await;
                            } else {
                                let _ = send.write_all(b"OK").await;
                                let _ = send.finish();
                            }
                        }
                        _ => {}
                    }
                });
            }
        });

        let token = new_token().unwrap();
        let link = QuicLink::connect(addr, &print, token).await.unwrap();
        let err = link.open_screen_checked(&["follow".to_owned()], STREAM_OPEN_TIMEOUT).await.err().expect("empty FIN must fail");
        assert_eq!(err, QuicScreenError::EmptyStartup);
        assert!(link.alive(), "empty startup must not kill healthy QUIC link");

        let err = link.open_screen_checked(&["stream".to_owned(), "no-screens".to_owned()], STREAM_OPEN_TIMEOUT).await.err().expect("reset(3) must fail");
        assert_eq!(err, QuicScreenError::Unsupported);
        assert!(link.alive(), "reset(3) must leave healthy QUIC link alive for terminals");

        let err = link.open_screen_checked(&["stream".to_owned(), "hang-startup".to_owned()], Duration::from_millis(150)).await.err().expect("post-dispatch hang must time out");
        assert_eq!(err, QuicScreenError::StartupTimeout, "post-dispatch stall must be classified as StartupTimeout, not OpenTimeout");
        assert!(link.alive(), "startup timeout must leave healthy QUIC link alive");

        let mut ok_stream = link.open_screen(&["stream".to_owned(), "good".to_owned()]).await.unwrap();
        let mut out = Vec::new();
        tokio::io::AsyncReadExt::read_to_end(&mut ok_stream, &mut out).await.unwrap();
        assert_eq!(out, b"OK");
        link.hello().await.unwrap();
        drop(link);
        srv.abort();

        // --- Server B: controlled 1-credit peer testing pre-dispatch OpenTimeout + recovery ---
        let server1 = make_server(1);
        let addr1 = server1.local_addr().unwrap();
        let srv1 = tokio::spawn(async move {
            let Some(inc) = server1.accept().await else { return };
            let Ok(conn) = inc.await else { return };
            while let Ok((mut send, mut recv)) = conn.accept_bi().await {
                tokio::spawn(async move {
                    let mut head = [0u8; wire::TOKEN + 1];
                    if recv.read_exact(&mut head).await.is_err() {
                        return;
                    }
                    match head[wire::TOKEN] {
                        wire::PLAIN => {
                            let _ = send.write_all(b"H").await;
                            // Hold stream open until client finishes/drops it.
                            let mut sink = [0u8; 64];
                            while let Ok(Some(_)) = recv.read(&mut sink).await {}
                            let _ = send.finish();
                        }
                        wire::SCREEN => {
                            let mut lb = [0u8; 2];
                            if recv.read_exact(&mut lb).await.is_err() {
                                return;
                            }
                            let mut body = vec![0u8; u16::from_le_bytes(lb) as usize];
                            if recv.read_exact(&mut body).await.is_err() {
                                return;
                            }
                            let _ = send.write_all(b"RECOVERED").await;
                            let _ = send.finish();
                        }
                        _ => {}
                    }
                });
            }
        });

        let link1 = QuicLink::connect(addr1, &print, token).await.unwrap();
        // Occupy the peer's sole bidirectional stream credit deterministically.
        let mut held = link1.open(false).await.unwrap();
        let mut ack = [0u8; 1];
        tokio::io::AsyncReadExt::read_exact(&mut held, &mut ack).await.unwrap();
        assert_eq!(&ack, b"H");

        let err = link1.open_screen_checked(&["follow".to_owned()], Duration::from_millis(150)).await.err().expect("exhausted 1-credit peer must time out");
        assert_eq!(err, QuicScreenError::OpenTimeout, "exhausted stream credit must be OpenTimeout");
        assert!(link1.alive(), "credit timeout must leave QUIC link alive");

        // Release the single credit -> same connection immediately recovers!
        drop(held);
        let mut recovered = link1.open_screen(&["stream".to_owned(), "good".to_owned()]).await.unwrap();
        let mut got = Vec::new();
        tokio::io::AsyncReadExt::read_to_end(&mut recovered, &mut got).await.unwrap();
        assert_eq!(got, b"RECOVERED");

        drop(link1);
        srv1.abort();
    }
}
