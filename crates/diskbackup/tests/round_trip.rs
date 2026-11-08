//! Back up, change, back up again, restore both: what comes back is what
//! was there; the second backup reads only what changed and keeps only
//! what's new; what's left out stays out; nothing in the store is readable.

use std::{
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
};

use olo_diskbackup::{
    Key, LocalStore, Options, Progress, Repo, Store, backup,
    exclude::{Excludes, default_groups},
    restore::{find, restore},
};

fn temp(name: &str) -> PathBuf {
    let d = PathBuf::from(format!("/var/tmp/olo-diskbackup-rt-{}-{name}", std::process::id()));
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

/// Every file, folder and link under `dir`: its path, kind, mode, mtime
/// and content (or target).
fn listing(dir: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    fn walk(root: &Path, at: &Path, out: &mut Vec<(String, String)>) {
        let mut names: Vec<_> = std::fs::read_dir(at).unwrap().map(|e| e.unwrap().path()).collect();
        names.sort();
        for p in names {
            let md = std::fs::symlink_metadata(&p).unwrap();
            let rel = p.strip_prefix(root).unwrap().display().to_string();
            if md.file_type().is_symlink() {
                out.push((rel, format!("link {}", std::fs::read_link(&p).unwrap().display())));
            } else if md.is_dir() {
                out.push((rel.clone(), format!("dir {:o}", md.mode() & 0o7777)));
                walk(root, &p, out);
            } else {
                let data = std::fs::read(&p).unwrap();
                out.push((rel, format!("file {:o} {} {:x}", md.mode() & 0o7777, md.mtime(), data.iter().fold(0u64, |h, b| h.wrapping_mul(31).wrapping_add(*b as u64)))));
            }
        }
    }
    walk(dir, dir, &mut out);
    out
}

#[test]
fn back_up_change_back_up_restore_both() {
    let base = temp("main");
    let src = base.join("home");
    std::fs::create_dir_all(src.join("docs/empty")).unwrap();
    std::fs::create_dir_all(src.join("proj/node_modules/pkg")).unwrap();
    std::fs::create_dir_all(src.join("proj/build-cache")).unwrap();
    std::fs::write(src.join("docs/note.txt"), "SECRET-MARKER a note to keep").unwrap();
    std::fs::write(src.join("docs/same-a.bin"), noise(3 << 20, 5)).unwrap();
    std::fs::write(src.join("docs/same-b.bin"), noise(3 << 20, 5)).unwrap();
    std::fs::write(src.join("big.bin"), noise(12 << 20, 9)).unwrap();
    std::fs::write(src.join("script.sh"), "#!/bin/sh\necho hi\n").unwrap();
    std::fs::set_permissions(src.join("script.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();
    std::os::unix::fs::symlink("docs/note.txt", src.join("link")).unwrap();
    std::fs::write(src.join("proj/node_modules/pkg/index.js"), "module.exports = 1").unwrap();
    std::fs::write(src.join("proj/build-cache/CACHEDIR.TAG"), "Signature: 8a477f597d28d172789f06886806bc55\n").unwrap();
    std::fs::write(src.join("proj/build-cache/junk"), "junk").unwrap();
    std::fs::write(src.join("proj/main.rs"), "fn main() {}").unwrap();

    let store_dir = base.join("store");
    let store: Arc<dyn Store> = Arc::new(LocalStore::new(&store_dir));
    let key = Key::generate();
    let mut repo = Repo::init(store.clone(), key.clone(), Some(base.join("cache"))).unwrap();
    let excludes = Excludes::new(&default_groups(), &[], &src);
    let opts = Options { paths: vec![src.clone()], excludes: &excludes, host: "test".into(), one_file_system: true };
    let (cancel, progress) = (AtomicBool::new(false), Progress::default());

    // The first: everything read; the twin files kept once.
    let (id1, s1) = backup(&mut repo, &opts, None, &cancel, &progress).unwrap();
    let total = (3u64 << 20) * 2 + (12 << 20);
    assert!(s1.summary.bytes >= total, "{:?}", s1.summary);
    assert!(s1.summary.new_bytes < total - (2 << 20), "the twin kept once: {:?}", s1.summary);
    assert!(s1.summary.errors.is_empty(), "{:?}", s1.summary.errors);
    // Left out: node_modules (a group), a folder tagged as a cache.
    assert!(find(&repo, &s1.tree, &src.join("proj/node_modules")).unwrap().is_none());
    assert!(find(&repo, &s1.tree, &src.join("proj/build-cache")).unwrap().is_none());
    assert!(find(&repo, &s1.tree, &src.join("proj/main.rs")).unwrap().is_some());

    // Restored whole: as it was (what was left out, left out).
    let before = listing(&src);
    let r1 = base.join("r1");
    restore(&repo, &s1.tree, &src, &r1, &cancel, &progress).unwrap();
    let kept: Vec<_> = before.iter().filter(|(p, _)| !p.starts_with("proj/node_modules") && !p.starts_with("proj/build-cache")).cloned().collect();
    assert_eq!(listing(&r1), kept);

    // Changed: bytes typed into the middle of the big file, one new, one gone.
    let mut big = std::fs::read(src.join("big.bin")).unwrap();
    big.splice(6 << 20..6 << 20, b"inserted".iter().copied());
    std::fs::write(src.join("big.bin"), &big).unwrap();
    std::fs::write(src.join("docs/new.txt"), "new").unwrap();
    std::fs::remove_file(src.join("docs/same-b.bin")).unwrap();

    // The second, from the first: only what changed read, only what's new kept.
    let mut repo = Repo::open(store.clone(), key.clone(), Some(base.join("cache"))).unwrap();
    let (_, s2) = backup(&mut repo, &opts, Some((id1, &s1)), &cancel, &progress).unwrap();
    assert_eq!((s2.summary.changed_files, s2.summary.new_files), (1, 1), "{:?}", s2.summary);
    assert!(s2.summary.unchanged_files >= 4, "{:?}", s2.summary);
    assert!(s2.summary.new_bytes < 4 << 20, "an edit's chunks only: {:?}", s2.summary);
    assert_eq!(repo.snapshots().unwrap().len(), 2);

    // Both versions of the big file come back.
    let old = base.join("old.bin");
    restore(&repo, &s1.tree, &src.join("big.bin"), &old, &cancel, &progress).unwrap();
    assert_eq!(std::fs::read(&old).unwrap(), noise(12 << 20, 9));
    let new = base.join("new.bin");
    restore(&repo, &s2.tree, &src.join("big.bin"), &new, &cancel, &progress).unwrap();
    assert_eq!(std::fs::read(&new).unwrap(), big);

    // Nothing readable in the store.
    for (name, _) in store.list("").unwrap() {
        let data = store.get(&name).unwrap();
        assert!(!data.windows(13).any(|w| w == b"SECRET-MARKER"), "{name}");
        assert!(!data.windows(7).any(|w| w == b"note.tx"), "names too: {name}");
    }

    // Stopped: said so.
    let stop = AtomicBool::new(true);
    assert!(matches!(backup(&mut repo, &opts, None, &stop, &progress), Err(olo_diskbackup::Error::Cancelled)));
    let _ = std::fs::remove_dir_all(base);
}

/// How fast, on a real folder (`OLO_DB_SRC`, default this repository's
/// crates/): a first backup, then one with nothing changed.
/// `cargo test --release -p olo-diskbackup --test round_trip throughput -- --ignored --nocapture`
#[test]
#[ignore]
fn throughput() {
    let src = std::env::var("OLO_DB_SRC").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/..")).canonicalize().unwrap());
    let base = temp("speed");
    let store: Arc<dyn Store> = Arc::new(LocalStore::new(base.join("store")));
    let mut repo = Repo::init(store, Key::generate(), Some(base.join("cache"))).unwrap();
    let home = std::env::var("HOME").map(PathBuf::from).unwrap();
    let excludes = Excludes::new(&default_groups(), &[], &home);
    let opts = Options { paths: vec![src.clone()], excludes: &excludes, host: "test".into(), one_file_system: true };
    let (cancel, progress) = (AtomicBool::new(false), Progress::default());
    let t = std::time::Instant::now();
    let (id, s1) = backup(&mut repo, &opts, None, &cancel, &progress).unwrap();
    let first = t.elapsed().as_secs_f64();
    let t = std::time::Instant::now();
    let (_, s2) = backup(&mut repo, &opts, Some((id, &s1)), &cancel, &progress).unwrap();
    let second = t.elapsed().as_secs_f64();
    let mb = |b: u64| b as f64 / 1e6;
    eprintln!(
        "{}: {} files, {} folders, {:.0} MB; first {first:.2} s ({:.0} MB/s), {:.0} MB new, {:.0} MB stored; again unchanged {second:.2} s ({} unchanged, {:.1} MB stored)",
        src.display(),
        s1.summary.files,
        s1.summary.dirs,
        mb(s1.summary.bytes),
        mb(s1.summary.bytes) / first,
        mb(s1.summary.new_bytes),
        mb(s1.summary.stored_bytes),
        s2.summary.unchanged_files,
        mb(s2.summary.stored_bytes)
    );
    let _ = std::fs::remove_dir_all(base);
}
