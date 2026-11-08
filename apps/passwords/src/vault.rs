//! The local store: every vault and item Passwords knows, kept on this
//! computer and encrypted at rest, so looking things up never waits on
//! 1Password. Items keep 1Password's shapes (`Item`, `ItemOverview`,
//! `VaultOverview`) plus what syncing needs: what 1Password last had of
//! each item, whether it changed here since, and the deletions still to
//! send (`sync.rs` does the talking). Field values are kept in the forms
//! 1Password's SDK has them (`onepassword::fields`: a card's expiry is
//! 08/2029), whoever made or changed the item.
//!
//! The file is `passwords.vault` in the config directory: "TABV", a
//! format byte, a 24-byte nonce, then the JSON sealed with
//! XChaCha20-Poly1305 (the header as associated data). The key is 32
//! random bytes kept in the login keyring (`olo_system::secrets`),
//! which PAM unlocks at login, so Touch ID can open Passwords;
//! `OLO_PASSWORDS_KEYFILE` keeps it in a file instead (tests).
//!
//! What's shared with agent screens (`Shares`, chosen in Agent Screens)
//! is kept in the store too, sealed with it, so only the person's
//! Passwords changes it. Each time the store changes, its items are
//! written out again as stores of their own (`write_copies`), one for
//! every screen and one for each screen given more or less, each sealed
//! with a key kept beside it (`olo_keychain::shared`): a screen's
//! Passwords opens its copy with that key alone ([`Vault::open_shared`]),
//! whether or not the person's is open or unlocked (the person chose
//! that, 2026-11-03, over sharing only while theirs is unlocked).

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
};

use olo_keychain::{ScreenShares, shared};

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
// (Kept with every other secret of Olo's, which Backup takes along.)
const KEY_ATTRS: &[(&str, &str)] = olo_system::secrets::PASSWORDS_KEY.attributes;
/// Vaults that live only here (before any account syncs) have ids
/// starting with this.
pub const LOCAL: &str = "local-";

pub fn path() -> PathBuf {
    olo_ui::config_dir().join("passwords.vault")
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
    /// Which 1Password account each synced vault is in (vaults made here,
    /// [`LOCAL`], are in none: their items go to the first account).
    pub vault_account: HashMap<String, String>,
    /// Before several accounts: the one account (read once, then moved
    /// into `vault_account`).
    account: Option<String>,
    next_rev: u64,
    /// What's shared with agent screens.
    pub shares: Shares,
}

/// What's shared with agent screens: items every screen has, and what
/// each screen has besides or without them (item ids).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Shares {
    pub everywhere: Vec<String>,
    pub screens: BTreeMap<String, ScreenShares>,
}

impl Shares {
    /// The items screen `screen` has (None: what every screen has).
    pub fn of(&self, screen: Option<&str>) -> HashSet<String> {
        let mut ids: HashSet<String> = self.everywhere.iter().cloned().collect();
        if let Some(s) = screen.and_then(|n| self.screens.get(n)) {
            ids.extend(s.add.iter().cloned());
            for id in &s.remove {
                ids.remove(id);
            }
        }
        ids
    }

    /// The copies there are to write: every screen's, and one for each
    /// screen given more or less.
    pub fn copies(&self) -> Vec<Option<String>> {
        std::iter::once(None).chain(self.screens.iter().filter(|(_, s)| !s.is_empty()).map(|(n, _)| Some(n.clone()))).collect()
    }
}

/// What a sync learned, applied with [`Vault::apply`].
#[derive(Debug)]
pub enum Change {
    /// All of an account's vaults in 1Password. `home`: the first
    /// account, where items made here go.
    Vaults { account: String, vaults: Vec<VaultOverview>, home: bool },
    /// A vault was listed at this content version.
    Listed { vault: String, content_version: u32 },
    /// An item new or changed in 1Password.
    Fetched { item: Box<Item>, state: ItemState },
    /// An item archived in 1Password (which won't let it be read): ours,
    /// unchanged here, is archived too.
    Archived { id: String, updated_at: DateTime<Utc> },
    /// An item gone from 1Password.
    Gone { id: String },
    /// Ours reached 1Password: `item` is 1Password's copy (its id may be
    /// new), sent at local revision `rev`.
    Pushed { local_id: String, rev: u64, item: Box<Item>, state: ItemState },
    /// A deletion reached 1Password.
    Buried(Tombstone),
}

/// What a sync with one account starts from: a copy of the store's sync
/// state for that account's vaults (and, for the first account, the
/// items made here).
pub struct Snapshot {
    pub account: String,
    /// How the person knows the account, when there are several (it goes
    /// after vault names); else "".
    pub label: String,
    /// The first account: items made here go to it.
    pub home: bool,
    pub synced: HashMap<String, u32>,
    pub entries: Vec<Entry>,
    pub tombstones: Vec<Tombstone>,
    /// Every vault known here, for names.
    pub vaults: Vec<VaultOverview>,
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
        None => match olo_system::secrets::lookup(KEY_ATTRS) {
            Ok(Some(b)) => Zeroizing::new(b),
            Ok(None) if store_exists => return Err(missing()),
            Ok(None) => {
                let k = Zeroizing::new(random::<32>().to_vec());
                olo_system::secrets::store(olo_system::secrets::PASSWORDS_KEY.label, KEY_ATTRS, &k).map_err(|e| fail(format!("Couldn’t keep the key in the login keyring: {e}")))?;
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
    let sealed = XChaCha20Poly1305::new(key.into()).encrypt(&XNonce::from(nonce), Payload { msg: plain, aad: &header }).expect("sealing");
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
    /// On an agent screen, what's shared with it instead.
    pub fn open() -> Result<Vault> {
        if let Some(screen) = shared::this_screen() {
            return Self::open_shared(&screen);
        }
        let keyfile = std::env::var_os("OLO_PASSWORDS_KEYFILE").map(PathBuf::from);
        Self::open_at(path(), keyfile.as_deref())
    }

    /// What's shared with agent screen `screen`: its own copy if it was
    /// given more or less than every screen, else every screen's; none
    /// yet, an empty store (made nowhere: it's never saved).
    pub fn open_shared(screen: &str) -> Result<Vault> {
        Self::open_shared_in(&shared::screens_dir(), screen)
    }

    fn open_shared_in(dir: &Path, screen: &str) -> Result<Vault> {
        let own = shared::copy_path_in(dir, Some(screen));
        let file = if own.exists() { own } else { shared::copy_path_in(dir, None) };
        if !file.exists() {
            return Ok(Vault { key: Zeroizing::new(random::<32>()), file: PathBuf::new(), data: Data::default() });
        }
        let keyfile = shared::key_path(&file);
        Self::open_at(file, Some(&keyfile))
    }

    /// Whether this is what's shared with an agent screen: read only.
    pub fn is_shared(&self) -> bool {
        shared::this_screen().is_some()
    }

    /// The shares' copies written again (`Shares::copies`), and copies of
    /// screens that no longer have one of their own taken away. The
    /// bytes each was made from are kept in `written`, so an unchanged
    /// one isn't written again.
    pub fn write_copies(&self, written: &mut HashMap<PathBuf, u64>) {
        self.write_copies_in(&shared::screens_dir(), written);
    }

    fn write_copies_in(&self, dir: &Path, written: &mut HashMap<PathBuf, u64>) {
        let copies = self.data.shares.copies();
        for screen in &copies {
            let ids = self.data.shares.of(screen.as_deref());
            let file = shared::copy_path_in(dir, screen.as_deref());
            if let Err(e) = self.write_copy(&ids, &file, &shared::key_path(&file), written) {
                tracing::warn!("couldn’t share passwords with {}: {e}", screen.as_deref().unwrap_or("every screen"));
            }
        }
        // A screen now as every screen is: its own copy goes.
        if let Ok(dirs) = std::fs::read_dir(dir) {
            for d in dirs.flatten().filter(|d| d.path().is_dir()) {
                let name = d.file_name().to_string_lossy().into_owned();
                if !copies.iter().any(|c| c.as_deref() == Some(name.as_str())) {
                    let file = shared::copy_path_in(dir, Some(&name));
                    if file.exists() {
                        let _ = std::fs::remove_file(&file);
                        written.remove(&file);
                    }
                }
            }
        }
    }

    /// Items `ids` (those not archived) as a store of their own in `file`,
    /// with their vaults' names and nothing of syncing, sealed with the
    /// key in `keyfile` (made the first time).
    fn write_copy(&self, ids: &HashSet<String>, file: &Path, keyfile: &Path, written: &mut HashMap<PathBuf, u64>) -> Result<()> {
        let entries: Vec<Entry> = self
            .data
            .entries
            .iter()
            .filter(|e| ids.contains(&e.item.id) && e.state == ItemState::Active)
            .map(|e| Entry { item: e.item.clone(), state: e.state, remote: None, dirty: false, rev: 0 })
            .collect();
        let vaults: Vec<VaultOverview> = self.data.vaults.iter().filter(|v| entries.iter().any(|e| e.item.vault_id == v.id)).cloned().collect();
        let copy = Data { vaults, entries, ..Default::default() };
        let plain = Zeroizing::new(serde_json::to_vec(&copy).map_err(|e| fail(e.to_string()))?);
        let digest = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            plain.hash(&mut h);
            h.finish()
        };
        if written.get(file) == Some(&digest) && file.exists() {
            return Ok(());
        }
        let key = key(file.exists(), Some(keyfile))?;
        write_private(file, &seal(&key, &plain)).map_err(|e| fail(format!("Couldn’t write the shared copy: {e}")))?;
        written.insert(file.to_owned(), digest);
        Ok(())
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
        let mut data = data;
        // From when there was one account: its vaults are its.
        if let Some(account) = data.account.take() {
            for v in data.vaults.iter().filter(|v| !v.id.starts_with(LOCAL)) {
                data.vault_account.entry(v.id.clone()).or_insert_with(|| account.clone());
            }
        }
        let v = Vault { key, file, data };
        if !v.file.exists() {
            v.save()?;
        }
        Ok(v)
    }

    pub fn save(&self) -> Result<()> {
        if self.is_shared() {
            return Err(fail("These are shared from your Passwords: change them there."));
        }
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
            v.active_item_count = self.data.entries.iter().filter(|e| e.item.vault_id == v.id && e.state == ItemState::Active).count() as u32;
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
        let mut fields = params.fields.clone();
        onepassword::fields::normalize(&mut fields).map_err(fail)?;
        let now = Utc::now();
        let item = Item {
            id: new_id(),
            title: params.title.clone(),
            category: params.category,
            vault_id: params.vault_id.clone(),
            fields,
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
        let mut item = item.clone();
        onepassword::fields::normalize(&mut item.fields).map_err(fail)?;
        let item = &item;
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

    /// Delete an item; it as it was (for Undo, which makes it again).
    pub fn delete(&mut self, _vault: &str, id: &str) -> Result<Item> {
        let at = self.data.entries.iter().position(|e| e.item.id == id).ok_or_else(|| fail("The item isn’t there any more."))?;
        let entry = self.data.entries.remove(at);
        if entry.remote.is_some() {
            self.data.tombstones.push(Tombstone { vault: entry.item.vault_id.clone(), id: entry.item.id.clone() });
        }
        self.save()?;
        Ok(entry.item)
    }

    /// Changes not yet in 1Password.
    pub fn pending(&self) -> usize {
        self.data.entries.iter().filter(|e| e.dirty).count() + self.data.tombstones.len()
    }

    /// Whether `vault` is synced with `account` (`home`: the first
    /// account, which also takes the vaults made here).
    fn ours(&self, vault: &str, account: &str, home: bool) -> bool {
        self.data.vault_account.get(vault).map_or(home, |a| a == account)
    }

    /// What a sync with `account` starts from. Changes nothing.
    pub fn snapshot(&self, account: &str, home: bool) -> Snapshot {
        let ours = |vault: &str| self.ours(vault, account, home);
        Snapshot {
            account: account.to_string(),
            label: String::new(),
            home,
            synced: self.data.synced.iter().filter(|(v, _)| ours(v)).map(|(v, n)| (v.clone(), *n)).collect(),
            entries: self.data.entries.iter().filter(|e| ours(&e.item.vault_id)).cloned().collect(),
            tombstones: self.data.tombstones.iter().filter(|t| ours(&t.vault)).cloned().collect(),
            vaults: self.data.vaults.clone(),
        }
    }

    /// Stop syncing with `account`: its vaults and their items go from
    /// here (items made here and never sent stay, in the local vault, to
    /// go to the first account). Nothing changes in 1Password.
    pub fn remove_account(&mut self, account: &str) -> Result<()> {
        let gone: HashSet<String> = self.data.vault_account.iter().filter(|(_, a)| *a == account).map(|(v, _)| v.clone()).collect();
        self.data.entries.retain(|e| !gone.contains(&e.item.vault_id) || e.remote.is_none());
        let local = local_vault();
        let mut kept = false;
        for e in self.data.entries.iter_mut().filter(|e| gone.contains(&e.item.vault_id)) {
            e.item.vault_id = local.id.clone();
            e.dirty = true;
            kept = true;
        }
        self.data.tombstones.retain(|t| !gone.contains(&t.vault));
        self.data.synced.retain(|v, _| !gone.contains(v));
        self.data.vaults.retain(|v| !gone.contains(&v.id));
        self.data.vault_account.retain(|_, a| a != account);
        if (kept || self.data.vaults.is_empty()) && !self.data.vaults.iter().any(|v| v.id == local.id) {
            self.data.vaults.push(local);
        }
        self.save()
    }

    /// Erase everything kept here (items, pending changes, which account
    /// was synced) for a fresh start. Nothing is sent to 1Password.
    pub fn reset(&mut self) -> Result<()> {
        self.data = Data { vaults: vec![local_vault()], ..Default::default() };
        self.save()
    }

    /// Apply what a sync learned. Returns the ids that changed
    /// (old, new) when ours reached 1Password under new ones.
    pub fn apply(&mut self, changes: Vec<Change>) -> Result<Vec<(String, String)>> {
        let mut renamed = Vec::new();
        for change in changes {
            match change {
                Change::Vaults { account, vaults, home } => self.apply_vaults(account, vaults, home),
                Change::Listed { vault, content_version } => {
                    self.data.synced.insert(vault, content_version);
                }
                Change::Fetched { item, state } => self.apply_fetched(*item, state),
                Change::Archived { id, updated_at } => {
                    let rev = self.rev();
                    if let Some(e) = self.data.entries.iter_mut().find(|e| e.item.id == id && !e.dirty)
                        && let Some(remote) = e.remote.as_mut()
                    {
                        remote.updated_at = updated_at;
                        remote.state = ItemState::Archived;
                        e.state = ItemState::Archived;
                        e.rev = rev;
                    }
                }
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
        vaults.iter().find(|v| v.vault_type == VaultType::Personal).or_else(|| vaults.iter().find(|v| matches!(v.title.as_str(), "Personal" | "Private" | "Employee"))).or(vaults.first())
    }

    fn apply_vaults(&mut self, account: String, remote: Vec<VaultOverview>, home: bool) {
        let Some(home_vault) = Self::default_vault(&remote).map(|v| v.id.clone()) else { return };
        let known: HashSet<&str> = remote.iter().map(|v| v.id.as_str()).collect();
        let map = self.data.vault_account.clone();
        let ours = |vault: &str| map.get(vault).map_or(home, |a| *a == account);
        // This account's items in vaults 1Password doesn't have (made
        // here, or a vault gone there): synced ones go with their vault;
        // changed or new ones move to its default vault, to be sent.
        let stray = |vault: &str| ours(vault) && !known.contains(vault);
        self.data.entries.retain(|e| !stray(&e.item.vault_id) || e.remote.is_none() || e.dirty);
        for e in self.data.entries.iter_mut().filter(|e| stray(&e.item.vault_id)) {
            e.item.vault_id = home_vault.clone();
            e.remote = None;
            e.dirty = true;
        }
        self.data.synced.retain(|id, _| !ours(id) || known.contains(id.as_str()));
        // This account's vaults, as 1Password has them, after the others.
        self.data.vaults.retain(|v| !ours(&v.id));
        self.data.vault_account.retain(|_, a| *a != account);
        for v in &remote {
            self.data.vault_account.insert(v.id.clone(), account.clone());
        }
        self.data.vaults.extend(remote);
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

    /// A store, and its key, as a backup brings them to another home:
    /// `OLO_BACKUP_TEST=make` makes one with a login in it (its key in the
    /// login keyring), `=check` opens it and finds the login. Only on a
    /// private test bus (tools/dev/private-keyring), never the person's
    /// keyring; the config directory as `XDG_CONFIG_HOME` has it.
    #[test]
    #[ignore]
    fn through_a_backup() {
        let bus = std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap_or_default();
        assert!(bus.contains("/var/tmp/olo-dev/"), "not a private test bus: {bus}");
        assert!(std::env::var("XDG_CONFIG_HOME").is_ok_and(|c| c.starts_with("/var/tmp/")), "not a scratch config directory");
        match std::env::var("OLO_BACKUP_TEST").as_deref() {
            Ok("make") => {
                let mut v = Vault::open().unwrap();
                login(&mut v, "Backup test");
                v.save().unwrap();
            }
            Ok("check") => {
                let v = Vault::open().unwrap();
                let vault = v.data.vaults[0].id.clone();
                let item = v.items(&vault).into_iter().find(|i| i.title == "Backup test").expect("the login came along");
                let full = v.get(&vault, &item.id).unwrap();
                assert!(full.fields.iter().any(|f| f.value == "hunter2"));
            }
            other => panic!("OLO_BACKUP_TEST=make or check, not {other:?}"),
        }
    }

    /// A fresh store in a directory of its own, its key in a file there.
    fn fresh(name: &str) -> (Vault, PathBuf) {
        let dir = std::env::temp_dir().join(format!("olo-vault-test-{}-{name}", std::process::id()));
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

    /// Shared with agent screens: every screen's copy has what every
    /// screen has, a screen given more or less a copy of its own, each
    /// opened with its own key alone; archived items stay out; a screen
    /// back to what every screen has loses its own copy.
    #[test]
    fn shared_with_screens() {
        let (mut v, dir) = fresh("shares");
        let screens = dir.join("screens");
        let (bank, mail, wifi) = (login(&mut v, "Bank"), login(&mut v, "Mail"), login(&mut v, "Wi-Fi"));
        v.data.shares.everywhere = vec![mail.id.clone(), wifi.id.clone()];
        v.data.shares.screens.insert("agent".into(), ScreenShares { add: vec![bank.id.clone()], remove: vec![wifi.id.clone()] });
        let mut written = HashMap::new();
        v.write_copies_in(&screens, &mut written);
        let titles = |screen: &str| {
            let mut t: Vec<String> = Vault::open_shared_in(&screens, screen).unwrap().data.entries.iter().map(|e| e.item.title.clone()).collect();
            t.sort();
            t
        };
        assert_eq!(titles("agent"), ["Bank", "Mail"]);
        assert_eq!(titles("other"), ["Mail", "Wi-Fi"]);
        // Sealed, each with its own key, secrets and all.
        let copy = shared::copy_path_in(&screens, Some("agent"));
        assert!(!std::fs::read(&copy).unwrap().windows(7).any(|w| w == b"hunter2"));
        assert_eq!(Vault::open_shared_in(&screens, "agent").unwrap().get("", &bank.id).unwrap().password(), Some("hunter2"));
        assert!(Vault::open_at(copy.clone(), Some(&shared::key_path(&shared::copy_path_in(&screens, None)))).is_err());
        // Unchanged, not written again.
        let at = std::fs::metadata(&copy).unwrap().modified().unwrap();
        v.write_copies_in(&screens, &mut written);
        assert_eq!(std::fs::metadata(&copy).unwrap().modified().unwrap(), at);
        // Archived: gone from what's shared.
        v.archive("", &mail.id).unwrap();
        v.write_copies_in(&screens, &mut written);
        assert_eq!(titles("other"), ["Wi-Fi"]);
        // Back to what every screen has: its own copy goes.
        v.data.shares.screens.clear();
        v.write_copies_in(&screens, &mut written);
        assert!(!copy.exists());
        assert_eq!(titles("agent"), ["Wi-Fi"]);
        // Nothing shared yet: an empty store, made nowhere.
        let none = dir.join("none");
        assert!(Vault::open_shared_in(&none, "agent").unwrap().data.entries.is_empty() && !none.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A store to try Passwords with (`OLO_FIXTURE_VAULT`, its key in
    /// `OLO_FIXTURE_KEY`): logins and a note in two vaults. `cargo test
    /// --release -p olo-passwords fixture -- --ignored`.
    #[test]
    #[ignore]
    fn fixture() {
        let (Some(file), Some(key)) = (std::env::var_os("OLO_FIXTURE_VAULT"), std::env::var_os("OLO_FIXTURE_KEY")) else { return };
        let mut v = Vault::open_at(PathBuf::from(file), Some(Path::new(&key))).unwrap();
        v.data.vaults.push(VaultOverview { id: format!("{LOCAL}work"), title: "Work".into(), ..local_vault() });
        for (vault, title, user, site) in [
            (0, "Bank", "ben", "https://bank.example.com"),
            (0, "Mail", "ben@example.com", "https://mail.example.com"),
            (1, "GitHub (work)", "ben-work", "https://github.com"),
            (1, "Build server", "deploy", "https://ci.example.org"),
            (1, "Staging database", "admin", "https://db.staging.example.org"),
        ] {
            let mut p = ItemCreateParams::new(ItemCategory::Login, v.data.vaults[vault].id.clone(), title);
            p.fields = vec![ItemField::new("username", "username", ItemFieldType::Text, user), ItemField::new("password", "password", ItemFieldType::Concealed, "correct horse battery staple")];
            p.websites = vec![onepassword::Website { url: site.into(), label: "website".into(), autofill_behavior: onepassword::AutofillBehavior::AnywhereOnWebsite }];
            v.create(&p).unwrap();
        }
        let mut note = ItemCreateParams::new(ItemCategory::SecureNote, v.data.vaults[0].id.clone(), "Wi-Fi at home");
        note.notes = Some("The network is Olo, the key is on the router.".into());
        v.create(&note).unwrap();
    }

    /// A store with an SSH key in it, to try the SSH agent with
    /// (`OLO_FIXTURE_VAULT`, its key in `OLO_FIXTURE_KEY`): an Ed25519 key
    /// made from a fixed seed; its public key printed. `cargo test --release
    /// -p olo-passwords ssh_fixture -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn ssh_fixture() {
        use ssh_key::private::{Ed25519Keypair, KeypairData};
        let (Some(file), Some(key)) = (std::env::var_os("OLO_FIXTURE_VAULT"), std::env::var_os("OLO_FIXTURE_KEY")) else { return };
        let mut v = Vault::open_at(PathBuf::from(file), Some(Path::new(&key))).unwrap();
        let private = ssh_key::PrivateKey::new(KeypairData::Ed25519(Ed25519Keypair::from_seed(&[5u8; 32])), "").unwrap();
        let mut p = ItemCreateParams::new(ItemCategory::SshKey, v.data.vaults[0].id.clone(), "Test SSH key");
        p.fields = vec![ItemField::new("private_key", "private key", ItemFieldType::SshKey, private.to_openssh(ssh_key::LineEnding::LF).unwrap().as_str())];
        v.create(&p).unwrap();
        println!("{}", private.public_key().to_openssh().unwrap());
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
        let snap = v.snapshot("acct", true);
        assert_eq!(snap.entries.len(), 1);
        // 1Password has a vault: the local one's item moves there.
        v.apply(vec![Change::Vaults { account: "acct".into(), vaults: vec![remote_vault("v1")], home: true }]).unwrap();
        assert_eq!(v.get("", &mine.id).unwrap().vault_id, "v1");
        assert!(v.data.entries[0].dirty && v.data.entries[0].remote.is_none());
        // Sent, and 1Password gave it its own id.
        let rev = v.data.entries[0].rev;
        let mut sent = v.get("", &mine.id).unwrap();
        sent.id = "remoteid".into();
        sent.version = 1;
        let renamed = v.apply(vec![Change::Pushed { local_id: mine.id.clone(), rev, item: Box::new(sent.clone()), state: ItemState::Active }]).unwrap();
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
        v.snapshot("acct", true);
        v.apply(vec![Change::Vaults { account: "acct".into(), vaults: vec![remote_vault("v1")], home: true }]).unwrap();
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
        v.snapshot("acct", true);
        v.apply(vec![
            Change::Vaults { account: "acct".into(), vaults: vec![remote_vault("v1")], home: true },
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
    fn several_accounts_keep_to_their_own_vaults() {
        let (mut v, _) = fresh("accounts");
        let mine = login(&mut v, "Mine");
        let vaults = |account: &str, ids: &[&str], home| Change::Vaults { account: account.into(), vaults: ids.iter().map(|id| remote_vault(id)).collect(), home };
        let fetched = |id: &str, vault: &str| {
            let mut item = theirs(id, 1);
            item.vault_id = vault.into();
            Change::Fetched { item: Box::new(item), state: ItemState::Active }
        };
        // Only the first account takes items made here.
        assert!(v.snapshot("two", false).entries.is_empty());
        assert_eq!(v.snapshot("one", true).entries.len(), 1);
        v.apply(vec![vaults("one", &["v1"], true), fetched("a", "v1"), vaults("two", &["w1", "w2"], false), fetched("b", "w1")]).unwrap();
        assert_eq!(v.get("", &mine.id).unwrap().vault_id, "v1");
        assert_eq!(v.data.vaults.iter().map(|v| v.id.as_str()).collect::<Vec<_>>(), ["v1", "w1", "w2"]);
        // Each account's snapshot holds only its vaults' items, so one
        // account never sees another's vaults as gone.
        let one: Vec<String> = v.snapshot("one", true).entries.iter().map(|e| e.item.id.clone()).collect();
        let two: Vec<String> = v.snapshot("two", false).entries.iter().map(|e| e.item.id.clone()).collect();
        assert!(one.contains(&"a".to_string()) && !one.contains(&"b".to_string()));
        assert_eq!(two, ["b"]);
        // A vault gone from one account takes only its items.
        v.apply(vec![vaults("two", &["w2"], false)]).unwrap();
        assert!(v.get("", "b").is_err() && v.get("", "a").is_ok());
        // Removing an account: its items go, an unsent one stays here.
        v.apply(vec![fetched("c", "w2")]).unwrap();
        let mut p = ItemCreateParams::new(onepassword::ItemCategory::Login, "w2", "Unsent");
        p.fields = vec![];
        let unsent = v.create(&p).unwrap();
        v.remove_account("two").unwrap();
        assert!(v.get("", "c").is_err());
        assert_eq!(v.get("", &unsent.id).unwrap().vault_id, local_vault().id);
        assert!(v.get("", "a").is_ok());
        assert!(!v.data.vault_account.values().any(|a| a == "two"));
        // A reset leaves nothing.
        v.reset().unwrap();
        assert!(v.data.entries.is_empty() && v.data.vault_account.is_empty() && v.data.tombstones.is_empty());
    }

    /// A store from when there was one account takes it for its vaults.
    #[test]
    fn one_account_stores_carry_over() {
        let (mut v, dir) = fresh("legacy");
        v.data.vaults.push(remote_vault("v1"));
        v.data.account = Some("old".into());
        v.save().unwrap();
        let file = v.file.clone();
        drop(v);
        let v = Vault::open_at(file, Some(&dir.join("key"))).unwrap();
        assert_eq!(v.data.vault_account.get("v1").map(String::as_str), Some("old"));
        assert!(!v.data.vault_account.contains_key(&local_vault().id));
    }

    /// Edit ▸ Undo's ways back (`undo.rs`): an edit put back as it was, a
    /// deleted item made again with every field as it was.
    #[test]
    fn undo_puts_items_back() {
        let (mut v, dir) = fresh("undo");
        let item = login(&mut v, "Mail");
        let mut changed = item.clone();
        changed.title = "Mail (work)".into();
        let after = v.put(&changed).unwrap();
        // Undo the edit: as it was.
        let back = v.put(&item).unwrap();
        assert_eq!((back.id.as_str(), back.title.as_str()), (after.id.as_str(), "Mail"));
        // Deleted, then made again from what the delete gave back.
        let was = v.delete("", &item.id).unwrap();
        assert!(v.get("", &item.id).is_err());
        let again = v.create(&crate::undo::params_of(&was)).unwrap();
        assert_eq!((again.title.as_str(), again.vault_id.as_str()), (was.title.as_str(), was.vault_id.as_str()));
        assert_eq!(again.fields, was.fields);
        assert_ne!(again.id, was.id, "a new item");
        let _ = std::fs::remove_dir_all(dir);
    }
}
