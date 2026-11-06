//! GNOME Keyring, as olo-secrets takes over from it and hands back: its
//! items read as a client of its Secret Service (a locked collection
//! unlocked first, GNOME Keyring asking through Olo's prompter), changes
//! written back, and the daemon asked to quit over its control socket
//! (gkd-control-server.c's GKD_CONTROL_OP_QUIT), which gives up
//! `org.freedesktop.secrets` to olo-secrets, waiting in line for it.
//! GNOME Keyring lets nobody replace it on the bus, so this is the way.

use std::{
    collections::{BTreeMap, HashMap},
    io::{Read as _, Write as _},
    os::unix::{fs::FileTypeExt as _, fs::MetadataExt as _, net::UnixStream},
    path::PathBuf,
    time::Duration,
};

use zbus::{
    blocking::{Connection, Proxy},
    zvariant::{OwnedObjectPath, OwnedValue, Value},
};
use zeroize::Zeroizing;

use crate::store::{self, Item};

pub const NAME: &str = "org.freedesktop.secrets";
const ROOT: &str = "/org/freedesktop/secrets";
const SERVICE: &str = "org.freedesktop.Secret.Service";
const COLLECTION: &str = "org.freedesktop.Secret.Collection";
const ITEM: &str = "org.freedesktop.Secret.Item";
const PROMPT: &str = "org.freedesktop.Secret.Prompt";

/// GKD_CONTROL_OP_QUIT and GKD_CONTROL_RESULT_OK.
const QUIT: u32 = 3;
const OK: u32 = 0;

/// One of GNOME Keyring's items, read.
pub struct Found {
    /// Its collection's id (its path's last part) and label.
    pub collection: String,
    pub collection_label: String,
    pub item: Item,
}

/// Who answers for the keyring now (a unique name), if anyone.
pub fn owner(conn: &Connection) -> Option<String> {
    let dbus = zbus::blocking::fdo::DBusProxy::new(conn).ok()?;
    dbus.get_name_owner(NAME.try_into().ok()?).ok().map(|o| o.to_string())
}

fn service(conn: &Connection) -> zbus::Result<Proxy<'_>> {
    Proxy::new(conn, NAME, ROOT, SERVICE)
}

/// Run a prompt GNOME Keyring asked for; true unless dismissed.
fn run_prompt(conn: &Connection, prompt: &OwnedObjectPath) -> zbus::Result<bool> {
    if prompt.as_str() == "/" {
        return Ok(true);
    }
    let p = Proxy::new(conn, NAME, prompt.as_str(), PROMPT)?;
    let mut done = p.receive_signal("Completed")?;
    p.call::<_, _, ()>("Prompt", &("",))?;
    let Some(msg) = done.next() else { return Ok(false) };
    let (dismissed, _): (bool, OwnedValue) = msg.body().deserialize()?;
    Ok(!dismissed)
}

/// Unlock `objects`, asking if need be; whether they were.
fn unlock(conn: &Connection, svc: &Proxy<'_>, objects: &[OwnedObjectPath]) -> zbus::Result<bool> {
    let (_, prompt): (Vec<OwnedObjectPath>, OwnedObjectPath) = svc.call("Unlock", &(objects,))?;
    run_prompt(conn, &prompt)
}

fn last(p: &str) -> String {
    p.rsplit('/').next().unwrap_or_default().to_owned()
}

/// Every item GNOME Keyring keeps (its session collection left out, as
/// it's kept in memory only), each locked collection unlocked first if
/// `ask` (else left); and the collections left locked, by label.
pub fn read_all(conn: &Connection, ask: bool) -> zbus::Result<(Vec<Found>, Vec<String>)> {
    let svc = service(conn)?;
    let (_, session): (OwnedValue, OwnedObjectPath) = svc.call("OpenSession", &("plain", Value::from("")))?;
    let collections: Vec<OwnedObjectPath> = svc.get_property("Collections")?;
    let (mut found, mut skipped) = (Vec::new(), Vec::new());
    for c in collections {
        let id = last(c.as_str());
        if id == store::SESSION {
            continue;
        }
        let col = Proxy::new(conn, NAME, c.as_str(), COLLECTION)?;
        let label: String = col.get_property("Label").unwrap_or_default();
        let locked: bool = col.get_property("Locked").unwrap_or(false);
        if locked && (!ask || !unlock(conn, &svc, std::slice::from_ref(&c))?) {
            skipped.push(label);
            continue;
        }
        let items: Vec<OwnedObjectPath> = col.get_property("Items")?;
        if items.is_empty() {
            continue;
        }
        let secrets: HashMap<OwnedObjectPath, (OwnedObjectPath, Vec<u8>, Vec<u8>, String)> = svc.call("GetSecrets", &(&items, &session))?;
        for p in items {
            let Some((_, _, value, content_type)) = secrets.get(&p) else { continue };
            let it = Proxy::new(conn, NAME, p.as_str(), ITEM)?;
            let attributes: HashMap<String, String> = it.get_property("Attributes").unwrap_or_default();
            found.push(Found {
                collection: id.clone(),
                collection_label: label.clone(),
                item: Item {
                    id: 0,
                    label: it.get_property("Label").unwrap_or_default(),
                    attributes: attributes.into_iter().collect(),
                    secret: Zeroizing::new(value.clone()),
                    content_type: content_type.clone(),
                    created: it.get_property("Created").unwrap_or(0),
                    modified: it.get_property("Modified").unwrap_or(0),
                },
            });
        }
    }
    let _ = Proxy::new(conn, NAME, session.as_str(), "org.freedesktop.Secret.Session").and_then(|s| s.call::<_, _, ()>("Close", &()));
    Ok((found, skipped))
}

/// The collection `id` (labelled `label`) of GNOME Keyring's, made if
/// it's missing; its path. The login one is GNOME's `login` keyring where
/// it has one (what go-keyring, gh's, reads, whatever the default alias
/// says), made the default again if the default was a second "Login"
/// (Olo's client made one, 2026-11-04); else the default's.
fn collection(conn: &Connection, svc: &Proxy<'_>, id: &str, label: &str) -> zbus::Result<OwnedObjectPath> {
    let collections: Vec<OwnedObjectPath> = svc.get_property("Collections")?;
    if let Some(p) = collections.iter().find(|p| last(p.as_str()) == id) {
        if id == store::LOGIN {
            let default: OwnedObjectPath = svc.call("ReadAlias", &("default",))?;
            if default.as_str() != p.as_str() && (default.as_str() == "/" || last(default.as_str()).eq_ignore_ascii_case(store::LOGIN)) {
                let _ = svc.call::<_, _, ()>("SetAlias", &("default", p));
            }
        }
        return Ok(p.clone());
    }
    let alias = if id == store::LOGIN { "default" } else { "" };
    if !alias.is_empty() {
        let p: OwnedObjectPath = svc.call("ReadAlias", &(alias,))?;
        if p.as_str() != "/" {
            return Ok(p);
        }
    }
    let mut props: HashMap<&str, Value> = HashMap::new();
    props.insert("org.freedesktop.Secret.Collection.Label", Value::from(label));
    let (made, prompt): (OwnedObjectPath, OwnedObjectPath) = svc.call("CreateCollection", &(props, alias))?;
    if made.as_str() != "/" {
        return Ok(made);
    }
    if !run_prompt(conn, &prompt)? {
        return Err(zbus::Error::Failure(format!("no collection “{label}” was made")));
    }
    let collections: Vec<OwnedObjectPath> = svc.get_property("Collections")?;
    collections.into_iter().find(|p| last(p.as_str()) == id).ok_or_else(|| zbus::Error::Failure(format!("no collection “{label}”")))
}

/// `items` ((collection id, its label, the item)) written into GNOME
/// Keyring, each replacing one with the same attributes, and the items
/// named by `deleted` (identities) deleted there. How many of each.
pub fn write(conn: &Connection, items: &[(String, String, Item)], deleted: &[String]) -> zbus::Result<(usize, usize)> {
    let svc = service(conn)?;
    let (_, session): (OwnedValue, OwnedObjectPath) = svc.call("OpenSession", &("plain", Value::from("")))?;
    let mut paths: BTreeMap<String, OwnedObjectPath> = BTreeMap::new();
    let mut written = 0;
    for (id, label, item) in items {
        let p = match paths.get(id) {
            Some(p) => p.clone(),
            None => {
                let p = collection(conn, &svc, id, label)?;
                paths.insert(id.clone(), p.clone());
                p
            }
        };
        let col = Proxy::new(conn, NAME, p.as_str(), COLLECTION)?;
        if col.get_property::<bool>("Locked").unwrap_or(false) && !unlock(conn, &svc, std::slice::from_ref(&p))? {
            return Err(zbus::Error::Failure(format!("GNOME Keyring's “{label}” stayed locked")));
        }
        let attrs: HashMap<&str, &str> = item.attributes.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let mut props: HashMap<&str, Value> = HashMap::new();
        props.insert("org.freedesktop.Secret.Item.Label", Value::from(item.label.as_str()));
        props.insert("org.freedesktop.Secret.Item.Attributes", Value::from(attrs));
        let secret = (&session, Vec::<u8>::new(), item.secret.to_vec(), item.content_type.as_str());
        let (_, prompt): (OwnedObjectPath, OwnedObjectPath) = col.call("CreateItem", &(props, secret, true))?;
        if run_prompt(conn, &prompt)? {
            written += 1;
        }
    }
    let mut gone = 0;
    for identity in deleted {
        let mut parts = identity.splitn(3, '\n');
        let (Some(id), Some(label), Some(attrs)) = (parts.next(), parts.next(), parts.next()) else { continue };
        let Ok(attrs) = serde_json::from_str::<BTreeMap<String, String>>(attrs) else { continue };
        let (unlocked, locked): (Vec<OwnedObjectPath>, Vec<OwnedObjectPath>) = svc.call("SearchItems", &(attrs.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect::<HashMap<_, _>>(),))?;
        for p in unlocked.iter().chain(&locked) {
            // (Only in that collection, with that label.)
            if !p.as_str().contains(&format!("/collection/{id}/")) {
                continue;
            }
            let it = Proxy::new(conn, NAME, p.as_str(), ITEM)?;
            if it.get_property::<String>("Label").unwrap_or_default() != label {
                continue;
            }
            let prompt: OwnedObjectPath = it.call("Delete", &())?;
            if run_prompt(conn, &prompt)? {
                gone += 1;
            }
        }
    }
    Ok((written, gone))
}

/// GNOME Keyring's control socket.
fn control() -> PathBuf {
    std::env::var_os("GNOME_KEYRING_CONTROL").map(PathBuf::from).unwrap_or_else(|| olo_xdg::runtime_dir().join("keyring")).join("control")
}

/// Ask the daemon to quit, and wait (up to five seconds) until it has:
/// it keeps the socket open till it exits. Ok(false): no daemon there.
pub fn quit() -> std::io::Result<bool> {
    let path = control();
    // Like the daemon's own client: only our socket, never a link.
    let Ok(m) = std::fs::symlink_metadata(&path) else { return Ok(false) };
    // SAFETY: geteuid cannot fail.
    if !m.file_type().is_socket() || m.uid() != unsafe { libc::geteuid() } {
        return Ok(false);
    }
    let mut s = match UnixStream::connect(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => return Ok(false),
        Err(e) => return Err(e),
    };
    s.set_read_timeout(Some(Duration::from_secs(5)))?;
    s.set_write_timeout(Some(Duration::from_secs(5)))?;
    // Credentials (one zero byte; the daemon checks SO_PEERCRED), then
    // the packet: its length, the op.
    let mut packet = vec![0u8];
    packet.extend(8u32.to_be_bytes());
    packet.extend(QUIT.to_be_bytes());
    s.write_all(&packet)?;
    let mut reply = [0u8; 8];
    s.read_exact(&mut reply)?;
    let result = u32::from_be_bytes(reply[4..8].try_into().unwrap());
    if result != OK {
        return Err(std::io::Error::other(format!("GNOME Keyring wouldn't quit (result {result})")));
    }
    // Till it's gone (the socket closed), or the timeout.
    let mut rest = Vec::new();
    let _ = s.read_to_end(&mut rest);
    Ok(true)
}
