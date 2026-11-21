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
    io::BufReader,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
    time::Duration,
};

use russh::{
    client::{self, Handle, KeyboardInteractiveAuthResponse},
    keys::{
        PublicKeyOrCertificate,
        agent::{AgentIdentity, client::AgentClient},
        known_hosts::{check_known_hosts, learn_known_hosts},
        load_secret_key,
    },
};
use ssh2_config::{ParseRule, SshConfig};
use tokio::sync::Mutex;
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
}

impl SshTarget {
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
        let (host, port) = if let Some(stripped) = host_port.strip_prefix('[') {
            let (h, rest) = stripped.split_once(']').ok_or_else(|| RexError::InvalidEndpoint(format!("unclosed bracket in {raw}")))?;
            let p = rest.strip_prefix(':').map(|ps| ps.parse::<u16>()).transpose().map_err(|_| RexError::InvalidEndpoint(format!("invalid ssh port in {raw}")))?;
            (h.to_owned(), p)
        } else if let Some((h, ps)) = host_port.rsplit_once(':')
            && let Ok(p) = ps.parse::<u16>()
        {
            (h.to_owned(), Some(p))
        } else {
            (host_port.to_owned(), None)
        };
        if host.is_empty() {
            return Err(RexError::InvalidEndpoint(format!("missing ssh host in {raw}")));
        }
        Ok(Self { host, user, port, socket_path, agent_socket: None })
    }

    pub fn display(&self) -> String {
        let mut s = String::from("ssh://");
        if let Some(u) = &self.user {
            s.push_str(u);
            s.push('@');
        }
        s.push_str(&self.host);
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
    /// Ask whether to trust an unknown or changed SSH host key.
    fn confirm_host_key(&self, host: &str, port: u16, fingerprint: &str, key_type: &str, changed: bool) -> bool;

    /// Ask for a passphrase or password (`echo = false` for secrets).
    fn prompt_secret(&self, title: &str, prompt: &str, echo: bool) -> Option<Zeroizing<String>>;
}

/// Live status of an SSH agent socket (Olo Passwords or custom agent).
#[derive(Debug, Clone, Default)]
pub struct AgentStatus {
    pub socket_path: Option<PathBuf>,
    pub reachable: bool,
    pub key_count: usize,
    pub key_comments: Vec<String>,
    pub error: Option<String>,
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
                AgentStatus { socket_path: Some(sock), reachable: true, key_count: ids.len(), key_comments: comments, error: None }
            }
            Err(e) => AgentStatus { socket_path: Some(sock), reachable: true, error: Some(format!("agent list failed: {e}")), ..Default::default() },
        },
        Err(e) => AgentStatus { socket_path: Some(sock), reachable: false, error: Some(format!("connect failed: {e}")), ..Default::default() },
    }
}

struct SshClientHandler {
    host: String,
    port: u16,
    askpass: Option<Arc<dyn SshAskpass>>,
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
                    let ok = ask.confirm_host_key(&self.host, self.port, &fp, &kt, false);
                    if ok {
                        let _ = learn_known_hosts(&self.host, self.port, &pubkey);
                    }
                    Ok(ok)
                } else {
                    tracing::warn!("rejecting unknown SSH host key for {}:{} ({fp})", self.host, self.port);
                    Ok(false)
                }
            }
            Err(russh::keys::Error::KeyChanged { .. }) => {
                let fp = pubkey.fingerprint(russh::keys::HashAlg::Sha256).to_string();
                let kt = pubkey.algorithm().to_string();
                if let Some(ask) = &self.askpass {
                    Ok(ask.confirm_host_key(&self.host, self.port, &fp, &kt, true))
                } else {
                    tracing::error!("SSH host key CHANGED for {}:{} ({fp})", self.host, self.port);
                    Ok(false)
                }
            }
            Err(e) => {
                tracing::warn!("known_hosts check error for {}:{}: {e}", self.host, self.port);
                Ok(false)
            }
        }
    }
}

/// Pool of live authenticated SSH sessions keyed by target, allowing `/api/control`
/// and every `/api/data` stream for a remote host to share one SSH connection.
#[derive(Default)]
pub struct SshSessionPool {
    sessions: Mutex<HashMap<SshTarget, Arc<Mutex<Handle<SshClientHandler>>>>>,
}

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
        let guard = handle.lock().await;

        if let Some(sock) = &target.socket_path {
            match guard.channel_open_direct_streamlocal(sock.clone()).await {
                Ok(ch) => return Ok(Box::pin(ch.into_stream())),
                Err(e) => {
                    tracing::debug!("direct-streamlocal {sock} failed ({e}), falling back to dial-stdio");
                }
            }
        }

        // Standard Rex SSH transport: `rex __internal dial-stdio` (auto-discovers
        // remote Rex Unix socket and autostart if needed).
        let channel = guard.channel_open_session().await.map_err(|e| RexError::Ssh(format!("open ssh session channel: {e}")))?;
        let cmd = if let Some(sock) = &target.socket_path { format!("rex -S unix://{} __internal dial-stdio", shell_escape(sock)) } else { "rex __internal dial-stdio".to_owned() };
        channel.exec(true, cmd).await.map_err(|e| RexError::Ssh(format!("exec `{}` failed: {e}", target.display())))?;
        Ok(Box::pin(channel.into_stream()))
    }

    async fn get_or_connect(&self, target: &SshTarget, askpass: Option<Arc<dyn SshAskpass>>) -> Result<Arc<Mutex<Handle<SshClientHandler>>>, RexError> {
        let existing = self.sessions.lock().await.get(target).cloned();
        if let Some(existing) = existing {
            if !existing.lock().await.is_closed() {
                return Ok(existing);
            }
            self.sessions.lock().await.remove(target);
        }

        let handle = connect_and_authenticate(target, askpass).await?;
        let shared = Arc::new(Mutex::new(handle));
        self.sessions.lock().await.insert(target.clone(), shared.clone());
        Ok(shared)
    }
}

struct ResolvedSshConfig {
    hostname: String,
    port: u16,
    user: String,
    identity_files: Vec<PathBuf>,
    keepalive_secs: u64,
    proxy_command: Option<String>,
}

fn resolve_ssh_config(target: &SshTarget) -> ResolvedSshConfig {
    let default_user = std::env::var("USER").unwrap_or_else(|_| "root".to_owned());
    let mut cfg = ResolvedSshConfig {
        hostname: target.host.clone(),
        port: target.port.unwrap_or(22),
        user: target.user.clone().unwrap_or(default_user),
        identity_files: Vec::new(),
        keepalive_secs: 30,
        proxy_command: None,
    };

    if let Ok(home) = std::env::var("HOME") {
        let ssh_dir = PathBuf::from(&home).join(".ssh");
        let cfg_path = ssh_dir.join("config");
        if let Ok(file) = File::open(&cfg_path) {
            let mut reader = BufReader::new(file);
            if let Ok(parsed) = SshConfig::default().parse(&mut reader, ParseRule::ALLOW_UNKNOWN_FIELDS) {
                let params = parsed.query(&target.host);
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
                    cfg.keepalive_secs = ka.as_secs().max(10);
                }
                if let Some(pc_parts) = params
                    .unsupported_fields
                    .get("proxycommand")
                    .or_else(|| params.ignored_fields.get("proxycommand"))
                    .or_else(|| params.unsupported_fields.get("ProxyCommand"))
                    .or_else(|| params.ignored_fields.get("ProxyCommand"))
                {
                    let pc = pc_parts.join(" ");
                    if !pc.is_empty() && !pc.eq_ignore_ascii_case("none") {
                        cfg.proxy_command = Some(pc);
                    }
                }
            }
        }
        for name in ["id_ed25519", "id_ecdsa", "id_rsa"] {
            let p = ssh_dir.join(name);
            if p.exists() && !cfg.identity_files.contains(&p) {
                cfg.identity_files.push(p);
            }
        }
    }
    cfg
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
    let resolved = resolve_ssh_config(target);
    let mut config = client::Config::default();
    config.keepalive_interval = Some(Duration::from_secs(resolved.keepalive_secs));
    config.keepalive_max = 3;
    let config = Arc::new(config);

    let handler = SshClientHandler { host: resolved.hostname.clone(), port: resolved.port, askpass: askpass.clone() };
    let mut session = if let Some(pc) = &resolved.proxy_command {
        let expanded = pc.replace("%h", &resolved.hostname).replace("%p", &resolved.port.to_string()).replace("%r", &resolved.user).replace("%%", "%");
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
        client::connect_stream(config, stream, handler).await.map_err(|e| RexError::Ssh(format!("SSH connect via ProxyCommand failed: {e}")))?
    } else {
        client::connect(config, (resolved.hostname.as_str(), resolved.port), handler).await.map_err(|e| RexError::Ssh(format!("SSH connect {}:{} failed: {e}", resolved.hostname, resolved.port)))?
    };

    // 1. Try SSH Agent (`SSH_AUTH_SOCK` / Olo Passwords).
    if let Some(agent_sock) = resolve_agent_socket(target.agent_socket.as_deref())
        && let Ok(mut agent) = AgentClient::connect_uds(&agent_sock).await
        && let Ok(identities) = agent.request_identities().await
    {
        let rsa_hash = session.best_supported_rsa_hash().await.ok().flatten().flatten();
        for ident in identities {
            let ok = match ident {
                AgentIdentity::PublicKey { key, .. } => {
                    let hash = if key.algorithm().is_rsa() { rsa_hash } else { None };
                    session.authenticate_publickey_with(&resolved.user, key, hash, &mut agent).await.map(|r| r.success()).unwrap_or(false)
                }
                AgentIdentity::Certificate { certificate, .. } => {
                    let hash = if certificate.algorithm().is_rsa() { rsa_hash } else { None };
                    session.authenticate_certificate_with(&resolved.user, certificate, hash, &mut agent).await.map(|r| r.success()).unwrap_or(false)
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
        if !key_path.is_file() {
            continue;
        }
        let key = match load_secret_key(key_path, None) {
            Ok(k) => Some(k),
            Err(_) => {
                if let Some(ask) = askpass.as_deref() {
                    let prompt = format!("Enter passphrase for SSH key {}:", key_path.display());
                    ask.prompt_secret("SSH Key Passphrase", &prompt, false).and_then(|pw| load_secret_key(key_path, Some(pw.as_str())).ok())
                } else {
                    None
                }
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
    if let Some(ask) = askpass.as_deref() {
        if let Ok(mut resp) = session.authenticate_keyboard_interactive_start(&resolved.user, None).await {
            loop {
                match resp {
                    KeyboardInteractiveAuthResponse::Success => return Ok(session),
                    KeyboardInteractiveAuthResponse::Failure { .. } => break,
                    KeyboardInteractiveAuthResponse::InfoRequest { name, instructions, prompts } => {
                        let mut answers = Vec::with_capacity(prompts.len());
                        let title = if name.is_empty() { format!("SSH Authentication ({}@{})", resolved.user, resolved.hostname) } else { name };
                        for p in prompts {
                            let full_prompt = if instructions.is_empty() { p.prompt } else { format!("{instructions}\n{}", p.prompt) };
                            let Some(ans) = ask.prompt_secret(&title, &full_prompt, p.echo) else {
                                return Err(RexError::Ssh("SSH authentication cancelled".to_owned()));
                            };
                            answers.push(ans.to_string());
                        }
                        resp = session.authenticate_keyboard_interactive_respond(answers).await.map_err(|e| RexError::Ssh(format!("keyboard-interactive failed: {e}")))?;
                    }
                }
            }
        }

        let prompt = format!("Password for {}@{}:", resolved.user, resolved.hostname);
        if let Some(pw) = ask.prompt_secret("SSH Password", &prompt, false) {
            if session.authenticate_password(&resolved.user, pw.as_str()).await.map(|r| r.success()).unwrap_or(false) {
                return Ok(session);
            }
        }
    }

    Err(RexError::Ssh(format!("SSH authentication failed for {}@{}:{}", resolved.user, resolved.hostname, resolved.port)))
}

fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}
