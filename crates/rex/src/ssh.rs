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
    /// How long the host's TCP connection took to open: a round trip
    /// of its link, near enough ([`next_window`]). Not known through a
    /// `ProxyCommand`.
    round_trip: Option<Duration>,
    /// What has come from the host on all channels, and since when it
    /// is counted: the link's rate, taken over a stretch
    /// ([`Self::adjust_window`]).
    came: (std::time::Instant, u64),
    /// The window stays what it began as (`OLO_REX_SSH_WINDOW`, for
    /// measuring).
    fixed: bool,
    /// The host's agent relay, where it has one.
    agent: AgentCell,
}

/// A forwarded agent connection (a program on the host asks its
/// `SSH_AUTH_SOCK`): each message, its length and then it, is answered
/// through the relay, until either side has had enough.
async fn relay_agent(stream: impl tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin, relay: AgentRelay) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let (mut from, mut to) = tokio::io::split(stream);
    loop {
        let mut len = [0u8; 4];
        if from.read_exact(&mut len).await.is_err() {
            return;
        }
        let len = u32::from_be_bytes(len) as usize;
        if len == 0 || len > AGENT_MESSAGE_MOST {
            return;
        }
        let mut message = vec![0u8; len];
        if from.read_exact(&mut message).await.is_err() {
            return;
        }
        let relay = relay.clone();
        // (A failure is a message of its own: one byte, 5.)
        let answer = tokio::task::spawn_blocking(move || relay(&message)).await.ok().flatten().unwrap_or_else(|| vec![5]);
        let mut framed = (answer.len() as u32).to_be_bytes().to_vec();
        framed.extend_from_slice(&answer);
        if to.write_all(&framed).await.is_err() || to.flush().await.is_err() {
            return;
        }
    }
}

/// The least and the most a host may send on a channel before it is
/// told to go on, and where a far host's connection begins (a near
/// one's begins at the most, as russh's own does).
const WINDOW_LEAST: u32 = 16 * 1024;
const WINDOW_MOST: u32 = 2 * 1024 * 1024;
const WINDOW_FAR: u32 = 64 * 1024;

/// How much a host may send on a channel before it is told to go on
/// (the channel's window), from the rate at which its link has just
/// been bringing things (`rate`, bytes a second, over a stretch in
/// which a window was used up) and the link's round trip.
///
/// What the host has been let send is on its way whatever happens
/// next: on one connection everything else waits behind it (another
/// terminal, a key's answer, the word that a command is over), for the
/// window's size over the link's rate. Beside a flood on a link of
/// 30,000 bytes a second a small request waited more than 40 s with
/// russh's window of 2 MiB, 1.2 to 1.8 s with 64 kB and 0.1 to 0.5 s
/// with 16 kB (measured, 2026-11-23: HACKING, Rex, "A flood that
/// doesn't hold the link"). And a window too small for the link holds
/// the host back: it sends what it may, and waits to be told to go on
/// (a window is topped up when half of it has come, so one under two
/// round trips' worth of the link runs dry before the top-up is
/// there).
///
/// So the window is what the link brings in a quarter of a second, or
/// in three round trips where that is longer: enough that the host
/// never waits for leave, and no more on its way than that holds
/// anything else up for. No more than four times, nor less than half,
/// what it was, at a step (a rate measured while the host was waiting
/// for leave is the window's own, not the link's: it grows by steps
/// until the link is the bound).
fn next_window(window: u32, rate: f64, round_trip: Duration) -> u32 {
    let held = Duration::from_millis(250).max(round_trip * 3).as_secs_f64();
    let wanted = (rate * held).clamp(f64::from(window) / 2.0, f64::from(window) * 4.0);
    (wanted as u32).clamp(WINDOW_LEAST, WINDOW_MOST)
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

    /// The host's SSH server opens an agent connection: taken only
    /// where this host has been given a relay (a server that opens one
    /// unasked, or after the person took the host's leave back, is
    /// refused), and answered through it.
    async fn server_channel_open_agent_forward(&mut self, channel: russh::Channel<client::Msg>, reply: client::ChannelOpenHandle, _session: &mut client::Session) -> Result<(), Self::Error> {
        let relay = self.agent.lock().unwrap_or_else(|e| e.into_inner()).clone();
        match relay {
            Some(relay) => {
                reply.accept().await;
                tokio::spawn(relay_agent(channel.into_stream(), relay));
            }
            None => reply.reject(russh::ChannelOpenFailure::AdministrativelyProhibited).await,
        }
        Ok(())
    }

    /// A channel's window was topped up (half of it had come): what
    /// the next top-ups go to, for every channel of the connection
    /// ([`next_window`]; the vendored russh takes the answer, where the
    /// published one asked and didn't listen).
    fn adjust_window(&mut self, _channel: russh::ChannelId, window: u32) -> u32 {
        if self.fixed {
            return window;
        }
        let now = std::time::Instant::now();
        let round_trip = self.round_trip.unwrap_or(Duration::from_millis(100));
        // The link's rate, over a stretch and all channels together
        // (not from one top-up to the next: what the link brings is
        // read in pieces of many packets, whose top-ups come
        // microseconds apart whatever the link's rate; taken one by
        // one they sent the window of a link of 300,000 bytes a second
        // to its most, with seven seconds of flood on its way:
        // measured). A stretch in which little came says nothing of
        // the link, and one that began long ago began in quiet.
        let over = now.duration_since(self.came.0);
        if over < Duration::from_millis(200).max(round_trip * 2) {
            return window;
        }
        let came = std::mem::replace(&mut self.came, (now, 0)).1;
        if over > Duration::from_secs(2) {
            return window;
        }
        next_window(window, came as f64 / over.as_secs_f64(), round_trip)
    }

    /// (Counted for the link's rate: [`Self::adjust_window`].)
    async fn data(&mut self, _channel: russh::ChannelId, data: &[u8], _session: &mut client::Session) -> Result<(), Self::Error> {
        self.came.1 += data.len() as u64;
        Ok(())
    }
}

/// Pool of live authenticated SSH sessions keyed by target, allowing `/api/control`
/// and every `/api/data` stream for a remote host to share one SSH connection.
/// Answers one message of the SSH agent protocol for a host's programs
/// (`None`: a failure). It may take as long as a person takes to say
/// yes: it is run off the connection's own thread.
pub type AgentRelay = Arc<dyn Fn(&[u8]) -> Option<Vec<u8>> + Send + Sync>;

/// A host's relay, where it has been given one ([`SshSessionPool::
/// set_agent`]): looked at when the host's SSH server opens an agent
/// connection, so one given or taken back later counts at once.
type AgentCell = Arc<std::sync::Mutex<Option<AgentRelay>>>;

/// The most an agent message may be (OpenSSH's own limit).
const AGENT_MESSAGE_MOST: usize = 256 * 1024;

/// What is run on a host to give its programs the forwarded agent at a
/// path that stays the same from one connection to the next (the SSH
/// server makes a new socket for each): a link to it in the account's
/// own folder, whose path is said back, and then nothing but waiting
/// (the socket is there as long as this is).
const AGENT_KEPT: &str =
    r#"d="$HOME/.local/state/olo-rex"; mkdir -p "$d" && chmod 700 "$d" && [ -n "$SSH_AUTH_SOCK" ] && ln -sfn "$SSH_AUTH_SOCK" "$d/agent.sock" && echo "$d/agent.sock" && exec cat"#;

#[derive(Default)]
pub struct SshSessionPool {
    sessions: Mutex<HashMap<SshTarget, SessionSlot>>,
    /// The hosts' agent relays, and the channels that keep a forwarded
    /// agent's socket there ([`Self::forward_agent`]).
    agents: std::sync::Mutex<HashMap<SshTarget, AgentCell>>,
    agent_channels: std::sync::Mutex<HashMap<SshTarget, russh::Channel<client::Msg>>>,
    /// Olo's helper on the hosts: the name of the build the app
    /// carries ([`Self::set_helper`]).
    helper: std::sync::Mutex<Option<String>>,
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

    /// Olo's helper on the hosts, by the name of the build the app
    /// carries (`helper::file`; letters and digits only, or it is
    /// refused): a control connection goes through that build where
    /// the host has it, through another build of the helper where it
    /// has only that, and straight to the server where it has none
    /// ([`Self::open_rex_channel`], [`dial_command`]). `None`: straight
    /// to the server everywhere.
    pub fn set_helper(&self, id: Option<&str>) {
        let id = id.filter(|id| !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric()));
        *self.helper.lock().unwrap_or_else(|e| e.into_inner()) = id.map(str::to_owned);
    }

    /// Open a bidirectional byte stream to the remote Rex server over SSH:
    /// tries `direct-streamlocal@openssh.com` if an explicit socket path is
    /// configured, otherwise launches `rex __internal dial-stdio` over an SSH
    /// exec channel (which automatically discovers or starts the remote Rex server).
    /// The control connection (`control`) goes through Olo's helper where the
    /// host has it ([`Self::set_helper`]); then always by `dial-stdio`, which
    /// the helper runs.
    fn agent_cell(&self, target: &SshTarget) -> AgentCell {
        self.agents.lock().unwrap_or_else(|e| e.into_inner()).entry(target.clone()).or_default().clone()
    }

    /// A host's programs may use an SSH agent of this computer's through
    /// `relay` (which answers each of their messages), or no longer
    /// (`None`: what the host's server opens from then on is refused,
    /// and the socket kept there goes). Nothing is forwarded until
    /// [`Self::forward_agent`].
    pub fn set_agent(&self, target: &SshTarget, relay: Option<AgentRelay>) {
        if relay.is_none() {
            self.agent_channels.lock().unwrap_or_else(|e| e.into_inner()).remove(target);
        }
        *self.agent_cell(target).lock().unwrap_or_else(|e| e.into_inner()) = relay;
    }

    /// The agent is forwarded to the host over its SSH connection, kept
    /// until the connection ends or the host's relay is taken back: the
    /// path on the host that its programs take as `SSH_AUTH_SOCK` (a
    /// link in the account's own folder to the socket the SSH server
    /// made, the same path at every connection). Asked again after each
    /// connection. An error where the host's SSH server doesn't forward
    /// agents.
    pub async fn forward_agent(&self, target: &SshTarget, askpass: Option<Arc<dyn SshAskpass>>) -> Result<String, RexError> {
        use russh::ChannelMsg;
        if self.agent_cell(target).lock().unwrap_or_else(|e| e.into_inner()).is_none() {
            return Err(RexError::Ssh("no agent has been given for this host".to_owned()));
        }
        let handle = self.get_or_connect(target, askpass).await?;
        let kept = async {
            let mut channel = handle.channel_open_session().await.map_err(|e| RexError::Ssh(format!("open ssh session channel: {e}")))?;
            channel.agent_forward(false).await.map_err(|e| RexError::Ssh(format!("asking {} to forward the agent failed: {e}", target.display())))?;
            channel.exec(true, format!("sh -c {}", shell_escape(AGENT_KEPT))).await.map_err(|e| RexError::Ssh(format!("exec on {} failed: {e}", target.display())))?;
            let mut said = Vec::new();
            loop {
                match channel.wait().await {
                    Some(ChannelMsg::Data { data }) => {
                        said.extend_from_slice(&data);
                        if let Some(end) = said.iter().position(|b| *b == b'\n') {
                            let path = String::from_utf8_lossy(&said[..end]).trim().to_owned();
                            return Ok((path, channel));
                        }
                    }
                    Some(ChannelMsg::ExitStatus { .. } | ChannelMsg::Close | ChannelMsg::Failure) | None => {
                        return Err(RexError::Ssh(format!("{}’s SSH server doesn’t forward agents (AllowAgentForwarding)", target.display())));
                    }
                    _ => {}
                }
            }
        };
        let (path, channel) = tokio::time::timeout(Duration::from_secs(15), kept).await.map_err(|_| RexError::Timeout)??;
        self.agent_channels.lock().unwrap_or_else(|e| e.into_inner()).insert(target.clone(), channel);
        Ok(path)
    }

    /// The host is connected and signed in to (or was already): what
    /// [`Self::open_rex_channel`] begins with, by itself.
    pub async fn reach(&self, target: &SshTarget, askpass: Option<Arc<dyn SshAskpass>>) -> Result<(), RexError> {
        self.get_or_connect(target, askpass).await.map(|_| ())
    }

    pub async fn open_rex_channel(&self, target: &SshTarget, askpass: Option<Arc<dyn SshAskpass>>, control: bool) -> Result<Pin<Box<dyn AsyncReadWrite + Send>>, RexError> {
        let handle = self.get_or_connect(target, askpass).await?;
        let helper = if control { self.helper.lock().unwrap_or_else(|e| e.into_inner()).clone() } else { None };
        tokio::time::timeout(Duration::from_secs(10), Self::channel(&handle, target, helper.as_deref())).await.map_err(|_| RexError::Timeout)?
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

    /// [`Self::exec`] with `input` given to the command on its input,
    /// which then ends: for putting a file on the host (the command
    /// reads it: `cat > …`). What the command writes comes back whole
    /// with its exit status. It is given two minutes and a second for
    /// each 20 kB: a command that stops reading, or a link that stops,
    /// isn't waited for longer.
    pub async fn put(&self, target: &SshTarget, askpass: Option<Arc<dyn SshAskpass>>, command: &str, input: &[u8]) -> Result<crate::provision::Ran, RexError> {
        use russh::ChannelMsg;
        let handle = self.get_or_connect(target, askpass).await?;
        let within = Duration::from_secs(120 + input.len() as u64 / 20_000);
        let sent = async {
            let mut channel = handle.channel_open_session().await.map_err(|e| RexError::Ssh(format!("open ssh session channel: {e}")))?;
            channel.exec(true, command).await.map_err(|e| RexError::Ssh(format!("exec on {} failed: {e}", target.display())))?;
            channel.data(input).await.map_err(|e| RexError::Ssh(format!("sending to {} failed: {e}", target.display())))?;
            channel.eof().await.map_err(|e| RexError::Ssh(format!("sending to {} failed: {e}", target.display())))?;
            let mut ran = crate::provision::Ran::default();
            let mut written = Vec::new();
            loop {
                match channel.wait().await {
                    Some(ChannelMsg::Data { data }) => written.extend_from_slice(&data),
                    Some(ChannelMsg::ExtendedData { data, .. }) => written.extend_from_slice(&data),
                    Some(ChannelMsg::ExitStatus { exit_status }) => ran.status = Some(exit_status),
                    Some(ChannelMsg::Close) | None => break,
                    _ => {}
                }
            }
            ran.output = String::from_utf8_lossy(&written).into_owned();
            Ok(ran)
        };
        tokio::time::timeout(within, sent).await.map_err(|_| RexError::Timeout)?
    }

    async fn channel(handle: &Handle<SshClientHandler>, target: &SshTarget, helper: Option<&str>) -> Result<Pin<Box<dyn AsyncReadWrite + Send>>, RexError> {
        if let Some(sock) = target.socket_path.as_ref().filter(|_| helper.is_none()) {
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
        let cmd = dial_command(target.socket_path.as_deref(), helper);
        // (Nothing can be sent behind this before the server has taken
        // it: OpenSSH's server opens a session's channel with a window
        // of nothing, and gives it one only when the command is set up,
        // so a stream over SSH costs a round trip for its channel, one
        // for this, and then its upgrade. Read off the wire against the
        // rig's far host and timed against the test host, 2026-11-23;
        // not asking for the reply changes nothing, so it is asked for:
        // a server that refuses the command says so.)
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
        let handle = Arc::new(connect_and_authenticate(target, askpass, self.agent_cell(target)).await?);
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
    /// `Compression` in the person's SSH config, where it says.
    compression: Option<bool>,
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
        compression: None,
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
            cfg.compression = params.compression;
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

/// A host whose TCP connection took this long to open is far: what
/// passes between it and here is worth packing ([`compresses`]).
const FAR: Duration = Duration::from_millis(20);

/// Whether an SSH connection asks for compression (`zlib@openssh.com`,
/// begun once signed in): what the person's SSH config says
/// (`Compression`), else `OLO_REX_SSH_COMPRESSION` (yes or no: for
/// measuring), else whether the host is far, by how long its TCP
/// connection took to open (`opened_in`; not known through a
/// `ProxyCommand`, which is taken for far).
///
/// Measured, 2026-11-23 (HACKING, Rex, "Fewer bytes"): packed, a far
/// host's flood is a quarter to a third of its bytes, a program that
/// redraws its box at each key a tenth, and what Rex sends a fifth;
/// on a link with no delay the same flood took a quarter longer (the
/// host's SSH server packs on one core) and cost the app 6% more
/// instructions. So where the link is the bound, and not where it
/// isn't.
fn compresses(said: Option<bool>, knob: Option<bool>, opened_in: Option<Duration>) -> bool {
    said.or(knob).unwrap_or_else(|| opened_in.is_none_or(|took| took >= FAR))
}

async fn connect_and_authenticate(target: &SshTarget, askpass: Option<Arc<dyn SshAskpass>>, agent: AgentCell) -> Result<Handle<SshClientHandler>, RexError> {
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
    // (For measuring: how much a host may send on a channel before it
    // is told to go on, to begin with.)
    let window_knob: Option<u32> = std::env::var("OLO_REX_SSH_WINDOW").ok().and_then(|v| v.parse().ok());
    if let Some(window) = window_knob {
        config.window_size = window;
    }
    let knob = std::env::var("OLO_REX_SSH_COMPRESSION").ok().map(|v| matches!(v.as_str(), "yes" | "1" | "on"));
    // (russh's names: its `ZLIB_LEGACY` is `zlib@openssh.com`, the one
    // OpenSSH's server offers; `ZLIB`, from the first packet, for
    // servers that still have it.)
    let packed = |mut config: client::Config, opened_in: Option<Duration>| {
        // (A far host's channels begin with a small window, which then
        // follows the link: `next_window`. `OLO_REX_SSH_WINDOW` says
        // otherwise, for measuring.)
        if window_knob.is_none() && opened_in.is_none_or(|took| took >= FAR) {
            config.window_size = WINDOW_FAR;
        }
        if compresses(resolved.compression, knob, opened_in) {
            config.preferred.compression = std::borrow::Cow::Borrowed(&[russh::compression::ZLIB_LEGACY, russh::compression::ZLIB, russh::compression::NONE]);
        }
        Arc::new(config)
    };

    let (waiting_tx, mut waiting) = tokio::sync::watch::channel(false);
    let askpass = askpass.map(|ui| Prompts { ui, waiting: waiting_tx.clone() });
    let mut handler = SshClientHandler {
        host: resolved.hostname.clone(),
        port: resolved.port,
        askpass: askpass.clone(),
        round_trip: None,
        came: (std::time::Instant::now(), 0),
        fixed: window_knob.is_some(),
        agent,
    };
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
            client::connect_stream(packed(config, None), stream, handler).await.map_err(|e| RexError::Ssh(format!("SSH connect via ProxyCommand failed: {e}")))
        } else {
            // (The connection is opened here, not by russh, to know
            // how long it took: the host's name is looked up first, so
            // that isn't counted.)
            let failed = |e: std::io::Error| RexError::Ssh(format!("SSH connect {}:{} failed: {e}", resolved.hostname, resolved.port));
            let addresses: Vec<std::net::SocketAddr> = tokio::net::lookup_host((resolved.hostname.as_str(), resolved.port)).await.map_err(failed)?.collect();
            let began = std::time::Instant::now();
            let stream = tokio::net::TcpStream::connect(&addresses[..]).await.map_err(failed)?;
            // (`OLO_REX_SSH_OPENED_IN_MS`: taken to have opened in so
            // long, for the rig, whose far host opens at once whatever
            // its link's delay: what follows from a host being far, or
            // from its round trip, is then as on a real one.)
            let opened_in = std::env::var("OLO_REX_SSH_OPENED_IN_MS").ok().and_then(|v| v.parse().ok()).map_or(began.elapsed(), Duration::from_millis);
            handler.round_trip = Some(opened_in);
            stream.set_nodelay(true).map_err(failed)?;
            client::connect_stream(packed(config, Some(opened_in)), stream, handler).await.map_err(|e| RexError::Ssh(format!("SSH connect {}:{} failed: {e}", resolved.hostname, resolved.port)))
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

/// What is run on the host for a connection to its Rex server: the
/// server's own `dial-stdio`, and for a control connection that may go
/// through Olo's helper (`helper`, the name of the build the app
/// carries), the helper in front of it:
/// - the app's own build, where the host has it;
/// - else the newest other build of the helper there (after an upgrade
///   of Olo, until the new one is put there: the helper says what it
///   does by a number, `olo_rex_link::VERSION`, and the app asks no
///   more of it);
/// - else the server itself, as before.
///
/// A helper is only run if the file is the account's own and answers
/// when asked its version (a home that runs nothing, a file for
/// another machine, a file cut short: straight to the server, never a
/// connection that fails). Its file is named for the machine too
/// (`uname -m` there): a home shared by hosts of two kinds holds one
/// for each. The choice is made by `sh`, whatever the account's own
/// shell is (fish and csh have no `if … then … fi`); every shell takes
/// the one quoted word it is given.
fn dial_command(socket: Option<&str>, helper: Option<&str>) -> String {
    let dial = match socket {
        Some(sock) => format!("rex -S unix://{} __internal dial-stdio", shell_escape(sock)),
        None => "rex __internal dial-stdio".to_owned(),
    };
    let Some(id) = helper else { return dial };
    let dir = crate::helper::DIR;
    let script = format!(
        "d=\"$HOME\"/{dir}; m=$(uname -m); \
         run() {{ if [ -f \"$1\" ] && [ -O \"$1\" ] && \"$1\" --version > /dev/null 2>&1; then exec \"$1\" -- {dial}; fi; }}; \
         run \"$d/olo-rex-link-{id}-$m\"; \
         for L in $(ls -t \"$d\"/olo-rex-link-*-\"$m\" 2> /dev/null); do run \"$L\"; done; \
         exec {dial}"
    );
    format!("sh -c {}", shell_escape(&script))
}

#[cfg(test)]
mod dial_tests {
    use super::*;

    /// A channel's window follows the link: what the link brings in a
    /// quarter of a second or three round trips, from whatever it
    /// began at, by steps of no more than four times and no less than
    /// half; never under the least nor over the most.
    #[test]
    fn a_channels_window_follows_the_link() {
        let ms = Duration::from_millis;
        // (What is measured of a link of `rate` with a window: the
        // link's rate, or the window's own where the host waits for
        // leave: a window's worth each round trip, about.)
        let settled = |rate: f64, trip: Duration, from: u32| {
            let (mut window, mut steps) = (from, 0);
            loop {
                let measured = rate.min(f64::from(window) / trip.as_secs_f64());
                let next = next_window(window, measured, trip);
                if next == window || steps > 40 {
                    return (window, steps);
                }
                (window, steps) = (next, steps + 1);
            }
        };
        // 30,000 bytes a second: a quarter of a second of it is under
        // the least; the least, from a far host's beginning in two steps.
        assert_eq!(settled(30_000.0, ms(100), WINDOW_FAR), (WINDOW_LEAST, 2));
        // 300,000 bytes a second, 100 ms there and back: 0.3 s of it.
        assert_eq!(settled(300_000.0, ms(100), WINDOW_FAR).0, 90_000);
        // 10 MB a second: the most, in four steps (each three times the
        // last, the host waiting for leave meanwhile; each a stretch of
        // a fifth of a second of flood).
        assert_eq!(settled(10_000_000.0, ms(100), WINDOW_FAR), (WINDOW_MOST, 4));
        // A slow, far link (600 ms there and back, 20,000 bytes a
        // second): three round trips' worth, 1.8 s of it.
        assert_eq!(settled(20_000.0, ms(600), WINDOW_FAR).0, 36_000);
        // From the most (a near host's beginning, or a link that got
        // worse): down by halves.
        assert_eq!(settled(30_000.0, ms(100), WINDOW_MOST), (WINDOW_LEAST, 7));
        // A window that never runs dry on a link of this rate: twice a
        // round trip's worth and more, wherever it settles.
        for (rate, trip) in [(50_000.0, ms(40)), (2_000_000.0, ms(250)), (700_000.0, ms(10))] {
            let window = f64::from(settled(rate, trip, WINDOW_FAR).0);
            assert!(window >= 2.0 * rate * trip.as_secs_f64() || window == f64::from(WINDOW_MOST), "{window} at {rate} and {trip:?}");
        }
    }

    /// A connection is packed where the person's SSH config says, else
    /// where the knob says, else where the host is far: its connection
    /// took a while to open, or went through a command of the person's.
    #[test]
    fn a_far_host_is_packed_and_a_near_one_is_not_unless_the_person_says() {
        let ms = Duration::from_millis;
        assert!(!compresses(None, None, Some(ms(0))), "this computer");
        assert!(!compresses(None, None, Some(ms(3))), "the same network");
        assert!(compresses(None, None, Some(FAR)));
        assert!(compresses(None, None, Some(ms(180))), "another continent");
        assert!(compresses(None, None, None), "through a ProxyCommand");
        // The person's word, either way, whatever the link.
        assert!(compresses(Some(true), None, Some(ms(0))));
        assert!(!compresses(Some(false), Some(true), Some(ms(180))));
        assert!(!compresses(Some(false), None, None));
        // The knob, where the config says nothing.
        assert!(compresses(None, Some(true), Some(ms(0))));
        assert!(!compresses(None, Some(false), Some(ms(180))));
    }

    /// The command is what a POSIX shell runs as meant: checked by
    /// running it, with stand-ins for `rex` and for the helper.
    #[test]
    fn a_control_connection_goes_through_the_helper_where_it_is_and_straight_where_it_isnt() {
        assert_eq!(dial_command(None, None), "rex __internal dial-stdio");
        assert_eq!(dial_command(Some("/run/it's/rex.sock"), None), "rex -S unix://'/run/it'\\''s/rex.sock' __internal dial-stdio");
        // (On disk: /tmp is memory here.)
        let dir = std::path::PathBuf::from(format!("/var/tmp/olo-rex-dial-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (home, bin) = (dir.join("home"), dir.join("bin"));
        let there = home.join(crate::helper::DIR);
        std::fs::create_dir_all(&there).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        let script = |path: &std::path::Path, body: &str| {
            use std::os::unix::fs::PermissionsExt;
            std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        };
        // (A helper that runs: it answers when asked its version, and
        // otherwise says who it is and what it was given.)
        let helper = |name: &str| format!("if [ \"$1\" = --version ]; then echo olo-rex-link 1; exit 0; fi; echo \"{name}: $*\"");
        script(&bin.join("rex"), "echo \"rex: $*\"");
        let run = |command: &str| {
            let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default());
            let out = std::process::Command::new("sh").arg("-c").arg(command).env("HOME", &home).env("PATH", path).output().unwrap();
            String::from_utf8_lossy(&out.stdout).trim().to_owned()
        };
        let machine = String::from_utf8(std::process::Command::new("uname").arg("-m").output().unwrap().stdout).unwrap().trim().to_owned();
        let file = |id: &str| there.join(format!("olo-rex-link-{id}-{machine}"));
        let aged = |id: &str, days: u64| std::fs::File::options().write(true).open(file(id)).unwrap().set_modified(std::time::SystemTime::now() - Duration::from_secs(days * 86400)).unwrap();
        for socket in [None, Some("/run/it's a/rex.sock")] {
            let wanted = match socket {
                Some(sock) => format!("-S unix://{sock} __internal dial-stdio"),
                None => "__internal dial-stdio".to_owned(),
            };
            let command = dial_command(socket, Some("0a1b2c"));
            for id in ["0a1b2c", "old1", "old2"] {
                let _ = std::fs::remove_file(file(id));
            }
            let _ = std::fs::remove_file(there.join("olo-rex-link-0a1b2c-othermachine"));
            // No helper there: straight to the server.
            assert_eq!(run(&command), format!("rex: {wanted}"));
            // The app's own build: through it, given the server's command.
            script(&file("0a1b2c"), &helper("mine"));
            assert_eq!(run(&command), format!("mine: -- rex {wanted}"));
            // With other builds beside it: still the app's own.
            script(&file("old1"), &helper("old1"));
            aged("0a1b2c", 3);
            assert_eq!(run(&command), format!("mine: -- rex {wanted}"));
            // Only other builds: the newest of them.
            std::fs::remove_file(file("0a1b2c")).unwrap();
            script(&file("old2"), &helper("old2"));
            aged("old1", 9);
            aged("old2", 2);
            assert_eq!(run(&command), format!("old2: -- rex {wanted}"));
            aged("old2", 20);
            assert_eq!(run(&command), format!("old1: -- rex {wanted}"));
            // The app's own build there, but it doesn't run (a file cut
            // short, a home that runs nothing): another that does.
            script(&file("0a1b2c"), "exit 126");
            assert_eq!(run(&command), format!("old1: -- rex {wanted}"));
            std::fs::write(file("0a1b2c"), b"\x7fELF not a program").unwrap();
            assert_eq!(run(&command), format!("old1: -- rex {wanted}"));
            // None that runs: straight to the server, never a failure.
            script(&file("old1"), "exit 1");
            script(&file("old2"), "exit 1");
            assert_eq!(run(&command), format!("rex: {wanted}"));
            // A build for another machine isn't this host's.
            for id in ["0a1b2c", "old1", "old2"] {
                std::fs::remove_file(file(id)).unwrap();
            }
            script(&there.join("olo-rex-link-0a1b2c-othermachine"), &helper("another machine's"));
            assert_eq!(run(&command), format!("rex: {wanted}"));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_a_plain_name_is_taken_for_the_helpers_build() {
        let pool = SshSessionPool::default();
        let set = |id: &str| {
            pool.set_helper(Some(id));
            pool.helper.lock().unwrap().clone()
        };
        assert_eq!(set("0a1b2c3d4e5f6071").as_deref(), Some("0a1b2c3d4e5f6071"));
        assert_eq!(set("../x"), None);
        assert_eq!(set("x; rm -rf ~"), None);
        assert_eq!(set("a'b"), None);
        assert_eq!(set(""), None);
    }
}
