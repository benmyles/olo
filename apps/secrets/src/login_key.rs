//! The store's key, sealed by the login password: `secrets.login`
//! beside the store, "OLOL", a format byte, a 24-byte nonce, then the
//! 32-byte key sealed with XChaCha20-Poly1305 under the key the login
//! password gives (`olo_login_key::login_key`; the header as associated
//! data). At login pam_olo_keyring hands olo-secrets that key (the
//! kernel keyring, `kernel::take_login_key`), so the store opens with
//! the password just typed and nothing else; GNOME Keyring isn't needed.

use std::path::{Path, PathBuf};

use chacha20poly1305::{
    KeyInit as _, XChaCha20Poly1305, XNonce,
    aead::{Aead as _, Payload},
};
use zeroize::Zeroizing;

const MAGIC: &[u8; 4] = b"OLOL";
const FORMAT: u8 = 1;

/// Where it's kept, for the store at `store`.
pub fn path(store: &Path) -> PathBuf {
    store.with_extension("login")
}

/// `key` sealed by `login`.
pub fn seal(login: &[u8; 32], key: &[u8; 32]) -> Vec<u8> {
    let mut nonce = [0u8; 24];
    getrandom::fill(&mut nonce).expect("random");
    let mut header = MAGIC.to_vec();
    header.push(FORMAT);
    let sealed = XChaCha20Poly1305::new(login.into()).encrypt(&XNonce::from(nonce), Payload { msg: key, aad: &header }).expect("sealing");
    let mut out = header;
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&sealed);
    out
}

/// The key `bytes` seal, if `login` opens them.
pub fn open(login: &[u8; 32], bytes: &[u8]) -> Option<Zeroizing<[u8; 32]>> {
    if bytes.len() < 5 + 24 || &bytes[..4] != MAGIC || bytes[4] != FORMAT {
        return None;
    }
    let (header, rest) = bytes.split_at(5);
    let (nonce, sealed) = rest.split_at(24);
    let plain = Zeroizing::new(XChaCha20Poly1305::new(login.into()).decrypt(&XNonce::try_from(nonce).ok()?, Payload { msg: sealed, aad: header }).ok()?);
    let mut key = Zeroizing::new([0u8; 32]);
    (plain.len() == 32).then(|| {
        key.copy_from_slice(&plain);
        key
    })
}

/// Keep `key` sealed by `login` for the store at `store` (written whole,
/// renamed into place, only the person may read it).
pub fn keep(store: &Path, login: &[u8; 32], key: &[u8; 32]) -> Result<(), String> {
    use std::{io::Write as _, os::unix::fs::OpenOptionsExt as _};
    let path = path(store);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("login.tmp");
    let written = (|| {
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
        f.write_all(&seal(login, key))?;
        f.sync_all()?;
        std::fs::rename(&tmp, &path)
    })();
    written.map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opened_by_the_login_key_alone() {
        let (login, other, key) = ([1u8; 32], [2u8; 32], [7u8; 32]);
        let sealed = seal(&login, &key);
        assert_eq!(open(&login, &sealed).as_deref(), Some(&key));
        assert!(open(&other, &sealed).is_none());
        let mut damaged = sealed.clone();
        *damaged.last_mut().unwrap() ^= 1;
        assert!(open(&login, &damaged).is_none());
        assert!(open(&login, b"OLOL").is_none());
        assert_eq!(path(Path::new("/c/olo/secrets.store")), Path::new("/c/olo/secrets.login"));
    }
}
