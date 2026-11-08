//! Where backups go and the key that opens them: a store made from the
//! settings (with their speed limits), the key from the login keyring.

use std::{path::PathBuf, sync::Arc};

use olo_diskbackup::{Key, Limit, LocalStore, Store, Throttled};
use olo_protocol::diskbackup::{Settings, Target};
use olo_system::secrets;

/// The speed limits every store paces itself by, changed live.
pub struct Limits {
    pub up: Arc<Limit>,
    pub down: Arc<Limit>,
}

impl Limits {
    pub fn new() -> Limits {
        Limits { up: Arc::new(Limit::default()), down: Arc::new(Limit::default()) }
    }

    /// As `settings` say now.
    pub fn follow(&self, s: &Settings) {
        self.up.set(s.upload_kbs * 1000);
        self.down.set(s.download_kbs * 1000);
    }
}

/// The store `target` names, paced by `limits`.
pub fn store(target: &Target, limits: &Limits) -> Result<Arc<dyn Store>, String> {
    match target {
        Target::None => Err("not set up yet".into()),
        Target::Folder { path } => {
            let path = expand(path);
            if !path.is_dir() {
                return Err(format!("{} isn't there (a disk not plugged in?)", path.display()));
            }
            Ok(Arc::new(Throttled { inner: LocalStore::new(path), up: limits.up.clone(), down: limits.down.clone() }))
        }
        Target::GoogleDrive { account, folder } => {
            let g = google()?;
            Ok(Arc::new(olo_diskbackup::drive::Drive::new(g.client, g.refresh_token, account.clone(), folder.clone(), limits.up.clone(), limits.down.clone(), endpoints())))
        }
    }
}

/// DiskBackup's sign-in to Google Drive: the person's own OAuth client,
/// and the refresh token it was given.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Google {
    pub client: olo_diskbackup::oauth::Client,
    pub refresh_token: String,
}

/// Google's addresses (`OLO_DISKBACKUP_GOOGLE_API`: a test's own Google,
/// its `/auth`, `/token` and APIs).
pub fn endpoints() -> olo_diskbackup::oauth::Endpoints {
    match std::env::var("OLO_DISKBACKUP_GOOGLE_API") {
        Ok(base) => olo_diskbackup::oauth::Endpoints { auth: format!("{base}/auth"), token: format!("{base}/token"), api: base },
        Err(_) => Default::default(),
    }
}

/// The sign-in, from the login keyring (`OLO_DISKBACKUP_GOOGLE`: JSON,
/// for tests).
pub fn google() -> Result<Google, String> {
    let text = match std::env::var("OLO_DISKBACKUP_GOOGLE") {
        Ok(t) => t.into_bytes(),
        Err(_) => match secrets::lookup(secrets::DISKBACKUP_GOOGLE.attributes) {
            Ok(Some(bytes)) => bytes,
            Ok(None) => return Err("DiskBackup isn't signed in to Google Drive on this computer: set it up again (Back Up To ▸ Google Drive…)".into()),
            Err(e) => return Err(format!("the keyring can't be read: {e}")),
        },
    };
    serde_json::from_slice(&text).map_err(|_| "DiskBackup's Google sign-in in the keyring is damaged: set it up again".into())
}

/// Keep the sign-in in the login keyring.
pub fn keep_google(g: &Google) -> Result<(), String> {
    if std::env::var_os("OLO_DISKBACKUP_GOOGLE").is_some() {
        return Ok(());
    }
    let json = serde_json::to_vec(g).unwrap_or_default();
    secrets::store(secrets::DISKBACKUP_GOOGLE.label, secrets::DISKBACKUP_GOOGLE.attributes, &json).map_err(|e| format!("the keyring can't keep it: {e}"))
}

/// The OAuth client's item in Passwords (so another computer signs in
/// without making one again).
pub const CLIENT_TITLE: &str = "DiskBackup Google Client";

/// The client kept in Passwords, if one is (Passwords started, unlocked
/// if it asks). Blocks.
pub fn client_in_passwords() -> Result<Option<olo_diskbackup::oauth::Client>, String> {
    let request = olo_keychain::Request::Secrets { title: CLIENT_TITLE.into() };
    match passwords(&request)? {
        olo_keychain::Response::Secrets { ref values } => Ok(values.iter().find_map(|v| serde_json::from_str(v).ok())),
        ref other => Err(format!("Passwords answered {other:?}")),
    }
}

/// Keep the client in Passwords.
pub fn keep_client(client: &olo_diskbackup::oauth::Client) -> Result<(), String> {
    let request = olo_keychain::Request::KeepSecret {
        title: CLIENT_TITLE.into(),
        secret: serde_json::to_string(client).unwrap_or_default(),
        note: "DiskBackup's own sign-in to Google Drive (a Desktop app OAuth client in your Google Cloud project). On another computer, DiskBackup signs in with it, without making another.".into(),
    };
    match passwords(&request)? {
        olo_keychain::Response::Saved { .. } => Ok(()),
        ref other => Err(format!("Passwords answered {other:?}")),
    }
}

/// A shared lock on `repo` (to restore from it), waiting while it's being
/// tidied (`waiting` told why) until `cancel`.
pub fn lock_to_read(repo: &olo_diskbackup::Repo, cancel: &std::sync::atomic::AtomicBool, waiting: impl Fn(&str)) -> Result<olo_diskbackup::Lock, olo_diskbackup::Error> {
    loop {
        match repo.lock(false) {
            Err(olo_diskbackup::Error::Busy(why)) => {
                waiting(&why);
                for _ in 0..20 {
                    if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                        return Err(olo_diskbackup::Error::Cancelled);
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            }
            r => return r,
        }
    }
}

/// `f` with `repo`; once more with its index read again if it fails (the
/// backup tidied since it was opened: what moved, found where it went).
pub fn retrying<T>(repo: &std::sync::RwLock<olo_diskbackup::Repo>, f: impl Fn(&olo_diskbackup::Repo) -> olo_diskbackup::Result<T>) -> Result<T, String> {
    fn poisoned<E>(_: E) -> String {
        "the backup isn't open".into()
    }
    match f(&*repo.read().map_err(poisoned)?) {
        Ok(t) => return Ok(t),
        Err(olo_diskbackup::Error::Cancelled) => return Err(olo_diskbackup::Error::Cancelled.to_string()),
        Err(_) => {}
    }
    repo.write().map_err(poisoned)?.refresh().map_err(|e| e.to_string())?;
    f(&*repo.read().map_err(poisoned)?).map_err(|e| e.to_string())
}

/// `~/…` as the home folder.
pub fn expand(p: &str) -> PathBuf {
    match p.strip_prefix('~') {
        Some(rest) => PathBuf::from(format!("{}{rest}", home().display())),
        None => PathBuf::from(p),
    }
}

pub fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

/// Where what's kept locally goes (the index's copies).
pub fn cache() -> PathBuf {
    olo_xdg::cache_dir().join("diskbackup")
}

/// The key, from the login keyring (where the service finds it with no
/// one there), or `OLO_DISKBACKUP_KEY` (tests).
pub fn key() -> Result<Key, String> {
    if let Ok(hex) = std::env::var("OLO_DISKBACKUP_KEY") {
        return Key::from_hex(&hex).ok_or_else(|| "OLO_DISKBACKUP_KEY isn't a key".into());
    }
    match secrets::lookup(secrets::DISKBACKUP_KEY.attributes) {
        Ok(Some(bytes)) => Key::from_hex(&String::from_utf8_lossy(&bytes)).ok_or_else(|| "the key in the keyring is damaged".into()),
        Ok(None) => Err("the key isn't in this computer's keyring: set DiskBackup up again, or put its key back from Passwords".into()),
        Err(e) => Err(format!("the keyring can't be read: {e}")),
    }
}

/// Keep the key in the login keyring.
pub fn keep_key(key: &Key) -> Result<(), String> {
    if std::env::var_os("OLO_DISKBACKUP_KEY").is_some() {
        return Ok(());
    }
    secrets::store(secrets::DISKBACKUP_KEY.label, secrets::DISKBACKUP_KEY.attributes, key.to_hex().as_bytes()).map_err(|e| format!("the keyring can't keep it: {e}"))
}

/// The key's item in Passwords.
pub const KEY_TITLE: &str = "DiskBackup Key";

/// The keys Passwords keeps for DiskBackup (one a backup set up anywhere),
/// asked for once Passwords is unlocked (started if it isn't running).
/// Blocks.
pub fn keys_in_passwords() -> Result<Vec<Key>, String> {
    let request = olo_keychain::Request::Secrets { title: KEY_TITLE.into() };
    match passwords(&request)? {
        olo_keychain::Response::Secrets { ref values } => Ok(values.iter().filter_map(|v| Key::from_hex(v)).collect()),
        ref other => Err(format!("Passwords answered {other:?}")),
    }
}

/// `request` to Passwords, starting it first if it isn't running.
pub fn passwords(request: &olo_keychain::Request) -> Result<olo_keychain::Response, String> {
    let mut started = false;
    loop {
        match olo_keychain::call(request) {
            Ok(olo_keychain::Response::Error { ref message }) => return Err(message.clone()),
            Ok(r) => return Ok(r),
            Err(olo_keychain::CallError::NotRunning) if !started => {
                started = true;
                let _ = olo_xdg::spawn_detached("olo-passwords");
                for _ in 0..50 {
                    std::thread::sleep(std::time::Duration::from_millis(200));
                    if olo_keychain::socket_path().exists() {
                        break;
                    }
                }
            }
            Err(e) => return Err(e.to_string()),
        }
    }
}
