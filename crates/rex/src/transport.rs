//! Unified Rex transport layer supporting:
//! - Local Rex Unix socket (auto-discovered + optional `rex server start` autostart)
//! - Explicit `unix:///path` Unix sockets
//! - Plain `http://host:port` TCP connections
//! - `https://host[:port]` and Tailscale MagicDNS TLS connections (`tokio-rustls` +
//!   `rustls-platform-verifier`)
//! - Native Rust `ssh://[user@]host[:port][/socket]` connections via [`crate::ssh`]
//!   with SSH session multiplexing (so data connections reuse the live SSH session!).

use std::{
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};

use rustls::{ClientConfig, pki_types::ServerName};
use rustls_platform_verifier::BuilderVerifierExt;
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::{TcpStream, UnixStream},
};
use tokio_rustls::TlsConnector;

use crate::{
    error::RexError,
    ssh::{SshAskpass, SshSessionPool, SshTarget},
};

/// Parsed Rex server endpoint.
#[derive(Debug, Clone)]
pub enum RexEndpoint {
    Local { autostart: bool },
    Unix(PathBuf),
    Http { host: String, port: u16 },
    Https { host: String, port: u16 },
    Ssh { target: SshTarget, pool: Arc<SshSessionPool>, askpass: Option<Arc<dyn SshAskpass>> },
}

pub(crate) fn authority_host(host: &str) -> String {
    if host.contains(':') { format!("[{host}]") } else { host.to_owned() }
}

impl RexEndpoint {
    /// Parse a Rex address string (`local`, `unix:///...`, `http://...`,
    /// `https://...`, `ssh://...`, `user@host`, `host.tail.ts.net`, or `host:port`).
    pub fn parse(raw: &str, ssh_pool: Arc<SshSessionPool>, askpass: Option<Arc<dyn SshAskpass>>) -> Result<Self, RexError> {
        let s = raw.trim();
        if s.is_empty() || s.eq_ignore_ascii_case("local") {
            return Ok(Self::Local { autostart: true });
        }
        if let Some(path) = s.strip_prefix("unix://") {
            if !path.starts_with('/') {
                return Err(RexError::InvalidEndpoint("expected an absolute Unix socket path".into()));
            }
            return Ok(Self::Unix(PathBuf::from(path)));
        }
        if s.starts_with('/') {
            return Ok(Self::Unix(PathBuf::from(s)));
        }
        if let Some(rest) = s.strip_prefix("ssh://") {
            let target = SshTarget::parse(rest)?;
            return Ok(Self::Ssh { target, pool: ssh_pool, askpass });
        }
        let http = s.strip_prefix("http://");
        let https = s.strip_prefix("https://");
        let explicit = http.or(https);
        if explicit.is_none() && s.contains("://") {
            return Err(RexError::InvalidEndpoint("unsupported endpoint scheme".into()));
        }
        if explicit.is_none() && (s.contains('@') || !s.contains(['.', ':', '['])) {
            return Ok(Self::Ssh { target: SshTarget::parse(s)?, pool: ssh_pool, askpass });
        }
        let raw_authority = explicit.unwrap_or(s);
        let authority = raw_authority.strip_suffix('/').unwrap_or(raw_authority);
        if authority.contains(['@', '/', '?', '#', '\\']) {
            return Err(RexError::InvalidEndpoint("expected an HTTP host and optional port".into()));
        }
        let target = SshTarget::parse(authority)?;
        let host = match url::Host::parse(&authority_host(&target.host)).map_err(|e| RexError::InvalidEndpoint(e.to_string()))? {
            url::Host::Ipv6(ip) => ip.to_string(),
            host => host.to_string(),
        };
        if http.is_some() || explicit.is_none() && target.port.is_some() {
            Ok(Self::Http { host, port: target.port.unwrap_or(7297) })
        } else {
            Ok(Self::Https { host, port: target.port.unwrap_or(443) })
        }
    }

    /// Display label for UI/diagnostics.
    pub fn display(&self) -> String {
        match self {
            Self::Local { .. } => "Local".to_owned(),
            Self::Unix(p) => format!("unix://{}", p.display()),
            Self::Http { host, port } => format!("http://{}:{port}", authority_host(host)),
            Self::Https { host, port } => {
                if *port == 443 {
                    format!("https://{}", authority_host(host))
                } else {
                    format!("https://{}:{port}", authority_host(host))
                }
            }
            Self::Ssh { target, .. } => target.display(),
        }
    }

    /// [`Self::connect_stream`] for the control connection: over SSH it
    /// goes through Olo's helper on the host where that is installed
    /// ([`SshSessionPool::set_helper`]), which the connection then says
    /// (`ControlClient::keeps_typing_in_order`). The same stream
    /// everywhere else.
    pub async fn connect_control_stream(&self) -> Result<(RexStream, String), RexError> {
        match self {
            Self::Ssh { target, pool, askpass } => {
                let chan = pool.open_rex_channel(target, askpass.clone(), true).await?;
                // (What is sent a host over SSH goes under a mask of
                // nothing, so that SSH can pack it: `plain`.)
                Ok((RexStream::Boxed(Box::pin(crate::plain::PlainMask::new(chan))), "localhost".to_owned()))
            }
            _ => self.connect_stream().await,
        }
    }

    /// The host is reached, as far as that can be said before a stream
    /// is opened to it: over SSH, connected and signed in to (the
    /// person asked what there is to ask, once). After it, streams
    /// opened side by side ([`Self::connect_control_stream`],
    /// [`Self::connect_stream`]) each cost a channel and nothing more,
    /// and none of them asks the person anything.
    pub async fn reach(&self) -> Result<(), RexError> {
        match self {
            Self::Ssh { target, pool, askpass } => pool.reach(target, askpass.clone()).await,
            _ => Ok(()),
        }
    }

    /// Whether a control connection's hello and first requests may be
    /// sent in one flight with its upgrade
    /// ([`ControlClient::connect_asking`](crate::ControlClient::connect_asking)):
    /// where nothing between here and the Rex server reads HTTP (its
    /// own socket, or an SSH channel to it). Not through a web server
    /// in front of it, which may not pass on what comes behind an
    /// upgrade's request before its answer.
    pub fn takes_a_flight(&self) -> bool {
        matches!(self, Self::Local { .. } | Self::Unix(_) | Self::Ssh { .. })
    }

    /// Open a connected [`RexStream`] and the HTTP `Host` header value to use for
    /// WebSocket upgrade on `/api/control` or `/api/data`.
    ///
    /// Over SSH the stream is for a WebSocket connection and nothing
    /// else: what is written to it is read as an upgrade's request and
    /// then frames, whose masks are taken off ([`crate::plain`]).
    pub async fn connect_stream(&self) -> Result<(RexStream, String), RexError> {
        match self {
            Self::Local { autostart } => {
                let sock = discover_or_start_local_socket(*autostart).await?;
                let stream = UnixStream::connect(&sock).await.map_err(|e| RexError::ServerUnavailable(format!("connect {}: {e}", sock.display())))?;
                Ok((RexStream::Unix(stream), "localhost".to_owned()))
            }
            Self::Unix(path) => {
                let stream = UnixStream::connect(path).await?;
                Ok((RexStream::Unix(stream), "localhost".to_owned()))
            }
            Self::Http { host, port } => {
                let tcp = tokio::time::timeout(Duration::from_secs(10), TcpStream::connect((host.as_str(), *port))).await.map_err(|_| RexError::Timeout)??;
                let _ = tcp.set_nodelay(true);
                Ok((RexStream::Tcp(tcp), format!("{}:{port}", authority_host(host))))
            }
            Self::Https { host, port } => {
                let tcp = tokio::time::timeout(Duration::from_secs(10), TcpStream::connect((host.as_str(), *port))).await.map_err(|_| RexError::Timeout)??;
                let _ = tcp.set_nodelay(true);
                let tls_cfg = build_tls_config()?;
                let connector = TlsConnector::from(Arc::new(tls_cfg));
                let server_name = ServerName::try_from(host.clone()).map_err(|e| RexError::Tls(format!("invalid server name {host}: {e}")))?;
                let tls = tokio::time::timeout(Duration::from_secs(10), connector.connect(server_name, tcp))
                    .await
                    .map_err(|_| RexError::Timeout)?
                    .map_err(|e| RexError::Tls(format!("TLS handshake with {host}:{port} failed: {e}")))?;
                let host_hdr = if *port == 443 { authority_host(host) } else { format!("{}:{port}", authority_host(host)) };
                Ok((RexStream::Tls(Box::new(tls)), host_hdr))
            }
            Self::Ssh { target, pool, askpass } => {
                let chan = pool.open_rex_channel(target, askpass.clone(), false).await?;
                Ok((RexStream::Boxed(Box::pin(crate::plain::PlainMask::new(chan))), "localhost".to_owned()))
            }
        }
    }
}

/// Unified async stream over Unix sockets, TCP, TLS, or SSH channels.
pub enum RexStream {
    Unix(UnixStream),
    Tcp(TcpStream),
    Tls(Box<tokio_rustls::client::TlsStream<TcpStream>>),
    Boxed(Pin<Box<dyn AsyncReadWrite + Send>>),
}

pub trait AsyncReadWrite: AsyncRead + AsyncWrite + Unpin {}
impl<T: AsyncRead + AsyncWrite + Unpin> AsyncReadWrite for T {}

impl AsyncRead for RexStream {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Unix(s) => Pin::new(s).poll_read(cx, buf),
            Self::Tcp(s) => Pin::new(s).poll_read(cx, buf),
            Self::Tls(s) => Pin::new(s.as_mut()).poll_read(cx, buf),
            Self::Boxed(s) => s.as_mut().poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for RexStream {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<std::io::Result<usize>> {
        match self.get_mut() {
            Self::Unix(s) => Pin::new(s).poll_write(cx, buf),
            Self::Tcp(s) => Pin::new(s).poll_write(cx, buf),
            Self::Tls(s) => Pin::new(s.as_mut()).poll_write(cx, buf),
            Self::Boxed(s) => s.as_mut().poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Unix(s) => Pin::new(s).poll_flush(cx),
            Self::Tcp(s) => Pin::new(s).poll_flush(cx),
            Self::Tls(s) => Pin::new(s.as_mut()).poll_flush(cx),
            Self::Boxed(s) => s.as_mut().poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Unix(s) => Pin::new(s).poll_shutdown(cx),
            Self::Tcp(s) => Pin::new(s).poll_shutdown(cx),
            Self::Tls(s) => Pin::new(s.as_mut()).poll_shutdown(cx),
            Self::Boxed(s) => s.as_mut().poll_shutdown(cx),
        }
    }
}

fn build_tls_config() -> Result<ClientConfig, RexError> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let platform = ClientConfig::builder_with_provider(provider.clone()).with_safe_default_protocol_versions().and_then(|b| b.with_platform_verifier()).map(|b| b.with_no_client_auth());
    platform
        .or_else(|_| {
            let mut roots = rustls::RootCertStore::empty();
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            Ok::<ClientConfig, rustls::Error>(ClientConfig::builder_with_provider(provider).with_safe_default_protocol_versions()?.with_root_certificates(roots).with_no_client_auth())
        })
        .map_err(|e| RexError::Tls(format!("tls config failed: {e}")))
}

/// Candidate Unix socket paths for the local Rex server.
pub fn local_socket_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(env_sock) = std::env::var("REX_SERVER_SOCKET")
        && !env_sock.is_empty()
    {
        return vec![PathBuf::from(env_sock.strip_prefix("unix://").unwrap_or(&env_sock))];
    }
    if let Ok(xdg_rt) = std::env::var("XDG_RUNTIME_DIR")
        && !xdg_rt.is_empty()
    {
        let rt = PathBuf::from(&xdg_rt);
        out.push(rt.join("rex/rex.sock"));
        out.push(rt.join("rex/server.sock"));
        out.push(rt.join("rex.sock"));
    }
    if let Ok(home) = std::env::var("HOME") {
        let h = PathBuf::from(home);
        out.push(h.join(".local/state/rex/rex.sock"));
        out.push(h.join(".local/state/rex/server.sock"));
        out.push(h.join(".rex/rex.sock"));
    }
    let uid = unsafe { libc::geteuid() };
    out.push(PathBuf::from(format!("/run/user/{uid}/rex/rex.sock")));
    out.push(PathBuf::from(format!("/tmp/rex-{uid}/rex.sock")));
    out
}

/// Discover the local Rex Unix socket, optionally autostarting `rex server start`
/// if no live local server is listening yet.
pub async fn discover_or_start_local_socket(autostart: bool) -> Result<PathBuf, RexError> {
    for path in local_socket_candidates() {
        if is_live_unix_socket(&path).await {
            return Ok(path);
        }
    }

    if std::env::var_os("REX_SERVER_SOCKET").is_some_and(|s| !s.is_empty()) {
        return Err(RexError::ServerUnavailable("no listener at REX_SERVER_SOCKET".to_owned()));
    }

    // Ask `rex server status --json` if `rex` is installed on PATH (it knows its exact default path).
    if let Some(p) = probe_rex_cli_socket().await {
        if is_live_unix_socket(&p).await {
            return Ok(p);
        }
    }

    if !autostart {
        return Err(RexError::ServerUnavailable("local Rex server is not running (no listening Unix socket found)".to_owned()));
    }

    // Start local server via `rex server start` (daemonized by rex itself).
    let status = tokio::process::Command::new(find_rex_binary())
        .args(["server", "start"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
        .map_err(|e| RexError::ServerUnavailable(format!("failed to launch `rex server start`: {e}")))?;
    if !status.status.success() {
        let err = String::from_utf8_lossy(&status.stderr);
        return Err(RexError::ServerUnavailable(format!("`rex server start` failed: {}", err.trim())));
    }

    for _ in 0..30 {
        for path in local_socket_candidates() {
            if is_live_unix_socket(&path).await {
                return Ok(path);
            }
        }
        if let Some(p) = probe_rex_cli_socket().await
            && is_live_unix_socket(&p).await
        {
            return Ok(p);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    Err(RexError::ServerUnavailable("started local Rex server, but its Unix socket did not appear in time".to_owned()))
}

async fn is_live_unix_socket(path: &Path) -> bool {
    path.exists() && UnixStream::connect(path).await.is_ok()
}

pub fn find_rex_binary() -> PathBuf {
    for candidate in ["/usr/bin/rex", "/usr/local/bin/rex"] {
        let p = PathBuf::from(candidate);
        if p.is_file() {
            return p;
        }
    }
    PathBuf::from("rex")
}

async fn probe_rex_cli_socket() -> Option<PathBuf> {
    let out = tokio::process::Command::new(find_rex_binary())
        .args(["--autostart=false", "server", "status", "--json"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let val: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    let listeners = val.get("listeners")?.as_array()?;
    for l in listeners {
        if l.get("transport").and_then(|v| v.as_str()) == Some("unix") {
            if let Some(addr) = l.get("addresses").and_then(|v| v.as_array()).and_then(|a| a.first()).and_then(|v| v.as_str()) {
                return Some(PathBuf::from(addr.strip_prefix("unix://").unwrap_or(addr)));
            }
            if let Some(ep) = l.get("endpoint").and_then(|v| v.as_str()) {
                return Some(PathBuf::from(ep.strip_prefix("unix://").unwrap_or(ep)));
            }
        }
    }
    None
}
