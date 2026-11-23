//! Native Rust SSH transport (`russh` + `ssh2-config`) for reaching remote Rex
//! servers without shelling out to `ssh`.
//!
//! Features:
//! - Honors `~/.ssh/config` (`HostName`, `User`, `Port`, `IdentityFile`, `ProxyCommand`)
//! - Connects to Olo Passwords' `SSH_AUTH_SOCK` (or a user-selected custom socket)
//!   and reports agent availability + loaded key count
//! - Native askpass callback ([`SshAskpass`]) for `known_hosts` verification,
//!   key passphrases, passwords, and keyboard-interactive 2FA prompts
//! - Opens `direct-streamlocal@openssh.com` to remote Rex Unix sockets, falling
//!   back to `rex __internal dial-stdio` over an SSH exec channel
//! - Pools authenticated SSH sessions so `/api/control` and all `/api/data`
//!   streams multiplex over a single SSH connection!

use std::{
    collections::HashMap,
    fs::File,
    io::{self, BufReader},
    net::Ipv6Addr,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
    time::Duration,
};

use russh::{
    MethodKind,
    client::{self, AuthResult, Handle, KeyboardInteractiveAuthResponse},
    keys::{
        PublicKeyOrCertificate,
        agent::{AgentIdentity, client::AgentClient},
        known_hosts::{check_known_hosts, learn_known_hosts},
        load_secret_key,
    },
};
use ssh2_config::{ParseRule, SshConfig};
use tokio::{
    io::{AsyncRead, AsyncReadExt, ReadBuf},
    sync::Mutex,
};
use zeroize::Zeroizing;

use crate::{error::RexError, transport::AsyncReadWrite};

/// Parsed SSH target (`[user@]host[:port][/remote/socket.sock]`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SshTarget {
    pub host: String,
    pub user: Option<String>,
    pub port: Option<u16>,
    /// Optional explicit remote Unix socket path; if `None`, discovers or runs
    /// `rex __internal dial-stdio`.
    pub socket_path: Option<String>,
    /// Optional explicit SSH agent socket path overriding `SSH_AUTH_SOCK`.
    pub agent_socket: Option<PathBuf>,
    /// Sign in with this key of the agent's alone (its SHA-256
    /// fingerprint, as [`AgentKey::fingerprint`] has it): the person chose
    /// it. A server lets a client try only so many keys (OpenSSH: six),
    /// so with many keys in the agent the right one may never be offered.
    pub only_key: Option<String>,
    /// Sign in with keys or not at all: no password or other question is
    /// put to the person (root, tried to set a host up).
    pub keys_only: bool,
}

impl SshTarget {
    /// The same host as root, signed in with keys alone (`key`: only that
    /// one of the agent's): for setting the host up, never for a
    /// connection that is kept.
    pub fn as_root(&self, key: Option<String>) -> SshTarget {
        SshTarget { user: Some("root".to_owned()), socket_path: None, only_key: key, keys_only: true, ..self.clone() }
    }

    pub fn parse(raw: &str) -> Result<Self, RexError> {
        let s = raw.trim().strip_prefix("ssh://").unwrap_or(raw.trim());
        if s.is_empty() {
            return Err(RexError::InvalidEndpoint("empty ssh target".to_owned()));
        }
        let (authority, socket_path) = match s.find('/') {
            Some(idx) => (&s[..idx], Some(s[idx..].to_owned())),
            None => (s, None),
        };
        let (user, host_port) = match authority.rsplit_once('@') {
            Some((u, hp)) => (Some(u.to_owned()), hp),
            None => (None, authority),
        };
        let invalid = || RexError::InvalidEndpoint(format!("invalid SSH authority in {raw}"));
        if authority.chars().any(|c| c.is_whitespace() || c.is_control()) || user.as_deref() == Some("") {
            return Err(invalid());
        }
        let (host, port) = if let Some(bracketed) = host_port.strip_prefix('[') {
            let (host, suffix) = bracketed.split_once(']').ok_or_else(invalid)?;
            host.parse::<Ipv6Addr>().map_err(|_| invalid())?;
            let port = if suffix.is_empty() { None } else { Some(suffix.strip_prefix(':').ok_or_else(invalid)?) };
            (host, port)
        } else if host_port.parse::<Ipv6Addr>().is_ok() {
            (host_port, None)
        } else {
            host_port.split_once(':').map_or((host_port, None), |(host, port)| (host, Some(port)))
        };
        if host.is_empty() || host.contains(['[', ']']) || host.starts_with('-') {
            return Err(invalid());
        }
        let port = port.map(|p| p.parse::<u16>()).transpose().map_err(|_| invalid())?;
        if port == Some(0) {
            return Err(invalid());
        }
        let host = host.to_owned();
        let socket_path = socket_path.filter(|s| s != "/");
        Ok(Self { host, user, port, socket_path, agent_socket: None, only_key: None, keys_only: false })
    }

    pub fn display(&self) -> String {
        let mut s = String::from("ssh://");
        if let Some(u) = &self.user {
            s.push_str(u);
            s.push('@');
        }
        s.push_str(&crate::transport::authority_host(&self.host));
        if let Some(p) = self.port {
            s.push(':');
            s.push_str(&p.to_string());
        }
        if let Some(sp) = &self.socket_path {
            s.push_str(sp);
        }
        s
    }
}

/// Interactive authentication & host-key verification prompt callback implemented
/// by the Olo UI (native sheets) or headless test stubs.
pub trait SshAskpass: Send + Sync + std::fmt::Debug {
    /// Ask whether to trust an unknown SSH host key. A known key that changes is refused.
    fn confirm_host_key(&self, host: &str, port: u16, fingerprint: &str, key_type: &str, changed: bool) -> bool;

    /// Ask for a passphrase or password (`echo = false` for secrets).
    fn prompt_secret(&self, title: &str, prompt: &str, echo: bool) -> Option<Zeroizing<String>>;

    /// The key agent was asked to sign for `host` and has not answered
    /// for a moment (`waiting`: it may be locked, or asking the person),
    /// or has now. Called on the connection's task: note it and return.
    fn waiting_for_agent(&self, _host: &str, _waiting: bool) {}
}

/// How long the key agent has to sign before the app is told it is
/// being waited for.
const AGENT_SLOW: Duration = Duration::from_secs(3);

/// Live status of an SSH agent socket (Olo Passwords or custom agent).
#[derive(Debug, Clone, Default)]
pub struct AgentStatus {
    pub socket_path: Option<PathBuf>,
    pub reachable: bool,
    pub key_count: usize,
    pub key_comments: Vec<String>,
    /// Its keys, as a person would choose among them.
    pub keys: Vec<AgentKey>,
    pub error: Option<String>,
}

/// A key the agent holds: its name there (a key's comment; in Olo
/// Passwords, the item's), its kind, and its fingerprint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentKey {
    pub name: String,
    /// "ED25519", "RSA", "ECDSA".
    pub kind: String,
    /// `SHA256:…`, as `ssh-keygen -l` and a server's log show it.
    pub fingerprint: String,
}

impl AgentKey {
    fn of(identity: &AgentIdentity) -> AgentKey {
        let (name, key) = match identity {
            AgentIdentity::PublicKey { key, comment } => (comment.clone(), key.clone()),
            AgentIdentity::Certificate { certificate, comment } => (comment.clone(), russh::keys::PublicKey::from(certificate.public_key().clone())),
        };
        let kind = match key.algorithm().as_str() {
            a if a.contains("ed25519") => "ED25519".to_owned(),
            a if a.contains("rsa") => "RSA".to_owned(),
            a if a.contains("ecdsa") => "ECDSA".to_owned(),
            other => other.to_owned(),
        };
        AgentKey { name, kind, fingerprint: key.fingerprint(russh::keys::HashAlg::Sha256).to_string() }
    }
}

/// Resolve the preferred SSH agent socket path: explicit override → `SSH_AUTH_SOCK`
/// → Olo Passwords' own SSH agent socket (`olo_xdg::ssh_agent_socket()`).
pub fn resolve_agent_socket(override_path: Option<&Path>) -> Option<PathBuf> {
    if let Some(p) = override_path
        && !p.as_os_str().is_empty()
    {
        return Some(p.to_path_buf());
    }
    if let Ok(env_sock) = std::env::var("SSH_AUTH_SOCK")
        && !env_sock.is_empty()
    {
        let p = PathBuf::from(env_sock);
        if p.exists() {
            return Some(p);
        }
    }
    let olo_sock = olo_xdg::ssh_agent_socket();
    if olo_sock.exists() {
        return Some(olo_sock);
    }
    None
}

/// Inspect an SSH agent socket to report reachability, key count, and comments.
pub async fn inspect_ssh_agent(override_path: Option<&Path>) -> AgentStatus {
    let Some(sock) = resolve_agent_socket(override_path) else {
        return AgentStatus { error: Some("No SSH agent socket found".to_owned()), ..Default::default() };
    };
    let query = async {
        match AgentClient::connect_uds(&sock).await {
            Ok(mut client) => match client.request_identities().await {
                Ok(ids) => {
                    let comments = ids
                        .iter()
                        .map(|id| match id {
                            AgentIdentity::PublicKey { comment, .. } => comment.clone(),
                            AgentIdentity::Certificate { comment, .. } => comment.clone(),
                        })
                        .collect::<Vec<_>>();
                    AgentStatus { socket_path: Some(sock.clone()), reachable: true, key_count: ids.len(), key_comments: comments, keys: ids.iter().map(AgentKey::of).collect(), error: None }
                }
                Err(e) => AgentStatus { socket_path: Some(sock.clone()), reachable: true, error: Some(format!("agent list failed: {e}")), ..Default::default() },
            },
            Err(e) => AgentStatus { socket_path: Some(sock.clone()), reachable: false, error: Some(format!("connect failed: {e}")), ..Default::default() },
        }
    };
    tokio::time::timeout(Duration::from_secs(5), query).await.unwrap_or_else(|_| AgentStatus { socket_path: Some(sock), error: Some("SSH agent timed out".into()), ..Default::default() })
}

#[derive(Clone)]
struct Prompts {
    ui: Arc<dyn SshAskpass>,
    waiting: tokio::sync::watch::Sender<bool>,
}

impl Prompts {
    async fn ask<T: Send + 'static>(&self, prompt: impl FnOnce(&dyn SshAskpass) -> T + Send + 'static) -> Result<T, tokio::task::JoinError> {
        let ui = self.ui.clone();
        self.waiting.send_replace(true);
        let result = tokio::task::spawn_blocking(move || prompt(ui.as_ref())).await;
        self.waiting.send_replace(false);
        result
    }

    async fn secret(&self, title: &str, message: &str, echo: bool) -> Result<Zeroizing<String>, RexError> {
        let (title, message) = (title.to_owned(), message.to_owned());
        self.ask(move |ui| ui.prompt_secret(&title, &message, echo)).await.map_err(|e| RexError::Ssh(e.to_string()))?.ok_or_else(cancelled)
    }
}

/// An answer from the key agent, the app told while it is slow to come
/// (and that it has come, or that the asking was given up). The
/// connection's deadline waits meanwhile, as it does while the person
/// reads a question: an agent that is locked answers when it is
/// unlocked, and the host's own patience bounds the wait.
async fn signed<T>(prompts: Option<&Prompts>, host: &str, sign: impl Future<Output = T>) -> T {
    struct Told<'a>(Option<&'a Prompts>, &'a str);
    impl Drop for Told<'_> {
        fn drop(&mut self) {
            if let Some(p) = self.0 {
                p.ui.waiting_for_agent(self.1, false);
                p.waiting.send_replace(false);
            }
        }
    }
    tokio::pin!(sign);
    tokio::select! {
        biased;
        answer = &mut sign => return answer,
        _ = tokio::time::sleep(AGENT_SLOW) => {}
    }
    let _told = Told(prompts, host);
    if let Some(p) = prompts {
        p.ui.waiting_for_agent(host, true);
        p.waiting.send_replace(true);
    }
    sign.await
}

// Network deadlines pause while the person is reading a sheet. The caller
// keeps the watch sender alive throughout the handshake, even without a UI.
async fn network_timeout<T>(limit: Duration, waiting: &mut tokio::sync::watch::Receiver<bool>, operation: impl Future<Output = Result<T, RexError>>) -> Result<T, RexError> {
    tokio::pin!(operation);
    let mut remaining = limit;
    loop {
        let paused = *waiting.borrow_and_update();
        let start = tokio::time::Instant::now();
        tokio::select! {
            result = &mut operation => return result,
            _ = tokio::time::sleep(remaining), if !paused => return Err(RexError::Timeout),
            _ = waiting.changed() => {}
        }
        if !paused {
            remaining = remaining.saturating_sub(start.elapsed());
        }
    }
}

struct SshClientHandler {
    host: String,
    port: u16,
    askpass: Option<Prompts>,
}

impl client::Handler for SshClientHandler {
    type Error = russh::Error;

    async fn check_server_key(&mut self, server_public_key: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        let pubkey = match server_public_key {
            PublicKeyOrCertificate::PublicKey { key, .. } => key.clone(),
            PublicKeyOrCertificate::Certificate(cert) => russh::keys::PublicKey::from(cert.public_key().clone()),
        };

        match check_known_hosts(&self.host, self.port, &pubkey) {
            Ok(true) => Ok(true),
            Ok(false) => {
                let fp = pubkey.fingerprint(russh::keys::HashAlg::Sha256).to_string();
                let kt = pubkey.algorithm().to_string();
                if let Some(ask) = &self.askpass {
                    let (host, port) = (self.host.clone(), self.port);
                    let ok = ask.ask(move |ui| ui.confirm_host_key(&host, port, &fp, &kt, false)).await.map_err(io::Error::other)?;
                    if ok {
                        learn_known_hosts(&self.host, self.port, &pubkey)?;
                    }
                    Ok(ok)
                } else {
                    tracing::warn!("rejecting unknown SSH host key for {}:{} ({fp})", self.host, self.port);
                    Ok(false)
                }
            }
            Err(error) => Err(error.into()),
        }
    }
}

/// Pool of live authenticated SSH sessions keyed by target, allowing `/api/control`
/// and every `/api/data` stream for a remote host to share one SSH connection.
#[derive(Default)]
pub struct SshSessionPool {
    sessions: Mutex<HashMap<SshTarget, SessionSlot>>,
}

type SessionSlot = Arc<Mutex<Option<Arc<Handle<SshClientHandler>>>>>;

impl std::fmt::Debug for SshSessionPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SshSessionPool").finish_non_exhaustive()
    }
}

impl SshSessionPool {
    /// Drop any cached SSH session for `target` (e.g. after disconnect/reconnect).
    pub async fn invalidate(&self, target: &SshTarget) {
        self.sessions.lock().await.remove(target);
    }

    /// Open a bidirectional byte stream to the remote Rex server over SSH:
    /// tries `direct-streamlocal@openssh.com` if an explicit socket path is
    /// configured, otherwise launches `rex __internal dial-stdio` over an SSH
    /// exec channel (which automatically discovers or starts the remote Rex server).
    pub async fn open_rex_channel(&self, target: &SshTarget, askpass: Option<Arc<dyn SshAskpass>>) -> Result<Pin<Box<dyn AsyncReadWrite + Send>>, RexError> {
        let handle = self.get_or_connect(target, askpass).await?;
        tokio::time::timeout(Duration::from_secs(10), Self::channel(&handle, target)).await.map_err(|_| RexError::Timeout)?
    }

    /// Runs `command` on the host, in a shell there, and waits for it:
    /// what it writes (both its outputs, as they come) goes to `said`
    /// line by line and comes back whole with its exit status. For
    /// setting a host up and looking after it ([`crate::provision`]).
    pub async fn exec(&self, target: &SshTarget, askpass: Option<Arc<dyn SshAskpass>>, command: &str, said: &mut (dyn FnMut(&str) + Send)) -> Result<crate::provision::Ran, RexError> {
        use russh::ChannelMsg;
        let handle = self.get_or_connect(target, askpass).await?;
        let mut channel = handle.channel_open_session().await.map_err(|e| RexError::Ssh(format!("open ssh session channel: {e}")))?;
        channel.exec(true, command).await.map_err(|e| RexError::Ssh(format!("exec on {} failed: {e}", target.display())))?;
        let mut ran = crate::provision::Ran::default();
        // (Each of its two outputs is cut into lines by itself: a line
        // half written on one isn't finished by the other.)
        fn take(pending: &mut Vec<u8>, bytes: &[u8], ran: &mut crate::provision::Ran, said: &mut (dyn FnMut(&str) + Send)) {
            pending.extend_from_slice(bytes);
            while let Some(end) = pending.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = pending.drain(..=end).collect();
                let line = String::from_utf8_lossy(&line[..line.len() - 1]).trim_end_matches('\r').to_owned();
                said(&line);
                ran.output.push_str(&line);
                ran.output.push('\n');
            }
        }
        let (mut out, mut err) = (Vec::new(), Vec::new());
        loop {
            match channel.wait().await {
                Some(ChannelMsg::Data { data }) => take(&mut out, &data, &mut ran, said),
                Some(ChannelMsg::ExtendedData { data, .. }) => take(&mut err, &data, &mut ran, said),
                Some(ChannelMsg::ExitStatus { exit_status }) => ran.status = Some(exit_status),
                Some(ChannelMsg::Close) | None => break,
                _ => {}
            }
        }
        // (A last line with no end.)
        for pending in [&mut out, &mut err] {
            if !pending.is_empty() {
                take(pending, b"\n", &mut ran, said);
            }
        }
        Ok(ran)
    }

    async fn channel(handle: &Handle<SshClientHandler>, target: &SshTarget) -> Result<Pin<Box<dyn AsyncReadWrite + Send>>, RexError> {
        if let Some(sock) = &target.socket_path {
            match handle.channel_open_direct_streamlocal(sock.clone()).await {
                Ok(ch) => return Ok(Box::pin(ch.into_stream())),
                Err(e) => {
                    tracing::debug!("direct-streamlocal {sock} failed ({e}), falling back to dial-stdio");
                }
            }
        }

        // Standard Rex SSH transport: `rex __internal dial-stdio` (auto-discovers
        // remote Rex Unix socket and autostart if needed).
        let channel = handle.channel_open_session().await.map_err(|e| RexError::Ssh(format!("open ssh session channel: {e}")))?;
        let cmd = if let Some(sock) = &target.socket_path { format!("rex -S unix://{} __internal dial-stdio", shell_escape(sock)) } else { "rex __internal dial-stdio".to_owned() };
        channel.exec(true, cmd).await.map_err(|e| RexError::Ssh(format!("exec `{}` failed: {e}", target.display())))?;
        let writer = channel.make_writer();
        let reader = ExecOutput::Starting(Some(channel));
        Ok(Box::pin(tokio::io::join(reader, writer)))
    }

    async fn get_or_connect(&self, target: &SshTarget, askpass: Option<Arc<dyn SshAskpass>>) -> Result<Arc<Handle<SshClientHandler>>, RexError> {
        let slot = self.sessions.lock().await.entry(target.clone()).or_default().clone();
        let mut session = slot.lock().await;
        if let Some(handle) = session.as_ref().filter(|h| !h.is_closed()) {
            return Ok(handle.clone());
        }
        let handle = Arc::new(connect_and_authenticate(target, askpass).await?);
        *session = Some(handle.clone());
        Ok(handle)
    }
}

// Before the first HTTP byte, a shell's exit status explains why dial-stdio
// didn't start. Once it replies, use russh's ordinary streaming reader.
enum ExecOutput {
    Starting(Option<russh::Channel<client::Msg>>),
    Open(tokio::io::Chain<io::Cursor<bytes::Bytes>, russh::ChannelStream<client::Msg>>),
}
impl AsyncRead for ExecOutput {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>, buf: &mut ReadBuf<'_>) -> std::task::Poll<io::Result<()>> {
        use russh::ChannelMsg;
        use std::task::{Poll, ready};
        loop {
            match self.as_mut().get_mut() {
                Self::Open(stream) => return Pin::new(stream).poll_read(cx, buf),
                Self::Starting(channel) => {
                    let ch = channel.as_mut().expect("channel present until first response");
                    let message = ready!(std::pin::pin!(ch.wait()).poll(cx));
                    match message {
                        Some(ChannelMsg::Data { data }) => {
                            let stream = channel.take().unwrap().into_stream();
                            *self = Self::Open(io::Cursor::new(data).chain(stream));
                        }
                        Some(ChannelMsg::ExitStatus { exit_status: 127 }) => {
                            return Poll::Ready(Err(io::Error::other("remote rex command not found; install Rex on the remote host and add it to PATH")));
                        }
                        Some(ChannelMsg::ExitStatus { exit_status }) => return Poll::Ready(Err(io::Error::other(format!("remote rex dial-stdio exited with status {exit_status}")))),
                        Some(ChannelMsg::Failure) => return Poll::Ready(Err(io::Error::other("remote SSH server refused dial-stdio"))),
                        Some(ChannelMsg::Close) | None => return Poll::Ready(Err(io::Error::other("remote SSH channel closed before dial-stdio replied"))),
                        _ => {}
                    }
                }
            }
        }
    }
}

impl Drop for ExecOutput {
    fn drop(&mut self) {
        if let Self::Starting(ch) = self
            && let Some(ch) = ch.take()
        {
            drop(ch.into_stream()); // ChannelStream sends SSH_MSG_CHANNEL_CLOSE on drop.
        }
    }
}

struct ResolvedSshConfig {
    hostname: String,
    port: u16,
    user: String,
    identity_files: Vec<PathBuf>,
    keepalive_secs: u64,
    keepalive_max: usize,
    connect_timeout: Duration,
    proxy_command: Option<String>,
}

fn resolve_ssh_config(target: &SshTarget) -> Result<ResolvedSshConfig, RexError> {
    let default_user = std::env::var("USER").unwrap_or_else(|_| "root".to_owned());
    let mut cfg = ResolvedSshConfig {
        hostname: target.host.clone(),
        port: target.port.unwrap_or(22),
        user: target.user.clone().unwrap_or(default_user),
        identity_files: Vec::new(),
        keepalive_secs: 30,
        keepalive_max: 3,
        connect_timeout: Duration::from_secs(10),
        proxy_command: None,
    };

    if let Ok(home) = std::env::var("HOME") {
        let ssh_dir = PathBuf::from(&home).join(".ssh");
        let cfg_path = ssh_dir.join("config");
        if let Ok(file) = File::open(&cfg_path) {
            let mut reader = BufReader::new(file);
            let parsed =
                SshConfig::default().parse(&mut reader, ParseRule::ALLOW_UNKNOWN_FIELDS | ParseRule::ALLOW_UNSUPPORTED_FIELDS).map_err(|e| RexError::Ssh(format!("reading SSH config: {e}")))?;
            let params = parsed.query(&target.host);
            if let Some(limit) = params.connect_timeout.filter(|t| !t.is_zero()) {
                cfg.connect_timeout = limit;
            }
            if let Some(hn) = params.host_name {
                cfg.hostname = hn;
            }
            if target.port.is_none()
                && let Some(p) = params.port
            {
                cfg.port = p;
            }
            if target.user.is_none()
                && let Some(u) = params.user
            {
                cfg.user = u;
            }
            if let Some(files) = params.identity_file {
                for f in files {
                    cfg.identity_files.push(expand_tilde(&f, &home));
                }
            }
            if let Some(ka) = params.server_alive_interval {
                cfg.keepalive_secs = ka.as_secs();
            }
            if let Some(value) = params.unsupported_fields.get("serveralivecountmax").and_then(|v| v.first()) {
                cfg.keepalive_max = value.parse().map_err(|_| RexError::Ssh("invalid ServerAliveCountMax".into()))?;
            }
            if let Some(parts) = params.unsupported_fields.get("proxycommand") {
                let command = parts.join(" ");
                if !command.is_empty() && !command.eq_ignore_ascii_case("none") {
                    cfg.proxy_command = Some(command);
                }
            }
            if cfg.proxy_command.is_none() && params.proxy_jump.is_some_and(|jumps| !jumps.is_empty() && jumps != ["none"]) {
                return Err(RexError::Ssh("ProxyJump is unsupported. Configure a ProxyCommand for this host.".into()));
            }
        }
        for name in ["id_ed25519", "id_ecdsa", "id_rsa"] {
            let p = ssh_dir.join(name);
            if p.exists() && !cfg.identity_files.contains(&p) {
                cfg.identity_files.push(p);
            }
        }
    }
    Ok(cfg)
}

fn expand_tilde(path: &Path, home: &str) -> PathBuf {
    if let Some(s) = path.to_str()
        && let Some(rest) = s.strip_prefix("~/")
    {
        return PathBuf::from(home).join(rest);
    }
    path.to_path_buf()
}

struct ChildStdioStream {
    _child: tokio::process::Child,
    stdout: tokio::process::ChildStdout,
    stdin: tokio::process::ChildStdin,
}

impl tokio::io::AsyncRead for ChildStdioStream {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>, buf: &mut tokio::io::ReadBuf<'_>) -> std::task::Poll<std::io::Result<()>> {
        Pin::new(&mut self.stdout).poll_read(cx, buf)
    }
}

impl tokio::io::AsyncWrite for ChildStdioStream {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>, buf: &[u8]) -> std::task::Poll<std::io::Result<usize>> {
        Pin::new(&mut self.stdin).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<std::io::Result<()>> {
        Pin::new(&mut self.stdin).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<std::io::Result<()>> {
        Pin::new(&mut self.stdin).poll_shutdown(cx)
    }
}

async fn connect_and_authenticate(target: &SshTarget, askpass: Option<Arc<dyn SshAskpass>>) -> Result<Handle<SshClientHandler>, RexError> {
    let resolved = resolve_ssh_config(target)?;
    let mut config = client::Config::default();
    // A small write goes when it is written. russh leaves Nagle's
    // algorithm on, which holds one back until what went before it is
    // acknowledged (a round trip of the link) and sends the two
    // together: two requests in one packet, which the server then takes
    // at once and may answer out of order. OpenSSH turns it off for a
    // terminal, and every other way Rex reaches a server has it off
    // (`transport`). Measured over a real link of 167 to 229 ms
    // (2026-11-23, `tests/host.rs`, typing): no change to what is typed
    // one write at a time, which is how Rex types (a letter's echo at
    // 241 to 355 ms with it on, 255 to 355 ms with it off, another
    // terminal printing or not); requests sent on each other's heels
    // are answered in one round trip, not one and a half (183 and 228
    // ms for 256 and 347), and in the order sent.
    config.nodelay = true;
    config.keepalive_interval = (resolved.keepalive_secs > 0).then(|| Duration::from_secs(resolved.keepalive_secs));
    config.keepalive_max = resolved.keepalive_max;
    let config = Arc::new(config);

    let (waiting_tx, mut waiting) = tokio::sync::watch::channel(false);
    let askpass = askpass.map(|ui| Prompts { ui, waiting: waiting_tx.clone() });
    let handler = SshClientHandler { host: resolved.hostname.clone(), port: resolved.port, askpass: askpass.clone() };
    let connection = async {
        if let Some(pc) = &resolved.proxy_command {
            let expanded = expand_proxy(pc, &resolved)?;
            let mut child = tokio::process::Command::new("/bin/sh")
                .arg("-c")
                .arg(&expanded)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .kill_on_drop(true)
                .spawn()
                .map_err(|e| RexError::Ssh(format!("ProxyCommand `{expanded}` failed: {e}")))?;
            let stdin = child.stdin.take().ok_or_else(|| RexError::Ssh("ProxyCommand missing stdin".into()))?;
            let stdout = child.stdout.take().ok_or_else(|| RexError::Ssh("ProxyCommand missing stdout".into()))?;
            let stream = ChildStdioStream { _child: child, stdout, stdin };
            client::connect_stream(config, stream, handler).await.map_err(|e| RexError::Ssh(format!("SSH connect via ProxyCommand failed: {e}")))
        } else {
            client::connect(config, (resolved.hostname.as_str(), resolved.port), handler).await.map_err(|e| RexError::Ssh(format!("SSH connect {}:{} failed: {e}", resolved.hostname, resolved.port)))
        }
    };

    let session = network_timeout(resolved.connect_timeout, &mut waiting, connection).await?;
    network_timeout(Duration::from_secs(120), &mut waiting, authenticate(session, target, &resolved, askpass)).await
}

fn expand_proxy(command: &str, resolved: &ResolvedSshConfig) -> Result<String, RexError> {
    let mut expanded = String::new();
    let mut chars = command.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            expanded.push(c);
            continue;
        }
        let value = match chars.next() {
            Some('%') => {
                expanded.push('%');
                continue;
            }
            Some('p') => {
                expanded.push_str(&resolved.port.to_string());
                continue;
            }
            Some('h') => &resolved.hostname,
            Some('r') => &resolved.user,
            _ => return Err(RexError::Ssh("Unsupported ProxyCommand substitution".into())),
        };
        if value.starts_with('-') || !value.bytes().all(|c| c.is_ascii_alphanumeric() || b".-_@:+%".contains(&c)) {
            return Err(RexError::Ssh("Invalid name for ProxyCommand substitution".into()));
        }
        expanded.push_str(value);
    }
    Ok(expanded)
}

async fn authenticate(mut session: Handle<SshClientHandler>, target: &SshTarget, resolved: &ResolvedSshConfig, askpass: Option<Prompts>) -> Result<Handle<SshClientHandler>, RexError> {
    // 1. Try SSH Agent (`SSH_AUTH_SOCK` / Olo Passwords).
    if let Some(agent_sock) = resolve_agent_socket(target.agent_socket.as_deref())
        && let Ok(mut agent) = AgentClient::connect_uds(&agent_sock).await
        && let Ok(identities) = agent.request_identities().await
    {
        let rsa_hash = session.best_supported_rsa_hash().await.ok().flatten().flatten();
        for ident in identities {
            // (The key the person chose, and no other: each one tried
            // counts against what the server allows.)
            if target.only_key.as_ref().is_some_and(|only| *only != AgentKey::of(&ident).fingerprint) {
                continue;
            }
            let ok = match ident {
                AgentIdentity::PublicKey { key, .. } => {
                    let hash = if key.algorithm().is_rsa() { rsa_hash } else { None };
                    signed(askpass.as_ref(), &resolved.hostname, session.authenticate_publickey_with(&resolved.user, key, hash, &mut agent)).await.map(|r| r.success()).unwrap_or(false)
                }
                AgentIdentity::Certificate { certificate, .. } => {
                    let hash = if certificate.algorithm().is_rsa() { rsa_hash } else { None };
                    signed(askpass.as_ref(), &resolved.hostname, session.authenticate_certificate_with(&resolved.user, certificate, hash, &mut agent)).await.map(|r| r.success()).unwrap_or(false)
                }
            };
            if ok {
                return Ok(session);
            }
        }
    }

    // 2. Try local identity files (with optional askpass passphrase prompt).
    let rsa_hash = session.best_supported_rsa_hash().await.ok().flatten().flatten();
    for key_path in &resolved.identity_files {
        if !key_path.is_file() || target.only_key.is_some() {
            continue;
        }
        let key = match load_secret_key(key_path, None) {
            Ok(k) => Some(k),
            Err(russh::keys::Error::KeyIsEncrypted) => {
                if let Some(ask) = askpass.as_ref() {
                    let prompt = format!("Enter passphrase for SSH key {}:", key_path.display());
                    let pw = ask.secret("SSH Key Passphrase", &prompt, false).await?;
                    load_secret_key(key_path, Some(pw.as_str())).ok()
                } else {
                    None
                }
            }
            Err(e) => {
                tracing::debug!("could not load SSH identity {}: {e}", key_path.display());
                None
            }
        };
        if let Some(k) = key {
            let hash = if k.algorithm().is_rsa() { rsa_hash } else { None };
            let pair = russh::keys::PrivateKeyWithHashAlg::new(Arc::new(k), hash);
            if session.authenticate_publickey(&resolved.user, pair).await.map(|r| r.success()).unwrap_or(false) {
                return Ok(session);
            }
        }
    }

    // 3. Try keyboard-interactive (2FA / PAM) or password via native Olo askpass.
    if let Some(ask) = askpass.as_ref().filter(|_| !target.keys_only) {
        let methods = match session.authenticate_none(&resolved.user).await.map_err(|e| RexError::Ssh(e.to_string()))? {
            AuthResult::Success => return Ok(session),
            AuthResult::Failure { remaining_methods, .. } => remaining_methods,
        };
        if methods.contains(&MethodKind::KeyboardInteractive) {
            'attempts: for _ in 0..3 {
                let mut resp = session.authenticate_keyboard_interactive_start(&resolved.user, None).await.map_err(|e| RexError::Ssh(e.to_string()))?;
                loop {
                    match resp {
                        KeyboardInteractiveAuthResponse::Success => return Ok(session),
                        KeyboardInteractiveAuthResponse::Failure { remaining_methods, .. } => {
                            if !remaining_methods.contains(&MethodKind::KeyboardInteractive) {
                                break 'attempts;
                            }
                            break;
                        }
                        KeyboardInteractiveAuthResponse::InfoRequest { name, instructions, prompts } => {
                            let mut answers = Vec::with_capacity(prompts.len());
                            let title = if name.is_empty() { format!("SSH Authentication ({}@{})", resolved.user, resolved.hostname) } else { name };
                            for p in prompts {
                                let full_prompt = if instructions.is_empty() { p.prompt } else { format!("{instructions}\n{}", p.prompt) };
                                let ans = ask.secret(&title, &full_prompt, p.echo).await?;
                                answers.push(ans.to_string());
                            }
                            resp = session.authenticate_keyboard_interactive_respond(answers).await.map_err(|e| RexError::Ssh(format!("keyboard-interactive failed: {e}")))?;
                        }
                    }
                }
            }
        }
        if methods.contains(&MethodKind::Password) {
            let prompt = format!("Password for {}@{}:", resolved.user, resolved.hostname);
            for _ in 0..3 {
                let pw = ask.secret("SSH Password", &prompt, false).await?;
                match session.authenticate_password(&resolved.user, pw.as_str()).await.map_err(|e| RexError::Ssh(e.to_string()))? {
                    AuthResult::Success => return Ok(session),
                    AuthResult::Failure { remaining_methods, .. } if !remaining_methods.contains(&MethodKind::Password) => break,
                    _ => {}
                }
            }
        }
    }

    Err(RexError::SshRefused(format!("{}@{}:{}", resolved.user, resolved.hostname, resolved.port)))
}

fn cancelled() -> RexError {
    RexError::Ssh("SSH authentication cancelled".into())
}

fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(test)]
mod agent_wait_tests {
    use super::*;

    #[derive(Debug, Default)]
    struct Told(std::sync::Mutex<Vec<(String, bool)>>);
    impl SshAskpass for Told {
        fn confirm_host_key(&self, _: &str, _: u16, _: &str, _: &str, _: bool) -> bool {
            false
        }
        fn prompt_secret(&self, _: &str, _: &str, _: bool) -> Option<Zeroizing<String>> {
            None
        }
        fn waiting_for_agent(&self, host: &str, waiting: bool) {
            self.0.lock().unwrap().push((host.to_owned(), waiting));
        }
    }

    /// An agent that signs at once is never spoken of; one that is slow
    /// is said to be waited for, and then not; and a wait given up (the
    /// connection dropped) is taken back too. The connection's deadline
    /// stands still meanwhile.
    #[tokio::test]
    async fn a_slow_agent_is_said_to_be_waited_for() {
        let told = Arc::new(Told::default());
        let (waiting, paused) = tokio::sync::watch::channel(false);
        let prompts = Prompts { ui: told.clone(), waiting };
        assert_eq!(signed(Some(&prompts), "build.example", async { 7 }).await, 7);
        assert!(told.0.lock().unwrap().is_empty());
        let slow = signed(Some(&prompts), "build.example", async {
            tokio::time::sleep(AGENT_SLOW + Duration::from_millis(300)).await;
            8
        });
        tokio::pin!(slow);
        tokio::select! { _ = &mut slow => panic!("answered before it could have"), _ = tokio::time::sleep(AGENT_SLOW + Duration::from_millis(100)) => {} }
        assert_eq!(*told.0.lock().unwrap(), [("build.example".to_owned(), true)]);
        assert!(*paused.borrow(), "the deadline waits with it");
        assert_eq!(slow.await, 8);
        assert_eq!(told.0.lock().unwrap().last(), Some(&("build.example".to_owned(), false)));
        assert!(!*paused.borrow());
        // Given up while waited for: taken back.
        told.0.lock().unwrap().clear();
        let never = signed(Some(&prompts), "gone.example", std::future::pending::<()>());
        let _ = tokio::time::timeout(AGENT_SLOW + Duration::from_millis(200), never).await;
        assert_eq!(*told.0.lock().unwrap(), [("gone.example".to_owned(), true), ("gone.example".to_owned(), false)]);
    }
}
