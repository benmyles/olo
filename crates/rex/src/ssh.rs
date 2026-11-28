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
    collections::{HashMap, VecDeque},
    fs::File,
    io::{self, BufReader},
    net::Ipv6Addr,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
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
    io::{AsyncRead, AsyncWrite, ReadBuf},
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
    user: String,
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
    /// The host's key once it was checked, where it went (primary only;
    /// secondary connections get a private cell and never overwrite the
    /// primary target's verified key).
    verified: KeyCell,
    /// When `Some`, secondary connections strictly require this exact OpenSSH
    /// host key (the one verified by the primary connection) instead of
    /// trusting any other known host or prompting `askpass` (Finding 6).
    expected_key: Option<String>,
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

        let openssh_key = pubkey.to_openssh().ok().map(|key| key.split_whitespace().take(2).collect::<Vec<_>>().join(" "));
        if let Some(expected) = &self.expected_key {
            if openssh_key.as_deref() == Some(expected.as_str()) {
                return Ok(true);
            }
            tracing::warn!("rejecting secondary SSH connection to {}:{}: host key does not match primary connection", self.host, self.port);
            return Ok(false);
        }

        let verified = |openssh_key: Option<&str>| {
            if let Some(key) = openssh_key {
                *self.verified.lock().unwrap_or_else(|e| e.into_inner()) = Some(Verified { hostname: self.host.clone(), port: self.port, user: self.user.clone(), key: key.to_owned() });
            }
        };
        match check_known_hosts(&self.host, self.port, &pubkey) {
            Ok(true) => {
                verified(openssh_key.as_deref());
                Ok(true)
            }
            Ok(false) => {
                let fp = pubkey.fingerprint(russh::keys::HashAlg::Sha256).to_string();
                let kt = pubkey.algorithm().to_string();
                if let Some(ask) = &self.askpass {
                    let (host, port) = (self.host.clone(), self.port);
                    let ok = ask.ask(move |ui| ui.confirm_host_key(&host, port, &fp, &kt, false)).await.map_err(io::Error::other)?;
                    if ok {
                        learn_known_hosts(&self.host, self.port, &pubkey)?;
                        verified(openssh_key.as_deref());
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
///
/// And `ssh` there is told of it, whatever shell runs it: a file of
/// Olo's own beside the account's SSH config (`~/.ssh/olo-rex.conf`,
/// read first by one `Include` line put at the config's head, once)
/// names the link as the agent while its socket is there and the shell's
/// own agent offers no keys: none, one that is gone, or a live one with
/// nothing in it. (Live and empty is what most hosts give: the
/// openssh user service starts an `ssh-agent` for every login and puts
/// it in `SSH_AUTH_SOCK`, `/run/user/UID/openssh_agent`. Going by
/// whether the shell had a live agent, Olo's was never used there, and
/// `git clone` in a Rex terminal was "Permission denied (publickey)"
/// with the keys forwarded beside it: Ben, 2026-11-23, "on rex it
/// doesn't work". `ssh-add -l` says no keys by its exit, and
/// `timeout` keeps an agent that never answers from holding up every
/// `ssh`: one that doesn't answer in two seconds counts as none.) A shell that was open before the keys
/// were given has no `SSH_AUTH_SOCK` for it, which is most shells in
/// sessions that last for weeks: there `git clone` was refused,
/// "Permission denied (publickey)", with the agent working beside it
/// (Ben, 2026-11-23, the day it was built). With the keys taken back
/// the socket goes and the file says nothing: `ssh` is as it was.
const AGENT_KEPT: &str = r##"d="$HOME/.local/state/olo-rex"; mkdir -p "$d" && chmod 700 "$d" && [ -n "$SSH_AUTH_SOCK" ] && ln -sfn "$SSH_AUTH_SOCK" "$d/agent.sock" || exit 1
s="$HOME/.ssh"; c="$s/config"
if mkdir -p "$s" 2> /dev/null && chmod 700 "$s" 2> /dev/null; then
  printf '%s\n' "# Olo's Rex: your SSH keys from Olo Passwords on this host, while Rex is connected and has" "# been given them (Preferences > Rex). Used where a shell's own agent offers no keys." 'Match exec "[ -S $HOME/.local/state/olo-rex/agent.sock ] && ! timeout 2 ssh-add -l > /dev/null 2>&1"' '    IdentityAgent ~/.local/state/olo-rex/agent.sock' > "$s/olo-rex.conf" && chmod 600 "$s/olo-rex.conf"
  if ! grep -qs 'olo-rex\.conf' "$c"; then
    { echo 'Include ~/.ssh/olo-rex.conf'; echo; cat "$c" 2> /dev/null || true; } > "$s/.olo-rex.new" && { [ -e "$c" ] || { : > "$c" && chmod 600 "$c"; }; } && cat "$s/.olo-rex.new" > "$c"
    rm -f "$s/.olo-rex.new"
  fi
fi
echo "$d/agent.sock" && exec cat"##;

#[derive(Default)]
pub struct SshSessionPool {
    /// Per-target SSH connection pools: the outer sync lock is only held to
    /// clone or remove an `Arc<TargetPool>`, never across any `.await` so one
    /// stalled host can never block another host's opens or `close` (Finding 8).
    targets: std::sync::Mutex<HashMap<SshTarget, Arc<TargetPool>>>,
    /// Monotonic generation counter for target lifecycles and QUIC bootstrap
    /// tasks (Findings 6, 8, 12).
    next_gen: AtomicU64,
    /// The hosts' agent relays, and the channels that keep a forwarded
    /// agent's socket there ([`Self::forward_agent`]).
    agents: std::sync::Mutex<HashMap<SshTarget, AgentCell>>,
    agent_channels: std::sync::Mutex<HashMap<SshTarget, GuardedChannel>>,
    /// Olo's helper on the hosts: the name of the build the app
    /// carries ([`Self::set_helper`]).
    helper: std::sync::Mutex<Option<String>>,
    /// The helper's own link over UDP to each host ([`crate::quic`]): sync
    /// lock (never held across `.await`) so `set_helper`, `close`, and
    /// `invalidate` immediately drop keepers and abort in-flight bootstraps
    /// (Finding 12).
    links: std::sync::Mutex<HashMap<SshTarget, Link>>,
    /// Where each host's primary SSH connection went (none: through a proxy):
    /// where its link goes too.
    peers: std::sync::Mutex<HashMap<SshTarget, PeerCell>>,
    /// Each host's key as its primary connection checked it ([`Self::verified`]).
    keys: std::sync::Mutex<HashMap<SshTarget, KeyCell>>,
}

/// An authenticated `russh` session paired with the Tokio runtime that drives
/// its connection task (so channel teardown always runs on the live driver's
/// runtime even if a stream's opener runtime has shut down) and its pinned
/// endpoint + verified host key (Findings 6 & 9).
struct PooledHandle {
    handle: Handle<SshClientHandler>,
    runtime: tokio::runtime::Handle,
    pinned: PinnedEndpoint,
    verified: Option<Verified>,
}

impl PooledHandle {
    fn is_closed(&self) -> bool {
        self.handle.is_closed()
    }
}

/// Dedicated SSH connection for Agent Screens fallback streams: tracks active
/// leases so the connection is closed immediately once its last stream drops
/// (zero idle keepalive wakeups — Finding 9) and marks itself unresponsive if
/// an open times out behind an unread stream (Finding 8).
struct ScreenHandle {
    pooled: Arc<PooledHandle>,
    leases: AtomicUsize,
    unresponsive: AtomicBool,
}

/// RAII lease on a [`ScreenHandle`]; acquired and released under
/// `TargetPool::state` so `1 -> 0` teardown is race-free with concurrent opens.
struct ScreenLease {
    target_pool: Arc<TargetPool>,
    handle: Arc<ScreenHandle>,
    released: bool,
}

impl ScreenLease {
    async fn release(mut self) {
        self.released = true;
        let disconnect = {
            let mut st = self.target_pool.state.lock().unwrap_or_else(|e| e.into_inner());
            let prev = self.handle.leases.fetch_sub(1, Ordering::AcqRel);
            if prev == 1 {
                st.screens.retain(|h| !Arc::ptr_eq(h, &self.handle));
                true
            } else {
                false
            }
        };
        if disconnect {
            let _ = self.handle.pooled.handle.disconnect(russh::Disconnect::ByApplication, "", "en").await;
        }
    }
}

impl Drop for ScreenLease {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        self.released = true;
        let disconnect = {
            let mut st = self.target_pool.state.lock().unwrap_or_else(|e| e.into_inner());
            let prev = self.handle.leases.fetch_sub(1, Ordering::AcqRel);
            if prev == 1 {
                st.screens.retain(|h| !Arc::ptr_eq(h, &self.handle));
                true
            } else {
                false
            }
        };
        if disconnect {
            let h = self.handle.clone();
            let rt = h.pooled.runtime.clone();
            rt.spawn(async move {
                let _ = h.pooled.handle.disconnect(russh::Disconnect::ByApplication, "", "en").await;
            });
        }
    }
}

#[derive(Default)]
struct TargetState {
    primary: Option<Arc<PooledHandle>>,
    further: Vec<Arc<PooledHandle>>,
    screens: Vec<Arc<ScreenHandle>>,
    /// Primary generation for which silent secondary screen connect failed
    /// (password-only or 1-connection host), avoiding repeated wasted dials.
    screen_secondary_disabled_gen: Option<u64>,
}

impl TargetState {
    fn prune_closed(&mut self) {
        if self.primary.as_ref().is_some_and(|h| h.is_closed()) {
            self.primary = None;
        }
        self.further.retain(|h| !h.is_closed());
        self.screens.retain(|h| !h.pooled.is_closed() && !h.unresponsive.load(Ordering::Acquire));
    }

    fn live_connection_count(&mut self) -> usize {
        self.prune_closed();
        usize::from(self.primary.is_some()) + self.further.len() + self.screens.len()
    }
}

struct TargetPool {
    primary_connect: Mutex<()>,
    secondary_connect: Mutex<()>,
    generation: u64,
    closed_tx: tokio::sync::watch::Sender<bool>,
    state: std::sync::Mutex<TargetState>,
}

impl TargetPool {
    fn new(generation: u64) -> Self {
        let (closed_tx, _) = tokio::sync::watch::channel(false);
        Self { primary_connect: Mutex::new(()), secondary_connect: Mutex::new(()), generation, closed_tx, state: std::sync::Mutex::new(TargetState::default()) }
    }

    fn is_closed(&self) -> bool {
        *self.closed_tx.borrow()
    }

    async fn until_closed(&self) {
        let mut rx = self.closed_tx.subscribe();
        let _ = rx.wait_for(|&closed| closed).await;
    }
}

/// Line-buffering collector for [`SshSessionPool::exec`] and
/// [`SshSessionPool::exec_bounded`]: separates stdout from stderr so
/// half-written lines never mix, keeps `Ran::stdout` and `Ran::stderr` apart,
/// and applies optional `(max_output, max_line)` caps only when requested
/// (`None` keeps all bytes whole).
struct ExecCollector {
    bounds: Option<(usize, usize)>,
    out: Vec<u8>,
    err: Vec<u8>,
    ran: crate::provision::Ran,
}

impl ExecCollector {
    fn new(bounds: Option<(usize, usize)>) -> Self {
        Self { bounds, out: Vec::new(), err: Vec::new(), ran: crate::provision::Ran::default() }
    }

    fn take(&mut self, bytes: &[u8], is_stdout: bool, said: &mut (dyn FnMut(&str) + Send)) {
        let pending = if is_stdout { &mut self.out } else { &mut self.err };
        for chunk in bytes.split_inclusive(|b| *b == b'\n') {
            if chunk.ends_with(b"\n") {
                let body = &chunk[..chunk.len() - 1];
                if let Some((_, max_line)) = self.bounds {
                    let room = max_line.saturating_sub(pending.len());
                    if body.len() > room {
                        self.ran.truncated = true;
                    }
                    pending.extend_from_slice(&body[..body.len().min(room)]);
                } else {
                    pending.extend_from_slice(body);
                }
                let line = String::from_utf8_lossy(pending).trim_end_matches('\r').to_owned();
                pending.clear();
                said(&line);
                if let Some((max_output, _)) = self.bounds {
                    if self.ran.output.len() + line.len() < max_output {
                        self.ran.output.push_str(&line);
                        self.ran.output.push('\n');
                    } else {
                        self.ran.truncated = true;
                    }
                    let stream = if is_stdout { &mut self.ran.stdout } else { &mut self.ran.stderr };
                    if stream.len() + line.len() < max_output {
                        stream.push_str(&line);
                        stream.push('\n');
                    } else {
                        self.ran.truncated = true;
                    }
                } else {
                    self.ran.output.push_str(&line);
                    self.ran.output.push('\n');
                    let stream = if is_stdout { &mut self.ran.stdout } else { &mut self.ran.stderr };
                    stream.push_str(&line);
                    stream.push('\n');
                }
            } else if let Some((_, max_line)) = self.bounds {
                let room = max_line.saturating_sub(pending.len());
                if chunk.len() > room {
                    self.ran.truncated = true;
                }
                pending.extend_from_slice(&chunk[..chunk.len().min(room)]);
            } else {
                pending.extend_from_slice(chunk);
            }
        }
    }

    fn finish(mut self, said: &mut (dyn FnMut(&str) + Send)) -> crate::provision::Ran {
        if !self.out.is_empty() {
            self.take(b"\n", true, said);
        }
        if !self.err.is_empty() {
            self.take(b"\n", false, said);
        }
        self.ran
    }
}

/// Where a host's SSH connection went, and how long it took to open
/// (its round trip).
type PeerCell = Arc<std::sync::Mutex<Option<(std::net::IpAddr, Duration)>>>;

/// A host as its SSH connection found it: the name and port it went to
/// (its `~/.ssh/config` alias resolved) and the host's key as it was
/// checked (known, or accepted by the person), in OpenSSH's form
/// (`ssh-ed25519 AAAA…`). What another machine is told to trust of it
/// (a move from one host to another pins it; never trusted on first
/// sight there).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verified {
    pub hostname: String,
    pub port: u16,
    pub user: String,
    pub key: String,
}

type KeyCell = Arc<std::sync::Mutex<Option<Verified>>>;

/// Whether the helper's own link is to be used, by its round trip and
/// the SSH connection's: not where the network carries UDP to the host
/// more slowly than TCP (more than a quarter and 10 ms: a letter's
/// echo is one round trip, and the link is for making things quicker).
/// Measured on a real host, 2026-11-23: 225 ms over UDP on four
/// connections of five where TCP's was 167 ms.
fn link_is_as_quick(link: Duration, ssh: Duration) -> bool {
    link <= ssh + ssh / 4 + Duration::from_millis(10)
}

enum Link {
    Up {
        link: Arc<crate::quic::QuicLink>,
        /// SSH keeper channel for `olo-rex-link quic`: dropping sends EOF +
        /// CLOSE on the connection driver's runtime so the host QUIC helper
        /// exits and frees its `MaxSessions` slot (Finding 6).
        #[allow(dead_code)]
        keeper: GuardedChannel,
        /// Helper build ID that launched this link (Finding 12).
        helper_id: String,
        /// Target lifecycle generation this link belongs to (Finding 12).
        target_gen: u64,
        /// Flips to `false` if this helper resets `wire::SCREEN` with code 3
        /// (built without `screens`), routing screen streams to SSH while
        /// keeping terminals on UDP (Finding 12).
        screens_supported: Arc<AtomicBool>,
    },
    /// Being made, behind the streams that go over SSH meanwhile. Dropping
    /// aborts the bootstrap task and wakes any `OLO_REX_QUIC=yes` waiters.
    Starting {
        generation: u64,
        target_gen: u64,
        helper_id: String,
        abort: tokio::task::AbortHandle,
        done: tokio::sync::watch::Sender<bool>,
    },
    Down(std::time::Instant),
}

impl Drop for Link {
    fn drop(&mut self) {
        if let Link::Starting { abort, done, .. } = self {
            abort.abort();
            let _ = done.send(true);
        }
    }
}

/// Ensures an aborted or runtime-shutdown QUIC bootstrap always clears its
/// `Link::Starting` marker instead of stranding the target on SSH forever
/// (Finding 12).
struct LinkStartGuard {
    pool: Arc<SshSessionPool>,
    target: SshTarget,
    link_gen: u64,
    done: tokio::sync::watch::Sender<bool>,
    finished: bool,
}

impl Drop for LinkStartGuard {
    fn drop(&mut self) {
        if !self.finished {
            let mut links = self.pool.links.lock().unwrap_or_else(|e| e.into_inner());
            if matches!(links.get(&self.target), Some(Link::Starting { generation, .. }) if *generation == self.link_gen) {
                links.remove(&self.target);
            }
            let _ = self.done.send(true);
        }
    }
}

/// A link that couldn't be made isn't tried again for so long (UDP
/// that doesn't get through to a host today won't in a minute).
const LINK_AGAIN: Duration = Duration::from_secs(600);

/// Maximum time allowed for opening an SSH session channel and receiving its
/// initial `exec` / `request_pty` reply (Findings 10 & 13).
const SSH_OPEN_TIMEOUT: Duration = Duration::from_secs(30);

/// Maximum bytes accepted while reading `olo-rex-link quic`'s single-line
/// `olo-rex-quic 1 PORT PRINT\n` announcement (Finding 13).
const ANNOUNCE_LINE_MAX: usize = 512;

/// Maximum early `ChannelMsg`s / bytes buffered if a quirky server emits data
/// ahead of `CHANNEL_SUCCESS` for `exec(true, ...)` (Finding 10).
const EXEC_PREFETCH_MSG_LIMIT: usize = 64;
const EXEC_PREFETCH_BYTES_LIMIT: usize = 256 * 1024;

/// The most further connections to one host: with OpenSSH's ten
/// sessions a connection, fifty sessions in all.
const FURTHER_MOST: usize = 4;

/// Unified per-target connection cap across primary + further + dedicated
/// screen SSH connections (1 primary + at most 4 secondary = 5, matching
/// docs/HACKING.md; Finding 9).
const MAX_CONNECTIONS_PER_TARGET: usize = 1 + FURTHER_MOST;

/// Whether a session channel was refused because the connection has
/// all the sessions the host's SSH server allows it (OpenSSH's
/// `MaxSessions`, ten unless its configuration says otherwise: it
/// answers "connect failed" then; others may say the other two).
fn sessions_full(e: &russh::Error) -> bool {
    use russh::ChannelOpenFailure::{AdministrativelyProhibited, ConnectFailed, ResourceShortage};
    matches!(e, russh::Error::ChannelOpenFailure(ConnectFailed | AdministrativelyProhibited | ResourceShortage))
}

enum SessionOpenErr {
    Full,
    Timeout,
    Other(russh::Error),
}

impl std::fmt::Debug for SshSessionPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SshSessionPool").finish_non_exhaustive()
    }
}

impl SshSessionPool {
    fn alloc_gen(&self) -> u64 {
        self.next_gen.fetch_add(1, Ordering::AcqRel) + 1
    }

    fn target_pool(&self, target: &SshTarget) -> Arc<TargetPool> {
        let mut map = self.targets.lock().unwrap_or_else(|e| e.into_inner());
        map.entry(target.clone()).or_insert_with(|| Arc::new(TargetPool::new(self.alloc_gen()))).clone()
    }

    fn target_pool_if_same(&self, target: &SshTarget, target_gen: u64) -> bool {
        self.targets.lock().unwrap_or_else(|e| e.into_inner()).get(target).is_some_and(|tp| tp.generation == target_gen && !tp.is_closed())
    }

    /// Drop any cached SSH session for `target` (e.g. after disconnect/reconnect).
    pub async fn invalidate(&self, target: &SshTarget) {
        if let Some(tp) = self.targets.lock().unwrap_or_else(|e| e.into_inner()).remove(target) {
            let mut st = tp.state.lock().unwrap_or_else(|e| e.into_inner());
            st.primary = None;
            st.further.clear();
            st.screens.clear();
            st.screen_secondary_disabled_gen = None;
        }
        self.keys.lock().unwrap_or_else(|e| e.into_inner()).remove(target);
        // (A link that is up or starting went with its SSH lifecycle; one that
        // couldn't be made is still not tried before its time.)
        let mut links = self.links.lock().unwrap_or_else(|e| e.into_inner());
        if matches!(links.get(target), Some(Link::Up { .. } | Link::Starting { .. })) {
            links.remove(target);
        }
    }

    fn peer_cell(&self, target: &SshTarget) -> PeerCell {
        self.peers.lock().unwrap_or_else(|e| e.into_inner()).entry(target.clone()).or_default().clone()
    }

    fn key_cell(&self, target: &SshTarget) -> KeyCell {
        self.keys.lock().unwrap_or_else(|e| e.into_inner()).entry(target.clone()).or_default().clone()
    }

    /// The host as its connection found it (where it went, its key as it
    /// was checked): `None` before a connection to it was made.
    pub fn verified(&self, target: &SshTarget) -> Option<Verified> {
        self.keys.lock().unwrap_or_else(|e| e.into_inner()).get(target).and_then(|c| c.lock().unwrap_or_else(|e| e.into_inner()).clone())
    }

    /// Whether the host's streams go over the helper's own link (UDP)
    /// now.
    pub async fn linked(&self, target: &SshTarget) -> bool {
        matches!(self.links.lock().unwrap_or_else(|e| e.into_inner()).get(target), Some(Link::Up { link, .. }) if link.alive())
    }

    /// Close the live QUIC connection for `target` without touching its SSH
    /// session, then demote the link (for tests verifying keeper cleanup on
    /// link discard while SSH stays alive, Finding 6).
    #[doc(hidden)]
    pub async fn discard_dead_link_for_test(&self, target: &SshTarget) {
        let link = match self.links.lock().unwrap_or_else(|e| e.into_inner()).get(target) {
            Some(Link::Up { link, .. }) => link.clone(),
            _ => return,
        };
        link.close_now();
        self.demote_link_if_dead(target, &link, "test discard").await;
    }

    /// Count `(session_channels, encrypted_channels)` across all cached SSH
    /// connections for `target` (for tests verifying zero channel leaks).
    /// Returns an error if any cached handle's driver died or is wedged past 2 s.
    #[doc(hidden)]
    pub async fn open_channel_count(&self, target: &SshTarget) -> Result<(usize, usize), RexError> {
        let handles = {
            let tp = self.targets.lock().unwrap_or_else(|e| e.into_inner()).get(target).cloned();
            let mut list = Vec::new();
            if let Some(tp) = tp {
                let st = tp.state.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(h) = &st.primary {
                    list.push(h.clone());
                }
                list.extend(st.further.iter().cloned());
                list.extend(st.screens.iter().map(|s| s.pooled.clone()));
            }
            list
        };
        let (mut sess, mut enc) = (0usize, 0usize);
        for h in handles {
            let (s, e) = tokio::time::timeout(Duration::from_secs(2), h.handle.open_channel_count())
                .await
                .map_err(|_| RexError::Timeout)?
                .map_err(|e| RexError::Ssh(format!("open_channel_count failed: {e}")))?;
            sess += s;
            enc += e;
        }
        Ok((sess, enc))
    }

    /// Count live cached `(primary, further, screens)` SSH connections for
    /// `target` (for tests verifying unified connection budget and immediate
    /// idle screen-connection teardown, Finding 9).
    #[doc(hidden)]
    pub fn open_connection_count(&self, target: &SshTarget) -> (usize, usize, usize) {
        let Some(tp) = self.targets.lock().unwrap_or_else(|e| e.into_inner()).get(target).cloned() else {
            return (0, 0, 0);
        };
        let mut st = tp.state.lock().unwrap_or_else(|e| e.into_inner());
        st.prune_closed();
        (usize::from(st.primary.is_some()), st.further.len(), st.screens.len())
    }

    /// The helper's own link to the host, where it is up. Where it
    /// isn't and may be tried, it is begun now, behind what asked: a
    /// stream never waits for it (made first, a connection took
    /// seven tenths of a second longer on a real host, and longer
    /// still where UDP then turned out the slower way), so a host's
    /// first streams go over SSH and those opened once it is up go
    /// over it. None where the host hasn't the app's helper, where UDP
    /// doesn't get through or is the slower way, or where it is turned
    /// off (`OLO_REX_QUIC=no`). `OLO_REX_QUIC=yes`: waited for, and
    /// used whatever its round trip, for measuring.
    async fn link(self: &Arc<Self>, target: &SshTarget, askpass: Option<Arc<dyn SshAskpass>>) -> Option<Arc<crate::quic::QuicLink>> {
        self.link_entry(target, askpass).await.map(|(link, _)| link)
    }

    async fn link_for_screens(self: &Arc<Self>, target: &SshTarget, askpass: Option<Arc<dyn SshAskpass>>) -> Option<(Arc<crate::quic::QuicLink>, Arc<AtomicBool>)> {
        let (link, screens_supported) = self.link_entry(target, askpass).await?;
        if !screens_supported.load(Ordering::Acquire) {
            return None;
        }
        Some((link, screens_supported))
    }

    async fn link_entry(self: &Arc<Self>, target: &SshTarget, askpass: Option<Arc<dyn SshAskpass>>) -> Option<(Arc<crate::quic::QuicLink>, Arc<AtomicBool>)> {
        let knob = std::env::var("OLO_REX_QUIC").ok();
        if knob.as_deref() == Some("no") {
            return None;
        }
        let helper = self.helper.lock().unwrap_or_else(|e| e.into_inner()).clone()?;
        let tp = self.target_pool(target);
        let target_gen = tp.generation;
        let wait_sync = knob.as_deref() == Some("yes");

        let mut wait_rx: Option<tokio::sync::watch::Receiver<bool>> = None;
        {
            let mut links = self.links.lock().unwrap_or_else(|e| e.into_inner());
            match links.get(target) {
                Some(Link::Up { link, helper_id, screens_supported, target_gen: g, .. }) if link.alive() && *helper_id == helper && *g == target_gen => {
                    return Some((link.clone(), screens_supported.clone()));
                }
                Some(Link::Down(until)) if std::time::Instant::now() < *until => return None,
                Some(Link::Starting { helper_id, target_gen: g, done, .. }) if *helper_id == helper && *g == target_gen => {
                    if wait_sync {
                        wait_rx = Some(done.subscribe());
                    } else {
                        return None;
                    }
                }
                _ => {
                    // Evict stale / dead / old-helper link immediately so its
                    // `GuardedChannel` keeper sends SSH EOF + CLOSE right away
                    // or its stale `Starting` task aborts.
                    links.remove(target);
                }
            }
            if wait_rx.is_none() {
                let link_gen = self.alloc_gen();
                let (done_tx, done_rx) = tokio::sync::watch::channel(false);
                if wait_sync {
                    wait_rx = Some(done_rx);
                }
                let runtime = tp.state.lock().unwrap_or_else(|e| e.into_inner()).primary.as_ref().filter(|h| !h.is_closed()).map(|h| h.runtime.clone()).unwrap_or_else(tokio::runtime::Handle::current);
                let (pool, target_c, helper_c, tp_c, done_c) = (self.clone(), target.clone(), helper.clone(), tp.clone(), done_tx.clone());
                let guard = LinkStartGuard { pool: pool.clone(), target: target_c.clone(), link_gen, done: done_c.clone(), finished: false };
                let join = runtime.spawn(async move {
                    let mut guard = guard;
                    let made = tokio::select! {
                        biased;
                        _ = tp_c.until_closed() => Err("SSH target closed during QUIC bootstrap".to_owned()),
                        res = pool.start_link(&target_c, askpass, &helper_c, target_gen) => res,
                    };
                    // Acquire `helper` -> `links` in canonical order, and only
                    // publish if helper selection, target lifecycle, and start
                    // generation ALL still match (Finding 12).
                    let helper_cur = pool.helper.lock().unwrap_or_else(|e| e.into_inner());
                    let mut links = pool.links.lock().unwrap_or_else(|e| e.into_inner());
                    let still_valid = helper_cur.as_deref() == Some(helper_c.as_str())
                        && pool.target_pool_if_same(&target_c, target_gen)
                        && matches!(links.get(&target_c), Some(Link::Starting { generation, .. }) if *generation == link_gen);
                    if still_valid {
                        pool.made(&mut links, &target_c, &helper_c, target_gen, made);
                    }
                    guard.finished = true;
                    drop(links);
                    drop(helper_cur);
                    let _ = done_c.send(true);
                });
                links.insert(target.clone(), Link::Starting { generation: link_gen, target_gen, helper_id: helper.clone(), abort: join.abort_handle(), done: done_tx });
            }
        }

        if let Some(mut rx) = wait_rx {
            let _ = rx.wait_for(|&done| done).await;
            let links = self.links.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(Link::Up { link, helper_id, screens_supported, target_gen: g, .. }) = links.get(target)
                && link.alive()
                && *helper_id == helper
                && *g == target_gen
            {
                return Some((link.clone(), screens_supported.clone()));
            }
        }
        None
    }

    fn made(&self, links: &mut HashMap<SshTarget, Link>, target: &SshTarget, helper_id: &str, target_gen: u64, made: Result<(crate::quic::QuicLink, GuardedChannel), String>) {
        match made {
            Ok((link, keeper)) => {
                tracing::info!("ssh: {}'s streams go over the helper's own link (UDP) from now", target.display());
                let link = Arc::new(link);
                let screens_supported = Arc::new(AtomicBool::new(true));
                links.insert(target.clone(), Link::Up { link, keeper, helper_id: helper_id.to_owned(), target_gen, screens_supported });
            }
            Err(why) => {
                tracing::info!("ssh: no link over UDP to {} ({why}); its streams go over SSH", target.display());
                links.insert(target.clone(), Link::Down(std::time::Instant::now() + LINK_AGAIN));
            }
        }
    }

    /// Demote a failed QUIC link to `Link::Down` ONLY when the connection
    /// itself is actually dead (`!failed.alive()`) AND it is still the cached
    /// `Link::Up` instance (Findings 5 & 6). Replacing `Link::Up` drops its
    /// `GuardedChannel` keeper, sending SSH `EOF` + `CLOSE` to end the host
    /// helper immediately.
    async fn demote_link_if_dead(&self, target: &SshTarget, failed: &Arc<crate::quic::QuicLink>, why: &str) {
        if failed.alive() {
            tracing::info!("ssh: {}'s QUIC stream failed while link stays up ({why}); using SSH for this stream", target.display());
            return;
        }
        tracing::info!("ssh: {}'s link over UDP failed ({why}); its streams go over SSH", target.display());
        let mut links = self.links.lock().unwrap_or_else(|e| e.into_inner());
        if matches!(links.get(target), Some(Link::Up { link, .. }) if Arc::ptr_eq(link, failed)) {
            links.insert(target.clone(), Link::Down(std::time::Instant::now() + LINK_AGAIN));
        }
    }

    /// The helper started in its `quic` mode over SSH, given a token,
    /// and the link made to where it says it listens: the link, and
    /// the guarded SSH channel that keeps the helper there.
    async fn start_link(&self, target: &SshTarget, askpass: Option<Arc<dyn SshAskpass>>, helper: &str, target_gen: u64) -> Result<(crate::quic::QuicLink, GuardedChannel), String> {
        // (Made first: the host's address is known once it is reached.)
        let primary = self.get_or_connect(target, askpass.clone()).await.map_err(|e| e.to_string())?;
        if primary.pinned.generation != target_gen {
            return Err("SSH target was closed or replaced during QUIC bootstrap".to_owned());
        }
        let (peer, ssh_round_trip) = (*self.peer_cell(target).lock().unwrap_or_else(|e| e.into_inner())).ok_or("the host is reached through a proxy")?;
        let token = crate::quic::new_token()?;
        let mut channel = self.session(target, askpass).await.map_err(|e| e.to_string())?;
        channel.exec_and_confirm(quic_command(target.socket_path.as_deref(), helper), SSH_OPEN_TIMEOUT, "the host hasn't the helper with its link").await.map_err(|e| e.to_string())?;
        tokio::time::timeout(SSH_OPEN_TIMEOUT, channel.data(format!("{}\n", crate::quic::hex(&token)).as_bytes()))
            .await
            .map_err(|_| "timed out sending QUIC token over SSH".to_owned())?
            .map_err(|e| e.to_string())?;
        let (port, print) = tokio::time::timeout(Duration::from_secs(5), read_announce_line(&mut channel)).await.map_err(|_| "the helper didn't say where it listens")??;
        // (`OLO_REX_QUIC_DIAL=IP:PORT`: reached there instead, for the
        // rig, whose slow link for UDP stands in front of the helper.)
        let to = std::env::var("OLO_REX_QUIC_DIAL").ok().and_then(|v| v.parse().ok()).unwrap_or_else(|| std::net::SocketAddr::new(peer, port));
        let link = crate::quic::QuicLink::connect(to, &print, token).await?;
        // (`OLO_REX_QUIC=yes`: used whatever its round trip, for measuring.)
        let (round_trip, forced) = (link.round_trip(), std::env::var("OLO_REX_QUIC").is_ok_and(|v| v == "yes"));
        tracing::info!("ssh: {}'s round trip is {} ms over UDP, {} ms over SSH's connection", target.display(), round_trip.as_millis(), ssh_round_trip.as_millis());
        if !forced && !link_is_as_quick(round_trip, ssh_round_trip) {
            return Err(format!("UDP's round trip there is {} ms, SSH's {} ms", round_trip.as_millis(), ssh_round_trip.as_millis()));
        }
        Ok((link, channel))
    }

    /// Drops `stale` from the primary slot of `tp` if it is still cached there.
    fn drop_stale_primary(tp: &TargetPool, stale: &Arc<PooledHandle>) {
        let mut st = tp.state.lock().unwrap_or_else(|e| e.into_inner());
        if st.primary.as_ref().is_some_and(|h| Arc::ptr_eq(h, stale)) {
            st.primary = None;
        }
    }

    fn drop_stale_further(tp: &TargetPool, stale: &Arc<PooledHandle>) {
        let mut st = tp.state.lock().unwrap_or_else(|e| e.into_inner());
        st.further.retain(|h| !Arc::ptr_eq(h, stale));
    }

    /// Open a session channel on `pooled` directly (no detached wrapper task!)
    /// bounded by `within` and cancelled immediately if `tp` closes (Findings 8,
    /// 10, 15): russh's own `UnclaimedOpenGuard` + `encrypted.rs` send-error
    /// close ensure cancellation or timeout at any point leaves zero detached
    /// tasks and zero leaked channels.
    async fn open_session_guarded(tp: &TargetPool, pooled: &Arc<PooledHandle>, within: Duration) -> Result<GuardedChannel, SessionOpenErr> {
        let open = tokio::select! {
            biased;
            _ = tp.until_closed() => return Err(SessionOpenErr::Other(russh::Error::Disconnect)),
            res = tokio::time::timeout(within, pooled.handle.channel_open_session()) => res,
        };
        match open {
            Ok(Ok(ch)) => Ok(GuardedChannel::new(ch, pooled.runtime.clone())),
            Ok(Err(e)) if sessions_full(&e) => Err(SessionOpenErr::Full),
            Ok(Err(e)) => Err(SessionOpenErr::Other(e)),
            Err(_) => Err(SessionOpenErr::Timeout),
        }
    }

    /// A session channel on the host (what a command there is run on).
    /// Everything Rex has on a host goes over one SSH connection, each
    /// stream to its server a session of it, and an SSH server opens
    /// only so many sessions on a connection (OpenSSH ten, its
    /// `MaxSessions`). Where the connection is full, the session is opened on
    /// a further connection pinned to the primary's endpoint and host key
    /// (never re-resolving `~/.ssh/config`, never holding any global lock
    /// across `.await`, and sharing the unified [`MAX_CONNECTIONS_PER_TARGET`]
    /// budget; Findings 6, 8, 9, 10).
    async fn session(&self, target: &SshTarget, askpass: Option<Arc<dyn SshAskpass>>) -> Result<GuardedChannel, RexError> {
        self.session_with_verified(target, askpass).await.map(|(ch, _)| ch)
    }

    /// A session channel on the host paired with the exact authenticated
    /// [`Verified`] identity of the live SSH handle that opened that channel.
    async fn session_with_verified(&self, target: &SshTarget, askpass: Option<Arc<dyn SshAskpass>>) -> Result<(GuardedChannel, Option<Verified>), RexError> {
        let failed = |e: SessionOpenErr| match e {
            SessionOpenErr::Timeout => RexError::Timeout,
            SessionOpenErr::Full => RexError::Ssh("open ssh session channel: MaxSessions reached".into()),
            SessionOpenErr::Other(e) => RexError::Ssh(format!("open ssh session channel: {e}")),
        };
        let tp = self.target_pool(target);
        let primary = self.get_or_connect(target, askpass.clone()).await?;
        match Self::open_session_guarded(&tp, &primary, SSH_OPEN_TIMEOUT).await {
            Ok(channel) => return Ok((channel, primary.verified.clone())),
            Err(SessionOpenErr::Full) => {}
            Err(SessionOpenErr::Timeout) => {
                // Retire the nonresponding handle from the pool so retries do
                // not pile abandoned OPENs onto a wedged connection (Finding 10).
                Self::drop_stale_primary(&tp, &primary);
                return Err(RexError::Timeout);
            }
            Err(SessionOpenErr::Other(_)) => {
                Self::drop_stale_primary(&tp, &primary);
                let fresh = self.get_or_connect(target, askpass.clone()).await?;
                match Self::open_session_guarded(&tp, &fresh, SSH_OPEN_TIMEOUT).await {
                    Ok(channel) => return Ok((channel, fresh.verified.clone())),
                    Err(SessionOpenErr::Full) => {}
                    Err(SessionOpenErr::Timeout) => {
                        Self::drop_stale_primary(&tp, &fresh);
                        return Err(RexError::Timeout);
                    }
                    Err(e) => return Err(failed(e)),
                }
            }
        }

        // Try existing `further` handles WITHOUT holding any lock across awaits.
        let snapshot: Vec<Arc<PooledHandle>> = {
            let mut st = tp.state.lock().unwrap_or_else(|e| e.into_inner());
            st.prune_closed();
            st.further.clone()
        };
        for handle in &snapshot {
            match Self::open_session_guarded(&tp, handle, SSH_OPEN_TIMEOUT).await {
                Ok(channel) => return Ok((channel, handle.verified.clone())),
                Err(SessionOpenErr::Full) => {}
                Err(SessionOpenErr::Timeout) => {
                    Self::drop_stale_further(&tp, handle);
                }
                Err(SessionOpenErr::Other(_)) => {
                    Self::drop_stale_further(&tp, handle);
                }
            }
        }

        // Single-flight secondary connection creation per target only.
        let _sec = tp.secondary_connect.lock().await;
        // Re-check any handle another task may have added while we waited.
        let fresh_further: Vec<Arc<PooledHandle>> = {
            let mut st = tp.state.lock().unwrap_or_else(|e| e.into_inner());
            st.prune_closed();
            st.further.iter().filter(|h| !snapshot.iter().any(|old| Arc::ptr_eq(old, h))).cloned().collect()
        };
        for handle in &fresh_further {
            match Self::open_session_guarded(&tp, handle, SSH_OPEN_TIMEOUT).await {
                Ok(channel) => return Ok((channel, handle.verified.clone())),
                Err(SessionOpenErr::Full) => {}
                Err(SessionOpenErr::Timeout) => {
                    Self::drop_stale_further(&tp, handle);
                }
                Err(SessionOpenErr::Other(_)) => {
                    Self::drop_stale_further(&tp, handle);
                }
            }
        }

        let (total, pinned) = {
            let mut st = tp.state.lock().unwrap_or_else(|e| e.into_inner());
            let total = st.live_connection_count();
            let pinned = st.primary.as_ref().map(|p| p.pinned.clone()).unwrap_or_else(|| primary.pinned.clone());
            (total, pinned)
        };
        if total >= MAX_CONNECTIONS_PER_TARGET {
            return Err(RexError::Ssh(format!(
                "{} will open no more sessions: its SSH server allows only so many on a connection (MaxSessions), and Rex has {} connections to it, all full",
                target.display(),
                total
            )));
        }
        tracing::info!("ssh: {}'s connection has all the sessions its server allows; opening another", target.display());
        let handle = tokio::select! {
            biased;
            _ = tp.until_closed() => return Err(RexError::Ssh("SSH connection closed".into())),
            res = connect_and_authenticate(target, askpass, self.agent_cell(target), ConnectMode::Secondary { pinned, silent: false }) => Arc::new(res?),
        };
        if !self.target_pool_matches_install(&tp, target, |st| st.further.push(handle.clone())) {
            let _ = handle.handle.disconnect(russh::Disconnect::ByApplication, "", "en").await;
            return Err(RexError::Ssh("SSH connection closed".into()));
        }
        drop(_sec);
        let channel = Self::open_session_guarded(&tp, &handle, SSH_OPEN_TIMEOUT).await.map_err(failed)?;
        Ok((channel, handle.verified.clone()))
    }

    /// Install a newly opened handle under `tp.state` ONLY if `tp` is still
    /// live and the active `TargetPool` for `target`; otherwise return `false`.
    fn target_pool_matches_install(&self, tp: &TargetPool, target: &SshTarget, install: impl FnOnce(&mut TargetState)) -> bool {
        if !self.target_pool_if_same(target, tp.generation) {
            return false;
        }
        let mut st = tp.state.lock().unwrap_or_else(|e| e.into_inner());
        if tp.is_closed() {
            return false;
        }
        install(&mut st);
        true
    }

    /// A session channel for Agent Screens SSH fallback: tries a dedicated
    /// SSH connection ONLY where it costs the person nothing (silent key/agent
    /// auth with NO askpass, pinned to the primary connection's exact address
    /// and host key, bounded by the unified [`MAX_CONNECTIONS_PER_TARGET`]
    /// budget, and closed immediately when its last stream ends — Findings 6,
    /// 7, 8, 9). Wherever a secondary connection is refused, needs interactive
    /// auth, or exceeds the budget, falls back transparently to a channel on
    /// the primary connection via [`Self::session`].
    async fn screen_session(&self, target: &SshTarget, askpass: Option<Arc<dyn SshAskpass>>) -> Result<GuardedChannel, RexError> {
        let tp = self.target_pool(target);
        let primary = self.get_or_connect(target, askpass.clone()).await?;
        let primary_gen = primary.pinned.generation;

        // If silent secondary connect already failed for this primary generation
        // (e.g. password-only or 1-connection host), go straight to primary.
        if tp.state.lock().unwrap_or_else(|e| e.into_inner()).screen_secondary_disabled_gen == Some(primary_gen) {
            return self.session(target, askpass).await;
        }

        // 1. Try existing dedicated screen connections (fewest active leases
        //    first, zero locks held across `.await`).
        let mut tried: Vec<Arc<ScreenHandle>> = Vec::new();
        loop {
            let next_lease = {
                let mut st = tp.state.lock().unwrap_or_else(|e| e.into_inner());
                st.prune_closed();
                st.screens.iter().filter(|h| !tried.iter().any(|t| Arc::ptr_eq(t, h))).min_by_key(|h| h.leases.load(Ordering::Acquire)).cloned().map(|h| {
                    h.leases.fetch_add(1, Ordering::AcqRel);
                    ScreenLease { target_pool: tp.clone(), handle: h, released: false }
                })
            };
            let Some(lease) = next_lease else { break };
            let sh = lease.handle.clone();
            tried.push(sh.clone());
            match Self::open_session_guarded(&tp, &sh.pooled, SSH_OPEN_TIMEOUT).await {
                Ok(mut channel) => {
                    channel.set_screen_lease(lease);
                    return Ok(channel);
                }
                Err(SessionOpenErr::Full) => {
                    drop(lease);
                }
                Err(SessionOpenErr::Timeout) => {
                    // Mark wedged screen driver unresponsive and remove from
                    // active pool so other opens proceed without waiting on it.
                    sh.unresponsive.store(true, Ordering::Release);
                    tp.state.lock().unwrap_or_else(|e| e.into_inner()).screens.retain(|h| !Arc::ptr_eq(h, &sh));
                    drop(lease);
                }
                Err(SessionOpenErr::Other(_)) => {
                    tp.state.lock().unwrap_or_else(|e| e.into_inner()).screens.retain(|h| !Arc::ptr_eq(h, &sh));
                    drop(lease);
                }
            }
        }

        // 2. Single-flight silent secondary screen connect per target.
        let _sec = tp.secondary_connect.lock().await;
        // Re-check handles added while waiting for `_sec`, and check budget / disabled flag.
        enum ScreenRetry {
            Disabled,
            Lease(ScreenLease),
            Connect,
        }
        let retry = {
            let mut st = tp.state.lock().unwrap_or_else(|e| e.into_inner());
            st.prune_closed();
            if st.screen_secondary_disabled_gen == Some(primary_gen) {
                ScreenRetry::Disabled
            } else if let Some(h) = st.screens.iter().filter(|h| !tried.iter().any(|t| Arc::ptr_eq(t, h))).min_by_key(|h| h.leases.load(Ordering::Acquire)).cloned() {
                h.leases.fetch_add(1, Ordering::AcqRel);
                ScreenRetry::Lease(ScreenLease { target_pool: tp.clone(), handle: h, released: false })
            } else {
                ScreenRetry::Connect
            }
        };
        match retry {
            ScreenRetry::Disabled => {
                drop(_sec);
                return self.session(target, askpass).await;
            }
            ScreenRetry::Lease(lease) => {
                let sh = lease.handle.clone();
                drop(_sec);
                if let Ok(mut channel) = Self::open_session_guarded(&tp, &sh.pooled, SSH_OPEN_TIMEOUT).await {
                    channel.set_screen_lease(lease);
                    return Ok(channel);
                }
                drop(lease);
                return self.session(target, askpass).await;
            }
            ScreenRetry::Connect => {}
        }

        let (total, pinned) = {
            let mut st = tp.state.lock().unwrap_or_else(|e| e.into_inner());
            let total = st.live_connection_count();
            let pinned = st.primary.as_ref().map(|p| p.pinned.clone()).unwrap_or_else(|| primary.pinned.clone());
            (total, pinned)
        };
        if total >= MAX_CONNECTIONS_PER_TARGET {
            drop(_sec);
            return self.session(target, askpass).await;
        }

        // Connect SILENTLY (`askpass = None`, `silent = true`, pinned to primary):
        // never prompts the user and never touches primary's verified key/peer.
        let silent_res = tokio::select! {
            biased;
            _ = tp.until_closed() => return Err(RexError::Ssh("SSH connection closed".into())),
            res = connect_and_authenticate(target, None, self.agent_cell(target), ConnectMode::Secondary { pinned, silent: true }) => res,
        };
        let pooled = match silent_res {
            Ok(h) => Arc::new(h),
            Err(why) => {
                tracing::info!("ssh: silent secondary screen connection to {} unavailable ({why}); using primary SSH connection", target.display());
                {
                    let mut st = tp.state.lock().unwrap_or_else(|e| e.into_inner());
                    if st.screens.is_empty() {
                        st.screen_secondary_disabled_gen = Some(primary_gen);
                    }
                }
                drop(_sec);
                return self.session(target, askpass).await;
            }
        };

        let sh = Arc::new(ScreenHandle { pooled, leases: AtomicUsize::new(1), unresponsive: AtomicBool::new(false) });
        if !self.target_pool_matches_install(&tp, target, |st| st.screens.push(sh.clone())) {
            let _ = sh.pooled.handle.disconnect(russh::Disconnect::ByApplication, "", "en").await;
            return Err(RexError::Ssh("SSH connection closed".into()));
        }
        let lease = ScreenLease { target_pool: tp.clone(), handle: sh.clone(), released: false };
        drop(_sec);

        match Self::open_session_guarded(&tp, &sh.pooled, SSH_OPEN_TIMEOUT).await {
            Ok(mut channel) => {
                channel.set_screen_lease(lease);
                Ok(channel)
            }
            Err(_) => {
                drop(lease);
                self.session(target, askpass).await
            }
        }
    }

    /// Olo's helper on the hosts, by the name of the build the app
    /// carries (`helper::file`; letters and digits only, or it is
    /// refused): a control connection goes through that build where
    /// the host has it, through another build of the helper where it
    /// has only that, and straight to the server where it has none
    /// ([`Self::open_rex_channel`], [`dial_command`]). `None`: straight
    /// to the server everywhere.
    ///
    /// Disabling (`None`) or changing the helper ID immediately drops any
    /// cached QUIC link / keeper and aborts any in-flight QUIC bootstrap for
    /// a different helper ID (Finding 12).
    pub fn set_helper(&self, id: Option<&str>) {
        let new_id = id.filter(|id| !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric())).map(str::to_owned);
        let mut helper_guard = self.helper.lock().unwrap_or_else(|e| e.into_inner());
        if *helper_guard == new_id {
            return;
        }
        *helper_guard = new_id.clone();
        let mut links = self.links.lock().unwrap_or_else(|e| e.into_inner());
        match new_id {
            None => {
                links.retain(|_, link| matches!(link, Link::Down(_)));
            }
            Some(ref id) => {
                links.retain(|_, link| match link {
                    Link::Up { helper_id, .. } | Link::Starting { helper_id, .. } => helper_id == id,
                    Link::Down(_) => false,
                });
            }
        }
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
        let kept = async {
            let mut channel = self.session(target, askpass).await?;
            channel.agent_forward(false).await.map_err(|e| RexError::Ssh(format!("asking {} to forward the agent failed: {e}", target.display())))?;
            channel.exec_and_confirm(format!("sh -c {}", shell_escape(AGENT_KEPT)), Duration::from_secs(15), &format!("exec on {} refused", target.display())).await?;
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
                    Some(ChannelMsg::Close) | None => {
                        channel.mark_peer_closed();
                        return Err(RexError::Ssh(format!("{}’s SSH server doesn’t forward agents (AllowAgentForwarding)", target.display())));
                    }
                    Some(ChannelMsg::ExitStatus { .. } | ChannelMsg::Failure) => {
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

    pub async fn open_rex_channel(self: &Arc<Self>, target: &SshTarget, askpass: Option<Arc<dyn SshAskpass>>, control: bool) -> Result<Pin<Box<dyn AsyncReadWrite + Send>>, RexError> {
        // Over the helper's own link where there is one (UDP: `quic`);
        // a link that fails a stream is given up for a while only if it is
        // actually dead, and the stream goes over SSH as every stream did.
        if let Some(link) = self.link(target, askpass.clone()).await {
            match link.open(control).await {
                Ok(stream) => return Ok(stream),
                Err(why) => {
                    self.demote_link_if_dead(target, &link, &why).await;
                }
            }
        }
        let handle = self.get_or_connect(target, askpass.clone()).await?;
        let helper = if control { self.helper.lock().unwrap_or_else(|e| e.into_inner()).clone() } else { None };
        // (Longer where a further connection has to be made for it.)
        tokio::time::timeout(Duration::from_secs(30), self.channel(&handle, target, askpass, helper.as_deref())).await.map_err(|_| RexError::Timeout)?
    }

    /// Open a host Agent Screens byte stream (`follow`, `stream NAME`,
    /// `history-stream NAME`, `history NAME`, legacy `history NAME --wire`,
    /// `export NAME …`, `--protocol`):
    /// - `["--protocol"]` is explicitly routed as a one-shot over SSH.
    /// - Validated legacy `["history", NAME, "--wire"]` is routed over the
    ///   isolated SSH screen path without touching QUIC (Finding 13).
    /// - Valid streaming args are checked and encoded up front; disallowed
    ///   arguments fail immediately with [`RexError::Protocol`], while valid
    ///   selectors exceeding QUIC's 4 KiB JSON cap go straight over SSH
    ///   without demoting the live UDP link (Finding 5).
    /// - Over QUIC, send priority is `-1`; `Unsupported` (reset 3),
    ///   `EmptyStartup` (pre-SCREEN helper or startup crash), and pre-dispatch
    ///   `OpenTimeout` fall back to SSH while keeping healthy UDP alive for
    ///   terminals, whereas post-dispatch `StartupTimeout` returns
    ///   [`RexError::Timeout`] without re-running an already-dispatched host
    ///   job a second time.
    /// - Over SSH, streaming verbs try a silent dedicated screen connection
    ///   pinned to the primary's endpoint and host key (no askpass), falling
    ///   back transparently to the primary connection whenever a second
    ///   connection is unavailable (Findings 6, 7, 8, 9).
    pub async fn open_screen_stream(self: &Arc<Self>, target: &SshTarget, askpass: Option<Arc<dyn SshAskpass>>, args: &[String]) -> Result<Pin<Box<dyn AsyncReadWrite + Send>>, RexError> {
        let helper = self.helper.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if args.len() == 1 && args[0] == "--protocol" {
            let cmd = screen_command(helper.as_deref(), args);
            return self.exec_io_kind(target, askpass, &cmd, ExecKind::Screen, false).await;
        }
        // Validated legacy binary history form (`history NAME --wire`): runs on
        // the isolated SSH screen path without touching QUIC (Finding 13).
        if args.len() == 3 && args[0] == "history" && args[2] == "--wire" && olo_rex_link::screen_wire::validate_quic_screen_args(&["history".to_owned(), args[1].clone()]).is_ok() {
            let cmd = screen_command(helper.as_deref(), args);
            return self.exec_io_kind(target, askpass, &cmd, ExecKind::Screen, true).await;
        }
        if let Err(e) = olo_rex_link::screen_wire::validate_quic_screen_args(args) {
            return Err(RexError::Protocol(e.to_string()));
        }
        let quic_encodable = olo_rex_link::screen_wire::encode_quic_screen_args(args).is_ok();
        if quic_encodable && let Some((link, screens_supported)) = self.link_for_screens(target, askpass.clone()).await {
            use crate::quic::QuicScreenError;
            match link.open_screen_checked(args, crate::quic::STREAM_OPEN_TIMEOUT).await {
                Ok(stream) => return Ok(stream),
                Err(QuicScreenError::Unsupported) => {
                    tracing::info!("ssh: {}'s QUIC helper does not support screens (reset 3); using SSH for screens while keeping UDP up", target.display());
                    screens_supported.store(false, Ordering::Release);
                }
                Err(QuicScreenError::EmptyStartup) => {
                    // Either an older pre-SCREEN helper silently closed with 0-byte
                    // FIN or the child crashed before emitting bytes (with stderr
                    // discarded by QUIC): try validated SSH once so an eligible
                    // helper works or surfaces the exact host stderr/exit status.
                    tracing::info!("ssh: {}'s QUIC screen helper ended before replying; trying validated SSH once", target.display());
                }
                Err(QuicScreenError::Refused(code)) => {
                    return Err(RexError::Protocol(format!("remote screen stream refused (code {code})")));
                }
                Err(QuicScreenError::InvalidArgs(msg)) => {
                    return Err(RexError::Protocol(msg));
                }
                Err(QuicScreenError::OpenTimeout) => {
                    self.demote_link_if_dead(target, &link, "screen stream open timed out before dispatch").await;
                }
                Err(QuicScreenError::StartupTimeout) => {
                    self.demote_link_if_dead(target, &link, "screen helper startup timed out after dispatch").await;
                    return Err(RexError::Timeout);
                }
                Err(QuicScreenError::Transport(why)) => {
                    self.demote_link_if_dead(target, &link, &why).await;
                }
            }
        }
        let cmd = screen_command(helper.as_deref(), args);
        self.exec_io_kind(target, askpass, &cmd, ExecKind::Screen, true).await
    }

    /// Runs `command` on the host, in a shell there, and waits for it:
    /// what it writes (both its outputs, as they come) goes to `said`
    /// line by line and comes back whole with its exit status. For
    /// setting a host up and looking after it ([`crate::provision`]).
    /// Unbounded so whole-record callers (Tracker directory listings,
    /// base64 accounts files) never lose data.
    pub async fn exec(&self, target: &SshTarget, askpass: Option<Arc<dyn SshAskpass>>, command: &str, said: &mut (dyn FnMut(&str) + Send)) -> Result<crate::provision::Ran, RexError> {
        let (ran, _) = self.exec_inner(target, askpass, command, None, None, said).await?;
        Ok(ran)
    }

    /// Runs `command` like [`Self::exec_bounded`], also returning the exact
    /// authenticated [`Verified`] identity of the live SSH handle that ran it.
    pub async fn exec_bounded_verified(
        &self,
        target: &SshTarget,
        askpass: Option<Arc<dyn SshAskpass>>,
        command: &str,
        max_output: usize,
        max_line: usize,
        said: &mut (dyn FnMut(&str) + Send),
    ) -> Result<(crate::provision::Ran, Option<Verified>), RexError> {
        self.exec_inner(target, askpass, command, Some((max_output, max_line)), None, said).await
    }

    /// Runs `command` like [`Self::exec_bounded`], verifying `check` against the
    /// exact authenticated [`Verified`] identity of the acquired channel's live
    /// SSH handle BEFORE sending `exec` on the wire.
    #[allow(clippy::too_many_arguments)]
    pub async fn exec_bounded_expect_verified(
        &self,
        target: &SshTarget,
        askpass: Option<Arc<dyn SshAskpass>>,
        command: &str,
        max_output: usize,
        max_line: usize,
        check: impl FnOnce(Option<&Verified>) -> Result<(), String> + Send,
        said: &mut (dyn FnMut(&str) + Send),
    ) -> Result<crate::provision::Ran, RexError> {
        let mut check_opt = Some(check);
        let mut wrapper = |v: Option<&Verified>| -> Result<(), String> { if let Some(f) = check_opt.take() { f(v) } else { Ok(()) } };
        let (ran, _) = self.exec_inner(target, askpass, command, Some((max_output, max_line)), Some(&mut wrapper), said).await?;
        Ok(ran)
    }

    /// Runs `command` like [`Self::exec`], capping stored output at
    /// `max_output` bytes per stream and each pending line at `max_line`
    /// bytes, setting [`crate::provision::Ran::truncated`] and closing the SSH
    /// channel immediately once any bound is exceeded (and keeping
    /// [`crate::provision::Ran::stderr`] separate from `stdout`).
    pub async fn exec_bounded(
        &self,
        target: &SshTarget,
        askpass: Option<Arc<dyn SshAskpass>>,
        command: &str,
        max_output: usize,
        max_line: usize,
        said: &mut (dyn FnMut(&str) + Send),
    ) -> Result<crate::provision::Ran, RexError> {
        let (ran, _) = self.exec_inner(target, askpass, command, Some((max_output, max_line)), None, said).await?;
        Ok(ran)
    }

    #[allow(clippy::type_complexity)]
    async fn exec_inner(
        &self,
        target: &SshTarget,
        askpass: Option<Arc<dyn SshAskpass>>,
        command: &str,
        bounds: Option<(usize, usize)>,
        verify_handle: Option<&mut (dyn FnMut(Option<&Verified>) -> Result<(), String> + Send)>,
        said: &mut (dyn FnMut(&str) + Send),
    ) -> Result<(crate::provision::Ran, Option<Verified>), RexError> {
        use russh::ChannelMsg;
        let (mut channel, verified) = self.session_with_verified(target, askpass).await?;
        if let Some(check) = verify_handle
            && let Err(msg) = check(verified.as_ref())
        {
            return Err(RexError::Ssh(msg));
        }
        channel.exec_and_confirm(command, SSH_OPEN_TIMEOUT, &format!("exec on {} refused", target.display())).await?;
        let mut collector = ExecCollector::new(bounds);
        let mut closed_cleanly = false;
        loop {
            match channel.wait().await {
                Some(ChannelMsg::Data { data }) => {
                    collector.take(&data, true, said);
                    if bounds.is_some() && collector.ran.truncated {
                        break;
                    }
                }
                Some(ChannelMsg::ExtendedData { data, .. }) => {
                    collector.take(&data, false, said);
                    if bounds.is_some() && collector.ran.truncated {
                        break;
                    }
                }
                Some(ChannelMsg::ExitStatus { exit_status }) => collector.ran.status = Some(exit_status),
                Some(ChannelMsg::ExitSignal { .. }) => {
                    collector.ran.status.get_or_insert(128);
                }
                Some(ChannelMsg::Close) => {
                    channel.mark_peer_closed();
                    closed_cleanly = true;
                    break;
                }
                None => {
                    channel.mark_peer_closed();
                    break;
                }
                _ => {}
            }
        }
        // If we broke early on truncation, `channel` drops here with its guard
        // still armed, immediately sending EOF + CLOSE on the driver runtime.
        let ran = collector.finish(said);
        if !closed_cleanly && !ran.truncated && ran.status.is_none() && ran.output.trim().is_empty() {
            return Err(RexError::Ssh(format!("connection to {} closed unexpectedly", target.display())));
        }
        Ok((ran, verified))
    }

    /// [`Self::exec`] with `input` given to the command on its input,
    /// which then ends: for putting a file on the host (the command
    /// reads it: `cat > …`). What the command writes comes back whole
    /// with its exit status. It is given two minutes and a second for
    /// each 20 kB: a command that stops reading, or a link that stops,
    /// isn't waited for longer.
    pub async fn put(&self, target: &SshTarget, askpass: Option<Arc<dyn SshAskpass>>, command: &str, input: &[u8]) -> Result<crate::provision::Ran, RexError> {
        let within = Duration::from_secs(120 + input.len() as u64 / 20_000);
        let sent = async {
            let channel = self.session(target, askpass).await?;
            let (rx, tx) = channel.into_parts();
            let (chunk_tx, chunk_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(4);
            let feed = async {
                for chunk in input.chunks(64 * 1024) {
                    if chunk_tx.send(chunk.to_vec()).await.is_err() {
                        break;
                    }
                }
                drop(chunk_tx);
            };
            let target_display = target.display();
            // Whole-reply callers (such as carried-settings comparisons) can
            // legitimately return more than `PUT_STREAM_MAX_OUTPUT` (1 MiB).
            let run = put_stream_driver(&target_display, rx, tx, command, chunk_rx, PUT_STREAM_SEND_TIMEOUT, PUT_STREAM_DRAIN_IDLE_TIMEOUT, PUT_STREAM_EXIT_DRAIN_TIMEOUT, None, None);
            let ((), res) = tokio::join!(feed, run);
            res
        };
        tokio::time::timeout(within, sent).await.map_err(|_| RexError::Timeout)?
    }

    /// A command on the host to talk with: what is written goes to its
    /// input, what it writes comes back (a program's own protocol over
    /// its input and output: Codex's server, told of copied sessions).
    /// Let go of, its input ends (`EOF`) and its channel closes.
    ///
    /// **Ending rule for command streams:**
    /// - Any explicit failure (non-zero `ExitStatus` or `ExitSignal`) is
    ///   returned as an `io::Error` after all received stdout bytes have been
    ///   delivered, regardless of arrival order.
    /// - Channel `EOF` certifies that output is complete: `EOF` followed by
    ///   `ExitStatus(0)`, or `EOF` followed by explicit `CLOSE` (with status 0
    ///   or with no status), ends cleanly (`Ok(0)`).
    /// - Ending without `EOF` (lost transport, bare `CLOSE`, or dropped
    ///   receiver) returns `io::ErrorKind::UnexpectedEof` regardless of any
    ///   earlier `ExitStatus(0)`.
    /// - After `EOF`, the reader waits for `ExitStatus`, `ExitSignal`, or
    ///   `CLOSE` before returning so a failure reported after `EOF` is never
    ///   lost. Consequently, a remote command that closes stdout/stderr and
    ///   continues running will keep its reader waiting until it exits or the
    ///   channel closes. All workspace callers were audited and none expects
    ///   stdout EOF from a still-living process:
    ///   `crates/rex/src/sync.rs` (`send_to`, `receive_from`, `fetch_index`,
    ///   `mutate_host`, `receive_paths`, `send_paths`),
    ///   `apps/rex/src/files_sock.rs` (`handle_read_to_cache`),
    ///   `apps/rex/src/hub.rs` (`tell_codex`, `look_at_screen`, remote folder
    ///   watcher), `apps/rex/src/activity_watch.rs` (`tail_command`,
    ///   `run_host_link_within`, `follow_host_link`,
    ///   `resolve_pane_transcript_once`), and `apps/rex/src/screens_sock.rs`
    ///   (legacy `history --wire`, `open_screen_stream`).
    pub async fn exec_io(&self, target: &SshTarget, askpass: Option<Arc<dyn SshAskpass>>, command: &str) -> Result<Pin<Box<dyn AsyncReadWrite + Send>>, RexError> {
        self.exec_io_kind(target, askpass, command, ExecKind::Command, false).await
    }

    async fn exec_io_kind(
        &self,
        target: &SshTarget,
        askpass: Option<Arc<dyn SshAskpass>>,
        command: &str,
        kind: ExecKind,
        isolate_screen: bool,
    ) -> Result<Pin<Box<dyn AsyncReadWrite + Send>>, RexError> {
        let mut channel = if isolate_screen { self.screen_session(target, askpass).await? } else { self.session(target, askpass).await? };
        let refused_msg = match kind {
            ExecKind::Dial => "remote SSH server refused dial-stdio",
            ExecKind::Screen => "remote SSH server refused screen helper",
            ExecKind::Command => "remote SSH server refused exec",
        };
        channel.exec_and_confirm(command, SSH_OPEN_TIMEOUT, refused_msg).await?;
        Ok(channel.into_exec_stream(kind))
    }

    /// Run a one-shot command on the host and stream its raw stdout chunks
    /// (for `olo-rex-link fs read`, where zero stdout bytes is valid when exit
    /// status is 0). Checks the exit status on close, collects stderr for
    /// error messages, and closes the SSH channel immediately if dropped early.
    pub async fn exec_stdout(&self, target: &SshTarget, askpass: Option<Arc<dyn SshAskpass>>, command: &str) -> Result<ExecStdout, RexError> {
        let mut channel = self.session(target, askpass).await?;
        channel.exec_and_confirm(command, SSH_OPEN_TIMEOUT, &format!("exec on {} refused", target.display())).await?;
        let _ = channel.eof().await;
        let (rx, tx) = channel.into_parts();
        Ok(ExecStdout { target: target.display(), rx, tx, exit_status: None, exit_signal: None, stderr: Vec::new(), done: false })
    }

    /// Run a command on the host inside a remote PTY (`xterm-256color`, `cols × rows`):
    /// used for read-only `tmux attach -r -f ignore-size` viewers of nested subagents.
    /// Waits for the server's PTY allocation reply before issuing `exec`, and
    /// waits for the `exec` acknowledgement before returning (Findings 10 & 15).
    /// Dropping the returned stream closes the SSH channel and cleanly detaches tmux.
    pub async fn exec_pty(&self, target: &SshTarget, askpass: Option<Arc<dyn SshAskpass>>, cols: u16, rows: u16, command: &str) -> Result<Pin<Box<dyn AsyncReadWrite + Send>>, RexError> {
        let mut channel = self.session(target, askpass).await?;
        tokio::time::timeout(SSH_OPEN_TIMEOUT, channel.request_pty(true, "xterm-256color", u32::from(cols.max(20)), u32::from(rows.max(5)), 0, 0, &[]))
            .await
            .map_err(|_| RexError::Timeout)?
            .map_err(|e| RexError::Ssh(format!("pty on {} failed: {e}", target.display())))?;
        match channel.wait_request_reply(SSH_OPEN_TIMEOUT).await {
            Ok(true) => {}
            Ok(false) => return Err(RexError::Ssh(format!("pty on {} refused by SSH server", target.display()))),
            Err(e) => return Err(e),
        }
        channel.exec_and_confirm(command, SSH_OPEN_TIMEOUT, "remote SSH server refused exec").await?;
        Ok(channel.into_exec_stream(ExecKind::Command))
    }

    /// [`Self::put`] for input too large to hold at once (a week of an
    /// agent's sessions can be gigabytes): `chunks` given to the command
    /// on its input as they come, its input ending when they end; what
    /// it writes back whole (up to 1 MiB), with its exit status. Let go of
    /// before it ends (Stop), its channel is closed and the command ends on
    /// the host. Drains `channel.wait()` concurrently while writing so an
    /// early remote exit returns the helper's reply immediately instead of
    /// stalling for 60 s once the channel window fills.
    pub async fn put_stream(&self, target: &SshTarget, askpass: Option<Arc<dyn SshAskpass>>, command: &str, chunks: tokio::sync::mpsc::Receiver<Vec<u8>>) -> Result<crate::provision::Ran, RexError> {
        self.put_stream_tracked(target, askpass, command, chunks, None).await
    }

    /// Like [`Self::put_stream`], flipping `eof_sent` to `true` ONLY after
    /// `channel.eof().await` succeeds on the SSH channel (the exact remote
    /// commit boundary for one-shot helpers like `olo-rex-link fs write --len`).
    pub async fn put_stream_tracked(
        &self,
        target: &SshTarget,
        askpass: Option<Arc<dyn SshAskpass>>,
        command: &str,
        chunks: tokio::sync::mpsc::Receiver<Vec<u8>>,
        eof_sent: Option<Arc<std::sync::atomic::AtomicBool>>,
    ) -> Result<crate::provision::Ran, RexError> {
        let channel = self.session(target, askpass).await?;
        let (rx, tx) = channel.into_parts();
        put_stream_driver(&target.display(), rx, tx, command, chunks, PUT_STREAM_SEND_TIMEOUT, PUT_STREAM_DRAIN_IDLE_TIMEOUT, PUT_STREAM_EXIT_DRAIN_TIMEOUT, Some(PUT_STREAM_MAX_OUTPUT), eof_sent)
            .await
    }

    async fn channel(&self, pooled: &PooledHandle, target: &SshTarget, askpass: Option<Arc<dyn SshAskpass>>, helper: Option<&str>) -> Result<Pin<Box<dyn AsyncReadWrite + Send>>, RexError> {
        if let Some(sock) = target.socket_path.as_ref().filter(|_| helper.is_none()) {
            match tokio::time::timeout(SSH_OPEN_TIMEOUT, pooled.handle.channel_open_direct_streamlocal(sock.clone())).await {
                Ok(Ok(ch)) => return Ok(Box::pin(ch.into_stream())),
                Ok(Err(e)) => {
                    tracing::debug!("direct-streamlocal {sock} failed ({e}), falling back to dial-stdio");
                }
                Err(_) => {
                    tracing::debug!("direct-streamlocal {sock} timed out, falling back to dial-stdio");
                }
            }
        }

        // Standard Rex SSH transport: `rex __internal dial-stdio` (auto-discovers
        // remote Rex Unix socket and autostart if needed).
        let mut channel = self.session(target, askpass).await?;
        let cmd = dial_command(target.socket_path.as_deref(), helper);
        // (Nothing can be sent behind this before the server has taken
        // it: OpenSSH's server opens a session's channel with a window
        // of nothing, and gives it one only when the command is set up,
        // so a stream over SSH costs a round trip for its channel, one
        // for this, and then its upgrade. Read off the wire against the
        // rig's far host and timed against the test host, 2026-11-23;
        // not asking for the reply changes nothing, so it is asked for:
        // a server that refuses the command says so.)
        channel.exec_and_confirm(cmd, SSH_OPEN_TIMEOUT, "remote SSH server refused dial-stdio").await?;
        Ok(channel.into_exec_stream(ExecKind::Dial))
    }

    /// Lets go of a host: its link over UDP (or in-flight bootstrap), the
    /// channel that keeps a forwarded agent's socket there, and its SSH
    /// connections (the primary, any further, and any screen connections),
    /// cancelling any pending opens/connects on that host without blocking on
    /// or stalling any other host (Findings 8 & 12).
    pub async fn close(&self, target: &SshTarget) {
        {
            let mut links = self.links.lock().unwrap_or_else(|e| e.into_inner());
            if matches!(links.get(target), Some(Link::Up { .. } | Link::Starting { .. })) {
                links.remove(target);
            }
        }
        self.agent_channels.lock().unwrap_or_else(|e| e.into_inner()).remove(target);
        self.keys.lock().unwrap_or_else(|e| e.into_inner()).remove(target);
        let mut handles = Vec::new();
        if let Some(tp) = self.targets.lock().unwrap_or_else(|e| e.into_inner()).remove(target) {
            let _ = tp.closed_tx.send(true);
            let mut st = tp.state.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(h) = st.primary.take() {
                handles.push(h);
            }
            handles.append(&mut st.further);
            handles.extend(st.screens.drain(..).map(|s| s.pooled.clone()));
        }
        for pooled in handles {
            let _ = pooled.handle.disconnect(russh::Disconnect::ByApplication, "", "en").await;
        }
    }

    async fn get_or_connect(&self, target: &SshTarget, askpass: Option<Arc<dyn SshAskpass>>) -> Result<Arc<PooledHandle>, RexError> {
        let tp = self.target_pool(target);
        if let Some(handle) = tp.state.lock().unwrap_or_else(|e| e.into_inner()).primary.as_ref().filter(|h| !h.is_closed()).cloned() {
            return Ok(handle);
        }
        let _guard = tp.primary_connect.lock().await;
        if let Some(handle) = tp.state.lock().unwrap_or_else(|e| e.into_inner()).primary.as_ref().filter(|h| !h.is_closed()).cloned() {
            return Ok(handle);
        }
        let handle = tokio::select! {
            biased;
            _ = tp.until_closed() => return Err(RexError::Ssh("SSH connection closed".into())),
            res = connect_and_authenticate(
                target,
                askpass,
                self.agent_cell(target),
                ConnectMode::Primary { peer: self.peer_cell(target), verified: self.key_cell(target), generation: tp.generation },
            ) => Arc::new(res?),
        };
        if !self.target_pool_matches_install(&tp, target, |st| {
            st.primary = Some(handle.clone());
            st.screen_secondary_disabled_gen = None;
        }) {
            let _ = handle.handle.disconnect(russh::Disconnect::ByApplication, "", "en").await;
            return Err(RexError::Ssh("SSH connection closed".into()));
        }
        Ok(handle)
    }
}

/// Read `olo-rex-link quic`'s single-line `olo-rex-quic 1 PORT PRINT\n`
/// announcement with a strict [`ANNOUNCE_LINE_MAX`] byte cap, scanning only
/// newly appended bytes for `\n` (Findings 11 & 13).
///
/// Only marks `peer_closed` on explicit `ChannelMsg::Close` or receiver loss
/// (`None`); `Failure` or `ExitStatus` before `Close` leaves `ChannelDropGuard`
/// armed so dropping the channel sends `CHANNEL_CLOSE` and frees the server's
/// `MaxSessions` slot.
async fn read_announce_line(channel: &mut GuardedChannel) -> Result<(u16, String), &'static str> {
    use russh::ChannelMsg;
    let mut said = Vec::new();
    loop {
        match channel.wait().await {
            Some(ChannelMsg::Data { data }) => {
                let prev = said.len();
                let room = ANNOUNCE_LINE_MAX.saturating_sub(prev);
                let take = data.len().min(room);
                said.extend_from_slice(&data[..take]);
                if let Some(rel) = said[prev..].iter().position(|b| *b == b'\n') {
                    let end = prev + rel;
                    return crate::quic::announced(&String::from_utf8_lossy(&said[..end])).ok_or("the helper said something else");
                }
                if said.len() >= ANNOUNCE_LINE_MAX {
                    return Err("the helper's announcement exceeded 512 bytes");
                }
            }
            Some(ChannelMsg::Close) | None => {
                channel.mark_peer_closed();
                return Err("the host hasn't the helper with its link");
            }
            Some(ChannelMsg::ExitStatus { .. } | ChannelMsg::ExitSignal { .. } | ChannelMsg::Failure) => {
                channel.mark_peer_exited();
                return Err("the host hasn't the helper with its link");
            }
            _ => {}
        }
    }
}

/// Shared lifecycle flags between `ExecOutput`, `ExecWriter`, and
/// `ChannelDropGuard` so writes fail immediately on local shutdown or peer
/// exit/close, `EOF` is sent at most once, and a pending writer wakes
/// promptly when the channel closes (Finding 10).
#[derive(Default)]
struct ExecShared {
    shutdown_started: AtomicBool,
    eof_sent: AtomicBool,
    peer_exited: AtomicBool,
    peer_closed: AtomicBool,
    writer_waker: futures_util::task::AtomicWaker,
}

impl ExecShared {
    fn mark_peer_exited(&self) {
        self.peer_exited.store(true, Ordering::Release);
        self.writer_waker.wake();
    }

    fn mark_peer_closed(&self) {
        self.peer_closed.store(true, Ordering::Release);
        self.peer_exited.store(true, Ordering::Release);
        self.writer_waker.wake();
    }
}

/// Drop guard attached to every SSH session channel from confirmation until
/// peer close: dropping from any thread schedules `EOF` (if not already sent)
/// and `CLOSE` on the SSH connection driver's own runtime, then releases any
/// dedicated [`ScreenLease`] so an unused screen connection closes immediately
/// (Findings 6, 8, 9).
struct ChannelDropGuard {
    write_half: Option<russh::ChannelWriteHalf<client::Msg>>,
    shared: Arc<ExecShared>,
    runtime: tokio::runtime::Handle,
    screen_lease: Option<ScreenLease>,
}

impl Drop for ChannelDropGuard {
    fn drop(&mut self) {
        let lease = self.screen_lease.take();
        if let Some(write_half) = self.write_half.take() {
            if self.shared.peer_closed.load(Ordering::Acquire) {
                if let Some(lease) = lease {
                    self.runtime.spawn(async move {
                        lease.release().await;
                    });
                }
                return;
            }
            let send_eof = !self.shared.eof_sent.swap(true, Ordering::AcqRel) && !self.shared.peer_exited.load(Ordering::Acquire);
            self.runtime.spawn(async move {
                if send_eof {
                    let _ = write_half.eof().await;
                }
                let _ = write_half.close().await;
                if let Some(lease) = lease {
                    lease.release().await;
                }
            });
        } else if let Some(lease) = lease {
            drop(lease);
        }
    }
}

/// An SSH session channel wrapped in [`ChannelDropGuard`] from the instant
/// `channel_open_session` confirms, so cancellation or failure during `exec`,
/// `request_pty`, `agent_forward`, or `start_link` never leaks a host session
/// or local russh channel (Findings 6, 8, 9, 10, 11).
struct GuardedChannel {
    read: russh::ChannelReadHalf,
    /// Early channel messages (e.g. `Data` / `ExtendedData` / `Eof` /
    /// `ExitStatus`) received before `ChannelMsg::Success` during
    /// [`Self::exec_and_confirm`], drained in order before polling `read`.
    prefetched: VecDeque<russh::ChannelMsg>,
    guard: ChannelDropGuard,
}

impl GuardedChannel {
    fn new(channel: russh::Channel<client::Msg>, runtime: tokio::runtime::Handle) -> Self {
        let (read, write_half) = channel.split();
        let shared = Arc::new(ExecShared::default());
        Self { read, prefetched: VecDeque::new(), guard: ChannelDropGuard { write_half: Some(write_half), shared, runtime, screen_lease: None } }
    }

    fn set_screen_lease(&mut self, lease: ScreenLease) {
        self.guard.screen_lease = Some(lease);
    }

    fn write_half(&self) -> &russh::ChannelWriteHalf<client::Msg> {
        self.guard.write_half.as_ref().expect("GuardedChannel write_half present")
    }

    async fn exec<A: Into<Vec<u8>>>(&self, want_reply: bool, command: A) -> Result<(), russh::Error> {
        self.write_half().exec(want_reply, command).await
    }

    /// Send `exec(true, command)` and wait (bounded by `within`) for the
    /// server's `ChannelMsg::Success` / `Failure` reply, preserving any early
    /// messages that arrive ahead of `Success` (Finding 10). On `Failure`,
    /// leaves `peer_closed` unset so dropping `GuardedChannel` closes the
    /// channel on the server (Finding 11).
    async fn exec_and_confirm<A: Into<Vec<u8>>>(&mut self, command: A, within: Duration, refused_msg: &str) -> Result<(), RexError> {
        use russh::ChannelMsg;
        let deadline = tokio::time::Instant::now() + within;
        tokio::time::timeout_at(deadline, self.exec(true, command)).await.map_err(|_| RexError::Timeout)?.map_err(|e| RexError::Ssh(format!("exec failed: {e}")))?;
        let mut early_bytes = 0usize;
        let wait_reply = async {
            loop {
                match self.read.wait().await {
                    Some(ChannelMsg::Success) => return Ok(()),
                    Some(ChannelMsg::Failure) => {
                        self.mark_peer_exited();
                        return Err(RexError::Ssh(refused_msg.to_owned()));
                    }
                    Some(ChannelMsg::Close) | None => {
                        self.mark_peer_closed();
                        return Err(RexError::Ssh("SSH channel closed before exec reply".into()));
                    }
                    Some(other) => {
                        if let ChannelMsg::Data { ref data } | ChannelMsg::ExtendedData { ref data, .. } = other {
                            early_bytes = early_bytes.saturating_add(data.len());
                        }
                        if self.prefetched.len() >= EXEC_PREFETCH_MSG_LIMIT || early_bytes > EXEC_PREFETCH_BYTES_LIMIT {
                            return Err(RexError::Ssh("SSH server sent excessive data before exec reply".into()));
                        }
                        self.prefetched.push_back(other);
                    }
                }
            }
        };
        tokio::time::timeout_at(deadline, wait_reply).await.map_err(|_| RexError::Timeout)?
    }

    #[allow(clippy::too_many_arguments)]
    async fn request_pty(&self, want_reply: bool, term: &str, col_width: u32, row_height: u32, pix_width: u32, pix_height: u32, terminal_modes: &[(russh::Pty, u32)]) -> Result<(), russh::Error> {
        self.write_half().request_pty(want_reply, term, col_width, row_height, pix_width, pix_height, terminal_modes).await
    }

    async fn agent_forward(&self, want_reply: bool) -> Result<(), russh::Error> {
        self.write_half().agent_forward(want_reply).await
    }

    async fn data(&self, bytes: &[u8]) -> Result<(), russh::Error> {
        self.write_half().data(bytes).await
    }

    async fn eof(&self) -> Result<(), russh::Error> {
        self.guard.shared.shutdown_started.store(true, Ordering::Release);
        let res = self.write_half().eof().await;
        if res.is_ok() {
            self.guard.shared.eof_sent.store(true, Ordering::Release);
        }
        res
    }

    async fn wait(&mut self) -> Option<russh::ChannelMsg> {
        if let Some(msg) = self.prefetched.pop_front() {
            return Some(msg);
        }
        self.read.wait().await
    }

    fn mark_peer_exited(&self) {
        self.guard.shared.mark_peer_exited();
    }

    fn mark_peer_closed(&self) {
        self.guard.shared.mark_peer_closed();
    }

    /// Wait for the server's `ChannelMsg::Success` / `Failure` reply to a
    /// preceding `want_reply = true` request (e.g. `request_pty`, Finding 15).
    async fn wait_request_reply(&mut self, within: Duration) -> Result<bool, RexError> {
        use russh::ChannelMsg;
        let fut = async {
            loop {
                match self.read.wait().await {
                    Some(ChannelMsg::Success) => return Ok(true),
                    Some(ChannelMsg::Failure) => {
                        self.mark_peer_exited();
                        return Ok(false);
                    }
                    Some(ChannelMsg::Close) | None => {
                        self.mark_peer_closed();
                        return Err(RexError::Ssh("SSH channel closed before request reply".into()));
                    }
                    Some(_) => {}
                }
            }
        };
        tokio::time::timeout(within, fut).await.map_err(|_| RexError::Timeout)?
    }

    fn into_parts(self) -> (GuardedChannelRx, ChannelDropGuard) {
        (GuardedChannelRx { read: self.read, prefetched: self.prefetched }, self.guard)
    }

    fn into_exec_stream(self, kind: ExecKind) -> Pin<Box<dyn AsyncReadWrite + Send>> {
        let shared = self.guard.shared.clone();
        let writer = ExecWriter { inner: self.write_half().make_writer(), shared: shared.clone() };
        let reader = ExecOutput {
            read: Some(self.read),
            prefetched: self.prefetched,
            pending: None,
            kind,
            opened: false,
            eof_seen: false,
            done: false,
            exit_status: None,
            exit_signal: None,
            stderr: Vec::new(),
            shared,
            _guard: self.guard,
        };
        Box::pin(tokio::io::join(reader, writer))
    }
}

struct GuardedChannelRx {
    read: russh::ChannelReadHalf,
    prefetched: VecDeque<russh::ChannelMsg>,
}

const PUT_STREAM_SEND_TIMEOUT: Duration = Duration::from_secs(60);
const PUT_STREAM_DRAIN_IDLE_TIMEOUT: Duration = Duration::from_secs(120);
const PUT_STREAM_EXIT_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);
const PUT_STREAM_MAX_OUTPUT: usize = 1024 * 1024;
const EXEC_STDOUT_MAX_STDERR: usize = 64 * 1024;

trait ChannelRx {
    async fn wait(&mut self) -> Option<russh::ChannelMsg>;
}

trait ChannelTx {
    async fn exec(&self, command: &str) -> Result<(), String>;
    async fn data(&self, chunk: &[u8]) -> Result<(), String>;
    async fn eof(&self) -> Result<(), String>;
    fn disarm_close(&mut self);
}

impl ChannelRx for GuardedChannelRx {
    async fn wait(&mut self) -> Option<russh::ChannelMsg> {
        if let Some(msg) = self.prefetched.pop_front() {
            return Some(msg);
        }
        self.read.wait().await
    }
}

impl ChannelTx for ChannelDropGuard {
    async fn exec(&self, command: &str) -> Result<(), String> {
        let Some(writer) = self.write_half.as_ref() else {
            return Err("channel closed".to_owned());
        };
        writer.exec(true, command).await.map_err(|e| e.to_string())
    }

    async fn data(&self, chunk: &[u8]) -> Result<(), String> {
        let Some(writer) = self.write_half.as_ref() else {
            return Err("channel closed".to_owned());
        };
        writer.data(chunk).await.map_err(|e| e.to_string())
    }

    async fn eof(&self) -> Result<(), String> {
        let Some(writer) = self.write_half.as_ref() else {
            return Err("channel closed".to_owned());
        };
        self.shared.shutdown_started.store(true, Ordering::Release);
        let res = writer.eof().await.map_err(|e| e.to_string());
        if res.is_ok() {
            self.shared.eof_sent.store(true, Ordering::Release);
        }
        res
    }

    fn disarm_close(&mut self) {
        self.shared.mark_peer_closed();
    }
}

enum PutMsgAction {
    Continue,
    Exited,
    ClosedClean,
    TransportLost,
}

fn handle_put_msg(
    target_display: &str,
    msg: Option<russh::ChannelMsg>,
    written: &mut Vec<u8>,
    max_output: Option<usize>,
    ran: &mut crate::provision::Ran,
    refused: &mut Option<String>,
) -> Result<(PutMsgAction, bool), RexError> {
    use russh::ChannelMsg;
    match msg {
        Some(ChannelMsg::Data { data }) | Some(ChannelMsg::ExtendedData { data, .. }) => {
            if let Some(limit) = max_output
                && written.len().saturating_add(data.len()) > limit
            {
                return Err(RexError::Ssh(format!("output from {target_display} exceeded {limit} byte limit")));
            }
            written.extend_from_slice(&data);
            Ok((PutMsgAction::Continue, true))
        }
        Some(ChannelMsg::ExitStatus { exit_status }) => {
            ran.status = Some(exit_status);
            Ok((PutMsgAction::Exited, true))
        }
        Some(ChannelMsg::ExitSignal { signal_name, .. }) => {
            ran.status.get_or_insert(128);
            refused.get_or_insert_with(|| format!("remote command on {target_display} terminated by signal {signal_name:?}"));
            Ok((PutMsgAction::Exited, true))
        }
        Some(ChannelMsg::Failure) => Err(RexError::Ssh(format!("exec on {target_display} was refused"))),
        Some(ChannelMsg::Close) => Ok((PutMsgAction::ClosedClean, true)),
        None => {
            refused.get_or_insert_with(|| format!("connection to {target_display} closed unexpectedly"));
            Ok((PutMsgAction::TransportLost, false))
        }
        Some(ChannelMsg::Success | ChannelMsg::Eof) => Ok((PutMsgAction::Continue, true)),
        Some(_) => Ok((PutMsgAction::Continue, false)),
    }
}

#[allow(clippy::too_many_arguments)]
async fn put_stream_driver<R: ChannelRx, W: ChannelTx>(
    target_display: &str,
    mut rx: R,
    mut tx: W,
    command: &str,
    mut chunks: tokio::sync::mpsc::Receiver<Vec<u8>>,
    send_timeout: Duration,
    drain_idle_timeout: Duration,
    exit_drain_timeout: Duration,
    max_output: Option<usize>,
    eof_sent: Option<Arc<std::sync::atomic::AtomicBool>>,
) -> Result<crate::provision::Ran, RexError> {
    tx.exec(command).await.map_err(|e| RexError::Ssh(format!("exec on {target_display} failed: {e}")))?;

    let mut ran = crate::provision::Ran::default();
    let mut written = Vec::new();
    let mut refused = None;
    let mut remote_closed = false;
    let mut closed_cleanly = false;
    let mut transport_lost = false;
    let mut remote_exited = false;

    'send: loop {
        let chunk = tokio::select! {
            c = chunks.recv() => match c {
                Some(chunk) => chunk,
                None => break 'send,
            },
            msg = rx.wait() => {
                let (action, _) = handle_put_msg(target_display, msg, &mut written, max_output, &mut ran, &mut refused)?;
                match action {
                    PutMsgAction::Continue => continue 'send,
                    PutMsgAction::Exited => {
                        remote_exited = true;
                        break 'send;
                    }
                    PutMsgAction::ClosedClean => {
                        remote_closed = true;
                        closed_cleanly = true;
                        break 'send;
                    }
                    PutMsgAction::TransportLost => {
                        remote_closed = true;
                        transport_lost = true;
                        break 'send;
                    }
                }
            }
        };
        let send = tx.data(&chunk[..]);
        tokio::pin!(send);
        let deadline = tokio::time::sleep(send_timeout);
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                res = &mut send => match res {
                    Ok(()) => break,
                    Err(e) => {
                        refused = Some(format!("sending to {target_display} failed: {e}"));
                        break 'send;
                    }
                },
                msg = rx.wait() => {
                    let (action, _) = handle_put_msg(target_display, msg, &mut written, max_output, &mut ran, &mut refused)?;
                    match action {
                        PutMsgAction::Continue => {}
                        PutMsgAction::Exited => {
                            remote_exited = true;
                            break 'send;
                        }
                        PutMsgAction::ClosedClean => {
                            remote_closed = true;
                            closed_cleanly = true;
                            break 'send;
                        }
                        PutMsgAction::TransportLost => {
                            remote_closed = true;
                            transport_lost = true;
                            break 'send;
                        }
                    }
                }
                () = &mut deadline => return Err(RexError::Timeout),
            }
        }
    }

    if !remote_closed && !remote_exited && refused.is_none() {
        let eof = tx.eof();
        tokio::pin!(eof);
        let deadline = tokio::time::sleep(send_timeout);
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                res = &mut eof => match res {
                    Ok(()) => {
                        if let Some(flag) = &eof_sent {
                            flag.store(true, std::sync::atomic::Ordering::Release);
                        }
                        break;
                    }
                    Err(e) => {
                        refused = Some(format!("sending to {target_display} failed: {e}"));
                        break;
                    }
                },
                msg = rx.wait() => {
                    let (action, _) = handle_put_msg(target_display, msg, &mut written, max_output, &mut ran, &mut refused)?;
                    match action {
                        PutMsgAction::Continue => {}
                        PutMsgAction::Exited => {
                            remote_exited = true;
                            break;
                        }
                        PutMsgAction::ClosedClean => {
                            remote_closed = true;
                            closed_cleanly = true;
                            break;
                        }
                        PutMsgAction::TransportLost => {
                            remote_closed = true;
                            transport_lost = true;
                            break;
                        }
                    }
                }
                () = &mut deadline => return Err(RexError::Timeout),
            }
        }
    }

    if !remote_closed {
        let mut post_exit_deadline = remote_exited.then(|| tokio::time::Instant::now() + exit_drain_timeout);
        let initial_deadline = post_exit_deadline.unwrap_or_else(|| tokio::time::Instant::now() + drain_idle_timeout);
        let deadline = tokio::time::sleep_until(initial_deadline);
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                msg = rx.wait() => {
                    let (action, progress) = handle_put_msg(target_display, msg, &mut written, max_output, &mut ran, &mut refused)?;
                    match action {
                        PutMsgAction::ClosedClean => {
                            closed_cleanly = true;
                            break;
                        }
                        PutMsgAction::TransportLost => {
                            transport_lost = true;
                            break;
                        }
                        PutMsgAction::Exited => {
                            remote_exited = true;
                            // Set ONE fixed post-exit drain deadline on the first terminal
                            // exit message; never renew it on subsequent descendant output.
                            if post_exit_deadline.is_none() {
                                let fixed = tokio::time::Instant::now() + exit_drain_timeout;
                                post_exit_deadline = Some(fixed);
                                deadline.as_mut().reset(fixed);
                            }
                        }
                        PutMsgAction::Continue if progress && !remote_exited => {
                            deadline.as_mut().reset(tokio::time::Instant::now() + drain_idle_timeout);
                        }
                        PutMsgAction::Continue => {}
                    }
                }
                () = &mut deadline => {
                    if remote_exited {
                        // Command already exited and reported status; do not wait
                        // indefinitely for a lingering Close and leave the close
                        // guard armed so the channel closes on drop.
                        break;
                    }
                    return Err(RexError::Timeout);
                }
            }
        }
    }

    if closed_cleanly {
        tx.disarm_close();
    }
    ran.output = String::from_utf8_lossy(&written).into_owned();
    if transport_lost && !(ran.status.is_some_and(|s| s != 0) && !ran.output.trim().is_empty()) {
        return Err(RexError::Ssh(refused.unwrap_or_else(|| format!("connection to {target_display} closed unexpectedly"))));
    }
    if let Some(refused) = refused.filter(|_| ran.output.trim().is_empty()) {
        ran.output = refused;
        ran.status.get_or_insert(1);
    }
    Ok(ran)
}

/// Streaming stdout reader for one-shot remote commands (such as `olo-rex-link
/// fs read`) where 0 bytes of stdout is a valid result when exit status is 0.
pub struct ExecStdout {
    target: String,
    rx: GuardedChannelRx,
    tx: ChannelDropGuard,
    exit_status: Option<u32>,
    exit_signal: Option<String>,
    stderr: Vec<u8>,
    done: bool,
}

impl ExecStdout {
    /// Read the next non-empty stdout chunk, `Ok(None)` when the command exited
    /// cleanly with status 0 and closed its channel, or `Err` on non-zero exit,
    /// refused exec, or unexpected transport disconnect.
    pub async fn next_chunk(&mut self) -> Result<Option<bytes::Bytes>, RexError> {
        exec_stdout_next(&self.target, &mut self.rx, &mut self.tx, &mut self.exit_status, &mut self.exit_signal, &mut self.stderr, &mut self.done).await
    }
}

async fn exec_stdout_next<R: ChannelRx, W: ChannelTx>(
    target: &str,
    rx: &mut R,
    tx: &mut W,
    exit_status: &mut Option<u32>,
    exit_signal: &mut Option<String>,
    stderr: &mut Vec<u8>,
    done: &mut bool,
) -> Result<Option<bytes::Bytes>, RexError> {
    use russh::ChannelMsg;
    if *done {
        return Ok(None);
    }
    loop {
        match rx.wait().await {
            Some(ChannelMsg::Data { data }) => {
                if !data.is_empty() {
                    return Ok(Some(data));
                }
            }
            Some(ChannelMsg::ExtendedData { data, .. }) => {
                let room = EXEC_STDOUT_MAX_STDERR.saturating_sub(stderr.len());
                if room > 0 {
                    stderr.extend_from_slice(&data[..data.len().min(room)]);
                }
            }
            Some(ChannelMsg::ExitStatus { exit_status: code }) => {
                *exit_status = Some(code);
            }
            Some(ChannelMsg::ExitSignal { signal_name, .. }) => {
                *exit_signal = Some(format!("{signal_name:?}"));
            }
            Some(ChannelMsg::Failure) => {
                *done = true;
                return Err(RexError::Ssh(format!("exec on {target} was refused")));
            }
            Some(ChannelMsg::Close) => {
                *done = true;
                tx.disarm_close();
                if *exit_status == Some(0) {
                    return Ok(None);
                }
                let err_text = String::from_utf8_lossy(stderr).trim().to_owned();
                if !err_text.is_empty() {
                    return Err(RexError::Ssh(err_text));
                }
                if let Some(sig) = exit_signal.as_deref() {
                    return Err(RexError::Ssh(format!("remote command on {target} terminated by signal {sig}")));
                }
                if let Some(code) = *exit_status {
                    return Err(RexError::Ssh(format!("remote command on {target} exited with status {code}")));
                }
                return Err(RexError::Ssh(format!("remote command on {target} closed without exit status")));
            }
            None => {
                *done = true;
                return Err(RexError::Ssh(format!("connection to {target} closed unexpectedly")));
            }
            Some(_) => {}
        }
    }
}

#[derive(Clone, Copy)]
enum ExecKind {
    Dial,
    Command,
    Screen,
}

#[derive(Clone, Copy)]
enum EndReason {
    /// Both `ChannelMsg::Eof` and an explicit exit status or signal arrived.
    EofWithStatus,
    /// Peer sent explicit `ChannelMsg::Close`.
    ExplicitClose,
    /// Receiver ended (`None`) without explicit `ChannelMsg::Close`
    /// (SSH transport lost or driver exited).
    TransportLost,
}

/// Maximum stderr bytes retained across the lifetime of an exec stream so a
/// failed remote command explains why it failed without unbounded buffering.
const EXEC_STDERR_CAP: usize = 4096;

struct ExecWriter<W> {
    inner: W,
    shared: Arc<ExecShared>,
}

impl<W: AsyncWrite + Unpin> AsyncWrite for ExecWriter<W> {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>, buf: &[u8]) -> std::task::Poll<io::Result<usize>> {
        use std::task::Poll;
        if self.shared.shutdown_started.load(Ordering::Acquire) {
            return Poll::Ready(Err(io::Error::new(io::ErrorKind::BrokenPipe, "SSH channel stdin already shut down")));
        }
        if self.shared.peer_closed.load(Ordering::Acquire) || self.shared.peer_exited.load(Ordering::Acquire) {
            return Poll::Ready(Err(io::Error::new(io::ErrorKind::BrokenPipe, "SSH channel closed by peer")));
        }
        match Pin::new(&mut self.inner).poll_write(cx, buf) {
            Poll::Ready(res) => Poll::Ready(res),
            Poll::Pending => {
                // Only pay for waker registration when actually stalled (e.g.
                // waiting on window credit), then recheck peer state so a
                // concurrent peer exit/close can never lose a wakeup. Note:
                // russh's own driver ALSO wakes `ChannelTx` directly on peer
                // CLOSE / disconnect even when nobody is reading (Finding 4).
                self.shared.writer_waker.register(cx.waker());
                if self.shared.peer_closed.load(Ordering::Acquire) || self.shared.peer_exited.load(Ordering::Acquire) {
                    return Poll::Ready(Err(io::Error::new(io::ErrorKind::BrokenPipe, "SSH channel closed by peer")));
                }
                Poll::Pending
            }
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<io::Result<()>> {
        use std::task::Poll;
        if self.shared.eof_sent.load(Ordering::Acquire) {
            return Poll::Ready(Ok(()));
        }
        // Set `shutdown_started` BEFORE polling `inner.poll_shutdown` so even
        // if shutdown is cancelled at `Pending`, a subsequent `poll_write`
        // cannot complete russh's stored EOF permit (Finding 10).
        self.shared.shutdown_started.store(true, Ordering::Release);
        if self.shared.peer_closed.load(Ordering::Acquire) {
            self.shared.eof_sent.store(true, Ordering::Release);
            return Poll::Ready(Ok(()));
        }
        let res = std::task::ready!(Pin::new(&mut self.inner).poll_shutdown(cx));
        if res.is_ok() {
            self.shared.eof_sent.store(true, Ordering::Release);
        }
        Poll::Ready(res)
    }
}

/// Streams bytes from an SSH exec channel with the final ending rule for
/// `Command` / `Screen` streams (Findings 2, 3, 5):
/// - Any non-zero `ExitStatus` or `ExitSignal` is an error after received
///   stdout bytes, whenever it arrives (retaining bounded `ext == 1` stderr).
/// - Channel `EOF` certifies output completeness: `EOF` + `ExitStatus(0)`, or
///   `EOF` + explicit `CLOSE` (with status 0 or with no status), ends cleanly.
/// - Any end without `EOF` (transport lost, bare `CLOSE`, dropped receiver)
///   returns `UnexpectedEof` regardless of any earlier `ExitStatus(0)`.
/// - After `EOF`, waits for `ExitStatus`, `ExitSignal`, or `CLOSE` before
///   answering (see [`SshSessionPool::exec_io`] for the audited caller list).
/// - Releases `read` immediately on terminal completion (`Command`/`Screen`
///   end or `Dial` half-close `EOF`) so a kept stream never fills russh's
///   channel buffer, while preserving duplex half-close writes on `Dial`.
struct ExecOutput {
    read: Option<russh::ChannelReadHalf>,
    prefetched: VecDeque<russh::ChannelMsg>,
    pending: Option<(bytes::Bytes, usize)>,
    kind: ExecKind,
    opened: bool,
    eof_seen: bool,
    done: bool,
    exit_status: Option<u32>,
    exit_signal: Option<russh::Sig>,
    stderr: Vec<u8>,
    shared: Arc<ExecShared>,
    _guard: ChannelDropGuard,
}

impl ExecOutput {
    fn format_with_stderr(&self, base: String) -> io::Error {
        let err = String::from_utf8_lossy(&self.stderr).trim().to_owned();
        if err.is_empty() { io::Error::other(base) } else { io::Error::other(format!("{base}: {err}")) }
    }

    fn finish_outcome(&mut self, reason: EndReason) -> io::Result<()> {
        self.done = true;
        // Drop the russh mpsc::Receiver immediately so no subsequent
        // WindowAdjusted/ExitStatus/Close can fill its queue and stall the
        // SSH packet driver while the stream is still held (Finding 2).
        self.read = None;
        self.prefetched.clear();
        match self.kind {
            ExecKind::Dial => {
                if self.opened {
                    return Ok(());
                }
                if let Some(127) = self.exit_status {
                    return Err(io::Error::other("remote rex command not found; install Rex on the remote host and add it to PATH"));
                }
                if let Some(code) = self.exit_status {
                    return Err(io::Error::other(format!("remote rex dial-stdio exited with status {code}")));
                }
                if let Some(sig) = &self.exit_signal {
                    return Err(io::Error::other(format!("remote command terminated by signal {sig:?}")));
                }
                Err(io::Error::other("remote SSH channel closed before dial-stdio replied"))
            }
            ExecKind::Command | ExecKind::Screen => {
                // 1. Explicit failure (non-zero exit status or exit signal) is
                //    ALWAYS an error to the reader, whenever it arrived.
                if let Some(code) = self.exit_status
                    && code != 0
                {
                    if matches!(self.kind, ExecKind::Screen) && !self.opened {
                        if code == 69 {
                            return Err(io::Error::other("the host has no screens-capable Olo helper"));
                        }
                        if code == 127 {
                            return Err(io::Error::other("Olo's helper was not found on the host"));
                        }
                    }
                    let what = if matches!(self.kind, ExecKind::Screen) { "remote screen helper" } else { "remote command" };
                    return Err(self.format_with_stderr(format!("{what} exited with status {code}")));
                }
                if let Some(sig) = &self.exit_signal {
                    return Err(self.format_with_stderr(format!("remote command terminated by signal {sig:?}")));
                }
                // 2. Channel EOF certifies output is whole: EOF + ExitStatus(0)
                //    or EOF + explicit CLOSE (with status 0 or none) is clean.
                if self.eof_seen && matches!(reason, EndReason::EofWithStatus | EndReason::ExplicitClose) {
                    return Ok(());
                }
                // 3. Ending without EOF (transport lost, bare CLOSE, or dropped
                //    receiver), or losing transport after EOF before status or
                //    CLOSE, is `UnexpectedEof` whatever status came before it.
                let base = if self.opened { "remote SSH channel closed before command finished" } else { "remote SSH channel closed before replying" };
                let err = String::from_utf8_lossy(&self.stderr).trim().to_owned();
                let msg = if err.is_empty() { base.to_owned() } else { format!("{base}: {err}") };
                Err(io::Error::new(io::ErrorKind::UnexpectedEof, msg))
            }
        }
    }
}

impl AsyncRead for ExecOutput {
    fn poll_read(self: Pin<&mut Self>, cx: &mut std::task::Context<'_>, buf: &mut ReadBuf<'_>) -> std::task::Poll<io::Result<()>> {
        use russh::ChannelMsg;
        use std::task::{Poll, ready};
        let this = self.get_mut();
        if buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        if let Some((data, idx)) = this.pending.as_mut() {
            let n = buf.remaining().min(data.len() - *idx);
            buf.put_slice(&data[*idx..*idx + n]);
            *idx += n;
            if *idx == data.len() {
                this.pending = None;
            }
            return Poll::Ready(Ok(()));
        }
        if this.done {
            return Poll::Ready(Ok(()));
        }
        loop {
            let message = if let Some(msg) = this.prefetched.pop_front() {
                Some(msg)
            } else {
                let Some(read) = this.read.as_mut() else {
                    this.done = true;
                    return Poll::Ready(Ok(()));
                };
                ready!(std::pin::pin!(read.wait()).poll(cx))
            };
            match message {
                Some(ChannelMsg::Data { data }) => {
                    if data.is_empty() || this.eof_seen {
                        continue;
                    }
                    this.opened = true;
                    let n = buf.remaining().min(data.len());
                    buf.put_slice(&data[..n]);
                    if n < data.len() {
                        this.pending = Some((data, n));
                    }
                    return Poll::Ready(Ok(()));
                }
                Some(ChannelMsg::ExtendedData { data, ext: 1 }) if !matches!(this.kind, ExecKind::Dial) => {
                    let room = EXEC_STDERR_CAP.saturating_sub(this.stderr.len());
                    if room > 0 {
                        this.stderr.extend_from_slice(&data[..data.len().min(room)]);
                    }
                }
                Some(ChannelMsg::Eof) => {
                    this.eof_seen = true;
                    if matches!(this.kind, ExecKind::Dial) && this.opened {
                        // Preserve duplex byte-stream half-close for dial-stdio:
                        // drop receiver immediately (Finding 2) without marking
                        // `peer_closed` so opposite-direction writes still work
                        // (and russh's driver still wakes writes on peer CLOSE).
                        this.done = true;
                        this.read = None;
                        return Poll::Ready(Ok(()));
                    }
                    if this.exit_status.is_some() || this.exit_signal.is_some() {
                        return Poll::Ready(this.finish_outcome(EndReason::EofWithStatus));
                    }
                }
                Some(ChannelMsg::ExitStatus { exit_status }) => {
                    this.exit_status = Some(exit_status);
                    this.shared.mark_peer_exited();
                    if this.eof_seen && !matches!(this.kind, ExecKind::Dial) {
                        return Poll::Ready(this.finish_outcome(EndReason::EofWithStatus));
                    }
                }
                Some(ChannelMsg::ExitSignal { signal_name, .. }) => {
                    this.exit_signal = Some(signal_name);
                    this.shared.mark_peer_exited();
                    if this.eof_seen && !matches!(this.kind, ExecKind::Dial) {
                        return Poll::Ready(this.finish_outcome(EndReason::EofWithStatus));
                    }
                }
                Some(ChannelMsg::Failure) => {
                    this.shared.mark_peer_exited();
                    this.done = true;
                    this.read = None;
                    let msg = match this.kind {
                        ExecKind::Dial => "remote SSH server refused dial-stdio",
                        ExecKind::Screen => "remote SSH server refused screen helper",
                        ExecKind::Command => "remote SSH server refused exec",
                    };
                    return Poll::Ready(Err(io::Error::other(msg)));
                }
                Some(ChannelMsg::Close) => {
                    this.shared.mark_peer_closed();
                    return Poll::Ready(this.finish_outcome(EndReason::ExplicitClose));
                }
                None => {
                    this.shared.mark_peer_closed();
                    return Poll::Ready(this.finish_outcome(EndReason::TransportLost));
                }
                _ => {}
            }
        }
    }
}

#[derive(Clone)]
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

/// Resolved endpoint & host identity pinned by a primary connection so every
/// secondary (`further` or `screen`) connection dials the exact same machine
/// and verifies the exact same host key (Finding 6).
#[derive(Clone)]
struct PinnedEndpoint {
    resolved: ResolvedSshConfig,
    peer_addr: Option<std::net::SocketAddr>,
    verified_key: Option<String>,
    generation: u64,
}

enum ConnectMode {
    Primary { peer: PeerCell, verified: KeyCell, generation: u64 },
    Secondary { pinned: PinnedEndpoint, silent: bool },
}

/// Cap connect and silent key/agent authentication on automatic secondary
/// screen connections so a 1-connection gateway or firewall drop falls back
/// promptly to the primary connection (Finding 7).
const SILENT_SCREEN_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

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

async fn connect_and_authenticate(target: &SshTarget, askpass: Option<Arc<dyn SshAskpass>>, agent: AgentCell, mode: ConnectMode) -> Result<PooledHandle, RexError> {
    let (resolved, pinned_addr, expected_key, peer, verified, silent, generation) = match mode {
        ConnectMode::Primary { peer, verified, generation } => (resolve_ssh_config(target)?, None, None, peer, verified, false, generation),
        ConnectMode::Secondary { pinned, silent } => {
            let PinnedEndpoint { resolved, peer_addr, verified_key, generation } = pinned;
            (resolved, peer_addr, verified_key, PeerCell::default(), KeyCell::default(), silent, generation)
        }
    };
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
    let askpass = if silent { None } else { askpass.map(|ui| Prompts { ui, waiting: waiting_tx.clone() }) };
    // Keep Verified identity private to this attempt until user authentication
    // actually succeeds so failed auth attempts never poison the pool's cell.
    let attempt_verified: KeyCell = Arc::new(std::sync::Mutex::new(None));
    let mut handler = SshClientHandler {
        host: resolved.hostname.clone(),
        port: resolved.port,
        user: resolved.user.clone(),
        askpass: askpass.clone(),
        round_trip: None,
        came: (std::time::Instant::now(), 0),
        fixed: window_knob.is_some(),
        agent,
        verified: attempt_verified.clone(),
        expected_key: expected_key.clone(),
    };
    // (A key nobody could be asked about is its own error: what to do
    // about it is another thing than for a host that can't be reached.)
    let unasked = askpass.is_none();
    let failed_at = |how: &str, e: russh::Error| match e {
        russh::Error::UnknownKey if unasked => RexError::SshHostUnknown(format!("{}:{}", resolved.hostname, resolved.port)),
        e => RexError::Ssh(format!("{how} failed: {e}")),
    };
    let connected_addr: Arc<std::sync::Mutex<Option<std::net::SocketAddr>>> = Arc::new(std::sync::Mutex::new(pinned_addr));
    let connected_addr_w = connected_addr.clone();
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
            client::connect_stream(packed(config, None), stream, handler).await.map_err(|e| failed_at("SSH connect via ProxyCommand", e))
        } else {
            // (The connection is opened here, not by russh, to know
            // how long it took: the host's name is looked up first, so
            // that isn't counted. Secondary connections dial the exact
            // `pinned_addr` of the primary without re-resolving DNS.)
            let failed = |e: std::io::Error| RexError::Ssh(format!("SSH connect {}:{} failed: {e}", resolved.hostname, resolved.port));
            let addresses: Vec<std::net::SocketAddr> = match pinned_addr {
                Some(addr) => vec![addr],
                None => tokio::net::lookup_host((resolved.hostname.as_str(), resolved.port)).await.map_err(failed)?.collect(),
            };
            let began = std::time::Instant::now();
            let stream = tokio::net::TcpStream::connect(&addresses[..]).await.map_err(failed)?;

            // (`OLO_REX_SSH_OPENED_IN_MS`: taken to have opened in so
            // long, for the rig, whose far host opens at once whatever
            // its link's delay: what follows from a host being far, or
            // from its round trip, is then as on a real one.)
            let opened_in = std::env::var("OLO_REX_SSH_OPENED_IN_MS").ok().and_then(|v| v.parse().ok()).map_or(began.elapsed(), Duration::from_millis);
            handler.round_trip = Some(opened_in);
            let peer_sock = stream.peer_addr().ok();
            *connected_addr_w.lock().unwrap_or_else(|e| e.into_inner()) = peer_sock;
            *peer.lock().unwrap_or_else(|e| e.into_inner()) = peer_sock.map(|a| (a.ip(), opened_in));
            stream.set_nodelay(true).map_err(failed)?;
            client::connect_stream(packed(config, Some(opened_in)), stream, handler).await.map_err(|e| failed_at(&format!("SSH connect {}:{}", resolved.hostname, resolved.port), e))
        }
    };

    let connect_limit = if silent { resolved.connect_timeout.min(SILENT_SCREEN_CONNECT_TIMEOUT) } else { resolved.connect_timeout };
    let auth_limit = if silent { SILENT_SCREEN_CONNECT_TIMEOUT } else { Duration::from_secs(120) };
    let session = network_timeout(connect_limit, &mut waiting, connection).await?;
    let handle = network_timeout(auth_limit, &mut waiting, authenticate(session, target, &resolved, askpass)).await?;
    let peer_addr = *connected_addr.lock().unwrap_or_else(|e| e.into_inner());
    // Bind and publish Verified identity ONLY after user authentication succeeds.
    let handle_verified = attempt_verified
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .or_else(|| expected_key.clone().map(|key| Verified { hostname: resolved.hostname.clone(), port: resolved.port, user: resolved.user.clone(), key }));
    *verified.lock().unwrap_or_else(|e| e.into_inner()) = handle_verified.clone();
    let verified_key = expected_key.or_else(|| handle_verified.as_ref().map(|v| v.key.clone()));
    Ok(PooledHandle { handle, runtime: tokio::runtime::Handle::current(), pinned: PinnedEndpoint { resolved, peer_addr, verified_key, generation }, verified: handle_verified })
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
/// What is run on the host to start the helper's own link over UDP:
/// the app's own build of the helper (no other: the two ends of the
/// link are of one build), in its `quic` mode, with what it dials for
/// each stream. Where that build isn't there, or runs nothing, it ends
/// at once and there is no link.
fn quic_command(socket: Option<&str>, id: &str) -> String {
    let dial = rex_dial(socket);
    let dir = crate::helper::DIR;
    let script = format!("{FIND_REX}L=\"$HOME\"/{dir}/olo-rex-link-{id}-$(uname -m); if [ -f \"$L\" ] && [ -O \"$L\" ]; then exec \"$L\" quic -- {dial}; fi; exit 127");
    format!("sh -c {}", shell_escape(&script))
}

/// `rex` where an SSH command on the host finds it: on its path, else
/// where Rex's own installer puts an account's (`~/.local/bin`), which a
/// command's shell doesn't have on its path on Ubuntu (its `.bashrc` stops
/// before its lines when not interactive): as `provision`'s look finds it.
/// An Ubuntu host set up for an account's own server was never reached
/// ("remote rex command not found", the rig's move test, 2026-11-25).
const FIND_REX: &str = "R=$(command -v rex 2> /dev/null) || R=\"$HOME\"/.local/bin/rex; ";

/// The server's command, `rex` as [`FIND_REX`] found it.
fn rex_dial(socket: Option<&str>) -> String {
    match socket {
        Some(sock) => format!("\"$R\" -S unix://{} __internal dial-stdio", shell_escape(sock)),
        None => "\"$R\" __internal dial-stdio".to_owned(),
    }
}

fn dial_command(socket: Option<&str>, helper: Option<&str>) -> String {
    let dial = rex_dial(socket);
    let Some(id) = helper else { return format!("sh -c {}", shell_escape(&format!("{FIND_REX}exec {dial}"))) };
    let dir = crate::helper::DIR;
    let script = format!(
        "{FIND_REX}d=\"$HOME\"/{dir}; m=$(uname -m); \
         run() {{ if [ -f \"$1\" ] && [ -O \"$1\" ] && \"$1\" --version > /dev/null 2>&1; then exec \"$1\" -- {dial}; fi; }}; \
         run \"$d/olo-rex-link-{id}-$m\"; \
         for L in $(ls -t \"$d\"/olo-rex-link-*-\"$m\" 2> /dev/null); do run \"$L\"; done; \
         exec {dial}"
    );
    format!("sh -c {}", shell_escape(&script))
}

/// What is run on the host over SSH for an Agent Screens command or stream
/// ([`SshSessionPool::open_screen_stream`] and one-shot `olo-rex-link screen`
/// commands): prefers the app's own build of the helper (`helper`, from
/// [`SshSessionPool::set_helper`]), else the newest eligible other build for
/// this machine in `helper::DIR` (selected from quoted glob matches with
/// `[ -nt ]`, never word-splitting `ls` output).
///
/// Each candidate is verified to be an account-owned regular file whose
/// `PATH=/dev/null "$1" screen --protocol </dev/null 2>/dev/null` exits 0 and
/// emits valid JSON carrying `"proto": <digit>` and a `"commands": [...]`
/// array (allowing arbitrary JSON whitespace around `:` and `[`); for the
/// streaming verbs (`follow`, `stream`, `history-stream`, `export`, and 2-arg
/// `history NAME`), membership is tested strictly inside the `"commands"`
/// array (never `"capabilities"`). For non-streaming commands (`--protocol`,
/// one-shot CLI verbs, and legacy `history NAME --wire`), a `--protocol`-capable
/// helper always wins over a newer help-only build, falling back to a legacy
/// `screen --help` (`"usage: olo screen"*`) helper only when no
/// `--protocol`-capable build exists on the host. Exits with status 69 if no
/// eligible helper is found.
///
/// Mirrors `olo-rex-link`'s QUIC `SCREEN` dispatch by rewriting 2-arg
/// `history NAME` to `history-stream NAME` so both transports speak the same
/// binary wire protocol, and quotes every argument for the remote POSIX shell
/// via [`shell_escape`].
pub fn screen_command(helper: Option<&str>, args: &[String]) -> String {
    let dir = crate::helper::DIR;
    let helper = helper.filter(|id| !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric()));
    let streaming_verb = match args.first().map(String::as_str) {
        Some("follow") => Some("follow"),
        Some("stream") => Some("stream"),
        Some("history-stream") => Some("history-stream"),
        Some("history") if args.len() == 2 => Some("history-stream"),
        Some("export") => Some("export"),
        _ => None,
    };
    let mut quoted = String::new();
    for (i, arg) in args.iter().enumerate() {
        let arg = if i == 0 && arg == "history" && args.len() == 2 { "history-stream" } else { arg.as_str() };
        quoted.push(' ');
        quoted.push_str(&shell_escape(arg));
    }
    let verb_match = streaming_verb.map(|v| format!("case \"$cmds\" in *'\"{v}\"'*) ;; *) return 1 ;; esac; ")).unwrap_or_default();
    let preferred_proto = helper.map(|id| format!("if probe_proto \"$d/olo-rex-link-{id}-$m\"; then exec \"$d/olo-rex-link-{id}-$m\" screen{quoted}; fi; ")).unwrap_or_default();
    let legacy_fallback = if streaming_verb.is_none() {
        let preferred_legacy = helper.map(|id| format!("if probe_legacy \"$d/olo-rex-link-{id}-$m\"; then exec \"$d/olo-rex-link-{id}-$m\" screen{quoted}; fi; ")).unwrap_or_default();
        format!(
            "probe_legacy() {{ \
               [ -f \"$1\" ] && [ -O \"$1\" ] || return 1; \
               case \"$(PATH=/dev/null \"$1\" screen --help </dev/null 2>/dev/null)\" in \"usage: olo screen\"*) return 0 ;; esac; \
               return 1; \
             }}; \
             {preferred_legacy}\
             best=\"\"; \
             for p in \"$d\"/olo-rex-link-*-\"$m\"; do \
               if probe_legacy \"$p\"; then \
                 if [ -z \"$best\" ] || [ \"$p\" -nt \"$best\" ]; then best=\"$p\"; fi; \
               fi; \
             done; \
             if [ -n \"$best\" ]; then exec \"$best\" screen{quoted}; fi; "
        )
    } else {
        String::new()
    };
    // Pure POSIX `sh` JSON field extractor for `"proto": <digit>` and the
    // `"commands": [...]` slice (allows spaces, tabs, CR, LF around `:`/`[`).
    let skip_ws = "while :; do case \"$r\" in \" \"*|\"\t\"*|\"\r\"*|\"\n\"*) r=${r#?} ;; *) break ;; esac; done";
    let script = format!(
        "d=\"$HOME\"/{dir}; m=$(uname -m); \
         probe_proto() {{ \
           [ -f \"$1\" ] && [ -O \"$1\" ] || return 1; \
           out=$(PATH=/dev/null \"$1\" screen --protocol </dev/null 2>/dev/null) || return 1; \
           r=${{out#*'\"proto\"'}}; [ \"$r\" != \"$out\" ] || return 1; \
           {skip_ws}; \
           case \"$r\" in :*) r=${{r#:}} ;; *) return 1 ;; esac; \
           {skip_ws}; \
           case \"$r\" in [0-9]*) ;; *) return 1 ;; esac; \
           r=${{out#*'\"commands\"'}}; [ \"$r\" != \"$out\" ] || return 1; \
           {skip_ws}; \
           case \"$r\" in :*) r=${{r#:}} ;; *) return 1 ;; esac; \
           {skip_ws}; \
           case \"$r\" in '['*) r=${{r#'['}} ;; *) return 1 ;; esac; \
           cmds=${{r%%']'*}}; [ \"$cmds\" != \"$r\" ] || return 1; \
           {verb_match}return 0; \
         }}; \
         {preferred_proto}\
         best=\"\"; \
         for p in \"$d\"/olo-rex-link-*-\"$m\"; do \
           if probe_proto \"$p\"; then \
             if [ -z \"$best\" ] || [ \"$p\" -nt \"$best\" ]; then best=\"$p\"; fi; \
           fi; \
         done; \
         if [ -n \"$best\" ]; then exec \"$best\" screen{quoted}; fi; \
         {legacy_fallback}\
         exit 69"
    );
    format!("sh -c {}", shell_escape(&script))
}

#[cfg(test)]
mod dial_tests {
    use super::*;

    /// What keeps the agent on a host tells `ssh` there of it: a file
    /// of Olo's own, read first by one line put at the head of the
    /// account's SSH config, once, with what the config had kept as it
    /// was (a config that is a link stays one); and `ssh` itself reads
    /// it as meant: the link's path is its agent while the socket is
    /// there and the shell has none of its own, and not otherwise.
    #[test]
    fn ssh_on_the_host_finds_the_agent_with_no_variable() {
        use std::os::unix::net::UnixListener;
        let home = std::path::PathBuf::from(format!("/var/tmp/olo-rex-agent-kept-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join(".ssh")).unwrap();
        std::fs::create_dir_all(home.join("dotfiles")).unwrap();
        std::fs::write(home.join("dotfiles/ssh-config"), "Host build\n    User you\n").unwrap();
        std::os::unix::fs::symlink(home.join("dotfiles/ssh-config"), home.join(".ssh/config")).unwrap();
        let forwarded = home.join("forwarded.sock");
        let _listening = UnixListener::bind(&forwarded).unwrap();
        // (As the SSH server runs it: `sh -c`, with the socket it made
        // in SSH_AUTH_SOCK; it says the path and waits on its input.)
        let run = || {
            let out = std::process::Command::new("sh").arg("-c").arg(AGENT_KEPT).env("HOME", &home).env("SSH_AUTH_SOCK", &forwarded).stdin(std::process::Stdio::null()).output().unwrap();
            assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
            String::from_utf8_lossy(&out.stdout).trim().to_owned()
        };
        let link = home.join(".local/state/olo-rex/agent.sock");
        assert_eq!(run(), link.to_str().unwrap());
        assert_eq!(run(), link.to_str().unwrap());
        let config = std::fs::read_to_string(home.join(".ssh/config")).unwrap();
        assert_eq!(config, "Include ~/.ssh/olo-rex.conf\n\nHost build\n    User you\n", "one line at its head, however often; the rest as it was");
        assert!(std::fs::symlink_metadata(home.join(".ssh/config")).unwrap().file_type().is_symlink(), "a config that is a link stays one");
        // What ssh makes of it (`-G`: the settings it would use), read
        // from this home's files: by `-F`, and the include by its
        // whole path, since ssh takes `~` from the account, not HOME.
        let own = std::fs::read_to_string(home.join(".ssh/olo-rex.conf")).unwrap();
        let conf = home.join("read.conf");
        std::fs::write(&conf, own.replace("~/.local", &format!("{}/.local", home.display()))).unwrap();
        let agent = |auth: Option<&std::path::Path>| {
            let mut ssh = std::process::Command::new("ssh");
            ssh.arg("-F").arg(&conf).args(["-G", "example.invalid"]).env("HOME", &home).env_remove("SSH_AUTH_SOCK");
            if let Some(auth) = auth {
                ssh.env("SSH_AUTH_SOCK", auth);
            }
            let out = ssh.output().unwrap();
            String::from_utf8_lossy(&out.stdout).lines().find_map(|l| l.strip_prefix("identityagent ").map(str::to_owned)).unwrap_or_default()
        };
        if std::process::Command::new("ssh").arg("-V").output().is_ok() {
            assert_eq!(agent(None), link.to_str().unwrap(), "a shell with no agent: Olo's");
            assert_eq!(agent(Some(&home.join("gone.sock"))), link.to_str().unwrap(), "a shell whose agent is gone: Olo's");
            // A live agent with nothing in it (the host's own openssh
            // service gives every login one): Olo's.
            let empty = home.join("empty.sock");
            fake_agent(&empty, false);
            assert_eq!(agent(Some(&empty)), link.to_str().unwrap(), "a shell whose agent is empty: Olo's");
            // (No agent named: ssh goes by the shell's own, as without the file.)
            let keyed = home.join("keyed.sock");
            fake_agent(&keyed, true);
            assert_eq!(agent(Some(&keyed)), "", "a shell whose own agent has keys keeps it");
            // One that never answers holds nothing up for long, and counts as none.
            let began = std::time::Instant::now();
            assert_eq!(agent(Some(&forwarded)), link.to_str().unwrap(), "a shell whose agent never answers: Olo's");
            assert!(began.elapsed() < Duration::from_secs(5), "{:?}", began.elapsed());
            // The keys taken back: the socket goes, and ssh is as it was.
            drop(_listening);
            std::fs::remove_file(&forwarded).unwrap();
            assert_eq!(agent(None), "");
        }
        // An account with no SSH config yet gets one, its own alone,
        // that says only this (seen on the rig: none was made, and
        // ssh there went on without the agent).
        std::fs::remove_file(home.join(".ssh/config")).unwrap();
        let _listening = UnixListener::bind(&forwarded).unwrap();
        run();
        assert_eq!(std::fs::read_to_string(home.join(".ssh/config")).unwrap(), "Include ~/.ssh/olo-rex.conf\n\n");
        assert_eq!(std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(home.join(".ssh/config")).unwrap().permissions()) & 0o777, 0o600);
        let _ = std::fs::remove_dir_all(&home);
    }

    /// An SSH agent at `path` for the test, on a thread: asked for its
    /// keys, it has none, or one (an Ed25519 key's public half).
    fn fake_agent(path: &std::path::Path, with_a_key: bool) {
        use std::io::{Read, Write};
        let listening = std::os::unix::net::UnixListener::bind(path).unwrap();
        std::thread::spawn(move || {
            for mut stream in listening.incoming().flatten() {
                let mut len = [0u8; 4];
                while stream.read_exact(&mut len).is_ok() {
                    let mut msg = vec![0u8; u32::from_be_bytes(len) as usize];
                    if stream.read_exact(&mut msg).is_err() {
                        break;
                    }
                    let string = |b: &[u8]| [&(b.len() as u32).to_be_bytes()[..], b].concat();
                    // (SSH_AGENT_IDENTITIES_ANSWER, 12: how many, then each key's blob and comment.)
                    let mut answer = vec![12u8];
                    if with_a_key {
                        answer.extend(1u32.to_be_bytes());
                        answer.extend(string(&[string(b"ssh-ed25519"), string(&[7u8; 32])].concat()));
                        answer.extend(string(b"test"));
                    } else {
                        answer.extend(0u32.to_be_bytes());
                    }
                    let _ = stream.write_all(&[&(answer.len() as u32).to_be_bytes()[..], &answer].concat());
                }
            }
        });
    }

    /// The link is used where UDP's round trip is about TCP's, not
    /// where the network carries it slower.
    #[test]
    fn the_link_isnt_used_where_udp_is_slower() {
        let ms = Duration::from_millis;
        assert!(link_is_as_quick(ms(166), ms(167)) && link_is_as_quick(ms(210), ms(167)));
        assert!(!link_is_as_quick(ms(225), ms(167)), "what was measured: a third slower");
        assert!(link_is_as_quick(ms(11), ms(1)) && !link_is_as_quick(ms(30), ms(1)), "a host next door");
        let said = quic_command(Some("/run/it's/rex.sock"), "0a1b2c");
        assert!(said.contains("olo-rex-link-0a1b2c-$(uname -m)") && said.contains("quic -- \"$R\" -S unix://") && said.contains(".local/bin/rex") && said.ends_with("exit 127'"), "{said}");
    }

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
        // (The commands' path: the stand-ins and links to the two programs
        // the dial uses, nothing else: this computer's own `rex` is never
        // reached, whatever its path has.)
        let tools = dir.join("tools");
        std::fs::create_dir_all(&tools).unwrap();
        for program in ["sh", "uname", "ls"] {
            let at = String::from_utf8(std::process::Command::new("sh").arg("-c").arg(format!("command -v {program}")).output().unwrap().stdout).unwrap();
            std::os::unix::fs::symlink(at.trim(), tools.join(program)).unwrap();
        }
        let run = |command: &str| {
            let path = format!("{}:{}", bin.display(), tools.display());
            let out = std::process::Command::new("sh").arg("-c").arg(command).env("HOME", &home).env("PATH", path).output().unwrap();
            String::from_utf8_lossy(&out.stdout).trim().to_owned()
        };
        let rex = bin.join("rex");
        // (No helper, no socket: the server's own command, as the path finds it.)
        assert_eq!(run(&dial_command(None, None)), "rex: __internal dial-stdio");
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
            assert_eq!(run(&command), format!("mine: -- {} {wanted}", rex.display()));
            // With other builds beside it: still the app's own.
            script(&file("old1"), &helper("old1"));
            aged("0a1b2c", 3);
            assert_eq!(run(&command), format!("mine: -- {} {wanted}", rex.display()));
            // Only other builds: the newest of them.
            std::fs::remove_file(file("0a1b2c")).unwrap();
            script(&file("old2"), &helper("old2"));
            aged("old1", 9);
            aged("old2", 2);
            assert_eq!(run(&command), format!("old2: -- {} {wanted}", rex.display()));
            aged("old2", 20);
            assert_eq!(run(&command), format!("old1: -- {} {wanted}", rex.display()));
            // The app's own build there, but it doesn't run (a file cut
            // short, a home that runs nothing): another that does.
            script(&file("0a1b2c"), "exit 126");
            assert_eq!(run(&command), format!("old1: -- {} {wanted}", rex.display()));
            std::fs::write(file("0a1b2c"), b"\x7fELF not a program").unwrap();
            assert_eq!(run(&command), format!("old1: -- {} {wanted}", rex.display()));
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
        // Rex where its own installer puts an account's, not on a
        // command's path (Ubuntu's .bashrc stops before its lines): found.
        std::fs::remove_file(&rex).unwrap();
        std::fs::create_dir_all(home.join(".local/bin")).unwrap();
        script(&home.join(".local/bin/rex"), "echo \"rex in .local/bin: $*\"");
        assert_eq!(run(&dial_command(None, None)), "rex in .local/bin: __internal dial-stdio");
        script(&file("0a1b2c"), &helper("mine"));
        assert_eq!(run(&dial_command(None, Some("0a1b2c"))), format!("mine: -- {} __internal dial-stdio", home.join(".local/bin/rex").display()));
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

    /// Finding 13: ordinary `SshSessionPool::exec` stays unbounded so
    /// whole-record callers (Tracker listings, base64 accounts files) get a
    /// 200 KiB single line intact; `exec_bounded` (used only by tweaks LOOK)
    /// caps lines and total output and marks `Ran::truncated`.
    #[test]
    fn exec_ordinary_path_keeps_200k_single_line_whole_and_bounded_caps_it() {
        let payload = "a".repeat(200 * 1024);
        let wire = format!("{payload}\n");

        // Ordinary (unbounded) path: 200 KiB single line delivered whole across chunks.
        let mut unbounded = ExecCollector::new(None);
        let mut seen = Vec::new();
        for chunk in wire.as_bytes().chunks(4096) {
            unbounded.take(chunk, true, &mut |line| seen.push(line.to_owned()));
        }
        let ran = unbounded.finish(&mut |line| seen.push(line.to_owned()));
        assert!(!ran.truncated, "ordinary exec must never truncate");
        assert_eq!(ran.stdout, wire);
        assert_eq!(ran.output, wire);
        assert_eq!(seen, vec![payload.clone()]);

        // Bounded path (tweaks LOOK): 64 KiB single-line cap clips and marks truncated.
        let mut bounded = ExecCollector::new(Some((4 * 1024 * 1024, 64 * 1024)));
        for chunk in wire.as_bytes().chunks(4096) {
            bounded.take(chunk, true, &mut |_| {});
        }
        let clipped = bounded.finish(&mut |_| {});
        assert!(clipped.truncated, "bounded exec must report truncation when clipping a long line");
        assert_eq!(clipped.stdout.len(), 64 * 1024 + 1);

        // Bounded total output cap also sets `truncated`.
        let mut capped = ExecCollector::new(Some((32, 64 * 1024)));
        capped.take(b"01234567890123456789\n01234567890123456789\n", true, &mut |_| {});
        let capped_ran = capped.finish(&mut |_| {});
        assert!(capped_ran.truncated, "bounded exec must report truncation when total output cap is hit");
        assert_eq!(capped_ran.stdout, "01234567890123456789\n");
    }

    /// Findings 7 & 15a: `screen_command` probes `screen --protocol` with
    /// `PATH=/dev/null` and `</dev/null`, rejects old released helpers that
    /// print `"usage: olo screen"` on `--help` without supporting streaming
    /// verbs, never invokes an external `screen` binary or consumes queued
    /// client stdin during probing, sorts eligible builds by mtime without
    /// word-splitting filenames containing spaces, maps `history NAME` to
    /// `history-stream NAME`, quotes every argument literally, and exits 69
    /// when no capable helper exists.
    #[test]
    fn screen_command_selects_screens_capable_helper_and_quotes_args() {
        use std::io::Write as _;
        use std::os::unix::fs::PermissionsExt;
        use std::process::{Command, Stdio};

        let dir = crate::quic::TestTempDir::new("olo-scmd");
        let home = dir.path().join("home with spaces");
        let there = home.join(crate::helper::DIR);
        let fake_path_dir = dir.path().join("fake-bin");
        std::fs::create_dir_all(&there).unwrap();
        std::fs::create_dir_all(&fake_path_dir).unwrap();
        let script = |path: &std::path::Path, body: &str| {
            std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        };
        let screen_marker = dir.path().join("external-screen-ran");
        script(&fake_path_dir.join("screen"), &format!("echo ran > {}; exit 0", shell_escape(&screen_marker.display().to_string())));

        // Full streaming-capable helper (implements `screen --protocol` and echoes stdin).
        let capable_helper = |name: &str, verbs: &str| {
            format!(
                "if [ \"$1\" = screen ] && [ \"$2\" = --protocol ]; then printf '{{\"proto\":1,\"commands\":[{verbs}],\"capabilities\":[]}}\\n'; exit 0; fi; \
                 printf '{name}:'; for a in \"$@\"; do printf ' [%s]' \"$a\"; done; \
                 in=$(cat); if [ -n \"$in\" ]; then printf ' stdin=[%s]' \"$in\"; fi; echo"
            )
        };
        // Real old released helper (`1d97c88af`): prints `"usage: olo screen"` on `--help`,
        // forwards unknown verbs (like `--protocol`) to non-existent `olo-screen` (exits 1).
        let old_olo_cli_helper = "if [ \"$1\" = screen ] && [ \"$2\" = --help ]; then echo 'usage: olo screen install ...'; exit 0; fi; exit 1";
        // Even older pre-screens helper that runs external `screen` from PATH and reads stdin.
        let ancient_helper = "cat >/dev/null; exec screen \"$@\"";

        let machine = String::from_utf8(Command::new("uname").arg("-m").output().unwrap().stdout).unwrap().trim().to_owned();
        let file = |id: &str| there.join(format!("olo-rex-link-{id}-{machine}"));
        let path_env = format!("{}:/usr/bin:/bin", fake_path_dir.display());
        let run_with_stdin = |command: &str, stdin_bytes: &[u8]| {
            let mut child = Command::new("sh").arg("-c").arg(command).env("HOME", &home).env("PATH", &path_env).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
            if !stdin_bytes.is_empty() {
                child.stdin.take().unwrap().write_all(stdin_bytes).unwrap();
            } else {
                drop(child.stdin.take());
            }
            let out = child.wait_with_output().unwrap();
            (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).trim().to_owned())
        };
        let run = |command: &str| run_with_stdin(command, b"");

        // 1. No helper at all -> exit 69.
        assert_eq!(run(&screen_command(Some("cur"), &["follow".into()])), (69, String::new()));

        // 2. Old released Olo helper (`usage: olo screen`, no `--protocol`) + ancient
        //    helper that execs `screen` and drains stdin -> both skipped, fake `screen`
        //    NEVER executed, exit 69.
        script(&file("oldcli"), old_olo_cli_helper);
        script(&file("ancient"), ancient_helper);
        assert_eq!(run_with_stdin(&screen_command(Some("oldcli"), &["follow".into()]), b"queued-input"), (69, String::new()));
        assert!(!screen_marker.exists(), "probe must never execute external `screen` from PATH");

        // 3. Helper advertising only `follow` is skipped when asking for `stream qa`,
        //    but accepted when asking for `follow`.
        script(&file("followonly"), &capable_helper("followonly", "\"follow\""));
        assert_eq!(run(&screen_command(Some("followonly"), &["stream".into(), "qa".into()])), (69, String::new()));
        assert_eq!(run(&screen_command(Some("followonly"), &["follow".into()])), (0, "followonly: [screen] [follow]".to_owned()));

        // 4. Preferred capable build wins over newer other build, passes queued stdin
        //    intact (probing used `</dev/null`), maps `history NAME` -> `history-stream NAME`,
        //    and quotes shell metacharacters literally.
        let all_verbs = "\"follow\",\"stream\",\"history-stream\",\"export\"";
        script(&file("cur"), &capable_helper("cur", all_verbs));
        script(&file("other"), &capable_helper("other", all_verbs));
        std::fs::File::options().write(true).open(file("cur")).unwrap().set_modified(std::time::SystemTime::now() - Duration::from_secs(3600)).unwrap();
        assert_eq!(run(&screen_command(Some("cur"), &["--protocol".into()])), (0, format!("{{\"proto\":1,\"commands\":[{all_verbs}],\"capabilities\":[]}}")));
        assert_eq!(run(&screen_command(Some("cur"), &["history".into(), "qa".into()])), (0, "cur: [screen] [history-stream] [qa]".to_owned()));
        assert_eq!(run(&screen_command(Some("cur"), &["export".into(), "qa".into(), "--part".into(), "1-2,$(id);'x'".into()])), (0, "cur: [screen] [export] [qa] [--part] [1-2,$(id);'x']".to_owned()));
        assert_eq!(run_with_stdin(&screen_command(Some("cur"), &["stream".into(), "qa".into()]), b"queued-input"), (0, "cur: [screen] [stream] [qa] stdin=[queued-input]".to_owned()));
        assert!(!screen_marker.exists(), "probe must never execute external `screen` from PATH");

        // 5. Fallback mtime selection across TWO eligible builds — flip mtime both ways
        //    so removing `[ "$p" -nt "$best" ]` fails the test (Finding 7).
        std::fs::remove_file(file("cur")).unwrap();
        std::fs::remove_file(file("followonly")).unwrap();
        script(&file("alpha"), &capable_helper("alpha", all_verbs));
        script(&file("beta"), &capable_helper("beta", all_verbs));
        let now = std::time::SystemTime::now();
        std::fs::File::options().write(true).open(file("other")).unwrap().set_modified(now - Duration::from_secs(7200)).unwrap();
        std::fs::File::options().write(true).open(file("alpha")).unwrap().set_modified(now - Duration::from_secs(600)).unwrap();
        std::fs::File::options().write(true).open(file("beta")).unwrap().set_modified(now - Duration::from_secs(60)).unwrap();
        assert_eq!(run(&screen_command(None, &["follow".into()])), (0, "beta: [screen] [follow]".to_owned()));
        std::fs::File::options().write(true).open(file("beta")).unwrap().set_modified(now - Duration::from_secs(1200)).unwrap();
        assert_eq!(run(&screen_command(None, &["follow".into()])), (0, "alpha: [screen] [follow]".to_owned()));

        // 6. Finding 15a: eligible helper whose basename contains a space is NOT word-split.
        for id in ["alpha", "beta", "other"] {
            std::fs::remove_file(file(id)).unwrap();
        }
        script(&file("with space"), &capable_helper("spaced", all_verbs));
        assert_eq!(run(&screen_command(None, &["follow".into()])), (0, "spaced: [screen] [follow]".to_owned()));
        assert!(!screen_marker.exists());
    }

    #[tokio::test]
    async fn open_screen_stream_rejects_disallowed_args_before_connecting() {
        let pool = Arc::new(SshSessionPool::default());
        // Unreachable target: if validation didn't reject upfront, this would
        // try to connect and return an SSH connect error instead of Protocol.
        let target = SshTarget::parse("ssh://nobody@127.0.0.1:1").unwrap();
        for bad in [vec![], vec!["remove".into()], vec!["follow".into(), "extra".into()], vec!["stream".into(), "../bad".into()], vec!["export".into(), "qa".into(), "--out".into(), "/tmp/x".into()]] {
            let err = pool.open_screen_stream(&target, None, &bad).await.err().expect("disallowed args must fail");
            let expected = olo_rex_link::screen_wire::validate_quic_screen_args(&bad).unwrap_err().to_string();
            assert!(matches!(err, RexError::Protocol(ref msg) if msg == &expected), "for {bad:?}: got {err:?}, expected Protocol({expected:?})");
        }
    }
}

/// Deterministic in-process `russh::server` tests covering Findings 8, 10, 11,
/// 13 (bounded bootstrap announcement), 14 (local channel table count), 15b
/// (`exec_pty` refused PTY), Test Finding 2 (exact EOF/CLOSE counts across
/// double shutdown + drop beside a sibling channel), and Test Finding 4
/// (byte-for-byte non-repeating integrity under 1- to 3-byte reads + stderr).
#[cfg(test)]
mod round3_mock_server_tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    struct TestClient;
    impl client::Handler for TestClient {
        type Error = russh::Error;
        async fn check_server_key(&mut self, _key: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
            Ok(true)
        }
    }

    #[derive(Default)]
    struct MockStats {
        eof_count: AtomicUsize,
        close_count: AtomicUsize,
        data_after_eof: AtomicUsize,
        exec_called: AtomicBool,
        active_sessions: AtomicUsize,
        max_sessions: AtomicUsize,
        open_delay_ms: AtomicUsize,
        never_confirm: AtomicBool,
        deny_pty: AtomicBool,
        no_window_refill: AtomicBool,
        received_bytes: AtomicUsize,
        writer_blocked_notify: tokio::sync::Notify,
        received_on_dial: std::sync::Mutex<Vec<u8>>,
        eof_channels: std::sync::Mutex<std::collections::HashSet<russh::ChannelId>>,
        mode_by_channel: std::sync::Mutex<HashMap<russh::ChannelId, String>>,
    }

    struct MockServer {
        stats: Arc<MockStats>,
    }

    impl russh::server::Server for MockServer {
        type Handler = MockHandler;
        fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self::Handler {
            MockHandler { stats: self.stats.clone() }
        }
    }

    struct MockHandler {
        stats: Arc<MockStats>,
    }

    impl russh::server::Handler for MockHandler {
        type Error = russh::Error;

        async fn auth_none(&mut self, _user: &str) -> Result<russh::server::Auth, Self::Error> {
            Ok(russh::server::Auth::Accept)
        }

        async fn channel_open_session(
            &mut self,
            channel: russh::Channel<russh::server::Msg>,
            reply: russh::server::ChannelOpenHandle,
            _session: &mut russh::server::Session,
        ) -> Result<(), Self::Error> {
            if self.stats.never_confirm.load(Ordering::SeqCst) {
                // Hold the open handle without confirming or rejecting (Finding 10).
                tokio::spawn(async move {
                    let _keep = (channel, reply);
                    std::future::pending::<()>().await;
                });
                return Ok(());
            }
            let max = self.stats.max_sessions.load(Ordering::SeqCst);
            let cur = self.stats.active_sessions.fetch_add(1, Ordering::SeqCst);
            if max > 0 && cur >= max {
                self.stats.active_sessions.fetch_sub(1, Ordering::SeqCst);
                reply.reject(russh::ChannelOpenFailure::ResourceShortage).await;
                return Ok(());
            }
            let delay = self.stats.open_delay_ms.load(Ordering::SeqCst) as u64;
            let _id = channel.id();
            if delay > 0 {
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                    reply.accept().await;
                });
            } else {
                reply.accept().await;
            }
            Ok(())
        }

        fn adjust_window(&mut self, _channel: russh::ChannelId, current: u32) -> u32 {
            if self.stats.no_window_refill.load(Ordering::SeqCst) || (self.stats.close_count.load(Ordering::SeqCst) > 0 && current <= 4096) { 0 } else { current }
        }

        async fn pty_request(
            &mut self,
            channel: russh::ChannelId,
            _term: &str,
            _col_width: u32,
            _row_height: u32,
            _pix_width: u32,
            _pix_height: u32,
            _modes: &[(russh::Pty, u32)],
            session: &mut russh::server::Session,
        ) -> Result<(), Self::Error> {
            if self.stats.deny_pty.load(Ordering::SeqCst) {
                session.channel_failure(channel)?;
            } else {
                session.channel_success(channel)?;
            }
            Ok(())
        }

        async fn exec_request(&mut self, channel: russh::ChannelId, data: &[u8], session: &mut russh::server::Session) -> Result<(), Self::Error> {
            self.stats.exec_called.store(true, Ordering::SeqCst);
            let cmd = String::from_utf8_lossy(data).into_owned();
            self.stats.mode_by_channel.lock().unwrap().insert(channel, cmd.clone());
            match cmd.as_str() {
                "refuse_exec_leave_open" => {
                    session.channel_failure(channel)?;
                    return Ok(());
                }
                "hang_exec" => {
                    // Deliberately never send CHANNEL_SUCCESS or CHANNEL_FAILURE (Finding 10).
                    return Ok(());
                }
                "early_data_before_success" => {
                    // Send early stdout BEFORE CHANNEL_SUCCESS (Finding 10).
                    session.data(channel, bytes::Bytes::from_static(b"early-"))?;
                    session.channel_success(channel)?;
                    let handle = session.handle();
                    tokio::spawn(async move {
                        let _ = handle.data(channel, bytes::Bytes::from_static(b"after")).await;
                        let _ = handle.eof(channel).await;
                        let _ = handle.exit_status_request(channel, 0).await;
                        let _ = handle.close(channel).await;
                    });
                    return Ok(());
                }
                _ => {
                    session.channel_success(channel)?;
                }
            }
            let handle = session.handle();
            match cmd.as_str() {
                "echo" => {}
                "pty_ok" => {
                    tokio::spawn(async move {
                        let _ = handle.data(channel, bytes::Bytes::from_static(b"pty-ready\n")).await;
                        let _ = handle.eof(channel).await;
                        let _ = handle.exit_status_request(channel, 0).await;
                        let _ = handle.close(channel).await;
                    });
                }
                "status_before_data" => {
                    // Finding 11: ExitStatus(3) arrives BEFORE stderr and stdout!
                    tokio::spawn(async move {
                        let _ = handle.exit_status_request(channel, 3).await;
                        let _ = handle.extended_data(channel, 1, bytes::Bytes::from_static(b"late-stderr\n")).await;
                        let _ = handle.extended_data(channel, 2, bytes::Bytes::from_static(b"ignored-ext2\n")).await;
                        let _ = handle.data(channel, bytes::Bytes::from_static(b"late-stdout")).await;
                        let _ = handle.eof(channel).await;
                        let _ = handle.close(channel).await;
                    });
                }
                "signal_with_stderr" => {
                    tokio::spawn(async move {
                        let _ = handle.extended_data(channel, 1, bytes::Bytes::from_static(b"oom stderr\n")).await;
                        let _ = handle.exit_signal_request(channel, russh::Sig::TERM, false, String::new(), String::new()).await;
                        let _ = handle.close(channel).await;
                    });
                }
                "close_no_status_with_stderr" => {
                    tokio::spawn(async move {
                        let _ = handle.extended_data(channel, 1, bytes::Bytes::from_static(b"abrupt stderr\n")).await;
                        let _ = handle.close(channel).await;
                    });
                }
                "eof_then_illegal_data_and_status0_no_close" => {
                    // Finding 11: EOF, illegal DATA after EOF, ExitStatus(0), NO CLOSE.
                    tokio::spawn(async move {
                        let _ = handle.eof(channel).await;
                        let _ = handle.data(channel, bytes::Bytes::from_static(b"MUST-NOT-APPEAR")).await;
                        let _ = handle.exit_status_request(channel, 0).await;
                        // Deliberately do NOT send Close yet — reader must still return Ok(0)!
                    });
                }
                "data_exit0_disconnect_before_eof" => {
                    // Finding 2: Data + ExitStatus(0) + transport disconnect BEFORE EOF/CLOSE.
                    tokio::spawn(async move {
                        let _ = handle.data(channel, bytes::Bytes::from_static(b"truncated-prefix")).await;
                        let _ = handle.exit_status_request(channel, 0).await;
                        tokio::time::sleep(Duration::from_millis(20)).await;
                        let _ = handle.disconnect(russh::Disconnect::ConnectionLost, "lost".to_owned(), "en".to_owned()).await;
                    });
                }
                "data_exit0_bare_close_no_eof" => {
                    // Finding 2 / Rule 3: Data + ExitStatus(0) + bare CLOSE without EOF.
                    tokio::spawn(async move {
                        let _ = handle.data(channel, bytes::Bytes::from_static(b"prefix-no-eof")).await;
                        let _ = handle.exit_status_request(channel, 0).await;
                        let _ = handle.close(channel).await;
                    });
                }
                "data_eof_close_no_status" => {
                    // Findings 3 & 5: complete `agent request` JSON refusal + EOF + CLOSE with NO exit status.
                    tokio::spawn(async move {
                        let _ = handle.data(channel, bytes::Bytes::from_static(b"{\"ok\":false,\"error\":\"refused by policy\"}\n")).await;
                        let _ = handle.eof(channel).await;
                        let _ = handle.close(channel).await;
                    });
                }
                "data_eof_exit3" => {
                    // Rule 1 & 4: Data + EOF + ExitStatus(3) + CLOSE.
                    tokio::spawn(async move {
                        let _ = handle.data(channel, bytes::Bytes::from_static(b"partial-body")).await;
                        let _ = handle.eof(channel).await;
                        tokio::time::sleep(Duration::from_millis(20)).await;
                        let _ = handle.extended_data(channel, 1, bytes::Bytes::from_static(b"failed late\n")).await;
                        let _ = handle.exit_status_request(channel, 3).await;
                        let _ = handle.close(channel).await;
                    });
                }
                "data_eof_disconnect_no_status" => {
                    // Rule 4: Data + EOF + transport lost before status or CLOSE.
                    tokio::spawn(async move {
                        let _ = handle.data(channel, bytes::Bytes::from_static(b"partial-eof")).await;
                        let _ = handle.eof(channel).await;
                        tokio::time::sleep(Duration::from_millis(20)).await;
                        let _ = handle.disconnect(russh::Disconnect::ConnectionLost, "lost".to_owned(), "en".to_owned()).await;
                    });
                }
                "dial_half_close" => {
                    // Finding 10: peer sends banner + EOF (half-close), keeps reading stdin.
                    tokio::spawn(async move {
                        let _ = handle.data(channel, bytes::Bytes::from_static(b"hi")).await;
                        let _ = handle.eof(channel).await;
                    });
                }
                "dial_half_close_then_peer_close_on_fill" => {
                    // Finding 4: peer sends banner + EOF, waits until client uploads >= 4096 bytes, then sends CLOSE.
                    let stats = self.stats.clone();
                    tokio::spawn(async move {
                        let _ = handle.data(channel, bytes::Bytes::from_static(b"hi")).await;
                        let _ = handle.eof(channel).await;
                        while stats.received_bytes.load(Ordering::SeqCst) < 4096 {
                            let notified = stats.writer_blocked_notify.notified();
                            if stats.received_bytes.load(Ordering::SeqCst) >= 4096 {
                                break;
                            }
                            notified.await;
                        }
                        let _ = handle.close(channel).await;
                    });
                }
                "close_on_window_fill" => {
                    // Finding 4: deterministically wait until client uploads >= 256 KiB
                    // (generating > 100 WINDOW_ADJUST on a 4 KiB window with no reader),
                    // then send CHANNEL_CLOSE.
                    let stats = self.stats.clone();
                    tokio::spawn(async move {
                        while stats.received_bytes.load(Ordering::SeqCst) < 256 * 1024 {
                            let notified = stats.writer_blocked_notify.notified();
                            if stats.received_bytes.load(Ordering::SeqCst) >= 256 * 1024 {
                                break;
                            }
                            notified.await;
                        }
                        let _ = handle.close(channel).await;
                    });
                }
                "close_after_zero_credit_wait" => {
                    // Finding 4 & 15b: server starts with 0 window credit (no refill),
                    // lets client park in zero-credit wait / pending_data, then sends CLOSE.
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_millis(120)).await;
                        let _ = handle.close(channel).await;
                    });
                }
                "close_immediately_with_credit" => {
                    // Finding 4: close channel immediately while client still has unused credit.
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_millis(30)).await;
                        let _ = handle.close(channel).await;
                    });
                }
                "close_while_writing" => {
                    // Finding 10: wait briefly while client fills write window, then Close channel.
                    tokio::spawn(async move {
                        let _ = handle.data(channel, bytes::Bytes::from_static(b"go")).await;
                        tokio::time::sleep(Duration::from_millis(80)).await;
                        let _ = handle.close(channel).await;
                    });
                }
                "flood_bounded" => {
                    // Emits both stdout & stderr, then floods until client closes channel on truncation.
                    tokio::spawn(async move {
                        let _ = handle.data(channel, bytes::Bytes::from_static(b"same-line\nout-line\n")).await;
                        let _ = handle.extended_data(channel, 1, bytes::Bytes::from_static(b"same-line\nerr-line\n")).await;
                        let chunk = bytes::Bytes::from(vec![b'Z'; 8192]);
                        for _ in 0..1000 {
                            if handle.data(channel, chunk.clone()).await.is_err() {
                                break;
                            }
                            tokio::time::sleep(Duration::from_millis(5)).await;
                        }
                    });
                }
                "stdout_and_stderr_identical" => {
                    tokio::spawn(async move {
                        let _ = handle.data(channel, bytes::Bytes::from_static(b"dup\nonly-out\n")).await;
                        let _ = handle.extended_data(channel, 1, bytes::Bytes::from_static(b"dup\nonly-err\n")).await;
                        let _ = handle.eof(channel).await;
                        let _ = handle.exit_status_request(channel, 0).await;
                        let _ = handle.close(channel).await;
                    });
                }
                "exit_status_no_close" => {
                    // Finding 11: ExitStatus(127) without Close.
                    tokio::spawn(async move {
                        let _ = handle.exit_status_request(channel, 127).await;
                    });
                }
                "long_banner" => {
                    // Finding 13: stream > 512 bytes with no newline on quic announce.
                    tokio::spawn(async move {
                        let chunk = bytes::Bytes::from(vec![b'A'; 1024]);
                        let _ = handle.data(channel, chunk.clone()).await;
                        let _ = handle.data(channel, chunk).await;
                    });
                }
                "integrity_stream" => {
                    // Test Finding 4: 64 KiB non-repeating payload in irregular chunks
                    // interleaved with stderr, ending in ExitStatus(0).
                    tokio::spawn(async move {
                        let mut state: u64 = 0x1234_5678_9abc_def0;
                        let mut full = Vec::with_capacity(65536);
                        for _ in 0..65536 {
                            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                            full.push((state >> 56) as u8);
                        }
                        let mut off = 0usize;
                        let mut step = 13usize;
                        while off < full.len() {
                            let end = (off + step).min(full.len());
                            let _ = handle.data(channel, bytes::Bytes::copy_from_slice(&full[off..end])).await;
                            if off % 4096 < step {
                                let _ = handle.extended_data(channel, 1, bytes::Bytes::from_static(b"progress\n")).await;
                            }
                            off = end;
                            step = (step * 3 + 7) % 1021 + 1;
                        }
                        let _ = handle.eof(channel).await;
                        let _ = handle.exit_status_request(channel, 0).await;
                        let _ = handle.close(channel).await;
                    });
                }
                _ => {}
            }
            Ok(())
        }

        async fn data(&mut self, channel: russh::ChannelId, data: &[u8], session: &mut russh::server::Session) -> Result<(), Self::Error> {
            if self.stats.eof_channels.lock().unwrap().contains(&channel) {
                self.stats.data_after_eof.fetch_add(1, Ordering::SeqCst);
            }
            let mode = self.stats.mode_by_channel.lock().unwrap().get(&channel).cloned().unwrap_or_default();
            if mode == "echo" {
                session.data(channel, bytes::Bytes::copy_from_slice(data))?;
            } else if mode == "dial_half_close" {
                self.stats.received_on_dial.lock().unwrap().extend_from_slice(data);
            } else if mode == "close_on_window_fill" || mode == "dial_half_close_then_peer_close_on_fill" {
                self.stats.received_bytes.fetch_add(data.len(), Ordering::SeqCst);
                self.stats.writer_blocked_notify.notify_waiters();
            }
            Ok(())
        }

        async fn channel_eof(&mut self, channel: russh::ChannelId, _session: &mut russh::server::Session) -> Result<(), Self::Error> {
            self.stats.eof_count.fetch_add(1, Ordering::SeqCst);
            self.stats.eof_channels.lock().unwrap().insert(channel);
            Ok(())
        }

        async fn channel_close(&mut self, channel: russh::ChannelId, _session: &mut russh::server::Session) -> Result<(), Self::Error> {
            self.stats.close_count.fetch_add(1, Ordering::SeqCst);
            self.stats.active_sessions.fetch_sub(1, Ordering::SeqCst);
            self.stats.mode_by_channel.lock().unwrap().remove(&channel);
            Ok(())
        }
    }

    async fn start_mock_ssh(stats: Arc<MockStats>, initial_window: u32) -> (russh::client::Handle<TestClient>, tokio::task::JoinHandle<()>) {
        use russh::server::Server as _;
        let seed = crate::quic::new_token().unwrap();
        let key = russh::keys::PrivateKey::from(russh::keys::ssh_key::private::Ed25519Keypair::from_seed(&seed));
        let mut cfg = russh::server::Config::default();
        cfg.keys.push(key);
        cfg.window_size = initial_window;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let srv_task = tokio::spawn(async move {
            let mut srv = MockServer { stats };
            let _ = srv.run_on_socket(Arc::new(cfg), &listener).await;
        });
        let client_cfg = Arc::new(client::Config::default());
        let tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
        let mut handle = client::connect_stream(client_cfg, tcp, TestClient).await.unwrap();
        assert!(handle.authenticate_none("tester").await.unwrap().success());
        (handle, srv_task)
    }

    /// Findings 8 & 14: cancelling `channel_open_session` before delayed
    /// confirmation closes the channel on a 1-slot server and leaves `(0, 0)`
    /// entries in russh's channel tables so the next open succeeds immediately.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelled_channel_open_frees_single_slot_and_leaks_zero_channels() {
        let stats = Arc::new(MockStats::default());
        stats.max_sessions.store(1, Ordering::SeqCst);
        stats.open_delay_ms.store(120, Ordering::SeqCst);
        let (handle, srv) = start_mock_ssh(stats.clone(), 256 * 1024).await;

        // Cancel mid-open while server delays CHANNEL_OPEN_CONFIRMATION.
        assert!(tokio::time::timeout(Duration::from_millis(25), handle.channel_open_session()).await.is_err());

        // Wait for the delayed confirmation + russh's automatic CLOSE to complete.
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while (stats.close_count.load(Ordering::SeqCst) == 0 || handle.open_channel_count().await.unwrap() != (0, 0)) && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
        assert_eq!(stats.close_count.load(Ordering::SeqCst), 1, "cancelled open must send CHANNEL_CLOSE");
        assert_eq!(handle.open_channel_count().await.unwrap(), (0, 0), "cancelled open must leak zero russh channels");

        // Now the single slot is free and a second channel opens cleanly!
        stats.open_delay_ms.store(0, Ordering::SeqCst);
        let ch = handle.channel_open_session().await.expect("1-slot server must admit next channel after cancelled open");
        let rt = tokio::runtime::Handle::current();
        let guarded = GuardedChannel::new(ch, rt);
        drop(guarded);
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while handle.open_channel_count().await.unwrap() != (0, 0) && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(handle.open_channel_count().await.unwrap(), (0, 0));
        srv.abort();
    }

    /// Finding 10 & Test Finding 2: write after shutdown fails with BrokenPipe
    /// (zero DATA after EOF), double shutdown + drop sends EXACTLY 1 EOF and 1
    /// CLOSE while a sibling channel keeps echoing, peer EOF alone still allows
    /// writes in the opposite direction, and peer CLOSE wakes a stalled writer
    /// with BrokenPipe.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn exec_writer_shutdown_double_eof_half_close_and_peer_close_wakeup() {
        let stats = Arc::new(MockStats::default());
        // Small server receive window (4 KiB) so the stalled-writer test exhausts
        // credit immediately and waits on WindowAdjust until peer sends CLOSE.
        let (handle, srv) = start_mock_ssh(stats.clone(), 4096).await;
        let rt = tokio::runtime::Handle::current();

        // Sibling echo channel kept open throughout on the SAME SSH connection.
        let sib_ch = GuardedChannel::new(handle.channel_open_session().await.unwrap(), rt.clone());
        sib_ch.exec(true, "echo").await.unwrap();
        let mut sibling = sib_ch.into_exec_stream(ExecKind::Command);

        // 1. Double shutdown + write-after-shutdown + drop => exactly 1 EOF, 1 CLOSE, 0 DATA after EOF.
        let ch = GuardedChannel::new(handle.channel_open_session().await.unwrap(), rt.clone());
        ch.exec(true, "echo").await.unwrap();
        let mut s = ch.into_exec_stream(ExecKind::Command);
        s.shutdown().await.unwrap();
        s.shutdown().await.unwrap();
        let err = s.write(b"after-eof").await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::BrokenPipe);
        drop(s);
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while stats.close_count.load(Ordering::SeqCst) < 1 && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(stats.eof_count.load(Ordering::SeqCst), 1, "must send exactly ONE EOF across double shutdown + drop");
        assert_eq!(stats.close_count.load(Ordering::SeqCst), 1, "must send exactly ONE CLOSE");
        assert_eq!(stats.data_after_eof.load(Ordering::SeqCst), 0, "must never enqueue DATA after EOF");
        // Sibling echo still works on the same connection.
        sibling.write_all(b"sib").await.unwrap();
        let mut sb = [0u8; 3];
        sibling.read_exact(&mut sb).await.unwrap();
        assert_eq!(&sb, b"sib");

        // 2. Peer EOF alone on Dial still permits writes in the opposite direction.
        let ch = GuardedChannel::new(handle.channel_open_session().await.unwrap(), rt.clone());
        ch.exec(true, "dial_half_close").await.unwrap();
        let mut dial = ch.into_exec_stream(ExecKind::Dial);
        let mut out = Vec::new();
        dial.read_to_end(&mut out).await.unwrap();
        assert_eq!(out, b"hi");
        dial.write_all(b"upload-after-peer-eof").await.unwrap();
        dial.shutdown().await.unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while stats.received_on_dial.lock().unwrap().as_slice() != b"upload-after-peer-eof" && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(stats.received_on_dial.lock().unwrap().as_slice(), b"upload-after-peer-eof");
        drop(dial);

        // 3. Pending writer stalled on 0 window credit wakes with BrokenPipe when peer closes.
        let ch = GuardedChannel::new(handle.channel_open_session().await.unwrap(), rt.clone());
        ch.exec(true, "close_while_writing").await.unwrap();
        let stream = ch.into_exec_stream(ExecKind::Command);
        let (mut r, mut w) = tokio::io::split(stream);
        let mut go = [0u8; 2];
        r.read_exact(&mut go).await.unwrap();
        let reader_task = tokio::spawn(async move {
            let mut rest = Vec::new();
            let _ = r.read_to_end(&mut rest).await;
        });
        let big = vec![b'X'; 32 * 1024];
        let write_err = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                w.write_all(&big).await?;
            }
            #[allow(unreachable_code)]
            Ok::<(), io::Error>(())
        })
        .await
        .expect("writer must wake on peer CLOSE instead of hanging forever")
        .unwrap_err();
        assert_eq!(write_err.kind(), io::ErrorKind::BrokenPipe, "expected BrokenPipe, got: {write_err:?}");
        let _ = reader_task.await;
        drop(w);

        drop(sibling);
        srv.abort();
    }

    /// Finding 11 & Test Finding 4: out-of-order status before data/stderr,
    /// stderr retained on ExitSignal and Close-without-status, ext!=1 ignored,
    /// EOF+ExitStatus(0) finishes without waiting for Close while ignoring
    /// illegal post-EOF DATA, and 64 KiB byte-for-byte integrity with 1-3 byte reads.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn exec_output_packet_ordering_stderr_and_byte_for_byte_integrity() {
        let stats = Arc::new(MockStats::default());
        let (handle, srv) = start_mock_ssh(stats, 256 * 1024).await;
        let rt = tokio::runtime::Handle::current();

        // 1. ExitStatus(3) BEFORE stderr + stdout -> stdout delivered, stderr kept, ext=2 ignored.
        let ch = GuardedChannel::new(handle.channel_open_session().await.unwrap(), rt.clone());
        ch.exec(true, "status_before_data").await.unwrap();
        let mut s = ch.into_exec_stream(ExecKind::Command);
        let mut out = Vec::new();
        let err = s.read_to_end(&mut out).await.unwrap_err().to_string();
        assert_eq!(out, b"late-stdout", "stdout arriving after ExitStatus must not be lost");
        assert!(err.contains('3') && err.contains("late-stderr") && !err.contains("ignored-ext2"), "got: {err}");

        // 2. ExitSignal preserves retained stderr.
        let ch = GuardedChannel::new(handle.channel_open_session().await.unwrap(), rt.clone());
        ch.exec(true, "signal_with_stderr").await.unwrap();
        let mut s = ch.into_exec_stream(ExecKind::Command);
        let mut out = Vec::new();
        let err = s.read_to_end(&mut out).await.unwrap_err().to_string();
        assert!(err.contains("TERM") && err.contains("oom stderr"), "got: {err}");

        // 3. Close without status preserves retained stderr and returns UnexpectedEof.
        let ch = GuardedChannel::new(handle.channel_open_session().await.unwrap(), rt.clone());
        ch.exec(true, "close_no_status_with_stderr").await.unwrap();
        let mut s = ch.into_exec_stream(ExecKind::Command);
        let mut out = Vec::new();
        let err = s.read_to_end(&mut out).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
        assert!(err.to_string().contains("abrupt stderr"), "got: {err}");

        // 4. EOF + illegal DATA + ExitStatus(0) with NO Close -> clean Ok(0) immediately, no illegal bytes!
        let ch = GuardedChannel::new(handle.channel_open_session().await.unwrap(), rt.clone());
        ch.exec(true, "eof_then_illegal_data_and_status0_no_close").await.unwrap();
        let mut s = ch.into_exec_stream(ExecKind::Command);
        let mut out = Vec::new();
        tokio::time::timeout(Duration::from_secs(2), s.read_to_end(&mut out)).await.expect("EOF + ExitStatus(0) must finish without waiting for Close").unwrap();
        assert!(out.is_empty(), "illegal post-EOF DATA must never be delivered: {out:?}");

        // 5. Test Finding 4: 64 KiB non-repeating byte stream read in 1-, 2-, 3-byte chunks + interleaved stderr.
        let ch = GuardedChannel::new(handle.channel_open_session().await.unwrap(), rt);
        ch.exec(true, "integrity_stream").await.unwrap();
        let mut s = ch.into_exec_stream(ExecKind::Command);
        let mut got = Vec::with_capacity(65536);
        let mut sizes = [1usize, 2, 3].into_iter().cycle();
        let mut scratch = [0u8; 3];
        loop {
            let want = sizes.next().unwrap();
            let n = s.read(&mut scratch[..want]).await.unwrap();
            if n == 0 {
                break;
            }
            got.extend_from_slice(&scratch[..n]);
        }
        let mut expected = Vec::with_capacity(65536);
        let mut state: u64 = 0x1234_5678_9abc_def0;
        for _ in 0..65536 {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            expected.push((state >> 56) as u8);
        }
        assert_eq!(got, expected, "every byte of non-repeating stream must match under 1-3 byte reads");

        srv.abort();
    }

    /// Finding 15b & Finding 13: denied PTY never sends `exec` and reports PTY
    /// refusal; oversized `quic` bootstrap announcement (> 512 bytes without
    /// newline) is rejected immediately without buffering unbounded output.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn refused_pty_skips_exec_and_bootstrap_announcement_is_bounded() {
        let stats = Arc::new(MockStats::default());
        stats.deny_pty.store(true, Ordering::SeqCst);
        let (handle, srv) = start_mock_ssh(stats.clone(), 256 * 1024).await;
        let rt = tokio::runtime::Handle::current();

        // 1. Finding 15b: PTY refused -> wait_request_reply returns Ok(false), exec is never sent.
        let mut pty_ch = GuardedChannel::new(handle.channel_open_session().await.unwrap(), rt.clone());
        pty_ch.request_pty(true, "xterm-256color", 80, 24, 0, 0, &[]).await.unwrap();
        assert!(!pty_ch.wait_request_reply(Duration::from_secs(2)).await.unwrap());
        assert!(!stats.exec_called.load(Ordering::SeqCst), "exec must never be sent after refused PTY");
        drop(pty_ch);

        // 2. Finding 13: oversized bootstrap banner (> 512 bytes without '\n') rejected immediately.
        let mut boot_ch = GuardedChannel::new(handle.channel_open_session().await.unwrap(), rt);
        boot_ch.exec(true, "long_banner").await.unwrap();
        let err = tokio::time::timeout(Duration::from_secs(2), read_announce_line(&mut boot_ch)).await.expect("bounded announce read must not wait 5s").unwrap_err();
        assert!(err.contains("512"), "expected 512-byte cap error, got: {err}");
        drop(boot_ch);

        srv.abort();
    }

    /// Finding 5b: `demote_link_if_dead` never demotes a still-alive link and
    /// never lets a stale dead link A overwrite a freshly installed link B.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn demote_link_if_dead_preserves_healthy_and_fresh_links() {
        let Some(bin) = crate::quic::find_test_helper(&std::env::current_exe().unwrap()) else { return };
        let scratch = crate::quic::TestTempDir::new("olo-f5b");
        for sub in ["home", "run", "state", "desktop"] {
            std::fs::create_dir_all(scratch.path().join(sub)).unwrap();
        }
        let token = crate::quic::new_token().unwrap();
        let mut child = tokio::process::Command::new(&bin)
            .args(["quic", "--", "cat"])
            .env("HOME", scratch.path().join("home"))
            .env("XDG_RUNTIME_DIR", scratch.path().join("run"))
            .env("XDG_STATE_HOME", scratch.path().join("state"))
            .env("OLO_HOST_DESKTOP", scratch.path().join("desktop"))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(format!("{}\n", crate::quic::hex(&token)).as_bytes()).await.unwrap();
        let mut line = String::new();
        tokio::time::timeout(Duration::from_secs(10), tokio::io::AsyncBufReadExt::read_line(&mut tokio::io::BufReader::new(child.stdout.take().unwrap()), &mut line)).await.unwrap().unwrap();
        let (port, print) = crate::quic::announced(&line).unwrap();
        let addr = (std::net::Ipv4Addr::LOCALHOST, port).into();

        let link_a = Arc::new(crate::quic::QuicLink::connect(addr, &print, token).await.unwrap());
        let link_b = Arc::new(crate::quic::QuicLink::connect(addr, &print, token).await.unwrap());
        let stats = Arc::new(MockStats::default());
        let (ssh_h, srv) = start_mock_ssh(stats, 65536).await;
        let rt = tokio::runtime::Handle::current();
        let keeper = GuardedChannel::new(ssh_h.channel_open_session().await.unwrap(), rt);

        let pool = SshSessionPool::default();
        let target = SshTarget::parse("ssh://tester@127.0.0.1:22").unwrap();
        let target_gen = pool.target_pool(&target).generation;
        pool.links.lock().unwrap().insert(target.clone(), Link::Up { link: link_b.clone(), keeper, helper_id: "h1".into(), target_gen, screens_supported: Arc::new(AtomicBool::new(true)) });

        // 1. Healthy link_b failing one stream MUST NOT demote Link::Up.
        pool.demote_link_if_dead(&target, &link_b, "stream refused").await;
        assert!(pool.linked(&target).await, "healthy link must never be demoted");

        // 2. Stale dead link_a reporting failure MUST NOT overwrite freshly installed link_b.
        link_a.close_now();
        assert!(!link_a.alive());
        pool.demote_link_if_dead(&target, &link_a, "stale link died").await;
        assert!(pool.linked(&target).await, "stale dead link A must not overwrite fresh link B");

        // 3. When link_b itself actually dies, demote_link_if_dead transitions to Down.
        link_b.close_now();
        pool.demote_link_if_dead(&target, &link_b, "link_b died").await;
        assert!(!pool.linked(&target).await);

        drop((link_a, link_b, stdin, pool));
        let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
        srv.abort();
    }

    /// Connect an `SshClientHandler` (pinned by `expected_key`, zero `$HOME`
    /// access!) to a mock `russh::server` and install it as the primary
    /// `PooledHandle` in `pool` so tests exercise the REAL public pool methods.
    async fn attach_mock_primary(pool: &SshSessionPool, target: &SshTarget, stats: Arc<MockStats>, initial_window: u32) -> (Arc<PooledHandle>, tokio::task::JoinHandle<()>) {
        use russh::server::Server as _;
        let seed = crate::quic::new_token().unwrap();
        let key = russh::keys::PrivateKey::from(russh::keys::ssh_key::private::Ed25519Keypair::from_seed(&seed));
        let pub_openssh = key.public_key().to_openssh().unwrap().split_whitespace().take(2).collect::<Vec<_>>().join(" ");
        let mut cfg = russh::server::Config::default();
        cfg.keys.push(key);
        cfg.window_size = initial_window;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let srv_task = tokio::spawn(async move {
            let mut srv = MockServer { stats };
            let _ = srv.run_on_socket(Arc::new(cfg), &listener).await;
        });
        let client_cfg = Arc::new(client::Config::default());
        let tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
        let handler = SshClientHandler {
            host: "127.0.0.1".into(),
            port: addr.port(),
            user: "tester".into(),
            askpass: None,
            round_trip: Some(Duration::from_millis(1)),
            came: (std::time::Instant::now(), 0),
            fixed: false,
            agent: AgentCell::default(),
            verified: KeyCell::default(),
            expected_key: Some(pub_openssh.clone()),
        };
        let mut handle = client::connect_stream(client_cfg, tcp, handler).await.unwrap();
        assert!(handle.authenticate_none("tester").await.unwrap().success());
        let tp = pool.target_pool(target);
        let pooled = Arc::new(PooledHandle {
            handle,
            runtime: tokio::runtime::Handle::current(),
            pinned: PinnedEndpoint {
                resolved: ResolvedSshConfig {
                    hostname: "127.0.0.1".into(),
                    port: addr.port(),
                    user: "tester".into(),
                    identity_files: Vec::new(),
                    keepalive_secs: 0,
                    keepalive_max: 3,
                    connect_timeout: Duration::from_secs(5),
                    proxy_command: None,
                    compression: None,
                },
                peer_addr: Some(addr),
                verified_key: Some(pub_openssh.clone()),
                generation: tp.generation,
            },
            verified: Some(Verified { hostname: "127.0.0.1".into(), port: addr.port(), user: "tester".into(), key: pub_openssh }),
        });
        tp.state.lock().unwrap().primary = Some(pooled.clone());
        (pooled, srv_task)
    }

    /// Findings 2, 3, 5: verify every line of the final Command/Screen ending rule:
    /// - data + exit 0 + disconnect before EOF -> UnexpectedEof (never clean!)
    /// - data + exit 0 + bare CLOSE without EOF -> UnexpectedEof (never clean!)
    /// - data + EOF + CLOSE with NO exit status (`agent request` refusal) -> clean Ok with full reply!
    /// - data + EOF + exit 3 -> waits past EOF and returns Err with status 3 + stderr
    /// - data + EOF + disconnect before status/CLOSE -> UnexpectedEof
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn final_ending_rule_all_packet_orders() {
        let rt = tokio::runtime::Handle::current();

        // 1. Finding 2: Data + ExitStatus(0) + transport disconnect BEFORE EOF -> UnexpectedEof!
        {
            let stats = Arc::new(MockStats::default());
            let (handle, srv) = start_mock_ssh(stats, 65536).await;
            let mut ch = GuardedChannel::new(handle.channel_open_session().await.unwrap(), rt.clone());
            ch.exec_and_confirm("data_exit0_disconnect_before_eof", Duration::from_secs(2), "refused").await.unwrap();
            let mut s = ch.into_exec_stream(ExecKind::Command);
            let mut out = Vec::new();
            let err = s.read_to_end(&mut out).await.expect_err("exit 0 + disconnect before EOF MUST NOT certify prefix");
            assert_eq!(out, b"truncated-prefix");
            assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof, "got: {err:?}");
            srv.abort();
        }

        // 2. Rule 3: Data + ExitStatus(0) + bare CLOSE without EOF -> UnexpectedEof!
        {
            let stats = Arc::new(MockStats::default());
            let (handle, srv) = start_mock_ssh(stats, 65536).await;
            let mut ch = GuardedChannel::new(handle.channel_open_session().await.unwrap(), rt.clone());
            ch.exec_and_confirm("data_exit0_bare_close_no_eof", Duration::from_secs(2), "refused").await.unwrap();
            let mut s = ch.into_exec_stream(ExecKind::Command);
            let mut out = Vec::new();
            let err = s.read_to_end(&mut out).await.expect_err("exit 0 + bare CLOSE without EOF MUST fail");
            assert_eq!(out, b"prefix-no-eof");
            assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof, "got: {err:?}");
            srv.abort();
        }

        // 3. Findings 3 & 5: Data + EOF + CLOSE with NO ExitStatus (`agent request` refusal shape) -> clean Ok!
        {
            let stats = Arc::new(MockStats::default());
            let (handle, srv) = start_mock_ssh(stats, 65536).await;
            let mut ch = GuardedChannel::new(handle.channel_open_session().await.unwrap(), rt.clone());
            ch.exec_and_confirm("data_eof_close_no_status", Duration::from_secs(2), "refused").await.unwrap();
            let mut s = ch.into_exec_stream(ExecKind::Command);
            let mut reply = String::new();
            s.read_to_string(&mut reply).await.expect("EOF + CLOSE without optional exit status MUST succeed");
            assert_eq!(reply, "{\"ok\":false,\"error\":\"refused by policy\"}\n");
            srv.abort();
        }

        // 4. Rules 1 & 4: Data + EOF + ExitStatus(3) + CLOSE -> waits past EOF and reports status 3 + stderr!
        {
            let stats = Arc::new(MockStats::default());
            let (handle, srv) = start_mock_ssh(stats, 65536).await;
            let mut ch = GuardedChannel::new(handle.channel_open_session().await.unwrap(), rt.clone());
            ch.exec_and_confirm("data_eof_exit3", Duration::from_secs(2), "refused").await.unwrap();
            let mut s = ch.into_exec_stream(ExecKind::Command);
            let mut out = Vec::new();
            let err = s.read_to_end(&mut out).await.expect_err("exit 3 after EOF must fail").to_string();
            assert_eq!(out, b"partial-body");
            assert!(err.contains('3') && err.contains("failed late"), "got: {err}");
            srv.abort();
        }

        // 5. Rule 4: Data + EOF + transport lost before status or CLOSE -> UnexpectedEof!
        {
            let stats = Arc::new(MockStats::default());
            let (handle, srv) = start_mock_ssh(stats, 65536).await;
            let mut ch = GuardedChannel::new(handle.channel_open_session().await.unwrap(), rt);
            ch.exec_and_confirm("data_eof_disconnect_no_status", Duration::from_secs(2), "refused").await.unwrap();
            let mut s = ch.into_exec_stream(ExecKind::Command);
            let mut out = Vec::new();
            let err = s.read_to_end(&mut out).await.expect_err("disconnect after EOF without status or CLOSE must fail");
            assert_eq!(out, b"partial-eof");
            assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof, "got: {err:?}");
            srv.abort();
        }
    }

    /// Finding 4 & Test Finding 2: peer CLOSE wakes a zero-credit blocked writer
    /// AND fails subsequent writes with leftover credit when NOBODY is reading,
    /// and ALSO wakes a `Dial` writer after its reader finished on peer EOF!
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn final_f4_writer_fails_on_peer_close_without_reader_and_after_dial_eof() {
        let rt = tokio::runtime::Handle::current();

        // 1a. Writer alone (NO reader!) deterministically parked in TRUE zero-credit wait
        //     (`initial_window == 0`, verified still pending at 60 ms), then server sends
        //     CHANNEL_CLOSE at 120 ms -> writer wakes and fails with BrokenPipe within 1 s!
        {
            let stats = Arc::new(MockStats::default());
            let (handle, srv) = start_mock_ssh(stats, 0).await;
            let mut ch = GuardedChannel::new(handle.channel_open_session().await.unwrap(), rt.clone());
            ch.exec_and_confirm("close_after_zero_credit_wait", Duration::from_secs(2), "refused").await.unwrap();
            let stream = ch.into_exec_stream(ExecKind::Command);
            let (_unread, mut w) = tokio::io::split(stream);
            let mut write_task = tokio::spawn(async move { w.write_all(b"blocked-on-zero-credit").await });
            tokio::time::sleep(Duration::from_millis(60)).await;
            assert!(!write_task.is_finished(), "writer must genuinely be blocked in zero-credit wait before peer CLOSE");
            let err = tokio::time::timeout(Duration::from_secs(1), &mut write_task)
                .await
                .expect("zero-credit blocked writer with no reader must fail within 1s on peer CLOSE")
                .unwrap()
                .expect_err("write must error");
            assert_eq!(err.kind(), io::ErrorKind::BrokenPipe, "got: {err:?}");
            srv.abort();
        }

        // 1b. Write-before-read upload flooding > 256 KiB (> 100 WINDOW_ADJUST on a 4 KiB
        //     window) with NOBODY reading: peer CLOSE must NOT deadlock behind full
        //     WindowAdjusted queue and must fail writer within 1 s with BrokenPipe!
        {
            let stats = Arc::new(MockStats::default());
            let (handle, srv) = start_mock_ssh(stats.clone(), 4096).await;
            let mut ch = GuardedChannel::new(handle.channel_open_session().await.unwrap(), rt.clone());
            ch.exec_and_confirm("close_on_window_fill", Duration::from_secs(2), "refused").await.unwrap();
            let stream = ch.into_exec_stream(ExecKind::Command);
            let (_unread, mut w) = tokio::io::split(stream);
            let payload = vec![b'U'; 16 * 1024];
            let err = tokio::time::timeout(Duration::from_secs(1), async {
                loop {
                    w.write_all(&payload).await?;
                }
                #[allow(unreachable_code)]
                Ok::<(), io::Error>(())
            })
            .await
            .expect("upload with >100 WindowAdjusted and no reader must fail within 1s on peer CLOSE")
            .expect_err("write must error");
            assert_eq!(err.kind(), io::ErrorKind::BrokenPipe, "got: {err:?}");
            assert!(stats.received_bytes.load(Ordering::SeqCst) >= 256 * 1024);
            srv.abort();
        }

        // 2. Writer alone with UNUSED credit: peer sends CHANNEL_CLOSE -> next write
        //    immediately returns BrokenPipe (does not silently accept and discard).
        {
            let stats = Arc::new(MockStats::default());
            let (handle, srv) = start_mock_ssh(stats, 65536).await;
            let mut ch = GuardedChannel::new(handle.channel_open_session().await.unwrap(), rt.clone());
            ch.exec_and_confirm("close_immediately_with_credit", Duration::from_secs(2), "refused").await.unwrap();
            let stream = ch.into_exec_stream(ExecKind::Command);
            let (_unread, mut w) = tokio::io::split(stream);
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            while handle.open_channel_count().await.unwrap() != (0, 0) && std::time::Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            assert_eq!(handle.open_channel_count().await.unwrap(), (0, 0));
            let err = w.write(b"must-not-be-silently-dropped").await.expect_err("write after peer CLOSE must fail even with leftover credit");
            assert_eq!(err.kind(), io::ErrorKind::BrokenPipe);
            srv.abort();
        }

        // 3. Dial writer AFTER reader finished on peer EOF (dropping ChannelReadHalf):
        //    peer EOF alone stays writable, then peer CLOSE fails writer within 1 s!
        {
            let stats = Arc::new(MockStats::default());
            stats.no_window_refill.store(true, Ordering::SeqCst);
            let (handle, srv) = start_mock_ssh(stats.clone(), 4096).await;
            let mut ch = GuardedChannel::new(handle.channel_open_session().await.unwrap(), rt);
            ch.exec_and_confirm("dial_half_close_then_peer_close_on_fill", Duration::from_secs(2), "refused").await.unwrap();
            let mut dial = ch.into_exec_stream(ExecKind::Dial);
            let mut banner = Vec::new();
            dial.read_to_end(&mut banner).await.unwrap();
            assert_eq!(banner, b"hi");
            // Reader has finished and dropped ChannelReadHalf; peer EOF alone must stay writable!
            dial.write_all(b"still-writable").await.unwrap();
            let payload = vec![b'D'; 16 * 1024];
            let err = tokio::time::timeout(Duration::from_secs(1), async {
                loop {
                    dial.write_all(&payload).await?;
                }
                #[allow(unreachable_code)]
                Ok::<(), io::Error>(())
            })
            .await
            .expect("dial writer after reader EOF must fail within 1s on peer CLOSE")
            .expect_err("write must error");
            assert_eq!(err.kind(), io::ErrorKind::BrokenPipe, "got: {err:?}");
            assert!(stats.received_bytes.load(Ordering::SeqCst) >= 4096);
            srv.abort();
        }
    }

    /// Findings 9 (dup reply), 10 (no detached OPEN task, bounded EXEC ack,
    /// early data preserved), 11 (refused bootstrap / exit before close),
    /// 15a (unclaimed open confirmation & plain-thread `into_stream` drop),
    /// 15b (peer CLOSE with pending outbound data does not wedge sibling;
    /// closing-ID wraparound reservation), Test Finding 3 (public `exec_pty`
    /// & `open_channel_count` error), and `Ran.stderr` + `exec_bounded` early close.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn final_f9_f10_f11_f15_public_pty_and_bounded_stderr() {
        let rt = tokio::runtime::Handle::current();

        // 1. Test Finding 3: REAL public `pool.exec_pty` refused vs accepted!
        {
            let pool = SshSessionPool::default();
            let target = SshTarget::parse("ssh://tester@127.0.0.1:2201").unwrap();
            let stats = Arc::new(MockStats::default());
            stats.deny_pty.store(true, Ordering::SeqCst);
            let (_pooled, srv) = attach_mock_primary(&pool, &target, stats.clone(), 65536).await;

            let err = pool.exec_pty(&target, None, 80, 24, "pty_ok").await.err().expect("refused PTY must error").to_string();
            assert!(err.contains("pty"), "got: {err}");
            assert!(!stats.exec_called.load(Ordering::SeqCst), "public exec_pty MUST NOT send EXEC when PTY is refused");
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            while pool.open_channel_count(&target).await.unwrap() != (0, 0) && std::time::Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            assert_eq!(pool.open_channel_count(&target).await.unwrap(), (0, 0));

            stats.deny_pty.store(false, Ordering::SeqCst);
            let mut s = pool.exec_pty(&target, None, 80, 24, "pty_ok").await.expect("accepted PTY must succeed");
            let mut out = String::new();
            s.read_to_string(&mut out).await.unwrap();
            assert_eq!(out, "pty-ready\n");

            // Test Finding 3: `open_channel_count` returns Err on a dead handle (never false `(0, 0)`).
            _pooled.handle.disconnect(russh::Disconnect::ByApplication, "", "en").await.unwrap();
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            let mut saw_err = false;
            while std::time::Instant::now() < deadline {
                if pool.open_channel_count(&target).await.is_err() {
                    saw_err = true;
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            assert!(saw_err, "open_channel_count must propagate error from a disconnected handle");
            srv.abort();
        }

        // 2. Extra request: `Ran.stderr` separates identical stdout/stderr lines
        //    and `exec_bounded` closes the channel immediately on truncation!
        {
            let pool = SshSessionPool::default();
            let target = SshTarget::parse("ssh://tester@127.0.0.1:2202").unwrap();
            let stats = Arc::new(MockStats::default());
            let (_pooled, srv) = attach_mock_primary(&pool, &target, stats.clone(), 65536).await;

            let ran = pool.exec_bounded(&target, None, "stdout_and_stderr_identical", 4096, 1024, &mut |_| {}).await.unwrap();
            assert!(!ran.truncated);
            assert_eq!(ran.status, Some(0));
            assert_eq!(ran.stdout, "dup\nonly-out\n");
            assert_eq!(ran.stderr, "dup\nonly-err\n");

            let t0 = std::time::Instant::now();
            let trunc = pool.exec_bounded(&target, None, "flood_bounded", 64, 64, &mut |_| {}).await.unwrap();
            assert!(trunc.truncated, "bounded flood must mark truncated");
            assert!(t0.elapsed() < Duration::from_secs(1), "exec_bounded must close channel immediately on bound hit, took {:?}", t0.elapsed());
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            while pool.open_channel_count(&target).await.unwrap() != (0, 0) && std::time::Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            assert_eq!(pool.open_channel_count(&target).await.unwrap(), (0, 0));
            srv.abort();
        }

        // 3. Finding 10: timed-out OPEN leaves ZERO detached task holding `PooledHandle`,
        //    unanswered EXEC times out, and early data before CHANNEL_SUCCESS is preserved!
        {
            let pool = SshSessionPool::default();
            let target = SshTarget::parse("ssh://tester@127.0.0.1:2203").unwrap();
            let stats = Arc::new(MockStats::default());
            let (pooled, srv) = attach_mock_primary(&pool, &target, stats.clone(), 65536).await;
            let tp = pool.target_pool(&target);

            // 3a. Server never confirms OPEN -> timeout leaves Arc strong count unchanged (zero detached task!).
            stats.never_confirm.store(true, Ordering::SeqCst);
            let before_refs = Arc::strong_count(&pooled);
            let res = SshSessionPool::open_session_guarded(&tp, &pooled, Duration::from_millis(80)).await;
            assert!(matches!(res, Err(SessionOpenErr::Timeout)));
            assert_eq!(Arc::strong_count(&pooled), before_refs, "timed-out OPEN must leave zero detached tasks holding PooledHandle");

            // 3b. Server confirms OPEN, never replies to EXEC -> `exec_and_confirm` times out and drops cleanly.
            stats.never_confirm.store(false, Ordering::SeqCst);
            let mut ch = SshSessionPool::open_session_guarded(&tp, &pooled, Duration::from_secs(2)).await.ok().unwrap();
            let err = ch.exec_and_confirm("hang_exec", Duration::from_millis(120), "refused").await.unwrap_err();
            assert!(matches!(err, RexError::Timeout), "unanswered EXEC must time out, got: {err:?}");
            drop(ch);

            // 3c. Early DATA arriving before CHANNEL_SUCCESS is preserved verbatim!
            let mut s = pool.exec_io(&target, None, "early_data_before_success").await.unwrap();
            let mut out = String::new();
            s.read_to_string(&mut out).await.unwrap();
            assert_eq!(out, "early-after");
            srv.abort();
        }

        // 4. Finding 11 & Finding 9 (no duplicate Reply::ChannelOpenFailure):
        //    1-slot server refuses exec / sends ExitStatus without Close -> guard stays
        //    armed, frees the sole slot, and repeated ChannelOpenFailure leaves 0 queued replies!
        {
            let stats = Arc::new(MockStats::default());
            stats.max_sessions.store(1, Ordering::SeqCst);
            let (handle, srv) = start_mock_ssh(stats.clone(), 65536).await;

            // 4a. Refused exec (`Failure`) leaves close guard armed -> slot freed.
            let mut ch = GuardedChannel::new(handle.channel_open_session().await.unwrap(), rt.clone());
            assert!(ch.exec_and_confirm("refuse_exec_leave_open", Duration::from_secs(2), "refused").await.is_err());
            drop(ch);
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            while handle.open_channel_count().await.unwrap() != (0, 0) && std::time::Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            assert_eq!(handle.open_channel_count().await.unwrap(), (0, 0));

            // 4b. ExitStatus(127) without Close in `read_announce_line` leaves guard armed -> slot freed!
            let mut ch2 = GuardedChannel::new(handle.channel_open_session().await.unwrap(), rt.clone());
            ch2.exec_and_confirm("exit_status_no_close", Duration::from_secs(2), "refused").await.unwrap();
            assert!(read_announce_line(&mut ch2).await.is_err());
            drop(ch2);
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            while handle.open_channel_count().await.unwrap() != (0, 0) && std::time::Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            assert_eq!(handle.open_channel_count().await.unwrap(), (0, 0));

            // 4c. Finding 9: 10 refused opens leave 0 unread Reply::ChannelOpenFailure on Handle::receiver!
            let _held = GuardedChannel::new(handle.channel_open_session().await.unwrap(), rt.clone());
            for _ in 0..10 {
                assert!(matches!(handle.channel_open_session().await, Err(russh::Error::ChannelOpenFailure(_))));
            }
            assert_eq!(handle.pending_reply_count_for_test(), 0, "CHANNEL_OPEN_FAILURE must not leak unread replies");
            drop(_held);
            srv.abort();
        }

        // 5. Finding 15a: plain-thread `into_stream()` drop never panics AND
        //    dropping `channel_open_session` AFTER confirmation was already queued
        //    closes the unclaimed channel synchronously via `UnclaimedOpenGuard`!
        {
            let stats = Arc::new(MockStats::default());
            stats.max_sessions.store(1, Ordering::SeqCst);
            let (handle, srv) = start_mock_ssh(stats.clone(), 65536).await;

            // 5a. Plain OS thread drop of `into_stream()` must NOT panic and must close the channel.
            let ch = handle.channel_open_session().await.unwrap();
            let stream = ch.into_stream();
            std::thread::spawn(move || drop(stream)).join().expect("plain-thread into_stream drop must not panic");
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            while (stats.close_count.load(Ordering::SeqCst) < 1 || handle.open_channel_count().await.unwrap() != (0, 0)) && std::time::Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            assert_eq!(handle.open_channel_count().await.unwrap(), (0, 0));

            // 5b. Unclaimed open confirmation: poll future ONCE to send Msg::ChannelOpenSession,
            //     wait for server to confirm (so ChannelMsg::Open sits in receiver unread),
            //     then DROP the future without polling again -> UnclaimedOpenGuard closes it!
            {
                let mut open_fut = Box::pin(handle.channel_open_session());
                let waker = futures_util::task::noop_waker();
                let mut cx = std::task::Context::from_waker(&waker);
                assert!(open_fut.as_mut().poll(&mut cx).is_pending(), "first poll submits open and awaits confirmation");
                // Wait until server confirmed (active_sessions == 1) and confirmation reached client driver.
                let wait_until = std::time::Instant::now() + Duration::from_secs(2);
                while stats.active_sessions.load(Ordering::SeqCst) < 1 && std::time::Instant::now() < wait_until {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                tokio::time::sleep(Duration::from_millis(40)).await;
                // Drop WITHOUT polling again — confirmation is already queued inside `receiver`!
                drop(open_fut);
            }
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            while (stats.close_count.load(Ordering::SeqCst) < 2 || handle.open_channel_count().await.unwrap() != (0, 0)) && std::time::Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            assert_eq!(stats.close_count.load(Ordering::SeqCst), 2, "UnclaimedOpenGuard must close already-confirmed unclaimed channel");
            assert_eq!(handle.open_channel_count().await.unwrap(), (0, 0));
            // Single slot is free again!
            let next = GuardedChannel::new(handle.channel_open_session().await.expect("1-slot server must admit next open"), rt.clone());
            drop(next);
            srv.abort();
        }

        // 6. Finding 15b: peer CLOSE on a 0-window channel with genuine `pending_data`
        //    MUST discard pending data and NEVER freeze sibling channels; `rekey_soon`
        //    prunes closed channels; and near-wraparound allocation skips closing IDs!
        {
            let stats = Arc::new(MockStats::default());
            let (handle, srv) = start_mock_ssh(stats.clone(), 0).await;

            // Open channel A with 0 peer window, enqueue raw data directly into
            // `enc.channels[A].pending_data` (making `has_any_pending_data() == true`),
            // and let server send CHANNEL_CLOSE without any WINDOW_ADJUST!
            let mut ch_a = GuardedChannel::new(handle.channel_open_session().await.unwrap(), rt.clone());
            ch_a.exec_and_confirm("close_after_zero_credit_wait", Duration::from_secs(2), "refused").await.unwrap();
            let id_a = ch_a.write_half().id();
            handle.send_raw_channel_data_for_test(id_a, bytes::Bytes::from_static(b"stuck-in-pending-data")).await.unwrap();
            // Open sibling channel B (control traffic) while channel A has pending_data.
            let ch_b = GuardedChannel::new(handle.channel_open_session().await.unwrap(), rt.clone());
            // Wait for server's CHANNEL_CLOSE on A -> `close_on_peer_close` discards pending_data and purges A.
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            while handle.open_channel_count().await.unwrap() != (1, 1) && std::time::Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            assert_eq!(handle.open_channel_count().await.unwrap(), (1, 1), "peer CLOSE must discard pending_data and remove channel A");
            drop(ch_a);

            // Trigger a rekey and open a new channel C right after to confirm the
            // connection is NOT gated by A's discarded `pending_data` and rekey
            // leaves `(1, 1)` (just ch_b) with zero leaked entries!
            handle.rekey_soon().await.unwrap();
            let mut ch_c =
                tokio::time::timeout(Duration::from_secs(2), handle.channel_open_session()).await.expect("sibling channel open must not be wedged by closed channel's pending_data").unwrap();
            ch_c.exec(true, "pty_ok").await.unwrap();
            let mut saw_pty = false;
            while let Some(msg) = ch_c.wait().await {
                if let russh::ChannelMsg::Data { data } = msg
                    && data.as_ref() == b"pty-ready\n"
                {
                    saw_pty = true;
                }
            }
            assert!(saw_pty, "post-rekey sibling channel must complete cleanly");
            drop(ch_b);
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            while handle.open_channel_count().await.unwrap() != (0, 0) && std::time::Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            assert_eq!(handle.open_channel_count().await.unwrap(), (0, 0));

            // Near-wraparound check: allocate u32::MAX, close it, wrap around, verify next open succeeds cleanly.
            handle.set_last_channel_id_for_test(u32::MAX - 1).await.unwrap();
            let c1 = handle.channel_open_session().await.unwrap();
            assert_eq!(c1.id().to_string(), u32::MAX.to_string());
            c1.close().await.unwrap();
            let c2 = handle.channel_open_session().await.unwrap();
            assert_ne!(c2.id(), c1.id(), "allocator wraps cleanly past u32::MAX");
            c2.close().await.unwrap();
            srv.abort();
        }
    }

    /// Finding 12: `set_helper(None)` and changing helper ID immediately drop
    /// `Link::Up` (closing its SSH keeper) and abort `Link::Starting`; `close`
    /// and `invalidate` clear `Link::Starting`; and shutting down a bootstrap's
    /// opener runtime clears `Link::Starting` via `LinkStartGuard`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn final_f12_helper_change_and_cancelled_bootstrap_clear_link_state() {
        let stats = Arc::new(MockStats::default());
        let (ssh_h, srv) = start_mock_ssh(stats.clone(), 65536).await;
        let rt = tokio::runtime::Handle::current();

        let (cert_der, key_der, print) = crate::quic::make_test_quic_cert();
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let mut tls = rustls::ServerConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS13])
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![rustls::pki_types::CertificateDer::from(cert_der)], rustls::pki_types::PrivateKeyDer::Pkcs8(key_der.into()))
            .unwrap();
        tls.alpn_protocols = vec![olo_rex_link::quic_wire::ALPN.to_vec()];
        let scfg = quinn::ServerConfig::with_crypto(Arc::new(quinn::crypto::rustls::QuicServerConfig::try_from(tls).unwrap()));
        let qsrv = quinn::Endpoint::server(scfg, (std::net::Ipv4Addr::LOCALHOST, 0).into()).unwrap();
        let qaddr = qsrv.local_addr().unwrap();
        let qtask = tokio::spawn(async move {
            let mut _conns = Vec::new();
            while let Some(inc) = qsrv.accept().await {
                if let Ok(c) = inc.await {
                    _conns.push(c);
                }
            }
        });

        let pool = Arc::new(SshSessionPool::default());
        let target = SshTarget::parse("ssh://tester@127.0.0.1:2212").unwrap();
        let token = crate::quic::new_token().unwrap();

        // 1. `set_helper(None)` immediately drops Link::Up -> `linked()` false AND keeper closes!
        pool.set_helper(Some("h1"));
        let qlink = Arc::new(crate::quic::QuicLink::connect(qaddr, &print, token).await.unwrap());
        let keeper = GuardedChannel::new(ssh_h.channel_open_session().await.unwrap(), rt.clone());
        let tgen = pool.target_pool(&target).generation;
        pool.links.lock().unwrap().insert(target.clone(), Link::Up { link: qlink, keeper, helper_id: "h1".into(), target_gen: tgen, screens_supported: Arc::new(AtomicBool::new(true)) });
        assert!(pool.linked(&target).await);
        pool.set_helper(None);
        assert!(!pool.linked(&target).await, "set_helper(None) must immediately clear linked()");
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while ssh_h.open_channel_count().await.unwrap() != (0, 0) && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(ssh_h.open_channel_count().await.unwrap(), (0, 0), "set_helper(None) must close keeper channel immediately");

        // 2. Changing helper ID ("h1" -> "h2") immediately evicts "h1" Link::Up and closes keeper!
        pool.set_helper(Some("h1"));
        let qlink2 = Arc::new(crate::quic::QuicLink::connect(qaddr, &print, token).await.unwrap());
        let keeper2 = GuardedChannel::new(ssh_h.channel_open_session().await.unwrap(), rt.clone());
        pool.links.lock().unwrap().insert(target.clone(), Link::Up { link: qlink2, keeper: keeper2, helper_id: "h1".into(), target_gen: tgen, screens_supported: Arc::new(AtomicBool::new(true)) });
        assert!(pool.linked(&target).await);
        pool.set_helper(Some("h2"));
        assert!(!pool.linked(&target).await, "changing helper ID must evict old Link::Up immediately");
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while ssh_h.open_channel_count().await.unwrap() != (0, 0) && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(ssh_h.open_channel_count().await.unwrap(), (0, 0));

        // 3. `close` and `invalidate` clear `Link::Starting` and abort its task!
        let (done_tx, _) = tokio::sync::watch::channel(false);
        let j = tokio::spawn(std::future::pending::<()>());
        let abort = j.abort_handle();
        pool.links.lock().unwrap().insert(target.clone(), Link::Starting { generation: 99, target_gen: tgen, helper_id: "h2".into(), abort, done: done_tx });
        pool.close(&target).await;
        assert!(pool.links.lock().unwrap().get(&target).is_none(), "close() must clear Link::Starting");
        assert!(j.await.unwrap_err().is_cancelled(), "close() must abort Starting task");

        // 4. Opener runtime shutting down mid-bootstrap clears `Link::Starting` via `LinkStartGuard`!
        let pool_c = pool.clone();
        let target_c = target.clone();
        tokio::task::spawn_blocking(move || {
            let rt_b = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            let link_gen = pool_c.alloc_gen();
            let (done_tx, _) = tokio::sync::watch::channel(false);
            let p2 = pool_c.clone();
            let t2 = target_c.clone();
            let d2 = done_tx.clone();
            let guard = LinkStartGuard { pool: p2, target: t2, link_gen, done: d2, finished: false };
            let j = rt_b.spawn(async move {
                let _guard = guard;
                std::future::pending::<()>().await;
            });
            pool_c.links.lock().unwrap().insert(target_c.clone(), Link::Starting { generation: link_gen, target_gen: 1, helper_id: "h2".into(), abort: j.abort_handle(), done: done_tx });
            // Shut down Runtime B while the task is pending!
            drop(rt_b);
        })
        .await
        .unwrap();
        assert!(pool.links.lock().unwrap().get(&target).is_none(), "runtime shutdown mid-bootstrap must clear Link::Starting");

        qtask.abort();
        srv.abort();
    }

    /// Finding 13: `pub fn screen_command` parses `"proto"` and `"commands"`
    /// array with arbitrary JSON whitespace, rejects verbs appearing only in
    /// `"capabilities"`, prefers a `--protocol`-capable helper over a newer
    /// help-only build for `--protocol` and one-shot commands, and preserves
    /// legacy `--help` fallback when only a help-only build exists.
    #[test]
    fn final_f13_screen_command_json_array_and_two_tier_selection() {
        use std::os::unix::fs::PermissionsExt as _;
        use std::process::{Command, Stdio};

        let dir = crate::quic::TestTempDir::new("olo-f13");
        let home = dir.path().join("home");
        let there = home.join(crate::helper::DIR);
        std::fs::create_dir_all(&there).unwrap();
        let machine = String::from_utf8(Command::new("uname").arg("-m").output().unwrap().stdout).unwrap().trim().to_owned();
        let file = |id: &str| there.join(format!("olo-rex-link-{id}-{machine}"));
        let script = |path: &std::path::Path, body: &str| {
            std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        };
        let run = |helper: Option<&str>, args: &[&str]| {
            let a: Vec<String> = args.iter().map(|s| (*s).to_owned()).collect();
            let out = Command::new("sh").arg("-c").arg(screen_command(helper, &a)).env("HOME", &home).stdin(Stdio::null()).output().unwrap();
            (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).trim().to_owned())
        };

        // 1. JSON with whitespace before `:` and `"export"` ONLY in `capabilities`:
        //    `follow` MUST match, `export` MUST be rejected (exit 69)!
        script(
            &file("ws"),
            "if [ \"$1\" = screen ] && [ \"$2\" = --protocol ]; then printf '{ \"proto\" : 1 , \"capabilities\" : [ \"export\" ] , \"commands\" : [ \"follow\" ] }\\n'; exit 0; fi; echo \"ws:$*\"",
        );
        assert_eq!(run(Some("ws"), &["follow"]), (0, "ws:screen follow".to_owned()));
        assert_eq!(run(Some("ws"), &["export", "qa"]), (69, String::new()));

        // 2. Newer help-only build beside older `--protocol`-capable build:
        //    `--protocol` and one-shot `list` MUST pick the `--protocol`-capable build!
        script(
            &file("helponly"),
            "if [ \"$1\" = screen ] && [ \"$2\" = --help ]; then echo 'usage: olo screen list'; exit 0; fi; if [ \"$1\" = screen ] && [ \"$2\" = --protocol ]; then exit 1; fi; echo \"helponly:$*\"",
        );
        let now = std::time::SystemTime::now();
        std::fs::File::options().write(true).open(file("ws")).unwrap().set_modified(now - Duration::from_secs(3600)).unwrap();
        std::fs::File::options().write(true).open(file("helponly")).unwrap().set_modified(now).unwrap();
        assert_eq!(run(None, &["--protocol"]).0, 0, "older --protocol helper must beat newer help-only helper");
        assert_eq!(run(None, &["list"]), (0, "ws:screen list".to_owned()));

        // 3. Host with ONLY a legacy help-only build: `--protocol` runs it (exits 1 -> proto 0),
        //    one-shot `list` works, legacy `history qa --wire` works, streaming `follow` exits 69!
        std::fs::remove_file(file("ws")).unwrap();
        assert_eq!(run(None, &["--protocol"]).0, 1, "legacy-only host runs legacy helper for --protocol");
        assert_eq!(run(None, &["list"]), (0, "helponly:screen list".to_owned()));
        assert_eq!(run(None, &["history", "qa", "--wire"]), (0, "helponly:screen history qa --wire".to_owned()));
        assert_eq!(run(None, &["follow"]), (69, String::new()));
    }

    struct MockChannelRx(tokio::sync::mpsc::UnboundedReceiver<russh::ChannelMsg>);
    impl ChannelRx for MockChannelRx {
        async fn wait(&mut self) -> Option<russh::ChannelMsg> {
            self.0.recv().await
        }
    }

    struct MockChannelTx {
        closed: Arc<std::sync::atomic::AtomicBool>,
        armed: bool,
        block_exec: bool,
        block_data: bool,
        eof_err: bool,
        eof_notify: Option<Arc<tokio::sync::Notify>>,
    }
    impl Drop for MockChannelTx {
        fn drop(&mut self) {
            if self.armed {
                self.closed.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        }
    }
    impl ChannelTx for MockChannelTx {
        async fn exec(&self, _command: &str) -> Result<(), String> {
            if self.block_exec {
                std::future::pending::<()>().await;
            }
            Ok(())
        }
        async fn data(&self, _chunk: &[u8]) -> Result<(), String> {
            if self.block_data {
                std::future::pending::<()>().await;
            }
            Ok(())
        }
        async fn eof(&self) -> Result<(), String> {
            if let Some(n) = &self.eof_notify {
                n.notify_one();
            }
            if self.eof_err {
                return Err("eof failed".into());
            }
            Ok(())
        }
        fn disarm_close(&mut self) {
            self.armed = false;
        }
    }

    #[tokio::test]
    async fn put_stream_early_exit_with_full_window_and_delayed_close_returns_helper_error() {
        use russh::ChannelMsg;
        let (msg_tx, msg_rx) = tokio::sync::mpsc::unbounded_channel();
        let (chunk_tx, chunk_rx) = tokio::sync::mpsc::channel(4);
        chunk_tx.send(vec![b'x'; 64 * 1024]).await.unwrap();
        let closed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let eof_sent = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let tx = MockChannelTx { closed: closed.clone(), armed: true, block_exec: false, block_data: true, eof_err: false, eof_notify: None };

        // Simulate helper rejecting early with JSON error + ExitStatus(1), and
        // delaying Close forever (e.g. lingering child holds stdout).
        msg_tx.send(ChannelMsg::Data { data: bytes::Bytes::from_static(b"{\"ok\":false,\"error\":\"disk full\"}\n") }).unwrap();
        msg_tx.send(ChannelMsg::ExitStatus { exit_status: 1 }).unwrap();

        let ran = put_stream_driver(
            "ssh://test",
            MockChannelRx(msg_rx),
            tx,
            "fs write",
            chunk_rx,
            Duration::from_secs(5),
            Duration::from_secs(5),
            Duration::from_millis(50),
            Some(PUT_STREAM_MAX_OUTPUT),
            Some(eof_sent.clone()),
        )
        .await
        .unwrap();
        assert_eq!(ran.status, Some(1));
        assert!(ran.output.contains("disk full"), "got: {}", ran.output);
        assert!(closed.load(std::sync::atomic::Ordering::SeqCst), "delayed-close channel must be closed on drop");
        assert!(!eof_sent.load(std::sync::atomic::Ordering::SeqCst), "eof_sent must remain false when channel exited before tx.eof()");
    }

    #[tokio::test]
    async fn put_stream_refused_exec_fails_immediately_and_closes_channel() {
        use russh::ChannelMsg;
        let (msg_tx, msg_rx) = tokio::sync::mpsc::unbounded_channel();
        let (chunk_tx, chunk_rx) = tokio::sync::mpsc::channel(4);
        chunk_tx.send(vec![b'a'; 1024]).await.unwrap();
        let closed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let tx = MockChannelTx { closed: closed.clone(), armed: true, block_exec: false, block_data: true, eof_err: false, eof_notify: None };

        msg_tx.send(ChannelMsg::Failure).unwrap();
        let began = std::time::Instant::now();
        let err = put_stream_driver(
            "ssh://test",
            MockChannelRx(msg_rx),
            tx,
            "fs write",
            chunk_rx,
            Duration::from_secs(10),
            Duration::from_secs(10),
            Duration::from_millis(50),
            Some(PUT_STREAM_MAX_OUTPUT),
            None,
        )
        .await
        .unwrap_err();
        assert!(began.elapsed() < Duration::from_secs(2), "must fail immediately on ChannelMsg::Failure");
        assert!(err.to_string().contains("refused"), "{err}");
        assert!(closed.load(std::sync::atomic::Ordering::SeqCst), "refused exec must close channel on drop");
    }

    #[tokio::test]
    async fn put_stream_unexpected_disconnect_errors_instead_of_empty_ok() {
        let (msg_tx, msg_rx) = tokio::sync::mpsc::unbounded_channel();
        let (chunk_tx, chunk_rx) = tokio::sync::mpsc::channel(4);
        chunk_tx.send(vec![b'a'; 1024]).await.unwrap();
        let closed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let tx = MockChannelTx { closed: closed.clone(), armed: true, block_exec: false, block_data: true, eof_err: false, eof_notify: None };

        // Drop sender without ChannelMsg::Close (unexpected transport drop).
        drop(msg_tx);
        let err = put_stream_driver(
            "ssh://test",
            MockChannelRx(msg_rx),
            tx,
            "fs write",
            chunk_rx,
            Duration::from_secs(5),
            Duration::from_secs(5),
            Duration::from_millis(50),
            Some(PUT_STREAM_MAX_OUTPUT),
            None,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("closed unexpectedly"), "{err}");
    }

    #[tokio::test]
    async fn put_stream_enforces_combined_stdout_stderr_cap_and_allows_unbounded_put() {
        use russh::ChannelMsg;
        // 1. Just at cap succeeds and sets eof_sent=true after tx.eof() succeeds.
        //    Wait for `eof_notify` before replying ExitStatus+Close, matching a real remote helper.
        let (msg_tx, msg_rx) = tokio::sync::mpsc::unbounded_channel();
        let (_chunk_tx, chunk_rx) = tokio::sync::mpsc::channel(1);
        drop(_chunk_tx);
        let closed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let eof_sent = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let eof_notify = Arc::new(tokio::sync::Notify::new());
        let eof_wait = eof_notify.clone();
        let tx = MockChannelTx { closed: closed.clone(), armed: true, block_exec: false, block_data: false, eof_err: false, eof_notify: Some(eof_notify) };
        tokio::spawn(async move {
            eof_wait.notified().await;
            let _ = msg_tx.send(ChannelMsg::Data { data: bytes::Bytes::from(vec![b'a'; 60]) });
            let _ = msg_tx.send(ChannelMsg::ExtendedData { data: bytes::Bytes::from(vec![b'b'; 40]), ext: 1 });
            let _ = msg_tx.send(ChannelMsg::ExitStatus { exit_status: 0 });
            let _ = msg_tx.send(ChannelMsg::Close);
        });
        let ran =
            put_stream_driver("ssh://test", MockChannelRx(msg_rx), tx, "cmd", chunk_rx, Duration::from_secs(2), Duration::from_secs(2), Duration::from_millis(50), Some(100), Some(eof_sent.clone()))
                .await
                .unwrap();
        assert_eq!(ran.output.len(), 100);
        assert!(!closed.load(std::sync::atomic::Ordering::SeqCst), "clean close disarms guard");
        assert!(eof_sent.load(std::sync::atomic::Ordering::SeqCst), "eof_sent must be true after tx.eof() succeeds");

        // 1b. Failed tx.eof() must NOT set eof_sent.
        let (msg_tx, msg_rx) = tokio::sync::mpsc::unbounded_channel();
        let (_chunk_tx, chunk_rx) = tokio::sync::mpsc::channel(1);
        drop(_chunk_tx);
        let closed_eof = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let eof_sent_fail = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let tx_eof = MockChannelTx { closed: closed_eof.clone(), armed: true, block_exec: false, block_data: false, eof_err: true, eof_notify: None };
        msg_tx.send(ChannelMsg::ExitStatus { exit_status: 1 }).unwrap();
        msg_tx.send(ChannelMsg::Close).unwrap();
        let _ = put_stream_driver(
            "ssh://test",
            MockChannelRx(msg_rx),
            tx_eof,
            "cmd",
            chunk_rx,
            Duration::from_secs(2),
            Duration::from_secs(2),
            Duration::from_millis(50),
            Some(100),
            Some(eof_sent_fail.clone()),
        )
        .await;
        assert!(!eof_sent_fail.load(std::sync::atomic::Ordering::SeqCst), "eof_sent must remain false when tx.eof() fails");

        // 2. 1 byte over cap fails and triggers close guard.
        let (msg_tx, msg_rx) = tokio::sync::mpsc::unbounded_channel();
        let (_chunk_tx, chunk_rx) = tokio::sync::mpsc::channel(1);
        drop(_chunk_tx);
        let closed2 = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let tx2 = MockChannelTx { closed: closed2.clone(), armed: true, block_exec: false, block_data: false, eof_err: false, eof_notify: None };
        msg_tx.send(ChannelMsg::Data { data: bytes::Bytes::from(vec![b'a'; 60]) }).unwrap();
        msg_tx.send(ChannelMsg::ExtendedData { data: bytes::Bytes::from(vec![b'b'; 41]), ext: 1 }).unwrap();
        let err =
            put_stream_driver("ssh://test", MockChannelRx(msg_rx), tx2, "cmd", chunk_rx, Duration::from_secs(2), Duration::from_secs(2), Duration::from_millis(50), Some(100), None).await.unwrap_err();
        assert!(err.to_string().contains("exceeded 100 byte limit"), "{err}");
        assert!(closed2.load(std::sync::atomic::Ordering::SeqCst), "overflow must close channel");

        // 3. Unbounded `put` (max_output = None) accepts > 1 MiB replies (e.g. carried-settings comparisons).
        let (msg_tx, msg_rx) = tokio::sync::mpsc::unbounded_channel();
        let (_chunk_tx, chunk_rx) = tokio::sync::mpsc::channel(1);
        drop(_chunk_tx);
        let closed3 = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let tx3 = MockChannelTx { closed: closed3.clone(), armed: true, block_exec: false, block_data: false, eof_err: false, eof_notify: None };
        let big = PUT_STREAM_MAX_OUTPUT + 64 * 1024;
        msg_tx.send(ChannelMsg::Data { data: bytes::Bytes::from(vec![b'z'; big]) }).unwrap();
        msg_tx.send(ChannelMsg::ExitStatus { exit_status: 0 }).unwrap();
        msg_tx.send(ChannelMsg::Close).unwrap();
        let ran = put_stream_driver("ssh://test", MockChannelRx(msg_rx), tx3, "cmd", chunk_rx, Duration::from_secs(2), Duration::from_secs(2), Duration::from_millis(50), None, None).await.unwrap();
        assert_eq!(ran.output.len(), big);
        assert!(!closed3.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test]
    async fn put_stream_drain_renews_pre_exit_only_and_fixes_post_exit_deadline() {
        use russh::ChannelMsg;
        // 1. Pre-exit drain receives 5 progress lines 60 ms apart (300 ms total) with a 180 ms
        //    idle timeout: total > idle timeout proves pre-exit renewal actually fires, while
        //    60 ms << 180 ms gives 3x scheduling headroom.
        let (msg_tx, msg_rx) = tokio::sync::mpsc::unbounded_channel();
        let (_chunk_tx, chunk_rx) = tokio::sync::mpsc::channel(1);
        drop(_chunk_tx);
        let closed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let tx = MockChannelTx { closed: closed.clone(), armed: true, block_exec: false, block_data: false, eof_err: false, eof_notify: None };
        tokio::spawn(async move {
            for i in 0..5 {
                tokio::time::sleep(Duration::from_millis(60)).await;
                let _ = msg_tx.send(ChannelMsg::Data { data: bytes::Bytes::from(format!("step {i}\n")) });
            }
            tokio::time::sleep(Duration::from_millis(40)).await;
            let _ = msg_tx.send(ChannelMsg::ExitStatus { exit_status: 0 });
            let _ = msg_tx.send(ChannelMsg::Close);
        });
        let ran =
            put_stream_driver("ssh://test", MockChannelRx(msg_rx), tx, "cmd", chunk_rx, Duration::from_secs(2), Duration::from_millis(180), Duration::from_secs(2), Some(1024), None).await.unwrap();
        assert_eq!(ran.status, Some(0));
        assert!(ran.output.contains("step 4"), "{:?}", ran.output);

        // 2. Post-exit drain deadline is FIXED (120 ms): a lingering descendant writing
        //    every 40 ms for 800 ms must NOT keep renewing past the 120 ms post-exit deadline.
        let (msg_tx, msg_rx) = tokio::sync::mpsc::unbounded_channel();
        let (_chunk_tx, chunk_rx) = tokio::sync::mpsc::channel(1);
        drop(_chunk_tx);
        let closed_post = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let tx_post = MockChannelTx { closed: closed_post.clone(), armed: true, block_exec: false, block_data: false, eof_err: false, eof_notify: None };
        tokio::spawn(async move {
            let _ = msg_tx.send(ChannelMsg::ExitStatus { exit_status: 0 });
            for i in 0..20 {
                tokio::time::sleep(Duration::from_millis(40)).await;
                if msg_tx.send(ChannelMsg::Data { data: bytes::Bytes::from(format!("child {i}\n")) }).is_err() {
                    break;
                }
            }
        });
        let started = tokio::time::Instant::now();
        let ran_post = put_stream_driver("ssh://test", MockChannelRx(msg_rx), tx_post, "cmd", chunk_rx, Duration::from_secs(2), Duration::from_secs(10), Duration::from_millis(120), Some(4096), None)
            .await
            .unwrap();
        let elapsed = started.elapsed();
        assert_eq!(ran_post.status, Some(0));
        assert!(elapsed < Duration::from_millis(550), "post-exit drain renewed past fixed 120 ms deadline: {elapsed:?}");
        assert!(closed_post.load(std::sync::atomic::Ordering::SeqCst), "timed-out post-exit channel must close on drop");

        // 3. Dropping put_stream while exec().await is pending must close the channel immediately.
        let (_msg_tx, msg_rx) = tokio::sync::mpsc::unbounded_channel();
        let (_chunk_tx, chunk_rx) = tokio::sync::mpsc::channel(1);
        let closed_exec = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let tx_exec = MockChannelTx { closed: closed_exec.clone(), armed: true, block_exec: true, block_data: false, eof_err: false, eof_notify: None };
        let res = tokio::time::timeout(
            Duration::from_millis(20),
            put_stream_driver("ssh://test", MockChannelRx(msg_rx), tx_exec, "cmd", chunk_rx, Duration::from_secs(2), Duration::from_secs(2), Duration::from_millis(50), Some(1024), None),
        )
        .await;
        assert!(res.is_err(), "blocked exec must time out");
        assert!(closed_exec.load(std::sync::atomic::Ordering::SeqCst), "guard armed before exec must close channel on cancellation");
    }

    #[tokio::test]
    async fn exec_stdout_allows_empty_zero_exit_and_rejects_nonzero_or_disconnect() {
        use russh::ChannelMsg;
        // 1. Empty file / `--max 0`: ExitStatus(0) + Close with zero Data yields Ok(None).
        let (msg_tx, msg_rx) = tokio::sync::mpsc::unbounded_channel();
        msg_tx.send(ChannelMsg::ExitStatus { exit_status: 0 }).unwrap();
        msg_tx.send(ChannelMsg::Close).unwrap();
        let mut rx = MockChannelRx(msg_rx);
        let closed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut tx = MockChannelTx { closed: closed.clone(), armed: true, block_exec: false, block_data: false, eof_err: false, eof_notify: None };
        let (mut status, mut sig, mut stderr, mut done) = (None, None, Vec::new(), false);
        let first = exec_stdout_next("ssh://test", &mut rx, &mut tx, &mut status, &mut sig, &mut stderr, &mut done).await.unwrap();
        assert!(first.is_none(), "zero-byte exit-0 stream must finish cleanly");
        assert!(!closed.load(std::sync::atomic::Ordering::SeqCst), "clean close disarms guard");

        // 2. Non-zero exit with stderr surfaces stderr error.
        let (msg_tx, msg_rx) = tokio::sync::mpsc::unbounded_channel();
        msg_tx.send(ChannelMsg::ExtendedData { data: bytes::Bytes::from_static(b"permission denied\n"), ext: 1 }).unwrap();
        msg_tx.send(ChannelMsg::ExitStatus { exit_status: 1 }).unwrap();
        msg_tx.send(ChannelMsg::Close).unwrap();
        let mut rx = MockChannelRx(msg_rx);
        let mut tx = MockChannelTx { closed: Arc::new(std::sync::atomic::AtomicBool::new(false)), armed: true, block_exec: false, block_data: false, eof_err: false, eof_notify: None };
        let (mut status, mut sig, mut stderr, mut done) = (None, None, Vec::new(), false);
        let err = exec_stdout_next("ssh://test", &mut rx, &mut tx, &mut status, &mut sig, &mut stderr, &mut done).await.unwrap_err();
        assert!(err.to_string().contains("permission denied"), "{err}");

        // 3. Unexpected disconnect (None without Close) errors.
        let (msg_tx, msg_rx) = tokio::sync::mpsc::unbounded_channel();
        drop(msg_tx);
        let mut rx = MockChannelRx(msg_rx);
        let mut tx = MockChannelTx { closed: Arc::new(std::sync::atomic::AtomicBool::new(false)), armed: true, block_exec: false, block_data: false, eof_err: false, eof_notify: None };
        let (mut status, mut sig, mut stderr, mut done) = (None, None, Vec::new(), false);
        let err = exec_stdout_next("ssh://test", &mut rx, &mut tx, &mut status, &mut sig, &mut stderr, &mut done).await.unwrap_err();
        assert!(err.to_string().contains("closed unexpectedly"), "{err}");
    }
}
