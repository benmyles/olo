//! Keep and tidy: snapshots past keeping forgotten, then what only they
//! used let go of; every snapshot kept still restores whole; tidying needs
//! the backup to itself; with nothing to do, it does nothing.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
};

use olo_diskbackup::{
    Error, Key, LocalStore, Options, Progress, Repo, Store, backup,
    exclude::Excludes,
    keep::{Policy, to_forget},
    prune,
    restore::restore,
};

fn temp(name: &str) -> PathBuf {
    let d = PathBuf::from(format!("/var/tmp/olo-diskbackup-prune-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn noise(n: usize, seed: u64) -> Vec<u8> {
    let mut x = seed;
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect()
}

fn size(store: &dyn Store) -> u64 {
    store.list("").unwrap().iter().filter(|(n, _)| !n.starts_with("locks/")).map(|(_, s)| s).sum()
}

fn same(a: &Path, b: &Path) {
    for e in std::fs::read_dir(a).unwrap() {
        let e = e.unwrap();
        let other = b.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            same(&e.path(), &other);
        } else {
            assert!(std::fs::read(e.path()).unwrap() == std::fs::read(&other).unwrap(), "{}", e.path().display());
        }
    }
}

#[test]
fn forget_then_prune() {
    let base = temp("main");
    let src = base.join("home");
    std::fs::create_dir_all(src.join("docs")).unwrap();
    std::fs::write(src.join("docs/keep.bin"), noise(20 << 20, 1)).unwrap();
    std::fs::write(src.join("docs/note.txt"), "a note").unwrap();
    std::fs::write(src.join("old.bin"), noise(40 << 20, 2)).unwrap();

    let store: Arc<dyn Store> = Arc::new(LocalStore::new(base.join("store")));
    let key = Key::generate();
    let cache = base.join("cache");
    let mut repo = Repo::init(store.clone(), key.clone(), Some(cache.clone())).unwrap();
    let excludes = Excludes::none();
    let opts = Options { paths: vec![src.clone()], excludes: &excludes, host: "test".into(), one_file_system: true };
    let (cancel, progress) = (AtomicBool::new(false), Progress::default());

    let lock = repo.lock(false).unwrap();
    let (id1, s1) = backup(&mut repo, &opts, None, &cancel, &progress).unwrap();
    // Trees are in packs of their own, kept whole in the cache.
    let cached: Vec<_> = walk(&cache);
    assert!(cached.iter().any(|p| p.contains("/data/")), "{cached:?}");

    // The big file gone; a few small backups after (small packs each).
    std::fs::remove_file(src.join("old.bin")).unwrap();
    let mut parent = (id1, s1);
    for k in 0..9 {
        std::fs::write(src.join(format!("docs/small-{k}.bin")), noise(200 << 10, 10 + k)).unwrap();
        parent = backup(&mut repo, &opts, Some((parent.0, &parent.1)), &cancel, &progress).unwrap();
    }
    drop(lock);
    let before = size(&*store);

    // Tidying needs the backup to itself.
    let shared = repo.lock(false).unwrap();
    assert!(matches!(prune(&mut repo, &cancel, &progress), Err(Error::Busy(_))));
    assert!(matches!(repo.lock(true), Err(Error::Busy(_))));
    drop(shared);

    // Keep only the newest: the first (the only one with old.bin) goes.
    // (All made in the same second here: which is newest said outright.)
    let lock = repo.lock(true).unwrap();
    let all = repo.snapshots().unwrap();
    // (Keeping lets go of none of them: they're all from the last hour.)
    let forget = to_forget(&all, Policy { hourly: 0, daily: 0, weekly: 0, monthly: 0 });
    assert!(forget.is_empty());
    for (id, _) in all.iter().filter(|(id, _)| *id != parent.0) {
        repo.forget(id).unwrap();
    }
    // A pack nothing names (a backup stopped before its index went up).
    let orphan = format!("data/00/00{}", "ab".repeat(31));
    store.put(&orphan, &noise(1000, 3)).unwrap();
    let pruned = prune(&mut repo, &cancel, &progress).unwrap();
    let after = size(&*store);
    eprintln!("{pruned:?}; {before} → {after}");
    assert_eq!(pruned.missing, 0);
    assert!(after < before - (35 << 20), "old.bin's 40 MiB let go of: {before} → {after}");
    assert_eq!(pruned.size, after);
    assert!(pruned.packs_repacked >= 8, "the small packs put together: {pruned:?}");
    assert!(store.get(&orphan).is_err());

    // The newest restores whole, from the repository opened again.
    drop(lock);
    let repo = Repo::open(store.clone(), key.clone(), Some(cache.clone())).unwrap();
    let (_, newest) = repo.snapshots().unwrap().pop().unwrap();
    let out = base.join("out");
    restore(&repo, &newest.tree, &src, &out, &cancel, &progress).unwrap();
    same(&src, &out);
    // The cache holds only what's in the store.
    for p in walk(&cache) {
        let name = p.split_once(&format!("{}/", repo.id().hex())).map(|(_, n)| n.to_owned()).unwrap_or_default();
        assert!(store.get(&name).is_ok(), "{p} isn't in the store");
    }

    // Again: nothing to do, nothing done.
    let mut repo = repo;
    let lock = repo.lock(true).unwrap();
    let again = prune(&mut repo, &cancel, &progress).unwrap();
    assert_eq!((again.packs_deleted, again.packs_repacked, again.packs_written, again.bytes_deleted), (0, 0, 0, 0), "{again:?}");
    assert_eq!(size(&*store), after);
    drop(lock);
    let _ = std::fs::remove_dir_all(base);
}

fn walk(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        if e.file_type().unwrap().is_dir() {
            out.extend(walk(&e.path()));
        } else {
            out.push(e.path().display().to_string());
        }
    }
    out
}

/// A store that stops the backup once the first pack has gone up.
struct Tripwire {
    inner: LocalStore,
    cancel: Arc<AtomicBool>,
}

impl Store for Tripwire {
    fn put(&self, name: &str, data: &[u8]) -> std::io::Result<()> {
        self.inner.put(name, data)?;
        if name.starts_with("data/") {
            self.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        Ok(())
    }
    fn get(&self, name: &str) -> std::io::Result<Vec<u8>> {
        self.inner.get(name)
    }
    fn get_range(&self, name: &str, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        self.inner.get_range(name, offset, len)
    }
    fn list(&self, dir: &str) -> std::io::Result<Vec<(String, u64)>> {
        self.inner.list(dir)
    }
    fn delete(&self, name: &str) -> std::io::Result<()> {
        self.inner.delete(name)
    }
    fn describe(&self) -> String {
        self.inner.describe()
    }
}

/// Stopped once a pack has gone up: the index of it goes up too, so the
/// next backup (another process, say) doesn't send it again.
#[test]
fn stopped_backup_goes_on() {
    let base = temp("stopped");
    let src = base.join("home");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("big.bin"), noise(40 << 20, 4)).unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    let trip: Arc<dyn Store> = Arc::new(Tripwire { inner: LocalStore::new(base.join("store")), cancel: cancel.clone() });
    let key = Key::generate();
    let mut repo = Repo::init(trip, key.clone(), None).unwrap();
    let excludes = Excludes::none();
    let opts = Options { paths: vec![src.clone()], excludes: &excludes, host: "test".into(), one_file_system: true };
    let progress = Progress::default();
    assert!(matches!(backup(&mut repo, &opts, None, &cancel, &progress), Err(Error::Cancelled)));
    drop(repo);

    let store: Arc<dyn Store> = Arc::new(LocalStore::new(base.join("store")));
    let mut repo = Repo::open(store, key, None).unwrap();
    let go = AtomicBool::new(false);
    let (_, s) = backup(&mut repo, &opts, None, &go, &progress).unwrap();
    assert!(s.summary.new_bytes <= (40 << 20) - (15 << 20), "the first pack not sent again: {:?}", s.summary);
    let out = base.join("out");
    restore(&repo, &s.tree, &src, &out, &go, &progress).unwrap();
    same(&src, &out);
    let _ = std::fs::remove_dir_all(base);
}

/// The index lost (every file of it, store and cache): a tidy reads each
/// pack's own list, keeps what the snapshot uses, names it again, and
/// lets go of nothing that's used; the backup restores whole after.
#[test]
fn index_lost_found_again() {
    let base = temp("lost");
    let src = base.join("home");
    std::fs::create_dir_all(src.join("docs")).unwrap();
    std::fs::write(src.join("docs/a.bin"), noise(30 << 20, 7)).unwrap();
    std::fs::write(src.join("docs/note.txt"), "keep me").unwrap();
    let store_dir = base.join("store");
    let store: Arc<dyn Store> = Arc::new(LocalStore::new(&store_dir));
    let key = Key::generate();
    let mut repo = Repo::init(store.clone(), key.clone(), Some(base.join("cache"))).unwrap();
    let excludes = Excludes::none();
    let opts = Options { paths: vec![src.clone()], excludes: &excludes, host: "test".into(), one_file_system: true };
    let (cancel, progress) = (AtomicBool::new(false), Progress::default());
    backup(&mut repo, &opts, None, &cancel, &progress).unwrap();
    drop(repo);
    let packs_before = store.list("data").unwrap().len();
    for (name, _) in store.list("index").unwrap() {
        store.delete(&name).unwrap();
    }
    let _ = std::fs::remove_dir_all(base.join("cache"));

    let mut repo = Repo::open(store.clone(), key.clone(), Some(base.join("cache"))).unwrap();
    assert_eq!(repo.blobs(), 0, "nothing named");
    let lock = repo.lock(true).unwrap();
    let pruned = prune(&mut repo, &cancel, &progress).unwrap();
    drop(lock);
    eprintln!("{pruned:?}");
    assert_eq!(pruned.missing, 0, "{pruned:?}");
    assert_eq!(pruned.packs_found as usize, packs_before, "{pruned:?}");
    assert_eq!(store.list("data").unwrap().len(), packs_before, "none let go of");

    let repo = Repo::open(store.clone(), key, Some(base.join("cache2"))).unwrap();
    let (_, s) = repo.snapshots().unwrap().pop().unwrap();
    let out = base.join("out");
    restore(&repo, &s.tree, &src, &out, &cancel, &progress).unwrap();
    same(&src, &out);
    let _ = std::fs::remove_dir_all(base);
}

/// The index lost, then a backup: its parent's tree not found, the packs
/// read back from their own lists, so what's unchanged isn't sent again.
#[test]
fn index_lost_then_back_up() {
    let base = temp("lost-backup");
    let src = base.join("home");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("a.bin"), noise(30 << 20, 8)).unwrap();
    let store: Arc<dyn Store> = Arc::new(LocalStore::new(base.join("store")));
    let key = Key::generate();
    let mut repo = Repo::init(store.clone(), key.clone(), None).unwrap();
    let excludes = Excludes::none();
    let opts = Options { paths: vec![src.clone()], excludes: &excludes, host: "test".into(), one_file_system: true };
    let (cancel, progress) = (AtomicBool::new(false), Progress::default());
    let (id, s) = backup(&mut repo, &opts, None, &cancel, &progress).unwrap();
    drop(repo);
    for (name, _) in store.list("index").unwrap() {
        store.delete(&name).unwrap();
    }
    let mut repo = Repo::open(store.clone(), key, None).unwrap();
    let (_, s2) = backup(&mut repo, &opts, Some((id, &s)), &cancel, &progress).unwrap();
    assert!(s2.summary.new_bytes < 1 << 20, "not sent again: {:?}", s2.summary);
    assert_eq!(s2.summary.unchanged_files, 1, "{:?}", s2.summary);
    let _ = std::fs::remove_dir_all(base);
}
