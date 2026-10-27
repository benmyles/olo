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

/// The secret stored with exactly these attributes, if there is one.
pub fn lookup(attributes: &[(&str, &str)]) -> Result<Option<Vec<u8>>> {
    let conn = bus::session()?;
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

#[cfg(test)]
mod tests {
    /// Stores, finds and replaces a secret. Needs a private session bus
    /// with its own gnome-keyring (tools/dev/private-keyring sets one up);
    /// it refuses to run on any other bus, so it never touches yours.
    #[test]
    #[ignore]
    fn round_trip() {
        let bus = std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap_or_default();
        assert!(bus.contains("/tmp/tabula-dev/"), "not a private test bus: {bus}");
        let attrs = [("application", "tabula-test"), ("purpose", "round-trip")];
        super::store("Tabula test", &attrs, b"first").unwrap();
        assert_eq!(super::lookup(&attrs).unwrap().as_deref(), Some(&b"first"[..]));
        super::store("Tabula test", &attrs, b"second").unwrap();
        assert_eq!(super::lookup(&attrs).unwrap().as_deref(), Some(&b"second"[..]));
        assert_eq!(super::lookup(&[("application", "tabula-test"), ("purpose", "none")]).unwrap(), None);
    }
}
