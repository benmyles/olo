//! The helper's side of a session keeping files in step (`olo-rex-link
//! sync`), spoken to over its input and output as the app speaks to it
//! over SSH: a folder sent to it and taken from it, its options, and the
//! index it keeps so a second session costs next to nothing.
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use olo_rex_sync::wire::{FolderSource, Options, receive, send};

struct Pipes {
    to: ChildStdin,
    from: ChildStdout,
}

impl Read for Pipes {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.from.read(buf)
    }
}

impl Write for Pipes {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.to.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.to.flush()
    }
}

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(format!("/var/tmp/olo-rex-link-sync-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn helper(args: &[&str], cache: &Path) -> (Child, Pipes) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_olo-rex-link")).arg("sync").args(args).env("XDG_CACHE_HOME", cache).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let pipes = Pipes { to: child.stdin.take().unwrap(), from: child.stdout.take().unwrap() };
    (child, pipes)
}

fn tree(root: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for e in fs::read_dir(&dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push((p.strip_prefix(root).unwrap().to_string_lossy().into_owned(), fs::read(&p).unwrap()));
            }
        }
    }
    out.sort();
    out
}

#[test]
fn a_folder_goes_to_the_helper_and_comes_from_it() {
    let (src, dst, back, cache) = (scratch("src"), scratch("dst"), scratch("back"), scratch("cache"));
    fs::create_dir_all(src.join("dist/bin")).unwrap();
    fs::write(src.join("dist/bin/tool"), vec![7u8; 300_000]).unwrap();
    fs::write(src.join("dist/notes.txt"), b"built\n".repeat(1000)).unwrap();
    fs::write(src.join("skip.tmp"), b"not this").unwrap();

    // To the helper, receiving, `--exclude` passed on.
    let (mut child, pipes) = helper(&["receive", dst.to_str().unwrap(), "--exclude", "*.tmp"], &cache);
    let sent = send(pipes, &FolderSource::new(&src), Options { filter: olo_rex_sync::index::Filter { exclude: vec!["*.tmp".into()], ..Default::default() }, ..Default::default() }).unwrap();
    assert!(child.wait().unwrap().success());
    assert_eq!(sent.new, 2, "{sent:?}");
    assert!(sent.errors.is_empty(), "{:?}", sent.errors);
    let mut want = tree(&src);
    want.retain(|(p, _)| p != "skip.tmp");
    assert_eq!(tree(&dst), want);

    // Again, nothing changed: the helper kept its index (in the cache
    // folder it was given), so a few dozen bytes.
    assert!(fs::read_dir(cache.join("olo/rex-sync")).unwrap().count() == 1);
    let (mut child, pipes) = helper(&["receive", dst.to_str().unwrap(), "--exclude", "*.tmp"], &cache);
    let again = send(pipes, &FolderSource::new(&src), Options { filter: olo_rex_sync::index::Filter { exclude: vec!["*.tmp".into()], ..Default::default() }, ..Default::default() }).unwrap();
    assert!(child.wait().unwrap().success());
    assert!(again.wire_bytes() < 256, "{} bytes", again.wire_bytes());

    // From the helper, sending: the same tree here.
    let (mut child, pipes) = helper(&["send", dst.to_str().unwrap()], &cache);
    let got = receive(pipes, &back, Options::default()).unwrap();
    assert!(child.wait().unwrap().success());
    assert!(got.errors.is_empty(), "{:?}", got.errors);
    assert_eq!(tree(&back), want);

    // `--delete` and `--keep-newer` are the receiver's: passed on to it.
    fs::write(dst.join("extra"), b"only there").unwrap();
    let (mut child, pipes) = helper(&["receive", dst.to_str().unwrap(), "--delete", "--no-cache", "--exclude", "*.tmp"], &cache);
    let mirrored = send(pipes, &FolderSource::new(&src), Options { filter: olo_rex_sync::index::Filter { exclude: vec!["*.tmp".into()], ..Default::default() }, ..Default::default() }).unwrap();
    assert!(child.wait().unwrap().success());
    assert_eq!(mirrored.removed, 1, "{mirrored:?}");
    assert!(!dst.join("extra").exists());

    for d in [src, dst, back, cache] {
        fs::remove_dir_all(d).unwrap();
    }
}

/// A folder named by a relative path is under the home (`$HOME`), not
/// where the helper runs: over SSH that is the account's home as the
/// system keeps it, which isn't always `$HOME` (the rig's far host).
#[test]
fn a_relative_folder_is_under_the_home() {
    let (src, home, elsewhere, cache) = (scratch("rel-src"), scratch("rel-home"), scratch("rel-elsewhere"), scratch("rel-cache"));
    fs::write(src.join("a.txt"), b"here").unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_olo-rex-link"))
        .args(["sync", "receive", "inside/it"])
        .env("HOME", &home)
        .env("XDG_CACHE_HOME", &cache)
        .current_dir(&elsewhere)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let pipes = Pipes { to: child.stdin.take().unwrap(), from: child.stdout.take().unwrap() };
    send(pipes, &FolderSource::new(&src), Options::default()).unwrap();
    assert!(child.wait().unwrap().success());
    assert_eq!(fs::read(home.join("inside/it/a.txt")).unwrap(), b"here");
    assert!(!elsewhere.join("inside").exists());
    for d in [src, home, elsewhere, cache] {
        fs::remove_dir_all(d).unwrap();
    }
}

/// A file sent by its own path: its folder, and it alone, by its name
/// exactly (a `*` in it is itself, not a pattern taking its neighbour).
#[test]
fn a_single_file_is_sent_by_its_path() {
    let (src, dst, cache) = (scratch("one-src"), scratch("one-dst"), scratch("one-cache"));
    fs::write(src.join("a*b.txt"), b"this one").unwrap();
    fs::write(src.join("axb.txt"), b"not this").unwrap();
    fs::create_dir_all(src.join("sub")).unwrap();
    fs::write(src.join("sub/c.txt"), b"nor this").unwrap();
    let (mut child, pipes) = helper(&["send", src.join("a*b.txt").to_str().unwrap()], &cache);
    let got = receive(pipes, &dst, Options { scoped: true, ..Default::default() }).unwrap();
    assert!(child.wait().unwrap().success());
    assert!(got.errors.is_empty(), "{:?}", got.errors);
    assert_eq!(tree(&dst), vec![("a*b.txt".to_owned(), b"this one".to_vec())]);
    for d in [src, dst, cache] {
        fs::remove_dir_all(d).unwrap();
    }
}

/// The watch: "watching", then "changed" once a change settles (a file
/// made, a folder made and a file in it), at most five seconds after the
/// first while changes go on, "gone" when its folder goes; it ends when
/// its input closes.
#[test]
fn a_folder_watched_says_when_it_changed() {
    use std::io::BufRead;
    use std::time::{Duration, Instant};
    let (root, cache) = (scratch("watch-root"), scratch("watch-cache"));
    let mut child = Command::new(env!("CARGO_BIN_EXE_olo-rex-link"))
        .args(["sync", "watch", root.to_str().unwrap(), "--settle", "150"])
        .env("XDG_CACHE_HOME", &cache)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let out = child.stdout.take().unwrap();
    std::thread::spawn(move || {
        for line in io::BufReader::new(out).lines().map_while(Result::ok) {
            let _ = tx.send((line, Instant::now()));
        }
    });
    let next = |within: u64| rx.recv_timeout(Duration::from_millis(within)).map(|(l, _)| l).ok();
    assert_eq!(next(3000).as_deref(), Some("watching"));
    fs::write(root.join("a.txt"), b"one").unwrap();
    assert_eq!(next(3000).as_deref(), Some("changed"));
    assert_eq!(next(400), None, "said once");
    fs::create_dir_all(root.join("new/deeper")).unwrap();
    std::thread::sleep(Duration::from_millis(50));
    fs::write(root.join("new/deeper/b.txt"), b"two").unwrap();
    assert_eq!(next(3000).as_deref(), Some("changed"));
    // Written to all the time: said within its longest wait all the same.
    let began = Instant::now();
    let mut said = None;
    while began.elapsed() < Duration::from_secs(8) && said.is_none() {
        fs::OpenOptions::new().append(true).open(root.join("a.txt")).unwrap().write_all(b".").unwrap();
        said = rx.recv_timeout(Duration::from_millis(50)).ok();
    }
    let took = began.elapsed();
    assert!(said.is_some_and(|(l, _)| l == "changed") && took < Duration::from_millis(6500), "said while it went on, after {took:?}");
    std::thread::sleep(Duration::from_millis(400));
    while rx.try_recv().is_ok() {}
    fs::remove_dir_all(&root).unwrap();
    assert_eq!(next(3000).as_deref(), Some("gone"));
    assert!(!child.wait().unwrap().success());
    // Its input closed: it ends.
    let watched = scratch("watch-again");
    let mut again = Command::new(env!("CARGO_BIN_EXE_olo-rex-link")).args(["sync", "watch", watched.to_str().unwrap()]).stdin(Stdio::piped()).stdout(Stdio::null()).spawn().unwrap();
    drop(again.stdin.take());
    let ended = Instant::now();
    while again.try_wait().unwrap().is_none() {
        assert!(ended.elapsed() < Duration::from_secs(3), "went on after its input closed");
        std::thread::sleep(Duration::from_millis(20));
    }
    for d in [cache, watched] {
        fs::remove_dir_all(d).unwrap();
    }
}

/// `index`, `mutate`, `send --only` / `--only-file`, and `watch --shallow`:
/// the pieces two-way sync builds on.
#[test]
fn index_mutate_and_only_work_together() {
    let (src, dst, cache) = (scratch("tw-src"), scratch("tw-dst"), scratch("tw-cache"));
    fs::create_dir_all(src.join("sub")).unwrap();
    fs::write(src.join("a.txt"), b"alpha").unwrap();
    fs::write(src.join("sub/b.txt"), b"beta").unwrap();
    fs::write(src.join("skip.tmp"), b"ignored").unwrap();

    // `index`: emits a valid framed index respecting --exclude and --shallow.
    let out = Command::new(env!("CARGO_BIN_EXE_olo-rex-link")).args(["sync", "index", src.to_str().unwrap(), "--exclude", "*.tmp"]).env("XDG_CACHE_HOME", &cache).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let idx = olo_rex_sync::twoway::decode_index_frame(&out.stdout[..]).unwrap();
    assert!(idx.entries.contains_key("a.txt"));
    assert!(idx.entries.contains_key("sub/b.txt"));
    assert!(!idx.entries.contains_key("skip.tmp"));

    let out_shallow = Command::new(env!("CARGO_BIN_EXE_olo-rex-link")).args(["sync", "index", src.to_str().unwrap(), "--shallow", "--no-cache"]).env("XDG_CACHE_HOME", &cache).output().unwrap();
    assert!(out_shallow.status.success());
    let idx_s = olo_rex_sync::twoway::decode_index_frame(&out_shallow.stdout[..]).unwrap();
    assert!(idx_s.entries.contains_key("a.txt"));
    assert!(!idx_s.entries.contains_key("sub/b.txt"));

    // `mutate`: copies a file to a conflict name, removes a file, creates a dir.
    let mut child = Command::new(env!("CARGO_BIN_EXE_olo-rex-link")).args(["sync", "mutate", src.to_str().unwrap()]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(br#"{"copy":[["a.txt","a.conflict-host-20261125-100000.txt"]],"remove":["skip.tmp"],"mkdir":[["fresh/dir",493]]}"#).unwrap();
    let res = child.wait_with_output().unwrap();
    assert!(res.status.success(), "{}", String::from_utf8_lossy(&res.stderr));
    assert_eq!(fs::read(src.join("a.conflict-host-20261125-100000.txt")).unwrap(), b"alpha");
    assert!(!src.join("skip.tmp").exists());
    assert!(src.join("fresh/dir").is_dir());

    // `send --only-file`: pulls only the named paths and unlinks the list file.
    let only_file = cache.join("only.list");
    fs::write(&only_file, b"sub/b.txt\0a.conflict-host-20261125-100000.txt\0").unwrap();
    let (mut child, pipes) = helper(&["send", src.to_str().unwrap(), "--no-cache", "--only-file", only_file.to_str().unwrap()], &cache);
    let got = receive(pipes, &dst, Options { scoped: true, ..Default::default() }).unwrap();
    assert!(child.wait().unwrap().success());
    assert!(got.errors.is_empty(), "{:?}", got.errors);
    assert!(!only_file.exists(), "--only-file should be unlinked");
    assert_eq!(tree(&dst), vec![("a.conflict-host-20261125-100000.txt".to_owned(), b"alpha".to_vec()), ("sub/b.txt".to_owned(), b"beta".to_vec())]);

    for d in [src, dst, cache] {
        fs::remove_dir_all(d).unwrap();
    }
}

/// A mistake in what it is asked is said, never a session begun.
#[test]
fn what_it_isnt_asked_right_it_says() {
    let cache = scratch("usage");
    for args in [vec!["receive"], vec!["copy", "x"], vec!["receive", "x", "--bogus"]] {
        let out = Command::new(env!("CARGO_BIN_EXE_olo-rex-link")).arg("sync").args(&args).env("XDG_CACHE_HOME", &cache).stdin(Stdio::null()).output().unwrap();
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(out.stdout.is_empty(), "{args:?}: the session's output holds nothing else");
        assert!(String::from_utf8_lossy(&out.stderr).contains("usage:"), "{args:?}");
    }
    fs::remove_dir_all(cache).unwrap();
}
