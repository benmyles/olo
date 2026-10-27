//! The local store: every vault and item Passwords knows, kept on this
//! computer and encrypted at rest, so looking things up never waits on
//! 1Password. Items keep 1Password's shapes (`Item`, `ItemOverview`,
//! `VaultOverview`) plus what syncing needs: what 1Password last had of
//! each item, whether it changed here since, and the deletions still to
//! send (`sync.rs` does the talking).
//!
//! The file is `passwords.vault` in the config directory: "TABV", a
//! format byte, a 24-byte nonce, then the JSON sealed with
//! XChaCha20-Poly1305 (the header as associated data). The key is 32
//! random bytes kept in the login keyring (`tabula_system::secrets`),
//! which PAM unlocks at login, so Touch ID can open Passwords;
//! `TABULA_PASSWORDS_KEYFILE` keeps it in a file instead (tests).

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit, Payload},
};
use chrono::{DateTime, Utc};
use onepassword::{Error, Item, ItemCreateParams, ItemOverview, ItemState, Result, VaultOverview, VaultType};
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

const MAGIC: &[u8; 4] = b"TABV";
const FORMAT: u8 = 1;
const KEY_ATTRS: [(&str, &str); 2] = [("application", "tabula-passwords"), ("purpose", "vault-key")];
/// Vaults that live only here (before any account syncs) have ids
/// starting with this.
pub const LOCAL: &str = "local-";

pub fn path() -> PathBuf {
    tabula_kit::config_dir().join("passwords.vault")
}

/// What 1Password had of an item when it was last synced.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Remote {
    pub version: u32,
    pub updated_at: DateTime<Utc>,
    pub state: ItemState,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entry {
    pub item: Item,
    pub state: ItemState,
    /// None: not in 1Password (yet).
    pub remote: Option<Remote>,
    /// Changed here since the last sync.
    pub dirty: bool,
    /// Bumped by every change here, so a sync can tell what changed
    /// while it ran.
    pub rev: u64,
}

impl Entry {
    pub fn overview(&self) -> ItemOverview {
        let i = &self.item;
        ItemOverview {
            id: i.id.clone(),
            title: i.title.clone(),
            category: i.category,
            vault_id: i.vault_id.clone(),
            websites: i.websites.clone(),
            tags: i.tags.clone(),
            created_at: i.created_at,
            updated_at: i.updated_at,
            state: self.state,
        }
    }
}

/// An item deleted here, still to delete in 1Password.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Tombstone {
    pub vault: String,
    pub id: String,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Data {
    pub vaults: Vec<VaultOverview>,
    pub entries: Vec<Entry>,
    pub tombstones: Vec<Tombstone>,
    /// Each 1Password vault's content version when last listed: an
    /// unchanged one needn't be listed again.
    pub synced: HashMap<String, u32>,
    pub last_sync: Option<DateTime<Utc>>,
    /// The account this store syncs with; another account starts afresh.
    pub account: Option<String>,
    next_rev: u64,
}

/// What a sync learned, applied with [`Vault::apply`].
#[derive(Debug)]
pub enum Change {
    /// 1Password's vaults, all of them.
    Vaults(Vec<VaultOverview>),
    /// A vault was listed at this content version.
    Listed { vault: String, content_version: u32 },
    /// An item new or changed in 1Password.
    Fetched { item: Box<Item>, state: ItemState },
    /// An item gone from 1Password.
    Gone { id: String },
    /// Ours reached 1Password: `item` is 1Password's copy (its id may be
    /// new), sent at local revision `rev`.
    Pushed { local_id: String, rev: u64, item: Box<Item>, state: ItemState },
    /// A deletion reached 1Password.
    Buried(Tombstone),
}

/// What a sync starts from: a copy of the store's sync state.
pub struct Snapshot {
    pub account: String,
    pub synced: HashMap<String, u32>,
    pub entries: Vec<Entry>,
    pub tombstones: Vec<Tombstone>,
}

pub struct Vault {
    key: Zeroizing<[u8; 32]>,
    file: PathBuf,
    pub data: Data,
}

fn fail(message: impl Into<String>) -> Error {
    Error::Core(message.into())
}

fn random<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    getrandom::fill(&mut b).expect("random bytes");
    b
}

/// A new item id, shaped like 1Password's (26 of a–z, 0–9).
pub fn new_id() -> String {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz234567";
    random::<26>().iter().map(|b| ALPHABET[(*b & 31) as usize] as char).collect()
}

/// The vault key: from the key file or the login keyring, made (and
/// kept) if there is none and no store yet.
fn key(store_exists: bool, keyfile: Option<&Path>) -> Result<Zeroizing<[u8; 32]>> {
    let missing = || fail("The key to your passwords is missing from the login keyring, so they can’t be opened.");
    let mut bytes = match keyfile {
        Some(file) => match std::fs::read(file) {
            Ok(b) => Zeroizing::new(b),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if store_exists {
                    return Err(missing());
                }
                let k = Zeroizing::new(random::<32>().to_vec());
                write_private(file, &k).map_err(|e| fail(format!("Couldn’t keep the key: {e}")))?;
                k
            }
            Err(e) => return Err(fail(format!("Couldn’t read the key: {e}"))),
        },
        None => match tabula_system::secrets::lookup(&KEY_ATTRS) {
            Ok(Some(b)) => Zeroizing::new(b),
            Ok(None) if store_exists => return Err(missing()),
            Ok(None) => {
                let k = Zeroizing::new(random::<32>().to_vec());
                tabula_system::secrets::store("Tabula Passwords", &KEY_ATTRS, &k)
                    .map_err(|e| fail(format!("Couldn’t keep the key in the login keyring: {e}")))?;
                k
            }
            Err(e) => return Err(fail(format!("Couldn’t reach the login keyring: {e}"))),
        },
    };
    let key: [u8; 32] = bytes.as_slice().try_into().map_err(|_| fail("The key to your passwords is damaged."))?;
    bytes.zeroize();
    Ok(Zeroizing::new(key))
}

/// Write `bytes` to `path` readable only by us, replacing it whole.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
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
    let sealed = XChaCha20Poly1305::new(key.into())
        .encrypt(&XNonce::from(nonce), Payload { msg: plain, aad: &header })
        .expect("sealing");
    let mut out = header;
    out.extend_from_slice(&nonce);
    out.extend(sealed);
    out
}

fn open(key: &[u8; 32], bytes: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    if bytes.len() < 5 + 24 || &bytes[..4] != MAGIC {
        return Err(fail("The passwords file isn’t one Passwords made."));
    }
    if bytes[4] != FORMAT {
        return Err(fail("The passwords file is from a newer Passwords."));
    }
    let (header, rest) = bytes.split_at(5);
    let (nonce, sealed) = rest.split_at(24);
    XChaCha20Poly1305::new(key.into())
        .decrypt(&XNonce::try_from(nonce).map_err(|_| fail("The passwords file is damaged."))?, Payload { msg: sealed, aad: header })
        .map(Zeroizing::new)
        .map_err(|_| fail("The passwords file can’t be opened with the key in the login keyring."))
}

/// A vault for items that live only here.
fn local_vault() -> VaultOverview {
    let now = Utc::now();
    VaultOverview {
        id: format!("{LOCAL}personal"),
        title: "Personal".into(),
        description: String::new(),
        vault_type: VaultType::Personal,
        active_item_count: 0,
        content_version: 0,
        attribute_version: 0,
        created_at: now,
        updated_at: now,
    }
}

impl Vault {
    /// Open the store (making it the first time). Blocks on the keyring.
    pub fn open() -> Result<Vault> {
        let keyfile = std::env::var_os("TABULA_PASSWORDS_KEYFILE").map(PathBuf::from);
        Self::open_at(path(), keyfile.as_deref())
    }

    /// The store in `file`, its key in `keyfile` or else the keyring.
    pub fn open_at(file: PathBuf, keyfile: Option<&Path>) -> Result<Vault> {
        let existing = std::fs::read(&file).ok();
        let key = key(existing.is_some(), keyfile)?;
        let data = match existing {
            Some(bytes) => {
                let plain = open(&key, &bytes)?;
                serde_json::from_slice(&plain).map_err(|e| fail(format!("The passwords file is damaged: {e}")))?
            }
            None => Data { vaults: vec![local_vault()], ..Default::default() },
        };
        let v = Vault { key, file, data };
        if !v.file.exists() {
            v.save()?;
        }
        Ok(v)
    }

    pub fn save(&self) -> Result<()> {
        let plain = Zeroizing::new(serde_json::to_vec(&self.data).map_err(|e| fail(e.to_string()))?);
        write_private(&self.file, &seal(&self.key, &plain)).map_err(|e| fail(format!("Couldn’t save your passwords: {e}")))
    }

    fn rev(&mut self) -> u64 {
        self.data.next_rev += 1;
        self.data.next_rev
    }

    fn entry(&self, id: &str) -> Option<&Entry> {
        self.data.entries.iter().find(|e| e.item.id == id)
    }

    fn entry_mut(&mut self, id: &str) -> Option<&mut Entry> {
        self.data.entries.iter_mut().find(|e| e.item.id == id)
    }

    pub fn vaults(&self) -> Vec<VaultOverview> {
        let mut vaults = self.data.vaults.clone();
        for v in &mut vaults {
            v.active_item_count =
                self.data.entries.iter().filter(|e| e.item.vault_id == v.id && e.state == ItemState::Active).count() as u32;
        }
        vaults
    }

    pub fn items(&self, vault: &str) -> Vec<ItemOverview> {
        self.data.entries.iter().filter(|e| e.item.vault_id == vault).map(Entry::overview).collect()
    }

    pub fn get(&self, _vault: &str, id: &str) -> Result<Item> {
        self.entry(id).map(|e| e.item.clone()).ok_or_else(|| fail("The item isn’t there any more."))
    }

    pub fn create(&mut self, params: &ItemCreateParams) -> Result<Item> {
        if !self.data.vaults.iter().any(|v| v.id == params.vault_id) {
            return Err(fail("That vault isn’t there any more."));
        }
        let now = Utc::now();
        let item = Item {
            id: new_id(),
            title: params.title.clone(),
            category: params.category,
            vault_id: params.vault_id.clone(),
            fields: params.fields.clone(),
            sections: params.sections.clone(),
            notes: params.notes.clone().unwrap_or_default(),
            tags: params.tags.clone(),
            websites: params.websites.clone(),
            version: 0,
            files: Vec::new(),
            document: None,
            created_at: now,
            updated_at: now,
        };
        let rev = self.rev();
        self.data.entries.push(Entry { item: item.clone(), state: ItemState::Active, remote: None, dirty: true, rev });
        self.save()?;
        Ok(item)
    }

    /// Change an item (moving it if its vault changed).
    pub fn put(&mut self, item: &Item) -> Result<Item> {
        let rev = self.rev();
        let entry = self.entry_mut(&item.id).ok_or_else(|| fail("The item isn’t there any more."))?;
        let mut moved = None;
        if entry.item.vault_id != item.vault_id
            && let Some(_) = entry.remote.take()
        {
            // 1Password can't move items: it becomes a new one there,
            // and the old one goes.
            moved = Some(Tombstone { vault: entry.item.vault_id.clone(), id: entry.item.id.clone() });
        }
        let mut new = item.clone();
        new.version = entry.item.version;
        new.created_at = entry.item.created_at;
        new.updated_at = Utc::now();
        entry.item = new.clone();
        entry.dirty = true;
        entry.rev = rev;
        if let Some(t) = moved {
            // Give it a fresh local id so the tombstone's id stays 1Password's.
            let entry = self.entry_mut(&item.id).expect("entry");
            entry.item.id = new_id();
            new.id = entry.item.id.clone();
            self.data.tombstones.push(t);
        }
        self.save()?;
        Ok(new)
    }

    pub fn archive(&mut self, _vault: &str, id: &str) -> Result<()> {
        let rev = self.rev();
        let entry = self.entry_mut(id).ok_or_else(|| fail("The item isn’t there any more."))?;
        entry.state = ItemState::Archived;
        entry.dirty = true;
        entry.rev = rev;
        self.save()
    }

    pub fn delete(&mut self, _vault: &str, id: &str) -> Result<()> {
        let at = self.data.entries.iter().position(|e| e.item.id == id).ok_or_else(|| fail("The item isn’t there any more."))?;
        let entry = self.data.entries.remove(at);
        if entry.remote.is_some() {
            self.data.tombstones.push(Tombstone { vault: entry.item.vault_id, id: entry.item.id });
        }
        self.save()
    }

    /// Changes not yet in 1Password.
    pub fn pending(&self) -> usize {
        self.data.entries.iter().filter(|e| e.dirty).count() + self.data.tombstones.len()
    }

    /// Start a sync with `account`. A different account than last time
    /// starts afresh: what came from the old one goes, what was made here
    /// stays and goes to the new one.
    pub fn snapshot(&mut self, account: &str) -> Snapshot {
        if self.data.account.as_deref() != Some(account) {
            if self.data.account.is_some() {
                self.data.entries.retain(|e| e.remote.is_none());
                self.data.tombstones.clear();
                self.data.vaults.retain(|v| v.id.starts_with(LOCAL));
                if self.data.vaults.is_empty() {
                    self.data.vaults.push(local_vault());
                }
                for e in &mut self.data.entries {
                    e.item.vault_id = local_vault().id;
                }
            }
            self.data.synced.clear();
            self.data.account = Some(account.to_string());
        }
        Snapshot {
            account: account.to_string(),
            synced: self.data.synced.clone(),
            entries: self.data.entries.clone(),
            tombstones: self.data.tombstones.clone(),
        }
    }

    /// Apply what a sync learned. Returns the ids that changed
    /// (old, new) when ours reached 1Password under new ones.
    pub fn apply(&mut self, changes: Vec<Change>) -> Result<Vec<(String, String)>> {
        let mut renamed = Vec::new();
        for change in changes {
            match change {
                Change::Vaults(remote) => self.apply_vaults(remote),
                Change::Listed { vault, content_version } => {
                    self.data.synced.insert(vault, content_version);
                }
                Change::Fetched { item, state } => self.apply_fetched(*item, state),
                Change::Gone { id } => {
                    if let Some(at) = self.data.entries.iter().position(|e| e.item.id == id && e.remote.is_some()) {
                        if self.data.entries[at].dirty {
                            // Changed here: keep it, as a new item.
                            self.data.entries[at].remote = None;
                        } else {
                            self.data.entries.remove(at);
                        }
                    }
                }
                Change::Pushed { local_id, rev, item, state } => {
                    let remote = Remote { version: item.version, updated_at: item.updated_at, state };
                    if local_id != item.id {
                        renamed.push((local_id.clone(), item.id.clone()));
                    }
                    match self.entry_mut(&local_id) {
                        Some(e) if e.rev == rev => {
                            e.item = *item;
                            e.state = state;
                            e.remote = Some(remote);
                            e.dirty = false;
                        }
                        Some(e) => {
                            // Changed again meanwhile: still to send, now
                            // against 1Password's copy.
                            e.item.id = item.id.clone();
                            e.item.vault_id = item.vault_id.clone();
                            e.item.version = item.version;
                            e.remote = Some(remote);
                        }
                        // Deleted meanwhile.
                        None => self.data.tombstones.push(Tombstone { vault: item.vault_id.clone(), id: item.id.clone() }),
                    }
                }
                Change::Buried(t) => self.data.tombstones.retain(|x| *x != t),
            }
        }
        self.data.last_sync = Some(Utc::now());
        self.save()?;
        Ok(renamed)
    }

    /// Where items with no vault in 1Password go: Personal (or Private,
    /// or Employee), else the first.
    pub fn default_vault(vaults: &[VaultOverview]) -> Option<&VaultOverview> {
        vaults
            .iter()
            .find(|v| v.vault_type == VaultType::Personal)
            .or_else(|| vaults.iter().find(|v| matches!(v.title.as_str(), "Personal" | "Private" | "Employee")))
            .or(vaults.first())
    }

    fn apply_vaults(&mut self, remote: Vec<VaultOverview>) {
        let Some(home) = Self::default_vault(&remote).map(|v| v.id.clone()) else { return };
        let known: Vec<&str> = remote.iter().map(|v| v.id.as_str()).collect();
        // Items in vaults 1Password doesn't have (made here, or a vault
        // gone there): synced ones go with their vault; changed or new
        // ones move to the default vault, to be sent.
        self.data.entries.retain(|e| known.contains(&e.item.vault_id.as_str()) || e.remote.is_none() || e.dirty);
        for e in &mut self.data.entries {
            if !known.contains(&e.item.vault_id.as_str()) {
                e.item.vault_id = home.clone();
                e.remote = None;
                e.dirty = true;
            }
        }
        self.data.synced.retain(|id, _| known.contains(&id.as_str()));
        self.data.vaults = remote;
    }

    fn apply_fetched(&mut self, item: Item, state: ItemState) {
        let remote = Remote { version: item.version, updated_at: item.updated_at, state };
        let id = item.id.clone();
        let Some(at) = self.data.entries.iter().position(|e| e.item.id == id) else {
            let rev = self.rev();
            self.data.entries.push(Entry { item, state, remote: Some(remote), dirty: false, rev });
            return;
        };
        let entry = &self.data.entries[at];
        if entry.dirty && entry.remote.as_ref().is_some_and(|r| r.version != remote.version) {
            // Changed on both sides: 1Password's stays as the item, ours
            // is kept beside it as a copy, to be sent as a new item.
            let mut copy = entry.clone();
            copy.item.id = new_id();
            copy.item.title = format!("{} (conflict)", copy.item.title);
            copy.remote = None;
            copy.rev = self.rev();
            self.data.entries.push(copy);
        } else if entry.dirty {
            // Only ours changed; it still goes up.
            return;
        }
        let rev = self.rev();
        self.data.entries[at] = Entry { item, state, remote: Some(remote), dirty: false, rev };
    }
}

impl Drop for Vault {
    fn drop(&mut self) {
        // Best effort: the secrets' strings go when the entries do.
        for e in &mut self.data.entries {
            for f in &mut e.item.fields {
                f.value.zeroize();
            }
            e.item.notes.zeroize();
        }
    }
}

#[cfg(test)]
mod tests {
    use onepassword::{ItemCategory, ItemField, ItemFieldType};

    use super::*;

    /// A fresh store in a directory of its own, its key in a file there.
    fn fresh(name: &str) -> (Vault, PathBuf) {
        let dir = std::env::temp_dir().join(format!("tabula-vault-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        (Vault::open_at(dir.join("passwords.vault"), Some(&dir.join("key"))).unwrap(), dir)
    }

    fn login(v: &mut Vault, title: &str) -> Item {
        let vault = v.data.vaults[0].id.clone();
        let mut p = ItemCreateParams::new(ItemCategory::Login, vault, title);
        p.fields = vec![ItemField::new("password", "password", ItemFieldType::Concealed, "hunter2")];
        v.create(&p).unwrap()
    }

    /// An item as 1Password would send it.
    fn theirs(id: &str, version: u32) -> Item {
        let now = Utc::now();
        Item {
            id: id.into(),
            title: id.to_uppercase(),
            category: ItemCategory::Login,
            vault_id: "v1".into(),
            fields: vec![ItemField::new("password", "password", ItemFieldType::Concealed, "remote")],
            sections: Vec::new(),
            notes: String::new(),
            tags: Vec::new(),
            websites: Vec::new(),
            version,
            files: Vec::new(),
            document: None,
            created_at: now,
            updated_at: now,
        }
    }

    fn remote_vault(id: &str) -> VaultOverview {
        VaultOverview { id: id.into(), title: "Private".into(), ..local_vault() }
    }

    #[test]
    fn sealed_and_reopened() {
        let (mut v, dir) = fresh("seal");
        let item = login(&mut v, "Example");
        drop(v);
        let file = dir.join("passwords.vault");
        let bytes = std::fs::read(&file).unwrap();
        assert!(!bytes.windows(7).any(|w| w == b"hunter2"), "secrets are sealed");
        let key = dir.join("key");
        let v = Vault::open_at(file.clone(), Some(&key)).unwrap();
        assert_eq!(v.get("", &item.id).unwrap().password(), Some("hunter2"));
        drop(v);
        // A different key can't open it, and none is made over it.
        std::fs::write(&key, [7u8; 32]).unwrap();
        assert!(Vault::open_at(file.clone(), Some(&key)).is_err());
        std::fs::remove_file(&key).unwrap();
        assert!(Vault::open_at(file, Some(&key)).is_err(), "no new key for an existing store");
        assert!(!key.exists());
    }

    #[test]
    fn local_items_move_to_1password_and_conflicts_are_kept() {
        let (mut v, _) = fresh("sync");
        let mine = login(&mut v, "Mine");
        let snap = v.snapshot("acct");
        assert_eq!(snap.entries.len(), 1);
        // 1Password has a vault: the local one's item moves there.
        v.apply(vec![Change::Vaults(vec![remote_vault("v1")])]).unwrap();
        assert_eq!(v.get("", &mine.id).unwrap().vault_id, "v1");
        assert!(v.data.entries[0].dirty && v.data.entries[0].remote.is_none());
        // Sent, and 1Password gave it its own id.
        let rev = v.data.entries[0].rev;
        let mut sent = v.get("", &mine.id).unwrap();
        sent.id = "remoteid".into();
        sent.version = 1;
        let renamed =
            v.apply(vec![Change::Pushed { local_id: mine.id.clone(), rev, item: Box::new(sent.clone()), state: ItemState::Active }]).unwrap();
        assert_eq!(renamed, vec![(mine.id.clone(), "remoteid".to_string())]);
        assert_eq!(v.pending(), 0);
        // Changed here and there: theirs wins in place, ours is a copy.
        let mut here = v.get("", "remoteid").unwrap();
        here.field_mut("password").unwrap().value = "local".into();
        v.put(&here).unwrap();
        let mut there = sent.clone();
        there.version = 2;
        there.field_mut("password").unwrap().value = "remote".into();
        v.apply(vec![Change::Fetched { item: Box::new(there), state: ItemState::Active }]).unwrap();
        assert_eq!(v.get("", "remoteid").unwrap().password(), Some("remote"));
        let copy = v.data.entries.iter().find(|e| e.item.title == "Mine (conflict)").unwrap();
        assert_eq!(copy.item.password(), Some("local"));
        assert!(copy.dirty && copy.remote.is_none());
        // Deleting a synced item leaves a tombstone to send.
        v.delete("v1", "remoteid").unwrap();
        assert_eq!(v.data.tombstones, vec![Tombstone { vault: "v1".into(), id: "remoteid".into() }]);
    }

    #[test]
    fn a_push_racing_an_edit_keeps_the_edit() {
        let (mut v, _) = fresh("race");
        v.snapshot("acct");
        v.apply(vec![Change::Vaults(vec![remote_vault("v1")])]).unwrap();
        let mine = login(&mut v, "Mine");
        let rev = v.data.entries[0].rev;
        let mut edited = mine.clone();
        edited.title = "Edited".into();
        v.put(&edited).unwrap();
        let mut sent = mine.clone();
        sent.id = "rid".into();
        sent.version = 1;
        v.apply(vec![Change::Pushed { local_id: mine.id.clone(), rev, item: Box::new(sent), state: ItemState::Active }]).unwrap();
        let e = &v.data.entries[0];
        assert_eq!((e.item.id.as_str(), e.item.title.as_str(), e.dirty), ("rid", "Edited", true));
        assert_eq!(e.remote.as_ref().map(|r| r.version), Some(1));
    }

    #[test]
    fn gone_there_goes_here_unless_changed() {
        let (mut v, _) = fresh("gone");
        v.snapshot("acct");
        v.apply(vec![
            Change::Vaults(vec![remote_vault("v1")]),
            Change::Fetched { item: Box::new(theirs("a", 3)), state: ItemState::Active },
            Change::Fetched { item: Box::new(theirs("b", 3)), state: ItemState::Active },
        ])
        .unwrap();
        let mut b = v.get("", "b").unwrap();
        b.title = "B changed".into();
        v.put(&b).unwrap();
        v.apply(vec![Change::Gone { id: "a".into() }, Change::Gone { id: "b".into() }]).unwrap();
        assert!(v.get("", "a").is_err());
        let kept = v.data.entries.iter().find(|e| e.item.id == "b").unwrap();
        assert!(kept.remote.is_none() && kept.dirty, "kept, to be sent again");
    }

    #[test]
    fn another_account_starts_afresh_but_keeps_local_items() {
        let (mut v, _) = fresh("account");
        v.snapshot("one");
        v.apply(vec![Change::Vaults(vec![remote_vault("v1")]), Change::Fetched { item: Box::new(theirs("a", 1)), state: ItemState::Active }])
            .unwrap();
        let mine = login(&mut v, "Mine");
        let snap = v.snapshot("two");
        assert_eq!(snap.entries.len(), 1);
        assert_eq!(snap.entries[0].item.id, mine.id);
        assert!(v.data.synced.is_empty());
        assert!(v.get("", "a").is_err());
    }
}
