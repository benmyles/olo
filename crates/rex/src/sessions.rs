//! The person's coding agents' sessions, copied to a host's account
//! (Ben, 2026-11-23: "an Agent Sessions item that copies over the
//! transcripts/session logs for all the agents … a dropdown to choose
//! the time window and by default have it be last 7 days so that it
//! would just copy sessions that have been used/modified in the last 7
//! days (not only created but also any used)").
//!
//! *Which*: those last written to within the window ([`Window`]): a
//! session in use is written at each turn, so its time is when it was
//! last used. Claude Code's are `~/.claude/projects/PROJECT/UUID.jsonl`,
//! with `UUID/` beside it (its subagents, its tools' long outputs),
//! PROJECT being the project's path with each character but a letter or
//! a digit as `-`. Codex's are `~/.codex/sessions/YYYY/MM/DD/rollout-…-
//! UUID.jsonl`, each with its line of `session_index.jsonl` (its name):
//! put beside the host's, never over them ([`PUT`]); and its database
//! there told of each ([`REGISTER`]: Codex reads its sessions' folder
//! only once, as its database is made, and a session copied after that
//! is listed once something asks for it by its id). Antigravity's are
//! `~/.gemini/antigravity-cli/conversations/UUID.db`, an SQLite file
//! (copied as a whole snapshot, its write-ahead log in it: one in use
//! keeps much of itself in its log), with `annotations/UUID.pbtxt` (its
//! title) and `brain/UUID/` (what it made, less its own logs);
//! Antigravity finds them there by itself as it starts. (Found by the
//! second investigator reading both programs, 2026-11-23, and Codex's
//! part tried with the program itself in a scratch home.)
//!
//! *Where they go*: under the host's home, as here, with this
//! computer's home said as the host's: in a project folder's name and
//! in what a transcript says (its working folder, the files it read), so
//! `claude --resume` in the same project there lists it.
//!
//! *What is sent*: what is missing there or older there, by its time
//! (the host is asked each file's size and time; hashing gigabytes at
//! each look would cost too much), never what is newer there (a session
//! continued on the host stays as it is); each file with its time here,
//! so the next look finds it the same. As one gzip'd tar, made as it is
//! sent ([`stream`]): a week of sessions can be gigabytes.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// How far back sessions are taken, by when each was last written to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Window {
    Day,
    #[default]
    Week,
    Month,
    All,
}

impl Window {
    pub const ALL: [Window; 4] = [Window::Day, Window::Week, Window::Month, Window::All];

    /// How many days back; none: all of them.
    pub fn days(self) -> Option<u64> {
        match self {
            Window::Day => Some(1),
            Window::Week => Some(7),
            Window::Month => Some(30),
            Window::All => None,
        }
    }

    /// As a person chooses it (a pop-up's choice).
    pub fn label(self) -> &'static str {
        match self {
            Window::Day => "Last day",
            Window::Week => "Last 7 days",
            Window::Month => "Last 30 days",
            Window::All => "All time",
        }
    }

    /// From how many days a setting keeps (0: all), the nearest there is.
    pub fn of_days(days: u32) -> Window {
        match days {
            0 => Window::All,
            1 => Window::Day,
            2..=7 => Window::Week,
            _ => Window::Month,
        }
    }

    /// As a setting keeps it: days back, 0 for all.
    pub fn to_days(self) -> u32 {
        self.days().map_or(0, |d| d as u32)
    }

    /// The earliest time a session may have been last written to.
    fn since(self, now: SystemTime) -> Option<SystemTime> {
        self.days().map(|days| now - std::time::Duration::from_secs(days * 86_400))
    }
}

/// One file of a session: where it is here, and where under the home.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Part {
    pub from: PathBuf,
    /// Under this computer's home, `/` between its parts.
    pub path: String,
    pub size: u64,
    /// When it was last written to, in seconds.
    pub modified: i64,
    /// The session's own transcript (one a session: what is counted).
    pub transcript: bool,
    /// Text whose paths are said as the host's ([`rewritten`]): a
    /// transcript, a text file of a session's; not a database nor a
    /// picture.
    pub rewrite: bool,
    /// An SQLite database, copied as a snapshot ([`snapshot`]).
    pub snapshot: bool,
}

/// Whether a file of a session's is text whose paths may be said as
/// the host's, by its name.
fn text(path: &str) -> bool {
    [".jsonl", ".json", ".md", ".txt", ".log", ".pbtxt"].iter().any(|e| path.ends_with(e))
}

/// What this computer has to copy, in the window.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Gathered {
    pub parts: Vec<Part>,
    /// This computer's home, as the paths in the files say it.
    pub home: String,
}

impl Gathered {
    pub fn sessions(&self) -> usize {
        self.parts.iter().filter(|p| p.transcript).count()
    }
}

/// No deeper than this in a session's folder.
const DEEPEST: usize = 6;

fn seconds(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// The sessions under `home` last written to within `window` (as of
/// `now`), each with its folder's files.
pub fn gather(home: &Path, window: Window, now: SystemTime) -> Gathered {
    let since = window.since(now).map(seconds);
    let mut parts = Vec::new();
    let projects = home.join(".claude/projects");
    let mut folders: Vec<String> = std::fs::read_dir(&projects).map(|d| d.flatten().filter_map(|e| e.file_name().into_string().ok()).collect()).unwrap_or_default();
    folders.sort();
    for project in folders {
        let at = projects.join(&project);
        if !std::fs::symlink_metadata(&at).is_ok_and(|m| m.is_dir()) {
            continue;
        }
        let mut names: Vec<String> = std::fs::read_dir(&at).map(|d| d.flatten().filter_map(|e| e.file_name().into_string().ok()).collect()).unwrap_or_default();
        names.sort();
        for name in names {
            let Some(id) = name.strip_suffix(".jsonl") else { continue };
            let Ok(meta) = std::fs::symlink_metadata(at.join(&name)) else { continue };
            let modified = meta.modified().map(seconds).unwrap_or(0);
            if !meta.is_file() || since.is_some_and(|since| modified < since) {
                continue;
            }
            parts.push(Part { from: at.join(&name), path: format!(".claude/projects/{project}/{name}"), size: meta.len(), modified, transcript: true, rewrite: true, snapshot: false });
            walk(&at.join(id), &format!(".claude/projects/{project}/{id}"), 0, &[], &mut parts);
        }
    }
    codex(home, since, &mut parts);
    antigravity(home, since, &mut parts);
    Gathered { parts, home: home.to_string_lossy().into_owned() }
}

/// Codex's rollouts last written to since `since`, a session each.
fn codex(home: &Path, since: Option<i64>, parts: &mut Vec<Part>) {
    fn days(dir: &Path, path: &str, depth: usize, since: Option<i64>, parts: &mut Vec<Part>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        let mut names: Vec<String> = entries.flatten().filter_map(|e| e.file_name().into_string().ok()).collect();
        names.sort();
        for name in names {
            let Ok(meta) = std::fs::symlink_metadata(dir.join(&name)) else { continue };
            let path = format!("{path}/{name}");
            if meta.is_dir() && depth < 3 {
                days(&dir.join(&name), &path, depth + 1, since, parts);
            } else if meta.is_file() && name.starts_with("rollout-") && name.ends_with(".jsonl") {
                let modified = meta.modified().map(seconds).unwrap_or(0);
                if since.is_none_or(|since| modified >= since) {
                    parts.push(Part { from: dir.join(&name), path, size: meta.len(), modified, transcript: true, rewrite: true, snapshot: false });
                }
            }
        }
    }
    days(&home.join(".codex/sessions"), ".codex/sessions", 0, since, parts);
}

/// Antigravity's conversations last written to since `since` (by the
/// database or its log, whichever was last), each with its title and
/// what it made.
fn antigravity(home: &Path, since: Option<i64>, parts: &mut Vec<Part>) {
    let base = home.join(".gemini/antigravity-cli");
    let Ok(entries) = std::fs::read_dir(base.join("conversations")) else { return };
    let mut names: Vec<String> = entries.flatten().filter_map(|e| e.file_name().into_string().ok()).filter(|n| n.ends_with(".db")).collect();
    names.sort();
    for name in names {
        let from = base.join("conversations").join(&name);
        let Ok(meta) = std::fs::symlink_metadata(&from) else { continue };
        if !meta.is_file() {
            continue;
        }
        let log = std::fs::metadata(base.join("conversations").join(format!("{name}-wal"))).ok();
        let modified = meta.modified().map(seconds).unwrap_or(0).max(log.as_ref().and_then(|m| m.modified().ok()).map(seconds).unwrap_or(0));
        if since.is_some_and(|since| modified < since) {
            continue;
        }
        let size = meta.len() + log.map_or(0, |m| m.len());
        let id = name.trim_end_matches(".db");
        parts.push(Part { from, path: format!(".gemini/antigravity-cli/conversations/{name}"), size, modified, transcript: true, rewrite: false, snapshot: true });
        let note = base.join("annotations").join(format!("{id}.pbtxt"));
        if let Ok(meta) = std::fs::metadata(&note) {
            parts.push(Part {
                from: note,
                path: format!(".gemini/antigravity-cli/annotations/{id}.pbtxt"),
                size: meta.len(),
                modified: meta.modified().map(seconds).unwrap_or(0),
                transcript: false,
                rewrite: false,
                snapshot: false,
            });
        }
        // (What it made, less its own logs, which can be large and are
        // nobody's to read there.)
        walk(&base.join("brain").join(id), &format!(".gemini/antigravity-cli/brain/{id}"), 0, &[".system_generated"], parts);
    }
}

fn walk(dir: &Path, path: &str, depth: usize, skip: &[&str], parts: &mut Vec<Part>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut names: Vec<String> = entries.flatten().filter_map(|e| e.file_name().into_string().ok()).filter(|n| !skip.contains(&n.as_str())).collect();
    names.sort();
    for name in names {
        let Ok(meta) = std::fs::symlink_metadata(dir.join(&name)) else { continue };
        let (from, path) = (dir.join(&name), format!("{path}/{name}"));
        if meta.is_dir() && depth < DEEPEST {
            walk(&from, &path, depth + 1, skip, parts);
        } else if meta.is_file() {
            let rewrite = text(&path);
            parts.push(Part { from, path, size: meta.len(), modified: meta.modified().map(seconds).unwrap_or(0), transcript: false, rewrite, snapshot: false });
        }
    }
}

/// Where a database's snapshot is made, on disk (the account's cache:
/// never /tmp, which is memory here).
fn scratch() -> PathBuf {
    let cache = std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")));
    cache.unwrap_or_else(|| PathBuf::from("/var/tmp")).join("olo-rex/sessions")
}

/// An SQLite database as it is now, whole (what its write-ahead log
/// holds in it), made from a read-only connection: SQLite's own
/// `VACUUM INTO`, into a file of `scratch` read back and removed.
pub fn snapshot(db: &Path, scratch: &Path) -> std::io::Result<Vec<u8>> {
    let other = |e: rusqlite::Error| std::io::Error::other(e.to_string());
    std::fs::create_dir_all(scratch)?;
    let into = scratch.join(format!("snapshot-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&into);
    let conn = rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX).map_err(other)?;
    conn.busy_timeout(std::time::Duration::from_secs(5)).map_err(other)?;
    conn.execute("VACUUM INTO ?1", [into.to_string_lossy().as_ref()]).map_err(other)?;
    drop(conn);
    let bytes = std::fs::read(&into);
    let _ = std::fs::remove_file(&into);
    bytes
}

/// A path as Claude Code names a project's folder: each character but
/// a letter or a digit as `-` (`/home/you/src/app` → `-home-you-src-app`).
pub fn project_name(path: &str) -> String {
    path.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

/// Where `path` (under this computer's home, `here`) goes under the
/// host's home (`there`): a project's folder named for the host's.
pub fn there_path(path: &str, here: &str, there: &str) -> String {
    let (here_name, there_name) = (project_name(here.trim_end_matches('/')), project_name(there.trim_end_matches('/')));
    let Some(rest) = path.strip_prefix(".claude/projects/") else { return path.to_owned() };
    let (project, inside) = rest.split_once('/').unwrap_or((rest, ""));
    let project = match project.strip_prefix(here_name.as_str()) {
        // (Only where the home is the whole of a part of the path: not
        // `/home/you2` for `/home/you`.)
        Some(after) if after.is_empty() || after.starts_with('-') => format!("{there_name}{after}"),
        _ => project.to_owned(),
    };
    if inside.is_empty() { format!(".claude/projects/{project}") } else { format!(".claude/projects/{project}/{inside}") }
}

/// What a transcript says of this computer's home (`here`), said as the
/// host's (`there`): the home where it is the whole of a path's start.
pub fn rewritten(bytes: Vec<u8>, here: &str, there: &str) -> Vec<u8> {
    let (here, there) = (here.trim_end_matches('/'), there.trim_end_matches('/'));
    if here == there || here.is_empty() {
        return bytes;
    }
    let (from, to) = (here.as_bytes(), there.as_bytes());
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while let Some(found) = find(&bytes[at..], from) {
        let start = at + found;
        let end = start + from.len();
        // (`/home/you` in `/home/you/src`, `"/home/you"`: not in `/home/you2`.)
        let whole = bytes.get(end).is_none_or(|b| !(b.is_ascii_alphanumeric() || *b == b'_' || *b == b'-' || *b == b'.'));
        out.extend_from_slice(&bytes[at..start]);
        out.extend_from_slice(if whole { to } else { from });
        at = end;
    }
    out.extend_from_slice(&bytes[at..]);
    out
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// The host's home, asked first (a project's folder there is named for
/// it).
pub const HOME: &str = r#"printf 'home %s\n' "$HOME""#;

/// Run as `sh -c LOOK` with the paths on its input (each under the
/// home, one a line): each one's size and time there, or `-`, a line
/// each. (Given on a shell's own input after the script, as a script
/// that reads its input is elsewhere here, the paths were the shell's
/// next lines to run: every answer read as missing, and a copy was
/// asked for again at once. Found on the rig.)
pub const LOOK: &str = r#"cd || exit 1
while IFS= read -r p; do
  if [ -f "$p" ]; then stat -c '%s %Y' -- "$p" 2> /dev/null || echo -; else echo -; fi
done"#;

/// What [`LOOK`] is given on its input: the paths, a line each.
pub fn look(paths: &[String]) -> String {
    paths.iter().map(|p| format!("{p}\n")).collect()
}

/// How a host stands, in a few numbers: what a row says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    /// Sessions with something to copy, and their bytes here.
    pub sessions: usize,
    pub bytes: u64,
    /// Sessions there as here; sessions continued there.
    pub same: usize,
    pub newer_there: usize,
}

/// How the host stands against what this computer would copy.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stands {
    /// The host's home.
    pub home: String,
    /// The parts to copy, by their place in what was gathered.
    pub to_copy: Vec<usize>,
    /// Sessions with something to copy, and their bytes here.
    pub sessions: usize,
    pub bytes: u64,
    /// Sessions there as they are here.
    pub same: usize,
    /// Sessions continued there since (newer there): left as they are.
    pub newer_there: usize,
}

impl Stands {
    pub fn summary(&self) -> Summary {
        Summary { sessions: self.sessions, bytes: self.bytes, same: self.same, newer_there: self.newer_there }
    }

    /// What the host answered [`look`] for `gathered`'s parts (its paths
    /// as they are there). None: it didn't answer for each.
    pub fn read(home: &str, output: &str, gathered: &Gathered) -> Option<Stands> {
        let lines: Vec<&str> = output.lines().collect();
        if lines.len() < gathered.parts.len() {
            return None;
        }
        let mut stands = Stands { home: home.to_owned(), ..Stands::default() };
        let (mut open, mut session_copies, mut session_newer) = (false, false, false);
        for (i, (part, line)) in gathered.parts.iter().zip(lines).enumerate() {
            let there: Option<(u64, i64)> = line.split_once(' ').and_then(|(size, time)| Some((size.trim().parse().ok()?, time.trim().parse().ok()?)));
            if part.transcript {
                // (The session before it done: counted.)
                if open {
                    count(&mut stands, session_copies, session_newer);
                }
                open = true;
                session_copies = false;
                session_newer = there.is_some_and(|(_, time)| time > part.modified);
            }
            let copy = match there {
                None => true,
                Some((_, time)) => time < part.modified,
            };
            if copy && !session_newer {
                stands.to_copy.push(i);
                stands.bytes += part.size;
                session_copies = true;
            }
        }
        if open {
            count(&mut stands, session_copies, session_newer);
        }
        Some(stands)
    }
}

fn count(stands: &mut Stands, copies: bool, newer: bool) {
    match (copies, newer) {
        (_, true) => stands.newer_there += 1,
        (true, false) => stands.sessions += 1,
        (false, false) => stands.same += 1,
    }
}

/// Run on the host with the archive on its input: each file in its
/// place under the home, with its time here, readable by the account
/// alone.
pub const PUT: &str = r#"umask 077; cd && gzip -dc | tar -xf - --no-same-owner || exit 1
f=.codex/session_index.olo-new.jsonl
if [ -f "$f" ]; then
  touch .codex/session_index.jsonl
  while IFS= read -r l; do
    id=$(printf '%s\n' "$l" | sed -n 's/^{"id":"\([0-9a-f-]*\)".*/\1/p')
    [ -n "$id" ] && ! grep -qF "\"id\":\"$id\"" .codex/session_index.jsonl && printf '%s\n' "$l" >> .codex/session_index.jsonl
  done < "$f"
  rm -f "$f"
fi
exit 0"#;

/// The archive of `parts` for a host whose home is `there`, made as it
/// is written to `out` (a gzip'd tar), `sent` told how many bytes of the
/// files have gone in, and stopped (with an error) once `stop` is set.
/// Each file is read as it is now, a transcript up to its last whole
/// line (one being written to is taken as far as it was written), its
/// time with it.
pub fn stream(parts: &[Part], here: &str, there: &str, out: impl Write, sent: &mut dyn FnMut(u64), stop: &AtomicBool) -> std::io::Result<()> {
    let gz = flate2::write::GzEncoder::new(out, flate2::Compression::fast());
    let mut tar = tar::Builder::new(gz);
    for part in parts {
        if stop.load(Ordering::Relaxed) {
            return Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "stopped"));
        }
        let (bytes, modified) = if part.snapshot {
            let Ok(bytes) = snapshot(&part.from, &scratch()) else { continue };
            (bytes, part.modified)
        } else {
            let mut bytes = Vec::with_capacity(part.size as usize);
            let Ok(mut file) = std::fs::File::open(&part.from) else { continue };
            let modified = file.metadata().ok().and_then(|m| m.modified().ok()).map(seconds).unwrap_or(part.modified);
            file.read_to_end(&mut bytes)?;
            if part.path.ends_with(".jsonl")
                && let Some(last) = bytes.iter().rposition(|b| *b == b'\n')
            {
                bytes.truncate(last + 1);
            }
            (bytes, modified)
        };
        let read = bytes.len() as u64;
        let bytes = if part.rewrite { rewritten(bytes, here, there) } else { bytes };
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mtime(modified.max(0) as u64);
        header.set_mode(0o600);
        header.set_entry_type(tar::EntryType::Regular);
        tar.append_data(&mut header, there_path(&part.path, here, there), bytes.as_slice())?;
        sent(read);
    }
    // (Codex's: the names of those copied, from this computer's index,
    // for [`PUT`] to put beside the host's.)
    let ids = codex_ids(parts);
    if !ids.is_empty() {
        let index = std::fs::read_to_string(Path::new(here).join(".codex/session_index.jsonl")).unwrap_or_default();
        let lines: String = index.lines().filter(|l| ids.iter().any(|id| l.contains(&format!("\"id\":\"{id}\"")))).map(|l| format!("{l}\n")).collect();
        if !lines.is_empty() {
            let mut header = tar::Header::new_gnu();
            header.set_size(lines.len() as u64);
            header.set_mode(0o600);
            header.set_mtime(seconds(SystemTime::now()) as u64);
            header.set_entry_type(tar::EntryType::Regular);
            tar.append_data(&mut header, INDEX_NEW, lines.as_bytes())?;
        }
    }
    tar.into_inner()?.finish()?.flush()
}

/// Where the names of Codex's sessions copied wait to be put beside
/// the host's own ([`PUT`]).
const INDEX_NEW: &str = ".codex/session_index.olo-new.jsonl";

/// The ids of the Codex sessions among `parts` (a rollout's file name
/// ends with its id).
pub fn codex_ids(parts: &[Part]) -> Vec<String> {
    parts
        .iter()
        .filter(|p| p.path.starts_with(".codex/sessions/"))
        .filter_map(|p| p.path.strip_suffix(".jsonl"))
        .filter_map(|p| p.len().checked_sub(36).map(|at| &p[at..]))
        .filter(|id| id.len() == 36 && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-'))
        .map(str::to_owned)
        .collect()
}

/// Run on the host after a copy that had Codex's sessions in it, given
/// what [`register`] says on its input: Codex's own server, which adds
/// each session asked for by its id to its database (none there: it
/// reads them all as it makes it). Nothing where Codex isn't there.
pub const REGISTER: &str = r#"PATH="$HOME/.local/bin:$PATH"; command -v codex > /dev/null 2>&1 || exit 0; exec codex app-server"#;

/// What [`REGISTER`] is asked: its JSON-RPC, a line each, the answers to
/// look for being `ids`'s (10 on).
pub fn register(ids: &[String]) -> String {
    let mut out = String::from(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"clientInfo":{"name":"olo-rex","version":"1"}}}"#);
    out.push('\n');
    out.push_str(r#"{"jsonrpc":"2.0","method":"initialized"}"#);
    out.push('\n');
    for (i, id) in ids.iter().enumerate() {
        out.push_str(&serde_json::json!({"jsonrpc": "2.0", "id": 10 + i, "method": "thread/read", "params": {"threadId": id, "includeTurns": false}}).to_string());
        out.push('\n');
    }
    out
}

/// Where [`stream`] writes: into chunks for [`crate::SshSessionPool::put_stream`],
/// a quarter of a megabyte each, waiting while the link is behind (a
/// few chunks are held at most). Gone, the archive's making ends.
pub struct Chunks {
    to: tokio::sync::mpsc::Sender<Vec<u8>>,
    pending: Vec<u8>,
}

impl Chunks {
    const SIZE: usize = 256 << 10;

    /// The writer, and what the put reads.
    pub fn new() -> (Chunks, tokio::sync::mpsc::Receiver<Vec<u8>>) {
        let (to, from) = tokio::sync::mpsc::channel(4);
        (Chunks { to, pending: Vec::with_capacity(Self::SIZE) }, from)
    }

    fn send(&mut self) -> std::io::Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let chunk = std::mem::replace(&mut self.pending, Vec::with_capacity(Self::SIZE));
        self.to.blocking_send(chunk).map_err(|_| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "the copy ended"))
    }
}

impl Write for Chunks {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.pending.extend_from_slice(bytes);
        if self.pending.len() >= Self::SIZE {
            self.send()?;
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.send()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = PathBuf::from(format!("/var/tmp/olo-rex-sessions-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write(home: &Path, path: &str, text: &str, days_ago: u64) {
        let full = home.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(&full, text).unwrap();
        let t = SystemTime::now() - std::time::Duration::from_secs(days_ago * 86_400 + 60);
        std::fs::File::options().write(true).open(&full).unwrap().set_modified(t).unwrap();
    }

    #[test]
    fn the_window_and_a_setting() {
        assert_eq!(Window::default(), Window::Week);
        for w in Window::ALL {
            assert_eq!(Window::of_days(w.to_days()), w);
        }
        assert_eq!(Window::of_days(14), Window::Month);
    }

    /// Those last written to within the window, each with its folder;
    /// another project's too; nothing else of Claude Code's.
    #[test]
    fn sessions_used_within_the_window_are_gathered() {
        let home = scratch("gather");
        let h = home.to_string_lossy().into_owned();
        let p = format!(".claude/projects/{}-src-app", project_name(&h));
        write(&home, &format!("{p}/aaaa.jsonl"), "{\"cwd\":\"x\"}\n", 0);
        write(&home, &format!("{p}/aaaa/subagents/agent-1.jsonl"), "{}\n", 20);
        write(&home, &format!("{p}/aaaa/tool-results/t1.txt"), "out", 0);
        write(&home, &format!("{p}/bbbb.jsonl"), "{}\n", 10);
        write(&home, &format!("{p}/memory/MEMORY.md"), "not a session", 0);
        write(&home, ".claude/projects/-other/cccc.jsonl", "{}\n", 2);
        write(&home, ".claude/history.jsonl", "{}\n", 0);
        let now = SystemTime::now();
        let week = gather(&home, Window::Week, now);
        let paths: Vec<&str> = week.parts.iter().map(|p| p.path.strip_prefix(".claude/projects/").unwrap()).collect();
        let app = p.strip_prefix(".claude/projects/").unwrap();
        assert_eq!(
            paths,
            ["-other/cccc.jsonl".to_owned(), format!("{app}/aaaa.jsonl"), format!("{app}/aaaa/subagents/agent-1.jsonl"), format!("{app}/aaaa/tool-results/t1.txt")],
            "a session's folder goes with it whatever its own times; not one used ten days ago, nor what isn't a session"
        );
        assert_eq!(week.sessions(), 2);
        assert_eq!(gather(&home, Window::Day, now).sessions(), 1);
        assert_eq!(gather(&home, Window::All, now).sessions(), 3);
        let _ = std::fs::remove_dir_all(&home);
    }

    /// Said as the host's: a project's folder, and the paths in what a
    /// transcript says, where the home is the whole of a path's start.
    #[test]
    fn the_home_is_said_as_the_hosts() {
        assert_eq!(project_name("/home/you/.config/my_app"), "-home-you--config-my-app");
        let p = ".claude/projects/-home-you-src-app/aaaa/t.txt";
        assert_eq!(there_path(p, "/home/you", "/home/far"), ".claude/projects/-home-far-src-app/aaaa/t.txt");
        assert_eq!(there_path(".claude/projects/-home-you/x.jsonl", "/home/you", "/root"), ".claude/projects/-root/x.jsonl");
        assert_eq!(there_path(".claude/projects/-home-you2-src/x.jsonl", "/home/you", "/home/far"), ".claude/projects/-home-you2-src/x.jsonl", "another home");
        assert_eq!(there_path(".claude/projects/-srv-app/x.jsonl", "/home/you", "/home/far"), ".claude/projects/-srv-app/x.jsonl", "not under the home");
        let said = br#"{"cwd":"/home/you/src/app","file":"/home/you","other":"/home/you2/x","also":"/home/you.old"}"#.to_vec();
        assert_eq!(rewritten(said.clone(), "/home/you", "/home/far"), br#"{"cwd":"/home/far/src/app","file":"/home/far","other":"/home/you2/x","also":"/home/you.old"}"#.to_vec());
        assert_eq!(rewritten(said.clone(), "/home/you", "/home/you/"), said, "the same home: as it is");
    }

    /// What is missing or older there is copied; what is the same isn't;
    /// a session continued there isn't touched.
    #[test]
    fn what_the_host_has_decides_what_is_copied() {
        let part = |path: &str, transcript: bool, modified: i64| Part { from: PathBuf::new(), path: path.into(), size: 10, modified, transcript, rewrite: true, snapshot: false };
        let gathered = Gathered {
            parts: vec![part("a.jsonl", true, 100), part("a/t.txt", false, 100), part("b.jsonl", true, 200), part("c.jsonl", true, 300), part("c/t.txt", false, 300), part("d.jsonl", true, 400)],
            home: "/home/you".into(),
        };
        // a: the same there; b: older there; c: newer there (continued); d: missing.
        let output = "10 100\n10 100\n8 150\n12 350\n10 300\n-\n";
        let s = Stands::read("/home/far", output, &gathered).unwrap();
        assert_eq!((s.to_copy.as_slice(), s.sessions, s.same, s.newer_there, s.bytes), ([2, 5].as_slice(), 2, 1, 1, 20));
        assert_eq!(Stands::read("/home/far", "-\n", &gathered), None, "an answer short of the paths");
    }

    /// Codex's rollouts by when each was last written to, a session
    /// each, its id from its name; Antigravity's conversations by their
    /// database or its log, each with its title and what it made, less
    /// its logs.
    #[test]
    fn codex_and_antigravity_sessions_are_gathered() {
        let home = scratch("agents");
        let id = "019a1b2c-3d4e-7f60-8a9b-0c1d2e3f4a5b";
        write(&home, &format!(".codex/sessions/2026/11/20/rollout-2026-11-20T10-00-00-{id}.jsonl"), "{}\n", 2);
        write(&home, ".codex/sessions/2026/10/01/rollout-2026-10-01T10-00-00-019a0000-0000-7000-8000-000000000000.jsonl", "{}\n", 40);
        write(&home, ".codex/session_index.jsonl", "{}\n", 0);
        let conv = "c0ffee00-1111-2222-3333-444455556666";
        write(&home, &format!(".gemini/antigravity-cli/conversations/{conv}.db"), "not really", 3);
        write(&home, &format!(".gemini/antigravity-cli/annotations/{conv}.pbtxt"), "title: \"x\"\n", 3);
        write(&home, &format!(".gemini/antigravity-cli/brain/{conv}/plan.md"), "# plan\n", 3);
        write(&home, &format!(".gemini/antigravity-cli/brain/{conv}/.system_generated/logs/a.log"), "noise", 3);
        let week = gather(&home, Window::Week, SystemTime::now());
        let paths: Vec<&str> = week.parts.iter().map(|p| p.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                format!(".codex/sessions/2026/11/20/rollout-2026-11-20T10-00-00-{id}.jsonl"),
                format!(".gemini/antigravity-cli/conversations/{conv}.db"),
                format!(".gemini/antigravity-cli/annotations/{conv}.pbtxt"),
                format!(".gemini/antigravity-cli/brain/{conv}/plan.md")
            ]
        );
        assert_eq!(week.sessions(), 2);
        assert_eq!(codex_ids(&week.parts), [id]);
        let db = &week.parts[1];
        assert!(db.snapshot && !db.rewrite && db.transcript);
        assert!(week.parts[3].rewrite, "a plan, in text");
        let _ = std::fs::remove_dir_all(&home);
    }

    /// A database in use, its latest in its write-ahead log: its
    /// snapshot holds all of it, whole.
    #[test]
    fn a_database_in_use_is_copied_whole() {
        let home = scratch("snapshot");
        let db = home.join("c.db");
        let writer = rusqlite::Connection::open(&db).unwrap();
        writer.pragma_update(None, "journal_mode", "WAL").unwrap();
        writer.pragma_update(None, "wal_autocheckpoint", 0).unwrap();
        writer.execute_batch("CREATE TABLE steps (n INTEGER); INSERT INTO steps VALUES (1), (2), (3);").unwrap();
        assert!(home.join("c.db-wal").metadata().unwrap().len() > 0, "the rows only in the log");
        let bytes = snapshot(&db, &home.join("scratch")).unwrap();
        let copy = home.join("copy.db");
        std::fs::write(&copy, bytes).unwrap();
        let read = rusqlite::Connection::open(&copy).unwrap();
        assert_eq!(read.query_row("SELECT count(*) FROM steps", [], |r| r.get::<_, i64>(0)).unwrap(), 3);
        assert!(!home.join("scratch").read_dir().unwrap().any(|_| true), "nothing left behind");
        drop(writer);
        let _ = std::fs::remove_dir_all(&home);
    }

    /// On the host: the archive in its place, and the names of Codex's
    /// sessions copied put beside the host's own, none twice.
    #[test]
    fn codexs_names_are_put_beside_the_hosts() {
        let here = scratch("put-here");
        let there = scratch("put-there");
        let (a, b) = ("019a1b2c-3d4e-7f60-8a9b-0c1d2e3f4a5b", "019a1b2c-3d4e-7f60-8a9b-0c1d2e3f4a5c");
        write(&here, &format!(".codex/sessions/2026/11/20/rollout-x-{a}.jsonl"), "{}\n", 1);
        write(&here, &format!(".codex/sessions/2026/11/20/rollout-y-{b}.jsonl"), "{}\n", 1);
        write(
            &here,
            ".codex/session_index.jsonl",
            &format!("{{\"id\":\"{a}\",\"thread_name\":\"A\",\"updated_at\":\"t\"}}\n{{\"id\":\"{b}\",\"thread_name\":\"B\",\"updated_at\":\"t\"}}\n{{\"id\":\"other\",\"thread_name\":\"O\"}}\n"),
            1,
        );
        write(&there, ".codex/session_index.jsonl", &format!("{{\"id\":\"{a}\",\"thread_name\":\"A there\",\"updated_at\":\"t\"}}\n"), 1);
        let g = gather(&here, Window::Week, SystemTime::now());
        let (h, t) = (here.to_string_lossy().into_owned(), there.to_string_lossy().into_owned());
        let mut archive = Vec::new();
        stream(&g.parts, &h, &t, &mut archive, &mut |_| {}, &AtomicBool::new(false)).unwrap();
        let mut sh = std::process::Command::new("sh").arg("-c").arg(PUT).env("HOME", &there).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).spawn().unwrap();
        sh.stdin.take().unwrap().write_all(&archive).unwrap();
        assert!(sh.wait_with_output().unwrap().status.success());
        let index = std::fs::read_to_string(there.join(".codex/session_index.jsonl")).unwrap();
        assert_eq!(
            index,
            format!("{{\"id\":\"{a}\",\"thread_name\":\"A there\",\"updated_at\":\"t\"}}\n{{\"id\":\"{b}\",\"thread_name\":\"B\",\"updated_at\":\"t\"}}\n"),
            "the host's own kept, the one new added, none other"
        );
        assert!(there.join(format!(".codex/sessions/2026/11/20/rollout-y-{b}.jsonl")).exists());
        assert!(!there.join(INDEX_NEW).exists());
        // And what the host's Codex is asked: each by its id, after it is ready.
        let asked: Vec<serde_json::Value> = register(&codex_ids(&g.parts)).lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!((asked[0]["method"].as_str(), asked[1]["method"].as_str()), (Some("initialize"), Some("initialized")));
        assert_eq!((asked[2]["id"].as_u64(), asked[2]["params"]["threadId"].as_str()), (Some(10), Some(a)));
        assert_eq!(asked.len(), 4);
        let _ = std::fs::remove_dir_all(&here);
        let _ = std::fs::remove_dir_all(&there);
    }

    /// The look as the host runs it (`sh -c`, the paths on its input):
    /// each file's size and time, or `-`, a line each, whatever is in a
    /// path.
    #[test]
    fn the_look_says_each_files_size_and_time() {
        let home = scratch("look");
        write(&home, "a b/it's.jsonl", "12345", 1);
        let paths = vec!["a b/it's.jsonl".to_owned(), "missing.jsonl".to_owned(), "$(touch x)".to_owned()];
        let mut sh = std::process::Command::new("sh").arg("-c").arg(LOOK).env("HOME", &home).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).spawn().unwrap();
        sh.stdin.take().unwrap().write_all(look(&paths).as_bytes()).unwrap();
        let out = String::from_utf8(sh.wait_with_output().unwrap().stdout).unwrap();
        let time = std::fs::metadata(home.join("a b/it's.jsonl")).unwrap().modified().map(seconds).unwrap();
        assert_eq!(out, format!("5 {time}\n-\n-\n"));
        assert!(!home.join("x").exists(), "a path is never run");
        let _ = std::fs::remove_dir_all(&home);
    }

    /// The archive: each file at its place there, its time with it, the
    /// home said as the host's; a transcript up to its last whole line.
    #[test]
    fn the_archive_has_each_file_as_the_host_has_it() {
        let home = scratch("archive");
        let h = home.to_string_lossy().into_owned();
        let p = format!(".claude/projects/{}-src-app", project_name(&h));
        write(&home, &format!("{p}/aaaa.jsonl"), &format!("{{\"cwd\":\"{h}/src/app\"}}\n{{\"half\":"), 1);
        write(&home, &format!("{p}/aaaa/tool-results/t1.txt"), "out", 1);
        let gathered = gather(&home, Window::Week, SystemTime::now());
        let mut out = Vec::new();
        let mut sent = 0;
        stream(&gathered.parts, &h, "/home/far", &mut out, &mut |n| sent += n, &AtomicBool::new(false)).unwrap();
        let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(out.as_slice()));
        let mut seen = Vec::new();
        for entry in tar.entries().unwrap() {
            let mut entry = entry.unwrap();
            let mut text = String::new();
            entry.read_to_string(&mut text).unwrap();
            seen.push((entry.path().unwrap().to_string_lossy().into_owned(), text, entry.header().mtime().unwrap() as i64));
        }
        assert_eq!(seen[0].0, ".claude/projects/-home-far-src-app/aaaa.jsonl");
        assert_eq!(seen[0].1, "{\"cwd\":\"/home/far/src/app\"}\n", "rewritten, and only its whole lines");
        assert_eq!(seen[0].2, gathered.parts[0].modified);
        assert_eq!((seen[1].0.as_str(), seen[1].1.as_str()), (".claude/projects/-home-far-src-app/aaaa/tool-results/t1.txt", "out"));
        assert_eq!(sent, (format!("{{\"cwd\":\"{h}/src/app\"}}\n").len() + 3) as u64);
        // Stopped: an error, nothing more made.
        assert!(stream(&gathered.parts, &h, "/home/far", &mut Vec::new(), &mut |_| {}, &AtomicBool::new(true)).is_err());
        let _ = std::fs::remove_dir_all(&home);
    }
}
