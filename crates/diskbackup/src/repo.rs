//! A repository in a store: blobs (chunks and trees) packed, indexed and
//! sealed; snapshots; the config that proves the key.

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
};

use serde::{Deserialize, Serialize};

use crate::{
    Error, Result,
    crypto::{Id, Key},
    store::Store,
};

/// A pack goes up once it's this big (few large objects for a slow store).
pub const PACK_TARGET: usize = 16 << 20;
/// The repository format.
const VERSION: u32 = 1;

/// What a blob is, and how it's kept.
const TREE: u8 = 1;
const COMPRESSED: u8 = 2;

/// Where a blob is in a pack.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct Entry {
    id: Id,
    flags: u8,
    offset: u32,
    len: u32,
    /// Its size before compression.
    raw: u32,
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
}

pub struct Repo {
    store: Arc<dyn Store>,
    key: Key,
    id: Id,
    /// Local copies of what never changes in the store (index files).
    cache: Option<PathBuf>,
    index: HashMap<Id, Loc>,
    packs: Vec<Id>,
    /// The pack being filled, and what's in it.
    writing: Vec<u8>,
    entries: Vec<Entry>,
    pending: HashSet<Id>,
    /// Packs gone up since the index was last written.
    written: Vec<(Id, Vec<Entry>)>,
    /// What went up, sealed, since opened.
    pub stored: u64,
}

fn pack_name(id: &Id) -> String {
    let h = id.hex();
    format!("data/{}/{h}", &h[..2])
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
        let mut repo =
            Repo { store, key, id: config.id, cache, index: HashMap::new(), packs: Vec::new(), writing: Vec::new(), entries: Vec::new(), pending: HashSet::new(), written: Vec::new(), stored: 0 };
        repo.read_index()?;
        Ok(repo)
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

    fn read_index(&mut self) -> Result<()> {
        let mut packs: HashMap<Id, u32> = HashMap::new();
        for name in self.store.list("index")? {
            let plain = self.cached(&name)?;
            let files: Vec<(Id, Vec<Entry>)> = decode(&plain)?;
            for (pack, entries) in files {
                let n = *packs.entry(pack).or_insert_with(|| {
                    self.packs.push(pack);
                    (self.packs.len() - 1) as u32
                });
                for e in entries {
                    self.index.insert(e.id, Loc { pack: n, flags: e.flags, offset: e.offset, len: e.len });
                }
            }
        }
        Ok(())
    }

    /// An object that never changes, opened: from the cache, else fetched
    /// (and cached).
    fn cached(&self, name: &str) -> Result<Vec<u8>> {
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
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(p, &sealed);
        }
        Ok(plain)
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
        let compressed = zstd::bulk::compress(data, 3)?;
        let body = if compressed.len() < data.len() - data.len() / 32 {
            flags |= COMPRESSED;
            compressed
        } else {
            data.to_vec()
        };
        let sealed = self.key.seal(&body);
        self.entries.push(Entry { id, flags, offset: self.writing.len() as u32, len: sealed.len() as u32, raw: data.len() as u32 });
        self.writing.extend_from_slice(&sealed);
        self.pending.insert(id);
        if self.writing.len() >= PACK_TARGET {
            self.finish_pack()?;
        }
        Ok((id, true))
    }

    /// The blob `id` holds (checked against its id).
    pub fn get(&self, id: &Id) -> Result<Vec<u8>> {
        let (sealed, flags) = if self.pending.contains(id) {
            let e = self.entries.iter().find(|e| e.id == *id).expect("pending");
            (self.writing[e.offset as usize..(e.offset + e.len) as usize].to_vec(), e.flags)
        } else {
            let loc = self.index.get(id).ok_or_else(|| Error::Damaged(format!("no blob {id:?}")))?;
            (self.store.get_range(&pack_name(&self.packs[loc.pack as usize]), loc.offset as u64, loc.len as u64)?, loc.flags)
        };
        let body = self.key.open(&sealed).ok_or_else(|| Error::Damaged(format!("blob {id:?} doesn't open")))?;
        let plain = if flags & COMPRESSED != 0 { zstd::stream::decode_all(&body[..])? } else { body };
        if self.key.id(&plain) != *id {
            return Err(Error::Damaged(format!("blob {id:?} isn't what it was")));
        }
        Ok(plain)
    }

    /// The pack being filled, up: its blobs then its sealed list of them.
    fn finish_pack(&mut self) -> Result<()> {
        if self.entries.is_empty() {
            return Ok(());
        }
        let header = self.key.seal(&encode(&self.entries));
        let mut data = std::mem::take(&mut self.writing);
        data.extend_from_slice(&header);
        data.extend_from_slice(&(header.len() as u32).to_le_bytes());
        let pid = self.key.id(&data);
        self.store.put(&pack_name(&pid), &data)?;
        self.stored += data.len() as u64;
        self.packs.push(pid);
        let n = (self.packs.len() - 1) as u32;
        let entries = std::mem::take(&mut self.entries);
        for e in &entries {
            self.index.insert(e.id, Loc { pack: n, flags: e.flags, offset: e.offset, len: e.len });
        }
        self.pending.clear();
        self.written.push((pid, entries));
        Ok(())
    }

    /// Everything kept so far up, and the index of it.
    pub fn flush(&mut self) -> Result<()> {
        self.finish_pack()?;
        if self.written.is_empty() {
            return Ok(());
        }
        let plain = encode(&self.written);
        let name = format!("index/{}", self.key.id(&plain).hex());
        let sealed = self.key.seal(&plain);
        self.store.put(&name, &sealed)?;
        self.stored += sealed.len() as u64;
        if let Some(c) = &self.cache {
            let p = c.join(&name);
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(p, &sealed);
        }
        self.written.clear();
        Ok(())
    }

    /// Keep a snapshot (everything before it flushed first); its id.
    pub fn save_snapshot(&mut self, s: &Snapshot) -> Result<Id> {
        self.flush()?;
        let plain = encode(s);
        let id = self.key.id(&plain);
        self.store.put(&format!("snapshots/{}", id.hex()), &self.key.seal(&plain))?;
        Ok(id)
    }

    /// Every snapshot, oldest first.
    pub fn snapshots(&self) -> Result<Vec<(Id, Snapshot)>> {
        let mut out = Vec::new();
        for name in self.store.list("snapshots")? {
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
        Ok(self.store.delete(&format!("snapshots/{}", id.hex()))?)
    }

    /// A tree, by id.
    pub fn tree(&self, id: &Id) -> Result<crate::Tree> {
        crate::Tree::decode(&self.get(id)?)
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

fn decode<T: serde::de::DeserializeOwned>(data: &[u8]) -> Result<T> {
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
