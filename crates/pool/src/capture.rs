//! What Claude Code and Codex send, captured from the clients themselves
//! (Ben, 2026-11-19: "all requests from the pool need to be exactly the
//! same as if they came from claude code / codex"; "do a capture with each
//! connected real account and then store that in user's pool data dir and
//! use the right one based on the account in use"). The installed client
//! is run in a sandbox of its own (a scratch home holding one of the
//! pool's accounts: its access token, never its refresh token) through a
//! proxy of the pool's that writes down every request exactly as it came,
//! byte for byte, and passes on only the reads the client needs to go on as
//! it would with that account (`fate`). A model request, a reset's claim, a
//! sign-in's renewal and the client's logging are answered here and never
//! sent: no quota is spent, no reset used, no sign-in rotated. What was
//! captured is kept in Olo's state folder (`pool/captures/ACCOUNT.json`),
//! every secret and the account's own ids as placeholders; the pool's own
//! requests are made from it (`likeness`).
//!
//! Each client is run as people run it, so what's captured is what a
//! session sends: Claude Code in a terminal of its own (`/usage`,
//! `/limit-reset`, a message) and as `claude -p` (its print mode), once
//! more with its sign-in due for renewal; Codex through its app server
//! (its limits read and a reset's claim, as its own TUI asks them), as
//! `codex exec`, and once more with its sign-in due for renewal.

use std::{
    collections::HashMap,
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::fs::DirBuilderExt,
    },
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use rcgen::{BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair, KeyUsagePurpose};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

use crate::model::{Account, Credential, Provider, now};

/// Where an account's secrets stand in what's kept.
pub const ACCESS: &str = "{{access_token}}";
pub const REFRESH: &str = "{{refresh_token}}";
pub const ID_TOKEN: &str = "{{id_token}}";
/// ChatGPT's account id (Codex's `chatgpt-account-id`).
pub const ACCOUNT_ID: &str = "{{account_id}}";
/// Claude's organization and account (in paths, headers and
/// `metadata.user_id`).
pub const ORG: &str = "{{organization_uuid}}";
pub const ACCOUNT_UUID: &str = "{{account_uuid}}";
pub const EMAIL: &str = "{{email}}";
/// The cookies a client sends back (Codex keeps what ChatGPT sets): its
/// own, as the server set them for it.
pub const COOKIE: &str = "{{cookie}}";

/// The refresh token a sandbox holds: made up, so a sign-in's renewal can
/// be captured without one that works ever leaving the pool.
const MADE_UP_REFRESH: &str = "olo-pool-capture-refresh-token-not-a-real-one-0000000000000000";

/// One request a client sent, as it sent it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sent {
    /// The run it came from ("session", "print", "renewal", "app-server").
    pub run: String,
    pub host: String,
    pub method: String,
    /// Its path and query.
    pub target: String,
    /// Each header as it was sent: its name in its own case, its value, in
    /// their order.
    pub headers: Vec<(String, String)>,
    /// Its body as text (empty: none, or one too big to keep, `body_len`).
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub body_len: usize,
    /// What its TLS handshake offered: the protocols (ALPN), the cipher
    /// suites and the signature schemes, in its order.
    #[serde(default)]
    pub alpn: Vec<String>,
    #[serde(default)]
    pub suites: Vec<String>,
    #[serde(default)]
    pub schemes: Vec<String>,
    /// Sent on to its host (a read, with the account's own sign-in); else
    /// answered by the capture, never sent.
    #[serde(default)]
    pub passed_on: bool,
}

impl Sent {
    /// Its path, without the query.
    pub fn path(&self) -> &str {
        self.target.split('?').next().unwrap_or(&self.target)
    }

    /// A header's value (the first by that name, any case).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

/// What a client's TLS handshake offered a host: the protocols (ALPN),
/// the cipher suites and the signature schemes, in its order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hello {
    pub run: String,
    pub host: String,
    pub alpn: Vec<String>,
    pub suites: Vec<String>,
    pub schemes: Vec<String>,
    /// The whole ClientHello as the client wrote it (`hello::Offer`:
    /// every suite and extension in its order, and what's in them, less
    /// what is one connection's alone), read off the connection before
    /// the acceptor has it. None in a capture from before 2026-11-20.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offer: Option<crate::hello::Offer>,
}

/// What one installed client sent for one account.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Capture {
    pub provider: Provider,
    /// The client's version ("2.1.329").
    pub version: String,
    /// The installed client when it was captured (`Installed::stamp`).
    pub installed: String,
    pub account: String,
    pub at: i64,
    pub requests: Vec<Sent>,
    /// The account's own id as its client names it (Claude's
    /// `account_uuid`, in each model request's `metadata.user_id`): what
    /// a request the pool passes on for this account says in its place.
    #[serde(default)]
    pub account_uuid: Option<String>,
    /// Every handshake's offer, the native one among them (`native`: the
    /// client trusting only its own roots, which for Codex is another TLS
    /// stack than the one a custom authority makes it use).
    #[serde(default)]
    pub hellos: Vec<Hello>,
}

impl Capture {
    /// The last request it sent to `host` with `method` on `path` (its
    /// query aside), from the runs in `runs`' order of preference.
    pub fn find(&self, host: &str, method: &str, path: &str, runs: &[&str]) -> Option<&Sent> {
        let matches = |s: &&Sent| s.host == host && s.method == method && s.path() == path;
        runs.iter().find_map(|run| self.requests.iter().rev().filter(|s| s.run == *run).find(matches)).or_else(|| self.requests.iter().rev().find(matches))
    }
}

/// Where an account's capture is kept.
pub fn path(account: &str) -> PathBuf {
    olo_xdg::state_dir().join("pool/captures").join(format!("{account}.json"))
}

/// An account's capture, if one was made.
pub fn load(account: &str) -> Option<Capture> {
    serde_json::from_slice(&std::fs::read(path(account)).ok()?).ok()
}

/// Keep `c` (readable by the person alone: it says what their clients send).
pub fn save(c: &Capture) -> Result<(), String> {
    let p = path(&c.account);
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_vec_pretty(c).map_err(|e| e.to_string())?;
    crate::store::write_private(&p, &text).map_err(|e| e.to_string())
}

/// The installed client of a provider: where it is, its version, and what
/// says it changed (its file's size and time).
#[derive(Debug, Clone, PartialEq)]
pub struct Installed {
    pub exe: PathBuf,
    pub version: String,
    pub stamp: String,
}

/// The client the person runs (`claude` or `codex` on their PATH, its
/// symlinks followed to the version they lead to).
pub fn installed(p: Provider) -> Option<Installed> {
    let name = match p {
        Provider::Claude => "claude",
        Provider::Codex => "codex",
    };
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    // (The service's PATH may lack the person's own bin folder.)
    dirs.push(home.join(".local/bin"));
    let exe = dirs.iter().map(|d| d.join(name)).find(|p| p.is_file())?;
    let real = std::fs::canonicalize(&exe).ok()?;
    let meta = std::fs::metadata(&real).ok()?;
    let mtime = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    let version = version_of(p, &real).unwrap_or_default();
    Some(Installed { exe, version, stamp: format!("{}:{}:{}", real.display(), meta.len(), mtime) })
}

/// The version a client's install says it is: Claude Code's versions are
/// kept as `…/claude/versions/2.1.329`, Codex's as
/// `…/releases/0.170.0-x86_64-unknown-linux-musl/bin/codex`.
fn version_of(p: Provider, real: &Path) -> Option<String> {
    let s = real.to_string_lossy();
    let pick = |rest: &str| rest.split(['/', '-']).next().map(str::to_owned).filter(|v| v.chars().next().is_some_and(|c| c.is_ascii_digit()));
    match p {
        Provider::Claude => s.split("/versions/").nth(1).and_then(pick),
        Provider::Codex => s.split("/releases/").nth(1).and_then(pick),
    }
}

// ---------------------------------------------------------------------
// What the proxy does with each request.

/// What happens to a request during a capture.
#[derive(Debug, Clone, PartialEq)]
enum Fate {
    /// Sent on to its host with the sandbox's account, its answer passed
    /// back: a read the client needs to go on as it would.
    Forward,
    /// Answered here, never sent: (status, body).
    Answer(u16, &'static str),
}

/// What happens to `method` `target` on `host`. Anything that spends,
/// claims or rotates is never sent, whatever else is true; of the rest,
/// only reads on the providers' own hosts go on.
fn fate(host: &str, method: &str, target: &str) -> Fate {
    let path = target.split('?').next().unwrap_or(target);
    // A reset's claim (Claude's, Codex's): answered as if nothing needed
    // resetting, so nothing is spent.
    if path.ends_with("/reset_rate_limits") {
        return Fate::Answer(200, r#"{"result":"not_limited"}"#);
    }
    if path.contains("rate-limit-reset-credits/consume") {
        // (As ChatGPT says it: Codex's `ConsumeRateLimitResetCreditResponse`.)
        return Fate::Answer(200, r#"{"code":"nothing_to_reset","windows_reset":0}"#);
    }
    // A sign-in's renewal: refused, so no token is rotated.
    if path.ends_with("/oauth/token") || host.starts_with("auth.") || host == "platform.claude.com" || host == "console.anthropic.com" {
        return Fate::Answer(400, r#"{"error":"invalid_grant","error_description":"Captured by Olo's Token Pool, not sent."}"#);
    }
    // A model request: refused as a client error (never retried).
    if path.starts_with("/v1/messages") || path.starts_with("/backend-api/codex/responses") {
        return Fate::Answer(400, r#"{"type":"error","error":{"type":"invalid_request_error","message":"Captured by Olo's Token Pool, not sent."}}"#);
    }
    if reads(host, method, path) {
        return Fate::Forward;
    }
    match method {
        // Logging, analytics and anything else: taken, not sent.
        "POST" => Fate::Answer(200, "{}"),
        _ => Fate::Answer(404, r#"{"error":"Captured by Olo's Token Pool, not sent."}"#),
    }
}

/// The reads a client needs answered as its account's to go on as it would
/// (its profile, flags, limits, settings), each named: nothing else is
/// ever sent on (Codex's review: "an allowlist of concrete, necessary read
/// operations").
fn reads(host: &str, method: &str, path: &str) -> bool {
    // ("{}" stands for one part of a path: an organization's id.)
    let like = |pattern: &str| {
        let (p, q): (Vec<&str>, Vec<&str>) = (pattern.split('/').collect(), path.split('/').collect());
        p.len() == q.len() && p.iter().zip(&q).all(|(a, b)| *a == "{}" && !b.is_empty() || a == b)
    };
    match (host, method) {
        ("api.anthropic.com", "GET") => [
            "/api/oauth/profile",
            "/api/oauth/usage",
            "/api/oauth/account/settings",
            "/api/claude_cli/bootstrap",
            "/api/claude_code_penguin_mode",
            "/api/claude_code_grove",
            "/api/directory/server-urls",
            "/api/frame/contract/latest",
            "/v1/mcp_servers",
            "/v1/privacy-consents/checkup",
            "/v1/ultrareview/quota",
            "/api/oauth/organizations/{}/referral/eligibility",
            "/api/oauth/organizations/{}/skills/list-skills",
            "/api/oauth/organizations/{}/sync/github/auth",
            "/api/oauth/organizations/{}/quirky-lollipop/link-offer",
            "/api/organizations/{}/model_selector/cc",
        ]
        .iter()
        .any(|p| like(p)),
        // (Claude Code's feature flags, read for the account.)
        ("api.anthropic.com", "POST") => like("/api/eval/{}"),
        ("chatgpt.com", "GET") => [
            "/backend-api/wham/accounts/check",
            "/backend-api/codex/models",
            "/backend-api/wham/usage",
            "/backend-api/wham/rate-limit-reset-credits",
            "/backend-api/wham/settings/user",
            "/backend-api/wham/security-setup",
        ]
        .iter()
        .any(|p| like(p)),
        _ => false,
    }
}

// ---------------------------------------------------------------------
// The recording proxy.

struct Recorder {
    issuer: Issuer<'static, KeyPair>,
    ca_der: CertificateDer<'static>,
    ca_pem: String,
    crypto: Arc<rustls::crypto::CryptoProvider>,
    leaves: Mutex<HashMap<String, Arc<rustls::sign::CertifiedKey>>>,
    sent: Mutex<Vec<Sent>>,
    hellos: Mutex<Vec<Hello>>,
    /// A limit to show the client (its claim, when it resets): its model
    /// requests answered as at that limit, so it offers what it offers
    /// there (a reset).
    wall: Mutex<Option<(String, i64)>>,
    /// The account's real access token: only a request carrying it (or no
    /// credential at all) is ever sent on. A provider's API never gets a
    /// made-up one (Ben, 2026-11-19: "we don't want to make requests to
    /// the real api with fake data though that is not polite").
    real: Mutex<String>,
    /// The run under way.
    run: Mutex<String>,
}

impl Recorder {
    fn new() -> Result<Recorder, String> {
        let key = KeyPair::generate().map_err(|e| e.to_string())?;
        let mut p = CertificateParams::default();
        p.distinguished_name.push(DnType::CommonName, "Olo Token Pool capture");
        p.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
        p.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign, KeyUsagePurpose::DigitalSignature];
        let cert = p.self_signed(&key).map_err(|e| e.to_string())?;
        Ok(Recorder {
            ca_der: cert.der().clone(),
            ca_pem: cert.pem(),
            issuer: Issuer::new(p, key),
            crypto: Arc::new(rustls::crypto::aws_lc_rs::default_provider()),
            leaves: Mutex::new(HashMap::new()),
            sent: Mutex::new(Vec::new()),
            hellos: Mutex::new(Vec::new()),
            wall: Mutex::new(None),
            real: Mutex::new(String::new()),
            run: Mutex::new(String::new()),
        })
    }

    /// A certificate for `host`, signed by this capture's authority (which
    /// only the sandbox trusts, and only while it runs).
    fn leaf(&self, host: &str) -> Result<Arc<rustls::sign::CertifiedKey>, String> {
        let mut leaves = self.leaves.lock().unwrap();
        if let Some(k) = leaves.get(host) {
            return Ok(k.clone());
        }
        use chrono::Datelike;
        let key = KeyPair::generate().map_err(|e| e.to_string())?;
        let mut p = CertificateParams::new(vec![host.to_owned()]).map_err(|e| e.to_string())?;
        p.distinguished_name.push(DnType::CommonName, host);
        p.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        p.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        p.use_authority_key_identifier_extension = true;
        let (ago, until) = (chrono::Utc::now() - chrono::Duration::days(1), chrono::Utc::now() + chrono::Duration::days(2));
        p.not_before = rcgen::date_time_ymd(ago.year(), ago.month() as u8, ago.day() as u8);
        p.not_after = rcgen::date_time_ymd(until.year(), until.month() as u8, until.day() as u8);
        let cert = p.signed_by(&key, &self.issuer).map_err(|e| e.to_string())?;
        let private = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der()));
        let signing = self.crypto.key_provider.load_private_key(private).map_err(|e| e.to_string())?;
        let k = Arc::new(rustls::sign::CertifiedKey::new(vec![cert.der().clone(), self.ca_der.clone()], signing));
        leaves.insert(host.to_owned(), k.clone());
        Ok(k)
    }
}

#[derive(Debug)]
struct OneCert(Arc<rustls::sign::CertifiedKey>);

impl rustls::server::ResolvesServerCert for OneCert {
    fn resolve(&self, _: rustls::server::ClientHello<'_>) -> Option<Arc<rustls::sign::CertifiedKey>> {
        Some(self.0.clone())
    }
}

/// Read up to the end of a head (CRLF CRLF) into `buf`; where it ends.
async fn read_head<S: AsyncRead + Unpin>(s: &mut S, buf: &mut Vec<u8>) -> Option<usize> {
    let mut chunk = [0u8; 8192];
    loop {
        if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            return Some(end + 4);
        }
        if buf.len() > 256 * 1024 {
            return None;
        }
        let n = tokio::time::timeout(Duration::from_secs(60), s.read(&mut chunk)).await.ok()?.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

/// The rest of a request's body after its head (`have`: what came with the
/// head): by its length, or its chunks until the last (decoded).
/// The most of a request body a capture reads (a model request's is
/// answered without its body kept).
const BODY_MOST: usize = 64 << 20;

async fn read_body<S: AsyncRead + Unpin>(s: &mut S, mut have: Vec<u8>, length: Option<usize>, chunked: bool) -> Vec<u8> {
    let mut chunk = [0u8; 16384];
    if chunked {
        // (Up to where its framing ends, or framing that isn't chunked, or
        // `BODY_MOST`.)
        loop {
            match crate::likeness::chunked_end(&have) {
                Some(Ok(end)) => {
                    have.truncate(end);
                    break;
                }
                Some(Err(())) => break,
                None if have.len() >= BODY_MOST => break,
                None => match tokio::time::timeout(Duration::from_secs(30), s.read(&mut chunk)).await {
                    Ok(Ok(n)) if n > 0 => have.extend_from_slice(&chunk[..n]),
                    _ => break,
                },
            }
        }
        return crate::likeness::unchunk(&have);
    }
    let want = length.unwrap_or(0).min(BODY_MOST);
    while have.len() < want {
        match tokio::time::timeout(Duration::from_secs(30), s.read(&mut chunk)).await {
            Ok(Ok(n)) if n > 0 => have.extend_from_slice(&chunk[..n]),
            _ => break,
        }
    }
    have.truncate(want);
    have
}

/// A body as text to keep: decoded if its encoding says so; nothing for one
/// too big (a model request's, which the pool never makes itself).
fn body_text(body: &[u8], coding: &str) -> String {
    let plain = match coding {
        "zstd" => zstd::stream::decode_all(body).unwrap_or_default(),
        "gzip" => {
            let mut o = Vec::new();
            let _ = flate2::read::GzDecoder::new(body).read_to_end(&mut o);
            o
        }
        _ => body.to_vec(),
    };
    if plain.len() > 16 * 1024 { String::new() } else { String::from_utf8_lossy(&plain).into_owned() }
}

/// Replace a head's `Connection` with `close` (so an answer ends with its
/// connection), adding one if it had none.
fn closing(head: &str) -> String {
    let mut lines: Vec<String> = head.trim_end_matches("\r\n").split("\r\n").map(str::to_owned).collect();
    let mut found = false;
    for l in lines.iter_mut().skip(1) {
        if l.split(':').next().is_some_and(|k| k.trim().eq_ignore_ascii_case("connection")) {
            *l = "Connection: close".to_owned();
            found = true;
        }
    }
    if !found {
        lines.push("Connection: close".to_owned());
    }
    format!("{}\r\n\r\n", lines.join("\r\n"))
}

/// The roots the pool trusts for the providers' hosts (Mozilla's).
pub fn roots() -> Arc<rustls::RootCertStore> {
    static ROOTS: std::sync::OnceLock<Arc<rustls::RootCertStore>> = std::sync::OnceLock::new();
    ROOTS.get_or_init(|| Arc::new(rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() })).clone()
}

/// Send a request on to `host` as it came (its connection closing after),
/// and pass its answer back to `out` as it comes.
async fn forward<W: AsyncWrite + Unpin>(crypto: &Arc<rustls::crypto::CryptoProvider>, host: &str, head: &str, body: &[u8], out: &mut W) -> Result<(), String> {
    let tcp = tokio::time::timeout(Duration::from_secs(20), TcpStream::connect((host, 443))).await.map_err(|_| format!("{host}: no connection"))?.map_err(|e| e.to_string())?;
    let mut config =
        rustls::ClientConfig::builder_with_provider(crypto.clone()).with_safe_default_protocol_versions().map_err(|e| e.to_string())?.with_root_certificates(roots()).with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    let name = ServerName::try_from(host.to_owned()).map_err(|e| e.to_string())?;
    let mut tls = tokio::time::timeout(Duration::from_secs(20), tokio_rustls::TlsConnector::from(Arc::new(config)).connect(name, tcp))
        .await
        .map_err(|_| format!("{host}: no TLS"))?
        .map_err(|e| e.to_string())?;
    tls.write_all(closing(head).as_bytes()).await.map_err(|e| e.to_string())?;
    tls.write_all(body).await.map_err(|e| e.to_string())?;
    let _ = tokio::time::timeout(Duration::from_secs(60), tokio::io::copy(&mut tls, out)).await;
    Ok(())
}

/// One connection from the sandboxed client: its CONNECT, its TLS (what
/// it offers written down), then each request, written down and sent on
/// or answered.
async fn connection(rec: Arc<Recorder>, mut client: TcpStream) -> Option<()> {
    let mut buf = Vec::new();
    let end = read_head(&mut client, &mut buf).await?;
    let head = String::from_utf8_lossy(&buf[..end]).into_owned();
    let target = head.lines().next()?.split_whitespace().nth(1)?.to_owned();
    if !head.starts_with("CONNECT ") {
        let _ = client.write_all(b"HTTP/1.1 405 Method Not Allowed\r\ncontent-length: 0\r\nconnection: close\r\n\r\n").await;
        return None;
    }
    let host = target.rsplit_once(':').map(|(h, _)| h.trim_matches(['[', ']']).to_ascii_lowercase()).unwrap_or(target.to_ascii_lowercase());
    client.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").await.ok()?;
    // (What came after the head, a client not waiting for the answer: the
    // start of its TLS.)
    let mut client = crate::intercept::Prefixed::new(buf[end..].to_vec(), client);
    // (Its ClientHello read as it came, for the whole of what it offers,
    // and given to the acceptor before the rest of the stream.)
    let (read, offer) = tokio::time::timeout(Duration::from_secs(20), crate::hello::read(&mut client)).await.ok()??;
    client.unread(read);
    let start = tokio::time::timeout(Duration::from_secs(20), tokio_rustls::LazyConfigAcceptor::new(rustls::server::Acceptor::default(), client)).await.ok()?.ok()?;
    let hello = start.client_hello();
    let sni = hello.server_name().map(str::to_ascii_lowercase).unwrap_or_else(|| host.clone());
    let alpn: Vec<String> = hello.alpn().map(|a| a.map(|p| String::from_utf8_lossy(p).into_owned()).collect()).unwrap_or_default();
    let suites: Vec<String> = hello.cipher_suites().iter().map(|c| format!("{c:?}")).collect();
    let schemes: Vec<String> = hello.signature_schemes().iter().map(|c| format!("{c:?}")).collect();
    // (Written down before the handshake goes on: a client that doesn't
    // trust this capture still shows what it offers.)
    let run = rec.run.lock().unwrap().clone();
    rec.hellos.lock().unwrap().push(Hello { run, host: sni.clone(), alpn: alpn.clone(), suites: suites.clone(), schemes: schemes.clone(), offer });
    let mut config =
        rustls::ServerConfig::builder_with_provider(rec.crypto.clone()).with_safe_default_protocol_versions().ok()?.with_no_client_auth().with_cert_resolver(Arc::new(OneCert(rec.leaf(&sni).ok()?)));
    // (HTTP/1.1, so each head is read as it came; what the client would
    // speak with the host, from its offer, is kept with it.)
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    let mut tls = tokio::time::timeout(Duration::from_secs(20), start.into_stream(Arc::new(config))).await.ok()?.ok()?;
    let mut buf = Vec::new();
    let end = read_head(&mut tls, &mut buf).await?;
    let head = String::from_utf8_lossy(&buf[..end]).into_owned();
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or("").to_owned();
    let mut parts = request_line.split_whitespace();
    let (method, target) = (parts.next().unwrap_or("").to_owned(), parts.next().unwrap_or("").to_owned());
    let mut headers = Vec::new();
    let (mut length, mut chunked, mut coding) = (None, false, String::new());
    for l in lines.filter(|l| !l.is_empty()) {
        let (k, v) = l.split_once(':').map(|(k, v)| (k.to_owned(), v.trim_start().to_owned())).unwrap_or((l.to_owned(), String::new()));
        match k.to_ascii_lowercase().as_str() {
            "content-length" => length = v.trim().parse().ok(),
            "transfer-encoding" => chunked = v.to_ascii_lowercase().contains("chunked"),
            "content-encoding" => coding = v.trim().to_ascii_lowercase(),
            _ => {}
        }
        headers.push((k, v));
    }
    let body = read_body(&mut tls, buf[end..].to_vec(), length, chunked).await;
    let run = rec.run.lock().unwrap().clone();
    let real = rec.real.lock().unwrap().clone();
    let honest = carries_only_headers(&headers, &real);
    let wall = rec.wall.lock().unwrap().clone().filter(|_| sni == "api.anthropic.com" && method == "POST" && target.starts_with("/v1/messages"));
    // Sent on only with the account's own credential, or none: a request
    // carrying a made-up one (a renewal's run) is answered here.
    // Nor one whose body didn't come whole, or came chunked (sent on, it
    // would go as it was framed with what was read of it), nor Codex's TUI
    // asking for Reserve (Codex's third look: none of these is a plain read).
    let whole = !chunked && length.is_none_or(|n| body.len() == n);
    let reserve = headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("x-openai-codex-luna-reserve"));
    let fate = match fate(&sni, &method, &target) {
        Fate::Forward if !honest || wall.is_some() => Fate::Answer(401, r#"{"error":"Captured by Olo's Token Pool, not sent: not the account's own sign-in."}"#),
        Fate::Forward if !whole || reserve => Fate::Answer(400, r#"{"error":"Captured by Olo's Token Pool, not sent: not a plain read."}"#),
        f => f,
    };
    rec.sent.lock().unwrap().push(Sent {
        run,
        host: sni.clone(),
        method: method.clone(),
        target: target.clone(),
        headers,
        body: body_text(&body, &coding),
        body_len: body.len(),
        alpn,
        suites,
        schemes,
        passed_on: fate == Fate::Forward,
    });
    if let Some((claim, reset)) = wall {
        // At the limit, as Anthropic answers there (never sent).
        let answer = r#"{"type":"error","error":{"type":"rate_limit_error","message":"This request would exceed your account's rate limit. Please try again later."}}"#;
        let reply = format!(
            "HTTP/1.1 429 Too Many Requests\r\ncontent-type: application/json\r\nretry-after: {}\r\nanthropic-ratelimit-unified-status: rejected\r\nanthropic-ratelimit-unified-reset: {reset}\r\nanthropic-ratelimit-unified-representative-claim: {claim}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{answer}",
            (reset - now()).max(1),
            answer.len()
        );
        let _ = tls.write_all(reply.as_bytes()).await;
        let _ = tls.shutdown().await;
        return Some(());
    }
    match fate {
        Fate::Forward => {
            if let Err(e) = forward(&rec.crypto, &sni, &head, &body, &mut tls).await {
                tracing::debug!("pool capture: {sni}: {e}");
            }
        }
        Fate::Answer(status, answer) => {
            let reason = match status {
                200 => "OK",
                400 => "Bad Request",
                401 => "Unauthorized",
                _ => "Not Found",
            };
            let reply = format!("HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{answer}", answer.len());
            let _ = tls.write_all(reply.as_bytes()).await;
        }
    }
    let _ = tls.shutdown().await;
    Some(())
}

/// Whether every credential `headers` carry is `real` (or none):
/// its authorization, an API key, a session token in its cookies aside
/// (those are the server's own, set for this session).
fn carries_only_headers(headers: &[(String, String)], real: &str) -> bool {
    headers.iter().all(|(k, v)| match k.to_ascii_lowercase().as_str() {
        "authorization" => !real.is_empty() && v.strip_prefix("Bearer ").is_some_and(|t| t == real),
        "x-api-key" => false,
        _ => true,
    })
}

// ---------------------------------------------------------------------
// The sandbox and the client's runs.

/// A scratch home for one capture (on disk, in Olo's cache folder), gone
/// when dropped.
struct Sandbox {
    dir: PathBuf,
}

impl Sandbox {
    fn new() -> Result<Sandbox, String> {
        sweep();
        let mut id = [0u8; 8];
        getrandom::fill(&mut id).map_err(|e| e.to_string())?;
        let base = olo_xdg::cache_dir().join("pool-capture");
        std::fs::create_dir_all(&base).map_err(|e| e.to_string())?;
        let dir = base.join(id.iter().map(|b| format!("{b:02x}")).collect::<String>());
        // (Private from its making: no moment it can be read by another.)
        std::fs::DirBuilder::new().mode(0o700).create(&dir).map_err(|e| e.to_string())?;
        std::fs::DirBuilder::new().mode(0o700).create(dir.join("work")).map_err(|e| e.to_string())?;
        // Its owner (and when it started, so another process given its id
        // later isn't taken for it), so a sweep takes only a dead one's.
        let me = std::process::id() as i32;
        std::fs::write(dir.join("owner"), format!("{me} {}", crate::drain::started(me).unwrap_or(0))).map_err(|e| e.to_string())?;
        Ok(Sandbox { dir })
    }
    fn home(&self) -> PathBuf {
        self.dir.join("home")
    }
    fn work(&self) -> PathBuf {
        self.dir.join("work")
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Sandboxes a capture left when it was killed (an access token in
/// each), removed: those whose owner is gone, never a live one's. Run at
/// each capture and when the pool starts.
pub fn sweep() {
    sweep_where(false);
}

/// The same, as the pool starts: its own too (an upgrade in place keeps the
/// process's id, and nothing of this copy is capturing yet).
pub fn sweep_at_start() {
    sweep_where(true);
}

fn sweep_where(own_too: bool) {
    let base = olo_xdg::cache_dir().join("pool-capture");
    let Ok(old) = std::fs::read_dir(&base) else { return };
    let me = std::process::id() as i32;
    for e in old.flatten() {
        // (One being made has no owner yet: left alone its first ten
        // minutes.)
        if !e.path().join("owner").exists() && e.metadata().and_then(|m| m.modified()).is_ok_and(|t| t.elapsed().is_ok_and(|d| d < Duration::from_secs(600))) {
            continue;
        }
        let owner = std::fs::read_to_string(e.path().join("owner")).unwrap_or_default();
        let mut owner = owner.split_whitespace().map(|n| n.parse::<u64>().ok());
        let (pid, started) = (owner.next().flatten(), owner.next().flatten());
        let alive = pid.and_then(|p| i32::try_from(p).ok()).is_some_and(|pid| pid != 0 && !(own_too && pid == me) && crate::drain::started(pid).is_some_and(|now| started.is_none_or(|s| s == now)));
        if !alive {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

fn write_secret(p: &Path, text: &str) -> Result<(), String> {
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    crate::store::write_private(p, text.as_bytes()).map_err(|e| e.to_string())
}

/// One way of running the client during a capture.
enum Step {
    /// A command, its input closed; ends by itself (or at the deadline).
    /// `trusted`: given this capture's authority (else the client keeps
    /// its own roots, and only its handshake's offer is seen).
    Command { run: &'static str, args: Vec<String>, wait: Duration, trusted: bool },
    /// A command in a terminal of its own, typed into: (what, then how
    /// long to wait after it). `wall`: its model requests answered as at
    /// the account's used-up limit, when it has one (a reset's claim is
    /// made there).
    Terminal { run: &'static str, args: Vec<String>, keys: Vec<(String, Duration)>, wall: bool },
    /// A command given lines on its input, each after the wait before it.
    Lines { run: &'static str, args: Vec<String>, lines: Vec<(String, Duration)> },
    /// The sign-in made due for renewal before the next steps.
    Expire,
}

/// The environment a sandboxed client runs in: nothing of the person's
/// but their PATH and language; its home, its proxy and its trust.
fn client_env(sb: &Sandbox, p: Provider, port: u16, ca: &Path, exe: &Path) -> Vec<(String, String)> {
    let proxy = format!("http://127.0.0.1:{port}");
    let mut path = exe.parent().map(|d| d.display().to_string()).unwrap_or_default();
    path.push_str(":/usr/local/bin:/usr/bin:/bin");
    let mut env = vec![
        ("PATH".to_owned(), path),
        ("HOME".to_owned(), sb.home().display().to_string()),
        ("LANG".to_owned(), std::env::var("LANG").unwrap_or_else(|_| "en_US.UTF-8".into())),
        ("TERM".to_owned(), std::env::var("TERM").ok().filter(|t| !t.is_empty() && t != "dumb").unwrap_or_else(|| "xterm-256color".into())),
        ("HTTPS_PROXY".to_owned(), proxy.clone()),
        ("HTTP_PROXY".to_owned(), proxy.clone()),
        ("https_proxy".to_owned(), proxy.clone()),
        ("http_proxy".to_owned(), proxy),
        ("XDG_CONFIG_HOME".to_owned(), sb.home().join(".config").display().to_string()),
        ("XDG_CACHE_HOME".to_owned(), sb.home().join(".cache").display().to_string()),
        ("XDG_DATA_HOME".to_owned(), sb.home().join(".local/share").display().to_string()),
        ("XDG_STATE_HOME".to_owned(), sb.home().join(".local/state").display().to_string()),
    ];
    match p {
        Provider::Claude => {
            env.push(("CLAUDE_CONFIG_DIR".into(), sb.home().join(".claude").display().to_string()));
            env.push(("NODE_EXTRA_CA_CERTS".into(), ca.display().to_string()));
            env.push(("DISABLE_AUTOUPDATER".into(), "1".into()));
        }
        Provider::Codex => {
            env.push(("CODEX_HOME".into(), sb.home().join(".codex").display().to_string()));
            env.push(("SSL_CERT_FILE".into(), ca.display().to_string()));
        }
    }
    env
}

/// Claude Code's sandbox: its sign-in (the account's access token, a
/// made-up refresh token) and a settled first run (onboarding done, the
/// folder trusted), as a person's install has them.
fn claude_files(sb: &Sandbox, a: &Account, c: &Credential, expired: bool) -> Result<(), String> {
    let expires = if expired { (now() - 3600) * 1000 } else { c.expires_at.unwrap_or(now() + 3600) * 1000 };
    let creds = json!({"claudeAiOauth": {
        "accessToken": c.access,
        "refreshToken": MADE_UP_REFRESH,
        "expiresAt": expires,
        "scopes": ["user:file_upload", "user:inference", "user:mcp_servers", "user:profile", "user:sessions:claude_code"],
        "subscriptionType": a.plan.clone().unwrap_or_else(|| "max".into()),
    }});
    write_secret(&sb.home().join(".claude/.credentials.json"), &creds.to_string())?;
    // (Kept by Claude Code between runs: what it learned the first time,
    // its account from the profile it read, stays for the next.)
    let config_path = sb.home().join(".claude/.claude.json");
    let mut config: Value = std::fs::read(&config_path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_else(|| json!({}));
    config["hasCompletedOnboarding"] = json!(true);
    config["autoUpdates"] = json!(false);
    config["theme"] = json!("dark");
    if config["numStartups"].is_null() {
        config["numStartups"] = json!(12);
    }
    config["projects"][sb.work().display().to_string()] = json!({"hasTrustDialogAccepted": true, "hasCompletedProjectOnboarding": true, "projectOnboardingSeenCount": 1});
    if config["oauthAccount"].is_null()
        && let Some(org) = a.org.as_deref()
    {
        config["oauthAccount"] = json!({"organizationUuid": org, "emailAddress": a.email.clone().unwrap_or_default()});
    }
    write_secret(&config_path, &config.to_string())?;
    write_secret(&sb.home().join(".claude.json"), &config.to_string())
}

/// The used-up limit of a Claude account to show its client, as the
/// rate-limit header names it, and when it resets: its Fable week's or its
/// week's (`claude_label`'s names; Claude Code calls Fable's
/// `seven_day_overage_included`).
fn wall_of(a: &Account) -> Option<(String, i64)> {
    let out = |w: &crate::model::Window| w.used >= 99.5 && w.resets_at.is_some_and(|r| r > now());
    let by = |names: &[&str]| a.windows.iter().find(|w| names.contains(&w.name.as_str()) && out(w)).and_then(|w| w.resets_at);
    if let Some(r) = by(&["seven_day_overage_included", "seven_day_fable"]) {
        return Some(("seven_day_overage_included".into(), r));
    }
    by(&["seven_day"]).map(|r| ("seven_day".into(), r))
}

/// A JWT nobody signed: Codex reads the account from its id token, which
/// it never sends; the pool keeps none.
fn unsigned_jwt(claims: &Value) -> String {
    use base64::Engine;
    let b = |v: &Value| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v.to_string());
    format!("{}.{}.{}", b(&json!({"alg": "none", "typ": "JWT"})), b(claims), "c2lnbmF0dXJl")
}

/// Codex's sandbox: its sign-in (the account's access token and id, a
/// made-up refresh token) and the folder trusted.
fn codex_files(sb: &Sandbox, a: &Account, c: &Credential, expired: bool) -> Result<(), String> {
    let claims = crate::upstream::jwt_claims(&c.access).unwrap_or_else(|| json!({}));
    let auth = claims.get("https://api.openai.com/auth").cloned().unwrap_or_else(|| json!({"chatgpt_account_id": c.account_id, "chatgpt_plan_type": a.plan}));
    let id = json!({"email": a.email.clone().unwrap_or_default(), "exp": now() + 7 * 86400, "iat": now(), "https://api.openai.com/auth": auth});
    // (Codex renews a sign-in whose token has expired: one that has, made
    // up, for the renewal's capture; its answer refused, nothing rotates.)
    let access = if expired { expired_access(c) } else { c.access.clone() };
    let last = chrono::DateTime::from_timestamp(if expired { now() - 30 * 86400 } else { now() }, 0).unwrap_or_default().format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string();
    let file = json!({"OPENAI_API_KEY": null, "tokens": {"id_token": unsigned_jwt(&id), "access_token": access, "refresh_token": MADE_UP_REFRESH, "account_id": c.account_id}, "last_refresh": last});
    let dir = sb.home().join(".codex");
    write_secret(&dir.join("auth.json"), &file.to_string())?;
    let config = format!("check_for_update_on_startup = false\n\n[projects.\"{}\"]\ntrust_level = \"trusted\"\n", sb.work().display());
    write_secret(&dir.join("config.toml"), &config)
}

/// A made-up access token like `c`'s, expired an hour ago (Codex renews on
/// that).
fn expired_access(c: &Credential) -> String {
    let mut claims = crate::upstream::jwt_claims(&c.access).unwrap_or_else(|| json!({}));
    claims["exp"] = json!(now() - 3600);
    claims["iat"] = json!(now() - 7200);
    unsigned_jwt(&claims)
}

/// The steps of a capture: what a person's session sends first, then the
/// rest.
fn steps(p: Provider) -> Vec<Step> {
    let s = Duration::from_secs;
    let args = |a: &[&str]| a.iter().map(|x| (*x).to_owned()).collect::<Vec<String>>();
    match p {
        Provider::Claude => vec![
            // (A session first: what it reads it asks for itself, before
            // print mode's runs leave it kept, its usage among it.)
            Step::Terminal {
                run: "session",
                args: Vec::new(),
                // (Its usage read asked for twice: the first `/usage` can come
                // before it's ready for it.)
                keys: vec![
                    (String::new(), s(15)),
                    ("/usage\r".into(), s(8)),
                    ("\u{1b}".into(), s(2)),
                    ("/limit-reset\r".into(), s(6)),
                    ("\r".into(), s(5)),
                    ("\r".into(), s(5)),
                    ("\u{1b}".into(), s(2)),
                    ("hi\r".into(), s(8)),
                    ("/usage\r".into(), s(8)),
                    ("\u{1b}".into(), s(2)),
                    ("/exit\r".into(), s(4)),
                ],
                wall: false,
            },
            Step::Command { run: "print", args: args(&["-p", "hi"]), wait: s(60), trusted: true },
            Step::Command { run: "print", args: args(&["-p", "/usage"]), wait: s(60), trusted: true },
            // At the limit: the reset Claude Code offers there, used (its
            // claim answered as needing nothing, so none is spent).
            Step::Terminal {
                run: "wall",
                args: args(&["--model", "claude-fable-5-2"]),
                keys: vec![(String::new(), s(12)), ("hi\r".into(), s(10)), ("/limit-reset\r".into(), s(8)), ("\r".into(), s(6)), ("\r".into(), s(6)), ("/exit\r".into(), s(4))],
                wall: true,
            },
            Step::Expire,
            Step::Command { run: "renewal", args: args(&["-p", "hi"]), wait: s(40), trusted: true },
        ],
        Provider::Codex => {
            let rpc = |id: u32, method: &str, params: Value| json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string() + "\n";
            vec![
                Step::Lines {
                    run: "app-server",
                    args: args(&["app-server"]),
                    lines: vec![
                        (rpc(1, "initialize", json!({"clientInfo": {"name": "codex_cli_rs", "title": "Codex", "version": ""}})), s(0)),
                        (json!({"jsonrpc": "2.0", "method": "initialized"}).to_string() + "\n", s(2)),
                        (rpc(2, "account/read", json!({})), s(2)),
                        (rpc(3, "account/rateLimits/read", json!({})), s(4)),
                        (rpc(4, "account/rateLimitResetCredit/consume", json!({"idempotencyKey": "olo-pool-capture-0000"})), s(6)),
                    ],
                },
                Step::Command { run: "print", args: args(&["exec", "--skip-git-repo-check", "hi"]), wait: s(60), trusted: true },
                Step::Terminal { run: "session", args: Vec::new(), keys: vec![(String::new(), s(12)), ("/status\r".into(), s(6)), ("hi\r".into(), s(8)), ("/quit\r".into(), s(4))], wall: false },
                // (Its own TLS, without the authority that makes it use rustls.)
                Step::Command { run: "native", args: args(&["exec", "--skip-git-repo-check", "hi"]), wait: s(20), trusted: false },
                Step::Expire,
                Step::Command { run: "renewal", args: args(&["exec", "--skip-git-repo-check", "hi"]), wait: s(40), trusted: true },
            ]
        }
    }
}

/// Read and let go of what `fd` gives until `stop` (polled, so nothing
/// that keeps the other end open can hold a capture up); with
/// `OLO_POOL_CAPTURE_TRANSCRIPT` set (a folder), what a terminal showed is
/// written there, to see what a client did.
fn drain(fd: OwnedFd, stop: Arc<std::sync::atomic::AtomicBool>) -> std::thread::JoinHandle<()> {
    // (Readable by the person alone: a terminal shows what it shows.)
    let mut keep = std::env::var_os("OLO_POOL_CAPTURE_TRANSCRIPT").and_then(|d| {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(PathBuf::from(d).join(format!("transcript-{}.txt", now()))).ok()
    });
    std::thread::spawn(move || {
        let mut sink = [0u8; 8192];
        while !stop.load(std::sync::atomic::Ordering::Relaxed) {
            let mut pfd = libc::pollfd { fd: fd.as_raw_fd(), events: libc::POLLIN, revents: 0 };
            // SAFETY: one pollfd, ours.
            let ready = unsafe { libc::poll(&mut pfd, 1, 200) };
            if ready <= 0 {
                continue;
            }
            // SAFETY: reads into our buffer from our fd.
            let n = unsafe { libc::read(fd.as_raw_fd(), sink.as_mut_ptr().cast(), sink.len()) };
            if n <= 0 {
                break;
            }
            if let Some(k) = keep.as_mut() {
                let _ = k.write_all(&sink[..n as usize]);
            }
        }
    })
}

/// End a client and everything it started (its own session's group).
fn end_group(pid: u32) {
    // SAFETY: signals a process group this capture started.
    unsafe {
        libc::kill(-(pid as i32), libc::SIGTERM);
    }
    std::thread::sleep(Duration::from_millis(400));
    unsafe {
        libc::kill(-(pid as i32), libc::SIGKILL);
    }
}

/// Allow the calling process (and what it starts) to connect over TCP
/// only to `port`: the capture's recorder (Linux's Landlock, its network
/// rules, kernel 6.7 on). Nothing it runs can reach a provider but through
/// the recorder, whatever it makes of its proxy settings (Codex's review:
/// "Give the child OS-enforced access only to its recorder ... Fail closed
/// if confinement cannot start"). Plain system calls, nothing allocated: it
/// runs between fork and exec.
fn confine(port: u16) -> std::io::Result<()> {
    const ACCESS_NET_CONNECT_TCP: u64 = 1 << 1;
    const RULE_NET_PORT: libc::c_int = 2;
    #[repr(C)]
    struct RulesetAttr {
        handled_access_fs: u64,
        handled_access_net: u64,
    }
    #[repr(C, packed)]
    struct NetPortAttr {
        allowed_access: u64,
        port: u64,
    }
    let attr = RulesetAttr { handled_access_fs: 0, handled_access_net: ACCESS_NET_CONNECT_TCP };
    // SAFETY: Landlock's own calls, with structs laid out as it reads them.
    unsafe {
        let fd = libc::syscall(libc::SYS_landlock_create_ruleset, &attr as *const RulesetAttr, std::mem::size_of::<RulesetAttr>(), 0u32) as libc::c_int;
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let rule = NetPortAttr { allowed_access: ACCESS_NET_CONNECT_TCP, port: port as u64 };
        let added = libc::syscall(libc::SYS_landlock_add_rule, fd, RULE_NET_PORT, &rule as *const NetPortAttr, 0u32);
        let quiet = libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0);
        let restricted = if added == 0 && quiet == 0 { libc::syscall(libc::SYS_landlock_restrict_self, fd, 0u32) } else { -1 };
        libc::close(fd);
        if restricted != 0 {
            return Err(std::io::Error::other("Landlock refused the capture's network rule"));
        }
    }
    Ok(())
}

/// Run one step (blocking: on a thread of its own); `port`, the recorder's,
/// the only one it may connect to.
fn run_step(step: &Step, exe: &Path, env: &[(String, String)], work: &Path, port: u16) -> Result<(), String> {
    use std::os::unix::process::CommandExt;
    let command = |args: &[String], terminal: bool| {
        let mut c = std::process::Command::new(exe);
        c.args(args).env_clear().envs(env.iter().map(|(k, v)| (k, v))).current_dir(work);
        // SAFETY: in the child before exec, system calls alone: its own
        // session (its group ended whole after), its terminal the
        // controlling one, nothing of the pool's open in it, and its
        // network confined to the recorder.
        unsafe {
            c.pre_exec(move || {
                if libc::setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                // (Ended with the capture's thread, should the pool go
                // mid-capture: an upgrade in place.)
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL, 0, 0, 0);
                if terminal && libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::syscall(libc::SYS_close_range, 3u32, u32::MAX, libc::CLOSE_RANGE_CLOEXEC) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                confine(port)
            });
        }
        c
    };
    match step {
        Step::Expire => Ok(()),
        Step::Command { args, wait, trusted, .. } => {
            let mut c = command(args, false);
            if !trusted {
                c.env_remove("SSL_CERT_FILE").env_remove("NODE_EXTRA_CA_CERTS");
            }
            let mut child = c.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().map_err(|e| e.to_string())?;
            let end = Instant::now() + *wait;
            while Instant::now() < end && child.try_wait().ok().flatten().is_none() {
                std::thread::sleep(Duration::from_millis(200));
            }
            // (What it started ends with it.)
            end_group(child.id());
            let _ = child.wait();
            Ok(())
        }
        Step::Lines { args, lines, .. } => {
            let mut child = command(args, false).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).spawn().map_err(|e| e.to_string())?;
            // (Its answers read and let go, so it never waits on a full pipe.)
            let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let out = child.stdout.take().map(OwnedFd::from);
            let drained = out.map(|fd| drain(fd, stop.clone()));
            let mut input = child.stdin.take();
            for (line, wait) in lines {
                std::thread::sleep(*wait);
                if let Some(i) = input.as_mut() {
                    let _ = i.write_all(line.as_bytes());
                    let _ = i.flush();
                }
            }
            std::thread::sleep(Duration::from_secs(6));
            drop(input);
            end_group(child.id());
            let _ = child.wait();
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
            if let Some(d) = drained {
                let _ = d.join();
            }
            Ok(())
        }
        Step::Terminal { args, keys, .. } => {
            let (master, slave) = open_terminal(50, 160)?;
            let stdio = |f: &OwnedFd| f.try_clone().map(std::process::Stdio::from).map_err(|e| e.to_string());
            let mut c = command(args, true);
            c.stdin(stdio(&slave)?).stdout(stdio(&slave)?).stderr(stdio(&slave)?);
            let mut child = c.spawn().map_err(|e| e.to_string())?;
            // (The command holds its copies of the terminal's other end
            // until it goes: gone now, with ours.)
            drop(c);
            drop(slave);
            let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let reader = match master.try_clone() {
                Ok(r) => r,
                Err(e) => {
                    end_group(child.id());
                    let _ = child.wait();
                    return Err(e.to_string());
                }
            };
            let drained = drain(reader, stop.clone());
            let mut writer = std::fs::File::from(master);
            for (keys, wait) in keys {
                if !keys.is_empty() {
                    // (Typed a key at a time, as a person types.)
                    for ch in keys.chars() {
                        let mut b = [0u8; 4];
                        let _ = writer.write_all(ch.encode_utf8(&mut b).as_bytes());
                        std::thread::sleep(Duration::from_millis(30));
                    }
                }
                std::thread::sleep(*wait);
                if child.try_wait().ok().flatten().is_some() {
                    break;
                }
            }
            end_group(child.id());
            let _ = child.wait();
            drop(writer);
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
            let _ = drained.join();
            Ok(())
        }
    }
}

/// A pseudo-terminal: (master, slave), the size given.
fn open_terminal(rows: u16, cols: u16) -> Result<(OwnedFd, OwnedFd), String> {
    let (mut master, mut slave) = (0, 0);
    let ws = libc::winsize { ws_row: rows, ws_col: cols, ws_xpixel: 0, ws_ypixel: 0 };
    // SAFETY: openpty fills in two fds.
    if unsafe { libc::openpty(&mut master, &mut slave, std::ptr::null_mut(), std::ptr::null(), &ws) } != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    // SAFETY: openpty just gave us these, and nothing else owns them; kept
    // from every program the pool starts but the one given them.
    unsafe {
        libc::fcntl(master, libc::F_SETFD, libc::FD_CLOEXEC);
        libc::fcntl(slave, libc::F_SETFD, libc::FD_CLOEXEC);
        Ok((OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)))
    }
}

// ---------------------------------------------------------------------
// Making what was captured keepable.

/// Every secret and id of the account in `s` as its placeholder.
fn hide(s: &mut Sent, secrets: &[(String, &'static str)]) {
    let swap = |text: &str| {
        let mut out = text.to_owned();
        for (value, placeholder) in secrets {
            if !value.is_empty() && out.contains(value.as_str()) {
                out = out.replace(value.as_str(), placeholder);
            }
        }
        out
    };
    s.target = swap(&s.target);
    for (k, v) in s.headers.iter_mut() {
        // (The session's cookies are its own: where they go is kept.)
        *v = if k.eq_ignore_ascii_case("cookie") { COOKIE.to_owned() } else { swap(v) };
    }
    s.body = swap(&s.body);
}

/// The values to hide in what an account's client sent, longest first (a
/// token contains no other, but an id might be part of a path).
fn secrets_of(a: &Account, c: &Credential, sent: &[Sent]) -> Vec<(String, &'static str)> {
    let mut out: Vec<(String, &'static str)> = vec![(c.access.clone(), ACCESS), (MADE_UP_REFRESH.to_owned(), REFRESH)];
    // (A renewal's run's made-up token stands for the account's own too.)
    for s in sent.iter().filter(|s| s.run == "renewal") {
        if let Some(v) = s.header("authorization").and_then(|v| v.strip_prefix("Bearer "))
            && v != c.access
        {
            out.push((v.to_owned(), ACCESS));
        }
    }
    if let Some(r) = c.refresh.as_deref() {
        // (Never in a sandbox; hidden all the same, should one ever be.)
        out.push((r.to_owned(), REFRESH));
    }
    if let Some(id) = c.account_id.as_deref() {
        out.push((id.to_owned(), ACCOUNT_ID));
    }
    if let Some(org) = a.org.as_deref() {
        out.push((org.to_owned(), ORG));
    }
    if let Some(e) = a.email.as_deref().filter(|e| e.contains('@')) {
        out.push((e.to_owned(), EMAIL));
    }
    // Claude's account uuid: in each model request's `metadata.user_id`.
    for s in sent {
        if let Ok(v) = serde_json::from_str::<Value>(&s.body)
            && let Some(user) = v["metadata"]["user_id"].as_str()
            && let Ok(u) = serde_json::from_str::<Value>(user)
            && let Some(uuid) = u["account_uuid"].as_str().filter(|u| !u.is_empty())
        {
            out.push((uuid.to_owned(), ACCOUNT_UUID));
            break;
        }
    }
    out.sort_by_key(|(v, _)| std::cmp::Reverse(v.len()));
    out.dedup_by(|a, b| a.0 == b.0);
    out
}

/// Capture what the installed client of `account`'s provider sends for
/// it (blocking for a minute or two: run it off the async threads). The
/// sign-in given is the account's own; only its access token goes into
/// the sandbox.
pub fn capture(account: &Account, cred: &Credential, progress: &dyn Fn(&str)) -> Result<Capture, String> {
    let p = account.provider;
    let client = installed(p).ok_or_else(|| format!("{} isn’t installed.", p.client()))?;
    let sb = Sandbox::new()?;
    let rec = Arc::new(Recorder::new()?);
    *rec.real.lock().unwrap() = cred.access.clone();
    let ca = sb.dir.join("ca.pem");
    std::fs::write(&ca, &rec.ca_pem).map_err(|e| e.to_string())?;
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().map_err(|e| e.to_string())?;
    let listener = runtime.block_on(TcpListener::bind(("127.0.0.1", 0))).map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let r = rec.clone();
    runtime.spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else { continue };
            tokio::spawn(connection(r.clone(), stream));
        }
    });
    let env = client_env(&sb, p, port, &ca, &client.exe);
    let mut expired = false;
    let write = |expired: bool| match p {
        Provider::Claude => claude_files(&sb, account, cred, expired),
        Provider::Codex => codex_files(&sb, account, cred, expired),
    };
    write(false)?;
    // (`OLO_POOL_CAPTURE_ONLY`: one run's steps, to work on it.)
    let only = std::env::var("OLO_POOL_CAPTURE_ONLY").ok();
    for step in steps(p) {
        if let (Some(o), Step::Command { run, .. } | Step::Terminal { run, .. } | Step::Lines { run, .. }) = (&only, &step)
            && o != run
        {
            continue;
        }
        match &step {
            Step::Expire => {
                expired = true;
                write(true)?;
            }
            Step::Command { run, .. } | Step::Terminal { run, .. } | Step::Lines { run, .. } => {
                *rec.run.lock().unwrap() = (*run).to_owned();
                // (The limit shown: the account's used-up one, Fable's or
                // the week's, with its real reset; none, none shown.)
                *rec.wall.lock().unwrap() = match &step {
                    Step::Terminal { wall: true, .. } => wall_of(account),
                    _ => None,
                };
                if matches!(step, Step::Terminal { wall: true, .. }) && rec.wall.lock().unwrap().is_none() {
                    continue;
                }
                if !expired {
                    write(false)?;
                }
                progress(&format!("{} {run}: {} requests so far", p.client(), rec.sent.lock().unwrap().len()));
                if let Err(e) = run_step(&step, &client.exe, &env, &sb.work(), port) {
                    tracing::warn!("pool capture: {} {run}: {e}", p.client());
                }
            }
        }
    }
    // (Answers still on their way: a moment, then the proxy goes.)
    std::thread::sleep(Duration::from_millis(500));
    runtime.shutdown_timeout(Duration::from_secs(2));
    let mut sent = std::mem::take(&mut *rec.sent.lock().unwrap());
    // Only what went to the providers' own hosts is kept.
    let hosts: &[&str] = match p {
        Provider::Claude => &["api.anthropic.com", "platform.claude.com", "console.anthropic.com", "claude.ai"],
        Provider::Codex => &["chatgpt.com", "auth.openai.com"],
    };
    sent.retain(|s| hosts.contains(&s.host.as_str()));
    let secrets = secrets_of(account, cred, &sent);
    let account_uuid = secrets.iter().find(|(_, p)| *p == ACCOUNT_UUID).map(|(v, _)| v.clone());
    for s in sent.iter_mut() {
        // (A body kept only where the pool makes the request itself: a
        // claim's and a renewal's; the rest, its head alone.)
        let p = s.path().to_owned();
        if !(p.ends_with("/reset_rate_limits") || p.ends_with("rate-limit-reset-credits/consume") || p.ends_with("/oauth/token")) {
            s.body.clear();
        }
        hide(s, &secrets);
    }
    if sent.is_empty() {
        return Err(format!("{} sent nothing through the capture.", p.client()));
    }
    let mut hellos = std::mem::take(&mut *rec.hellos.lock().unwrap());
    hellos.retain(|h| hosts.contains(&h.host.as_str()));
    // (One a run and host: they repeat.)
    hellos.dedup_by(|a, b| a.run == b.run && a.host == b.host && a.alpn == b.alpn && a.suites == b.suites);
    // Each of the client's offers beside the pool's own for the same
    // host and protocols, said once a capture: where they differ is
    // what a host could tell the pool from the client by, before any
    // request (docs/reports/token-pool-tls.md). No request is made.
    let mut said: Vec<String> = Vec::new();
    for h in &hellos {
        let alpn: Vec<&str> = h.alpn.iter().map(String::as_str).collect();
        let (Some(theirs), Some(ours)) = (&h.offer, crate::relay::own_offer(&alpn, &h.host)) else { continue };
        let apart = theirs.differences(&ours);
        let what = match apart.as_slice() {
            [] => "are the same".to_owned(),
            // (Two of rustls's never agree on it: said once, whatever
            // the two orders were.)
            [only] if only.starts_with(crate::hello::ORDER) => "are the same but for the order of their extensions".to_owned(),
            _ => format!("differ in {} things (the client's | the pool's): {}", apart.len(), apart.join("; ")),
        };
        if !said.contains(&what) {
            tracing::info!("pool: {}'s TLS offer to {} ({}) and the pool's {what}", p.client(), h.host, h.run);
            said.push(what);
        }
    }
    Ok(Capture { provider: p, version: client.version, installed: client.stamp, account: account.id.clone(), at: now(), requests: sent, account_uuid, hellos })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Nothing that spends, claims or rotates is ever sent on, whichever
    /// host or method; reads on the providers' hosts are.
    #[test]
    fn claims_renewals_and_model_requests_never_go_on() {
        for (host, method, target) in [
            ("api.anthropic.com", "POST", "/api/organizations/0000/reset_rate_limits"),
            ("api.anthropic.com", "GET", "/api/organizations/0000/reset_rate_limits"),
            ("chatgpt.com", "POST", "/backend-api/wham/rate-limit-reset-credits/consume"),
            ("chatgpt.com", "GET", "/backend-api/wham/rate-limit-reset-credits/consume"),
            ("platform.claude.com", "POST", "/v1/oauth/token"),
            ("platform.claude.com", "GET", "/anything"),
            ("auth.openai.com", "POST", "/oauth/token"),
            ("api.anthropic.com", "POST", "/v1/oauth/token"),
            ("api.anthropic.com", "POST", "/v1/messages?beta=true"),
            ("chatgpt.com", "POST", "/backend-api/codex/responses"),
            ("chatgpt.com", "GET", "/backend-api/codex/responses"),
            ("api.anthropic.com", "POST", "/api/event_logging/v2/batch"),
            ("chatgpt.com", "POST", "/backend-api/codex/analytics-events/events"),
            ("raw.githubusercontent.com", "GET", "/anthropics/claude-code/refs/heads/main/CHANGELOG.md"),
        ] {
            assert!(matches!(fate(host, method, target), Fate::Answer(..)), "{method} {host}{target} would go on");
        }
        for (host, method, target) in [
            ("api.anthropic.com", "GET", "/api/oauth/usage?cedar_ember=1&skip_spend=1"),
            ("api.anthropic.com", "GET", "/api/oauth/profile"),
            ("api.anthropic.com", "POST", "/api/eval/sdk-abc"),
            ("chatgpt.com", "GET", "/backend-api/wham/usage"),
            ("chatgpt.com", "GET", "/backend-api/wham/rate-limit-reset-credits"),
        ] {
            assert_eq!(fate(host, method, target), Fate::Forward, "{method} {host}{target}");
        }
    }

    /// A client's ClientHello is read whole before the acceptor has
    /// it (`hello::read`), kept with what the acceptor says of it, and
    /// the handshake goes on with the same bytes.
    #[tokio::test]
    async fn the_whole_offer_is_kept_and_the_handshake_goes_on() {
        let rec = Arc::new(Recorder::new().unwrap());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let serving = {
            let rec = rec.clone();
            tokio::spawn(async move {
                let (client, _) = listener.accept().await.unwrap();
                connection(rec, client).await;
            })
        };
        let mut tcp = TcpStream::connect(addr).await.unwrap();
        tcp.write_all(b"CONNECT example.test:443 HTTP/1.1\r\nHost: example.test:443\r\n\r\n").await.unwrap();
        let mut answer = [0u8; 39];
        tcp.read_exact(&mut answer).await.unwrap();
        assert_eq!(&answer, b"HTTP/1.1 200 Connection Established\r\n\r\n");
        let mut roots = rustls::RootCertStore::empty();
        roots.add(rec.ca_der.clone()).unwrap();
        let mut config = rustls::ClientConfig::builder_with_provider(rec.crypto.clone()).with_safe_default_protocol_versions().unwrap().with_root_certificates(roots).with_no_client_auth();
        config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
        let tls = tokio_rustls::TlsConnector::from(Arc::new(config)).connect("example.test".try_into().unwrap(), tcp).await.unwrap();
        let hellos = rec.hellos.lock().unwrap().clone();
        assert_eq!(hellos.len(), 1);
        assert_eq!((hellos[0].host.as_str(), hellos[0].alpn.as_slice()), ("example.test", &["h2".to_owned(), "http/1.1".to_owned()][..]));
        let offer = hellos[0].offer.as_ref().expect("the whole offer");
        assert_eq!(offer.suites.len(), hellos[0].suites.len());
        assert!(offer.extensions.iter().any(|e| e.kind == 16 && e.body.as_deref() == Some("000c02683208687474702f312e31")));
        // The pool's own, for the same host and protocols, is this
        // library's too: the same but for the order of its extensions.
        let ours = crate::relay::own_offer(&["h2", "http/1.1"], "example.test").unwrap();
        assert!(offer.differences(&ours).iter().all(|d| d.starts_with(crate::hello::ORDER)), "{:?}", offer.differences(&ours));
        drop(tls);
        serving.abort();
    }

    /// A program confined to the recorder's port reaches it, and nothing
    /// else, however open (Landlock: the kernel refuses the connection).
    #[test]
    fn a_capture_reaches_its_recorder_alone() {
        use std::os::unix::process::CommandExt;
        let allowed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let other = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let (a, o) = (allowed.local_addr().unwrap().port(), other.local_addr().unwrap().port());
        let connects = |port: u16| {
            let mut c = std::process::Command::new("/usr/bin/bash");
            c.args(["-c", &format!("exec 3<>/dev/tcp/127.0.0.1/{port}")]).stderr(std::process::Stdio::null());
            // SAFETY: confine is system calls alone, as between fork and exec.
            unsafe {
                c.pre_exec(move || confine(a));
            }
            c.status().unwrap().success()
        };
        assert!(connects(a), "its recorder refused");
        assert!(!connects(o), "it reached another port");
    }

    /// Only the account's own credential (or none) is ever sent on.
    #[test]
    fn nothing_made_up_goes_on() {
        let with = |auth: Option<&str>| Sent {
            run: "renewal".into(),
            host: "chatgpt.com".into(),
            method: "GET".into(),
            target: "/backend-api/wham/accounts/check".into(),
            headers: auth.map(|a| vec![("authorization".to_owned(), a.to_owned())]).unwrap_or_default(),
            body: String::new(),
            body_len: 0,
            alpn: vec![],
            suites: vec![],
            schemes: vec![],
            passed_on: false,
        };
        let ok = |s: Sent, real: &str| carries_only_headers(&s.headers, real);
        assert!(ok(with(Some("Bearer real")), "real"));
        assert!(ok(with(None), "real"));
        assert!(!ok(with(Some("Bearer made-up")), "real"));
        assert!(!ok(with(Some("Bearer real")), ""));
    }

    #[test]
    fn a_head_is_closed() {
        assert_eq!(closing("GET / HTTP/1.1\r\nHost: a\r\nConnection: keep-alive\r\n\r\n"), "GET / HTTP/1.1\r\nHost: a\r\nConnection: close\r\n\r\n");
        assert_eq!(closing("GET / HTTP/1.1\r\nHost: a\r\n\r\n"), "GET / HTTP/1.1\r\nHost: a\r\nConnection: close\r\n\r\n");
    }

    #[test]
    fn versions_from_installs() {
        assert_eq!(version_of(Provider::Claude, Path::new("/home/u/.local/share/claude/versions/2.1.329")).as_deref(), Some("2.1.329"));
        assert_eq!(version_of(Provider::Codex, Path::new("/home/u/.codex/packages/standalone/releases/0.170.0-x86_64-unknown-linux-musl/bin/codex")).as_deref(), Some("0.170.0"));
    }

    /// The account's secrets and ids never stay in what's kept.
    #[test]
    fn secrets_become_placeholders() {
        let a = Account { org: Some("org-uuid-1".into()), email: Some("a@example.com".into()), ..Account::new("acct".into(), "A".into(), Provider::Claude) };
        let c = Credential { access: "tok-access-123".into(), refresh: Some("tok-refresh".into()), expires_at: None, account_id: None };
        let body = json!({"metadata": {"user_id": json!({"device_id": "d", "account_uuid": "acct-uuid-9", "session_id": "s"}).to_string()}}).to_string();
        let mut sent = vec![Sent {
            run: "print".into(),
            host: "api.anthropic.com".into(),
            method: "GET".into(),
            target: "/api/oauth/organizations/org-uuid-1/x".into(),
            headers: vec![("Authorization".into(), "Bearer tok-access-123".into()), ("x-organization-uuid".into(), "org-uuid-1".into())],
            body,
            body_len: 0,
            alpn: vec![],
            suites: vec![],
            schemes: vec![],
            passed_on: false,
        }];
        let secrets = secrets_of(&a, &c, &sent);
        hide(&mut sent[0], &secrets);
        let s = &sent[0];
        assert_eq!(s.target, "/api/oauth/organizations/{{organization_uuid}}/x");
        assert_eq!(s.headers[0].1, "Bearer {{access_token}}");
        assert_eq!(s.headers[1].1, "{{organization_uuid}}");
        assert!(s.body.contains("{{account_uuid}}") && !s.body.contains("acct-uuid-9"), "{}", s.body);
    }
}
