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
