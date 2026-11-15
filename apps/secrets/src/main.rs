//! olo-secrets: Olo's own Secret Service (`org.freedesktop.secrets`), so
//! programs' secrets (Chrome's and Slack's "Safe Storage" keys, Wi-Fi
//! passwords, Olo's own keys) live in Olo's store. Always, in an Olo
//! session: olo-session starts it at login, and nothing of GNOME's is
//! started, used or handed back to (Ben, 2026-11-15: "we should be 100%
//! independent of gnome and not rely on any of its services"). It serves
//! the Secret portal too (`service.rs`, `Portal`: a secret for each
//! program).
//!
//! Taking over, at login: the store's key is opened with the login
//! password (`key`: sealed by it, `login_key.rs`, pam_olo_keyring handing
//! olo-secrets the key the password gives at Olo's login screen; else the
//! password asked for once), the store is opened, and with the password
//! typed, every item of GNOME Keyring's files that's new or changed since
//! the last time is copied in, read natively (`legacy.rs`). It asks for
//! the bus name waiting in line; if a GNOME Keyring something else
//! started holds it, it's asked to quit (it lets nobody replace it), and
//! the name comes to olo-secrets the moment it's gone.
//!
//! What it's doing is in `$XDG_RUNTIME_DIR/olo-secrets.json` (`Status`),
//! which Preferences shows and `olo-secrets --status` prints. Locked, it
//! says so and anyone can ask it to unlock (`locked.rs`).
//!
//! While nobody answers for the keyring (at login before olo-secrets
//! has, as it becomes a new version), D-Bus starts `olo-secrets
//! --activated` for it in an Olo session (`activated.rs`; olo-session
//! names it so), which waits for this one. Upgraded (olo-session's
//! SIGURG), it becomes the new version in place once no password is
//! being asked for, the store's key kept for the login in the kernel's
//! keyring, so nothing is asked again.
//!
//! For tests: `OLO_SECRETS_SERVE=1` answers outside an Olo session,
//! `OLO_SECRETS_KEYFILE` keeps the store's key in a file, and
//! `OLO_SECRETS_STORE` puts the store elsewhere.

mod activated;
mod gnome;
mod kernel;
mod legacy;
mod locked;
mod login_key;
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
    /// Its process (`activated.rs` waits while it's there).
    pid: u32,
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
    /// On, but locked: a notification says so, and `Unlock` (locked.rs)
    /// asks for the password.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    locked: bool,
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
    let s = Status { pid: std::process::id(), ..s.clone() };
    let tmp = status_path().with_extension("json.tmp");
    if std::fs::write(&tmp, serde_json::to_vec_pretty(&s).unwrap_or_default()).is_ok() {
        let _ = std::fs::rename(tmp, status_path());
    }
}

/// How many passwords are being asked for now: an upgrade waits till
/// none is (a prompt cut off by the new version would come back empty).
static ASKING: (std::sync::Mutex<usize>, std::sync::Condvar) = (std::sync::Mutex::new(0), std::sync::Condvar::new());

/// A password being asked for, while it lives.
pub struct Asking;

impl Asking {
    pub fn new() -> Asking {
        *ASKING.0.lock().unwrap_or_else(|e| e.into_inner()) += 1;
        Asking
    }
}

impl Drop for Asking {
    fn drop(&mut self) {
        let mut n = ASKING.0.lock().unwrap_or_else(|e| e.into_inner());
        *n = n.saturating_sub(1);
        ASKING.1.notify_all();
    }
}

/// Upgraded (olo-session's SIGURG: this program's file was replaced):
/// the new version, once no password is being asked for (olo-session
/// told it waits meanwhile), in the same process. The store's key is in
/// the kernel's keyring, so it asks nothing; the bus name is free only
/// for the moment it takes to start, and what asks then waits for it
/// (`activated.rs`). It was left on the old version and stopped 90 s
/// later, the name free till it started again.
fn become_new_version_when_upgraded() {
    olo_kit_base::restore::on_upgrade(|| {
        let mut n = ASKING.0.lock().unwrap_or_else(|e| e.into_inner());
        if *n > 0 {
            olo_kit_base::restore::mark_waiting(true);
            while *n > 0 {
                n = ASKING.1.wait(n).unwrap_or_else(|e| e.into_inner());
            }
        }
        tracing::info!("secrets: upgraded: becoming the new version");
        olo_kit_base::restore::mark_waiting(false);
        let e = olo_kit_base::restore::exec_new_version();
        tracing::warn!("secrets: couldn't become the new version: {e}");
    });
}

fn store_path() -> PathBuf {
    std::env::var_os("OLO_SECRETS_STORE").map(PathBuf::from).unwrap_or_else(store::path)
}

/// The store's key, as it was had.
struct Key {
    key: Zeroizing<[u8; 32]>,
    /// Kept for this login in the kernel's keyring (started again: an
    /// upgrade, a crash), so nothing was asked.
    again: bool,
    /// The login password, if the person typed it (PAM said it's theirs):
    /// what opens GNOME Keyring's keyrings, to bring them over.
    typed: Option<Zeroizing<String>>,
}

/// Why the login password is asked for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Why {
    /// To open the store's key, sealed by it: no key from PAM (another
    /// login screen, or pam_olo_keyring missing), or the password
    /// changed since it was sealed (`changed`).
    Open { changed: bool },
    /// A new store, with no key from PAM to seal it by.
    New,
    /// The store's key isn't sealed by the login password yet: an earlier
    /// version kept it in GNOME Keyring's login keyring, read natively
    /// (`legacy.rs`) with the password, once.
    Move,
    /// The key is open (kept for this login) but not sealed by the login
    /// password yet: typed once, the next login opens it.
    Seal,
}

/// The store's key: from `OLO_SECRETS_KEYFILE` (tests), else as kept for
/// this login in the kernel's keyring (started again), else sealed by the
/// login password (`login_key.rs`) and opened with the key PAM gave this
/// login (pam_olo_keyring, at Olo's login screen), else the password
/// asked for through Olo's prompter: to open it (no key from PAM, or the
/// password changed since), for a new store, or, not sealed yet, to read
/// the key an earlier version kept in GNOME Keyring's files. Nothing of
/// GNOME's runs, and nothing is asked of the bus: olo-secrets is what
/// answers there (it once asked, D-Bus started `--activated` for it,
/// which waits for this one, and both waited 10 s: Ben's keyring stayed
/// locked, 2026-11-15). Sealed by the login password from then on, and
/// again after it changes.
fn key() -> Result<Key, String> {
    if let Some(file) = std::env::var_os("OLO_SECRETS_KEYFILE") {
        return Ok(Key { key: keyfile_key(&PathBuf::from(file))?, again: false, typed: None });
    }
    let store = store_path();
    if let Some(k) = kernel::get(&store) {
        return Ok(Key { key: k, again: true, typed: None });
    }
    let login = kernel::take_login_key();
    let sealed = std::fs::read(login_key::path(&store)).ok();
    let (k, typed) = match (&sealed, login.as_ref().and_then(|l| login_key::open(l, sealed.as_deref()?))) {
        (_, Some(k)) => (k, None),
        (Some(bytes), None) => typed(ask_password(Why::Open { changed: login.is_some() }, Some(bytes))?),
        (None, None) if !store.exists() => match &login {
            Some(_) => (store::new_key(), None),
            None => typed(ask_password(Why::New, None)?),
        },
        (None, None) => typed(ask_password(Why::Move, None)?),
    };
    seal(&store, &k, login.as_deref(), typed.as_ref().map(|p| p.as_str()), sealed.as_deref());
    if !kernel::put(&store, &k) {
        tracing::warn!("secrets: couldn't keep the store's key in the kernel's keyring (started again, it asks again)");
    }
    Ok(Key { key: k, again: false, typed })
}

/// A key and the password typed for it.
fn typed((k, password): (Zeroizing<[u8; 32]>, Zeroizing<String>)) -> (Zeroizing<[u8; 32]>, Option<Zeroizing<String>>) {
    (k, Some(password))
}

/// The key to the store at `store` sealed by the login password (the key
/// PAM gave, else the one the typed password gives), unless `sealed`
/// already is by it.
fn seal(store: &std::path::Path, k: &[u8; 32], login: Option<&[u8; 32]>, typed: Option<&str>, sealed: Option<&[u8]>) {
    let from_typed = typed.map(|p| olo_login_key::login_key(p.as_bytes(), uid(), &olo_login_key::machine_id()));
    let Some(l) = login.or(from_typed.as_deref()) else { return };
    if sealed.is_some_and(|b| login_key::open(l, b).is_some()) {
        return;
    }
    match login_key::keep(store, l, k) {
        Ok(()) => tracing::info!("secrets: the store's key is sealed by the login password"),
        Err(e) => tracing::warn!("secrets: couldn't seal the store's key by the login password: {e}"),
    }
}

fn uid() -> u32 {
    // SAFETY: getuid can't fail.
    unsafe { libc::getuid() }
}

/// Kept for this login, but not sealed by the login password (the key
/// came from GNOME Keyring through an earlier version): the password
/// asked for once, now, so the next login opens it. Not Now leaves it
/// as it is, asked again the next time olo-secrets starts.
fn seal_if_unsealed(k: &[u8; 32]) {
    let store = store_path();
    if std::env::var_os("OLO_SECRETS_KEYFILE").is_some() || login_key::path(&store).exists() {
        return;
    }
    match ask_password_for(Why::Seal, None, k) {
        Ok((_, typed)) => seal(&store, k, None, Some(&typed), None),
        Err(e) => tracing::info!("secrets: the store's key isn't sealed by the login password yet: {e}"),
    }
}

/// The login password asked for through Olo's prompter, as `why` says,
/// until it's right or the person says Not Now: the store's key, and the
/// password.
fn ask_password(why: Why, sealed: Option<&[u8]>) -> Result<(Zeroizing<[u8; 32]>, Zeroizing<String>), String> {
    ask_password_for(why, sealed, &[0; 32])
}

fn ask_password_for(why: Why, sealed: Option<&[u8]>, open: &[u8; 32]) -> Result<(Zeroizing<[u8; 32]>, Zeroizing<String>), String> {
    let user = olo_system::auth::current_user().ok_or("who's logged in isn't known")?;
    let machine = olo_login_key::machine_id();
    let description = match why {
        Why::Open { changed: true } => "Your login password has changed since they were last opened. Type the password you logged in with before.",
        Why::Open { changed: false } => "Type your login password to open the secrets programs keep: their sign-ins, keys and Wi-Fi passwords.",
        Why::New | Why::Seal => "Type your login password once, so your login opens the secrets programs keep from now on: their sign-ins, keys and Wi-Fi passwords.",
        Why::Move => "Type your login password once to bring the secrets programs keep over from GNOME Keyring: their sign-ins, keys and Wi-Fi passwords. From then on your login opens them.",
    };
    let props = olo_system::keyring_prompt::Props {
        title: "Programs’ Secrets".into(),
        message: if why == Why::Seal { "Keep the secrets programs keep opening when you log in".into() } else { "Unlock the secrets programs keep".into() },
        description: description.into(),
        continue_label: "Unlock".into(),
        cancel_label: "Not Now".into(),
        ..Default::default()
    };
    let new = store::new_key();
    let mut found: Option<Zeroizing<[u8; 32]>> = None;
    // (What can't be helped by typing again: said once the prompt is gone.)
    let mut stuck: Option<String> = None;
    let _asking = Asking::new();
    let given = olo_system::keyring_prompt::ask_password(&props, "That isn’t your login password.", |password| {
        found = match why {
            Why::Open { .. } => sealed.and_then(|b| login_key::open(&olo_login_key::login_key(password.as_bytes(), uid(), &machine), b)),
            // (Checked by PAM: what seals the key must be the password the
            // next login gives, or that login can't open it.)
            Why::New | Why::Seal | Why::Move => match password_check(&user, password) {
                Err(olo_system::auth::AuthError::Denied(_)) => None,
                Err(olo_system::auth::AuthError::Unavailable(e)) => {
                    stuck = Some(format!("the password couldn't be checked: {e}"));
                    return true;
                }
                Ok(()) => match why {
                    Why::New => Some(new.clone()),
                    Why::Seal => Some(Zeroizing::new(*open)),
                    _ => match moved_key(password) {
                        Ok(k) => Some(k),
                        Err(e) => {
                            stuck = Some(e);
                            return true;
                        }
                    },
                },
            },
        };
        found.is_some()
    })
    .map_err(|e| format!("asking for the login password: {e}"))?;
    if let Some(e) = stuck {
        return Err(e);
    }
    match (given, found) {
        (Some(password), Some(k)) => Ok((k, password)),
        _ => Err(NOT_GIVEN.into()),
    }
}

/// Whether `password` is `user`'s, as PAM says. (Tests check against a
/// password of their own, `OLO_SECRETS_TEST_PASSWORD`, never the
/// account's.)
fn password_check(user: &str, password: &str) -> Result<(), olo_system::auth::AuthError> {
    match std::env::var("OLO_SECRETS_TEST_PASSWORD") {
        Ok(t) if t == password => Ok(()),
        Ok(_) => Err(olo_system::auth::AuthError::Denied("not the test's password".into())),
        Err(_) => olo_system::auth::check_password(user, password),
    }
}

/// What the person said Not Now to.
const NOT_GIVEN: &str = "the secrets stay locked: the login password wasn't given";

/// The store's key as an earlier version kept it in GNOME Keyring's login
/// keyring, its files opened natively with `password`.
fn moved_key(password: &str) -> Result<Zeroizing<[u8; 32]>, String> {
    let (keyrings, _) = legacy::open_all(password);
    if keyrings.is_empty() {
        return Err(format!("the key to the secrets programs keep was in GNOME Keyring's keyrings ({}), and your login password opens none of them", legacy::folder().display()));
    }
    let wanted: std::collections::BTreeMap<String, String> = olo_system::secrets::SECRETS_KEY.attributes.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect();
    let store = store_path();
    let mut candidates = keyrings.iter().flat_map(|k| &k.items).filter(|i| i.matches(&wanted)).collect::<Vec<_>>();
    // (The newest first: a key made again since is the one in use.)
    candidates.sort_by_key(|i| std::cmp::Reverse(i.modified));
    for item in candidates {
        if let Ok(k) = <[u8; 32]>::try_from(item.secret.as_slice())
            && store::opens(&store, &k)
        {
            return Ok(Zeroizing::new(k));
        }
    }
    Err("the key to the secrets programs keep isn't in GNOME Keyring's keyrings, so they can't be opened".into())
}

/// The store's key kept in `file` (tests), made the first time.
fn keyfile_key(file: &std::path::Path) -> Result<Zeroizing<[u8; 32]>, String> {
    match std::fs::read(file) {
        Ok(b) => {
            let b = Zeroizing::new(b);
            <[u8; 32]>::try_from(b.as_slice()).map(Zeroizing::new).map_err(|_| "the store's key isn't 32 bytes".into())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let k = store::new_key();
            std::fs::write(file, k.as_slice()).map_err(|e| e.to_string())?;
            Ok(k)
        }
        Err(e) => Err(e.to_string()),
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
    let Key { key, again, typed } = key()?;
    if again {
        seal_if_unsealed(&key);
    }
    let mut store = Store::open(&store_path(), key)?;
    // GNOME Keyring's items copied in, read natively from its files with
    // the login password when it was typed (the move, or asked for) and
    // they changed since the last copy (or were never copied). Nothing of
    // GNOME's runs for it.
    if let Some(password) = typed.as_deref().filter(|_| gnome::changed_since(store.data.imported_at)) {
        let (keyrings, unopened) = legacy::open_all(password);
        let found = keyrings.into_iter().flat_map(|k| k.items.into_iter().map(move |item| gnome::Found { collection: k.id.clone(), collection_label: k.label.clone(), item }));
        status.imported = import(&mut store, found.collect());
        if !unopened.is_empty() {
            tracing::info!("secrets: GNOME Keyring's {} don't open with the login password; not copied", unopened.join(", "));
        }
        status.skipped = unopened;
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

/// Whether to answer: in an Olo session (app_server's socket there),
/// always; outside one only for a test (`OLO_SECRETS_SERVE=1`).
fn wanted() -> bool {
    std::env::var("OLO_SECRETS_SERVE").as_deref() == Ok("1") || olo_protocol::socket_path().is_some()
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
    if args.iter().any(|a| a == "--activated") {
        return activated::run();
    }
    if args.iter().any(|a| a == "--status") {
        match std::fs::read_to_string(status_path()) {
            Ok(s) => println!("{s}"),
            Err(_) => println!("{}", serde_json::to_string_pretty(&Status::default()).unwrap_or_default()),
        }
        return std::process::ExitCode::SUCCESS;
    }
    if !args.is_empty() {
        eprintln!("usage: olo-secrets [--status | --activated]");
        return std::process::ExitCode::from(2);
    }
    if !wanted() {
        eprintln!("olo-secrets: not in an Olo session (OLO_SECRETS_SERVE=1 answers anyway, for a test)");
        return std::process::ExitCode::SUCCESS;
    }
    become_new_version_when_upgraded();
    let (tx, rx) = mpsc::channel();
    locked::serve(tx.clone());
    let mut note = locked::Note::new(tx.clone());
    let mut serving: Option<Serving> = None;
    // When the person last said Not Now (a program's request doesn't
    // ask again for a while after).
    let mut not_now: Option<std::time::Instant> = None;
    let mut status = Status::default();
    write_status(&status);
    // Taken over at once, and again when asked to unlock while locked (a
    // program's request not within a while of a Not Now).
    let _ = tx.send(locked::Msg::Unlock { asked_by_person: true, reply: None });
    drop(tx);
    for msg in rx {
        let locked::Msg::Unlock { asked_by_person, reply } = msg;
        let ask = asked_by_person || not_now.is_none_or(|t| t.elapsed() >= locked::QUIET_AFTER_NOT_NOW);
        if serving.is_none() && ask {
            status = Status::default();
            match take_over(&mut status) {
                Ok(s) => {
                    let imported = std::mem::take(&mut status.imported);
                    let skipped = std::mem::take(&mut status.skipped);
                    let healed = status.healed.take();
                    status = Status { imported, skipped, healed, ..status_of(&s.state.store.lock().unwrap_or_else(|e| e.into_inner()), true) };
                    forget_the_gone(&s.conn, s.state.clone());
                    serving = Some(s);
                    not_now = None;
                    note.close();
                }
                Err(e) => {
                    tracing::warn!("secrets: not answering for the keyring: {e}");
                    not_now = Some(std::time::Instant::now());
                    note.show(&e);
                    status.error = Some(e);
                    status.locked = true;
                }
            }
        }
        if let Some(reply) = reply {
            let _ = reply.send(serving.is_some());
        }
        write_status(&status);
    }
    std::process::ExitCode::SUCCESS
}
