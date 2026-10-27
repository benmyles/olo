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
    /// Whether a login just used on `url` is already saved.
    Check { url: String, username: String, password: String },
    /// Save a login used on `url`: replacing `replace`'s password, or new.
    Save { url: String, title: String, username: String, password: String, replace: Option<(String, String)> },
    /// Bring Passwords forward and answer once it is unlocked.
    Unlock,
    /// Items whose title, website or tags hold every word of `query`
    /// (the command palette). Only while unlocked; doesn't wait.
    Search { query: String },
    /// Bring Passwords forward showing item `id` (once unlocked).
    Show { vault: String, id: String },
    /// From `op`: answered once unlocked and the asking terminal session
    /// is allowed.
    Cli { query: CliQuery },
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
    Items { vault: Option<String> },
    /// One item with its fields' values.
    Item { vault: Option<String>, item: String },
    /// `op://` references' values, in order.
    Resolve { references: Vec<String> },
}

impl Request {
    /// How long a reply can take.
    pub fn timeout(&self) -> Duration {
        match self {
            Request::Logins { .. } | Request::Search { .. } => Duration::from_secs(15),
            Request::Check { .. } => Duration::from_secs(60),
            // These wait for you to unlock Passwords.
            Request::Fill { .. }
            | Request::Save { .. }
            | Request::Unlock
            | Request::Show { .. }
            | Request::Cli { .. } => Duration::from_secs(300),
        }
    }
}

impl Drop for Request {
    fn drop(&mut self) {
        if let Request::Check { password, .. } | Request::Save { password, .. } = self {
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

/// An item `Search` found.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Found {
    pub vault: String,
    pub id: String,
    pub title: String,
    /// Its website's host, or its kind ("Secure Note").
    pub subtitle: String,
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
    /// For `Show`.
    Shown,
    Error {
        message: String,
    },
    /// For `Cli`: JSON as 1Password's `op` prints it.
    Cli {
        json: serde_json::Value,
    },
}

impl Drop for Response {
    fn drop(&mut self) {
        if let Response::Credentials { password, otp, .. } = self {
            password.zeroize();
            if let Some(otp) = otp {
                otp.zeroize();
            }
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
            CallError::Io(e)
                if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                f.write_str("Passwords didn’t answer in time")
            }
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
    rt.join(format!("olo-keychain-{display}.sock"))
}

/// Ask Passwords; blocks (call it from a worker thread).
pub fn call(request: &Request) -> Result<Response, CallError> {
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
                    Err(e) => {
                        Incoming { request: Request::Unlock, from, pid, stream }.reply(Response::Error { message: e.to_string() })
                    }
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
    let r = unsafe {
        libc::getsockopt(stream.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED, (&raw mut cred).cast(), &mut len)
    };
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
    fn round_trip() {
        let r = Request::Save {
            url: "https://a.example".into(),
            title: "A".into(),
            username: "me".into(),
            password: "pw".into(),
            replace: Some(("v".into(), "i".into())),
        };
        let text = serde_json::to_string(&r).unwrap();
        assert!(text.contains("\"type\":\"save\""));
        let back: Request = serde_json::from_str(&text).unwrap();
        assert!(matches!(back, Request::Save { replace: Some(_), .. }));
        let r: Response =
            serde_json::from_str(r#"{"type":"logins","logins":[{"vault":"v","id":"i","title":"T","username":"u"}]}"#)
                .unwrap();
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
                Request::Logins { url } => Response::Logins {
                    logins: vec![Login { vault: "v".into(), id: "i".into(), title: url.clone(), username: "u".into(), otp: false }],
                },
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
