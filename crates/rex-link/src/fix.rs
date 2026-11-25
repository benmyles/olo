//! `olo-rex-link move fix --from OLD --to NEW [--once FILE]`: a moved home
//! made right where its path changed (`/home/ubuntu` on the source,
//! `/home/fedora` here), run on the destination after the copy. What keys
//! by the home's path is rewritten, each file keeping its time and mode
//! (an agent's sessions sorted by when they were last used must stay so):
//! the agents' settings and sessions (Claude Code's trust of a folder is
//! in `~/.claude.json` by its path, Codex's in its `config.toml`, agy's in
//! its settings), git's and SSH's configs, rustup's and cargo's; Claude
//! Code's project folders renamed (named by their path), merged where the
//! destination has one already; symlinks into the old home retargeted; git
//! worktrees' links both ways. What else names the old home (shell startup
//! files) is said, not edited. The old path is replaced only as a whole
//! path (`/home/ubuntu` in `/home/ubuntu-work` or `/mnt/x/home/ubuntu` is
//! left as it is). Run once (`--once`: a file it writes when done).

use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// A name for a file made beside another a moment (the edit, the new
/// link): its own, of a fixed length (a name of 240 bytes and more can't
/// take a suffix), begun with a dot.
fn beside(path: &Path, what: &str) -> PathBuf {
    let mut bytes = [0u8; 8];
    // SAFETY: getrandom into a buffer of ours (a failure leaves zeros and
    // the pid tells two apart).
    unsafe { libc::getrandom(bytes.as_mut_ptr().cast(), bytes.len(), 0) };
    let tag: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    path.with_file_name(format!(".olo-move-{what}-{}-{tag}", std::process::id()))
}

/// `f` done in `dir`, opened to the account a moment if it won't let it
/// make or rename what is in it (a folder of 0555: Go's module cache), and
/// put back as it was, mode and time.
fn in_folder<T>(dir: &Path, f: impl Fn() -> io::Result<T>) -> io::Result<T> {
    match f() {
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
            let meta = std::fs::symlink_metadata(dir)?;
            let mode = meta.permissions().mode() & 0o7777;
            if mode & 0o200 != 0 {
                return Err(e);
            }
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(mode | 0o700))?;
            let done = f();
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(mode));
            let _ = set_times(dir, meta.atime(), meta.atime_nsec(), meta.mtime(), meta.mtime_nsec(), false);
            done
        }
        other => other,
    }
}

/// Whether the byte before a path can start one (not the middle of a word
/// or of another path).
fn starts_path(before: Option<u8>, text: &[u8], at: usize) -> bool {
    match before {
        None => true,
        Some(b'/') => text[..at].ends_with(b"file://"),
        Some(b) => !(b.is_ascii_alphanumeric() || b"._-~$".contains(&b)),
    }
}

/// Whether the byte after the old home ends it (the home itself, or a path
/// under it).
fn ends_path(after: Option<u8>) -> bool {
    match after {
        None => true,
        Some(b) => !(b.is_ascii_alphanumeric() || b"._-".contains(&b)),
    }
}

/// `text` with `old` replaced by `new` wherever it is a whole path; `None`
/// if it has none.
pub fn rewritten(text: &[u8], old: &[u8], new: &[u8]) -> Option<Vec<u8>> {
    if old.is_empty() {
        return None;
    }
    let mut out = Vec::with_capacity(text.len());
    let (mut from, mut changed) = (0, false);
    let mut i = 0;
    while let Some(off) = text[i..].windows(old.len()).position(|w| w == old) {
        let at = i + off;
        let end = at + old.len();
        if starts_path(at.checked_sub(1).map(|b| text[b]), text, at) && ends_path(text.get(end).copied()) {
            out.extend_from_slice(&text[from..at]);
            out.extend_from_slice(new);
            from = end;
            changed = true;
            // (On after it: a match inside the one replaced would start
            // before what is already written.)
            i = end;
        } else {
            i = at + 1;
        }
    }
    changed.then(|| {
        out.extend_from_slice(&text[from..]);
        out
    })
}

/// A file rewritten whole beside itself and put in its place, its mode and
/// times kept; a symlink's target edited, not the link, where it is in the
/// home (`home`, by its real path: one elsewhere, the system's or another
/// account's, is never edited). Line by line (a session's transcript may
/// be hundreds of megabytes); private while it is written (it may hold
/// secrets).
/// What [`rewrite_file`] did.
#[derive(Debug, PartialEq, Eq)]
enum Rewrite {
    Changed,
    Same,
    /// A link to a file outside the home: left as it is.
    Outside,
}

fn rewrite_file(path: &Path, home: &Path, old: &str, new: &str) -> io::Result<Rewrite> {
    let real = std::fs::canonicalize(path)?;
    if !real.starts_with(home) {
        return Ok(Rewrite::Outside);
    }
    let meta = std::fs::metadata(&real)?;
    if !meta.is_file() {
        return Ok(Rewrite::Same);
    }
    let temp = beside(&real, "fix");
    let mut changed = false;
    let written = (|| -> io::Result<()> {
        let mut reader = BufReader::new(std::fs::File::open(&real)?);
        let file = in_folder(real.parent().unwrap_or(home), || std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&temp))?;
        let mut writer = BufWriter::new(file);
        let mut line = Vec::new();
        loop {
            line.clear();
            if reader.read_until(b'\n', &mut line)? == 0 {
                break;
            }
            match rewritten(&line, old.as_bytes(), new.as_bytes()) {
                Some(r) => {
                    changed = true;
                    writer.write_all(&r)?;
                }
                None => writer.write_all(&line)?,
            }
        }
        writer.flush()
    })();
    let parent = real.parent().unwrap_or(home);
    if written.is_err() || !changed {
        let _ = in_folder(parent, || std::fs::remove_file(&temp));
        return written.map(|()| Rewrite::Same);
    }
    let put = std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(meta.permissions().mode() & 0o7777))
        .and_then(|()| set_times(&temp, meta.atime(), meta.atime_nsec(), meta.mtime(), meta.mtime_nsec(), false))
        .and_then(|()| in_folder(parent, || std::fs::rename(&temp, &real)));
    if put.is_err() {
        let _ = in_folder(parent, || std::fs::remove_file(&temp));
    }
    put.map(|()| Rewrite::Changed)
}

pub(crate) fn set_times(path: &Path, atime: i64, atime_ns: i64, mtime: i64, mtime_ns: i64, link: bool) -> io::Result<()> {
    let c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).map_err(io::Error::other)?;
    let times = [libc::timespec { tv_sec: atime, tv_nsec: atime_ns }, libc::timespec { tv_sec: mtime, tv_nsec: mtime_ns }];
    let flags = if link { libc::AT_SYMLINK_NOFOLLOW } else { 0 };
    // SAFETY: a NUL-ended path and two timespecs.
    if unsafe { libc::utimensat(libc::AT_FDCWD, c.as_ptr(), times.as_ptr(), flags) } == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
}

/// Claude Code's name for a project's folder: its path, each byte but a
/// letter or a digit as `-`.
pub fn project_name(path: &str) -> String {
    path.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

/// The working folder a Claude Code project's transcripts say (`"cwd"`),
/// from the first that says one.
fn project_cwd(dir: &Path) -> Option<String> {
    let files = std::fs::read_dir(dir).ok()?;
    for f in files.filter_map(|e| e.ok()).filter(|e| e.file_name().to_str().is_some_and(|n| n.ends_with(".jsonl"))) {
        let reader = BufReader::new(std::fs::File::open(f.path()).ok()?);
        for line in reader.split(b'\n').take(50).filter_map(|l| l.ok()) {
            if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&line)
                && let Some(cwd) = v["cwd"].as_str()
            {
                return Some(cwd.to_owned());
            }
        }
    }
    None
}

/// `from` moved into `to`, entry by entry: what `to` has already stays
/// (the newer of two files with one name in its place, the older beside it
/// as `NAME.older`, which no agent reads; a file where the other has a
/// folder beside it as `NAME.moved`), `from` taken away once empty: nothing
/// lost.
fn merge_into(from: &Path, to: &Path) -> io::Result<()> {
    let when = |m: &std::fs::Metadata| (m.mtime(), m.mtime_nsec());
    let aside = |there: &Path, tag: &str| -> PathBuf {
        let name = there.file_name().and_then(|n| n.to_str()).unwrap_or("entry");
        (0..)
            .map(|n| there.with_file_name(if n == 0 { format!("{name}.{tag}") } else { format!("{name}.{tag}-{n}") }))
            .find(|p| std::fs::symlink_metadata(p).is_err())
            .unwrap_or_else(|| there.to_path_buf())
    };
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let there = to.join(e.file_name());
        let mine = std::fs::symlink_metadata(e.path())?;
        match std::fs::symlink_metadata(&there) {
            Err(_) => std::fs::rename(e.path(), &there)?,
            Ok(m) if m.is_dir() && mine.is_dir() => merge_into(&e.path(), &there)?,
            Ok(m) if m.is_dir() != mine.is_dir() => std::fs::rename(e.path(), aside(&there, "moved"))?,
            Ok(m) if when(&mine) > when(&m) => {
                std::fs::rename(&there, aside(&there, "older"))?;
                std::fs::rename(e.path(), &there)?;
            }
            Ok(_) => std::fs::rename(e.path(), aside(&there, "older"))?,
        }
    }
    std::fs::remove_dir(from)
}

/// What `fix` did, and what it left to the person.
#[derive(Debug, Default)]
pub struct Fixed {
    pub rewritten: Vec<String>,
    pub projects: Vec<String>,
    pub relinked: Vec<String>,
    pub worktrees: Vec<String>,
    pub mentions: Vec<String>,
    pub failed: Vec<String>,
}

impl Fixed {
    fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "rewritten": self.rewritten, "projects": self.projects, "relinked": self.relinked,
            "worktrees": self.worktrees, "mentions": self.mentions, "failed": self.failed,
        })
    }
}

/// The files that key by the home's path, under the home (globs: `*` one
/// level, `**` any).
const KEYED: &[&str] = &[
    ".claude.json",
    ".claude/settings.json",
    ".claude/settings.local.json",
    ".claude/history.jsonl",
    ".claude/plugins/**/*.json",
    ".claude/projects/*/*.jsonl",
    ".claude/projects/*/sessions-index.json",
    ".claude/projects/*/*/subagents/*.jsonl",
    ".claude/projects/*/memory/*.md",
    ".codex/config.toml",
    ".codex/hooks.json",
    ".codex/history.jsonl",
    ".codex/session_index.jsonl",
    ".codex/sessions/**/*.jsonl",
    ".codex/archived_sessions/**/*.jsonl",
    ".gemini/antigravity-cli/settings.json",
    ".gemini/antigravity-cli/mcp_config.json",
    ".gemini/antigravity-cli/brain/**/*.md",
    ".gemini/antigravity-cli/brain/**/*.json",
    ".ssh/config",
    ".gitconfig",
    ".config/git/config",
    ".rustup/settings.toml",
    ".cargo/config.toml",
    ".config/systemd/user/*.service",
    ".config/systemd/user/*.timer",
    ".config/systemd/user/*.socket",
    ".config/systemd/user/*.path",
    ".config/systemd/user/*.d/*.conf",
    ".config/containers/systemd/*.container",
    ".config/containers/systemd/*.volume",
    ".config/containers/systemd/*.network",
    ".config/containers/systemd/*.kube",
    ".config/containers/systemd/*.pod",
    ".config/containers/systemd/*.image",
    ".config/containers/systemd/*.build",
    ".config/containers/systemd/*.d/*.conf",
    ".gemini/antigravity-cli/annotations/*.pbtxt",
];

/// Shell startup files: said where they name the old home, not edited
/// (the link from the old home, where there is one, keeps them working).
const STARTUP: &[&str] = &[".bashrc", ".bash_profile", ".bash_login", ".profile", ".zshrc", ".zprofile", ".zshenv", ".zlogin", ".config/fish/config.fish"];

/// The files under `home` a pattern of [`KEYED`] names.
fn expand(home: &Path, pattern: &str) -> Vec<PathBuf> {
    let mut found = vec![home.to_path_buf()];
    let parts: Vec<&str> = pattern.split('/').collect();
    for (i, part) in parts.iter().enumerate() {
        let last = i + 1 == parts.len();
        let mut next = Vec::new();
        for at in &found {
            if *part == "**" {
                // (Any depth of folders, this one included.)
                let mut stack = vec![at.clone()];
                while let Some(d) = stack.pop() {
                    next.push(d.clone());
                    if let Ok(rd) = std::fs::read_dir(&d) {
                        for e in rd.filter_map(|e| e.ok()) {
                            if e.file_type().is_ok_and(|t| t.is_dir()) {
                                stack.push(e.path());
                            }
                        }
                    }
                }
            } else if let Some((pre, post)) = part.split_once('*') {
                if let Ok(rd) = std::fs::read_dir(at) {
                    for e in rd.filter_map(|e| e.ok()) {
                        let name = e.file_name();
                        let Some(name) = name.to_str() else { continue };
                        let is_dir = e.file_type().is_ok_and(|t| t.is_dir());
                        if name.starts_with(pre) && name.ends_with(post) && name.len() >= pre.len() + post.len() && (last || is_dir) {
                            next.push(e.path());
                        }
                    }
                }
            } else {
                let p = at.join(part);
                if std::fs::symlink_metadata(&p).is_ok() {
                    next.push(p);
                }
            }
        }
        found = next;
    }
    found.retain(|p| std::fs::metadata(p).is_ok_and(|m| m.is_file()));
    found.sort();
    found.dedup();
    found
}

/// Walks the home (one filesystem, never into what the move's own files
/// are), calling `each` for every entry by its `lstat`.
fn walk(home: &Path, each: &mut dyn FnMut(&Path, &std::fs::Metadata)) {
    let dev = std::fs::symlink_metadata(home).map(|m| m.dev()).unwrap_or(0);
    let mut stack = vec![home.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.filter_map(|e| e.ok()) {
            let path = e.path();
            let Ok(meta) = std::fs::symlink_metadata(&path) else { continue };
            each(&path, &meta);
            if meta.is_dir() && meta.dev() == dev && !path.ends_with(".local/state/olo-rex/move") {
                stack.push(path);
            }
        }
    }
}

/// The fix itself (`home` the destination's, now holding the moved files;
/// `also`, folders of `/var/tmp` carried with it: their links into the old
/// home and their git links made right too).
#[cfg(test)]
pub fn fix(home: &Path, old: &str, new: &str) -> Fixed {
    fix_with(home, &[], old, new)
}

pub fn fix_with(home: &Path, also: &[PathBuf], old: &str, new: &str) -> Fixed {
    let mut done = Fixed::default();
    let rel = |p: &Path| p.strip_prefix(home).map(|r| r.display().to_string()).unwrap_or_else(|_| p.display().to_string());
    // Claude Code's project folders first (their transcripts are then
    // rewritten where they are).
    let projects = home.join(".claude/projects");
    let (old_name, new_name) = (project_name(old), project_name(new));
    if let Ok(rd) = std::fs::read_dir(&projects) {
        let mut names: Vec<String> = rd.filter_map(|e| e.ok()?.file_name().into_string().ok()).collect();
        names.sort();
        for name in names {
            let Some(rest) = name.strip_prefix(&old_name) else { continue };
            if !(rest.is_empty() || rest.starts_with('-')) {
                continue;
            }
            // (Named as the old home's, but it may be a folder beside it:
            // `/home/ubuntu-work`. Its transcripts say.)
            let dir = projects.join(&name);
            if let Some(cwd) = project_cwd(&dir)
                && !(cwd == old || cwd.starts_with(&format!("{old}/")))
            {
                continue;
            }
            let target = projects.join(format!("{new_name}{rest}"));
            let moved = if target.exists() { merge_into(&dir, &target) } else { std::fs::rename(&dir, &target) };
            match moved {
                Ok(()) => done.projects.push(format!("{name} → {new_name}{rest}")),
                Err(e) => done.failed.push(format!("{name}: {e}")),
            }
        }
    }
    // Symlinks into the old home first (a dotfile linked into a folder of
    // the home's is only found as a file once its link leads there), with
    // git's worktree links and own settings: one walk.
    let mut links = Vec::new();
    let mut gitdirs = Vec::new();
    let mut each = |path: &Path, meta: &std::fs::Metadata| {
        let parent = path.parent().unwrap_or(home);
        if meta.file_type().is_symlink() {
            if let Ok(target) = std::fs::read_link(path)
                && let Some(t) = target.to_str()
                && (t == old || t.starts_with(&format!("{old}/")))
            {
                links.push((path.to_path_buf(), format!("{new}{}", &t[old.len()..]), meta.clone()));
            }
        } else if meta.is_file() && path.file_name().is_some_and(|n| n == ".git") {
            gitdirs.push(path.to_path_buf());
        } else if meta.is_file() && path.file_name().is_some_and(|n| n == "gitdir") && parent.parent().is_some_and(|p| p.ends_with(".git/worktrees")) {
            gitdirs.push(path.to_path_buf());
        } else if meta.is_file()
            && path.file_name().is_some_and(|n| n == "config")
            && (parent.ends_with(".git") || parent.components().any(|c| c.as_os_str() == "modules") && parent.components().any(|c| c.as_os_str() == ".git"))
        {
            // (A repository's own settings: a remote in the old home, a
            // worktree's path; a submodule's too.)
            gitdirs.push(path.to_path_buf());
        }
    };
    walk(home, &mut each);
    for root in also {
        if let Ok(meta) = std::fs::symlink_metadata(root) {
            if meta.is_dir() {
                walk(root, &mut each);
            } else {
                each(root, &meta);
            }
        }
    }
    for (path, target, meta) in links {
        let temp = beside(&path, "link");
        let parent = path.parent().unwrap_or(home).to_path_buf();
        let relinked = in_folder(&parent, || std::os::unix::fs::symlink(&target, &temp))
            .and_then(|()| set_times(&temp, meta.atime(), meta.atime_nsec(), meta.mtime(), meta.mtime_nsec(), true))
            .and_then(|()| in_folder(&parent, || std::fs::rename(&temp, &path)));
        match relinked {
            Ok(()) => done.relinked.push(rel(&path)),
            Err(e) => {
                let _ = in_folder(&parent, || std::fs::remove_file(&temp));
                done.failed.push(format!("{}: {e}", rel(&path)));
            }
        }
    }
    let real_home = std::fs::canonicalize(home).unwrap_or_else(|_| home.to_path_buf());
    for path in gitdirs {
        // (A git link of a folder of /var/tmp's is edited there: its own
        // root stands for the home's.)
        let within = also.iter().find(|r| path.starts_with(r)).and_then(|r| std::fs::canonicalize(r).ok()).unwrap_or_else(|| real_home.clone());
        match rewrite_file(&path, &within, old, new) {
            Ok(Rewrite::Changed) => done.worktrees.push(rel(&path)),
            Ok(Rewrite::Same) => {}
            Ok(Rewrite::Outside) => done.mentions.push(rel(&path)),
            Err(e) => done.failed.push(format!("{}: {e}", rel(&path))),
        }
    }
    for pattern in KEYED {
        for path in expand(home, pattern) {
            match rewrite_file(&path, &real_home, old, new) {
                Ok(Rewrite::Changed) => done.rewritten.push(rel(&path)),
                Ok(Rewrite::Same) => {}
                // (A link to a file outside the home: said, not edited.)
                Ok(Rewrite::Outside) => done.mentions.push(rel(&path)),
                Err(e) => done.failed.push(format!("{}: {e}", rel(&path))),
            }
        }
    }
    for name in STARTUP {
        let path = home.join(name);
        let Ok(text) = std::fs::read(&path) else { continue };
        for (n, line) in text.split(|&b| b == b'\n').enumerate() {
            if rewritten(line, old.as_bytes(), new.as_bytes()).is_some() {
                done.mentions.push(format!("{name}:{}", n + 1));
            }
        }
    }
    done
}

/// Top-level entries in `/var/tmp` owned by `uid`.
fn var_tmp_owned(uid: u32) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir("/var/tmp") else { return Vec::new() };
    let mut out: Vec<PathBuf> = rd.filter_map(|e| e.ok()).filter(|e| std::fs::symlink_metadata(e.path()).is_ok_and(|m| m.uid() == uid)).map(|e| e.path()).collect();
    out.sort();
    out
}

/// `move fix --from OLD --to NEW [--once FILE] [--var-tmp] [--also /var/tmp/NAME]…`.
pub fn run(args: &[String]) -> i32 {
    let (mut from, mut to, mut once, mut also, mut var_tmp) = (None, None, None, Vec::new(), false);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--from" => match it.next() {
                Some(v) => from = Some(v.clone()),
                None => {
                    eprintln!("usage: olo-rex-link move fix --from OLD --to NEW [--once FILE] [--var-tmp]");
                    return 2;
                }
            },
            "--to" => match it.next() {
                Some(v) => to = Some(v.clone()),
                None => {
                    eprintln!("usage: olo-rex-link move fix --from OLD --to NEW [--once FILE] [--var-tmp]");
                    return 2;
                }
            },
            "--once" => match it.next() {
                Some(v) => once = Some(PathBuf::from(v)),
                None => {
                    eprintln!("usage: olo-rex-link move fix --from OLD --to NEW [--once FILE] [--var-tmp]");
                    return 2;
                }
            },
            "--var-tmp" => var_tmp = true,
            "--also" => match it.next() {
                Some(v) if Path::new(v).parent() == Some(Path::new("/var/tmp")) => also.push(PathBuf::from(v)),
                _ => {
                    eprintln!("usage: olo-rex-link move fix --from OLD --to NEW [--once FILE] [--var-tmp]");
                    return 2;
                }
            },
            _ => {
                eprintln!("usage: olo-rex-link move fix --from OLD --to NEW [--once FILE] [--var-tmp]");
                return 2;
            }
        }
    }
    if var_tmp {
        // SAFETY: a getter.
        also.extend(var_tmp_owned(unsafe { libc::getuid() }));
        also.sort();
        also.dedup();
    }
    let (Some(old), Some(new)) = (from, to) else {
        eprintln!("usage: olo-rex-link move fix --from OLD --to NEW [--once FILE]");
        return 2;
    };
    let ok = |p: &str| p.starts_with('/') && p.len() > 1 && !p.ends_with('/');
    // (A home inside the other would be rewritten again by a second run:
    // never; and the same home has nothing to rewrite.)
    if !ok(&old) || !ok(&new) || old == new || new.starts_with(&format!("{old}/")) || old.starts_with(&format!("{new}/")) {
        eprintln!("olo-rex-link move fix: two homes, whole paths, neither inside the other");
        return 2;
    }
    if let Some(once) = &once
        && let Ok(said) = std::fs::read_to_string(once)
    {
        println!("{}", said.trim());
        return 0;
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(&new));
    let done = fix_with(&home, &also, &old, &new);
    let report = done.json().to_string();
    // (Kept, whole or not at all, only when nothing failed: a second run
    // tries again what didn't go.)
    if let Some(once) = once
        && done.failed.is_empty()
    {
        let temp = once.with_extension(format!("{}.new", std::process::id()));
        let _ = std::fs::write(&temp, &report).and_then(|()| std::fs::rename(&temp, &once));
    }
    println!("{report}");
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The old home is replaced only as a whole path: in JSON, TOML, a
    /// `file://` link, a key; not inside another path or word.
    #[test]
    fn only_whole_paths_are_rewritten() {
        let r = |s: &str| rewritten(s.as_bytes(), b"/home/ubuntu", b"/home/fedora").map(|v| String::from_utf8(v).unwrap());
        assert_eq!(
            r(r#"{"projects":{"/home/ubuntu/proj":{"hasTrustDialogAccepted":true}},"cwd":"/home/ubuntu"}"#).unwrap(),
            r#"{"projects":{"/home/fedora/proj":{"hasTrustDialogAccepted":true}},"cwd":"/home/fedora"}"#
        );
        assert_eq!(r("[projects.\"/home/ubuntu/a b\"]\ntrust_level = \"trusted\"\n").unwrap(), "[projects.\"/home/fedora/a b\"]\ntrust_level = \"trusted\"\n");
        assert_eq!(r("see file:///home/ubuntu/x.md and PATH=/home/ubuntu/.local/bin:/usr/bin").unwrap(), "see file:///home/fedora/x.md and PATH=/home/fedora/.local/bin:/usr/bin");
        assert_eq!(r("/home/ubuntu-work /mnt/x/home/ubuntu /home/ubuntu2 ~/home/ubuntu"), None);
        assert_eq!(r("IdentityFile /home/ubuntu/.ssh/id_ed25519\n").unwrap(), "IdentityFile /home/fedora/.ssh/id_ed25519\n");
        assert_eq!(r(r#"{"p":"C:\\x","q":"/home/ubuntu\\"}"#).unwrap(), r#"{"p":"C:\\x","q":"/home/fedora\\"}"#);
        assert_eq!(r("nothing here"), None);
    }

    fn scratch(name: &str) -> PathBuf {
        let d = PathBuf::from(format!("/var/tmp/olo-rex-link-fix-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A moved home made right: the agents' files rewritten (their times
    /// kept), Claude's project folders renamed and merged (one beside the
    /// home's left), symlinks into the old home retargeted, a worktree's
    /// links rewritten both ways, a startup file's mention said, not edited.
    #[test]
    fn a_moved_home_is_made_right() {
        let home = scratch("home");
        let w = |p: &str, s: &str| {
            let path = home.join(p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, s).unwrap();
        };
        w(".claude.json", r#"{"projects":{"/home/ubuntu/proj":{"hasTrustDialogAccepted":true}}}"#);
        w(".claude/projects/-home-ubuntu-proj/s1.jsonl", "{\"cwd\":\"/home/ubuntu/proj\",\"type\":\"user\"}\n{\"type\":\"x\"}\n");
        w(".claude/projects/-home-ubuntu-work/s2.jsonl", "{\"cwd\":\"/home/ubuntu-work\"}\n");
        w(".claude/projects/-home-fedora-proj/s3.jsonl", "{\"cwd\":\"/home/fedora/proj\"}\n");
        w(".codex/config.toml", "[projects.\"/home/ubuntu/proj\"]\ntrust_level = \"trusted\"\n");
        w(".codex/sessions/2026/11/25/rollout-a-ID.jsonl", "{\"cwd\":\"/home/ubuntu/proj\"}\n");
        w(".bashrc", "export PATH=/home/ubuntu/bin:$PATH\n# fine\n");
        w("repo/.git/worktrees/wt/gitdir", "/home/ubuntu/wt/.git\n");
        w("wt/.git", "gitdir: /home/ubuntu/repo/.git/worktrees/wt\n");
        std::os::unix::fs::symlink("/home/ubuntu/proj/main.rs", home.join("abs-link")).unwrap();
        std::os::unix::fs::symlink("proj/main.rs", home.join("rel-link")).unwrap();
        let before = std::fs::metadata(home.join(".codex/sessions/2026/11/25/rollout-a-ID.jsonl")).unwrap().mtime();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let done = fix(&home, "/home/ubuntu", "/home/fedora");
        assert!(done.failed.is_empty(), "{:?}", done.failed);
        let r = |p: &str| std::fs::read_to_string(home.join(p)).unwrap();
        assert_eq!(r(".claude.json"), r#"{"projects":{"/home/fedora/proj":{"hasTrustDialogAccepted":true}}}"#);
        assert_eq!(r(".codex/config.toml"), "[projects.\"/home/fedora/proj\"]\ntrust_level = \"trusted\"\n");
        assert_eq!(r(".claude/projects/-home-fedora-proj/s1.jsonl"), "{\"cwd\":\"/home/fedora/proj\",\"type\":\"user\"}\n{\"type\":\"x\"}\n");
        assert!(home.join(".claude/projects/-home-fedora-proj/s3.jsonl").exists(), "merged with what was there");
        assert!(!home.join(".claude/projects/-home-ubuntu-proj").exists());
        assert!(home.join(".claude/projects/-home-ubuntu-work/s2.jsonl").exists(), "a folder beside the home isn't the home's");
        assert_eq!(std::fs::metadata(home.join(".codex/sessions/2026/11/25/rollout-a-ID.jsonl")).unwrap().mtime(), before, "its time kept");
        assert_eq!(std::fs::read_link(home.join("abs-link")).unwrap(), PathBuf::from("/home/fedora/proj/main.rs"));
        assert_eq!(std::fs::read_link(home.join("rel-link")).unwrap(), PathBuf::from("proj/main.rs"));
        assert_eq!(r("repo/.git/worktrees/wt/gitdir"), "/home/fedora/wt/.git\n");
        assert_eq!(r("wt/.git"), "gitdir: /home/fedora/repo/.git/worktrees/wt\n");
        assert_eq!(r(".bashrc"), "export PATH=/home/ubuntu/bin:$PATH\n# fine\n", "said, not edited");
        assert_eq!(done.mentions, [".bashrc:1"]);
        assert_eq!(done.projects, ["-home-ubuntu-proj → -home-fedora-proj"]);
        let _ = std::fs::remove_dir_all(&home);
    }

    /// A dotfile linked into a folder of the old home's is found once its
    /// link leads there, and edited where it points; one linked outside the
    /// home is said, not edited; a link in a read-only folder is made
    /// again there, the folder's mode put back; a match never runs into
    /// the one before it.
    #[test]
    fn linked_dotfiles_are_made_right_where_they_point() {
        let root = scratch("linked");
        // (The destination's home as the source's links name it: the old
        // home is `root/old`, which isn't there, the new `root/home`.)
        let home = root.join("home");
        let (old, new) = (root.join("old").display().to_string(), home.display().to_string());
        std::fs::create_dir_all(home.join("dotfiles")).unwrap();
        std::fs::create_dir_all(home.join("ro")).unwrap();
        std::fs::write(home.join("dotfiles/gitconfig"), format!("[core]\n\texcludesFile = {old}/.gitignore\n")).unwrap();
        std::os::unix::fs::symlink(format!("{old}/dotfiles/gitconfig"), home.join(".gitconfig")).unwrap();
        std::fs::write(root.join("elsewhere.toml"), format!("dir = \"{old}/x\"\n")).unwrap();
        std::fs::create_dir_all(home.join(".cargo")).unwrap();
        std::os::unix::fs::symlink(root.join("elsewhere.toml"), home.join(".cargo/config.toml")).unwrap();
        std::os::unix::fs::symlink(format!("{old}/dotfiles"), home.join("ro/link")).unwrap();
        std::fs::set_permissions(home.join("ro"), std::fs::Permissions::from_mode(0o555)).unwrap();
        let done = fix(&home, &old, &new);
        assert!(done.failed.is_empty(), "{:?}", done.failed);
        assert_eq!(std::fs::read_to_string(home.join("dotfiles/gitconfig")).unwrap(), format!("[core]\n\texcludesFile = {new}/.gitignore\n"));
        assert_eq!(std::fs::read_link(home.join(".gitconfig")).unwrap(), PathBuf::from(format!("{new}/dotfiles/gitconfig")));
        assert_eq!(std::fs::read_to_string(root.join("elsewhere.toml")).unwrap(), format!("dir = \"{old}/x\"\n"), "outside the home: left");
        assert!(done.mentions.contains(&".cargo/config.toml".to_owned()), "{:?}", done.mentions);
        assert_eq!(std::fs::read_link(home.join("ro/link")).unwrap(), PathBuf::from(format!("{new}/dotfiles")));
        assert_eq!(std::fs::metadata(home.join("ro")).unwrap().permissions().mode() & 0o777, 0o555);
        std::fs::set_permissions(home.join("ro"), std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(rewritten(b"/a+/a+/a+/x", b"/a+/a+", b"/b").map(|v| String::from_utf8(v).unwrap()), Some("/b/a+/x".to_owned()));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Two project folders merged lose nothing: the newer of two files in
    /// its place, the older beside it; a file where the other has a folder
    /// beside it.
    #[test]
    fn merging_loses_nothing() {
        let root = scratch("merge");
        let (from, to) = (root.join("from"), root.join("to"));
        for d in [&from, &to] {
            std::fs::create_dir_all(d.join("sub")).unwrap();
        }
        std::fs::write(to.join("s.jsonl"), "theirs").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(from.join("s.jsonl"), "mine, newer").unwrap();
        std::fs::write(from.join("sub/a"), "a").unwrap();
        std::fs::write(from.join("x"), "a file").unwrap();
        std::fs::create_dir_all(to.join("x")).unwrap();
        merge_into(&from, &to).unwrap();
        assert!(!from.exists());
        assert_eq!(std::fs::read_to_string(to.join("s.jsonl")).unwrap(), "mine, newer");
        assert_eq!(std::fs::read_to_string(to.join("s.jsonl.older")).unwrap(), "theirs");
        assert_eq!(std::fs::read_to_string(to.join("sub/a")).unwrap(), "a");
        assert_eq!(std::fs::read_to_string(to.join("x.moved")).unwrap(), "a file");
        let _ = std::fs::remove_dir_all(&root);
    }
}
