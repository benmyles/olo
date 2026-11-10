//! Passwords is Olo's keychain: while it runs it answers other Olo
//! programs (Chrome's extension host, the palette) on a unix socket in
//! `$XDG_RUNTIME_DIR`, one JSON request and one JSON reply per connection. Only our own user's
//! `olo-*` programs are served.
//!
//! Passwords decides what may be said: logins for a site only while it is
//! unlocked; a fill or save while locked brings it forward and waits for
//! the unlock (so those calls can take minutes).
//!
//! `op` (`olo-op`, apps/op) asks with [`Request::Cli`]: Passwords
//! answers in the shapes 1Password's own `op` prints as JSON, after the
//! person allows that terminal session.
//!
//! Passkeys work the same way: Chrome's extension asks for a page's
//! passkeys, and Passwords makes and keeps the keys and signs with them
//! once the person agrees, so private keys never leave it. Byte strings
//! are unpadded base64url, as in WebAuthn's JSON forms.

use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::PathBuf,
    time::Duration,
};

use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    /// The saved logins for the page at `url`.
    Logins { url: String },
    /// One login's username and password, to fill into the page at `url`.
    Fill { url: String, vault: String, id: String },
    /// The credit cards there are, to fill a card form on the page at
    /// `url` with: what may be said of each (never its number or code).
    /// Doesn't wait: `Locked` while locked.
    Cards { url: String },
    /// Card `vault`/`id`'s number, name, expiry and security code, to
    /// fill into the page at `url`: the person chose it in Chrome's
    /// context menu (a card isn't kept for a site, so any page can be
    /// given one, but only that way).
    FillCard { url: String, vault: String, id: String },
    /// Whether a login just used on `url` is already saved.
    Check { url: String, username: String, password: String },
    /// Save a login used on `url`: replacing `replace`'s password, or new.
    Save { url: String, title: String, username: String, password: String, replace: Option<(String, String)> },
    /// Bring Passwords forward and answer once it is unlocked.
    Unlock,
    /// The passkeys for relying party `rp_id` the page at `url` may use
    /// (only those in `allow`, if it isn't empty). Doesn't wait: `Locked`
    /// while locked.
    Passkeys { url: String, rp_id: String, allow: Vec<String> },
    /// Make a passkey for `rp_id`, once the person agrees; one whose id is
    /// in `exclude` already there means they have one.
    PasskeyCreate { url: String, rp_id: String, rp_name: String, user_id: String, user_name: String, display_name: String, exclude: Vec<String> },
    /// Sign in to `rp_id` with a passkey (one in `allow`, if it isn't
    /// empty): the person picks which and agrees, unless `chosen` (an
    /// item) was picked already, in Chrome's context menu.
    /// `client_data_hash` is the SHA-256 of the page's client data.
    PasskeySign { url: String, rp_id: String, allow: Vec<String>, chosen: Option<(String, String)>, client_data_hash: String },
    /// Items whose title, website or tags hold every word of `query`
    /// (the command palette). Only while unlocked; doesn't wait.
    Search { query: String },
    /// Keep an Olo program's own secret (DiskBackup's key) as a Password
    /// item titled `title`, with `note` saying what it is; one with that
    /// title and secret already there is left as it is. Answered once
    /// unlocked: `Saved`.
    KeepSecret { title: String, secret: String, note: String },
    /// The secrets of the Password items titled `title` (an Olo program's
    /// own, kept by `KeepSecret`), once unlocked: `Secrets`.
    Secrets { title: String },
    /// Bring Passwords forward showing item `id` (once unlocked).
    Show { vault: String, id: String },
    /// From `op`: answered once unlocked and the asking terminal session
    /// is allowed.
    Cli { query: CliQuery },
    /// From Passwords' SSH agent, for a program asking it (ssh, git): the
    /// SSH keys there are, once unlocked.
    SshKeys,
    /// From Passwords' SSH agent: sign `data` (base64) with the key whose
    /// public key is `key` (base64, SSH's encoding), with `flags` as ssh
    /// gives them (2 and 4: RSA with SHA-256 or SHA-512); once unlocked and
    /// the asking program's terminal session is allowed.
    SshSign { key: String, data: String, flags: u32 },
    /// What's shared with agent screens (Agent Screens' Passwords tab and
    /// Shared with Every Screen), with every item it could share: once
    /// unlocked. Never answered to a program on an agent screen.
    Shares,
    /// Share these with every screen (`everywhere`), or set what screen
    /// `screen` has besides (or without) those. Never from a program on
    /// an agent screen.
    SetShares { everywhere: Option<Vec<String>>, screen: Option<(String, ScreenShares)> },
}

/// What one agent screen has besides what every screen has (`add`), and
/// what it goes without of that (`remove`): item ids.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenShares {
    #[serde(default)]
    pub add: Vec<String>,
    #[serde(default)]
    pub remove: Vec<String>,
}

impl ScreenShares {
    /// Nothing besides or without: the screen has what every screen has.
    pub fn is_empty(&self) -> bool {
        self.add.is_empty() && self.remove.is_empty()
    }
}

/// An item that can be shared, as `Shares` lists it (no secrets).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Shareable {
    pub vault: String,
    /// Its vault's name.
    pub vault_title: String,
    pub id: String,
    pub title: String,
    /// Its website's host, or its username.
    pub subtitle: String,
    /// Its kind ("Login", "Secure Note").
    pub kind: String,
}

/// Where what's shared with agent screens is kept: a copy of the items
/// in Passwords' own format, sealed with a key kept beside it, so a
/// screen's Passwords opens it on its own (whether or not the person's
/// is open or unlocked). `None` is what every screen has; a screen given
/// more or less has a copy of its own.
pub mod shared {
    use std::path::PathBuf;

    /// The folder agent screens keep their things in.
    pub fn screens_dir() -> PathBuf {
        olo_xdg::state_dir().join("screens")
    }

    /// The copy for `screen` (None: every screen's).
    pub fn copy_path(screen: Option<&str>) -> PathBuf {
        copy_path_in(&screens_dir(), screen)
    }

    /// The copy for `screen` with the screens' things in `dir`.
    pub fn copy_path_in(dir: &std::path::Path, screen: Option<&str>) -> PathBuf {
        match screen {
            Some(name) => dir.join(name).join("passwords.vault"),
            None => dir.join("passwords.vault"),
        }
    }

    /// The key that opens a copy.
    pub fn key_path(copy: &std::path::Path) -> PathBuf {
        copy.with_extension("key")
    }

    /// The agent screen this process is on, if it is on one (its
    /// session sets `OLO_SCREEN`).
    pub fn this_screen() -> Option<String> {
        std::env::var("OLO_SCREEN").ok().filter(|s| !s.is_empty())
    }

    /// Whether process `pid` runs on an agent screen.
    pub fn on_a_screen(pid: i32) -> bool {
        std::fs::read(format!("/proc/{pid}/environ")).map_or(true, |env| env.split(|b| *b == 0).any(|kv| kv.starts_with(b"OLO_SCREEN=") && kv.len() > b"OLO_SCREEN=".len()))
    }
}

/// What `op` asks. Items and vaults are named by ID or by name (any case).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CliQuery {
    /// The accounts, as `op account list --format json` prints them. No
    /// permission needed.
    Accounts,
    /// Ask the person now (`op signin`), if this session isn't allowed.
    Signin,
    Vaults,
    Items {
        vault: Option<String>,
    },
    /// Field labels, types and stable references only; never values or OTPs.
    Fields {
        vault: String,
        item: String,
    },
    /// One item with its fields' values.
    Item {
        vault: Option<String>,
        item: String,
    },
    /// `op://` references' values, in order.
    Resolve {
        references: Vec<String>,
    },
}

impl Request {
    /// How long a reply can take.
    pub fn timeout(&self) -> Duration {
        match self {
            Request::Logins { .. } | Request::Cards { .. } | Request::Search { .. } | Request::Passkeys { .. } => Duration::from_secs(15),
            Request::Check { .. } => Duration::from_secs(60),
            // These wait for you to unlock Passwords.
            Request::Fill { .. }
            | Request::FillCard { .. }
            | Request::Save { .. }
            | Request::Unlock
            | Request::Show { .. }
            | Request::PasskeyCreate { .. }
            | Request::PasskeySign { .. }
            | Request::Cli { .. }
            | Request::SshKeys
            | Request::SshSign { .. }
            | Request::Shares
            | Request::SetShares { .. }
            | Request::KeepSecret { .. }
            | Request::Secrets { .. } => Duration::from_secs(300),
        }
    }
}

impl Drop for Request {
    fn drop(&mut self) {
        if let Request::Check { password, .. } | Request::Save { password, .. } | Request::KeepSecret { secret: password, .. } = self {
            password.zeroize();
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Login {
    pub vault: String,
    pub id: String,
    pub title: String,
    pub username: String,
    /// It has a one-time code (TOTP) to offer.
    #[serde(default)]
    pub otp: bool,
}

/// A credit card, as a page's menu shows it: its title, the name on it,
/// its brand, its number's last four digits and its expiry ("08/2029").
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Card {
    pub vault: String,
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub cardholder: String,
    #[serde(default)]
    pub brand: String,
    #[serde(default)]
    pub last4: String,
    #[serde(default)]
    pub expiry: String,
}

/// An item `Search` found.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Found {
    pub vault: String,
    pub id: String,
    pub title: String,
    /// Its website's host, or its kind ("Secure Note").
    pub subtitle: String,
}

/// A passkey kept in Passwords (one per login item).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Passkey {
    pub vault: String,
    pub id: String,
    pub title: String,
    pub user_name: String,
    pub credential_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    Logins {
        logins: Vec<Login>,
    },
    Credentials {
        username: String,
        password: String,
        otp: Option<String>,
    },
    /// For `Cards`.
    Cards {
        cards: Vec<Card>,
    },
    /// For `FillCard`: the card's number (digits only), the name on it,
    /// its expiry (month "08", year "2029") and security code, and its
    /// brand ("Visa"), as far as it has them.
    CardDetails {
        number: String,
        cardholder: String,
        expiry_month: String,
        expiry_year: String,
        code: String,
        brand: String,
    },
    /// For `Check`.
    Known,
    New,
    Changed {
        vault: String,
        id: String,
        title: String,
    },
    Saved {
        title: String,
        updated: bool,
    },
    Unlocked,
    /// Passwords is locked (for `Logins` and `Check`, which don't wait).
    Locked,
    /// Passwords has no 1Password account set up yet.
    NotSetUp,
    /// For `Search`.
    Items {
        items: Vec<Found>,
    },
    /// For `Secrets`.
    Secrets {
        values: Vec<String>,
    },
    /// For `Show`.
    Shown,
    Passkeys {
        passkeys: Vec<Passkey>,
    },
    /// A new passkey: `public_key` is its SubjectPublicKeyInfo (DER).
    PasskeyCreated {
        title: String,
        credential_id: String,
        attestation_object: String,
        authenticator_data: String,
        public_key: String,
    },
    /// One of `exclude` is already saved.
    PasskeyExists,
    Assertion {
        credential_id: String,
        authenticator_data: String,
        signature: String,
        user_handle: String,
    },
    /// For `PasskeySign`: there's none for the site.
    NoPasskey,
    /// The person said no (to a passkey made or used).
    Declined,
    /// The person chose a passkey on another device (a phone, by its QR
    /// code, or a security key): the browser's own way.
    OtherDevice,
    Error {
        message: String,
    },
    /// For `Cli`: JSON as 1Password's `op` prints it.
    Cli {
        json: serde_json::Value,
    },
    /// For `Shares`: every item that could be shared, what every screen
    /// has, and what each screen has besides or without.
    Shares {
        items: Vec<Shareable>,
        everywhere: Vec<String>,
        screens: std::collections::BTreeMap<String, ScreenShares>,
    },
    /// For `SetShares`: kept, and the screens' copies written.
    Shared,
    /// For `SshKeys`.
    SshKeys {
        keys: Vec<SshKey>,
    },
    /// For `SshSign`: the signature, base64 of SSH's encoding.
    SshSignature {
        signature: String,
    },
}

/// An SSH key Passwords has: its public key (base64 of SSH's encoding) and
/// what it's called (the item's title).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SshKey {
    pub key: String,
    pub comment: String,
}

impl Drop for Response {
    fn drop(&mut self) {
        match self {
            Response::Credentials { password, otp, .. } => {
                password.zeroize();
                if let Some(otp) = otp {
                    otp.zeroize();
                }
            }
            Response::CardDetails { number, code, .. } => {
                number.zeroize();
                code.zeroize();
            }
            Response::Secrets { values } => values.iter_mut().for_each(|v| v.zeroize()),
            _ => {}
        }
    }
}

#[derive(Debug)]
pub enum CallError {
    /// Passwords isn't running.
    NotRunning,
    Io(std::io::Error),
    Protocol(String),
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CallError::NotRunning => f.write_str("Passwords isn’t running"),
            CallError::Io(e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut => f.write_str("Passwords didn’t answer in time"),
            CallError::Io(e) => write!(f, "Couldn’t talk to Passwords: {e}"),
            CallError::Protocol(e) => write!(f, "Passwords answered oddly: {e}"),
        }
    }
}

impl std::error::Error for CallError {}

/// The service's socket for this Wayland display.
pub fn socket_path() -> PathBuf {
    if let Some(p) = std::env::var_os("OLO_KEYCHAIN_SOCKET") {
        return PathBuf::from(p);
    }
    let rt = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let display = std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "wayland-0".into());
    // (By its name, if given as its socket's full path.)
    let display = display.rsplit('/').next().unwrap_or_default();
    rt.join(format!("olo-keychain-{display}.sock"))
}

/// Ask Passwords; blocks (call it from a worker thread).
pub fn call(request: &Request) -> Result<Response, CallError> {
    finish(connect(request)?)
}

/// Connect and send `request`.
fn connect(request: &Request) -> Result<UnixStream, CallError> {
    let mut s = UnixStream::connect(socket_path()).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused => CallError::NotRunning,
        _ => CallError::Io(e),
    })?;
    s.set_read_timeout(Some(request.timeout())).map_err(CallError::Io)?;
    s.set_write_timeout(Some(Duration::from_secs(5))).map_err(CallError::Io)?;
    let mut line = serde_json::to_string(request).map_err(|e| CallError::Protocol(e.to_string()))?;
    line.push('\n');
    let written = s.write_all(line.as_bytes());
    line.zeroize();
    written.map_err(CallError::Io)?;
    Ok(s)
}

/// The reply to the request sent on `s`.
fn finish(s: UnixStream) -> Result<Response, CallError> {
    let mut reply = String::new();
    let read = BufReader::new(&s).read_line(&mut reply);
    let result = match read {
        Ok(0) => Err(CallError::Protocol("no reply".into())),
        Ok(_) => serde_json::from_str(&reply).map_err(|e| CallError::Protocol(e.to_string())),
        Err(e) => Err(CallError::Io(e)),
    };
    reply.zeroize();
    result
}

/// As [`call`], handing `keep` the connection first, so another thread
/// can end it (`shutdown`) to give up waiting: the asker went away.
/// Passwords sees that as [`Incoming::gone`].
pub fn call_keeping(request: &Request, keep: impl FnOnce(UnixStream)) -> Result<Response, CallError> {
    let stream = connect(request)?;
    if let Ok(clone) = stream.try_clone() {
        keep(clone);
    }
    finish(stream)
}

/// A request waiting for its answer; answer it once, from any thread.
pub struct Incoming {
    pub request: Request,
    /// The asking program's file name (always `olo-…`).
    pub from: String,
    /// Its process.
    pub pid: i32,
    stream: UnixStream,
}

impl Incoming {
    /// Whether the asker has hung up (given up waiting).
    pub fn gone(&self) -> bool {
        use std::os::fd::AsRawFd;
        let mut fd = libc::pollfd { fd: self.stream.as_raw_fd(), events: libc::POLLRDHUP, revents: 0 };
        // SAFETY: one valid pollfd, not waiting.
        let n = unsafe { libc::poll(&mut fd, 1, 0) };
        n > 0 && fd.revents & (libc::POLLRDHUP | libc::POLLHUP | libc::POLLERR) != 0
    }

    /// A request made in this process for another program (Passwords'
    /// SSH agent, for ssh at `pid`): answered on the stream given back, a
    /// line of JSON as over the socket ([`read_reply`]).
    pub fn local(request: Request, from: String, pid: i32) -> std::io::Result<(Incoming, UnixStream)> {
        let (ours, theirs) = UnixStream::pair()?;
        Ok((Incoming { request, from, pid, stream: ours }, theirs))
    }

    pub fn reply(mut self, response: Response) {
        let Ok(mut line) = serde_json::to_string(&response) else { return };
        line.push('\n');
        let _ = self.stream.write_all(line.as_bytes());
        line.zeroize();
    }
}

impl std::fmt::Debug for Incoming {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Incoming")
    }
}

/// The answer to an [`Incoming::local`] request, read from its stream
/// (None if it was dropped unanswered).
pub fn read_reply(stream: &UnixStream) -> Option<Response> {
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).ok()?;
    let r = serde_json::from_str(&line).ok();
    line.zeroize();
    r
}

/// Serve requests: each goes to `deliver` (from a thread) as an
/// [`Incoming`]. Fails if another Passwords already serves this display.
pub fn serve(deliver: impl Fn(Incoming) + Send + Sync + 'static) -> std::io::Result<()> {
    let path = socket_path();
    if UnixStream::connect(&path).is_ok() {
        return Err(std::io::Error::new(std::io::ErrorKind::AddrInUse, "already served"));
    }
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    let deliver = std::sync::Arc::new(deliver);
    std::thread::Builder::new().name("keychain".into()).spawn(move || {
        for stream in listener.incoming().flatten() {
            let deliver = deliver.clone();
            std::thread::spawn(move || {
                let (from, pid) = match trusted(&stream) {
                    Ok(from) => from,
                    Err(e) => {
                        tracing::warn!("keychain: refused a caller: {e}");
                        let message = "Passwords answers only Olo’s own apps.".into();
                        Incoming { request: Request::Unlock, from: String::new(), pid: 0, stream }.reply(Response::Error { message });
                        return;
                    }
                };
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                let mut line = String::new();
                let Ok(reader) = stream.try_clone() else { return };
                if BufReader::new(reader).read_line(&mut line).is_err() {
                    return;
                }
                let parsed = serde_json::from_str::<Request>(&line);
                line.zeroize();
                match parsed {
                    Ok(request) => deliver(Incoming { request, from, pid, stream }),
                    Err(e) => Incoming { request: Request::Unlock, from, pid, stream }.reply(Response::Error { message: e.to_string() }),
                }
            });
        }
    })?;
    Ok(())
}

/// Our own user, running one of our programs: its file name and pid.
fn trusted(stream: &UnixStream) -> Result<(String, i32), String> {
    use std::os::fd::AsRawFd;
    let mut cred = libc::ucred { pid: 0, uid: 0, gid: 0 };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: cred and len are valid for getsockopt to write.
    let r = unsafe { libc::getsockopt(stream.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED, (&raw mut cred).cast(), &mut len) };
    if r != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    // SAFETY: getuid can't fail.
    if cred.uid != unsafe { libc::getuid() } {
        return Err(format!("uid {}", cred.uid));
    }
    let exe = std::fs::read_link(format!("/proc/{}/exe", cred.pid)).map_err(|e| e.to_string())?;
    // A program still running after an upgrade replaced its file.
    let name = exe.file_name().and_then(|n| n.to_str()).unwrap_or("").trim_end_matches(" (deleted)");
    // Or still running from before the rename (Tabula), until it
    // re-executes as the new program: known by its new name.
    let name = match name.strip_prefix("tabula-") {
        Some(rest) => format!("olo-{rest}"),
        None => name.to_owned(),
    };
    if !name.starts_with("olo-") {
        return Err(format!("{} isn’t an Olo program", exe.display()));
    }
    Ok((name, cred.pid))
}

/// The host a login's website names, without `www.`.
pub fn host(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).or_else(|_| url::Url::parse(&format!("https://{url}"))).ok()?;
    let host = parsed.host_str()?.to_ascii_lowercase();
    Some(host.strip_prefix("www.").map(str::to_string).unwrap_or(host))
}

/// Whether a login saved for `website` belongs on the page at `page`: the
/// same site, as 1Password matches (the same registrable domain, so a
/// login saved on mail.google.com fills accounts.google.com, but one on
/// alice.github.io never fills bob.github.io), or the same host where
/// there is no such domain (localhost, addresses); and never an https
/// login into a plain-http page unless it's this machine.
pub fn matches(website: &str, page: &str) -> bool {
    let Ok(page_url) = url::Url::parse(page) else {
        return false;
    };
    if !matches!(page_url.scheme(), "http" | "https") {
        return false;
    }
    let (Some(site), Some(here)) = (host(website), host(page)) else {
        return false;
    };
    let same_site = match (psl::domain_str(&site), psl::domain_str(&here)) {
        (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
        _ => false,
    };
    if !(same_site || here == site || here.ends_with(&format!(".{site}"))) {
        return false;
    }
    let local = matches!(here.as_str(), "localhost" | "127.0.0.1" | "[::1]");
    let website_https = !website.starts_with("http://");
    !(website_https && page_url.scheme() == "http" && !local)
}

/// How near a login saved for `website` is to the page at `page`, to list
/// the nearest first: 0 for the page's own host, then its parent domains
/// from the nearest (the number of labels dropped), then the site's other
/// hosts (more labels in common first). None if it doesn't belong there.
pub fn closeness(website: &str, page: &str) -> Option<u32> {
    if !matches(website, page) {
        return None;
    }
    let (site, here) = (host(website)?, host(page)?);
    if site == here {
        return Some(0);
    }
    if here.ends_with(&format!(".{site}")) {
        return Some((here.split('.').count() - site.split('.').count()) as u32);
    }
    let shared = site.rsplit('.').zip(here.rsplit('.')).take_while(|(a, b)| a == b).count();
    Some(100u32.saturating_sub(shared as u32))
}

/// Whether a page at `url` may use passkeys for `rp_id`: a secure page
/// (or this machine's) whose host is `rp_id` or under it, where `rp_id`
/// isn't a public suffix (`com`, `github.io`) or an address.
pub fn rp_id_fits(rp_id: &str, url: &str) -> bool {
    let Ok(page) = url::Url::parse(url) else {
        return false;
    };
    let Some(url::Host::Domain(here)) = page.host().map(|h| h.to_owned()) else {
        return false;
    };
    let here = here.to_ascii_lowercase();
    let rp_id = rp_id.to_ascii_lowercase();
    let local = here == "localhost" || here.ends_with(".localhost");
    let secure = page.scheme() == "https" || (page.scheme() == "http" && local);
    let under = here == rp_id || here.ends_with(&format!(".{rp_id}"));
    let suffix = !(rp_id == "localhost" || rp_id.ends_with(".localhost")) && psl::suffix_str(&rp_id).is_none_or(|s| s == rp_id);
    secure && !rp_id.is_empty() && under && !suffix
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_logins_first() {
        let page = "https://accounts.google.com/signin";
        fn order<'a>(sites: &[&'a str], page: &str) -> Vec<&'a str> {
            let mut v = sites.to_vec();
            v.sort_by_key(|s| closeness(s, page));
            v
        }
        assert_eq!(
            order(&["https://mail.google.com", "https://google.com", "https://accounts.google.com", "https://www.google.com/x"], page),
            ["https://accounts.google.com", "https://google.com", "https://www.google.com/x", "https://mail.google.com"]
        );
        assert_eq!(closeness("https://login.accounts.google.com", page), Some(97));
        assert_eq!(closeness("https://example.com", page), None);
        // Two levels up comes after one.
        assert!(closeness("https://b.c.example.com", "https://a.b.c.example.com") < closeness("https://example.com", "https://a.b.c.example.com"));
    }

    #[test]
    fn matching() {
        assert!(matches("https://github.com", "https://github.com/login"));
        assert!(matches("github.com", "https://www.github.com/login"));
        assert!(matches("https://example.com", "https://login.example.com/"));
        // The same site, as 1Password matches.
        assert!(matches("https://login.example.com", "https://example.com/"));
        assert!(matches("https://mail.google.com", "https://accounts.google.com/v3/signin"));
        assert!(matches("google.com", "https://accounts.google.com/"));
        assert!(!matches("https://alice.github.io", "https://bob.github.io/"));
        assert!(!matches("https://a.co.uk", "https://b.co.uk/"));
        assert!(!matches("https://192.168.1.2", "https://192.168.1.3/"));
        assert!(!matches("https://example.com", "https://badexample.com/"));
        assert!(!matches("https://example.com", "https://example.com.evil.net/"));
        assert!(!matches("https://example.com", "http://example.com/"));
        assert!(matches("http://example.com", "http://example.com/"));
        assert!(matches("http://localhost", "http://localhost:8080/login"));
        assert!(matches("https://localhost", "http://localhost:8080/login"));
        assert!(!matches("https://example.com", "file:///example.com"));
    }

    #[test]
    fn rp_ids() {
        assert!(rp_id_fits("example.com", "https://example.com/"));
        assert!(rp_id_fits("example.com", "https://login.example.com/"));
        assert!(rp_id_fits("Example.COM", "https://LOGIN.example.com/"));
        assert!(!rp_id_fits("login.example.com", "https://example.com/"));
        assert!(!rp_id_fits("example.com", "http://example.com/"));
        assert!(!rp_id_fits("example.com", "https://badexample.com/"));
        assert!(!rp_id_fits("com", "https://example.com/"));
        assert!(!rp_id_fits("github.io", "https://alice.github.io/"));
        assert!(rp_id_fits("alice.github.io", "https://alice.github.io/"));
        assert!(!rp_id_fits("co.uk", "https://bbc.co.uk/"));
        assert!(rp_id_fits("localhost", "http://localhost:8765/"));
        assert!(!rp_id_fits("127.0.0.1", "http://127.0.0.1:8765/"));
        assert!(!rp_id_fits("", "https://example.com/"));
    }

    #[test]
    fn round_trip() {
        let r = Request::Save { url: "https://a.example".into(), title: "A".into(), username: "me".into(), password: "pw".into(), replace: Some(("v".into(), "i".into())) };
        let text = serde_json::to_string(&r).unwrap();
        assert!(text.contains("\"type\":\"save\""));
        let back: Request = serde_json::from_str(&text).unwrap();
        assert!(matches!(back, Request::Save { replace: Some(_), .. }));
        let r: Response = serde_json::from_str(r#"{"type":"logins","logins":[{"vault":"v","id":"i","title":"T","username":"u"}]}"#).unwrap();
        assert!(matches!(r, Response::Logins { ref logins } if logins.len() == 1));
    }

    #[test]
    fn service() {
        let dir = std::env::temp_dir().join(format!("keychain-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // SAFETY: tests in this module don't read the environment concurrently.
        unsafe { std::env::set_var("OLO_KEYCHAIN_SOCKET", dir.join("k.sock")) };
        serve(|incoming| {
            let response = match &incoming.request {
                Request::Logins { url } => Response::Logins { logins: vec![Login { vault: "v".into(), id: "i".into(), title: url.clone(), username: "u".into(), otp: false }] },
                _ => Response::Locked,
            };
            incoming.reply(response);
        })
        .unwrap();
        // The test binary isn't called olo-*, so it's refused.
        let r = call(&Request::Logins { url: "https://a.example/".into() });
        assert!(matches!(r, Ok(Response::Error { .. })), "{r:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// `~/.ssh/config` and Passwords' SSH agent: ssh uses the agent in
/// `SSH_AUTH_SOCK` unless an `IdentityAgent` line names another (1Password's
/// app writes `IdentityAgent ~/.1password/agent.sock`).
pub mod ssh_config {
    /// The person's ssh configuration.
    pub fn path() -> std::path::PathBuf {
        std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".ssh/config")
    }

    /// The agent `text` sends ssh to instead of `SSH_AUTH_SOCK`'s, if it
    /// names one (its first `IdentityAgent` that isn't `SSH_AUTH_SOCK` or
    /// `none`).
    pub fn other_agent(text: &str) -> Option<String> {
        text.lines().find_map(|l| {
            let (key, value) = split(l)?;
            (key.eq_ignore_ascii_case("IdentityAgent") && !value.eq_ignore_ascii_case("SSH_AUTH_SOCK") && !value.eq_ignore_ascii_case("none")).then(|| value.to_owned())
        })
    }

    /// `text` with each `IdentityAgent` naming another agent set to
    /// `SSH_AUTH_SOCK` (Passwords'), the line it was kept above as a
    /// comment, to put back by hand.
    pub fn use_session_agent(text: &str) -> String {
        let mut out = String::new();
        for l in text.split_inclusive('\n') {
            let line = l.trim_end_matches('\n');
            match split(line) {
                Some((key, value)) if key.eq_ignore_ascii_case("IdentityAgent") && !value.eq_ignore_ascii_case("SSH_AUTH_SOCK") && !value.eq_ignore_ascii_case("none") => {
                    let indent = &line[..line.len() - line.trim_start().len()];
                    out.push_str(&format!("{indent}# {} (before Olo Passwords was the SSH agent: Preferences ▸ Security)\n", line.trim()));
                    out.push_str(&format!("{indent}IdentityAgent SSH_AUTH_SOCK\n"));
                }
                _ => out.push_str(l),
            }
        }
        out
    }

    /// A line's keyword and value (`Key value` or `Key=value`), comments
    /// and blank lines none.
    fn split(line: &str) -> Option<(&str, &str)> {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            return None;
        }
        let at = t.find(|c: char| c.is_whitespace() || c == '=')?;
        let value = t[at..].trim_start_matches(|c: char| c.is_whitespace() || c == '=').trim().trim_matches('"');
        Some((&t[..at], value))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn finds_and_replaces_another_agent() {
            let text = "Host *\n\tIdentityAgent ~/.1password/agent.sock\n\nHost work\n  User me\n";
            assert_eq!(other_agent(text).as_deref(), Some("~/.1password/agent.sock"));
            let new = use_session_agent(text);
            assert_eq!(
                new,
                "Host *\n\t# IdentityAgent ~/.1password/agent.sock (before Olo Passwords was the SSH agent: Preferences ▸ Security)\n\tIdentityAgent SSH_AUTH_SOCK\n\nHost work\n  User me\n"
            );
            assert_eq!(other_agent(&new), None);
            assert_eq!(other_agent("Host x\n  IdentityAgent none\n"), None);
            assert_eq!(other_agent("IdentityAgent=\"/run/a.sock\"\n").as_deref(), Some("/run/a.sock"));
        }
    }
}
