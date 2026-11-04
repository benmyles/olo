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
const ROOT: &str = "/org/freedesktop/secrets";

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

/// Every secret Olo keeps in the keyring. One added anywhere goes here,
/// or a backup leaves it behind.
pub const KEPT: &[Kept] = &[PASSWORDS_KEY, POOL_KEY, GITHUB, TELEGRAM_API, TELEGRAM_SESSION];

fn service(conn: &Connection) -> Result<Proxy<'_>> {
    Proxy::new(conn, DEST, ROOT, SERVICE)
}

/// A "plain" session: the secret travels over the private session bus
/// unencrypted, as with libsecret's default for local daemons.
fn open_session(svc: &Proxy) -> Result<OwnedObjectPath> {
    let (_, session): (OwnedValue, OwnedObjectPath) = svc.call("OpenSession", &("plain", Value::from("")))?;
    Ok(session)
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
    let session = open_session(&svc)?;
    let item = Proxy::new(conn, DEST, &item, ITEM)?;
    let (_, _, value, _): Secret = item.call("GetSecret", &(&session,))?;
    Ok(Some(value))
}

/// Store `secret` under `label` with these attributes in the default
/// collection (the login keyring), replacing one with the same attributes.
pub fn store(label: &str, attributes: &[(&str, &str)], secret: &[u8]) -> Result<()> {
    let conn = bus::session()?;
    if !available(conn) {
        return Err(zbus::Error::Failure("there is no keyring".into()));
    }
    let svc = service(conn)?;
    let session = open_session(&svc)?;
    let alias: OwnedObjectPath = svc.call("ReadAlias", &("default",))?;
    let collection = if alias.as_str() == "/" { format!("{ROOT}/aliases/default") } else { alias.as_str().to_string() };
    let col = Proxy::new(conn, DEST, collection.as_str(), COLLECTION)?;
    let attrs: HashMap<&str, &str> = attributes.iter().copied().collect();
    let mut props: HashMap<&str, Value> = HashMap::new();
    props.insert("org.freedesktop.Secret.Item.Label", Value::from(label));
    props.insert("org.freedesktop.Secret.Item.Attributes", Value::from(attrs));
    let secret: Secret = (session, Vec::new(), secret.to_vec(), "application/octet-stream".into());
    let (_, prompt): (OwnedObjectPath, OwnedObjectPath) = col.call("CreateItem", &(props, secret, true))?;
    if !run_prompt(conn, &prompt)? {
        return Err(zbus::Error::Failure("the keyring stayed locked".into()));
    }
    Ok(())
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
    /// Stores, finds and replaces a secret. Needs a private session bus
    /// with its own gnome-keyring (tools/dev/private-keyring sets one up);
    /// it refuses to run on any other bus, so it never touches yours.
    #[test]
    #[ignore]
    fn round_trip() {
        let bus = std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap_or_default();
        assert!(bus.contains("/tmp/olo-dev/"), "not a private test bus: {bus}");
        let attrs = [("application", "olo-test"), ("purpose", "round-trip")];
        super::store("Olo test", &attrs, b"first").unwrap();
        assert_eq!(super::lookup(&attrs).unwrap().as_deref(), Some(&b"first"[..]));
        super::store("Olo test", &attrs, b"second").unwrap();
        assert_eq!(super::lookup(&attrs).unwrap().as_deref(), Some(&b"second"[..]));
        assert_eq!(super::lookup(&[("application", "olo-test"), ("purpose", "none")]).unwrap(), None);
    }
}
