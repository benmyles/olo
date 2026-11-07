//! The store: every collection and item olo-secrets keeps, in
//! `$XDG_CONFIG_HOME/olo/secrets.store`: "OLOS", a format byte, a
//! 24-byte nonce, then the JSON sealed with XChaCha20-Poly1305 (the header
//! as associated data), as Passwords' vault is. Its key is 32 random bytes
//! sealed by the login password beside it (`login_key.rs`), and kept in
//! GNOME Keyring's login keyring too where it was made there
//! (`olo_system::secrets::SECRETS_KEY`); `OLO_SECRETS_KEYFILE` keeps it
//! in a file instead (tests). The session collection is kept in memory
//! only, as GNOME Keyring's is.

use std::{
    collections::{BTreeMap, BTreeSet},
    io::Write as _,
    os::unix::fs::OpenOptionsExt as _,
    path::{Path, PathBuf},
};

use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit, Payload},
};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

const MAGIC: &[u8; 4] = b"OLOS";
const FORMAT: u8 = 1;

/// The collection kept in memory only (libsecret's
/// `SECRET_COLLECTION_SESSION`).
pub const SESSION: &str = "session";
/// The collection made first, the `default` alias's.
pub const LOGIN: &str = "login";

/// The `xdg:schema` GNOME Keyring gives an item made without one; Olo's
/// do too, so an item looks the same here as there (and is replaced
/// alike: `put`).
pub const GENERIC: &str = "org.freedesktop.Secret.Generic";

/// What `Store::heal` mended.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Healed {
    /// Collections merged into the login one (a second "Login").
    pub merged: Vec<String>,
    /// Items that were another's copy (the newest of each kept).
    pub duplicates: usize,
    /// Items given GNOME's `xdg:schema`.
    pub schemas: usize,
}

impl Healed {
    pub fn is_empty(&self) -> bool {
        self.merged.is_empty() && self.duplicates == 0 && self.schemas == 0
    }
}

pub fn path() -> PathBuf {
    olo_xdg::config_dir().join("secrets.store")
}

/// Seconds since 1970, as the Secret Service's `Created` and `Modified`.
pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

mod b64 {
    use base64::Engine as _;
    use serde::{Deserialize, Deserializer, Serializer};
    use zeroize::Zeroizing;

    pub fn serialize<S: Serializer>(v: &Zeroizing<Vec<u8>>, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&base64::engine::general_purpose::STANDARD.encode(v.as_slice()))
    }

    pub fn deserialize<'d, D: Deserializer<'d>>(d: D) -> Result<Zeroizing<Vec<u8>>, D::Error> {
        let s = Zeroizing::new(String::deserialize(d)?);
        base64::engine::general_purpose::STANDARD.decode(s.as_bytes()).map(Zeroizing::new).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Item {
    /// Its number in its collection (its object path's last part).
    pub id: u64,
    pub label: String,
    pub attributes: BTreeMap<String, String>,
    #[serde(with = "b64")]
    pub secret: Zeroizing<Vec<u8>>,
    pub content_type: String,
    pub created: u64,
    pub modified: u64,
}

impl Item {
    /// Whether it has every one of `wanted` (an empty search: every item).
    pub fn matches(&self, wanted: &BTreeMap<String, String>) -> bool {
        wanted.iter().all(|(k, v)| self.attributes.get(k) == Some(v))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Collection {
    /// Its object path's last part (`login`).
    pub id: String,
    pub label: String,
    pub created: u64,
    pub modified: u64,
    /// The next item's number.
    pub next: u64,
    pub items: Vec<Item>,
}

impl Collection {
    pub fn new(id: &str, label: &str) -> Collection {
        let t = now();
        Collection { id: id.to_owned(), label: label.to_owned(), created: t, modified: t, next: 1, items: Vec::new() }
    }

    pub fn item(&self, id: u64) -> Option<&Item> {
        self.items.iter().find(|i| i.id == id)
    }

    pub fn item_mut(&mut self, id: u64) -> Option<&mut Item> {
        self.items.iter_mut().find(|i| i.id == id)
    }
}

/// The attributes that say which item it is: all but libsecret's
/// `xdg:schema`, which GNOME Keyring adds to an item that had none once
/// it has read it back from its file.
pub fn core(attributes: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    attributes.iter().filter(|(k, _)| *k != "xdg:schema").map(|(k, v)| (k.clone(), v.clone())).collect()
}

/// An item's identity across stores (here and GNOME Keyring), for copying
/// one way and back: its collection, label and attributes.
pub fn identity(collection: &str, label: &str, attributes: &BTreeMap<String, String>) -> String {
    format!("{collection}\n{label}\n{}", serde_json::to_string(&core(attributes)).unwrap_or_default())
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Data {
    pub collections: Vec<Collection>,
    /// Alias → collection id (`default` → `login`).
    pub aliases: BTreeMap<String, String>,
    /// GNOME Keyring's items as last copied here: identity → its
    /// `Modified` then (one changed there since is copied again).
    pub imported: BTreeMap<String, u64>,
    /// Since when Olo answers for the keyring (None: it doesn't).
    pub serving_since: Option<u64>,
    /// Items made or changed here while serving, and ones deleted (their
    /// identities): written into GNOME Keyring, or deleted there, when
    /// handing back.
    pub changed: BTreeSet<String>,
    pub deleted: BTreeSet<String>,
    /// When GNOME Keyring's items were last copied here.
    pub imported_at: Option<u64>,
}

pub struct Store {
    path: PathBuf,
    key: Zeroizing<[u8; 32]>,
    pub data: Data,
}

impl Store {
    /// The store at `path` opened with `key`, or a new one (with the login
    /// collection, the default) if there is none.
    pub fn open(path: &Path, key: Zeroizing<[u8; 32]>) -> Result<Store, String> {
        let mut data = match std::fs::read(path) {
            Ok(bytes) => {
                let plain = open(&key, &bytes)?;
                serde_json::from_slice::<Data>(&plain).map_err(|e| format!("{}: {e}", path.display()))?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Data::default(),
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        if !data.collections.iter().any(|c| c.id == LOGIN) {
            data.collections.insert(0, Collection::new(LOGIN, "Login"));
        }
        data.aliases.entry("default".into()).or_insert_with(|| LOGIN.into());
        data.collections.retain(|c| c.id != SESSION);
        data.collections.push(Collection::new(SESSION, "Session"));
        Ok(Store { path: path.to_owned(), key, data })
    }

    /// Written out (the session collection left out), atomically.
    pub fn save(&self) -> Result<(), String> {
        let kept = Data { collections: self.data.collections.iter().filter(|c| c.id != SESSION).cloned().collect(), ..self.data.clone() };
        let plain = Zeroizing::new(serde_json::to_vec(&kept).map_err(|e| e.to_string())?);
        let sealed = seal(&self.key, &plain);
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let tmp = self.path.with_extension("store.tmp");
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
        f.write_all(&sealed).and_then(|()| f.sync_all()).map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &self.path).map_err(|e| format!("{}: {e}", self.path.display()))
    }

    pub fn collection(&self, id: &str) -> Option<&Collection> {
        self.data.collections.iter().find(|c| c.id == id)
    }

    pub fn collection_mut(&mut self, id: &str) -> Option<&mut Collection> {
        self.data.collections.iter_mut().find(|c| c.id == id)
    }

    /// The collection an alias names (None: no such alias, or it names
    /// one that's gone).
    pub fn alias(&self, name: &str) -> Option<&str> {
        self.data.aliases.get(name).map(String::as_str).filter(|id| self.collection(id).is_some())
    }

    /// A new collection's id, from its label (`[A-Za-z0-9_]`, unused).
    pub fn new_id(&self, label: &str) -> String {
        let base: String = label.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' }).collect();
        let base = if base.is_empty() { "collection".to_owned() } else { base };
        let mut id = base.clone();
        let mut n = 1;
        while self.collection(&id).is_some() {
            n += 1;
            id = format!("{base}{n}");
        }
        id
    }

    /// While serving: item `id` of `collection` made or changed here.
    pub fn note_changed(&mut self, collection: &str, id: u64) {
        if self.data.serving_since.is_none() || collection == SESSION {
            return;
        }
        if let Some(i) = self.collection(collection).and_then(|c| c.item(id)) {
            let k = identity(collection, &i.label, &i.attributes);
            self.data.deleted.remove(&k);
            self.data.changed.insert(k);
        }
    }

    /// While serving: an item of `collection` gone (or no longer what it
    /// was: its label or attributes changed).
    pub fn note_gone(&mut self, collection: &str, label: &str, attributes: &BTreeMap<String, String>) {
        if self.data.serving_since.is_none() || collection == SESSION {
            return;
        }
        let k = identity(collection, label, attributes);
        self.data.changed.remove(&k);
        self.data.deleted.insert(k);
    }

    /// A copy of GNOME Keyring's `item` in `collection`: replacing the one
    /// that is it (by label and attributes but `xdg:schema`), else new.
    pub fn put_copy(&mut self, collection: &str, mut item: Item) {
        let Some(c) = self.collection_mut(collection) else { return };
        let wanted = core(&item.attributes);
        if let Some(old) = c.items.iter_mut().find(|i| i.label == item.label && core(&i.attributes) == wanted) {
            item.id = old.id;
            *old = item;
            return;
        }
        item.id = c.next;
        c.next += 1;
        c.items.push(item);
    }

    /// Put `item` in `collection` (made if missing, `label` its label):
    /// replacing one with the same attributes if `replace` (CreateItem's
    /// `replace`), else as a new item. The item's id.
    ///
    /// As GNOME Keyring does: an item made without `xdg:schema` gets the
    /// generic one, and `replace` replaces an item that has every one of
    /// the new item's attributes (`gkm_secret_fields_match`), not only
    /// one with exactly them. (Matching exactly, gh's token written again
    /// over one copied from GNOME Keyring, which had GNOME's `xdg:schema`,
    /// was added beside it; go-keyring then read the old one: Ben,
    /// 2026-11-06.)
    pub fn put(&mut self, collection: &str, mut item: Item, replace: bool) -> Option<u64> {
        item.attributes.entry("xdg:schema".into()).or_insert_with(|| GENERIC.into());
        let c = self.collection_mut(collection)?;
        let t = now();
        c.modified = t;
        if replace && let Some(old) = c.items.iter_mut().find(|i| i.matches(&item.attributes)) {
            old.label = item.label;
            old.secret = item.secret;
            old.content_type = item.content_type;
            // (A client's change is stamped now by the caller; a copy from
            // GNOME Keyring keeps its own time.)
            old.modified = item.modified;
            return Some(old.id);
        }
        item.id = c.next;
        c.next += 1;
        let id = item.id;
        c.items.push(item);
        Some(id)
    }
}

impl Store {
    /// Mend what earlier versions left, each time the store is opened to
    /// serve (so an upgrade mends it): every item with GNOME's
    /// `xdg:schema`; a second login collection (GNOME Keyring's "Login",
    /// which Olo's own client made beside its `login` on a computer with no
    /// default alias, 2026-11-04) merged into `login`, aliases following;
    /// and in each collection, items that are copies of one another (the
    /// same attributes) kept once, the newest, and first, as a program
    /// taking the first found gets the newest. What it did.
    pub fn heal(&mut self) -> Healed {
        let mut h = Healed::default();
        for i in self.data.collections.iter_mut().flat_map(|c| c.items.iter_mut()) {
            if !i.attributes.contains_key("xdg:schema") {
                i.attributes.insert("xdg:schema".into(), GENERIC.into());
                h.schemas += 1;
            }
        }
        let others: Vec<String> = self.data.collections.iter().filter(|c| c.id != LOGIN && c.id.eq_ignore_ascii_case(LOGIN)).map(|c| c.id.clone()).collect();
        for id in others {
            let Some(at) = self.data.collections.iter().position(|c| c.id == id) else { continue };
            let gone = self.data.collections.remove(at);
            let mut moved = Vec::new();
            if let Some(login) = self.collection_mut(LOGIN) {
                for mut item in gone.items {
                    item.id = login.next;
                    login.next += 1;
                    moved.push(item.id);
                    login.items.push(item);
                }
                login.modified = now();
            }
            for n in moved {
                self.note_changed(LOGIN, n);
            }
            for target in self.data.aliases.values_mut() {
                if *target == id {
                    *target = LOGIN.into();
                }
            }
            h.merged.push(id);
        }
        let mut kept: Vec<(String, u64)> = Vec::new();
        for c in &mut self.data.collections {
            // Newest first, then the first of each kept.
            c.items.sort_by_key(|i| std::cmp::Reverse((i.modified, i.created, i.id)));
            let mut seen: std::collections::BTreeMap<BTreeMap<String, String>, (u64, bool)> = Default::default();
            let before = c.items.len();
            c.items.retain(|i| match seen.get_mut(&i.attributes) {
                Some((_, copies)) => {
                    *copies = true;
                    false
                }
                None => {
                    seen.insert(i.attributes.clone(), (i.id, false));
                    true
                }
            });
            h.duplicates += before - c.items.len();
            kept.extend(seen.into_values().filter(|(_, copies)| *copies).map(|(id, _)| (c.id.clone(), id)));
        }
        // (What was kept goes back to GNOME Keyring as the one to keep.)
        for (c, id) in kept {
            self.note_changed(&c, id);
        }
        h
    }
}

fn seal(key: &[u8; 32], plain: &[u8]) -> Vec<u8> {
    let mut nonce = [0u8; 24];
    getrandom::fill(&mut nonce).expect("random");
    let mut header = MAGIC.to_vec();
    header.push(FORMAT);
    let sealed = XChaCha20Poly1305::new(key.into()).encrypt(&XNonce::from(nonce), Payload { msg: plain, aad: &header }).expect("sealing");
    let mut out = header;
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&sealed);
    out
}

fn open(key: &[u8; 32], bytes: &[u8]) -> Result<Zeroizing<Vec<u8>>, String> {
    if bytes.len() < 5 + 24 || &bytes[..4] != MAGIC {
        return Err("the secrets store isn't one".into());
    }
    if bytes[4] != FORMAT {
        return Err(format!("the secrets store is of a newer format ({})", bytes[4]));
    }
    let (header, rest) = bytes.split_at(5);
    let (nonce, sealed) = rest.split_at(24);
    let nonce = XNonce::try_from(nonce).map_err(|_| "the secrets store is damaged")?;
    XChaCha20Poly1305::new(key.into())
        .decrypt(&nonce, Payload { msg: sealed, aad: header })
        .map(Zeroizing::new)
        .map_err(|_| "the secrets store couldn't be opened with its key (damaged, or another key)".into())
}

/// A new random key.
pub fn new_key() -> Zeroizing<[u8; 32]> {
    let mut k = Zeroizing::new([0u8; 32]);
    getrandom::fill(k.as_mut()).expect("random");
    k
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(attrs: &[(&str, &str)], secret: &[u8]) -> Item {
        Item {
            id: 0,
            label: "a".into(),
            attributes: attrs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            secret: Zeroizing::new(secret.to_vec()),
            content_type: "text/plain".into(),
            created: 1,
            modified: 1,
        }
    }

    #[test]
    fn kept_sealed_and_opened_again() {
        let dir = std::env::temp_dir().join(format!("olo-secrets-store-{}", std::process::id()));
        let path = dir.join("secrets.store");
        let key = new_key();
        let mut s = Store::open(&path, key.clone()).unwrap();
        assert_eq!(s.alias("default"), Some(LOGIN));
        let id = s.put(LOGIN, item(&[("application", "chrome")], b"one"), true).unwrap();
        s.put(SESSION, item(&[("x", "y")], b"gone"), true).unwrap();
        s.save().unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert!(!bytes.windows(3).any(|w| w == b"one"), "sealed");
        let again = Store::open(&path, key).unwrap();
        assert_eq!(again.collection(LOGIN).unwrap().item(id).unwrap().secret.as_slice(), b"one");
        // The session collection is kept in memory only.
        assert!(again.collection(SESSION).unwrap().items.is_empty());
        assert!(Store::open(&path, new_key()).is_err(), "another key");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn replacing_by_attributes() {
        let mut s = Store { path: PathBuf::new(), key: new_key(), data: Data::default() };
        s.data.collections.push(Collection::new(LOGIN, "Login"));
        let a = s.put(LOGIN, item(&[("k", "1")], b"a"), true).unwrap();
        let b = s.put(LOGIN, item(&[("k", "1")], b"b"), true).unwrap();
        assert_eq!(a, b);
        let c = s.put(LOGIN, item(&[("k", "1")], b"c"), false).unwrap();
        assert_ne!(a, c);
        assert_eq!(s.collection(LOGIN).unwrap().items.len(), 2);
        assert_eq!(s.collection(LOGIN).unwrap().item(a).unwrap().secret.as_slice(), b"b");
        let wanted: BTreeMap<String, String> = [("k".to_string(), "1".to_string())].into();
        assert!(s.collection(LOGIN).unwrap().items.iter().all(|i| i.matches(&wanted)));
        assert!(s.collection(LOGIN).unwrap().items.iter().all(|i| i.matches(&BTreeMap::new())));
    }

    /// gh (go-keyring) writing its token again over one copied from GNOME
    /// Keyring, which has GNOME's `xdg:schema`: replaced, as GNOME does,
    /// not added beside it; and its item gets the schema too.
    #[test]
    fn replacing_as_gnome_keyring_does() {
        let mut s = Store { path: PathBuf::new(), key: new_key(), data: Data::default() };
        s.data.collections.push(Collection::new(LOGIN, "Login"));
        let copied = s.put(LOGIN, item(&[("service", "gh:github.com"), ("username", "ben"), ("xdg:schema", GENERIC)], b"old"), true).unwrap();
        let again = s.put(LOGIN, item(&[("service", "gh:github.com"), ("username", "ben")], b"new"), true).unwrap();
        assert_eq!(copied, again);
        let c = s.collection(LOGIN).unwrap();
        assert_eq!(c.items.len(), 1);
        assert_eq!(c.item(copied).unwrap().secret.as_slice(), b"new");
        // The active account's (username "") is another item.
        let active = s.put(LOGIN, item(&[("service", "gh:github.com"), ("username", "")], b"new"), true).unwrap();
        assert_ne!(active, copied);
        assert!(s.collection(LOGIN).unwrap().items.iter().all(|i| i.attributes.get("xdg:schema").map(String::as_str) == Some(GENERIC)));
    }

    /// What Ben's store had (2026-11-06): GNOME's second "Login" copied in
    /// beside `login`, the same token item in both and twice in one (one
    /// without the schema): one collection, each item once, the newest
    /// kept and first, the default alias on `login`.
    #[test]
    fn healed() {
        let mut s = Store { path: PathBuf::new(), key: new_key(), data: Data::default() };
        s.data.serving_since = Some(1);
        s.data.collections.push(Collection::new(LOGIN, "Login"));
        s.data.collections.push(Collection::new("Login", "Login"));
        s.data.aliases.insert("default".into(), "Login".into());
        let at = |mut i: Item, t: u64| {
            i.modified = t;
            i.created = t;
            i
        };
        let gh = [("service", "gh:github.com"), ("username", "ben")];
        let gh_schema = [("service", "gh:github.com"), ("username", "ben"), ("xdg:schema", GENERIC)];
        s.put_copy(LOGIN, at(item(&gh_schema, b"older"), 20));
        s.data.collections[0].items.push(Item { id: 9, ..at(item(&gh, b"newest"), 30) });
        s.put_copy("Login", at(item(&gh_schema, b"oldest"), 10));
        s.put_copy("Login", at(item(&[("service", "gh:github.com"), ("username", ""), ("xdg:schema", GENERIC)], b"active"), 15));
        s.put_copy("Login", at(item(&[("application", "chrome"), ("xdg:schema", GENERIC)], b"chrome"), 5));
        let h = s.heal();
        assert_eq!(h.merged, ["Login"]);
        assert_eq!((h.duplicates, h.schemas), (2, 1));
        assert_eq!(s.data.collections.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), [LOGIN]);
        assert_eq!(s.alias("default"), Some(LOGIN));
        let c = s.collection(LOGIN).unwrap();
        assert_eq!(c.items.len(), 3, "{:?}", c.items.iter().map(|i| &i.attributes).collect::<Vec<_>>());
        let wanted: BTreeMap<String, String> = gh.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let found: Vec<&Item> = c.items.iter().filter(|i| i.matches(&wanted)).collect();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].secret.as_slice(), b"newest");
        // Ids stay unique; nothing more to mend the next time.
        let mut ids: Vec<u64> = c.items.iter().map(|i| i.id).collect();
        ids.dedup();
        assert_eq!(ids.len(), 3);
        assert!(s.heal().is_empty());
        assert!(!s.data.changed.is_empty(), "what was kept goes back to GNOME Keyring");
    }

    #[test]
    fn new_ids_from_labels() {
        let mut s = Store { path: PathBuf::new(), key: new_key(), data: Data::default() };
        s.data.collections.push(Collection::new(LOGIN, "Login"));
        assert_eq!(s.new_id("My Keys!"), "my_keys_");
        assert_eq!(s.new_id("Login"), "login2");
        assert_eq!(s.new_id(""), "collection");
    }
}
