//! GNOME Keyring's keyring files (`$XDG_DATA_HOME/keyrings/*.keyring`),
//! read natively with the login password: nothing of GNOME's runs. Only
//! for the move to Olo's keyring, once (Ben, 2026-11-15: "let's make
//! sure the migration doesn't depend on gnome"): an earlier olo-secrets
//! kept its store's key in GNOME's login keyring, and someone coming from
//! GNOME has their programs' secrets there (Chrome's and Slack's keys,
//! Wi-Fi passwords).
//!
//! The format is GNOME Keyring's own (gkm-secret-binary.c, "GnomeKeyring"
//! version 0.0): a header in the clear (the keyring's name, times, the
//! salt and iterations, each item's id, type and hashed attributes), then
//! every item's whole record (label, secret, times, attributes, ACL)
//! encrypted with AES-128-CBC. The key and IV are SHA-256 of the password
//! and salt, hashed again `iterations` times (egg-symkey.c's
//! `generate_simple`: the first 16 bytes the key, the next 16 the IV).
//! The decrypted records open with an MD5 of the rest, which is how a
//! wrong password shows.

use std::{collections::BTreeMap, path::Path};

use aes::cipher::{BlockModeDecrypt, KeyIvInit, block_padding::NoPadding};
use md5::Md5;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::store::Item;

const MAGIC: &[u8; 16] = b"GnomeKeyring\n\r\0\n";

/// One of GNOME Keyring's keyrings, opened.
pub struct Keyring {
    /// Its file's name without `.keyring` (`login`): its collection's id.
    pub id: String,
    /// Its label (`Login`).
    pub label: String,
    pub items: Vec<Item>,
}

/// What opening one keyring file with a password found.
pub enum Opened {
    Keyring(Keyring),
    /// It isn't this password.
    WrongPassword,
}

/// Every keyring in GNOME Keyring's folder (`$XDG_DATA_HOME/keyrings`),
/// as `password` opens each: those it opens, and the names of those it
/// doesn't (another password, or not a keyring file).
pub fn open_all(password: &str) -> (Vec<Keyring>, Vec<String>) {
    open_all_in(&folder(), password)
}

pub fn open_all_in(dir: &Path, password: &str) -> (Vec<Keyring>, Vec<String>) {
    let (mut opened, mut not) = (Vec::new(), Vec::new());
    let mut files: Vec<_> = std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "keyring")).collect();
    files.sort();
    for path in files {
        let id = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        match std::fs::read(&path).map_err(|e| e.to_string()).and_then(|b| open(&id, &b, password)) {
            Ok(Opened::Keyring(k)) => opened.push(k),
            Ok(Opened::WrongPassword) => not.push(id),
            Err(e) => {
                tracing::info!("secrets: GNOME Keyring's {} not read: {e}", path.display());
                not.push(id);
            }
        }
    }
    (opened, not)
}

/// Where GNOME Keyring keeps its keyrings (`$XDG_DATA_HOME/keyrings`).
pub fn folder() -> std::path::PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .filter(|d| !d.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share"))
        .join("keyrings")
}

/// The keyring file `bytes` (whose collection is `id`), opened with
/// `password`.
pub fn open(id: &str, bytes: &[u8], password: &str) -> Result<Opened, String> {
    let mut r = Reader { b: bytes, at: 0 };
    if r.take(16)? != MAGIC {
        return Err("not a GNOME Keyring file".into());
    }
    let (major, minor, crypto, hash) = (r.u8()?, r.u8()?, r.u8()?, r.u8()?);
    if (major, minor) != (0, 0) || crypto != 0 || hash != 0 {
        return Err(format!("a GNOME Keyring file of a kind not known (version {major}.{minor}, cipher {crypto}, hash {hash})"));
    }
    let label = r.string()?.unwrap_or_default();
    let (_ctime, _mtime) = (r.time()?, r.time()?);
    let (_flags, _lock_timeout, iterations) = (r.u32()?, r.u32()?, r.u32()?);
    let salt: [u8; 8] = r.take(8)?.try_into().map_err(|_| "short salt")?;
    for _ in 0..4 {
        r.u32()?;
    }
    let n = r.u32()? as usize;
    // The items' ids and types in the clear (their attributes hashed,
    // read past: the encrypted part has them as they are).
    let mut types = Vec::with_capacity(n.min(4096));
    for _ in 0..n {
        let (_id, kind) = (r.u32()?, r.u32()?);
        types.push(kind);
        for _ in 0..r.u32()? {
            r.string()?;
            match r.u32()? {
                0 => {
                    r.string()?;
                }
                1 => {
                    r.u32()?;
                }
                t => return Err(format!("an attribute of a kind not known ({t})")),
            }
        }
    }
    let size = r.u32()? as usize;
    if size % 16 != 0 || size < 16 {
        return Err("its encrypted part isn't whole blocks".into());
    }
    let mut data = Zeroizing::new(r.take(size)?.to_vec());
    let (key, iv) = derive(password, &salt, iterations);
    cbc::Decryptor::<aes::Aes128>::new((&*key).into(), (&*iv).into()).decrypt_padded::<NoPadding>(&mut data).map_err(|_| "its encrypted part couldn't be decrypted")?;
    if Md5::digest(&data[16..]).as_slice() != &data[..16] {
        return Ok(Opened::WrongPassword);
    }
    let mut r = Reader { b: &data, at: 16 };
    let mut items = Vec::with_capacity(types.len());
    for kind in types {
        let label = r.string()?.unwrap_or_default();
        let secret = Zeroizing::new(r.bytes()?.unwrap_or_default());
        let (created, modified) = (r.time()?, r.time()?);
        r.string()?;
        for _ in 0..4 {
            r.u32()?;
        }
        let mut attributes = BTreeMap::new();
        for _ in 0..r.u32()? {
            let name = r.string()?.unwrap_or_default();
            let value = match r.u32()? {
                0 => r.string()?.unwrap_or_default(),
                1 => r.u32()?.to_string(),
                t => return Err(format!("an attribute of a kind not known ({t})")),
            };
            attributes.insert(name, value);
        }
        // Its ACL (who may read it: GNOME Keyring's old access control).
        for _ in 0..r.u32()? {
            r.u32()?;
            r.string()?;
            r.string()?;
            r.string()?;
            r.u32()?;
        }
        // The Secret Service's schema, as GNOME Keyring shows it, from the
        // item's type when its attributes don't say.
        if let Some(schema) = schema(kind) {
            attributes.entry("xdg:schema".into()).or_insert_with(|| schema.into());
        }
        items.push(Item { id: 0, label, attributes, secret, content_type: "text/plain".into(), created, modified });
    }
    Ok(Opened::Keyring(Keyring { id: id.to_owned(), label, items }))
}

/// The schema GNOME Keyring gives an item of `kind` (its old item
/// types; the high bits are flags).
fn schema(kind: u32) -> Option<&'static str> {
    Some(match kind & 0xffff {
        0 => "org.freedesktop.Secret.Generic",
        1 => "org.gnome.keyring.NetworkPassword",
        2 => "org.gnome.keyring.Note",
        3 => "org.gnome.keyring.ChainedKeyring",
        4 => "org.gnome.keyring.EncryptionKey",
        0x100 => "org.gnome.keyring.PkStorage",
        _ => return None,
    })
}

/// AES-128's key and IV for `password` and `salt` (egg-symkey.c's
/// `generate_simple` with SHA-256: one digest gives both).
fn derive(password: &str, salt: &[u8; 8], iterations: u32) -> (Zeroizing<[u8; 16]>, Zeroizing<[u8; 16]>) {
    let mut h = Sha256::new();
    h.update(password.as_bytes());
    h.update(salt);
    let mut digest = Zeroizing::new(<[u8; 32]>::from(h.finalize()));
    for _ in 1..iterations.max(1) {
        *digest = Sha256::digest(&*digest).into();
    }
    let (mut key, mut iv) = (Zeroizing::new([0u8; 16]), Zeroizing::new([0u8; 16]));
    key.copy_from_slice(&digest[..16]);
    iv.copy_from_slice(&digest[16..]);
    (key, iv)
}

/// GNOME Keyring's buffer: big-endian numbers, strings as a length (all
/// ones: none) and their bytes, times as two halves.
struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.at.checked_add(n).filter(|e| *e <= self.b.len()).ok_or("the file ends too soon")?;
        let s = &self.b[self.at..end];
        self.at = end;
        Ok(s)
    }

    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn time(&mut self) -> Result<u64, String> {
        Ok((u64::from(self.u32()?) << 32) | u64::from(self.u32()?))
    }

    fn bytes(&mut self) -> Result<Option<Vec<u8>>, String> {
        match self.u32()? {
            u32::MAX => Ok(None),
            n => Ok(Some(self.take(n as usize)?.to_vec())),
        }
    }

    fn string(&mut self) -> Result<Option<String>, String> {
        Ok(self.bytes()?.map(|b| String::from_utf8_lossy(&b).into_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Made by GNOME Keyring 51.1 in a sandbox (its own bus and runtime
    /// folder): `gnome-keyring-daemon --login` given "fixture password",
    /// then three items stored with secret-tool.
    const FIXTURE: &[u8] = include_bytes!("../tests/data/gnome-login.keyring");

    fn opened() -> Keyring {
        match open("login", FIXTURE, "fixture password").unwrap() {
            Opened::Keyring(k) => k,
            Opened::WrongPassword => panic!("the fixture's password didn't open it"),
        }
    }

    #[test]
    fn its_password_opens_every_item() {
        let k = opened();
        assert_eq!((k.id.as_str(), k.label.as_str()), ("login", "Login"));
        let by_label = |l: &str| k.items.iter().find(|i| i.label == l).unwrap_or_else(|| panic!("no “{l}”"));
        let key = by_label("Olo Secrets");
        assert_eq!(key.secret.as_slice(), b"0123456789abcdef0123456789abcdef");
        assert_eq!(key.attributes.get("application").map(String::as_str), Some("olo"));
        assert_eq!(key.attributes.get("purpose").map(String::as_str), Some("secrets-key"));
        assert_eq!(by_label("Chrome Safe Storage").secret.as_slice(), b"peanuts");
        let wifi = by_label("Wi-Fi “Home”");
        assert_eq!(wifi.secret.as_slice(), b"hunter2");
        assert_eq!(wifi.attributes.get("xdg:schema").map(String::as_str), Some("org.freedesktop.NetworkManager.Connection"));
        assert_eq!(wifi.attributes.get("connection-uuid").map(String::as_str), Some("1234"));
        assert!(k.items.iter().all(|i| i.created > 1_700_000_000 && i.modified >= i.created));
    }

    #[test]
    fn another_password_doesnt() {
        assert!(matches!(open("login", FIXTURE, "fixture passwore").unwrap(), Opened::WrongPassword));
        assert!(matches!(open("login", FIXTURE, "").unwrap(), Opened::WrongPassword));
    }

    #[test]
    fn a_damaged_file_is_said_to_be() {
        assert!(open("login", b"GnomeKeyring\n\r\0\n\x00\x00", "x").is_err());
        assert!(open("login", b"not a keyring at all", "x").is_err());
        for cut in [20, 100, FIXTURE.len() - 1] {
            assert!(open("login", &FIXTURE[..cut], "fixture password").is_err(), "cut at {cut}");
        }
    }

    #[test]
    fn a_folder_is_opened_keyring_by_keyring() {
        let dir = std::env::temp_dir().join(format!("olo-secrets-legacy-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("login.keyring"), FIXTURE).unwrap();
        std::fs::write(dir.join("Other.keyring"), b"GnomeKeyring\n\r\0\nbroken").unwrap();
        std::fs::write(dir.join("user.keystore"), b"an old one, left alone").unwrap();
        let (opened, not) = open_all_in(&dir, "fixture password");
        assert_eq!((opened.len(), not), (1, vec!["Other".to_owned()]));
        assert_eq!(opened[0].items.len(), 3);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
