//! `olo-rex-link sync receive|send|index|mutate|watch ROOT [OPTION…]`: the
//! host's side of a session of the engine that keeps files in step
//! (`olo-rex-sync`; HACKING, Rex, "Files kept in step"), over the helper's
//! own input and output, the app the other side over SSH
//! (`olo_rex_client::sync`). Nothing else is written on its output: it is the
//! session's.
//!
//! ROOT is the folder on this host, under the home (`$HOME`) unless whole
//! (sending, it may be a file: then its folder, and it alone): not where it
//! runs, which over SSH is the account's home as the system keeps it, not
//! always `$HOME`.
//! Status 0 when the session ended, 1 when it didn't, 2 for a mistake in what
//! it was asked.

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use olo_rex_sync::index::{Filter, Index};
use olo_rex_sync::twoway::{apply_mutations, encode_index_frame};
use olo_rex_sync::wire::{FolderSource, Options, receive, send};
use serde::Deserialize;

const USAGE: &str = "\
usage: olo-rex-link sync watch ROOT [--shallow] [--exclude PATTERN]… [--settle MS]
       olo-rex-link sync index ROOT [--create] [--shallow] [--exclude PATTERN]… [--no-cache] [--cache FILE]
       olo-rex-link sync mutate ROOT
       olo-rex-link sync receive|send ROOT [--delete] [--keep-newer] [--exclude PATTERN]… [--protect PATH]… [--only PATH]… [--only-file FILE] [--shallow] [--scoped] [--no-cache] [--cache FILE]";

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

/// Resolve `~`, `~/…`, or relative sync roots against `$HOME` WITHOUT lexically
/// popping `..` (so kernel symlink resolution for `link/../docs` is preserved)
/// and failing explicitly if `$HOME` is unset rather than defaulting to `/`.
pub(crate) fn resolve_root(raw: &str) -> io::Result<PathBuf> {
    let need_home = || -> io::Result<PathBuf> {
        match std::env::var_os("HOME") {
            Some(h) if !h.is_empty() => Ok(PathBuf::from(h)),
            _ => Err(io::Error::new(io::ErrorKind::NotFound, "HOME is not set")),
        }
    };
    if raw == "~" {
        return need_home();
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        return Ok(need_home()?.join(rest));
    }
    let p = PathBuf::from(raw);
    if p.is_relative() {
        return Ok(need_home()?.join(p));
    }
    Ok(p)
}

/// Where the index of `root` is kept between sessions: one file a folder,
/// named by its whole path.
fn cache_for(root: &Path) -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    let whole = std::path::absolute(root).ok()?;
    let name: String = olo_rex_sync::chunk::chunk_id(whole.as_os_str().as_encoded_bytes()).iter().map(|b| format!("{b:02x}")).collect();
    Some(base.join("olo/rex-sync").join(format!("{name}.idx")))
}

/// `olo-rex-link sync index ROOT [--create] [--shallow] [--exclude PATTERN]… [--no-cache] [--cache FILE]`
fn run_index(args: Vec<String>, fail: &impl Fn(&str) -> !) -> ! {
    let mut it = args.into_iter();
    let Some(raw_root) = it.next() else { fail("say which folder") };
    let root = match resolve_root(&raw_root) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("olo-rex-link sync index: {raw_root}: {e}");
            std::process::exit(1);
        }
    };
    let (mut filter, mut cached, mut cache_file, mut create) = (Filter::default(), true, None, false);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--create" => create = true,
            "--shallow" => filter.recursive = false,
            "--no-cache" => cached = false,
            "--cache" => cache_file = Some(PathBuf::from(it.next().unwrap_or_else(|| fail("--cache takes a file")))),
            "--exclude" => filter.exclude.push(it.next().unwrap_or_else(|| fail("--exclude takes a pattern"))),
            other => fail(&format!("{other} isn't one of its options")),
        }
    }
    if create && let Err(e) = std::fs::create_dir_all(&root) {
        eprintln!("olo-rex-link sync index: {}: {e}", root.display());
        std::process::exit(1);
    }
    let cache = if !cached { None } else { cache_file.or_else(|| cache_for(&root)) };
    let prev = cache.as_deref().and_then(|p| Index::load(p).ok());
    let index = match Index::scan(&root, &filter, prev.as_ref()) {
        Ok(idx) => idx,
        Err(e) => {
            eprintln!("olo-rex-link sync index: {}: {e}", root.display());
            std::process::exit(1);
        }
    };
    if let Some(c) = &cache {
        let _ = index.save(c);
    }
    let out = io::BufWriter::new(io::stdout().lock());
    match encode_index_frame(&index, out) {
        Ok(()) => std::process::exit(0),
        Err(e) => {
            eprintln!("olo-rex-link sync index: {}: {e}", root.display());
            std::process::exit(1)
        }
    }
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct MutateRequest {
    copy: Vec<(String, String)>,
    remove: Vec<String>,
    mkdir: Vec<(String, u32)>,
}

/// `olo-rex-link sync mutate ROOT`: reads JSON `MutateRequest` on stdin and
/// applies copies, removes and mkdirs confined strictly under `ROOT`.
fn run_mutate(args: Vec<String>, fail: &impl Fn(&str) -> !) -> ! {
    let mut it = args.into_iter();
    let Some(raw_root) = it.next() else { fail("say which folder") };
    if let Some(other) = it.next() {
        fail(&format!("{other} isn't one of its options"));
    }
    let root = match resolve_root(&raw_root) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("olo-rex-link sync mutate: {raw_root}: {e}");
            std::process::exit(1);
        }
    };
    let mut body = Vec::new();
    if let Err(e) = io::stdin().read_to_end(&mut body) {
        eprintln!("olo-rex-link sync mutate: read stdin: {e}");
        std::process::exit(1);
    }
    let req: MutateRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("olo-rex-link sync mutate: invalid request: {e}");
            std::process::exit(2);
        }
    };
    if let Err(e) = apply_mutations(&root, &req.copy, &req.remove, &req.mkdir) {
        eprintln!("olo-rex-link sync mutate: {}: {e}", root.display());
        std::process::exit(1);
    }
    let _ = writeln!(io::stdout().lock(), "{{\"ok\":true}}");
    std::process::exit(0);
}

fn read_only_file(path: &Path) -> io::Result<Vec<String>> {
    let bytes = std::fs::read(path)?;
    let _ = std::fs::remove_file(path);
    let mut out = Vec::new();
    for piece in bytes.split(|&b| b == 0) {
        if piece.is_empty() {
            continue;
        }
        let s = std::str::from_utf8(piece).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "non-UTF-8 path in --only-file"))?;
        out.push(s.to_owned());
    }
    Ok(out)
}

pub fn run(args: Vec<String>) -> ! {
    let fail = |why: &str| -> ! {
        eprintln!("olo-rex-link sync: {why}\n{USAGE}");
        std::process::exit(2)
    };
    match args.first().map(String::as_str) {
        Some("watch") => crate::watch::run(args[1..].to_vec()),
        Some("index") => run_index(args[1..].to_vec(), &fail),
        Some("mutate") => run_mutate(args[1..].to_vec(), &fail),
        _ => {}
    }
    let mut it = args.into_iter();
    let (Some(side), Some(raw_root)) = (it.next(), it.next()) else { fail("say which side and its folder") };
    let root = match resolve_root(&raw_root) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("olo-rex-link sync: {raw_root}: {e}");
            std::process::exit(1);
        }
    };
    let (mut options, mut filter, mut cached, mut cache_file) = (Options::default(), Filter::default(), true, None);
    let mut only_file: Option<PathBuf> = None;
    while let Some(a) = it.next() {
        match a.as_str() {
            "--delete" => options.delete_extra = true,
            "--keep-newer" => options.keep_newer = true,
            "--shallow" => filter.recursive = false,
            "--scoped" => options.scoped = true,
            "--no-cache" => cached = false,
            "--cache" => cache_file = Some(PathBuf::from(it.next().unwrap_or_else(|| fail("--cache takes a file")))),
            "--exclude" => filter.exclude.push(it.next().unwrap_or_else(|| fail("--exclude takes a pattern"))),
            "--protect" => options.protect.push(it.next().unwrap_or_else(|| fail("--protect takes a path"))),
            "--only" => filter.only.push(it.next().unwrap_or_else(|| fail("--only takes a path"))),
            "--only-file" => only_file = Some(PathBuf::from(it.next().unwrap_or_else(|| fail("--only-file takes a file")))),
            other => fail(&format!("{other} isn't one of its options")),
        }
    }
    if let Some(path) = only_file {
        match read_only_file(&path) {
            Ok(paths) => filter.only.extend(paths),
            Err(e) => {
                eprintln!("olo-rex-link sync: --only-file {}: {e}", path.display());
                std::process::exit(1);
            }
        }
    }
    // (A file sent: its folder, and it alone, by its name exactly.)
    let root = match (side.as_str(), root.file_name().map(|n| n.to_string_lossy().into_owned())) {
        ("send", Some(name)) if filter.only.is_empty() && std::fs::symlink_metadata(&root).is_ok_and(|m| m.is_file()) => {
            filter.only = vec![name];
            root.parent().map(Path::to_path_buf).unwrap_or(root)
        }
        _ => root,
    };
    options.filter = filter.clone();
    // Never overwrite the default full-tree cache with a subset `--only` scan.
    let cache = if !cached || (!filter.only.is_empty() && cache_file.is_none()) { None } else { cache_file.or_else(|| cache_for(&root)) };
    let conn = Stdio { input: io::stdin(), output: io::stdout() };
    let ended = match side.as_str() {
        "receive" => receive(conn, &root, Options { cache_path: cache, ..options }).map(|_| ()),
        "send" => send(conn, &FolderSource { root: root.clone(), filter, cache_path: cache }, options).map(|_| ()),
        other => fail(&format!("{other}: receive, send, index, mutate or watch")),
    };
    match ended {
        Ok(()) => std::process::exit(0),
        Err(e) => {
            eprintln!("olo-rex-link sync: {}: {e}", root.display());
            std::process::exit(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_root_expands_tilde_and_preserves_dotdot_for_kernel() {
        let home = PathBuf::from(std::env::var_os("HOME").expect("HOME set in test env"));
        assert_eq!(resolve_root("~").unwrap(), home);
        assert_eq!(resolve_root("~/docs/pic.png").unwrap(), home.join("docs/pic.png"));
        assert_eq!(resolve_root("rel/dir").unwrap(), home.join("rel/dir"));
        assert_eq!(resolve_root("/var/tmp/abs").unwrap(), PathBuf::from("/var/tmp/abs"));
        // Preserves `..` so kernel symlink resolution is NOT altered lexically!
        assert_eq!(resolve_root("link/../docs").unwrap(), home.join("link/../docs"));
        assert_eq!(resolve_root("/var/tmp/link/../docs").unwrap(), PathBuf::from("/var/tmp/link/../docs"));
    }
}
