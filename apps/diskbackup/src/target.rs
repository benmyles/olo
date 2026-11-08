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
        Target::GoogleDrive { .. } => Err("Google Drive isn't ready yet".into()),
    }
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
