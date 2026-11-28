//! The login keyring, as a client of the Secret Service
//! (org.freedesktop.secrets, served by gnome-keyring-daemon): find a
//! secret by its attributes, or store one in the default collection. PAM
//! unlocks the login keyring at login, so neither normally asks anything;
//! if it is locked the daemon prompts (through our prompter) and the call
//! waits for the answer. Blocking: call from a worker thread.

use std::collections::HashMap;

use zbus::blocking::{Connection, Proxy};
use zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

use crate::{Result, bus};

const DEST: &str = "org.freedesktop.secrets";
const SERVICE: &str = "org.freedesktop.Secret.Service";
const ITEM: &str = "org.freedesktop.Secret.Item";
const COLLECTION: &str = "org.freedesktop.Secret.Collection";
const PROMPT: &str = "org.freedesktop.Secret.Prompt";
const SESSION: &str = "org.freedesktop.Secret.Session";
const ROOT: &str = "/org/freedesktop/secrets";

/// What [`lookup_login`] says of a secret bigger than it was asked for
/// (`max`): left in the reply, never copied out of it.
pub const TOO_BIG: &str = "what the keyring keeps there is bigger than a secret of that kind";

/// (session, parameters, value, content type), as the Secret Service has it.
type Secret = (OwnedObjectPath, Vec<u8>, Vec<u8>, String);

/// A secret Olo keeps in the login keyring: every owner finds it by these
/// attributes, and Backup takes each along (`KEPT`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Kept {
    /// Its label in the keyring.
    pub label: &'static str,
    pub attributes: &'static [(&'static str, &'static str)],
    /// What it is, as a backup's summary names it.
    pub what: &'static str,
    /// Whether it may go to another computer, or only back to this one.
    pub carry: Carry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Carry {
    /// Anywhere: it means the same on any computer.
    Anywhere,
    /// Only back to the computer it came from: a sign-in of this
    /// computer's own, which two computers can't share (Telegram's
    /// session: each device signs in as itself).
    SameComputer,
}

/// The key to Passwords' vault (`passwords.vault`).
pub const PASSWORDS_KEY: Kept = Kept { label: "Olo Passwords", attributes: &[("application", "tabula-passwords"), ("purpose", "vault-key")], what: "Passwords", carry: Carry::Anywhere };
/// The key to the Token Pool's store (`pool.vault`).
pub const POOL_KEY: Kept = Kept { label: "Olo Token Pool", attributes: &[("application", "tabula-pool"), ("purpose", "pool-key")], what: "Token Pool", carry: Carry::Anywhere };
/// The GitHub token Integrations keeps (`github`).
pub const GITHUB: Kept = Kept { label: "GitHub (Olo)", attributes: &[("application", "olo"), ("service", "github.com")], what: "GitHub", carry: Carry::Anywhere };
/// The API ID and hash registered for Telegram.
pub const TELEGRAM_API: Kept = Kept { label: "Telegram API ID (Olo)", attributes: &[("application", "olo"), ("service", "telegram"), ("kind", "api")], what: "Telegram", carry: Carry::Anywhere };
/// Telegram's session: its authorization keys, this computer signed in.
pub const TELEGRAM_SESSION: Kept =
    Kept { label: "Telegram session (Olo)", attributes: &[("application", "olo"), ("service", "telegram"), ("kind", "session")], what: "Telegram sign-in", carry: Carry::SameComputer };

/// The key to Olo's own Secret Service's store (olo-secrets'
/// `secrets.store`), kept in GNOME Keyring's login keyring, which PAM
/// unlocks at login.
pub const SECRETS_KEY: Kept = Kept { label: "Olo Secrets", attributes: &[("application", "olo"), ("purpose", "secrets-key")], what: "Programs’ Secrets", carry: Carry::Anywhere };

/// The key to DiskBackup's backups (its copy for the service, which runs
/// with no one there; the key itself is kept in Passwords too).
pub const DISKBACKUP_KEY: Kept = Kept { label: "Olo DiskBackup", attributes: &[("application", "olo"), ("purpose", "diskbackup-key")], what: "DiskBackup", carry: Carry::Anywhere };

/// DiskBackup's sign-in to Google Drive: the person's own OAuth client
/// and its refresh token (JSON), for the service.
pub const DISKBACKUP_GOOGLE: Kept = Kept { label: "Olo DiskBackup Google Drive", attributes: &[("application", "olo"), ("purpose", "diskbackup-google")], what: "DiskBackup", carry: Carry::Anywhere };

/// Every secret Olo keeps in the keyring. One added anywhere goes here,
/// or a backup leaves it behind.
pub const KEPT: &[Kept] = &[PASSWORDS_KEY, POOL_KEY, GITHUB, TELEGRAM_API, TELEGRAM_SESSION, SECRETS_KEY, DISKBACKUP_KEY, DISKBACKUP_GOOGLE];

fn service(conn: &Connection) -> Result<Proxy<'_>> {
    Proxy::new(conn, DEST, ROOT, SERVICE)
}

/// A Secret Service session, closed when it goes: one left open stays the
/// service's until this process leaves the bus, which for a program that
/// runs all day (Token Pool) was each lookup's session kept to the end of
/// the desktop's session (Astra's review, 2026-11-28).
struct Opened<'a> {
    conn: &'a Connection,
    path: OwnedObjectPath,
}

impl Drop for Opened<'_> {
    fn drop(&mut self) {
        if let Ok(p) = Proxy::new(self.conn, DEST, self.path.as_str(), SESSION) {
            let _ = p.call::<_, _, ()>("Close", &());
        }
    }
}

/// A "plain" session: the secret travels over the private session bus
/// unencrypted, as with libsecret's default for local daemons. Closed when
/// it goes ([`Opened`]).
fn open_session<'a>(conn: &'a Connection, svc: &Proxy) -> Result<Opened<'a>> {
    let (_, path): (OwnedValue, OwnedObjectPath) = svc.call("OpenSession", &("plain", Value::from("")))?;
    Ok(Opened { conn, path })
}

/// What `item` keeps, through `session`: one bigger than `max` bytes is
/// [`TOO_BIG`], its reply's size looked at before anything is copied out
/// of it (a sign-in is a few kilobytes; what's kept there could be any
/// size).
fn secret_of(conn: &Connection, item: &OwnedObjectPath, session: &Opened, max: usize) -> Result<Vec<u8>> {
    let item = Proxy::new(conn, DEST, item, ITEM)?;
    let reply = item.call_method("GetSecret", &(&session.path,))?;
    let body = reply.body();
    // (The value's bytes, and the session's path and content type round
    // them: a page is room for those.)
    if body.len() > max.saturating_add(4096) {
        return Err(zbus::Error::Failure(TOO_BIG.into()));
    }
    let (_, _, value, _): Secret = body.deserialize()?;
    if value.len() > max {
        return Err(zbus::Error::Failure(TOO_BIG.into()));
    }
    Ok(value)
}

/// Wait for a prompt the daemon asked for (unlocking a keyring);
/// true unless it was dismissed.
fn run_prompt(conn: &Connection, prompt: &ObjectPath) -> Result<bool> {
    if prompt.as_str() == "/" {
        return Ok(true);
    }
    let p = Proxy::new(conn, DEST, prompt, PROMPT)?;
    let mut done = p.receive_signal("Completed")?;
    p.call::<_, _, ()>("Prompt", &("",))?;
    let Some(msg) = done.next() else { return Ok(false) };
    let (dismissed, _): (bool, OwnedValue) = msg.body().deserialize()?;
    Ok(!dismissed)
}

/// Whether a keyring serves the bus, or can be started on it.
fn available(conn: &Connection) -> bool {
    let ask = |method: &str, body: &dyn Fn() -> zbus::Result<bool>| {
        body().unwrap_or_else(|e| {
            tracing::debug!("secrets: {method}: {e}");
            false
        })
    };
    let owned = || -> zbus::Result<bool> { conn.call_method(Some("org.freedesktop.DBus"), "/org/freedesktop/DBus", Some("org.freedesktop.DBus"), "NameHasOwner", &(DEST,))?.body().deserialize() };
    let activatable = || -> zbus::Result<bool> {
        let names: Vec<String> = conn.call_method(Some("org.freedesktop.DBus"), "/org/freedesktop/DBus", Some("org.freedesktop.DBus"), "ListActivatableNames", &())?.body().deserialize()?;
        Ok(names.iter().any(|n| n == DEST))
    };
    ask("NameHasOwner", &owned) || ask("ListActivatableNames", &activatable)
}

/// Whether there is a login keyring to keep secrets in (one serving the
/// session bus, or that can be started on it).
pub fn present() -> bool {
    bus::session().is_ok_and(available)
}

/// Whether Olo's keyring (olo-secrets) is on but locked: the login
/// password wasn't given, or it couldn't be opened. What asks it for a
/// secret then gets an error (or waits for the password, if a program's
/// request brings the prompt); [`unlock`] asks. What it said of itself
/// (`olo-secrets.json`) is taken only while it's on this bus: one that
/// stopped locked left that behind, and another keyring may serve the
/// bus now (Neon's review, 2026-11-28). (Asked of the bus only when the
/// file says locked: nothing more is asked of it when the keyring is
/// open, as it nearly always is.)
pub fn locked() -> bool {
    let says = status(&olo_xdg::runtime_dir().join("olo-secrets.json")).is_some_and(|v| v["locked"] == true);
    says && bus::session().is_ok_and(|conn| {
        conn.call_method(Some("org.freedesktop.DBus"), "/org/freedesktop/DBus", Some("org.freedesktop.DBus"), "NameHasOwner", &("org.olo.Secrets1",))
            .and_then(|reply| reply.body().deserialize::<bool>())
            .unwrap_or(false)
    })
}

/// The most of `olo-secrets.json` read: it's a line or two.
const STATUS_MOST: u64 = 64 << 10;

/// What Olo's keyring said of itself at `path`, if that's a plain file of
/// a status's size: opened without waiting, and read no further than that
/// (a pipe left there held every import of a sign-in behind it, and a
/// file of any size was read whole; Astra's review, 2026-11-28). None for
/// anything else: unknown, as no file.
fn status(path: &std::path::Path) -> Option<serde_json::Value> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    let f = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY).open(path).ok()?;
    if !f.metadata().ok()?.is_file() {
        return None;
    }
    let mut b = Vec::new();
    f.take(STATUS_MOST + 1).read_to_end(&mut b).ok()?;
    if b.len() as u64 > STATUS_MOST {
        return None;
    }
    serde_json::from_slice(&b).ok()
}

/// How long [`unlock`] waits for its answer: as long as a password takes
/// to type, and no longer than that (a keyring that never answers held
/// every import of a sign-in behind it, Astra's review, 2026-11-28).
const UNLOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(300);

/// Ask Olo's keyring to open, as the person asked (a button they pressed):
/// the login password is asked for through Olo's prompter. Whether it's
/// open. Blocks as long as they take, up to five minutes: call it from a
/// worker thread. (On a connection of its own: the shared one gives up on
/// a call after ten seconds, and a password typed slower than that was
/// "Couldn't ask the login keyring" with the prompt still showing.)
pub fn unlock() -> Result<bool> {
    let conn = zbus::blocking::connection::Builder::session()?.method_timeout(UNLOCK_WAIT).build()?;
    let p = Proxy::new(&conn, "org.olo.Secrets1", "/org/olo/Secrets1", "org.olo.Secrets1")?;
    p.call("Unlock", &(true,))
}

/// The secret stored with exactly these attributes, if there is one (none
/// with no keyring).
pub fn lookup(attributes: &[(&str, &str)]) -> Result<Option<Vec<u8>>> {
    let conn = bus::session()?;
    if !available(conn) {
        return Ok(None);
    }
    let svc = service(conn)?;
    let attrs: HashMap<&str, &str> = attributes.iter().copied().collect();
    let (unlocked, locked): (Vec<OwnedObjectPath>, Vec<OwnedObjectPath>) = svc.call("SearchItems", &(attrs,))?;
    let item = match (unlocked.into_iter().next(), locked.into_iter().next()) {
        (Some(item), _) => item,
        (None, Some(item)) => {
            let (_, prompt): (Vec<OwnedObjectPath>, OwnedObjectPath) = svc.call("Unlock", &(vec![&item],))?;
            if !run_prompt(conn, &prompt)? {
                return Err(zbus::Error::Failure("the keyring stayed locked".into()));
            }
            item
        }
        (None, None) => return Ok(None),
    };
    let session = open_session(conn, &svc)?;
    // (Whatever its size, as `store` takes it: only a sign-in read for
    // another program is held to its size, `lookup_login`. A megabyte
    // here, and what was stored couldn't be read back; Astra's review,
    // 2026-11-28.)
    Ok(Some(secret_of(conn, &item, &session, usize::MAX)?))
}

/// The secret stored with these attributes in the login keyring as
/// go-keyring finds its own (gh's, agy's): in `…/collection/login`, else
/// the default collection; that collection unlocked first (a keyring's
/// prompt, if it's locked), then its first match. One kept in another
/// collection isn't theirs, so it isn't looked at: the service-wide
/// [`lookup`] took an older one kept unlocked elsewhere over the one in a
/// locked login keyring. None with no keyring. One bigger than `max`
/// bytes is [`TOO_BIG`], never copied out of the reply.
pub fn lookup_login(attributes: &[(&str, &str)], max: usize) -> Result<Option<Vec<u8>>> {
    let conn = bus::session()?;
    if !available(conn) {
        return Ok(None);
    }
    let svc = service(conn)?;
    // (The default collection only where there's no login keyring, as
    // the keyring's own list says: one that couldn't be listed is no
    // answer, and another keyring's older item is never taken for it.)
    let collection = match existing_login(&svc)? {
        Some(login) => login,
        None => {
            let alias: OwnedObjectPath = svc.call("ReadAlias", &("default",))?;
            if alias.as_str() == "/" {
                return Ok(None);
            }
            alias
        }
    };
    let (unlocked, prompt): (Vec<OwnedObjectPath>, OwnedObjectPath) = svc.call("Unlock", &(vec![&collection],))?;
    // (Neither opened nor a prompt to open it: it stays locked.)
    if (prompt.as_str() == "/" && unlocked.is_empty()) || !run_prompt(conn, &prompt)? {
        return Err(zbus::Error::Failure("the keyring stayed locked".into()));
    }
    let col = Proxy::new(conn, DEST, collection.as_str(), COLLECTION)?;
    let attrs: HashMap<&str, &str> = attributes.iter().copied().collect();
    let found: Vec<OwnedObjectPath> = col.call("SearchItems", &(attrs,))?;
    let Some(item) = found.into_iter().next() else { return Ok(None) };
    let session = open_session(conn, &svc)?;
    Ok(Some(secret_of(conn, &item, &session, max)?))
}

/// Store `secret` under `label` with these attributes in the default
/// collection (the login keyring), replacing one with the same attributes.
pub fn store(label: &str, attributes: &[(&str, &str)], secret: &[u8]) -> Result<()> {
    let conn = bus::session()?;
    if !available(conn) {
        return Err(zbus::Error::Failure("there is no keyring".into()));
    }
    let svc = service(conn)?;
    let session = open_session(conn, &svc)?;
    let alias: OwnedObjectPath = svc.call("ReadAlias", &("default",))?;
    // No default: the login keyring made the default (a new computer's
    // login made one without saying so), else, with none at all, made,
    // as GNOME makes it, the keyring asking for its password (with
    // Olo's prompter). (Made beside an existing `login`, 2026-11-04, a
    // second "Login" took programs' secrets that go-keyring, gh's, never
    // looks in.)
    let collection = if alias.as_str() != "/" {
        alias.as_str().to_string()
    } else if let Some(login) = existing_login(&svc)? {
        let _ = svc.call::<_, _, ()>("SetAlias", &("default", &login));
        login.as_str().to_string()
    } else {
        create_login(conn, &svc)?
    };
    let col = Proxy::new(conn, DEST, collection.as_str(), COLLECTION)?;
    let attrs: HashMap<&str, &str> = attributes.iter().copied().collect();
    let mut props: HashMap<&str, Value> = HashMap::new();
    props.insert("org.freedesktop.Secret.Item.Label", Value::from(label));
    props.insert("org.freedesktop.Secret.Item.Attributes", Value::from(attrs));
    let secret: Secret = (session.path.clone(), Vec::new(), secret.to_vec(), "application/octet-stream".into());
    let (_, prompt): (OwnedObjectPath, OwnedObjectPath) = col.call("CreateItem", &(props, secret, true))?;
    if !run_prompt(conn, &prompt)? {
        return Err(zbus::Error::Failure("the keyring stayed locked".into()));
    }
    Ok(())
}

/// The login keyring (`…/collection/login`), if there is one; an error if
/// the keyring's collections couldn't be listed (not "there's none").
fn existing_login(svc: &Proxy<'_>) -> Result<Option<OwnedObjectPath>> {
    let collections: Vec<OwnedObjectPath> = svc.get_property("Collections")?;
    Ok(collections.into_iter().find(|p| p.as_str().rsplit('/').next() == Some("login")))
}

/// The login keyring made (`CreateCollection`, as the default): its
/// path, once its password is given.
fn create_login(conn: &Connection, svc: &Proxy<'_>) -> Result<String> {
    let mut props: HashMap<&str, Value> = HashMap::new();
    props.insert("org.freedesktop.Secret.Collection.Label", Value::from("Login"));
    let (made, prompt): (OwnedObjectPath, OwnedObjectPath) = svc.call("CreateCollection", &(props, "default"))?;
    if made.as_str() != "/" {
        return Ok(made.as_str().to_string());
    }
    if !run_prompt(conn, &prompt)? {
        return Err(zbus::Error::Failure("no login keyring was made".into()));
    }
    // (The prompt's result names it; the alias says it too.)
    let alias: OwnedObjectPath = svc.call("ReadAlias", &("default",))?;
    if alias.as_str() == "/" {
        return Err(zbus::Error::Failure("no login keyring was made".into()));
    }
    Ok(alias.as_str().to_string())
}

/// Delete every secret stored with these attributes; how many went.
pub fn delete(attributes: &[(&str, &str)]) -> Result<usize> {
    let conn = bus::session()?;
    if !available(conn) {
        return Ok(0);
    }
    let svc = service(conn)?;
    let attrs: HashMap<&str, &str> = attributes.iter().copied().collect();
    let (unlocked, locked): (Vec<OwnedObjectPath>, Vec<OwnedObjectPath>) = svc.call("SearchItems", &(attrs,))?;
    let mut n = 0;
    for path in unlocked.iter().chain(&locked) {
        let item = Proxy::new(conn, DEST, path, ITEM)?;
        let prompt: OwnedObjectPath = item.call("Delete", &())?;
        if run_prompt(conn, &prompt)? {
            n += 1;
        }
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use zbus::blocking::Proxy;
    use zvariant::{OwnedObjectPath, Value};

    /// What the keyring said of itself is read only from a plain file of
    /// a status's size, at once: a pipe left there (no writer, ever) or
    /// something else is unknown, never a wait (Astra's review,
    /// 2026-11-28).
    #[test]
    fn a_status_is_read_without_waiting() {
        let scratch = tempfile::Builder::new().prefix("olo-secrets-status-").tempdir_in("/var/tmp").unwrap();
        let dir = scratch.path();
        let at = dir.join("olo-secrets.json");
        std::fs::write(&at, r#"{"locked":true}"#).unwrap();
        assert_eq!(super::status(&at).unwrap()["locked"], true);
        // (Whole JSON, too big: past the most read, never taken in.)
        let mut big = br#"{"locked":true}"#.to_vec();
        big.resize(super::STATUS_MOST as usize + 1, b' ');
        std::fs::write(&at, big).unwrap();
        assert!(super::status(&at).is_none(), "a status of any size read");
        std::fs::remove_file(&at).unwrap();
        let c = std::ffi::CString::new(at.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: a NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        let started = std::time::Instant::now();
        assert!(super::status(&at).is_none(), "a pipe read as a status");
        assert!(started.elapsed() < std::time::Duration::from_secs(2), "waited on a pipe");
        assert!(super::status(dir).is_none() && super::status(&dir.join("none")).is_none());
    }

    /// What a test kept under `0`, deleted when the test ends, as it
    /// ends: a failed assertion included.
    struct Gone<'a>(&'a [(&'a str, &'a str)]);

    impl Drop for Gone<'_> {
        fn drop(&mut self) {
            let _ = super::delete(self.0);
        }
    }

    /// The login keyring's, as go-keyring looks: one kept in another
    /// collection (an older one in the session collection) is never what
    /// `lookup_login` gives, though the service-wide `lookup` finds it;
    /// and one bigger than asked for is `TOO_BIG`. Needs a private
    /// session bus with a keyring of its own, as `round_trip` (olo-bus
    /// and olo-secrets will do), never yours. Its items are its run's
    /// own (two runs on one bus, or one after a failure, never see or
    /// delete another's) and go as it ends.
    #[test]
    #[ignore]
    fn login_first() {
        let bus = std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap_or_default();
        assert!(bus.contains("/var/tmp/olo-"), "not a private test bus: {bus}");
        let run = format!("login-first-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
        let attrs = [("application", "olo-test"), ("purpose", run.as_str())];
        let _gone = Gone(&attrs);
        let conn = super::bus::session().unwrap();
        let svc = super::service(conn).unwrap();
        let session = super::open_session(conn, &svc).unwrap();
        let elsewhere = Proxy::new(conn, super::DEST, "/org/freedesktop/secrets/collection/session", super::COLLECTION).unwrap();
        let mut props: HashMap<&str, Value> = HashMap::new();
        props.insert("org.freedesktop.Secret.Item.Label", Value::from("Olo test (older)"));
        props.insert("org.freedesktop.Secret.Item.Attributes", Value::from(attrs.iter().copied().collect::<HashMap<&str, &str>>()));
        let secret: super::Secret = (session.path.clone(), Vec::new(), b"older".to_vec(), "text/plain".into());
        let (_, prompt): (OwnedObjectPath, OwnedObjectPath) = elsewhere.call("CreateItem", &(props, secret, true)).unwrap();
        assert!(super::run_prompt(conn, &prompt).unwrap());
        drop(session);
        assert_eq!(super::lookup(&attrs).unwrap().as_deref(), Some(&b"older"[..]));
        assert_eq!(super::lookup_login(&attrs, 64).unwrap(), None, "the session collection's isn't the login keyring's");
        super::store("Olo test", &attrs, b"current").unwrap();
        assert_eq!(super::lookup_login(&attrs, 64).unwrap().as_deref(), Some(&b"current"[..]));
        match super::lookup_login(&attrs, 3) {
            Err(zbus::Error::Failure(m)) => assert_eq!(m, super::TOO_BIG),
            other => panic!("a secret bigger than asked for was {other:?}"),
        }
        assert_eq!(super::delete(&attrs).unwrap(), 2);
        assert_eq!(super::lookup_login(&attrs, 64).unwrap(), None);
    }

    /// Stores, finds and replaces a secret. Needs a private session bus
    /// with its own gnome-keyring (tools/dev/private-keyring sets one up);
    /// it refuses to run on any other bus, so it never touches yours.
    #[test]
    #[ignore]
    fn round_trip() {
        let bus = std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap_or_default();
        assert!(bus.contains("/var/tmp/olo-dev/"), "not a private test bus: {bus}");
        let attrs = [("application", "olo-test"), ("purpose", "round-trip")];
        super::store("Olo test", &attrs, b"first").unwrap();
        assert_eq!(super::lookup(&attrs).unwrap().as_deref(), Some(&b"first"[..]));
        super::store("Olo test", &attrs, b"second").unwrap();
        assert_eq!(super::lookup(&attrs).unwrap().as_deref(), Some(&b"second"[..]));
        assert_eq!(super::lookup(&[("application", "olo-test"), ("purpose", "none")]).unwrap(), None);
    }
}
