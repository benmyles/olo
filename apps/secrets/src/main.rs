//! olo-secrets: Olo's own Secret Service (`org.freedesktop.secrets`), so
//! programs' secrets (Chrome's and Slack's "Safe Storage" keys, Wi-Fi
//! passwords, Olo's own keys) live in Olo's store rather than GNOME
//! Keyring's. Off unless Preferences ▸ Security turns it on
//! (`security.passwords.secret_service`); olo-session starts it at login
//! either way, and it does nothing while it's off.
//!
//! Taking over, at login or when turned on: the store's key comes from
//! GNOME Keyring's login keyring (PAM unlocked it with the password just
//! typed; made and kept there the first time), the store is opened, every
//! item GNOME Keyring has that's new or changed since the last time is
//! copied in, olo-secrets waits in line for the bus name and asks GNOME
//! Keyring to quit (it lets nobody replace it), and the name comes to
//! olo-secrets the moment it's gone. Handing back, when turned off: the
//! name given up, then what changed while Olo answered written into GNOME
//! Keyring (D-Bus starts it again; it asks for the password to unlock),
//! and what was deleted deleted there.
//!
//! What it's doing is in `$XDG_RUNTIME_DIR/olo-secrets.json` (`Status`),
//! which Preferences shows and `olo-secrets --status` prints.
//!
//! For tests: `OLO_SECRETS_SERVE=1` answers whatever the settings say
//! (`stdin`: as the lines "on" and "off" say),
//! `OLO_SECRETS_KEYFILE` keeps the store's key in a file, and
//! `OLO_SECRETS_STORE` puts the store elsewhere.

mod gnome;
mod kernel;
mod service;
mod session;
mod store;

use std::{
    path::PathBuf,
    sync::{Arc, mpsc},
};

use serde::Serialize;
use zeroize::Zeroizing;

use crate::store::Store;

/// What olo-secrets is doing, for Preferences and agents.
#[derive(Debug, Default, Clone, Serialize)]
struct Status {
    /// Olo answers for the keyring.
    serving: bool,
    /// Since when (seconds since 1970).
    #[serde(skip_serializing_if = "Option::is_none")]
    since: Option<u64>,
    /// Each collection: its id, label and how many items.
    collections: Vec<CollectionStatus>,
    /// The programs with secrets kept (their `application` attribute, or
    /// libsecret's schema).
    programs: Vec<String>,
    /// When GNOME Keyring's items were last copied in, and how many came
    /// that time.
    #[serde(skip_serializing_if = "Option::is_none")]
    imported_at: Option<u64>,
    imported: usize,
    /// GNOME Keyring's collections left locked (not copied).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    skipped: Vec<String>,
    /// What the store had that earlier versions left and was mended as
    /// it was opened (`Store::heal`): Olo Doctor says so.
    #[serde(skip_serializing_if = "Option::is_none")]
    healed: Option<store::Healed>,
    /// What went wrong last, if anything.
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Debug, Default, Clone, Serialize)]
struct CollectionStatus {
    id: String,
    label: String,
    items: usize,
}

fn status_path() -> PathBuf {
    olo_xdg::runtime_dir().join("olo-secrets.json")
}

fn write_status(s: &Status) {
    let tmp = status_path().with_extension("json.tmp");
    if std::fs::write(&tmp, serde_json::to_vec_pretty(s).unwrap_or_default()).is_ok() {
        let _ = std::fs::rename(tmp, status_path());
    }
}

fn store_path() -> PathBuf {
    std::env::var_os("OLO_SECRETS_STORE").map(PathBuf::from).unwrap_or_else(store::path)
}

/// The store's key: from `OLO_SECRETS_KEYFILE` (tests), else as kept for
/// this login in the kernel's keyring (started again: true), else the
/// login keyring (GNOME Keyring's, which must answer now), made and kept
/// there the first time.
fn key() -> Result<(Zeroizing<[u8; 32]>, bool), String> {
    if std::env::var_os("OLO_SECRETS_KEYFILE").is_none()
        && let Some(k) = kernel::get(&store_path())
    {
        return Ok((k, true));
    }
    let k = keyring_key()?;
    if std::env::var_os("OLO_SECRETS_KEYFILE").is_none() && !kernel::put(&store_path(), &k) {
        tracing::warn!("secrets: couldn't keep the store's key in the kernel's keyring (started again, it asks GNOME Keyring)");
    }
    Ok((k, false))
}

fn keyring_key() -> Result<Zeroizing<[u8; 32]>, String> {
    let from = |bytes: &[u8]| -> Result<Zeroizing<[u8; 32]>, String> {
        let mut k = Zeroizing::new([0u8; 32]);
        if bytes.len() != 32 {
            return Err("the store's key isn't 32 bytes".into());
        }
        k.copy_from_slice(bytes);
        Ok(k)
    };
    if let Some(file) = std::env::var_os("OLO_SECRETS_KEYFILE") {
        return match std::fs::read(&file) {
            Ok(b) => from(&Zeroizing::new(b)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let k = store::new_key();
                std::fs::write(&file, k.as_slice()).map_err(|e| e.to_string())?;
                Ok(k)
            }
            Err(e) => Err(e.to_string()),
        };
    }
    let kept = olo_system::secrets::SECRETS_KEY;
    match olo_system::secrets::lookup(kept.attributes).map_err(|e| format!("the login keyring: {e}"))? {
        Some(b) => from(&Zeroizing::new(b)),
        None if !olo_system::secrets::present() => Err("there's no login keyring to keep the store's key in".into()),
        None => {
            let k = store::new_key();
            olo_system::secrets::store(kept.label, kept.attributes, k.as_slice()).map_err(|e| format!("couldn't keep the store's key in the login keyring: {e}"))?;
            Ok(k)
        }
    }
}

/// GNOME Keyring's items copied in: each new or changed there since it
/// was last copied (`Data::imported`); a newer copy here (changed while
/// Olo answered) is kept. How many came.
fn import(store: &mut Store, found: Vec<gnome::Found>) -> usize {
    let mut n = 0;
    for mut f in found {
        let identity = store::identity(&f.collection, &f.item.label, &f.item.attributes);
        if store.data.imported.get(&identity) == Some(&f.item.modified) {
            continue;
        }
        // (GNOME Keyring's second login keyring, "Login", is ours: one.)
        if f.collection != store::LOGIN && f.collection.eq_ignore_ascii_case(store::LOGIN) {
            f.collection = store::LOGIN.to_owned();
        }
        if store.collection(&f.collection).is_none() {
            store.data.collections.push(store::Collection::new(&f.collection, &f.collection_label));
        }
        let wanted = store::core(&f.item.attributes);
        let ours = store.collection(&f.collection).and_then(|c| c.items.iter().find(|i| i.label == f.item.label && store::core(&i.attributes) == wanted)).map(|i| i.modified);
        let modified = f.item.modified;
        if ours.is_none_or(|m| m <= modified) {
            tracing::debug!("secrets: copying “{}” of {} (modified {modified}; copied before at {:?})", f.item.label, f.collection, store.data.imported.get(&identity));
            store.put_copy(&f.collection, f.item);
            n += 1;
        }
        store.data.imported.insert(identity, modified);
    }
    store.data.imported_at = Some(store::now());
    n
}

/// The status of a store.
fn status_of(store: &Store, serving: bool) -> Status {
    let mut programs: Vec<String> = store.data.collections.iter().flat_map(|c| &c.items).filter_map(|i| i.attributes.get("application").or_else(|| i.attributes.get("xdg:schema")).cloned()).collect();
    programs.sort();
    programs.dedup();
    Status {
        serving,
        since: store.data.serving_since.filter(|_| serving),
        collections: store.data.collections.iter().map(|c| CollectionStatus { id: c.id.clone(), label: c.label.clone(), items: c.items.len() }).collect(),
        programs,
        imported_at: store.data.imported_at,
        ..Status::default()
    }
}

/// Olo answering for the keyring.
struct Serving {
    conn: zbus::blocking::Connection,
    state: Arc<service::State>,
}

/// Take over from GNOME Keyring (see the top).
fn take_over(status: &mut Status) -> Result<Serving, String> {
    let conn = zbus::blocking::Connection::session().map_err(|e| format!("the session bus: {e}"))?;
    let (key, again) = key()?;
    let mut store = Store::open(&store_path(), key)?;
    let before = gnome::owner(&conn);
    if before.is_some() {
        // (Started again, nothing is asked: a locked collection waits for
        // the next login.)
        let (found, skipped) = gnome::read_all(&conn, !again).map_err(|e| format!("reading GNOME Keyring: {e}"))?;
        status.imported = import(&mut store, found);
        status.skipped = skipped;
    }
    // What earlier versions left, mended (an upgrade starts it again).
    let healed = store.heal();
    if !healed.is_empty() {
        tracing::info!("secrets: mended the store: {healed:?}");
        status.healed = Some(healed);
    }
    store.data.serving_since.get_or_insert_with(store::now);
    store.save()?;
    let state = service::State::new(store);
    zbus::block_on(state.export(&conn.object_server().inner().clone())).map_err(|e| format!("exporting the objects: {e}"))?;
    // (Told when the name comes: listened for before asking.)
    let (came_tx, came) = mpsc::channel();
    let acquired = zbus::blocking::fdo::DBusProxy::new(&conn).and_then(|d| d.receive_name_acquired()).map_err(|e| e.to_string())?;
    std::thread::spawn(move || {
        for s in acquired {
            if s.args().is_ok_and(|a| a.name() == gnome::NAME) {
                let _ = came_tx.send(());
                return;
            }
        }
    });
    let asked = conn.request_name_with_flags(gnome::NAME, enumflags2::BitFlags::empty()).map_err(|e| format!("asking for {}: {e}", gnome::NAME))?;
    if asked == zbus::fdo::RequestNameReply::InQueue {
        match gnome::quit() {
            Ok(true) => {}
            Ok(false) => return Err("another program answers for the keyring and can't be asked to quit".into()),
            Err(e) => return Err(format!("GNOME Keyring: {e}")),
        }
        if came.recv_timeout(std::time::Duration::from_secs(5)).is_err() {
            let _ = conn.release_name(gnome::NAME);
            return Err("the keyring's name didn't come to olo-secrets after GNOME Keyring quit".into());
        }
    }
    tracing::info!("secrets: answering for the keyring ({} copied from GNOME Keyring)", status.imported);
    Ok(Serving { conn, state })
}

/// Hand back to GNOME Keyring (see the top).
fn hand_back(serving: Serving) -> Result<(usize, usize), String> {
    let Serving { conn, state } = serving;
    let _ = conn.release_name(gnome::NAME);
    drop(conn);
    let (items, deleted, since) = {
        let s = state.store.lock().unwrap_or_else(|e| e.into_inner());
        let items: Vec<(String, String, store::Item)> = s
            .data
            .collections
            .iter()
            .flat_map(|c| c.items.iter().filter(|i| s.data.changed.contains(&store::identity(&c.id, &i.label, &i.attributes))).map(|i| (c.id.clone(), c.label.clone(), i.clone())))
            .collect();
        (items, s.data.deleted.iter().cloned().collect::<Vec<_>>(), s.data.serving_since.unwrap_or(0))
    };
    let conn = zbus::blocking::Connection::session().map_err(|e| e.to_string())?;
    let done = gnome::write(&conn, &items, &deleted).map_err(|e| format!("writing to GNOME Keyring: {e}"))?;
    let mut s = state.store.lock().unwrap_or_else(|e| e.into_inner());
    s.data.serving_since = None;
    s.data.changed.clear();
    s.data.deleted.clear();
    s.save()?;
    kernel::forget(&store_path());
    tracing::info!("secrets: handed back to GNOME Keyring ({} written, {} deleted; Olo answered since {since})", done.0, done.1);
    Ok(done)
}

/// Whether Olo should answer, now and as it changes: sent on `tx`.
fn watch_setting(tx: mpsc::Sender<bool>) {
    match std::env::var("OLO_SECRETS_SERVE").as_deref() {
        Ok("1") => {
            let _ = tx.send(true);
            return;
        }
        // (Tests: "on" and "off" lines.)
        Ok("stdin") => {
            std::thread::spawn(move || {
                for line in std::io::stdin().lines().map_while(Result::ok) {
                    if tx.send(line.trim() == "on").is_err() {
                        return;
                    }
                }
            });
            return;
        }
        _ => {}
    }
    if olo_protocol::socket_path().is_none() {
        let _ = tx.send(false);
        return;
    }
    std::thread::Builder::new()
        .name("settings".into())
        .spawn(move || {
            let wanted = |s: &olo_protocol::Settings| s.security.passwords.secret_service;
            loop {
                if let Ok(mut c) = olo_protocol::Client::connect()
                    && let Ok(olo_protocol::Reply::Settings { settings }) = c.request(&olo_protocol::Request::GetSettings)
                    && tx.send(wanted(&settings)).is_err()
                {
                    return;
                }
                if let Ok(client) = olo_protocol::Client::connect()
                    && let Ok(events) = client.into_events_of(&[olo_protocol::EventKind::Settings])
                {
                    for event in events {
                        if let Ok(olo_protocol::Event::SettingsChanged { settings }) = event
                            && tx.send(wanted(&settings)).is_err()
                        {
                            return;
                        }
                    }
                }
                // (app_server restarting: asked again in a moment.)
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
        })
        .ok();
}

/// Sessions of callers that left the bus, closed.
fn forget_the_gone(conn: &zbus::blocking::Connection, state: Arc<service::State>) {
    let Ok(dbus) = zbus::blocking::fdo::DBusProxy::new(conn) else { return };
    // (Only names left without an owner: a caller gone.)
    let Ok(changes) = dbus.receive_name_owner_changed_with_args(&[(2, "")]) else { return };
    let server = conn.object_server().inner().clone();
    std::thread::Builder::new()
        .name("callers".into())
        .spawn(move || {
            for c in changes {
                let Ok(args) = c.args() else { continue };
                if args.new_owner().is_none() && args.name().starts_with(':') {
                    for p in state.forget(args.name()) {
                        let _ = zbus::block_on(server.remove::<service::SessionObj, _>(p.as_str()));
                    }
                }
            }
        })
        .ok();
}

fn main() -> std::process::ExitCode {
    olo_crash::install();
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).with_writer(std::io::stderr).try_init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--status") {
        match std::fs::read_to_string(status_path()) {
            Ok(s) => println!("{s}"),
            Err(_) => println!("{}", serde_json::to_string_pretty(&Status::default()).unwrap_or_default()),
        }
        return std::process::ExitCode::SUCCESS;
    }
    if !args.is_empty() {
        eprintln!("usage: olo-secrets [--status]");
        return std::process::ExitCode::from(2);
    }
    let (tx, rx) = mpsc::channel();
    watch_setting(tx);
    let mut serving: Option<Serving> = None;
    let mut status = Status::default();
    write_status(&status);
    for wanted in rx {
        match (wanted, serving.take()) {
            (true, None) => {
                status = Status::default();
                match take_over(&mut status) {
                    Ok(s) => {
                        let imported = std::mem::take(&mut status.imported);
                        let skipped = std::mem::take(&mut status.skipped);
                        let healed = status.healed.take();
                        status = Status { imported, skipped, healed, ..status_of(&s.state.store.lock().unwrap_or_else(|e| e.into_inner()), true) };
                        forget_the_gone(&s.conn, s.state.clone());
                        serving = Some(s);
                    }
                    Err(e) => {
                        tracing::warn!("secrets: not answering for the keyring: {e}");
                        status.error = Some(e);
                    }
                }
            }
            (false, Some(s)) => {
                let state = s.state.clone();
                status = match hand_back(s) {
                    Ok(_) => status_of(&state.store.lock().unwrap_or_else(|e| e.into_inner()), false),
                    Err(e) => {
                        tracing::warn!("secrets: handing back: {e}");
                        Status { error: Some(e), ..status_of(&state.store.lock().unwrap_or_else(|e| e.into_inner()), false) }
                    }
                };
            }
            (_, s) => serving = s,
        }
        write_status(&status);
    }
    // (No settings to follow: keep answering, if it is, till ended.)
    if serving.is_some() {
        loop {
            std::thread::park();
        }
    }
    std::process::ExitCode::SUCCESS
}
