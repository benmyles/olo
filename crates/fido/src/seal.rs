//! The account password sealed by a key: set up once (a credential made,
//! then its `hmac-secret` for a random salt: two touches), the password
//! kept encrypted under that secret (XChaCha20-Poly1305), and given back
//! only after a touch of that key. Kept in `security-keys.json` in Olo's
//! config directory, readable by the person alone; useless without the
//! key.

use std::path::PathBuf;

use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit, Payload},
};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use zeroize::Zeroizing;

use crate::{Error, Result};

/// One key set up: a credential on it and the password sealed by it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sealed {
    /// What it is, as its driver named it when it was set up.
    pub name: String,
    /// Its model's AAGUID (hex).
    pub aaguid: String,
    /// Whose password.
    pub user: String,
    /// When it was set up (seconds since 1970).
    pub added: u64,
    pub credential: String,
    pub salt: String,
    pub nonce: String,
    pub sealed: String,
}

impl Sealed {
    pub fn credential_id(&self) -> Result<Vec<u8>> {
        B64.decode(&self.credential).map_err(|_| Error::Damaged)
    }

    pub fn salt(&self) -> Result<[u8; 32]> {
        B64.decode(&self.salt).ok().and_then(|s| s.try_into().ok()).ok_or(Error::Damaged)
    }
}

/// `password` sealed by `secret` (credential `credential`'s for `salt`).
pub fn seal(name: &str, aaguid: &[u8; 16], user: &str, credential: &[u8], salt: &[u8; 32], secret: &[u8; 32], password: &str) -> Result<Sealed> {
    let mut nonce = [0u8; 24];
    getrandom::fill(&mut nonce).map_err(|_| Error::Random)?;
    let aad = aad(user, credential);
    let key = key(secret)?;
    let sealed = XChaCha20Poly1305::new((&*key).into()).encrypt(&XNonce::from(nonce), Payload { msg: password.as_bytes(), aad: &aad }).map_err(|_| Error::Damaged)?;
    let added = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    Ok(Sealed {
        name: name.to_owned(),
        aaguid: aaguid.iter().map(|b| format!("{b:02x}")).collect(),
        user: user.to_owned(),
        added,
        credential: B64.encode(credential),
        salt: B64.encode(salt),
        nonce: B64.encode(nonce),
        sealed: B64.encode(sealed),
    })
}

/// The password `s` holds, given its credential's secret.
pub fn open(s: &Sealed, secret: &[u8; 32]) -> Result<Zeroizing<String>> {
    let nonce: [u8; 24] = B64.decode(&s.nonce).ok().and_then(|n| n.try_into().ok()).ok_or(Error::Damaged)?;
    let sealed = B64.decode(&s.sealed).map_err(|_| Error::Damaged)?;
    let aad = aad(&s.user, &s.credential_id()?);
    let key = key(secret)?;
    let plain = Zeroizing::new(XChaCha20Poly1305::new((&*key).into()).decrypt(&XNonce::from(nonce), Payload { msg: &sealed, aad: &aad }).map_err(|_| Error::WrongKey)?);
    String::from_utf8(plain.to_vec()).map(Zeroizing::new).map_err(|_| Error::Damaged)
}

fn key(secret: &[u8; 32]) -> Result<Zeroizing<[u8; 32]>> {
    let mut k = Zeroizing::new([0u8; 32]);
    hkdf::Hkdf::<Sha256>::new(None, secret).expand(b"olo: the account password", &mut *k).map_err(|_| Error::Protocol("HKDF"))?;
    Ok(k)
}

fn aad(user: &str, credential: &[u8]) -> Vec<u8> {
    [b"olo-fido-seal-1\0".as_slice(), user.as_bytes(), b"\0", credential].concat()
}

/// Where the keys set up are kept.
pub fn path() -> PathBuf {
    olo_xdg::config_dir().join("security-keys.json")
}

#[derive(Default, Serialize, Deserialize)]
struct File {
    keys: Vec<Sealed>,
}

/// The keys set up, for `user` (none if the file is missing or damaged).
pub fn load(user: &str) -> Vec<Sealed> {
    std::fs::read(path()).ok().and_then(|b| serde_json::from_slice::<File>(&b).ok()).map(|f| f.keys.into_iter().filter(|k| k.user == user).collect()).unwrap_or_default()
}

/// Keep `keys` (every user's in the file but `user`'s stay as they were).
pub fn save(user: &str, keys: &[Sealed]) -> Result<()> {
    let mut all: Vec<Sealed> = std::fs::read(path()).ok().and_then(|b| serde_json::from_slice::<File>(&b).ok()).map(|f| f.keys).unwrap_or_default();
    all.retain(|k| k.user != user);
    all.extend(keys.iter().cloned());
    let p = path();
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_vec_pretty(&File { keys: all }).map_err(|_| Error::Damaged)?;
    // Written beside it, readable by the person alone, then put in place.
    let tmp = p.with_extension("json.saving");
    {
        use std::{io::Write as _, os::unix::fs::OpenOptionsExt as _};
        let _ = std::fs::remove_file(&tmp);
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
        f.write_all(&json)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, &p)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sealed_opens_with_its_secret_only() {
        let secret = [9u8; 32];
        let s = seal("Yubico YubiKey OTP+FIDO+CCID", &[1; 16], "ben", b"cred", &[3; 32], &secret, "hunter2 ünïcode").unwrap();
        assert_eq!(&*open(&s, &secret).unwrap(), "hunter2 ünïcode");
        assert!(matches!(open(&s, &[8u8; 32]), Err(Error::WrongKey)));
        // Another user's or credential's name on it: not opened.
        let other = Sealed { user: "eve".into(), ..s.clone() };
        assert!(open(&other, &secret).is_err());
        assert_eq!(s.salt().unwrap(), [3; 32]);
        assert!(!s.sealed.contains("hunter2"));
    }
}
