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

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let (f, t) = (e.path(), to.join(e.file_name()));
        if e.file_type().unwrap().is_dir() {
            copy_dir(&f, &t)
        } else {
            std::fs::copy(&f, &t).unwrap();
        }
    }
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

/// `r` with packs of 16 MiB, as they were (these tests want several in a
/// backup of tens of MiB).
fn small(mut r: Repo) -> Repo {
    r.set_pack_sizes(16 << 20, 16 << 20);
    r
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
    let mut repo = small(Repo::init(trip, key.clone(), None).unwrap());
    let excludes = Excludes::none();
    let opts = Options { paths: vec![src.clone()], excludes: &excludes, host: "test".into(), one_file_system: true };
    let progress = Progress::default();
    assert!(matches!(backup(&mut repo, &opts, None, &cancel, &progress), Err(Error::Cancelled)));
    drop(repo);

    let store: Arc<dyn Store> = Arc::new(LocalStore::new(base.join("store")));
    let mut repo = small(Repo::open(store, key, None).unwrap());
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

/// An index file damaged in the store (one that doesn't open, and one
/// that opens but doesn't read): the backup still opens and restores,
/// the packs it named read back from their own lists; the next backup
/// names them again (nothing sent again), and tidying lets go of the
/// damaged files with nothing lost.
#[test]
fn index_damaged_healed() {
    let base = temp("damaged");
    let src = base.join("home");
    std::fs::create_dir_all(src.join("docs")).unwrap();
    std::fs::write(src.join("docs/a.bin"), noise(30 << 20, 9)).unwrap();
    std::fs::write(src.join("docs/note.txt"), "keep me").unwrap();
    let store: Arc<dyn Store> = Arc::new(LocalStore::new(base.join("store")));
    let key = Key::generate();
    let mut repo = Repo::init(store.clone(), key.clone(), Some(base.join("cache"))).unwrap();
    let excludes = Excludes::none();
    let opts = Options { paths: vec![src.clone()], excludes: &excludes, host: "test".into(), one_file_system: true };
    let (cancel, progress) = (AtomicBool::new(false), Progress::default());
    let (id, s) = backup(&mut repo, &opts, None, &cancel, &progress).unwrap();
    drop(repo);
    let blobs = Repo::open(store.clone(), key.clone(), None).unwrap().blobs();
    // Every index file damaged: the first's bytes flipped, the others
    // sealed but not an index.
    let index = store.list("index").unwrap();
    assert!(!index.is_empty());
    for (k, (name, _)) in index.iter().enumerate() {
        let mut bytes = store.get(name).unwrap();
        if k == 0 {
            let n = bytes.len() / 2;
            bytes[n] ^= 0x55;
        } else {
            bytes = key.seal(b"not an index");
        }
        store.put(name, &bytes).unwrap();
    }
    if index.len() == 1 {
        // (The second kind too.)
        store.put("index/0000000000000000000000000000000000000000000000000000000000000000", &key.seal(b"not an index")).unwrap();
    }
    let damaged = store.list("index").unwrap().len();

    // A cache's good copy is read in place of the store's damaged one.
    assert_eq!(Repo::open(store.clone(), key.clone(), Some(base.join("cache"))).unwrap().damaged_index_files(), damaged - 1);
    // Without: it opens (it used to fail), with everything there, restored
    // whole.
    let mut repo = Repo::open(store.clone(), key.clone(), None).unwrap();
    assert_eq!(repo.damaged_index_files(), damaged);
    assert_eq!(repo.blobs(), blobs, "every blob found again");
    let out = base.join("out");
    restore(&repo, &s.tree, &src, &out, &cancel, &progress).unwrap();
    same(&src, &out);

    // The next backup sends nothing again, and names the packs.
    let lock = repo.lock(false).unwrap();
    let (_, s2) = backup(&mut repo, &opts, Some((id, &s)), &cancel, &progress).unwrap();
    drop(lock);
    assert!(s2.summary.new_bytes < 1 << 20, "not sent again: {:?}", s2.summary);
    drop(repo);
    // (A copy of the store without the damaged files: the good index files
    // name everything.)
    let check = base.join("store-check");
    copy_dir(&base.join("store"), &check);
    // (Those still damaged: one written again may have the damaged one's
    // name, an index file being named by what it holds.)
    let still_damaged = |dir: &Path| -> Vec<PathBuf> {
        std::fs::read_dir(dir.join("index")).unwrap().flatten().map(|e| e.path()).filter(|p| key.open(&std::fs::read(p).unwrap()).is_none_or(|plain| plain == b"not an index")).collect()
    };
    for p in still_damaged(&check) {
        std::fs::remove_file(p).unwrap();
    }
    let repo = Repo::open(Arc::new(LocalStore::new(&check)), key.clone(), None).unwrap();
    assert_eq!(repo.damaged_index_files(), 0);
    assert!(repo.blobs() >= blobs, "named again in an index file: {} of {blobs}", repo.blobs());
    drop(repo);

    // Tidying lets go of the damaged files, and nothing else.
    let packs = store.list("data").unwrap().len();
    let still = still_damaged(&base.join("store")).len();
    assert!(still >= damaged - 1);
    let mut repo = Repo::open(store.clone(), key.clone(), None).unwrap();
    let lock = repo.lock(true).unwrap();
    let pruned = prune(&mut repo, &cancel, &progress).unwrap();
    drop(lock);
    assert_eq!(pruned.index_files_damaged as usize, still, "{pruned:?}");
    assert!(still_damaged(&base.join("store")).is_empty(), "let go of");
    assert_eq!(pruned.missing, 0, "{pruned:?}");
    assert_eq!(store.list("data").unwrap().len(), packs, "no pack let go of: {pruned:?}");
    let repo = Repo::open(store.clone(), key, Some(base.join("cache2"))).unwrap();
    assert_eq!(repo.damaged_index_files(), 0);
    let (_, s) = repo.snapshots().unwrap().pop().unwrap();
    let out = base.join("out2");
    restore(&repo, &s.tree, &src, &out, &cancel, &progress).unwrap();
    same(&src, &out);
    let _ = std::fs::remove_dir_all(base);
}

/// Stopped part way: what it got through kept as an unfinished snapshot
/// (no backup to restore from), which tidying keeps whole; the next backup
/// goes on from it (the files it holds not read again), and once that one
/// finishes, the unfinished one goes.
#[test]
fn stopped_backup_kept_and_resumed() {
    let base = temp("unfinished");
    let src = base.join("home");
    std::fs::create_dir_all(src.join("a")).unwrap();
    std::fs::create_dir_all(src.join("b")).unwrap();
    for i in 0..10 {
        std::fs::write(src.join(format!("a/{i}.bin")), noise(2 << 20, 100 + i)).unwrap();
    }
    std::fs::write(src.join("b/big.bin"), noise(24 << 20, 7)).unwrap();
    std::fs::write(src.join("c.txt"), "the last").unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    let trip: Arc<dyn Store> = Arc::new(Tripwire { inner: LocalStore::new(base.join("store")), cancel: cancel.clone() });
    let key = Key::generate();
    let mut repo = small(Repo::init(trip, key.clone(), Some(base.join("cache"))).unwrap());
    let excludes = Excludes::none();
    let opts = Options { paths: vec![src.clone()], excludes: &excludes, host: "test".into(), one_file_system: true };
    let progress = Progress::default();
    assert!(matches!(backup(&mut repo, &opts, None, &cancel, &progress), Err(Error::Cancelled)));
    drop(repo);

    let store: Arc<dyn Store> = Arc::new(LocalStore::new(base.join("store")));
    let mut repo = small(Repo::open(store.clone(), key.clone(), Some(base.join("cache"))).unwrap());
    assert!(repo.snapshots().unwrap().is_empty(), "no backup to restore from");
    let unfinished = repo.unfinished().unwrap();
    assert_eq!(unfinished.len(), 1);
    let (_, u) = &unfinished[0];
    let done = u.summary.files;
    assert!((2..10).contains(&done), "part of a/ done: {:?}", u.summary);
    assert!(u.summary.stored_bytes >= 16 << 20, "{:?}", u.summary);
    // Everything it sent is its own: nothing for tidying to free.
    assert_eq!(olo_diskbackup::unused(&repo, &AtomicBool::new(false)).unwrap(), 0);
    let before = size(&*store);
    let lock = repo.lock(true).unwrap();
    let go = AtomicBool::new(false);
    prune(&mut repo, &go, &progress).unwrap();
    drop(lock);
    assert!(size(&*store) >= before - (1 << 20), "kept: {} of {before}", size(&*store));

    // Gone on from: what it holds isn't read again.
    let mut repo = small(Repo::open(store.clone(), key.clone(), Some(base.join("cache"))).unwrap());
    let progress = Progress::default();
    let (_, s) = backup(&mut repo, &opts, None, &go, &progress).unwrap();
    assert!(progress.resuming.load(std::sync::atomic::Ordering::Relaxed));
    let read_again = progress.read.load(std::sync::atomic::Ordering::Relaxed);
    assert_eq!(read_again, 12 - done, "only what it hadn't got to");
    assert!(progress.new_bytes.load(std::sync::atomic::Ordering::Relaxed) < (44 << 20) - u.summary.stored_bytes + (1 << 20), "and nothing it sent, sent again");
    assert_eq!((s.summary.files, s.summary.new_files, s.summary.unchanged_files), (12, 12, 0), "{:?}", s.summary);
    assert!(s.summary.stored_bytes >= u.summary.stored_bytes, "its own and the unfinished one's: {:?}", s.summary);
    assert!(repo.unfinished().unwrap().is_empty(), "finished: the unfinished one goes");
    let out = base.join("out");
    restore(&repo, &s.tree, &src, &out, &go, &progress).unwrap();
    same(&src, &out);
    let _ = std::fs::remove_dir_all(base);
}

/// Checked: whole after the backup; a pack damaged in the store is found
/// reading everything back (fast, only if it's the sample: a folder keeps
/// no checksums); one gone is found either way. Repaired
/// (tidying salvages what's whole and lets go of the rest), the next
/// backup sends again what was lost, and the earlier backup is whole again
/// too (what it needs is the same, by content).
#[test]
fn checked_damaged_repaired() {
    use olo_diskbackup::{check, repair};
    let base = temp("check");
    let src = base.join("home");
    std::fs::create_dir_all(src.join("docs")).unwrap();
    std::fs::write(src.join("docs/a.bin"), noise(20 << 20, 11)).unwrap();
    std::fs::write(src.join("docs/b.bin"), noise(20 << 20, 12)).unwrap();
    std::fs::write(src.join("note.txt"), "a note").unwrap();
    let store_dir = base.join("store");
    let store: Arc<dyn Store> = Arc::new(LocalStore::new(&store_dir));
    let key = Key::generate();
    let mut repo = small(Repo::init(store.clone(), key.clone(), Some(base.join("cache"))).unwrap());
    let excludes = Excludes::none();
    let opts = Options { paths: vec![src.clone()], excludes: &excludes, host: "test".into(), one_file_system: true };
    let (cancel, progress) = (AtomicBool::new(false), Progress::default());
    let (_, s1) = backup(&mut repo, &opts, None, &cancel, &progress).unwrap();
    let c = check(&repo, &s1, false, &cancel, &progress).unwrap();
    assert!(c.problems.is_empty() && c.packs_read >= 1, "{c:?}");
    let c = check(&repo, &s1, true, &cancel, &progress).unwrap();
    assert!(c.problems.is_empty() && c.packs_read == c.packs && c.bytes_read >= 40 << 20, "{c:?}");

    // One data pack damaged, another gone.
    let mut data: Vec<(String, u64)> = store.list("data").unwrap();
    data.sort_by_key(|(_, size)| std::cmp::Reverse(*size));
    let (hurt, gone) = (store_dir.join(&data[0].0), store_dir.join(&data[1].0));
    let mut bytes = std::fs::read(&hurt).unwrap();
    bytes[1000] ^= 0x55;
    std::fs::write(&hurt, bytes).unwrap();
    std::fs::remove_file(&gone).unwrap();
    // (Fast: the gone one, and the damaged one only if it's the sample.)
    let fast = check(&repo, &s1, false, &cancel, &progress).unwrap();
    assert!(fast.problems.iter().any(|p| p.contains("missing")) && (1..=2).contains(&fast.bad_packs.len()) && fast.sampled == 1, "{fast:?}");
    let full = check(&repo, &s1, true, &cancel, &progress).unwrap();
    assert_eq!(full.bad_packs.len(), 2, "{full:?}");
    assert!(full.problems.iter().any(|p| p.contains("missing")) && full.problems.iter().any(|p| p.contains("damaged")), "{full:?}");
    assert!(full.files.iter().any(|f| f.ends_with(".bin")) && full.files_affected >= 1, "{full:?}");

    // Repaired, then backed up again: whole, the earlier one too.
    let lock = repo.lock(true).unwrap();
    let bad: std::collections::HashSet<_> = full.bad_packs.iter().copied().collect();
    repair(&mut repo, &bad, &cancel, &progress).unwrap();
    drop(lock);
    assert!(!hurt.exists(), "the damaged pack let go of");
    let mut repo = small(Repo::open(store.clone(), key.clone(), Some(base.join("cache"))).unwrap());
    let (id1, _) = repo.snapshots().unwrap().pop().unwrap();
    let (_, s2) = backup(&mut repo, &opts, Some((id1, &s1)), &cancel, &progress).unwrap();
    assert!(s2.summary.new_bytes > 0, "what was lost, sent again: {:?}", s2.summary);
    for s in [&s1, &s2] {
        let c = check(&repo, s, true, &cancel, &progress).unwrap();
        assert!(c.problems.is_empty(), "{c:?}");
    }
    let out = base.join("out");
    restore(&repo, &s1.tree, &src, &out, &cancel, &progress).unwrap();
    same(&src, &out);
    let _ = std::fs::remove_dir_all(base);
}
