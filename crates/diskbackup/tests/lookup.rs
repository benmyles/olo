//! What Tracker asks: is a file's current version backed up, and what
//! versions of it were; answered by a tree reader (another thread's, while
//! a backup runs) as a repository would.

use std::{
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
};

use olo_diskbackup::{
    Key, LocalStore, Options, Progress, Repo, Store,
    backup::{backup, unchanged_since},
    exclude::Excludes,
    restore::{find, versions},
};

#[test]
fn versions_and_marks() {
    let base = PathBuf::from(format!("/var/tmp/olo-diskbackup-lookup-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let src = base.join("home");
    std::fs::create_dir_all(src.join("docs")).unwrap();
    std::fs::write(src.join("docs/a.txt"), "first").unwrap();
    std::fs::write(src.join("docs/b.txt"), "stays").unwrap();
    let store: Arc<dyn Store> = Arc::new(LocalStore::new(base.join("store")));
    let mut repo = Repo::init(store, Key::generate(), Some(base.join("cache"))).unwrap();
    let excludes = Excludes::none();
    let opts = Options { paths: vec![src.clone()], excludes: &excludes, host: "test".into(), one_file_system: true };
    let (cancel, progress) = (AtomicBool::new(false), Progress::default());
    let (id1, s1) = backup(&mut repo, &opts, None, &cancel, &progress).unwrap();
    // (A second later: the snapshots' times apart.)
    std::thread::sleep(std::time::Duration::from_millis(1100));
    std::fs::write(src.join("docs/a.txt"), "second, longer").unwrap();
    let (id2, s2) = backup(&mut repo, &opts, Some((id1, &s1)), &cancel, &progress).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let (_, s3) = backup(&mut repo, &opts, Some((id2, &s2)), &cancel, &progress).unwrap();

    let reader = repo.tree_reader();
    let snaps = repo.snapshots().unwrap();
    // Two versions of a.txt (the third snapshot kept the second's), newest first.
    let v = versions(&reader, &snaps, &src.join("docs/a.txt")).unwrap();
    assert_eq!(v.len(), 2, "{v:?}");
    assert_eq!((v[0].node.size, v[1].node.size), (14, 5));
    assert_eq!(v[0].time, s3.time);
    assert_eq!(v[0].first, s2.time);
    assert_eq!(versions(&reader, &snaps, &src.join("docs/b.txt")).unwrap().len(), 1);
    // As the repository finds them.
    let docs = src.join("docs");
    assert_eq!(find(&reader, &s3.tree, &docs).unwrap(), find(&repo, &s3.tree, &docs).unwrap());

    // Backed up as it is now; then changed, not.
    let a = find(&reader, &s3.tree, &docs.join("a.txt")).unwrap().unwrap();
    assert!(unchanged_since(&a, &docs.join("a.txt")));
    std::fs::write(docs.join("a.txt"), "third").unwrap();
    assert!(!unchanged_since(&a, &docs.join("a.txt")));
    let _ = std::fs::remove_dir_all(base);
}
