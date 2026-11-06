//! Where the pool keeps itself: `pool.vault` in the config directory,
//! sealed as Passwords seals its store ("TABP", a format byte, a 24-byte
//! nonce, then the JSON sealed with XChaCha20-Poly1305, the header as
//! associated data). The key is 32 random bytes in the login keyring,
//! which PAM unlocks at login, so the gateway can work (and save the
//! sign-ins its providers rotate) without asking; `OLO_POOL_KEYFILE`
//! keeps the key in a file instead (tests).

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit, Payload},
};
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

use crate::model::{Account, Credential};

const MAGIC: &[u8; 4] = b"TABP";
const FORMAT: u8 = 1;
// (Kept with every other secret of Olo's, which Backup takes along.)
const KEY_ATTRS: &[(&str, &str)] = olo_system::secrets::POOL_KEY.attributes;

/// Everything kept: the accounts and, by account id, their sign-ins.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Saved {
    pub accounts: Vec<Account>,
    pub credentials: BTreeMap<String, Credential>,
}

pub fn path() -> PathBuf {
    olo_xdg::config_dir().join("pool.vault")
}

/// Beside the store, put there by a backup imported on another computer
/// (Preferences' `backup.rs` names it too): its accounts are paused as
/// it's next opened.
pub const ARRIVED: &str = "pool-arrived.json";

fn keyfile() -> Option<PathBuf> {
    std::env::var_os("OLO_POOL_KEYFILE").map(PathBuf::from)
}

pub fn random<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    getrandom::fill(&mut b).expect("random bytes");
    b
}

/// A new account id: 16 hex digits.
pub fn new_id() -> String {
    random::<8>().iter().map(|b| format!("{b:02x}")).collect()
}

/// A random (version 4) UUID.
pub fn new_uuid() -> String {
    let mut b = random::<16>();
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..])
}

/// The key: from the key file or the login keyring; made (and kept) when
/// there is none and nothing sealed yet.
fn key(sealed_exists: bool) -> Result<Zeroizing<[u8; 32]>, String> {
    let missing = || "The pool’s key is missing from the login keyring, so its accounts can’t be opened.".to_owned();
    let mut bytes = match keyfile() {
        Some(file) => match std::fs::read(&file) {
            Ok(b) => Zeroizing::new(b),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && !sealed_exists => {
                let k = Zeroizing::new(random::<32>().to_vec());
                write_private(&file, &k).map_err(|e| format!("Couldn’t keep the key: {e}"))?;
                k
            }
            Err(_) if sealed_exists => return Err(missing()),
            Err(e) => return Err(format!("Couldn’t read the key: {e}")),
        },
        None => match olo_system::secrets::lookup(KEY_ATTRS) {
            Ok(Some(b)) => Zeroizing::new(b),
            Ok(None) if sealed_exists => return Err(missing()),
            Ok(None) => {
                let k = Zeroizing::new(random::<32>().to_vec());
                olo_system::secrets::store(olo_system::secrets::POOL_KEY.label, KEY_ATTRS, &k).map_err(|e| format!("Couldn’t keep the key in the login keyring: {e}"))?;
                k
            }
            Err(e) => return Err(format!("Couldn’t reach the login keyring: {e}")),
        },
    };
    let key: [u8; 32] = bytes.as_slice().try_into().map_err(|_| "The pool’s key is damaged.".to_owned())?;
    bytes.zeroize();
    Ok(Zeroizing::new(key))
}

/// Write `bytes` to `path` readable only by us, replacing it whole.
pub fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    std::fs::rename(tmp, path)
}

fn seal(key: &[u8; 32], plain: &[u8]) -> Vec<u8> {
    let nonce = random::<24>();
    let mut header = MAGIC.to_vec();
    header.push(FORMAT);
    let sealed = XChaCha20Poly1305::new(key.into()).encrypt(&XNonce::from(nonce), Payload { msg: plain, aad: &header }).expect("sealing");
    let mut out = header;
    out.extend_from_slice(&nonce);
    out.extend(sealed);
    out
}

fn open(key: &[u8; 32], bytes: &[u8]) -> Result<Zeroizing<Vec<u8>>, String> {
    let damaged = || "The pool’s file is damaged.".to_owned();
    if bytes.len() < 5 + 24 || &bytes[..4] != MAGIC || bytes[4] != FORMAT {
        return Err(damaged());
    }
    let (header, rest) = bytes.split_at(5);
    let (nonce, sealed) = rest.split_at(24);
    XChaCha20Poly1305::new(key.into())
        .decrypt(&XNonce::try_from(nonce).map_err(|_| damaged())?, Payload { msg: sealed, aad: header })
        .map(Zeroizing::new)
        .map_err(|_| "The pool’s file can’t be opened with its key.".to_owned())
}

/// The sealed store, with its key (opened once).
pub struct Store {
    path: PathBuf,
    key: Zeroizing<[u8; 32]>,
}

impl Store {
    /// Open (blocks on the keyring): the store and what it holds.
    pub fn open() -> Result<(Store, Saved), String> {
        Store::open_at(path())
    }

    pub fn open_at(path: PathBuf) -> Result<(Store, Saved), String> {
        let exists = path.exists();
        let key = key(exists)?;
        let mut saved = if exists {
            let bytes = std::fs::read(&path).map_err(|e| format!("Couldn’t read the pool: {e}"))?;
            let plain = open(&key, &bytes)?;
            serde_json::from_slice(&plain).map_err(|_| "The pool’s file is damaged.".to_owned())?
        } else {
            Saved::default()
        };
        let store = Store { path, key };
        // Brought from another computer by a backup (Preferences ▸ Backup
        // leaves this beside it): its accounts are paused here till the
        // person turns them on. Their sign-ins renew themselves, and two
        // computers renewing one would sign each other out.
        let arrived = store.path.with_file_name(ARRIVED);
        if arrived.exists() {
            for a in &mut saved.accounts {
                a.enabled = false;
            }
            store.save(&saved)?;
            let _ = std::fs::remove_file(&arrived);
            tracing::info!(accounts = saved.accounts.len(), "pool: from another computer, its accounts paused");
        }
        Ok((store, saved))
    }

    /// Seal and write `saved` (blocks on the disk: call it off the UI and
    /// async threads).
    pub fn save(&self, saved: &Saved) -> Result<(), String> {
        let plain = Zeroizing::new(serde_json::to_vec(saved).map_err(|e| e.to_string())?);
        write_private(&self.path, &seal(&self.key, &plain)).map_err(|e| format!("Couldn’t save the pool: {e}"))
    }

    /// What the file holds now (another copy of the pool, the one an
    /// upgrade handed over to or from, may have written it since).
    pub fn read(&self) -> Result<Saved, String> {
        let bytes = std::fs::read(&self.path).map_err(|e| format!("Couldn’t read the pool: {e}"))?;
        let plain = open(&self.key, &bytes)?;
        serde_json::from_slice(&plain).map_err(|_| "The pool’s file is damaged.".to_owned())
    }

    /// Account `id`'s sign-in written in, the rest of the file as it is:
    /// what a copy of the pool finishing its last requests keeps (the new
    /// one owns everything else).
    pub fn put_credential(&self, id: &str, credential: &Credential) -> Result<(), String> {
        let mut saved = self.read()?;
        saved.credentials.insert(id.to_owned(), credential.clone());
        self.save(&saved)
    }

    /// Held while a sign-in is renewed or written: two copies of the pool
    /// live at once (an upgrade handing over) never renew one twice, which
    /// would sign it out where the provider rotates its refresh token.
    /// Blocks: off the async threads.
    pub fn lock(&self) -> Option<std::fs::File> {
        use std::os::fd::AsRawFd as _;
        let f = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(self.path.with_extension("lock")).ok()?;
        // SAFETY: flock on a file we hold open.
        (unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) } == 0).then_some(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Provider;

    #[test]
    fn seals_and_opens() {
        let dir = tempfile::tempdir().unwrap();
        // SAFETY: tests in this crate that touch the environment set the
        // same value.
        unsafe { std::env::set_var("OLO_POOL_KEYFILE", dir.path().join("key")) };
        let path = dir.path().join("pool.vault");
        let (store, saved) = Store::open_at(path.clone()).unwrap();
        assert!(saved.accounts.is_empty());
        let mut s = Saved::default();
        s.accounts.push(Account::new("a1".into(), "Work".into(), Provider::Claude));
        s.credentials.insert("a1".into(), Credential { access: "tok-secret".into(), refresh: None, expires_at: None, account_id: None });
        store.save(&s).unwrap();
        let raw = std::fs::read(&path).unwrap();
        assert!(!String::from_utf8_lossy(&raw).contains("tok-secret"));
        let (_, back) = Store::open_at(path.clone()).unwrap();
        assert_eq!(back.accounts[0].name, "Work");
        assert_eq!(back.credentials["a1"].access, "tok-secret");
        // A damaged file says so rather than starting empty.
        let mut bad = raw.clone();
        let n = bad.len();
        bad[n - 1] ^= 1;
        std::fs::write(&path, bad).unwrap();
        assert!(Store::open_at(path.clone()).is_err());

        // From another computer: its accounts come in paused, once.
        store.save(&s).unwrap();
        std::fs::write(dir.path().join(ARRIVED), b"{}").unwrap();
        let (_, here) = Store::open_at(path.clone()).unwrap();
        assert!(!here.accounts[0].enabled);
        assert!(!dir.path().join(ARRIVED).exists());
        let (store, mut again) = Store::open_at(path.clone()).unwrap();
        again.accounts[0].enabled = true;
        store.save(&again).unwrap();
        assert!(Store::open_at(path).unwrap().1.accounts[0].enabled, "turned on again, it stays on");
    }
}
