//! A repository in a store: blobs (chunks and trees) packed, indexed and
//! sealed; snapshots; the config that proves the key.
//!
//! Trees go in packs of their own, which the local cache keeps whole (a
//! pack's own bytes, still sealed): walking a snapshot (the next backup's
//! unchanged files, browsing to restore, tidying) reads only the disk, never
//! a slow store. The index goes up every few minutes while a backup runs,
//! so one stopped part way (or the computer turned off) loses at most a
//! pack's worth: the next goes on from there.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};

use crate::{
    Error, Result,
    crypto::{Id, Key},
    store::Store,
};

/// A pack goes up once it's this big (few large objects for a slow store).
pub const PACK_TARGET: usize = 16 << 20;
/// The index goes up at least this often while packs do.
const CHECKPOINT: Duration = Duration::from_secs(5 * 60);
/// The repository format.
const VERSION: u32 = 1;

/// What a blob is, and how it's kept.
pub(crate) const TREE: u8 = 1;
const COMPRESSED: u8 = 2;

/// Where a blob is in a pack.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(crate) struct Entry {
    pub id: Id,
    pub flags: u8,
    pub offset: u32,
    pub len: u32,
    /// Its size before compression.
    pub raw: u32,
}

/// The two packs being filled: chunks, and trees.
pub(crate) const DATA: usize = 0;
pub(crate) const TREES: usize = 1;

#[derive(Default)]
pub(crate) struct Writer {
    data: Vec<u8>,
    entries: Vec<Entry>,
}

#[derive(Clone, Copy)]
struct Loc {
    pack: u32,
    flags: u8,
    offset: u32,
    len: u32,
}

#[derive(Serialize, Deserialize)]
struct Config {
    version: u32,
    /// The repository's own id (a cache's name).
    id: Id,
    created: i64,
    chunks: (usize, usize, usize),
}

/// One run of backing up: what it saw and did.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    pub files: u64,
    pub dirs: u64,
    /// Every file's size, added up.
    pub bytes: u64,
    pub new_files: u64,
    pub changed_files: u64,
    /// Read again or not at all (unchanged since the last snapshot).
    pub unchanged_files: u64,
    /// New content: chunks the repository didn't have, before compression.
    pub new_bytes: u64,
    /// What went up, compressed and sealed.
    pub stored_bytes: u64,
    pub seconds: f64,
    /// What couldn't be read, and why.
    #[serde(default)]
    pub errors: Vec<String>,
}

/// Everything backed up at a moment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    /// Seconds since 1970.
    pub time: i64,
    pub host: String,
    /// The folders backed up.
    pub paths: Vec<String>,
    /// The tree of `/` holding them.
    pub tree: Id,
    pub parent: Option<Id>,
    pub summary: Summary,
    /// An unfinished one's chunks beyond its tree: the file it was part
    /// way through (kept by tidying, so they aren't sent again).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra: Vec<Id>,
}

pub struct Repo {
    pub(crate) store: Arc<dyn Store>,
    pub(crate) key: Key,
    id: Id,
    /// Local copies of what never changes in the store (index files, and
    /// tree packs).
    pub(crate) cache: Option<PathBuf>,
    index: HashMap<Id, Loc>,
    packs: Vec<Id>,
    pack_no: HashMap<Id, u32>,
    /// Each pack's holding only trees (so kept in the cache whole).
    tree_pack: Vec<bool>,
    /// The index files read.
    index_files: HashSet<String>,
    /// Index files that don't open or don't read (damaged in the store):
    /// passed over, their packs read back from their own lists instead,
    /// and let go of when tidying.
    damaged: HashSet<String>,
    /// Packs read back from their own lists here and not yet named in an
    /// index file in the store (`find_lost` names them).
    unnamed: Vec<(Id, Vec<Entry>)>,
    /// The packs being filled.
    writers: [Writer; 2],
    pending: HashSet<Id>,
    /// Packs gone up since the index was last written.
    pub(crate) written: Vec<(Id, Vec<Entry>)>,
    last_index: Instant,
    /// The index goes up every `CHECKPOINT` (not while tidying: that
    /// writes its own).
    pub(crate) checkpoints: bool,
    lock: crate::lock::Registered,
    /// One compressor for every blob (not one made for each).
    zstd: zstd::bulk::Compressor<'static>,
    /// What went up, sealed, since opened.
    pub stored: u64,
}

pub(crate) fn pack_name(id: &Id) -> String {
    let h = id.hex();
    format!("data/{}/{h}", &h[..2])
}

/// Written whole or not at all (a cache's copy).
fn write_file(path: &Path, data: &[u8]) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = path.with_extension("part");
    if std::fs::write(&tmp, data).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

impl Repo {
    /// A new repository in `store` (empty), sealed with `key`.
    pub fn init(store: Arc<dyn Store>, key: Key, cache: Option<PathBuf>) -> Result<Repo> {
        if store.get("config").is_ok() {
            return Err(Error::Exists);
        }
        let mut raw = [0u8; 32];
        getrandom::fill(&mut raw).expect("the system's random source failed");
        let config = Config { version: VERSION, id: Id(raw), created: now(), chunks: (crate::chunker::MIN, crate::chunker::AVG, crate::chunker::MAX) };
        store.put("config", &key.seal(&encode(&config)))?;
        Repo::open(store, key, cache)
    }

    /// The repository in `store`, if `key` opens it; its index read (from
    /// `cache` where it was before).
    pub fn open(store: Arc<dyn Store>, key: Key, cache: Option<PathBuf>) -> Result<Repo> {
        let sealed = match store.get("config") {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(Error::NoRepository),
            Err(e) => return Err(e.into()),
        };
        let config: Config = decode(&key.open(&sealed).ok_or(Error::WrongKey)?)?;
        if config.version > VERSION {
            return Err(Error::Damaged(format!("made by a newer DiskBackup (format {})", config.version)));
        }
        let cache = cache.map(|c| c.join(config.id.hex()));
        let mut repo = Repo {
            store,
            key,
            id: config.id,
            cache,
            index: HashMap::new(),
            packs: Vec::new(),
            pack_no: HashMap::new(),
            tree_pack: Vec::new(),
            index_files: HashSet::new(),
            damaged: HashSet::new(),
            unnamed: Vec::new(),
            writers: Default::default(),
            pending: HashSet::new(),
            written: Vec::new(),
            last_index: Instant::now(),
            checkpoints: true,
            lock: Default::default(),
            zstd: zstd::bulk::Compressor::new(3)?,
            stored: 0,
        };
        repo.refresh()?;
        Ok(repo)
    }

    /// Take a lock on it (see `lock`): shared to back up or restore, the
    /// only one to tidy. Held until dropped; what this `Repo` writes
    /// meanwhile is checked against it (none written once it's lost).
    pub fn lock(&self, exclusive: bool) -> Result<crate::Lock> {
        let lock = crate::lock::take(self.store.clone(), self.key.clone(), exclusive)?;
        self.lock.set(&lock);
        Ok(lock)
    }

    /// The lock this holds, still held (none taken: nothing to check).
    pub(crate) fn check_lock(&self) -> Result<()> {
        match self.lock.get() {
            Some(held) => held.check(),
            None => Ok(()),
        }
    }

    /// Held, and the only one (to tidy).
    pub(crate) fn check_exclusive(&self) -> Result<()> {
        match self.lock.get() {
            Some(held) if held.exclusive => held.check(),
            _ => Err(Error::Busy("tidying needs the backup to itself".into())),
        }
    }

    /// The repository's own id.
    pub fn id(&self) -> Id {
        self.id
    }

    pub fn key(&self) -> &Key {
        &self.key
    }

    pub fn store(&self) -> &Arc<dyn Store> {
        &self.store
    }

    /// Blobs it has (an idea of its size).
    pub fn blobs(&self) -> usize {
        self.index.len()
    }

    /// The index as the store has it now: index files new since it was
    /// read added; all read again if any went (someone tidied). One that
    /// doesn't open or read (damaged) is passed over, and the packs no
    /// index names are read back from their own lists in its place: what
    /// it named can still be restored, and isn't backed up again.
    pub fn refresh(&mut self) -> Result<()> {
        let names: Vec<String> = self.store.list("index")?.into_iter().map(|(n, _)| n).collect();
        let listed: HashSet<&String> = names.iter().collect();
        if !self.index_files.iter().all(|n| listed.contains(n)) {
            self.forget_index();
        }
        self.damaged.retain(|n| listed.contains(n));
        let mut damaged = false;
        for name in names {
            if self.index_files.contains(&name) || self.damaged.contains(&name) {
                continue;
            }
            match self.cached(&name).and_then(|plain| decode::<Vec<(Id, Vec<Entry>)>>(&plain)) {
                Ok(files) => {
                    for (pack, entries) in files {
                        self.add_pack(pack, &entries);
                    }
                    self.index_files.insert(name);
                }
                Err(Error::Damaged(_)) => {
                    self.damaged.insert(name);
                    damaged = true;
                }
                Err(e) => return Err(e),
            }
        }
        if damaged {
            self.read_lost()?;
        }
        Ok(())
    }

    /// Index files found damaged (passed over; tidying lets go of them).
    pub fn damaged_index_files(&self) -> usize {
        self.damaged.len()
    }

    /// The index read again, all of it.
    pub(crate) fn reload(&mut self) -> Result<()> {
        self.forget_index();
        self.refresh()
    }

    fn forget_index(&mut self) {
        self.index.clear();
        self.packs.clear();
        self.pack_no.clear();
        self.tree_pack.clear();
        self.index_files.clear();
        self.damaged.clear();
        self.unnamed.clear();
        // (Packs gone up whose index hasn't yet.)
        for (pack, entries) in self.written.clone() {
            self.add_pack(pack, &entries);
        }
    }

    fn add_pack(&mut self, pack: Id, entries: &[Entry]) {
        let n = match self.pack_no.get(&pack) {
            Some(n) => *n,
            None => {
                self.packs.push(pack);
                self.tree_pack.push(true);
                let n = (self.packs.len() - 1) as u32;
                self.pack_no.insert(pack, n);
                n
            }
        };
        for e in entries {
            self.tree_pack[n as usize] &= e.flags & TREE != 0;
            self.index.insert(e.id, Loc { pack: n, flags: e.flags, offset: e.offset, len: e.len });
        }
    }

    /// An object that never changes, opened: from the cache, else fetched
    /// (and cached).
    pub(crate) fn cached(&self, name: &str) -> Result<Vec<u8>> {
        let local = self.cache.as_ref().map(|c| c.join(name));
        if let Some(p) = &local
            && let Ok(sealed) = std::fs::read(p)
            && let Some(plain) = self.key.open(&sealed)
        {
            return Ok(plain);
        }
        let sealed = self.store.get(name)?;
        let plain = self.key.open(&sealed).ok_or_else(|| Error::Damaged(name.to_owned()))?;
        if let Some(p) = local {
            write_file(&p, &sealed);
        }
        Ok(plain)
    }

    /// `len` bytes of pack `pack` from `offset`: a tree pack's from the
    /// cache's copy (fetched whole the first time), unless `fresh`.
    fn pack_range(&self, n: u32, offset: u32, len: u32, fresh: bool) -> Result<Vec<u8>> {
        let name = pack_name(&self.packs[n as usize]);
        let local = self.cache.as_ref().filter(|_| self.tree_pack[n as usize]).map(|c| c.join(&name));
        let Some(local) = local else { return Ok(self.store.get_range(&name, offset as u64, len as u64)?) };
        if !fresh && let Ok(f) = std::fs::File::open(&local) {
            let mut out = vec![0u8; len as usize];
            if std::os::unix::fs::FileExt::read_exact_at(&f, &mut out, offset as u64).is_ok() {
                return Ok(out);
            }
        }
        let whole = self.store.get(&name)?;
        write_file(&local, &whole);
        whole.get(offset as usize..(offset + len) as usize).map(<[u8]>::to_vec).ok_or_else(|| Error::Damaged(format!("{name} is short")))
    }

    pub fn has(&self, id: &Id) -> bool {
        self.index.contains_key(id) || self.pending.contains(id)
    }

    /// Keep `data` (a chunk, or a tree if `tree`); its id, and whether it
    /// was new.
    pub fn put(&mut self, data: &[u8], tree: bool) -> Result<(Id, bool)> {
        let id = self.key.id(data);
        if self.has(&id) {
            return Ok((id, false));
        }
        let mut flags = if tree { TREE } else { 0 };
        let compressed = self.zstd.compress(data)?;
        let body = if compressed.len() < data.len() - data.len() / 32 {
            flags |= COMPRESSED;
            compressed
        } else {
            data.to_vec()
        };
        let sealed = self.key.seal(&body);
        self.put_sealed(Entry { id, flags, offset: 0, len: sealed.len() as u32, raw: data.len() as u32 }, &sealed)?;
        Ok((id, true))
    }

    /// A blob kept as it is (sealed; `e` says what it is), in the pack for
    /// its kind.
    pub(crate) fn put_sealed(&mut self, mut e: Entry, sealed: &[u8]) -> Result<()> {
        let which = if e.flags & TREE != 0 { TREES } else { DATA };
        let w = &mut self.writers[which];
        e.offset = w.data.len() as u32;
        w.entries.push(e);
        w.data.extend_from_slice(sealed);
        self.pending.insert(e.id);
        if w.data.len() >= PACK_TARGET {
            self.finish_pack(which)?;
        }
        Ok(())
    }

    /// A pack's own list of what it holds (its sealed header, at its end,
    /// before the header's length): two small reads, not the pack.
    pub(crate) fn pack_header(&self, id: &Id, size: u64) -> Result<Vec<Entry>> {
        let name = pack_name(id);
        if size < 4 {
            return Err(Error::Damaged(format!("{name} is short")));
        }
        let tail = self.store.get_range(&name, size - 4, 4)?;
        let len = u32::from_le_bytes([tail[0], tail[1], tail[2], tail[3]]) as u64;
        if len + 4 > size {
            return Err(Error::Damaged(format!("{name}'s list doesn't fit")));
        }
        let sealed = self.store.get_range(&name, size - 4 - len, len)?;
        let plain = self.key.open(&sealed).ok_or_else(|| Error::Damaged(format!("{name}'s list doesn't open")))?;
        decode(&plain)
    }

    /// Packs no index names (an index file lost or damaged; another
    /// computer's backup not named yet), read back from their own lists and
    /// named again in an index file: how many.
    pub fn find_lost(&mut self) -> Result<usize> {
        self.check_lock()?;
        self.read_lost()?;
        let found = std::mem::take(&mut self.unnamed);
        if !found.is_empty() {
            match self.put_index(&found) {
                // (Named by what it holds, it may be a damaged one's name:
                // that one is whole again.)
                Ok(name) => {
                    self.damaged.remove(&name);
                    self.index_files.insert(name);
                }
                Err(e) => {
                    self.unnamed = found;
                    return Err(e);
                }
            }
        }
        Ok(found.len())
    }

    /// Packs no index names read back from their own lists into the index
    /// here (`unnamed` till an index file names them): how many.
    fn read_lost(&mut self) -> Result<usize> {
        let named: HashSet<Id> = self.packs.iter().copied().collect();
        let mut n = 0;
        for (name, size) in self.store.list("data")? {
            let Some(id) = name.rsplit('/').next().and_then(Id::from_hex) else { continue };
            if named.contains(&id) {
                continue;
            }
            if let Ok(entries) = self.pack_header(&id, size) {
                self.add_pack(id, &entries);
                self.unnamed.push((id, entries));
                n += 1;
            }
        }
        Ok(n)
    }

    /// Packs read back from their own lists and not yet named in the
    /// store's index.
    pub(crate) fn has_unnamed(&self) -> bool {
        !self.unnamed.is_empty()
    }

    /// A pack taken into the index here (not yet named in the store's).
    pub(crate) fn take_pack(&mut self, pack: Id, entries: &[Entry]) {
        self.add_pack(pack, entries);
    }

    /// A whole pack (a tree pack's from the cache, if it's there whole).
    pub(crate) fn pack_bytes(&self, id: &Id, trees: bool) -> Result<Vec<u8>> {
        let name = pack_name(id);
        if trees
            && let Some(c) = &self.cache
            && let Ok(data) = std::fs::read(c.join(&name))
            && self.key.id(&data) == *id
        {
            return Ok(data);
        }
        Ok(self.store.get(&name)?)
    }

    /// The blob `id` holds (checked against its id).
    pub fn get(&self, id: &Id) -> Result<Vec<u8>> {
        if self.pending.contains(id) {
            for w in &self.writers {
                if let Some(e) = w.entries.iter().find(|e| e.id == *id) {
                    return self.unseal(id, &w.data[e.offset as usize..(e.offset + e.len) as usize], e.flags);
                }
            }
        }
        let loc = *self.index.get(id).ok_or_else(|| Error::Damaged(format!("no blob {id:?}")))?;
        let sealed = self.pack_range(loc.pack, loc.offset, loc.len, false)?;
        match self.unseal(id, &sealed, loc.flags) {
            // (The cache's copy damaged: the store's.)
            Err(Error::Damaged(_)) if self.cache.is_some() && self.tree_pack[loc.pack as usize] => {
                let sealed = self.pack_range(loc.pack, loc.offset, loc.len, true)?;
                self.unseal(id, &sealed, loc.flags)
            }
            r => r,
        }
    }

    fn unseal(&self, id: &Id, sealed: &[u8], flags: u8) -> Result<Vec<u8>> {
        unseal(&self.key, id, sealed, flags)
    }

    /// A reader of its trees that needs nothing of it after (for another
    /// thread, while this one backs up): where each tree is, as now.
    pub fn tree_reader(&self) -> TreeReader {
        let trees = self.index.iter().filter(|(_, l)| l.flags & TREE != 0).map(|(id, l)| (*id, (self.packs[l.pack as usize], l.offset, l.len, l.flags))).collect();
        TreeReader { key: self.key.clone(), store: self.store.clone(), cache: self.cache.clone(), trees, seen: Default::default() }
    }

    /// A pack being filled, up: its blobs then its sealed list of them (a
    /// tree pack kept in the cache too); the index too if it's been a while.
    pub(crate) fn finish_pack(&mut self, which: usize) -> Result<()> {
        if self.writers[which].entries.is_empty() {
            return Ok(());
        }
        self.check_lock()?;
        let Writer { data: mut pack, entries } = std::mem::take(&mut self.writers[which]);
        let header = self.key.seal(&encode(&entries));
        pack.extend_from_slice(&header);
        pack.extend_from_slice(&(header.len() as u32).to_le_bytes());
        let pid = self.key.id(&pack);
        let name = pack_name(&pid);
        self.store.put(&name, &pack)?;
        self.stored += pack.len() as u64;
        // (Its checksum, to compare with the store's when it's checked.)
        if self.store.keeps_checksums() {
            crate::verify::note_sum(self, &pid, &pack);
        }
        if which == TREES
            && let Some(c) = &self.cache
        {
            write_file(&c.join(&name), &pack);
        }
        self.add_pack(pid, &entries);
        for e in &entries {
            self.pending.remove(&e.id);
        }
        self.written.push((pid, entries));
        if self.checkpoints && self.last_index.elapsed() >= CHECKPOINT {
            self.write_index()?;
        }
        Ok(())
    }

    /// The index of the packs gone up since it last did, up.
    pub fn write_index(&mut self) -> Result<()> {
        if self.written.is_empty() {
            return Ok(());
        }
        self.check_lock()?;
        let written = std::mem::take(&mut self.written);
        match self.put_index(&written) {
            Ok(name) => {
                self.index_files.insert(name);
                self.last_index = Instant::now();
                Ok(())
            }
            Err(e) => {
                self.written = written;
                Err(e)
            }
        }
    }

    /// An index file of `packs`, up (and in the cache); its name.
    pub(crate) fn put_index(&mut self, packs: &[(Id, Vec<Entry>)]) -> Result<String> {
        let plain = encode(&packs);
        let name = format!("index/{}", self.key.id(&plain).hex());
        let sealed = self.key.seal(&plain);
        self.store.put(&name, &sealed)?;
        self.stored += sealed.len() as u64;
        if let Some(c) = &self.cache {
            write_file(&c.join(&name), &sealed);
        }
        Ok(name)
    }

    /// Everything kept so far up, and the index of it.
    pub fn flush(&mut self) -> Result<()> {
        self.finish_pack(DATA)?;
        self.finish_pack(TREES)?;
        self.write_index()
    }

    /// Keep a snapshot (everything before it flushed first); its id.
    pub fn save_snapshot(&mut self, s: &Snapshot) -> Result<Id> {
        self.flush()?;
        self.check_lock()?;
        let plain = encode(s);
        let id = self.key.id(&plain);
        self.store.put(&format!("snapshots/{}", id.hex()), &self.key.seal(&plain))?;
        Ok(id)
    }

    /// Every snapshot, oldest first.
    pub fn snapshots(&self) -> Result<Vec<(Id, Snapshot)>> {
        self.read_snapshots("snapshots")
    }

    /// Keep an unfinished snapshot (a backup's as far as it got: everything
    /// before it flushed first); its id.
    pub fn save_unfinished(&mut self, s: &Snapshot) -> Result<Id> {
        self.flush()?;
        self.check_lock()?;
        let plain = encode(s);
        let id = self.key.id(&plain);
        self.store.put(&format!("unfinished/{}", id.hex()), &self.key.seal(&plain))?;
        Ok(id)
    }

    /// Every unfinished snapshot (any computer's), oldest first.
    pub fn unfinished(&self) -> Result<Vec<(Id, Snapshot)>> {
        self.read_snapshots("unfinished")
    }

    /// Forget an unfinished snapshot (what only it used goes at the next
    /// prune).
    pub fn forget_unfinished(&self, id: &Id) -> Result<()> {
        self.check_lock()?;
        Ok(self.store.delete(&format!("unfinished/{}", id.hex()))?)
    }

    fn read_snapshots(&self, dir: &str) -> Result<Vec<(Id, Snapshot)>> {
        let mut out = Vec::new();
        for (name, _) in self.store.list(dir)? {
            let Some(id) = name.rsplit('/').next().and_then(Id::from_hex) else { continue };
            let sealed = self.store.get(&name)?;
            let plain = self.key.open(&sealed).ok_or_else(|| Error::Damaged(name.clone()))?;
            out.push((id, decode::<Snapshot>(&plain)?));
        }
        out.sort_by_key(|(_, s)| s.time);
        Ok(out)
    }

    /// Forget a snapshot (what only it used goes at the next prune).
    pub fn forget(&self, id: &Id) -> Result<()> {
        self.check_lock()?;
        Ok(self.store.delete(&format!("snapshots/{}", id.hex()))?)
    }

    /// Where blob `id` is: its pack, offset, length (sealed) and flags.
    pub(crate) fn locate(&self, id: &Id) -> Option<(Id, u32, u32, u8)> {
        self.index.get(id).map(|l| (self.packs[l.pack as usize], l.offset, l.len, l.flags))
    }

    /// Each blob the index has, and its size as kept (sealed).
    pub(crate) fn blob_sizes(&self) -> impl Iterator<Item = (&Id, u64)> {
        self.index.iter().map(|(id, l)| (id, l.len as u64))
    }

    /// The packs the index has, and whether each holds only trees.
    pub(crate) fn pack_ids(&self) -> impl Iterator<Item = (&Id, bool)> {
        self.packs.iter().zip(self.tree_pack.iter().copied())
    }

    /// A tree, by id.
    pub fn tree(&self, id: &Id) -> Result<crate::Tree> {
        crate::Tree::decode(&self.get(id)?)
    }
}

/// A blob opened: unsealed, decompressed, checked against its id.
pub(crate) fn unseal(key: &Key, id: &Id, sealed: &[u8], flags: u8) -> Result<Vec<u8>> {
    let body = key.open(sealed).ok_or_else(|| Error::Damaged(format!("blob {id:?} doesn't open")))?;
    let plain = if flags & COMPRESSED != 0 { zstd::stream::decode_all(&body[..])? } else { body };
    if key.id(&plain) != *id {
        return Err(Error::Damaged(format!("blob {id:?} isn't what it was")));
    }
    Ok(plain)
}

/// What reads trees: a repository, or a [`TreeReader`] of one.
pub trait Trees {
    fn tree(&self, id: &Id) -> Result<crate::Tree>;
}

impl Trees for Repo {
    fn tree(&self, id: &Id) -> Result<crate::Tree> {
        Repo::tree(self, id)
    }
}

/// A repository's trees, read from the cache's tree packs (the store's
/// if the cache hasn't one), as they were when it was made
/// ([`Repo::tree_reader`]); the last few hundred read kept.
pub struct TreeReader {
    key: Key,
    store: Arc<dyn Store>,
    cache: Option<PathBuf>,
    /// Each tree's pack, place, length and flags.
    trees: HashMap<Id, (Id, u32, u32, u8)>,
    seen: std::sync::Mutex<HashMap<Id, Arc<crate::Tree>>>,
}

impl TreeReader {
    /// The tree `id`, shared.
    pub fn get(&self, id: &Id) -> Result<Arc<crate::Tree>> {
        if let Some(t) = self.seen.lock().ok().and_then(|s| s.get(id).cloned()) {
            return Ok(t);
        }
        let &(pack, offset, len, flags) = self.trees.get(id).ok_or_else(|| Error::Damaged(format!("no tree {id:?}")))?;
        let name = pack_name(&pack);
        let local = self.cache.as_ref().map(|c| c.join(&name)).and_then(|p| std::fs::File::open(p).ok()).and_then(|f| {
            let mut out = vec![0u8; len as usize];
            std::os::unix::fs::FileExt::read_exact_at(&f, &mut out, offset as u64).ok().map(|()| out)
        });
        let plain = match local.map(|sealed| unseal(&self.key, id, &sealed, flags)) {
            Some(Ok(plain)) => plain,
            _ => unseal(&self.key, id, &self.store.get_range(&name, offset as u64, len as u64)?, flags)?,
        };
        let tree = Arc::new(crate::Tree::decode(&plain)?);
        if let Ok(mut seen) = self.seen.lock() {
            if seen.len() >= 512 {
                seen.clear();
            }
            seen.insert(*id, tree.clone());
        }
        Ok(tree)
    }
}

impl Trees for TreeReader {
    fn tree(&self, id: &Id) -> Result<crate::Tree> {
        Ok((*self.get(id)?).clone())
    }
}

pub(crate) fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn encode<T: Serialize>(v: &T) -> Vec<u8> {
    let mut out = Vec::new();
    ciborium::into_writer(v, &mut out).expect("encodes");
    out
}

pub(crate) fn decode<T: serde::de::DeserializeOwned>(data: &[u8]) -> Result<T> {
    ciborium::from_reader(data).map_err(|e| Error::Damaged(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LocalStore;

    pub fn temp(name: &str) -> PathBuf {
        let d = PathBuf::from(format!("/var/tmp/olo-diskbackup-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Blobs kept once, read back as they were, across opening again; a
    /// wrong key, or no repository, said so.
    #[test]
    fn blobs_round_trip() {
        let dir = temp("repo");
        let store: Arc<dyn Store> = Arc::new(LocalStore::new(dir.join("store")));
        let key = Key::generate();
        let mut r = Repo::init(store.clone(), key.clone(), Some(dir.join("cache"))).unwrap();
        let (a, new) = r.put(&vec![7u8; 100_000], false).unwrap();
        assert!(new);
        assert_eq!(r.put(&vec![7u8; 100_000], false).unwrap(), (a, false));
        let (b, _) = r.put(b"not compressible? short", true).unwrap();
        assert_eq!(r.get(&a).unwrap(), vec![7u8; 100_000]);
        r.flush().unwrap();
        let r = Repo::open(store.clone(), key.clone(), Some(dir.join("cache"))).unwrap();
        assert_eq!(r.get(&b).unwrap(), b"not compressible? short");
        assert_eq!(r.blobs(), 2);
        assert!(matches!(Repo::open(store.clone(), Key::generate(), None), Err(Error::WrongKey)));
        assert!(matches!(Repo::init(store, key, None), Err(Error::Exists)));
        let empty: Arc<dyn Store> = Arc::new(LocalStore::new(dir.join("nothing")));
        assert!(matches!(Repo::open(empty, Key::generate(), None), Err(Error::NoRepository)));
        let _ = std::fs::remove_dir_all(dir);
    }
}
