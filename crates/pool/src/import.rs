//! Bringing a subscription's sign-in into the pool: the one the official
//! client keeps on this computer, or a separate one made by running the
//! client's own sign-in in a private directory (so the account can differ
//! from the computer's), read and then deleted.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::{
    clients::{agy_dir, claude_dir, codex_dir},
    model::{Credential, Provider},
    upstream::{jwt_claims, jwt_exp, timestamp},
};

/// Said where a client keeps no sign-in.
const NO_SIGN_IN: &str = "There’s no sign-in there: sign in first.";

/// A sign-in found, with what it says about its account.
#[derive(Debug, Clone)]
pub struct Found {
    pub provider: Provider,
    pub credential: Credential,
    pub email: Option<String>,
    pub plan: Option<String>,
    /// Claude's organization.
    pub org: Option<String>,
}

impl Found {
    /// A name for the account: its email's local part, else the plan.
    pub fn name(&self) -> String {
        if let Some(e) = &self.email {
            return e.split('@').next().unwrap_or(e).to_owned();
        }
        match &self.plan {
            Some(p) => format!("{} {}", self.provider.client(), capitalized(p)),
            None => self.provider.client().to_owned(),
        }
    }
}

fn capitalized(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

/// The most a sign-in's file holds (a few kilobytes): anything bigger is
/// no sign-in, and isn't read whole.
const MAX_SIGN_IN: u64 = 1 << 20;

/// The file at `path`, if there is one: a sign-in's, so a plain file of a
/// sign-in's size. Opened without waiting (a pipe or a device put there
/// can't hold the reader, and with it Cancel and the wait's end) and read
/// no further than that size. A link is followed only where `follow`
/// (the person's own client's files, which a dotfiles folder may hold
/// as links; never in a sign-in's private directory, where one is
/// planted).
fn bytes(path: &Path, follow: bool) -> Result<Option<Vec<u8>>, String> {
    use std::{io::Read as _, os::unix::fs::OpenOptionsExt as _};
    let couldnt = |e: std::io::Error| format!("Couldn’t read {}: {e}", path.display());
    let flags = libc::O_NONBLOCK | if follow { 0 } else { libc::O_NOFOLLOW };
    let file = match std::fs::OpenOptions::new().read(true).custom_flags(flags).open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(couldnt(e)),
    };
    if !file.metadata().map_err(couldnt)?.is_file() {
        return Err(format!("{} isn’t a file.", path.display()));
    }
    let mut b = Vec::new();
    file.take(MAX_SIGN_IN + 1).read_to_end(&mut b).map_err(couldnt)?;
    if b.len() as u64 > MAX_SIGN_IN {
        return Err(format!("{} is too big to be a sign-in.", path.display()));
    }
    Ok(Some(b))
}

/// What a sign-in's file at `path` says, as JSON (`follow`: [`bytes`]).
fn read(path: &Path, follow: bool) -> Result<Value, Unread> {
    let bytes = bytes(path, follow).map_err(Unread::Never)?.ok_or_else(|| Unread::NotYet(NO_SIGN_IN.to_owned()))?;
    serde_json::from_slice(&bytes).map_err(|_| Unread::NotYet(format!("{} isn’t a sign-in this version understands.", path.display())))
}

/// Why a sign-in wasn't read.
#[derive(Debug, Clone, PartialEq)]
pub enum Unread {
    /// None there (yet), or one being written (half of one isn't JSON):
    /// a separate sign-in's is looked at again.
    NotYet(String),
    /// There, and never to be read: not a plain file, too big for a
    /// sign-in, unreadable, or a sign-in of a kind the pool can't use.
    /// The wait for a separate one ends at once, saying so.
    Never(String),
}

impl Unread {
    pub fn text(self) -> String {
        match self {
            Unread::NotYet(e) | Unread::Never(e) => e,
        }
    }
}

/// Claude Code's sign-in in `dir` (its config directory), its account's
/// details kept in `home_meta` (`~/.claude.json`: where Claude Code keeps
/// them for its default config directory, and only for that one,
/// [`this_computer`]) or else beside it in `dir`. Never a directory's
/// parent's: for a separate sign-in that was the runtime directory's, or
/// /var/tmp's, any account's (Astra's review, 2026-11-28).
fn read_claude_with(dir: &Path, home_meta: Option<&Path>, follow: bool) -> Result<Found, Unread> {
    let v = read(&dir.join(".credentials.json"), follow)?;
    let o = &v["claudeAiOauth"];
    let access = o["accessToken"].as_str().filter(|s| !s.is_empty()).ok_or_else(|| Unread::Never("Claude Code is signed in with an API key, not a subscription.".to_owned()))?;
    // Its account details, read as its sign-in is: a plain file, of a
    // sign-in's size, opened without waiting (this computer's own through
    // a link, as a dotfiles folder keeps them; a separate sign-in's, as a
    // private directory's, never). Where Claude Code keeps them first, as
    // Terminal reads them: the home's for the default config directory,
    // an old copy in that directory only where the home's has none
    // (Astra's review, 2026-11-28: the old copy's account was taken over
    // the one signed in now).
    let own = dir.join(".claude.json");
    let account = [home_meta, Some(own.as_path())]
        .into_iter()
        .flatten()
        .find_map(|p| bytes(p, follow).ok().flatten().and_then(|b| serde_json::from_slice::<Value>(&b).ok()).map(|v| v["oauthAccount"].clone()).filter(Value::is_object))
        .unwrap_or(Value::Null);
    Ok(Found {
        provider: Provider::Claude,
        credential: Credential { access: access.to_owned(), refresh: o["refreshToken"].as_str().map(str::to_owned), expires_at: timestamp(&o["expiresAt"]), account_id: None },
        email: account["emailAddress"].as_str().map(str::to_owned),
        plan: o["subscriptionType"].as_str().map(str::to_owned),
        org: account["organizationUuid"].as_str().map(str::to_owned),
    })
}

/// Claude Code's sign-in in its config directory `dir` on this computer.
pub fn read_claude(dir: &Path, home_meta: Option<&Path>) -> Result<Found, String> {
    read_claude_with(dir, home_meta, true).map_err(Unread::text)
}

/// Codex's sign-in in `dir` (its home directory).
pub fn read_codex(dir: &Path) -> Result<Found, String> {
    read_codex_with(dir, true).map_err(Unread::text)
}

fn read_codex_with(dir: &Path, follow: bool) -> Result<Found, Unread> {
    let v = read(&dir.join("auth.json"), follow)?;
    let t = &v["tokens"];
    let access = t["access_token"].as_str().filter(|s| !s.is_empty()).ok_or_else(|| Unread::Never("Codex is signed in with an API key, not ChatGPT.".to_owned()))?;
    let claims = t["id_token"].as_str().and_then(jwt_claims).unwrap_or(Value::Null);
    let auth = &claims["https://api.openai.com/auth"];
    Ok(Found {
        provider: Provider::Codex,
        credential: Credential {
            access: access.to_owned(),
            refresh: t["refresh_token"].as_str().map(str::to_owned),
            expires_at: jwt_exp(access),
            account_id: t["account_id"].as_str().or(auth["chatgpt_account_id"].as_str()).map(str::to_owned),
        },
        email: claims["email"].as_str().map(str::to_owned),
        plan: auth["chatgpt_plan_type"].as_str().map(str::to_owned),
        org: None,
    })
}

/// Antigravity's sign-in in its directory, where agy keeps it when there
/// is no keyring to keep it in (no session bus: a Rex host, a container).
const AGY_FILE: &str = "antigravity-oauth-token";

/// Where agy keeps its sign-in on a desktop: in the login keyring, the
/// Secret Service item go-keyring finds by these attributes, holding the
/// same JSON as its file, as it is (go-keyring's encodings are its macOS
/// Keychain's). agy looks there first and in its file after (agy 1.5.18,
/// run against a keyring of a test's own: "ChainedAuth: authenticated via
/// keyring"). Only its file had been read, so a desktop's sign-in was
/// "There's no sign-in there" (Ben, 2026-11-28).
pub const AGY_KEYRING: &[(&str, &str)] = &[("service", "gemini"), ("username", "antigravity")];

/// Said where the keyring Antigravity keeps its sign-in in stays locked
/// (as Passwords says it).
const AGY_LOCKED: &str = "Your keyring is locked, and Antigravity keeps its sign-in in it. Unlock it with your login password, then try again.";

/// Said where agy keeps no sign-in on this computer, in the keyring or
/// its file.
const AGY_SIGNED_OUT: &str = "Antigravity isn’t signed in on this computer: run agy and sign in first.";

/// What the login keyring said when asked for Antigravity's sign-in.
// (A Rex host's pool never asks one: only the tests say the rest there.
// No Debug: what it keeps is a sign-in.)
#[cfg_attr(not(feature = "desktop"), allow(dead_code))]
enum Keyring {
    /// What it keeps.
    Kept(Vec<u8>),
    /// Nothing kept, or no keyring: no session bus, or none serving it.
    Absent,
    /// Locked, and it stayed so (its password wasn't given).
    Locked,
    /// What it keeps there is bigger than a sign-in: never taken in.
    TooBig,
    /// It couldn't be asked.
    Failed(String),
}

/// What the keyring asked said, and whether it was opened on the way (its
/// password given): the pool's own accounts, kept in it too, may be
/// opened now whatever was found ([`Imported::unlocked`]).
#[cfg_attr(not(feature = "desktop"), allow(dead_code))]
struct Asked {
    keyring: Keyring,
    unlocked: bool,
}

/// An Antigravity sign-in as agy keeps it, in its file and the keyring
/// alike (its `StoredToken`: `{"token": {access_token, refresh_token,
/// expiry, …}, "auth_method", "id_token_email"}`). `kept` names where it
/// was, for what's said when it can't be read.
pub fn parse_agy_token(bytes: &[u8], kept: &str) -> Result<Found, String> {
    let v: Value = serde_json::from_slice(bytes).map_err(|_| format!("{kept} isn’t a sign-in this version understands."))?;
    let t = &v["token"];
    let text = |v: &Value| v.as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned);
    let access = text(&t["access_token"]).ok_or("Antigravity has no OAuth access token there: sign in first.")?;
    // The account: as agy keeps it, else as an ID token kept beside says.
    let email = text(&v["id_token_email"]).or_else(|| [&v["id_token"], &t["id_token"]].into_iter().find_map(|j| text(&jwt_claims(j.as_str()?)?["email"])));
    Ok(Found {
        provider: Provider::Antigravity,
        credential: Credential { access, refresh: text(&t["refresh_token"]), expires_at: timestamp(&t["expiry"]), account_id: None },
        email,
        plan: None,
        org: None,
    })
}

/// Antigravity's sign-in in `dir` (`$HOME/.gemini/antigravity-cli`): its
/// file.
pub fn read_agy(dir: &Path) -> Result<Found, String> {
    read_agy_with(dir, true).map_err(Unread::text)
}

fn read_agy_with(dir: &Path, follow: bool) -> Result<Found, Unread> {
    let path = dir.join(AGY_FILE);
    let bytes = bytes(&path, follow).map_err(Unread::Never)?.ok_or_else(|| Unread::NotYet(NO_SIGN_IN.to_owned()))?;
    // (One half written isn't JSON: looked at again. One whole without
    // an access token isn't one the pool can use.)
    serde_json::from_slice::<Value>(&bytes).map_err(|_| Unread::NotYet(format!("{} isn’t a sign-in this version understands.", path.display())))?;
    parse_agy_token(&bytes, &path.display().to_string()).map_err(Unread::Never)
}

/// Antigravity's sign-in on this computer, looked for as agy looks for it:
/// in the login keyring (what `keyring` said), else in its file in `dir`.
/// What the keyring gave no sign-in for (nothing kept, locked, not to be
/// asked, or what it keeps unreadable) leaves the file to give it; with
/// none there either, what kept the keyring's from being read is what's
/// said, before that the file has none.
fn agy_here(keyring: Keyring, dir: &Path) -> Result<Found, String> {
    let unread = match keyring {
        Keyring::Kept(bytes) => match parse_agy_token(&bytes, "What Antigravity keeps in the login keyring") {
            Ok(found) => return Ok(found),
            Err(e) => Some(e),
        },
        Keyring::Absent => None,
        Keyring::Locked => Some(AGY_LOCKED.to_owned()),
        Keyring::TooBig => Some("What Antigravity keeps in the login keyring is too big to be a sign-in.".to_owned()),
        Keyring::Failed(e) => Some(format!("Couldn’t ask the login keyring for Antigravity’s sign-in: {e}")),
    };
    let path = dir.join(AGY_FILE);
    match bytes(&path, true) {
        Ok(Some(b)) => parse_agy_token(&b, &path.display().to_string()).map_err(|e| unread.unwrap_or(e)),
        Ok(None) => Err(unread.unwrap_or_else(|| AGY_SIGNED_OUT.to_owned())),
        Err(e) => Err(unread.unwrap_or(e)),
    }
}

/// The login keyring asked for Antigravity's sign-in. Never the session's
/// from a test: this crate's (`cfg(test)`), or a program's that says so
/// (`OLO_POOL_TEST_ISOLATE`, as `authority` takes it).
fn agy_keyring() -> Asked {
    if cfg!(test) || std::env::var_os("OLO_POOL_TEST_ISOLATE").is_some() {
        return Asked { keyring: Keyring::Absent, unlocked: false };
    }
    ask_keyring(AGY_KEYRING)
}

/// How the session's keyring is asked ([`Session`]; a test's own in tests).
#[cfg_attr(not(feature = "desktop"), allow(dead_code))]
trait Secrets {
    /// Whether there's one: a session bus with a Secret Service on it, or
    /// one it can start.
    fn present(&self) -> bool;
    /// Whether Olo's keyring is on but locked.
    fn locked(&self) -> bool;
    /// Olo's keyring asked to open (the login password, asked for once):
    /// whether it did.
    fn unlock(&self) -> Result<bool, String>;
    /// What its login keyring keeps under `attributes`, as go-keyring
    /// finds it (`secrets::lookup_login`).
    fn lookup(&self, attributes: &[(&str, &str)]) -> Result<Option<Vec<u8>>, Shut>;
}

/// Why a keyring gave nothing back.
#[cfg_attr(not(feature = "desktop"), allow(dead_code))]
enum Shut {
    /// It stayed locked (its own prompt, dismissed).
    Locked,
    /// What it keeps is bigger than a sign-in (`MAX_SIGN_IN`).
    TooBig,
    Failed(String),
}

/// What a keyring keeps under `attributes`, asked through `s`. Olo's
/// keyring, locked, is asked to open once, as Passwords asks (the person
/// asked for what's in it), before anything is asked of it: a question
/// put to it locked brought its own prompt, and after Not Now there this
/// asked again, the same prompt twice. Stayed locked, nothing more is
/// asked.
#[cfg_attr(not(feature = "desktop"), allow(dead_code))]
fn ask_keyring_with(s: &impl Secrets, attributes: &[(&str, &str)]) -> Asked {
    // No session bus, or no keyring on it: as agy, the file alone.
    if !s.present() {
        return Asked { keyring: Keyring::Absent, unlocked: false };
    }
    let mut unlocked = false;
    if s.locked() {
        match s.unlock() {
            Ok(true) => unlocked = true,
            Ok(false) => return Asked { keyring: Keyring::Locked, unlocked },
            Err(e) => return Asked { keyring: Keyring::Failed(e), unlocked },
        }
    }
    let keyring = match s.lookup(attributes) {
        Ok(Some(b)) => Keyring::Kept(b),
        Ok(None) => Keyring::Absent,
        Err(Shut::Locked) => Keyring::Locked,
        Err(Shut::TooBig) => Keyring::TooBig,
        Err(Shut::Failed(e)) => Keyring::Failed(e),
    };
    Asked { keyring, unlocked }
}

/// The session's keyring, on its bus.
#[cfg(feature = "desktop")]
struct Session;

#[cfg(feature = "desktop")]
impl Secrets for Session {
    fn present(&self) -> bool {
        olo_system::secrets::present()
    }

    fn locked(&self) -> bool {
        olo_system::secrets::locked()
    }

    fn unlock(&self) -> Result<bool, String> {
        olo_system::secrets::unlock().map_err(|e| e.to_string())
    }

    fn lookup(&self, attributes: &[(&str, &str)]) -> Result<Option<Vec<u8>>, Shut> {
        use olo_system::Error;
        olo_system::secrets::lookup_login(attributes, MAX_SIGN_IN as usize).map_err(|e| {
            let said = Said {
                too_big: matches!(&e, Error::Failure(m) if m == olo_system::secrets::TOO_BIG),
                failure: match &e {
                    Error::Failure(m) => Some(m.as_str()),
                    _ => None,
                },
                name: match &e {
                    Error::MethodError(name, ..) => Some(name.as_str()),
                    _ => None,
                },
                text: e.to_string(),
            };
            shut(said, olo_system::secrets::locked)
        })
    }
}

/// What the keyring said when asked for a secret and none came back.
#[cfg_attr(not(feature = "desktop"), allow(dead_code))]
struct Said<'a> {
    /// That what it keeps is bigger than asked for (`secrets::TOO_BIG`).
    too_big: bool,
    /// Its failure's words (`Error::Failure`).
    failure: Option<&'a str>,
    /// The Secret Service's error's name (`Error::MethodError`).
    name: Option<&'a str>,
    /// All of it, as said.
    text: String,
}

/// Why the keyring gave nothing back, from what it said, and, where that
/// says nothing of it, whether Olo's is locked now (`locked`).
#[cfg_attr(not(feature = "desktop"), allow(dead_code))]
fn shut(said: Said, locked: impl FnOnce() -> bool) -> Shut {
    match (said.failure, said.name) {
        _ if said.too_big => Shut::TooBig,
        // (A keyring's own unlock prompt, dismissed: `lookup_login`; or a
        // collection locked again before its secret was read: the Secret
        // Service's `IsLocked`, Neon's review, 2026-11-28.)
        (Some(m), _) if m.contains("locked") => Shut::Locked,
        (_, Some("org.freedesktop.Secret.Error.IsLocked")) => Shut::Locked,
        _ if locked() => Shut::Locked,
        _ => Shut::Failed(said.text),
    }
}

/// What the login keyring keeps under `attributes`. Blocks on the bus and
/// on the person: a worker thread's.
#[cfg(feature = "desktop")]
fn ask_keyring(attributes: &[(&str, &str)]) -> Asked {
    ask_keyring_with(&Session, attributes)
}

/// (A Rex host's pool has no keyring: agy keeps its file there.)
#[cfg(not(feature = "desktop"))]
fn ask_keyring(_attributes: &[(&str, &str)]) -> Asked {
    Asked { keyring: Keyring::Absent, unlocked: false }
}

/// A sign-in on this computer looked for ([`this_computer`]).
pub struct Imported {
    pub found: Result<Found, String>,
    /// The keyring was opened as it was looked for (its password given):
    /// the pool's own accounts can be opened now, whatever was found
    /// (Astra's review, 2026-11-28: with no Antigravity sign-in in it,
    /// they waited for the vault's next try, up to 30 s).
    pub unlocked: bool,
}

/// The sign-in the official client keeps on this computer. Blocking (the
/// keyring, for Antigravity's): call from a worker thread.
pub fn this_computer(p: Provider) -> Imported {
    match p {
        // (Its account's details are in the home directory only for the
        // default config directory, as Claude Code keeps them.)
        Provider::Claude => {
            let home_meta = std::env::var_os("CLAUDE_CONFIG_DIR").is_none().then(|| crate::clients::home().join(".claude.json"));
            Imported { found: read_claude(&claude_dir(), home_meta.as_deref()), unlocked: false }
        }
        Provider::Codex => Imported { found: read_codex(&codex_dir()), unlocked: false },
        Provider::Antigravity => {
            let asked = agy_keyring();
            Imported { found: agy_here(asked.keyring, &agy_dir()), unlocked: asked.unlocked }
        }
    }
}

/// Where a separate sign-in's directory goes: in the runtime directory
/// (`runtime`, its tmpfs: a sign-in that never reaches the disk) where
/// there's one that's absolute, text and free of colons, else under
/// /var/tmp (never /tmp, which is memory). (Its `bin` goes first on a
/// PATH, where a colon would cut it in two, and agy find another
/// `xdg-open`.)
fn separate_base(runtime: Option<&std::ffi::OsStr>) -> PathBuf {
    runtime.and_then(|r| r.to_str()).map(PathBuf::from).filter(|p| p.is_absolute() && !p.to_string_lossy().contains(':')).unwrap_or_else(|| PathBuf::from("/var/tmp"))
}

/// A new private directory for a separate sign-in (`separate_base`).
pub fn separate_dir() -> Result<PathBuf, String> {
    use std::os::unix::fs::DirBuilderExt;
    let base = separate_base(std::env::var_os("XDG_RUNTIME_DIR").as_deref());
    let dir = base.join(format!("olo-pool-signin-{}", crate::store::new_id()));
    std::fs::DirBuilder::new().mode(0o700).create(&dir).map_err(|e| format!("Couldn’t make {}: {e}", dir.display()))?;
    Ok(dir)
}

/// What a separate Antigravity sign-in is never given of the environment:
/// the keyring's (`GNOME_KEYRING_CONTROL`), the pool's proxy and its
/// certificate ("Antigravity uses the pool" gives agy `HTTPS_PROXY`,
/// `AGY_CA_CERT` and the rest), and every other way agy signs in without
/// one: an API key, or Application Default Credentials (`AGY_ADC_AUTH`,
/// and the file `GOOGLE_APPLICATION_CREDENTIALS` names).
const AGY_UNSET: &[&str] = &[
    "GNOME_KEYRING_CONTROL",
    "HTTPS_PROXY",
    "HTTP_PROXY",
    "ALL_PROXY",
    "https_proxy",
    "http_proxy",
    "all_proxy",
    "SSL_CERT_FILE",
    "AGY_CA_CERT",
    "GEMINI_API_KEY",
    "GOOGLE_API_KEY",
    "AGY_ADC_AUTH",
    "GOOGLE_APPLICATION_CREDENTIALS",
];

/// The socket a client that's to find no session bus is told is its bus:
/// one in its own directory, never made ([`no_bus_address`]).
const NO_BUS: &str = "no-dbus";

/// The session bus address for a client kept in `dir` that's to find no
/// bus, nor a keyring on one (a separate Antigravity sign-in; a capture's
/// sandbox, `capture`): a socket there that's never made. Not an empty
/// address: agy's D-Bus (godbus) takes that for none given and finds the
/// session's own at `/run/user/UID/bus`, and on it the keyring and the
/// computer's sign-in, so "Sign In to Antigravity Separately…" opened the
/// one agy already had (Ben, 2026-11-28; agy 1.5.18, run with each: the
/// empty address reached a bus there and signed in from its keyring, this
/// one "Failed to load stored token from keyring, falling back to file").
pub fn no_bus_address(dir: &Path) -> String {
    format!("unix:path={}", bus_value(&dir.join(NO_BUS)))
}

/// `path` as a value in a D-Bus address: every byte the specification
/// doesn't leave as it is, %-escaped.
fn bus_value(path: &Path) -> String {
    use std::{fmt::Write as _, os::unix::ffi::OsStrExt as _};
    let mut out = String::new();
    for &b in path.as_os_str().as_bytes() {
        if b.is_ascii_alphanumeric() || b"-_/\\.*".contains(&b) {
            out.push(b as char);
        } else {
            let _ = write!(out, "%{b:02x}");
        }
    }
    out
}

/// Where a separate Antigravity sign-in's opener is, in its directory:
/// first on agy's PATH, so the `xdg-open` agy opens its sign-in page with
/// is Token Pool ([`opening`]).
const OPENER_DIR: &str = "bin";

/// The variable a separate Antigravity sign-in's opener finds the
/// person's own environment in: a JSON object of each of `PERSON` and its
/// value, null where they have none.
pub const OPENER_ENV: &str = "OLO_POOL_OPENER_ENV";

/// What a separate Antigravity sign-in has of its own that its opener
/// gives back to the page it opens, so the page is the person's: their
/// browser, as Default Apps has it (found under their XDG directories),
/// with its own profile, on their session bus. Opened with agy's, it was
/// a new browser in the sign-in's home (Olo's Chrome keeps its profile in
/// `$XDG_CONFIG_HOME/google-chrome`: an empty one, removed with the
/// directory under the browser showing it). Nothing secret: no key or
/// proxy's password goes into `OPENER_ENV`.
const PERSON: &[&str] = &["PATH", "HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME", "XDG_STATE_HOME", "DBUS_SESSION_BUS_ADDRESS", "BROWSER"];

/// The person's own `PERSON`, as this process has them (Token Pool's,
/// the session's, as Terminal has them). One that isn't text can't go
/// back whole, and isn't changed to be: the sign-in isn't begun.
fn person() -> Result<Vec<(&'static str, Option<String>)>, String> {
    PERSON
        .iter()
        .map(|&k| match std::env::var_os(k) {
            None => Ok((k, None)),
            Some(v) => v.into_string().map(|v| (k, Some(v))).map_err(|_| format!("Your {k} isn’t text, so a sign-in couldn’t give it back to your browser.")),
        })
        .collect()
}

/// The command that signs `p`'s client in, keeping it in `dir` (run in
/// Terminal, where the person can follow it). Claude Code and Codex are
/// told where to keep it, and open their pages as the person does.
/// Antigravity has no setting for where it keeps its sign-in, so it's
/// given a home of its own (and XDG directories: agy keeps its plugins'
/// secrets in the config one), a session bus that isn't there (`NO_BUS`),
/// so no keyring is found, none of `AGY_UNSET`, and, first on its PATH and
/// as its `BROWSER`, the `xdg-open` it opens its sign-in page with, Token
/// Pool (`prepare_sign_in`), which opens it with the person's own
/// environment (`OPENER_ENV`). The display's (`WAYLAND_DISPLAY`,
/// `XDG_RUNTIME_DIR`) it keeps. agy is found where the person has it
/// before its home is changed (on the PATH, or in `~/.local/bin`, which
/// the service's PATH may lack): missing, the sign-in isn't begun (Terminal
/// would end at once, and the wait for it last twenty minutes).
pub fn sign_in_command(p: Provider, dir: &Path) -> Result<Vec<String>, String> {
    sign_in_command_with(p, dir, person, || crate::capture::on_path("agy"))
}

/// [`sign_in_command`], the person's `PERSON` and agy asked for as
/// they're needed: Antigravity's only. (Claude Code's and Codex's had
/// come to fail on a `BROWSER` that isn't text, which neither uses:
/// Astra's review, 2026-11-28.)
fn sign_in_command_with(p: Provider, dir: &Path, person: impl FnOnce() -> Result<Vec<(&'static str, Option<String>)>, String>, agy: impl FnOnce() -> Option<PathBuf>) -> Result<Vec<String>, String> {
    match p {
        Provider::Antigravity => {
            let agy = agy().ok_or("Antigravity’s agy isn’t installed here (on your PATH or in ~/.local/bin).")?;
            command_for(p, dir, &person()?, Some(&agy))
        }
        Provider::Claude | Provider::Codex => command_for(p, dir, &[], None),
    }
}

/// [`sign_in_command`], for a person with these `PERSON`, agy at `agy`.
fn command_for(p: Provider, dir: &Path, person: &[(&str, Option<String>)], agy: Option<&Path>) -> Result<Vec<String>, String> {
    let d = dir.display();
    Ok(match p {
        Provider::Claude => vec!["/usr/bin/env".into(), format!("CLAUDE_CONFIG_DIR={d}"), "claude".into(), "auth".into(), "login".into()],
        Provider::Codex => vec!["/usr/bin/env".into(), format!("CODEX_HOME={d}"), "codex".into(), "-c".into(), "cli_auth_credentials_store=\"file\"".into(), "login".into()],
        Provider::Antigravity => {
            let agy = agy.and_then(Path::to_str).ok_or("Antigravity’s agy isn’t installed here.")?;
            let bin = dir.join(OPENER_DIR);
            let bin = bin.to_str().filter(|b| !b.contains(':')).ok_or_else(|| format!("{} can’t go on a PATH.", bin.display()))?;
            let path = person.iter().find(|(k, _)| *k == "PATH").and_then(|(_, v)| v.clone()).filter(|p| !p.is_empty()).unwrap_or_else(|| "/usr/local/bin:/usr/bin:/bin".into());
            let theirs: Map<String, Value> = person.iter().map(|(k, v)| ((*k).to_owned(), v.clone().map_or(Value::Null, Value::String))).collect();
            let mut argv = vec!["/usr/bin/env".to_owned()];
            for v in AGY_UNSET {
                argv.extend(["-u".to_owned(), (*v).to_owned()]);
            }
            argv.extend([
                format!("HOME={d}"),
                format!("XDG_CONFIG_HOME={d}/.config"),
                format!("XDG_DATA_HOME={d}/.local/share"),
                format!("XDG_CACHE_HOME={d}/.cache"),
                format!("XDG_STATE_HOME={d}/.local/state"),
                format!("DBUS_SESSION_BUS_ADDRESS={}", no_bus_address(dir)),
                format!("PATH={bin}:{path}"),
                format!("BROWSER={bin}/xdg-open"),
                format!("{OPENER_ENV}={}", Value::Object(theirs)),
                agy.to_owned(),
            ]);
            argv
        }
    })
}

/// What `p`'s sign-in in `dir` needs there before it runs: Antigravity's,
/// its opener (`bin/xdg-open`, a link to Token Pool's own program, which
/// started by that name does [`opening`]'s). It goes with the directory
/// (`discard`: the link, never what it names).
pub fn prepare_sign_in(p: Provider, dir: &Path) -> Result<(), String> {
    match p {
        Provider::Claude | Provider::Codex => Ok(()),
        Provider::Antigravity => {
            let exe = std::env::current_exe().map_err(|e| format!("Couldn’t find Token Pool’s program: {e}"))?;
            make_opener(dir, &exe)
        }
    }
}

/// `dir`'s `bin/xdg-open`, a link to `exe` (to the program at its path,
/// where an upgrade replaced the one running: the new one).
fn make_opener(dir: &Path, exe: &Path) -> Result<(), String> {
    use std::os::unix::fs::DirBuilderExt;
    let exe = exe.to_str().and_then(|s| s.strip_suffix(" (deleted)")).map_or_else(|| exe.to_owned(), PathBuf::from);
    let bin = dir.join(OPENER_DIR);
    std::fs::DirBuilder::new().mode(0o700).create(&bin).map_err(|e| format!("Couldn’t make {}: {e}", bin.display()))?;
    let link = bin.join("xdg-open");
    std::os::unix::fs::symlink(&exe, &link).map_err(|e| format!("Couldn’t make {}: {e}", link.display()))
}

/// A separate sign-in for `p` begun: its directory, made with what it
/// needs there, and the command that signs the client in (for Terminal).
/// It makes files: a worker thread's. Nothing is left of one that can't
/// begin.
pub fn begin_separate(p: Provider) -> Result<(PathBuf, Vec<String>), String> {
    let dir = separate_dir()?;
    match prepare_sign_in(p, &dir).and_then(|()| sign_in_command(p, &dir)) {
        Ok(command) => Ok((dir, command)),
        Err(e) => {
            discard(&dir);
            Err(e)
        }
    }
}

/// What Token Pool started as a separate Antigravity sign-in's `xdg-open`
/// does: puts the person's own `PERSON` back (a value, or None: unset),
/// takes `OPENER_ENV` away, and opens `urls`.
#[derive(Debug, PartialEq)]
pub struct Opening {
    pub env: Vec<(String, Option<String>)>,
    pub urls: Vec<String>,
}

/// What Token Pool started as `xdg-open ARG…` (by agy, for its sign-in
/// page) is to do. Only for a sign-in of its own (`carrier`, what
/// `OPENER_ENV` holds, there and whole: of it, only `PERSON` is taken),
/// and only web pages: anything else (a file, another scheme, an option)
/// is refused, and nothing opened.
pub fn opening(carrier: Option<&str>, args: &[String]) -> Result<Opening, String> {
    let carrier = carrier.ok_or("only the page a sign-in in Token Pool asks for is opened")?;
    let unread = || format!("{OPENER_ENV} isn’t what Token Pool gave");
    let given: Map<String, Value> = serde_json::from_str(carrier).map_err(|_| unread())?;
    let mut env = Vec::new();
    for k in PERSON {
        let v = match given.get(*k) {
            // (No NUL: the environment can't hold one.)
            Some(Value::String(v)) if !v.contains('\0') => Some(v.clone()),
            Some(Value::Null) => None,
            _ => return Err(unread()),
        };
        env.push(((*k).to_owned(), v));
    }
    env.push((OPENER_ENV.to_owned(), None));
    if args.is_empty() {
        return Err("no page to open".into());
    }
    if let Some(other) = args.iter().find(|a| !is_web_page(a)) {
        return Err(format!("only a web page (http or https) is opened, not {}", shown(other)));
    }
    Ok(Opening { env, urls: args.to_vec() })
}

/// Whether `s` is a web page's address: `http://` or `https://` (in any
/// case) and a host, as the URL parser reads it; no space, control
/// character or backslash in it, and an authority written out where the
/// host goes (no slash there, which the parser skips over to find one).
fn is_web_page(s: &str) -> bool {
    let Some((scheme, _)) = s.split_once("://") else { return false };
    let web = scheme.eq_ignore_ascii_case("https") || scheme.eq_ignore_ascii_case("http");
    web && authority(s).is_some() && !s.chars().any(|c| c.is_whitespace() || c.is_control() || c == '\\') && reqwest::Url::parse(s).is_ok_and(|u| u.host_str().is_some_and(|h| !h.is_empty()))
}

/// The authority written out in `s` right after its `http://` or
/// `https://`, its user and password left out: none where there's none
/// there (`https:///x`, or a backslash where it goes: the URL parser makes
/// one of what follows, a path's first part, Astra's review, 2026-11-28),
/// where the scheme isn't all that's before the `://` (`https:x/?u=https://y`:
/// the parser's host is `x`, a path's), or where it has a space or a
/// control character in it (`https://\t/x`: the parser drops it, and
/// makes a host of `x`; Astra's and Neon's reviews, 2026-11-28).
fn authority(s: &str) -> Option<&str> {
    let (scheme, rest) = s.split_once("://")?;
    if !(scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")) {
        return None;
    }
    let end = rest.find(['/', '\\', '?', '#']).unwrap_or(rest.len());
    let auth = &rest[..end];
    if auth.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    let host = auth.rsplit_once('@').map_or(auth, |(_, h)| h);
    (!host.is_empty() && !rest.starts_with(['/', '\\'])).then_some(host)
}

/// What's said of `s` where it's refused or couldn't be opened: its scheme
/// and the host written out in it, never its password, path, query or
/// fragment (an OAuth request's `state` and `login_hint` stay out of
/// what's logged), nor anything the URL parser would make a host of.
pub fn shown(s: &str) -> String {
    match reqwest::Url::parse(s) {
        Ok(u) => match (u.scheme(), u.host_str(), authority(s)) {
            (scheme @ ("http" | "https"), Some(host), Some(_)) => format!("{scheme}://{host}{}", u.port().map(|p| format!(":{p}")).unwrap_or_default()),
            (scheme, ..) => format!("a {scheme}: address"),
        },
        Err(_) => "something else".into(),
    }
}

/// The separate sign-in in `dir`, if it's there yet (and whole: one being
/// written fails to parse, `Unread::NotYet`, and is read again at the next
/// look). Its files are read as a private directory's: no link followed.
pub fn read_separate(p: Provider, dir: &Path) -> Result<Found, Unread> {
    match p {
        Provider::Claude => read_claude_with(dir, None, false),
        Provider::Codex => read_codex_with(dir, false),
        Provider::Antigravity => read_agy_with(&dir.join(".gemini/antigravity-cli"), false),
    }
}

/// Remove a separate sign-in's directory, once read (or given up).
pub fn discard(dir: &Path) {
    if dir.file_name().is_some_and(|n| n.to_string_lossy().starts_with("olo-pool-signin-")) {
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};

    fn scratch(name: &str) -> tempfile::TempDir {
        tempfile::Builder::new().prefix(&format!("olo-pool-import-{name}-")).tempdir_in("/var/tmp").unwrap()
    }

    /// agy's `StoredToken` for `email`, as it writes it.
    fn agy_token(access: &str, email: &str) -> String {
        format!(r#"{{"token":{{"access_token":"{access}","token_type":"Bearer","refresh_token":"1//0{access}","expiry":"2026-12-01T00:00:00Z"}},"auth_method":"consumer","id_token_email":"{email}"}}"#)
    }

    /// `dir` with agy's file in it, holding `token`.
    fn agy_file(dir: &Path, token: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(AGY_FILE), token).unwrap();
    }

    #[test]
    fn reads_both() {
        let dir = scratch("both");
        let c = dir.path().join("c");
        std::fs::create_dir_all(&c).unwrap();
        std::fs::write(c.join(".credentials.json"), r#"{"claudeAiOauth":{"accessToken":"a","refreshToken":"r","expiresAt":1900000000000,"subscriptionType":"max"}}"#).unwrap();
        std::fs::write(c.join(".claude.json"), r#"{"oauthAccount":{"emailAddress":"sam@example.com"}}"#).unwrap();
        let f = read_claude(&c, None).unwrap();
        assert_eq!((f.credential.expires_at, f.email.as_deref(), f.name().as_str()), (Some(1_900_000_000), Some("sam@example.com"), "sam"));
        let x = dir.path().join("x");
        std::fs::create_dir_all(&x).unwrap();
        let claims = URL_SAFE_NO_PAD.encode(r#"{"email":"lee@example.com","https://api.openai.com/auth":{"chatgpt_plan_type":"pro","chatgpt_account_id":"acc"}}"#);
        let access = format!("h.{}.s", URL_SAFE_NO_PAD.encode(r#"{"exp":1800000000}"#));
        std::fs::write(x.join("auth.json"), format!(r#"{{"tokens":{{"access_token":"{access}","refresh_token":"r","id_token":"h.{claims}.s"}}}}"#)).unwrap();
        let f = read_codex(&x).unwrap();
        assert_eq!((f.credential.expires_at, f.credential.account_id.as_deref(), f.plan.as_deref()), (Some(1_800_000_000), Some("acc"), Some("pro")));
        assert!(read_codex(&c).is_err());
        assert_eq!(read_codex(&dir.path().join("none")).unwrap_err(), NO_SIGN_IN);

        // Antigravity's file, and the separate sign-in's reader.
        let agy_home = dir.path().join("agy-home");
        agy_file(&agy_home.join(".gemini/antigravity-cli"), &agy_token("ya29.tok", "dev@example.com"));
        let fa = read_separate(Provider::Antigravity, &agy_home).unwrap();
        assert_eq!(
            (fa.provider, fa.credential.access.as_str(), fa.credential.refresh.as_deref(), fa.email.as_deref(), fa.name().as_str()),
            (Provider::Antigravity, "ya29.tok", Some("1//0ya29.tok"), Some("dev@example.com"), "dev")
        );
        assert_eq!(fa.credential.expires_at, Some(1_796_083_200));
    }

    #[test]
    fn agy_token_read_as_agy_keeps_it() {
        // Words trimmed, nothing empty kept; the account from an ID token
        // where agy kept no email.
        let id = format!("h.{}.s", URL_SAFE_NO_PAD.encode(r#"{"email":"jwt@example.com"}"#));
        let f = parse_agy_token(format!(r#"{{"token":{{"access_token":" ya29.a ","refresh_token":"","id_token":"{id}"}},"id_token_email":" "}}"#).as_bytes(), "it").unwrap();
        assert_eq!((f.credential.access.as_str(), f.credential.refresh, f.email.as_deref(), f.credential.expires_at), ("ya29.a", None, Some("jwt@example.com"), None));
        let f = parse_agy_token(format!(r#"{{"token":{{"access_token":"ya29.b"}},"id_token":"{id}"}}"#).as_bytes(), "it").unwrap();
        assert_eq!(f.email.as_deref(), Some("jwt@example.com"));
        // No email at all: named for the client.
        let f = parse_agy_token(br#"{"token":{"access_token":"ya29.c"}}"#, "it").unwrap();
        assert_eq!((f.email.as_deref(), f.name().as_str()), (None, "Antigravity"));
        // What isn't agy's JSON, and a sign-in with no OAuth token in it.
        assert_eq!(parse_agy_token(b"go-keyring-base64:e30=", "The keyring's").unwrap_err(), "The keyring's isn’t a sign-in this version understands.");
        assert_eq!(parse_agy_token(b"", "It").unwrap_err(), "It isn’t a sign-in this version understands.");
        assert_eq!(parse_agy_token(br#"{"token":{"access_token":"  "},"auth_method":"gcp"}"#, "it").unwrap_err(), "Antigravity has no OAuth access token there: sign in first.");
        assert_eq!(parse_agy_token(b"[]", "it").unwrap_err(), "Antigravity has no OAuth access token there: sign in first.");
    }

    #[test]
    fn agy_keyring_first_then_its_file() {
        let s = scratch("agy-here");
        let (with_file, without) = (s.path().join("with"), s.path().join("without"));
        agy_file(&with_file, &agy_token("ya29.file", "file@example.com"));
        std::fs::create_dir_all(&without).unwrap();
        let kept = || Keyring::Kept(agy_token("ya29.keyring", "keyring@example.com").into_bytes());
        let email = |r: Result<Found, String>| r.map(|f| f.email.unwrap_or_default());

        // The keyring's first, as agy's own: the file's is older then.
        assert_eq!(email(agy_here(kept(), &with_file)).as_deref(), Ok("keyring@example.com"));
        assert_eq!(email(agy_here(kept(), &without)).as_deref(), Ok("keyring@example.com"));
        // A file that can't be read doesn't matter while the keyring's can.
        let broken = s.path().join("broken");
        agy_file(&broken, "{\"token\":");
        assert_eq!(email(agy_here(kept(), &broken)).as_deref(), Ok("keyring@example.com"));

        // No keyring, or nothing in it: the file (a computer with no
        // session bus), else not signed in.
        assert_eq!(email(agy_here(Keyring::Absent, &with_file)).as_deref(), Ok("file@example.com"));
        assert_eq!(agy_here(Keyring::Absent, &without).unwrap_err(), AGY_SIGNED_OUT);
        assert!(agy_here(Keyring::Absent, &broken).unwrap_err().ends_with("antigravity-oauth-token isn’t a sign-in this version understands."));

        // Locked: the file if there is one, else that it's locked (never
        // "isn't signed in").
        assert_eq!(email(agy_here(Keyring::Locked, &with_file)).as_deref(), Ok("file@example.com"));
        assert_eq!(agy_here(Keyring::Locked, &without).unwrap_err(), AGY_LOCKED);
        assert_eq!(agy_here(Keyring::Locked, &broken).unwrap_err(), AGY_LOCKED);

        // Not to be asked: the file, else why.
        assert_eq!(email(agy_here(Keyring::Failed("timed out".into()), &with_file)).as_deref(), Ok("file@example.com"));
        assert_eq!(agy_here(Keyring::Failed("timed out".into()), &without).unwrap_err(), "Couldn’t ask the login keyring for Antigravity’s sign-in: timed out");

        // What it keeps unreadable (damaged, empty, no OAuth token): the
        // file, else what's wrong with it.
        for unreadable in [&b"{\"token\":{\"access"[..], b"", br#"{"token":{},"auth_method":"gcp"}"#] {
            assert_eq!(email(agy_here(Keyring::Kept(unreadable.to_vec()), &with_file)).as_deref(), Ok("file@example.com"));
        }
        assert_eq!(agy_here(Keyring::Kept(b"\x00garbled".to_vec()), &without).unwrap_err(), "What Antigravity keeps in the login keyring isn’t a sign-in this version understands.");
        assert_eq!(agy_here(Keyring::Kept(br#"{"token":{}}"#.to_vec()), &without).unwrap_err(), "Antigravity has no OAuth access token there: sign in first.");

        // Bigger than a sign-in (never taken in): the file, else that.
        assert_eq!(email(agy_here(Keyring::TooBig, &with_file)).as_deref(), Ok("file@example.com"));
        assert_eq!(agy_here(Keyring::TooBig, &without).unwrap_err(), "What Antigravity keeps in the login keyring is too big to be a sign-in.");
    }

    #[test]
    fn a_test_never_asks_the_sessions_keyring() {
        let asked = agy_keyring();
        assert!(matches!(asked.keyring, Keyring::Absent) && !asked.unlocked);
    }

    /// agy's item, as go-keyring stores it (agy 1.5.18: what it read its
    /// sign-in back from).
    #[test]
    fn agy_keyring_is_go_keyrings_item() {
        assert_eq!(AGY_KEYRING, [("service", "gemini"), ("username", "antigravity")]);
    }

    /// A keyring of a test's own: what it says, and what it was asked.
    #[derive(Default)]
    struct Fake {
        absent: bool,
        locked: bool,
        /// Its unlock prompt's answer (None: it couldn't be asked).
        opens: Option<bool>,
        /// What it keeps (None: its own prompt was dismissed).
        keeps: Option<Option<Vec<u8>>>,
        /// What it keeps is bigger than a sign-in.
        too_big: bool,
        asked: std::cell::RefCell<Vec<&'static str>>,
    }

    impl Secrets for Fake {
        fn present(&self) -> bool {
            !self.absent
        }

        fn locked(&self) -> bool {
            self.locked
        }

        fn unlock(&self) -> Result<bool, String> {
            self.asked.borrow_mut().push("unlock");
            self.opens.ok_or_else(|| "no answer".to_owned())
        }

        fn lookup(&self, _: &[(&str, &str)]) -> Result<Option<Vec<u8>>, Shut> {
            self.asked.borrow_mut().push("lookup");
            if self.too_big {
                return Err(Shut::TooBig);
            }
            self.keeps.clone().ok_or(Shut::Locked)
        }
    }

    #[test]
    fn a_locked_keyring_is_asked_to_open_once() {
        let token = agy_token("ya29.k", "k@example.com").into_bytes();
        // Olo's, locked: asked to open once, before anything else; Not
        // Now, and nothing more is asked (no second prompt).
        let f = Fake { locked: true, opens: Some(false), keeps: Some(Some(token.clone())), ..Fake::default() };
        let asked = ask_keyring_with(&f, AGY_KEYRING);
        assert!(matches!(asked.keyring, Keyring::Locked) && !asked.unlocked);
        assert_eq!(*f.asked.borrow(), ["unlock"]);
        // Opened: then asked for it, once; and said to be opened, so the
        // pool's own accounts are opened too.
        let f = Fake { locked: true, opens: Some(true), keeps: Some(Some(token.clone())), ..Fake::default() };
        let asked = ask_keyring_with(&f, AGY_KEYRING);
        assert!(matches!(asked.keyring, Keyring::Kept(b) if b == token) && asked.unlocked);
        assert_eq!(*f.asked.borrow(), ["unlock", "lookup"]);
        // Opened with no sign-in in it: still said to be opened (the
        // vault's wake isn't left to its next try).
        let f = Fake { locked: true, opens: Some(true), keeps: Some(None), ..Fake::default() };
        let asked = ask_keyring_with(&f, AGY_KEYRING);
        assert!(matches!(asked.keyring, Keyring::Absent) && asked.unlocked);
        // Couldn't be asked to open: why, and nothing else asked.
        let f = Fake { locked: true, ..Fake::default() };
        assert!(matches!(ask_keyring_with(&f, AGY_KEYRING).keyring, Keyring::Failed(e) if e == "no answer"));
        assert_eq!(*f.asked.borrow(), ["unlock"]);
        // Another keyring, its own prompt dismissed: locked.
        let f = Fake::default();
        assert!(matches!(ask_keyring_with(&f, AGY_KEYRING).keyring, Keyring::Locked));
        assert_eq!(*f.asked.borrow(), ["lookup"]);
        // Bigger than a sign-in: never taken in.
        let f = Fake { too_big: true, ..Fake::default() };
        assert!(matches!(ask_keyring_with(&f, AGY_KEYRING).keyring, Keyring::TooBig));
        // Nothing kept; no keyring at all (nothing asked).
        let f = Fake { keeps: Some(None), ..Fake::default() };
        assert!(matches!(ask_keyring_with(&f, AGY_KEYRING).keyring, Keyring::Absent));
        let f = Fake { absent: true, locked: true, ..Fake::default() };
        assert!(matches!(ask_keyring_with(&f, AGY_KEYRING).keyring, Keyring::Absent));
        assert!(f.asked.borrow().is_empty());
    }

    /// What the keyring said, read: too big; locked, as its own prompt
    /// dismissed says it or as the Secret Service's `IsLocked` does
    /// (Neon's review, 2026-11-28: that one was "couldn't ask"); else
    /// locked only if Olo's is.
    #[test]
    fn what_the_keyring_said_is_read() {
        let said = |too_big, failure, name| Said { too_big, failure, name, text: "what it said".to_owned() };
        let never = || panic!("asked whether Olo's keyring is locked");
        assert!(matches!(shut(said(true, Some("too big"), None), never), Shut::TooBig));
        assert!(matches!(shut(said(false, Some("the keyring stayed locked"), None), never), Shut::Locked));
        assert!(matches!(shut(said(false, None, Some("org.freedesktop.Secret.Error.IsLocked")), never), Shut::Locked));
        assert!(matches!(shut(said(false, None, Some("org.freedesktop.DBus.Error.NoReply")), || false), Shut::Failed(t) if t == "what it said"));
        assert!(matches!(shut(said(false, None, Some("org.freedesktop.DBus.Error.NoReply")), || true), Shut::Locked));
    }

    #[test]
    fn bus_values_escaped() {
        assert_eq!(bus_value(Path::new("/run/user/1000/olo-pool-signin-ab12/no-dbus")), "/run/user/1000/olo-pool-signin-ab12/no-dbus");
        assert_eq!(bus_value(Path::new("/tmp/a b,c;d=e%f/no-dbus")), "/tmp/a%20b%2cc%3bd%3de%25f/no-dbus");
        assert_eq!(no_bus_address(Path::new("/run/user/1000/olo-pool-signin-ab12")), "unix:path=/run/user/1000/olo-pool-signin-ab12/no-dbus");
    }

    #[test]
    fn a_separate_sign_in_goes_where_a_path_can_hold_it() {
        use std::{ffi::OsStr, os::unix::ffi::OsStrExt as _};
        assert_eq!(separate_base(Some(OsStr::new("/run/user/1000"))), Path::new("/run/user/1000"));
        // None, not absolute, a colon (its `bin` on a PATH), not text:
        // under /var/tmp, never /tmp.
        for elsewhere in [None, Some(OsStr::new("")), Some(OsStr::new("run/user/1000")), Some(OsStr::new("/run/user/10:00")), Some(OsStr::from_bytes(b"/run/\xff"))] {
            assert_eq!(separate_base(elsewhere), Path::new("/var/tmp"), "{elsewhere:?}");
        }
    }

    #[test]
    fn a_sign_ins_file_is_read_no_further_than_one_holds() {
        let s = scratch("bytes");
        // A pipe put there: refused at once, never waited on.
        let fifo = s.path().join("fifo");
        let c = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: a NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        assert!(bytes(&fifo, true).unwrap_err().ends_with("isn’t a file."));
        assert!(bytes(s.path(), true).unwrap_err().ends_with("isn’t a file."));
        let big = s.path().join("big");
        std::fs::write(&big, vec![b' '; MAX_SIGN_IN as usize + 1]).unwrap();
        assert!(bytes(&big, true).unwrap_err().ends_with("is too big to be a sign-in."));
        std::fs::write(&big, vec![b' '; MAX_SIGN_IN as usize]).unwrap();
        assert_eq!(bytes(&big, true).unwrap().map(|b| b.len()), Some(MAX_SIGN_IN as usize));
        assert_eq!(bytes(&s.path().join("none"), true).unwrap(), None);
        // A link: followed for the person's own client's files, never in a
        // sign-in's private directory.
        let link = s.path().join("link");
        std::os::unix::fs::symlink(&big, &link).unwrap();
        assert!(bytes(&link, true).unwrap().is_some());
        assert!(bytes(&link, false).unwrap_err().starts_with("Couldn’t read"));
    }

    /// A separate Claude Code sign-in's account is its own: never the one
    /// its directory's parent keeps details of (the runtime directory's,
    /// or /var/tmp's, `.claude.json`: anyone's), and its details are read
    /// as its sign-in is (a pipe put there holds nothing up). The default
    /// config directory's are in the home directory (`home_meta`).
    #[test]
    fn a_separate_claude_sign_in_is_its_own() {
        let s = scratch("claude-own");
        std::fs::write(s.path().join(".claude.json"), r#"{"oauthAccount":{"emailAddress":"someone@example.com","organizationUuid":"theirs"}}"#).unwrap();
        let dir = s.path().join("olo-pool-signin-cl1");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(".credentials.json"), r#"{"claudeAiOauth":{"accessToken":"a","refreshToken":"r","subscriptionType":"pro"}}"#).unwrap();
        let f = read_separate(Provider::Claude, &dir).unwrap();
        assert_eq!((f.email, f.org), (None, None), "a parent's account taken");
        // Its own details, once written, are.
        let fifo = dir.join(".claude.json");
        let c = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: a NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        assert_eq!(read_separate(Provider::Claude, &dir).unwrap().email, None, "a pipe read as details");
        std::fs::remove_file(&fifo).unwrap();
        std::fs::write(&fifo, r#"{"oauthAccount":{"emailAddress":"new@example.com"}}"#).unwrap();
        assert_eq!(read_separate(Provider::Claude, &dir).unwrap().email.as_deref(), Some("new@example.com"));
        // This computer's, in the default config directory: its home's.
        let home = s.path().join("home");
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        std::fs::write(home.join(".claude/.credentials.json"), r#"{"claudeAiOauth":{"accessToken":"a"}}"#).unwrap();
        std::fs::write(home.join(".claude.json"), r#"{"oauthAccount":{"emailAddress":"me@example.com"}}"#).unwrap();
        assert_eq!(read_claude(&home.join(".claude"), Some(&home.join(".claude.json"))).unwrap().email.as_deref(), Some("me@example.com"));
        assert_eq!(read_claude(&home.join(".claude"), None).unwrap().email, None);
        // An old copy in the default config directory: the home's first,
        // as Terminal reads it; the old one only where the home's has no
        // account (Astra's review, 2026-11-28).
        std::fs::write(home.join(".claude/.claude.json"), r#"{"oauthAccount":{"emailAddress":"old@example.com"}}"#).unwrap();
        assert_eq!(read_claude(&home.join(".claude"), Some(&home.join(".claude.json"))).unwrap().email.as_deref(), Some("me@example.com"));
        std::fs::write(home.join(".claude.json"), r#"{"projects":{}}"#).unwrap();
        assert_eq!(read_claude(&home.join(".claude"), Some(&home.join(".claude.json"))).unwrap().email.as_deref(), Some("old@example.com"));
        // This computer's through a link (a dotfiles folder's), followed;
        // a separate sign-in's, never.
        let kept = s.path().join("dotfiles.json");
        std::fs::write(&kept, r#"{"oauthAccount":{"emailAddress":"linked@example.com"}}"#).unwrap();
        let config = s.path().join("config");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(config.join(".credentials.json"), r#"{"claudeAiOauth":{"accessToken":"a"}}"#).unwrap();
        std::os::unix::fs::symlink(&kept, config.join(".claude.json")).unwrap();
        assert_eq!(read_claude(&config, None).unwrap().email.as_deref(), Some("linked@example.com"));
        std::fs::remove_file(dir.join(".claude.json")).unwrap();
        std::os::unix::fs::symlink(&kept, dir.join(".claude.json")).unwrap();
        assert_eq!(read_separate(Provider::Claude, &dir).unwrap().email, None, "a link followed in a private directory");
    }

    /// What a separate sign-in's directory has, read as the wait reads
    /// it: none yet, or one being written, looked at again; one never to
    /// be read (too big, not a file, no access token) ends the wait at
    /// once with why (Astra's review, 2026-11-28: a file too big was
    /// looked at for twenty minutes, and the wait said it timed out).
    #[test]
    fn a_sign_in_never_to_be_read_ends_the_wait() {
        let s = scratch("unread");
        let dir = s.path().join("olo-pool-signin-u1");
        let agy = dir.join(".gemini/antigravity-cli");
        std::fs::create_dir_all(&agy).unwrap();
        assert_eq!(read_separate(Provider::Antigravity, &dir).unwrap_err(), Unread::NotYet(NO_SIGN_IN.to_owned()));
        std::fs::write(agy.join(AGY_FILE), "{\"token\":").unwrap();
        assert!(matches!(read_separate(Provider::Antigravity, &dir), Err(Unread::NotYet(_))));
        std::fs::write(agy.join(AGY_FILE), vec![b' '; MAX_SIGN_IN as usize + 1]).unwrap();
        assert!(matches!(read_separate(Provider::Antigravity, &dir), Err(Unread::Never(e)) if e.ends_with("is too big to be a sign-in.")));
        std::fs::write(agy.join(AGY_FILE), r#"{"token":{}}"#).unwrap();
        assert!(matches!(read_separate(Provider::Antigravity, &dir), Err(Unread::Never(_))));
        std::fs::remove_file(agy.join(AGY_FILE)).unwrap();
        std::fs::create_dir(agy.join(AGY_FILE)).unwrap();
        assert!(matches!(read_separate(Provider::Antigravity, &dir), Err(Unread::Never(e)) if e.ends_with("isn’t a file.")));
        // Claude Code's and Codex's, with API keys: never.
        std::fs::write(dir.join(".credentials.json"), r#"{"claudeAiOauth":{}}"#).unwrap();
        assert!(matches!(read_separate(Provider::Claude, &dir), Err(Unread::Never(_))));
        std::fs::write(dir.join("auth.json"), r#"{"tokens":{}}"#).unwrap();
        assert!(matches!(read_separate(Provider::Codex, &dir), Err(Unread::Never(_))));
    }

    /// A person's `PERSON`, as the session gives them (no XDG directory
    /// of their own set, as is usual).
    fn sam() -> Vec<(&'static str, Option<String>)> {
        PERSON
            .iter()
            .map(|&k| {
                let v = match k {
                    "PATH" => Some("/home/sam/.local/bin:/usr/bin"),
                    "HOME" => Some("/home/sam"),
                    "DBUS_SESSION_BUS_ADDRESS" => Some("unix:path=/run/user/1000/bus"),
                    _ => None,
                };
                (k, v.map(str::to_owned))
            })
            .collect()
    }

    /// Where agy is, as `capture::on_path` finds it for Sam.
    const SAMS_AGY: &str = "/home/sam/.local/bin/agy";

    #[test]
    fn agy_signs_in_apart() {
        let dir = Path::new("/run/user/1000/olo-pool-signin-ab12");
        let cmd = command_for(Provider::Antigravity, dir, &sam(), Some(Path::new(SAMS_AGY))).unwrap();
        // agy where the person has it (found before its home changed).
        assert_eq!((cmd.first().map(String::as_str), cmd.last().map(String::as_str)), (Some("/usr/bin/env"), Some(SAMS_AGY)), "{cmd:?}");
        // Each of AGY_UNSET taken away, before anything is set.
        let unset: Vec<&str> = cmd.windows(2).filter(|w| w[0] == "-u").map(|w| w[1].as_str()).collect();
        assert_eq!(unset, AGY_UNSET);
        let set: Vec<&str> = cmd[1 + 2 * AGY_UNSET.len()..cmd.len() - 1].iter().map(String::as_str).collect();
        assert_eq!(
            set,
            [
                "HOME=/run/user/1000/olo-pool-signin-ab12",
                "XDG_CONFIG_HOME=/run/user/1000/olo-pool-signin-ab12/.config",
                "XDG_DATA_HOME=/run/user/1000/olo-pool-signin-ab12/.local/share",
                "XDG_CACHE_HOME=/run/user/1000/olo-pool-signin-ab12/.cache",
                "XDG_STATE_HOME=/run/user/1000/olo-pool-signin-ab12/.local/state",
                "DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/olo-pool-signin-ab12/no-dbus",
                // Its opener first, then the person's own; and as its
                // BROWSER (Terminal's would be olo-open, in agy's home).
                "PATH=/run/user/1000/olo-pool-signin-ab12/bin:/home/sam/.local/bin:/usr/bin",
                "BROWSER=/run/user/1000/olo-pool-signin-ab12/bin/xdg-open",
                r#"OLO_POOL_OPENER_ENV={"PATH":"/home/sam/.local/bin:/usr/bin","HOME":"/home/sam","XDG_CONFIG_HOME":null,"XDG_DATA_HOME":null,"XDG_CACHE_HOME":null,"XDG_STATE_HOME":null,"DBUS_SESSION_BUS_ADDRESS":"unix:path=/run/user/1000/bus","BROWSER":null}"#,
            ]
        );
        // The display's left as they are.
        assert!(!cmd.iter().any(|a| a.contains("WAYLAND_DISPLAY") || a.contains("XDG_RUNTIME_DIR")), "{cmd:?}");
        // With no PATH of the person's, the usual one after the opener.
        let none: Vec<_> = sam().into_iter().map(|(k, v)| (k, v.filter(|_| k != "PATH"))).collect();
        assert!(command_for(Provider::Antigravity, dir, &none, Some(Path::new(SAMS_AGY))).unwrap().contains(&"PATH=/run/user/1000/olo-pool-signin-ab12/bin:/usr/local/bin:/usr/bin:/bin".to_owned()));
        // No agy, or a directory a PATH can't hold: not begun.
        assert_eq!(command_for(Provider::Antigravity, dir, &sam(), None).unwrap_err(), "Antigravity’s agy isn’t installed here.");
        assert!(command_for(Provider::Antigravity, Path::new("/run/a:b/olo-pool-signin-x"), &sam(), Some(Path::new(SAMS_AGY))).unwrap_err().ends_with("can’t go on a PATH."));
    }

    #[test]
    fn claude_and_codex_sign_in_as_they_did() {
        // Told where to keep it, nothing else: their pages open as the
        // person's (their home, their bus), with no opener of Token Pool's.
        let dir = Path::new("/run/user/1000/olo-pool-signin-cd34");
        assert_eq!(command_for(Provider::Claude, dir, &sam(), None).unwrap(), ["/usr/bin/env", "CLAUDE_CONFIG_DIR=/run/user/1000/olo-pool-signin-cd34", "claude", "auth", "login"]);
        assert_eq!(
            command_for(Provider::Codex, dir, &sam(), None).unwrap(),
            ["/usr/bin/env", "CODEX_HOME=/run/user/1000/olo-pool-signin-cd34", "codex", "-c", "cli_auth_credentials_store=\"file\"", "login"]
        );
        let s = scratch("prepare");
        for p in [Provider::Claude, Provider::Codex] {
            prepare_sign_in(p, s.path()).unwrap();
        }
        assert_eq!(std::fs::read_dir(s.path()).unwrap().count(), 0, "nothing made for Claude Code or Codex");
        // Neither asks for what only agy's opener gives back: a `BROWSER`
        // that isn't text stops Antigravity's alone (Astra's review,
        // 2026-11-28: Claude Code's and Codex's had stopped on it too).
        let refused = || Err::<Vec<(&'static str, Option<String>)>, _>("Your BROWSER isn’t text, so a sign-in couldn’t give it back to your browser.".to_owned());
        let never = || -> Option<PathBuf> { panic!("agy looked for") };
        assert!(sign_in_command_with(Provider::Claude, dir, refused, never).is_ok());
        assert!(sign_in_command_with(Provider::Codex, dir, refused, never).is_ok());
        let agy = || Some(PathBuf::from(SAMS_AGY));
        assert_eq!(sign_in_command_with(Provider::Antigravity, dir, refused, agy).unwrap_err(), "Your BROWSER isn’t text, so a sign-in couldn’t give it back to your browser.");
        assert!(sign_in_command_with(Provider::Antigravity, dir, || Ok(sam()), agy).is_ok());
        assert!(sign_in_command_with(Provider::Antigravity, dir, || Ok(sam()), || None).unwrap_err().starts_with("Antigravity’s agy isn’t installed here"));
    }

    #[test]
    fn the_opener_is_a_link_that_goes_with_the_sign_in() {
        let s = scratch("opener");
        let exe = s.path().join("olo-pool");
        std::fs::write(&exe, "").unwrap();
        let dir = s.path().join("olo-pool-signin-ef56");
        std::fs::create_dir(&dir).unwrap();
        make_opener(&dir, &exe).unwrap();
        assert_eq!(std::fs::read_link(dir.join("bin/xdg-open")).unwrap(), exe);
        // The program that was replaced while it ran: the one there now.
        let again = s.path().join("olo-pool-signin-ab78");
        std::fs::create_dir(&again).unwrap();
        make_opener(&again, Path::new(&format!("{} (deleted)", exe.display()))).unwrap();
        assert_eq!(std::fs::read_link(again.join("bin/xdg-open")).unwrap(), exe);
        // Removed with the directory, the program it names left alone.
        assert!(make_opener(&dir, &exe).is_err(), "made twice");
        discard(&dir);
        assert!(!dir.exists() && exe.exists());
        // Token Pool's own program, from its process.
        let mine = s.path().join("olo-pool-signin-mine");
        std::fs::create_dir(&mine).unwrap();
        prepare_sign_in(Provider::Antigravity, &mine).unwrap();
        assert_eq!(std::fs::read_link(mine.join("bin/xdg-open")).unwrap(), std::env::current_exe().unwrap());
    }

    #[test]
    fn the_opener_gives_the_person_back_and_opens_only_web_pages() {
        let dir = Path::new("/run/user/1000/olo-pool-signin-ab12");
        let cmd = command_for(Provider::Antigravity, dir, &sam(), Some(Path::new(SAMS_AGY))).unwrap();
        let carrier = cmd.iter().find_map(|a| a.strip_prefix("OLO_POOL_OPENER_ENV=")).unwrap();
        let url = "https://accounts.google.com/o/oauth2/auth?access_type=offline&client_id=x&redirect_uri=https%3A%2F%2Fantigravity.google%2Foauth-callback";
        let o = opening(Some(carrier), &[url.to_owned()]).unwrap();
        let want: Vec<(String, Option<String>)> = sam().into_iter().map(|(k, v)| (k.to_owned(), v)).chain([(OPENER_ENV.to_owned(), None)]).collect();
        assert_eq!(o, Opening { env: want, urls: vec![url.to_owned()] });
        for page in ["http://localhost:8080/oauth-callback?code=x", "HTTPS://accounts.google.com/"] {
            assert!(opening(Some(carrier), &[page.to_owned()]).is_ok(), "{page}");
        }
        // Nothing but a web page.
        for other in [
            "file:///etc/passwd",
            "file://localhost/etc/passwd",
            "javascript:alert(1)",
            "mailto:sam@example.com",
            "ftp://example.com/",
            "https:///etc/passwd",
            "https://",
            // (No host, as the URL parser reads them.)
            "https://?q=x",
            "http://#x",
            "https://:443/",
            "--new-window",
            "/etc/passwd",
            "https://a b",
            "https://x\ny",
            "",
        ] {
            let e = opening(Some(carrier), &[url.to_owned(), other.to_owned()]).unwrap_err();
            assert!(e.starts_with("only a web page (http or https) is opened"), "{other}: {e}");
        }
        assert_eq!(opening(Some(carrier), &[]).unwrap_err(), "no page to open");
        // What's said of one: its scheme and host, never more (a path, a
        // password, an OAuth request's state).
        assert_eq!(opening(Some(carrier), &["file:///home/sam/notes".to_owned()]).unwrap_err(), "only a web page (http or https) is opened, not a file: address");
        assert_eq!(shown("https://sam:pw@accounts.google.com:8443/o/oauth2/auth?state=s3cret&login_hint=sam%40example.com#f"), "https://accounts.google.com:8443");
        assert_eq!(shown(url), "https://accounts.google.com");
        assert_eq!(shown("--new-window"), "something else");
        // A host only where one is written out: never what the parser
        // makes of a path (three slashes), nor of a backslash where the
        // authority goes; neither is opened (Astra's review, 2026-11-28).
        // Nor where the scheme isn't all that's before the `://` (one in
        // the query: the parser's host is the path's first part), nor
        // with a space or a control character where the host goes (the
        // parser drops it, and makes a host of what follows; Astra's and
        // Neon's reviews, 2026-11-28).
        for odd in [
            "https:///s3cret.example/o?state=s3cret",
            "https://\\s3cret.example/o?state=s3cret",
            "https:\\\\s3cret.example/o",
            "https://accounts.google.com\\@s3cret.example/",
            "https:s3cret.example/o?next=https://x",
            "https:/s3cret.example/o?next=https://x",
            "HTTPS:s3cret.example/o#https://x",
            "https://\t/s3cret.example/o",
            "https://\n/s3cret.example/o",
            "https://\r/s3cret.example/o",
            "https://\u{b}/s3cret.example/o",
            "https:// /s3cret.example/o",
        ] {
            assert!(!shown(odd).contains("s3cret"), "{odd:?}: {}", shown(odd));
            assert!(opening(Some(carrier), &[odd.to_owned()]).is_err(), "{odd:?} opened");
        }
        assert_eq!(shown("https:///s3cret.example/o?state=s3cret"), "a https: address");
        // Only for a sign-in of Token Pool's, and only what it gives taken.
        assert_eq!(opening(None, &[url.to_owned()]).unwrap_err(), "only the page a sign-in in Token Pool asks for is opened");
        let all = |path: &str| {
            format!(r#"{{"PATH":{path},"HOME":null,"XDG_CONFIG_HOME":null,"XDG_DATA_HOME":null,"XDG_CACHE_HOME":null,"XDG_STATE_HOME":null,"DBUS_SESSION_BUS_ADDRESS":null,"BROWSER":null}}"#)
        };
        assert!(opening(Some(&all(r#""/usr/bin""#)), &[url.to_owned()]).is_ok());
        // (A NUL, which the environment can't hold: refused, not a panic.)
        for wrong in ["", "{", "[]", r#"{"PATH":"/usr/bin"}"#, &all("1"), &all(r#""/usr/bin\u0000x""#)] {
            assert_eq!(opening(Some(wrong), &[url.to_owned()]).unwrap_err(), "OLO_POOL_OPENER_ENV isn’t what Token Pool gave", "{wrong}");
        }
        let more = carrier.replacen('{', r#"{"LD_PRELOAD":"/tmp/x.so","#, 1);
        let o = opening(Some(&more), &[url.to_owned()]).unwrap();
        assert!(o.env.iter().all(|(k, _)| PERSON.contains(&k.as_str()) || k == OPENER_ENV), "{:?}", o.env);
    }

    /// The bus a D-Bus client of agy's (godbus) uses with `env`: the
    /// address given, unless it's empty (or "autolaunch:"), and then the
    /// one it finds, which is the session's (at `/run/user/UID/bus` for
    /// godbus; `$XDG_RUNTIME_DIR/bus` stands in for it here).
    fn godbus_finds(env: &std::collections::HashMap<String, String>) -> PathBuf {
        match env.get("DBUS_SESSION_BUS_ADDRESS").map(String::as_str) {
            Some(a) if !a.is_empty() && a != "autolaunch:" => PathBuf::from(a.strip_prefix("unix:path=").expect("a unix:path address")),
            _ => Path::new(&env["XDG_RUNTIME_DIR"]).join("bus"),
        }
    }

    /// What a stand-in wrote of its environment (`env -0`).
    fn env_in(file: &Path) -> std::collections::HashMap<String, String> {
        std::fs::read(file).unwrap().split(|b| *b == 0).filter_map(|kv| std::str::from_utf8(kv).ok()?.split_once('=').map(|(k, v)| (k.to_owned(), v.to_owned()))).collect()
    }

    /// The sign-in command run as Terminal runs it (through `/usr/bin/env`,
    /// from an environment with the session's bus, keyring and the pool's
    /// proxy in it), agy a stand-in that writes down what it was given and
    /// opens its sign-in page as agy does (`xdg-open URL`, found on PATH):
    /// it has a home of its own and none of those, so the session's bus (a
    /// stand-in listening at `$XDG_RUNTIME_DIR/bus`) is never reached, and
    /// the page goes to its opener (a stand-in for Token Pool) with the
    /// person's own environment to give back; then the token agy writes
    /// there as it finishes, read once it's whole.
    #[test]
    fn agy_sign_in_never_reaches_the_sessions_bus() {
        use std::os::unix::{fs::PermissionsExt as _, net::UnixListener};
        let s = scratch("agy-apart");
        let run = s.path().join("run");
        let bin = s.path().join("bin");
        for d in [&run, &bin] {
            std::fs::create_dir_all(d).unwrap();
        }
        let session_bus = UnixListener::bind(run.join("bus")).unwrap();
        session_bus.set_nonblocking(true).unwrap();
        let (given, opened) = (s.path().join("given"), s.path().join("opened"));
        let page = "https://accounts.google.com/o/oauth2/auth?access_type=offline&client_id=x";
        let script = |path: &Path, body: &str| {
            std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        };
        script(&bin.join("agy"), &format!("/usr/bin/env -0 > \"$OLO_TEST_GIVEN\"\nexec xdg-open '{page}'"));
        // (Token Pool's stand-in: what it was started as, with what.)
        let token_pool = s.path().join("olo-pool");
        script(&token_pool, "printf '%s\\n' \"$0\" \"$@\" > \"$OLO_TEST_OPENED.args\"\nexec /usr/bin/env -0 > \"$OLO_TEST_OPENED\"");
        let dir = run.join("olo-pool-signin-test");
        std::fs::create_dir(&dir).unwrap();
        make_opener(&dir, &token_pool).unwrap();

        let person = s.path().join("person");
        let path = format!("{}:/usr/bin:/bin", bin.display());
        let theirs: Vec<(&str, Option<String>)> = PERSON
            .iter()
            .map(|&k| {
                let v = match k {
                    "PATH" => Some(path.clone()),
                    "HOME" => Some(person.display().to_string()),
                    "DBUS_SESSION_BUS_ADDRESS" => Some(format!("unix:path={}", run.join("bus").display())),
                    _ => None,
                };
                (k, v)
            })
            .collect();
        let cmd = command_for(Provider::Antigravity, &dir, &theirs, Some(&bin.join("agy"))).unwrap();
        let ran = std::process::Command::new(&cmd[0])
            .args(&cmd[1..])
            .env_clear()
            .env("PATH", &path)
            .env("HOME", &person)
            .env("XDG_RUNTIME_DIR", &run)
            .env("XDG_STATE_HOME", person.join(".local/state"))
            .env("WAYLAND_DISPLAY", "wayland-test")
            .env("DBUS_SESSION_BUS_ADDRESS", format!("unix:path={}", run.join("bus").display()))
            // (Terminal's own opener, olo-open, which would open the page
            // in agy's home.)
            .env("BROWSER", run.join("olo-terminal-bin-wayland-test/xdg-open"))
            .env("GNOME_KEYRING_CONTROL", run.join("keyring"))
            .env("HTTPS_PROXY", "http://127.0.0.1:17892")
            .env("https_proxy", "http://127.0.0.1:17892")
            .env("AGY_CA_CERT", "/nonexistent/pool-ca.pem")
            .env("GEMINI_API_KEY", "not-a-key")
            .env("AGY_ADC_AUTH", "1")
            .env("OLO_TEST_GIVEN", &given)
            .env("OLO_TEST_OPENED", &opened)
            .status()
            .unwrap();
        assert!(ran.success());
        let env = env_in(&given);
        let d = dir.display().to_string();
        assert_eq!(env["HOME"], d);
        for (k, sub) in [("XDG_CONFIG_HOME", ".config"), ("XDG_DATA_HOME", ".local/share"), ("XDG_CACHE_HOME", ".cache"), ("XDG_STATE_HOME", ".local/state")] {
            assert_eq!(env[k], format!("{d}/{sub}"));
        }
        for v in AGY_UNSET {
            assert!(!env.contains_key(*v), "{v} reached agy");
        }
        assert_eq!((env["WAYLAND_DISPLAY"].as_str(), env["XDG_RUNTIME_DIR"].as_str()), ("wayland-test", run.to_str().unwrap()));
        assert_eq!(env["PATH"], format!("{d}/bin:{path}"));
        assert_eq!(env["BROWSER"], format!("{d}/bin/xdg-open"), "Terminal's opener reached agy");

        // The page went to the opener, first on its PATH, which has what
        // puts the person's own back (`opening`).
        let args = std::fs::read_to_string(format!("{}.args", opened.display())).unwrap();
        assert_eq!(args, format!("{d}/bin/xdg-open\n{page}\n"));
        let opener = env_in(&opened);
        let back = opening(opener.get(OPENER_ENV).map(String::as_str), &[page.to_owned()]).unwrap();
        let want: Vec<(String, Option<String>)> = theirs.iter().map(|(k, v)| ((*k).to_owned(), v.clone())).chain([(OPENER_ENV.to_owned(), None)]).collect();
        assert_eq!(back.env, want);

        // The bus agy's D-Bus would use: none that's there, in its own
        // directory; the session's never asked.
        let bus = godbus_finds(&env);
        assert_eq!(bus, dir.join(NO_BUS));
        assert_eq!(std::os::unix::net::UnixStream::connect(&bus).unwrap_err().kind(), std::io::ErrorKind::NotFound);
        assert_eq!(session_bus.accept().unwrap_err().kind(), std::io::ErrorKind::WouldBlock, "the session's bus was reached");
        // (As the command was before: the empty address found the session's.)
        let before: std::collections::HashMap<String, String> = [("DBUS_SESSION_BUS_ADDRESS", ""), ("XDG_RUNTIME_DIR", run.to_str().unwrap())].map(|(k, v)| (k.to_owned(), v.to_owned())).into();
        assert_eq!(godbus_finds(&before), run.join("bus"));

        // agy signing in writes its file there: not read while it's
        // half-written, read once it's whole.
        let agy_data = dir.join(".gemini/antigravity-cli");
        assert_eq!(read_separate(Provider::Antigravity, &dir).unwrap_err(), Unread::NotYet(NO_SIGN_IN.to_owned()));
        let token = agy_token("ya29.new", "new@example.com");
        agy_file(&agy_data, &token[..token.len() / 2]);
        assert!(read_separate(Provider::Antigravity, &dir).unwrap_err().text().ends_with("isn’t a sign-in this version understands."));
        agy_file(&agy_data, &token);
        let found = read_separate(Provider::Antigravity, &dir).unwrap();
        assert_eq!((found.credential.access.as_str(), found.email.as_deref()), ("ya29.new", Some("new@example.com")));
        discard(&dir);
        assert!(!dir.exists());
    }

    /// Antigravity's sign-in read from a real keyring through the
    /// session's way of asking ([`Session`]: Olo's own Secret Service on a
    /// bus of the test's own): kept there, it's what's read, before the
    /// file; gone, the file's. Kept under attributes of this run's own,
    /// never agy's (`AGY_KEYRING`, pinned by
    /// `agy_keyring_is_go_keyrings_item`), and taken away however the test
    /// ends: no one's sign-in can be replaced or deleted by it. The bus is
    /// named twice, so it's never the session's by mistake. Run with
    /// olo-bus and olo-secrets (`OLO_SECRETS_SERVE=1`, a key file) on a
    /// bus under /var/tmp/olo-…, with a runtime directory of the test's own
    /// there too (never the session's, whose `olo-secrets.json` says
    /// whether its keyring is locked: Neon's review, 2026-11-28):
    ///
    ///   XDG_RUNTIME_DIR=/var/tmp/olo-…/run OLO_POOL_TEST_KEYRING=unix:path=/var/tmp/olo-…/bus \
    ///     DBUS_SESSION_BUS_ADDRESS=unix:path=/var/tmp/olo-…/bus cargo test -p olo-token-pool --lib import -- --ignored
    #[cfg(feature = "desktop")]
    #[test]
    #[ignore]
    fn agy_from_a_keyring_of_its_own() {
        let bus = std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap_or_default();
        assert!(bus.starts_with("unix:path=/var/tmp/olo-") && std::env::var("OLO_POOL_TEST_KEYRING").as_deref() == Ok(bus.as_str()), "not a test's own bus: {bus}");
        let run = format!("{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
        let attrs = [("service", "olo-pool-test-agy"), ("username", run.as_str())];
        /// Taken away however the test ends.
        struct Gone<'a>(&'a [(&'a str, &'a str)]);
        impl Drop for Gone<'_> {
            fn drop(&mut self) {
                let _ = olo_system::secrets::delete(self.0);
            }
        }
        let _gone = Gone(&attrs);
        let s = scratch("agy-keyring");
        agy_file(s.path(), &agy_token("ya29.file", "file@example.com"));
        olo_system::secrets::store("Olo test", &attrs, agy_token("ya29.keyring", "keyring@example.com").as_bytes()).unwrap();
        let found = agy_here(ask_keyring(&attrs).keyring, s.path()).unwrap();
        assert_eq!((found.credential.access.as_str(), found.email.as_deref()), ("ya29.keyring", Some("keyring@example.com")));
        assert_eq!(olo_system::secrets::delete(&attrs).unwrap(), 1);
        assert!(matches!(ask_keyring(&attrs).keyring, Keyring::Absent));
        assert_eq!(agy_here(ask_keyring(&attrs).keyring, s.path()).unwrap().email.as_deref(), Some("file@example.com"));
    }
}
