//! `olo-rex-link sync receive|send ROOT [OPTION…]`: the host's side of a
//! session of the engine that keeps files in step (`olo-rex-sync`; HACKING,
//! Rex, "Files kept in step"), over the helper's own input and output, the
//! app the other side over SSH (`olo_rex_client::sync`). Nothing else is
//! written on its output: it is the session's.
//!
//! ROOT is the folder on this host, under the home (`$HOME`) unless whole
//! (sending, it may be a file: then its folder, and it alone):
//! not where it runs, which over SSH is the account's home as the system
//! keeps it, not always `$HOME` (the rig's far host: a session copied
//! there into `.` went into the real home, 2026-11-24). Options:
//! `--delete` (receiving: what the
//! sender hasn't is taken away, a mirror), `--keep-newer` (receiving: a
//! file newer here is left), `--exclude PATTERN` (again for more; from
//! the root only when it starts with `/`), `--protect PATH` (receiving:
//! never written, replaced or taken away, nor what is under it: a moved
//! home's own keys),
//! `--shallow` (the folder's own files alone), `--scoped` (receiving:
//! only the files the sender names are looked at, nothing taken away: a
//! root that is the whole home), `--no-cache` (no index
//! kept between sessions: by default one is, under
//! `~/.cache/olo/rex-sync/`, so an unchanged tree is read for nothing).
//! Status 0 when the session ended (what failed of its files the other
//! side has in its report), 1 when it didn't, 2 for a mistake in what it
//! was asked.

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use olo_rex_sync::index::Filter;
use olo_rex_sync::wire::{FolderSource, Options, receive, send};

const USAGE: &str = "usage: olo-rex-link sync watch ROOT [--exclude PATTERN]… [--settle MS]\n       olo-rex-link sync receive|send ROOT [--delete] [--keep-newer] [--exclude PATTERN]… [--protect PATH]… [--shallow] [--scoped] [--no-cache]";

/// The helper's input and output, as the session's one stream.
struct Stdio {
    input: io::Stdin,
    output: io::Stdout,
}

impl Read for Stdio {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.input.read(buf)
    }
}

impl Write for Stdio {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.output.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.output.flush()
    }
}

/// Where the index of `root` is kept between sessions: one file a folder,
/// named by its whole path.
fn cache_for(root: &Path) -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    let whole = std::path::absolute(root).ok()?;
    let name: String = olo_rex_sync::chunk::chunk_id(whole.as_os_str().as_encoded_bytes()).iter().map(|b| format!("{b:02x}")).collect();
    Some(base.join("olo/rex-sync").join(format!("{name}.idx")))
}

pub fn run(args: Vec<String>) -> ! {
    let fail = |why: &str| -> ! {
        eprintln!("olo-rex-link sync: {why}\n{USAGE}");
        std::process::exit(2)
    };
    // (`watch`: a folder's changes said as they settle, `crate::watch`.)
    if args.first().is_some_and(|a| a == "watch") {
        crate::watch::run(args[1..].to_vec());
    }
    let mut it = args.into_iter();
    let (Some(side), Some(root)) = (it.next(), it.next()) else { fail("say which side and its folder") };
    let root = PathBuf::from(root);
    let root = match std::env::var_os("HOME") {
        Some(home) if root.is_relative() => PathBuf::from(home).join(root),
        _ => root,
    };
    let (mut options, mut filter, mut cached) = (Options::default(), Filter::default(), true);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--delete" => options.delete_extra = true,
            "--keep-newer" => options.keep_newer = true,
            "--shallow" => filter.recursive = false,
            "--scoped" => options.scoped = true,
            "--no-cache" => cached = false,
            "--exclude" => filter.exclude.push(it.next().unwrap_or_else(|| fail("--exclude takes a pattern"))),
            "--protect" => options.protect.push(it.next().unwrap_or_else(|| fail("--protect takes a path"))),
            other => fail(&format!("{other} isn't one of its options")),
        }
    }
    // (A file sent: its folder, and it alone, by its name exactly.)
    let root = match (side.as_str(), root.file_name().map(|n| n.to_string_lossy().into_owned())) {
        ("send", Some(name)) if std::fs::symlink_metadata(&root).is_ok_and(|m| m.is_file()) => {
            filter.only = vec![name];
            root.parent().map(Path::to_path_buf).unwrap_or(root)
        }
        _ => root,
    };
    options.filter = filter.clone();
    let cache = if cached { cache_for(&root) } else { None };
    let conn = Stdio { input: io::stdin(), output: io::stdout() };
    let ended = match side.as_str() {
        "receive" => receive(conn, &root, Options { cache_path: cache, ..options }).map(|_| ()),
        "send" => send(conn, &FolderSource { root: root.clone(), filter, cache_path: cache }, options).map(|_| ()),
        other => fail(&format!("{other}: receive or send")),
    };
    match ended {
        Ok(()) => std::process::exit(0),
        Err(e) => {
            eprintln!("olo-rex-link sync: {}: {e}", root.display());
            std::process::exit(1)
        }
    }
}
