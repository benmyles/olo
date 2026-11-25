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
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

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
        }
        i = at + 1;
    }
    changed.then(|| {
        out.extend_from_slice(&text[from..]);
        out
    })
}

/// A file rewritten whole beside itself and put in its place, its mode and
/// times kept; a symlink's target edited, not the link. Line by line (a
/// session's transcript may be hundreds of megabytes).
fn rewrite_file(path: &Path, old: &str, new: &str) -> io::Result<bool> {
    let real = std::fs::canonicalize(path)?;
    let meta = std::fs::metadata(&real)?;
    if !meta.is_file() {
        return Ok(false);
    }
    let name = real.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let temp = real.with_file_name(format!(".{name}.olo-move-fix"));
    let mut changed = false;
    {
        let mut reader = BufReader::new(std::fs::File::open(&real)?);
        let mut writer = BufWriter::new(std::fs::File::create(&temp)?);
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
        writer.flush()?;
    }
    if !changed {
        std::fs::remove_file(&temp)?;
        return Ok(false);
    }
    std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(meta.permissions().mode() & 0o7777))?;
    set_times(&temp, meta.atime(), meta.atime_nsec(), meta.mtime(), meta.mtime_nsec(), false)?;
    std::fs::rename(&temp, &real)?;
    Ok(true)
}

fn set_times(path: &Path, atime: i64, atime_ns: i64, mtime: i64, mtime_ns: i64, link: bool) -> io::Result<()> {
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
/// (the newer of two files with one name kept), `from` taken away once
/// empty.
fn merge_into(from: &Path, to: &Path) -> io::Result<()> {
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let there = to.join(e.file_name());
        match std::fs::symlink_metadata(&there) {
            Err(_) => std::fs::rename(e.path(), &there)?,
            Ok(m) if m.is_dir() && e.file_type()?.is_dir() => merge_into(&e.path(), &there)?,
            Ok(m) => {
                if e.metadata()?.mtime() > m.mtime() {
                    std::fs::rename(e.path(), &there)?;
                }
            }
        }
    }
    let _ = std::fs::remove_dir_all(from);
    Ok(())
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
    ".claude/plugins/*.json",
    ".claude/projects/*/*.jsonl",
    ".claude/projects/*/sessions-index.json",
    ".claude/projects/*/*/subagents/*.jsonl",
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

/// The fix itself (`home` the destination's, now holding the moved files).
pub fn fix(home: &Path, old: &str, new: &str) -> Fixed {
    let mut done = Fixed::default();
    let rel = |p: &Path| p.strip_prefix(home).unwrap_or(p).display().to_string();
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
    for pattern in KEYED {
        for path in expand(home, pattern) {
            match rewrite_file(&path, old, new) {
                Ok(true) => done.rewritten.push(rel(&path)),
                Ok(false) => {}
                Err(e) => done.failed.push(format!("{}: {e}", rel(&path))),
            }
        }
    }
    // Symlinks into the old home, git's worktree links: one walk.
    let mut links = Vec::new();
    let mut gitdirs = Vec::new();
    walk(home, &mut |path, meta| {
        if meta.file_type().is_symlink() {
            if let Ok(target) = std::fs::read_link(path)
                && let Some(t) = target.to_str()
                && (t == old || t.starts_with(&format!("{old}/")))
            {
                links.push((path.to_path_buf(), format!("{new}{}", &t[old.len()..]), meta.clone()));
            }
        } else if meta.is_file() && path.file_name().is_some_and(|n| n == ".git") {
            gitdirs.push(path.to_path_buf());
        } else if meta.is_file() && path.file_name().is_some_and(|n| n == "gitdir") && path.parent().and_then(Path::parent).is_some_and(|p| p.ends_with(".git/worktrees")) {
            gitdirs.push(path.to_path_buf());
        }
    });
    for (path, target, meta) in links {
        let temp = path.with_file_name(format!(".{}.olo-move-link", path.file_name().and_then(|n| n.to_str()).unwrap_or("link")));
        let relinked = std::os::unix::fs::symlink(&target, &temp)
            .and_then(|()| set_times(&temp, meta.atime(), meta.atime_nsec(), meta.mtime(), meta.mtime_nsec(), true))
            .and_then(|()| std::fs::rename(&temp, &path));
        match relinked {
            Ok(()) => done.relinked.push(rel(&path)),
            Err(e) => {
                let _ = std::fs::remove_file(&temp);
                done.failed.push(format!("{}: {e}", rel(&path)));
            }
        }
    }
    for path in gitdirs {
        match rewrite_file(&path, old, new) {
            Ok(true) => done.worktrees.push(rel(&path)),
            Ok(false) => {}
            Err(e) => done.failed.push(format!("{}: {e}", rel(&path))),
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

/// `move fix --from OLD --to NEW [--once FILE]`.
pub fn run(args: &[String]) -> i32 {
    let (mut from, mut to, mut once) = (None, None, None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match (a.as_str(), it.next()) {
            ("--from", Some(v)) => from = Some(v.clone()),
            ("--to", Some(v)) => to = Some(v.clone()),
            ("--once", Some(v)) => once = Some(PathBuf::from(v)),
            _ => {
                eprintln!("usage: olo-rex-link move fix --from OLD --to NEW [--once FILE]");
                return 2;
            }
        }
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
    let report = fix(&home, &old, &new).json().to_string();
    if let Some(once) = once {
        let _ = std::fs::write(once, &report);
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
}
