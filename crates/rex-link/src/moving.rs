//! `olo-rex-link move …`: the helper's part in moving an account from one
//! host to another (Ben, 2026-11-25: "machine to machine copy (don't go
//! via the user's desktop)"), driven by the app (`olo_rex_client::
//! migrate`). The same program on Ubuntu, Fedora or anything else (it is
//! static), so nothing of the host's is needed for it: no rsync, tar or
//! zstd (Fedora's cloud image has none of them), no ssh-keygen.
//!
//! - `move key DIR`: a key of the move's own (ed25519, made here) in the
//!   move's folder on the source, its public line printed.
//! - `move allow ID LINE` / `move disallow ID`: that key let into the
//!   destination's account (`~/.ssh/authorized_keys`) for a day, able to
//!   run nothing but this helper's `move receive`; and taken out again.
//! - `move receive`: what that key runs (its forced command): the one
//!   thing asked of it (`SSH_ORIGINAL_COMMAND`), from a short list.
//! - `move send DIR --to USER@HOST[:PORT] --host-key KEY --there ROOT
//!   [--pass N] [--exclude PATTERN]… [--detach]`: the copy, from the
//!   home here to ROOT there, over an SSH connection of its own with the
//!   move's key, the destination's host key pinned (`KEY`, as the app's
//!   own connection checked it: never trusted on first sight); detached,
//!   it goes on with the app gone (its log, pid and `status.json` in DIR).
//!   A second pass sends what changed and takes away there what the first
//!   put there and is gone here.
//! - `move status DIR`: how the copy in the move's folder stands.
//! - `move look`: what this host and account are, for a move (JSON).
//! - `move fix --from OLD --to NEW [--once FILE]`: a moved home made right
//!   where its path changed (`crate::fix`).
//! - `move look-pane PID…`: each pane's programs, its whole process
//!   tree (command lines whole, folders, start times, the agents' session
//!   files open), JSON: what Rex's own report of a pane leaves out.
//! - `move stop [--spare PID…]`: every process of the account stopped but
//!   this one's own line of parents and the account's own systemd.

use std::collections::HashSet;
use std::io::{self, Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use russh::keys::ssh_key::{LineEnding, PrivateKey, private::Ed25519Keypair};

const USAGE: &str = "usage: olo-rex-link move look\n       olo-rex-link move reach DIR --to USER@HOST[:PORT] --host-key KEY --there ROOT\n       olo-rex-link move look-pane PID...\n       olo-rex-link move key DIR\n       olo-rex-link move send DIR --to USER@HOST[:PORT] --host-key KEY --there ROOT [--pass N] [--exclude PATTERN]... [--detach]\n       olo-rex-link move allow ID PUBLIC-KEY-LINE\n       olo-rex-link move disallow ID\n       olo-rex-link move receive   (a move's key's forced command)\n       olo-rex-link move status DIR\n       olo-rex-link move stop [--spare PID]…";

pub fn run(args: Vec<String>) -> ! {
    let code = match args.first().map(String::as_str) {
        Some("key") => key(&args[1..]),
        Some("send") => send(&args[1..]),
        Some("reach") => reach(&args[1..]),
        Some("allow") => allow(&args[1..]),
        Some("disallow") => disallow(&args[1..]),
        Some("receive") => receive(),
        Some("status") => status(&args[1..]),
        Some("stop") => stop(&args[1..]),
        Some("look") => look(),
        Some("look-pane") => look_pane(&args[1..]),
        Some("fix") => crate::fix::run(&args[1..]),
        _ => {
            eprintln!("{USAGE}");
            2
        }
    };
    std::process::exit(code)
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

/// A move's name: what its key's comment and its folder are named by.
fn named(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// The comment a move's key carries, by which it is found and taken out.
fn comment(id: &str) -> String {
    format!("olo-move-{id}")
}

/// Bytes from the kernel's generator.
fn random(buf: &mut [u8]) -> io::Result<()> {
    let mut at = 0;
    while at < buf.len() {
        // SAFETY: writes at most `buf.len() - at` bytes into `buf[at..]`.
        let n = unsafe { libc::getrandom(buf[at..].as_mut_ptr().cast(), buf.len() - at, 0) };
        if n < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(e);
        }
        at += n as usize;
    }
    Ok(())
}

/// A file written whole beside its place, then put there: never half of
/// it where it belongs.
fn write_whole(path: &Path, bytes: &[u8], mode: u32) -> io::Result<()> {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let new = path.with_file_name(format!(".{name}.olo-move-new"));
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(mode).open(&new)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    std::fs::set_permissions(&new, std::fs::Permissions::from_mode(mode))?;
    std::fs::rename(&new, path)
}

/// `move key DIR`: the move's key, made once (a second ask prints the same).
fn key(args: &[String]) -> i32 {
    let [dir] = args else {
        eprintln!("{USAGE}");
        return 2;
    };
    let dir = PathBuf::from(dir);
    let id = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_owned();
    if !named(&id) {
        eprintln!("olo-rex-link move key: the move's folder is named by its id (letters, digits, dashes)");
        return 2;
    }
    let made = (|| -> io::Result<String> {
        std::fs::create_dir_all(&dir)?;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
        let (private, public) = (dir.join("key"), dir.join("key.pub"));
        if let Ok(line) = std::fs::read_to_string(&public)
            && private.exists()
        {
            return Ok(line.trim().to_owned());
        }
        let mut seed = [0u8; 32];
        random(&mut seed)?;
        let mut key = PrivateKey::from(Ed25519Keypair::from_seed(&seed));
        seed.fill(0);
        key.set_comment(comment(&id));
        let text = key.to_openssh(LineEnding::LF).map_err(io::Error::other)?;
        write_whole(&private, text.as_bytes(), 0o600)?;
        let line = key.public_key().to_openssh().map_err(io::Error::other)?;
        write_whole(&public, format!("{line}\n").as_bytes(), 0o644)?;
        Ok(line)
    })();
    match made {
        Ok(line) => {
            println!("{line}");
            0
        }
        Err(e) => {
            eprintln!("olo-rex-link move key: {e}");
            1
        }
    }
}

/// The account's `authorized_keys`, and the folder it is in.
fn authorized() -> (PathBuf, PathBuf) {
    let ssh = home().join(".ssh");
    (ssh.join("authorized_keys"), ssh)
}

/// `authorized_keys` less the move's own lines (by their comment), and
/// with `line` after them if given: each line whole, the last ended.
fn keys_with(text: &str, id: &str, line: Option<&str>) -> String {
    let mine = comment(id);
    let mut out: String = text.lines().filter(|l| l.split_whitespace().last() != Some(mine.as_str())).flat_map(|l| [l, "\n"]).collect();
    if let Some(line) = line {
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// A day from now, as `authorized_keys`' `expiry-time` has it (the
/// host's own time).
fn tomorrow() -> String {
    // SAFETY: `time` and `localtime_r` with a valid pointer to fill.
    unsafe {
        let now = libc::time(std::ptr::null_mut()) + 24 * 3600;
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&now, &mut tm);
        format!("{:04}{:02}{:02}{:02}{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min)
    }
}

/// The line that lets the move's key in: for a day, nothing but this
/// helper's `move receive` (no terminal, no forwarding of any kind).
fn allowed_line(public: &str, id: &str, helper: &Path, expiry: &str) -> Option<String> {
    let mut words = public.split_whitespace();
    let (kind, body) = (words.next()?, words.next()?);
    if kind != "ssh-ed25519" || body.is_empty() || !body.bytes().all(|b| b.is_ascii_alphanumeric() || b"+/=".contains(&b)) {
        return None;
    }
    let helper = helper.to_str()?;
    if helper.contains(['"', '\\', '\n']) {
        return None;
    }
    Some(format!("restrict,expiry-time=\"{expiry}\",command=\"{helper} move receive\" {kind} {body} {}", comment(id)))
}

/// `move allow ID LINE`: the move's key let in.
fn allow(args: &[String]) -> i32 {
    let [id, public] = args else {
        eprintln!("{USAGE}");
        return 2;
    };
    let Ok(helper) = std::env::current_exe() else {
        eprintln!("olo-rex-link move allow: where this helper is isn't known");
        return 1;
    };
    let Some(line) = named(id).then(|| allowed_line(public, id, &helper, &tomorrow())).flatten() else {
        eprintln!("olo-rex-link move allow: a move's id and an ed25519 key's public line");
        return 2;
    };
    match edit_keys(id, Some(&line)) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("olo-rex-link move allow: {e}");
            1
        }
    }
}

/// `move disallow ID`: the move's key taken out (nothing, if it isn't in).
fn disallow(args: &[String]) -> i32 {
    let [id] = args else {
        eprintln!("{USAGE}");
        return 2;
    };
    if !named(id) {
        eprintln!("{USAGE}");
        return 2;
    }
    match edit_keys(id, None) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("olo-rex-link move disallow: {e}");
            1
        }
    }
}

fn edit_keys(id: &str, line: Option<&str>) -> io::Result<()> {
    let (file, dir) = authorized();
    let text = match std::fs::read_to_string(&file) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            if line.is_none() {
                return Ok(());
            }
            String::new()
        }
        Err(e) => return Err(e),
    };
    let edited = keys_with(&text, id, line);
    if edited == text {
        return Ok(());
    }
    if !dir.exists() {
        std::fs::create_dir_all(&dir)?;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    // (Its own mode kept, 0600 for a new one: sshd refuses one others
    // may write.)
    let mode = std::fs::metadata(&file).map(|m| m.permissions().mode() & 0o777).unwrap_or(0o600) & !0o022;
    write_whole(&file, edited.as_bytes(), mode)
}

/// What a move never writes in the destination's home, whatever the sender
/// sends (the sender leaves them out too): the account's keys (the source's
/// would lock the destination's out), its helper, the move's own folder.
const KEPT: &[&str] = &[".ssh/authorized_keys", ".ssh/authorized_keys2", ".local/share/olo/rex-link", ".local/state/olo-rex/move"];

/// What a move's key may ask (`move receive`), hex-coded JSON as the last
/// word of its command (`olo-rex-link move do HEX`: no shell between).
#[derive(Debug, PartialEq, Eq)]
enum Asked {
    /// Can it be reached, and by which helper.
    Hello,
    /// The engine's receiving side into `root` (under the home, or the
    /// account's own folder in `/var/tmp`), with these of its options.
    Sync { root: String, options: Vec<String> },
    /// The paths named on the input (NUL after each, under `root`) taken
    /// away: the second pass's, of what the first put there and the
    /// source no longer has.
    Forget { root: String },
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    (s.len().is_multiple_of(2)).then_some(())?;
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

/// The ask in a command's last word (`SSH_ORIGINAL_COMMAND`).
fn asked(command: &str) -> Option<Asked> {
    let word = command.split_whitespace().last()?;
    let v: serde_json::Value = serde_json::from_slice(&unhex(word)?).ok()?;
    let root = || v["root"].as_str().map(str::to_owned);
    match v["op"].as_str()? {
        "hello" => Some(Asked::Hello),
        "sync" => {
            let options: Vec<String> = v["options"].as_array()?.iter().map(|o| o.as_str().map(str::to_owned)).collect::<Option<_>>()?;
            // (Only what keeps the home's own files: never `--delete`.)
            let mut it = options.iter();
            while let Some(o) = it.next() {
                match o.as_str() {
                    "--scoped" | "--keep-newer" | "--no-cache" => {}
                    "--exclude" => {
                        it.next()?;
                    }
                    _ => return None,
                }
            }
            Some(Asked::Sync { root: root()?, options })
        }
        "forget" => Some(Asked::Forget { root: root()? }),
        _ => None,
    }
}

/// Whether `root` is somewhere a move may write: the home, under it, or
/// the account's own folder in `/var/tmp`.
fn writable_root(root: &str, home: &Path, uid: u32) -> bool {
    let path = Path::new(root);
    if !path.is_absolute() || path.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return false;
    }
    if path.starts_with(home) {
        return true;
    }
    // (/var/tmp/NAME, the account's: an agent's scratch carried over.)
    let mut parts = path.strip_prefix("/var/tmp").ok().map(|p| p.components());
    let top = parts.as_mut().and_then(|c| c.next());
    top.is_some_and(|t| {
        let first = Path::new("/var/tmp").join(t);
        std::fs::symlink_metadata(&first).map_or(true, |m| m.is_dir() && std::os::unix::fs::MetadataExt::uid(&m) == uid)
    })
}

/// `move receive`: what the move's key runs.
fn receive() -> i32 {
    let Ok(command) = std::env::var("SSH_ORIGINAL_COMMAND") else {
        eprintln!("olo-rex-link move receive: only as a move's key's command");
        return 2;
    };
    let uid = unsafe { libc::getuid() };
    match asked(&command) {
        Some(Asked::Hello) => {
            println!("{{\"ok\":true,\"helper\":\"{}\"}}", olo_rex_link::VERSION);
            0
        }
        Some(Asked::Sync { root, options }) if writable_root(&root, &home(), uid) => {
            // (What the destination keeps as its own, whatever the sender
            // sends: its keys, its helper, the move's own folder.)
            let mut args = vec!["receive".to_owned(), root.clone()];
            args.extend(options);
            if Path::new(&root) == home() {
                for p in KEPT {
                    args.extend(["--protect".to_owned(), (*p).to_owned()]);
                }
            }
            crate::sync::run(args)
        }
        Some(Asked::Forget { root }) if writable_root(&root, &home(), uid) => forget(Path::new(&root)),
        _ => {
            eprintln!("olo-rex-link move receive: not something a move's key may ask");
            2
        }
    }
}

/// `forget`: each path on the input (NUL after each, under `root`) taken
/// away, a folder only if it is empty; deepest first.
fn forget(root: &Path) -> i32 {
    let mut input = Vec::new();
    if io::stdin().read_to_end(&mut input).is_err() {
        return 1;
    }
    let mut paths: Vec<&[u8]> = input.split(|&b| b == 0).filter(|p| !p.is_empty()).collect();
    paths.sort_by_key(|p| std::cmp::Reverse(p.iter().filter(|&&b| b == b'/').count()));
    let (mut gone, mut kept) = (0u64, 0u64);
    for p in paths {
        let rel = Path::new(std::ffi::OsStr::from_bytes_compat(p));
        if rel.is_absolute() || rel.components().any(|c| !matches!(c, std::path::Component::Normal(_))) {
            kept += 1;
            continue;
        }
        let path = root.join(rel);
        let done = match std::fs::symlink_metadata(&path) {
            Ok(m) if m.is_dir() => std::fs::remove_dir(&path).is_ok(),
            Ok(_) => std::fs::remove_file(&path).is_ok(),
            Err(e) => e.kind() == io::ErrorKind::NotFound,
        };
        if done {
            gone += 1;
        } else {
            kept += 1;
        }
    }
    println!("{{\"gone\":{gone},\"kept\":{kept}}}");
    0
}

trait FromBytes {
    fn from_bytes_compat(b: &[u8]) -> &Self;
}

impl FromBytes for std::ffi::OsStr {
    fn from_bytes_compat(b: &[u8]) -> &Self {
        std::os::unix::ffi::OsStrExt::from_bytes(b)
    }
}

/// What `move send` was asked.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Sending {
    dir: PathBuf,
    user: String,
    host: String,
    port: u16,
    host_key: String,
    there: String,
    pass: u32,
    exclude: Vec<String>,
    detach: bool,
}

fn sending(args: &[String]) -> Option<Sending> {
    let mut s = Sending { port: 22, pass: 1, ..Sending::default() };
    let mut it = args.iter();
    s.dir = PathBuf::from(it.next()?);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--to" => {
                let to = it.next()?;
                let (user, at) = to.split_once('@')?;
                let (host, port) = match at.rsplit_once(':') {
                    Some((h, p)) if !h.ends_with(']') || h.starts_with('[') => (h, p.parse().ok()?),
                    _ => (at, 22),
                };
                (s.user, s.host, s.port) = (user.to_owned(), host.trim_start_matches('[').trim_end_matches(']').to_owned(), port);
            }
            "--host-key" => s.host_key = it.next()?.split_whitespace().take(2).collect::<Vec<_>>().join(" "),
            "--there" => s.there = it.next()?.clone(),
            "--pass" => s.pass = it.next()?.parse().ok()?,
            "--exclude" => s.exclude.push(it.next()?.clone()),
            "--detach" => s.detach = true,
            _ => return None,
        }
    }
    let id = s.dir.file_name()?.to_str()?;
    (named(id) && !s.user.is_empty() && !s.host.is_empty() && s.host_key.split(' ').count() == 2 && Path::new(&s.there).is_absolute() && s.pass >= 1).then_some(s)
}

/// How the copy stands, as `status.json` has it.
#[derive(Debug, Default)]
struct Standing {
    pass: u32,
    phase: &'static str,
    total_files: u64,
    total_bytes: u64,
    files: u64,
    bytes: u64,
    sent: u64,
    current: String,
    started: u64,
    ended: Option<u64>,
    error: Option<String>,
    report: Option<serde_json::Value>,
}

impl Standing {
    fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "pass": self.pass, "phase": self.phase, "total_files": self.total_files, "total_bytes": self.total_bytes,
            "files": self.files, "bytes": self.bytes, "sent": self.sent, "current": self.current,
            "started": self.started, "ended": self.ended, "error": self.error, "report": self.report,
        })
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

type Shared = std::sync::Arc<std::sync::Mutex<Standing>>;

fn write_standing(dir: &Path, standing: &Shared) {
    let v = standing.lock().unwrap_or_else(|e| e.into_inner()).json();
    let _ = write_whole(&dir.join("status.json"), v.to_string().as_bytes(), 0o600);
}

/// The home as the engine sends it, counting what it reads for the
/// status, and keeping the index it sends (the next pass's: what it put
/// there and is gone here is taken away there).
struct Counted {
    inner: olo_rex_sync::wire::FolderSource,
    standing: Shared,
    keep_index: PathBuf,
}

impl olo_rex_sync::wire::Source for Counted {
    fn index(&self) -> io::Result<olo_rex_sync::Index> {
        self.standing.lock().unwrap_or_else(|e| e.into_inner()).phase = "looking";
        let index = self.inner.index()?;
        index.save(&self.keep_index)?;
        let (files, bytes) = index.entries.values().filter(|e| e.kind == olo_rex_sync::EntryKind::File).fold((0, 0), |(n, b), e| (n + 1, b + e.size));
        let mut s = self.standing.lock().unwrap_or_else(|e| e.into_inner());
        (s.phase, s.total_files, s.total_bytes) = ("copying", files, bytes);
        Ok(index)
    }

    fn check_unchanged(&self, entry: &olo_rex_sync::Entry) -> bool {
        self.inner.check_unchanged(entry)
    }

    fn extent_at(&self, path: &str, offset: u64, max_len: u64) -> io::Result<(bool, u64)> {
        self.inner.extent_at(path, offset, max_len)
    }

    fn read(&self, path: &str, offset: u64, len: usize) -> io::Result<Vec<u8>> {
        let bytes = self.inner.read(path, offset, len)?;
        let mut s = self.standing.lock().unwrap_or_else(|e| e.into_inner());
        s.bytes += bytes.len() as u64;
        if s.current != path {
            s.files += 1;
            path.clone_into(&mut s.current);
        }
        Ok(bytes)
    }
}

/// An async stream read and written from a blocking thread (the engine's
/// side), the runtime driving it on its own.
struct Blocking<S> {
    stream: S,
    handle: tokio::runtime::Handle,
    standing: Shared,
}

impl<S: tokio::io::AsyncRead + Unpin> Read for Blocking<S> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        use tokio::io::AsyncReadExt;
        self.handle.block_on(self.stream.read(buf))
    }
}

impl<S: tokio::io::AsyncWrite + Unpin> Write for Blocking<S> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        use tokio::io::AsyncWriteExt;
        let n = self.handle.block_on(self.stream.write(buf))?;
        self.standing.lock().unwrap_or_else(|e| e.into_inner()).sent += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        use tokio::io::AsyncWriteExt;
        self.handle.block_on(self.stream.flush())
    }
}

/// The destination as the app's own connection found it: its key, and
/// nothing else, let through.
struct Pinned {
    key: String,
}

impl russh::client::Handler for Pinned {
    type Error = russh::Error;

    async fn check_server_key(&mut self, server: &russh::keys::PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        let key = match server {
            russh::keys::PublicKeyOrCertificate::PublicKey { key, .. } => key.clone(),
            russh::keys::PublicKeyOrCertificate::Certificate(cert) => russh::keys::PublicKey::from(cert.public_key().clone()),
        };
        Ok(key.to_openssh().is_ok_and(|line| line.split_whitespace().take(2).collect::<Vec<_>>().join(" ") == self.key))
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The command a move's key sends for `ask` (its forced command reads it).
fn command_for(ask: &serde_json::Value) -> String {
    format!("olo-rex-link move do {}", hex(ask.to_string().as_bytes()))
}

/// `move send …`.
fn send(args: &[String]) -> i32 {
    let Some(s) = sending(args) else {
        eprintln!("{USAGE}");
        return 2;
    };
    if let Err(e) = std::fs::create_dir_all(&s.dir).and_then(|()| std::fs::set_permissions(&s.dir, std::fs::Permissions::from_mode(0o700))) {
        eprintln!("olo-rex-link move send: {}: {e}", s.dir.display());
        return 1;
    }
    if s.detach {
        // (Forked before any thread is: it goes on in a session of its
        // own, its output in the move's log, whatever becomes of the
        // connection that started it.)
        // SAFETY: fork with no other thread running; the child only
        // carries on with this one.
        match unsafe { libc::fork() } {
            -1 => {
                eprintln!("olo-rex-link move send: {}", io::Error::last_os_error());
                return 1;
            }
            0 => {
                // SAFETY: plain calls on the child's own descriptors.
                unsafe { libc::setsid() };
                let log = std::fs::OpenOptions::new().create(true).append(true).mode(0o600).open(s.dir.join("log"));
                let null = std::fs::File::open("/dev/null");
                if let (Ok(log), Ok(null)) = (log, null) {
                    use std::os::fd::AsRawFd;
                    // SAFETY: replacing this process's standard streams.
                    unsafe {
                        libc::dup2(null.as_raw_fd(), 0);
                        libc::dup2(log.as_raw_fd(), 1);
                        libc::dup2(log.as_raw_fd(), 2);
                    }
                }
            }
            child => {
                let _ = write_whole(&s.dir.join("pid"), format!("{child}\n").as_bytes(), 0o600);
                println!("{{\"started\":{child}}}");
                return 0;
            }
        }
    }
    // SAFETY: a getter.
    let me = unsafe { libc::getpid() };
    let _ = write_whole(&s.dir.join("pid"), format!("{me}\n").as_bytes(), 0o600);
    let standing: Shared = std::sync::Arc::new(std::sync::Mutex::new(Standing { pass: s.pass, phase: "connecting", started: unix_now(), ..Standing::default() }));
    write_standing(&s.dir, &standing);
    // (The status kept on disk twice a second while it goes.)
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let writer = {
        let (dir, standing, done) = (s.dir.clone(), standing.clone(), done.clone());
        std::thread::spawn(move || {
            while !done.load(std::sync::atomic::Ordering::Relaxed) {
                write_standing(&dir, &standing);
                std::thread::sleep(Duration::from_millis(500));
            }
        })
    };
    let ended = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string()).and_then(|rt| rt.block_on(copy(&s, standing.clone())));
    done.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = writer.join();
    let code = {
        let mut st = standing.lock().unwrap_or_else(|e| e.into_inner());
        st.ended = Some(unix_now());
        match ended {
            Ok(()) => {
                st.phase = "done";
                0
            }
            Err(e) => {
                eprintln!("olo-rex-link move send: {e}");
                (st.phase, st.error) = ("failed", Some(e));
                1
            }
        }
    };
    write_standing(&s.dir, &standing);
    code
}

/// The index a pass kept, by its number.
fn kept_index(dir: &Path, pass: u32) -> PathBuf {
    dir.join(format!("index-{pass}"))
}

/// The destination reached with the move's key, its host key pinned.
async fn connect(s: &Sending) -> Result<russh::client::Handle<Pinned>, String> {
    let private = std::fs::read_to_string(s.dir.join("key")).map_err(|e| format!("the move's key: {e}"))?;
    let key = PrivateKey::from_openssh(&private).map_err(|e| format!("the move's key: {e}"))?;
    let config = std::sync::Arc::new(russh::client::Config { keepalive_interval: Some(Duration::from_secs(15)), keepalive_max: 4, ..Default::default() });
    let session = tokio::time::timeout(Duration::from_secs(30), russh::client::connect(config, (s.host.as_str(), s.port), Pinned { key: s.host_key.clone() }))
        .await
        .map_err(|_| format!("{}:{} didn't answer in 30 seconds", s.host, s.port))?
        .map_err(|e| match e {
            russh::Error::UnknownKey => format!("{}:{} isn't the host the app knows (its key differs)", s.host, s.port),
            e => format!("{}:{}: {e}", s.host, s.port),
        })?;
    let mut session = session;
    let auth = session.authenticate_publickey(&s.user, russh::keys::PrivateKeyWithHashAlg::new(std::sync::Arc::new(key), None)).await.map_err(|e| e.to_string())?;
    if !auth.success() {
        return Err(format!("{}@{} didn't let the move's key in", s.user, s.host));
    }
    Ok(session)
}

/// `move reach DIR --to … --host-key …`: whether the destination can be
/// reached from here with the move's key (before anything is stopped).
fn reach(args: &[String]) -> i32 {
    let Some(s) = sending(args) else {
        eprintln!("{USAGE}");
        return 2;
    };
    let said = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string()).and_then(|rt| {
        rt.block_on(async {
            let session = connect(&s).await?;
            let channel = session.channel_open_session().await.map_err(|e| e.to_string())?;
            channel.exec(true, command_for(&serde_json::json!({"op": "hello"}))).await.map_err(|e| e.to_string())?;
            let mut stream = channel.into_stream();
            let mut answer = String::new();
            use tokio::io::AsyncReadExt;
            tokio::time::timeout(Duration::from_secs(20), stream.read_to_string(&mut answer)).await.map_err(|_| "the destination's helper didn't answer".to_owned())?.map_err(|e| e.to_string())?;
            let _ = session.disconnect(russh::Disconnect::ByApplication, "", "").await;
            Ok::<String, String>(answer)
        })
    });
    match said {
        Ok(answer) => {
            let v: serde_json::Value = serde_json::from_str(answer.trim()).unwrap_or_default();
            println!("{}", serde_json::json!({"ok": v["ok"].as_bool() == Some(true), "helper": v["helper"]}));
            0
        }
        Err(e) => {
            println!("{}", serde_json::json!({"ok": false, "error": e}));
            1
        }
    }
}

async fn copy(s: &Sending, standing: Shared) -> Result<(), String> {
    let session = connect(s).await?;
    // The engine's receiving side there, in the destination's home.
    let channel = session.channel_open_session().await.map_err(|e| e.to_string())?;
    let ask = serde_json::json!({"op": "sync", "root": s.there, "options": ["--scoped"]});
    channel.exec(true, command_for(&ask)).await.map_err(|e| e.to_string())?;
    let filter = olo_rex_sync::index::Filter { exclude: s.exclude.clone(), ..olo_rex_sync::index::Filter::default() };
    let source = Counted {
        inner: olo_rex_sync::wire::FolderSource { root: home(), filter: filter.clone(), cache_path: Some(s.dir.join("index-cache")) },
        standing: standing.clone(),
        keep_index: kept_index(&s.dir, s.pass),
    };
    let io = Blocking { stream: channel.into_stream(), handle: tokio::runtime::Handle::current(), standing: standing.clone() };
    let options = olo_rex_sync::wire::Options { filter, ..olo_rex_sync::wire::Options::default() };
    let report = tokio::task::spawn_blocking(move || olo_rex_sync::wire::send(io, &source, options)).await.map_err(|e| e.to_string())?.map_err(|e| e.to_string())?;
    let failed: Vec<serde_json::Value> = report.errors.iter().take(200).map(|e| serde_json::json!({"path": e.path, "why": e.reason})).collect();
    // (What the engine passed over, counted and named: files it couldn't
    // read, names that aren't UTF-8, sockets and FIFOs, other filesystems.)
    let kind = |k: &olo_rex_sync::SkippedKind| serde_json::json!({"count": k.count, "paths": k.paths});
    let skipped = &report.skipped;
    standing.lock().unwrap_or_else(|e| e.into_inner()).report = Some(serde_json::json!({
        "same": report.same, "copied": report.copied + report.new, "patched": report.patched + report.appended,
        "failed": report.errors.len(), "failures": failed, "wire": report.wire_bytes(),
        "skipped": {"unread": kind(&skipped.unread), "odd_names": kind(&skipped.non_utf8), "special": kind(&skipped.special), "mounts": kind(&skipped.mounts)},
    }));
    // A second pass: what the one before put there and is gone here
    // taken away there (and nothing else of the destination's).
    if s.pass > 1 {
        let before = olo_rex_sync::Index::load(&kept_index(&s.dir, s.pass - 1)).map_err(|e| format!("the pass before's index: {e}"))?;
        let now = olo_rex_sync::Index::load(&kept_index(&s.dir, s.pass)).map_err(|e| format!("this pass's index: {e}"))?;
        let gone: Vec<&String> = before.entries.keys().filter(|p| !now.entries.contains_key(*p)).collect();
        if !gone.is_empty() {
            standing.lock().unwrap_or_else(|e| e.into_inner()).phase = "tidying";
            let channel = session.channel_open_session().await.map_err(|e| e.to_string())?;
            channel.exec(true, command_for(&serde_json::json!({"op": "forget", "root": s.there}))).await.map_err(|e| e.to_string())?;
            let mut stream = channel.into_stream();
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let list: Vec<u8> = gone.iter().flat_map(|p| p.bytes().chain([0])).collect();
            stream.write_all(&list).await.map_err(|e| e.to_string())?;
            stream.shutdown().await.map_err(|e| e.to_string())?;
            let mut said = String::new();
            stream.read_to_string(&mut said).await.map_err(|e| e.to_string())?;
            let v: serde_json::Value = serde_json::from_str(said.trim()).unwrap_or_default();
            if let Some(r) = standing.lock().unwrap_or_else(|e| e.into_inner()).report.as_mut() {
                r["gone"] = v["gone"].clone();
            }
        }
    }
    let _ = session.disconnect(russh::Disconnect::ByApplication, "", "").await;
    Ok(())
}

/// `move status DIR`: the copy's own account of itself (`status.json`,
/// which `send` keeps), and whether it still runs.
fn status(args: &[String]) -> i32 {
    let [dir] = args else {
        eprintln!("{USAGE}");
        return 2;
    };
    let dir = PathBuf::from(dir);
    let mut v: serde_json::Value = std::fs::read(dir.join("status.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_else(|| serde_json::json!({}));
    let pid: Option<i32> = std::fs::read_to_string(dir.join("pid")).ok().and_then(|s| s.trim().parse().ok());
    let running = pid.is_some_and(|p| {
        std::fs::read(format!("/proc/{p}/cmdline")).is_ok_and(|c| {
            let words: Vec<&[u8]> = c.split(|&b| b == 0).collect();
            words.iter().any(|w| *w == b"move") && words.iter().any(|w| Path::new(std::ffi::OsStr::from_bytes_compat(w)) == dir)
        })
    });
    v["running"] = serde_json::Value::Bool(running);
    println!("{v}");
    0
}

/// A process of this host: its parent, its name, its state.
#[derive(Clone, Debug)]
struct Proc {
    pid: i32,
    ppid: i32,
    uid: u32,
    comm: String,
    state: char,
    user_manager: bool,
}

fn proc_of(pid: i32) -> Option<Proc> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let (open, close) = (stat.find('(')?, stat.rfind(')')?);
    let comm = stat[open + 1..close].to_owned();
    let mut rest = stat[close + 2..].split_whitespace();
    let state = rest.next()?.chars().next()?;
    let ppid = rest.next()?.parse().ok()?;
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let uid = status.lines().find_map(|l| l.strip_prefix("Uid:"))?.split_whitespace().next()?.parse().ok()?;
    // (The account's own systemd: `systemd --user`.)
    let user_manager = comm == "systemd" && std::fs::read(format!("/proc/{pid}/cmdline")).is_ok_and(|c| c.split(|&b| b == 0).any(|w| w == b"--user"));
    Some(Proc { pid, ppid, uid, comm, state, user_manager })
}

fn all_procs() -> Vec<Proc> {
    std::fs::read_dir("/proc").map(|d| d.filter_map(|e| e.ok()?.file_name().to_str()?.parse().ok()).filter_map(proc_of).collect()).unwrap_or_default()
}

/// What stopping leaves alone of the account's: the account's own
/// systemd and what it keeps for it (its PAM half, its bus: `systemctl
/// --user` must still answer, to put things back), and what is gone
/// already (a zombie).
fn spared_kind(p: &Proc) -> bool {
    p.user_manager || p.comm == "(sd-pam)" || matches!(p.comm.as_str(), "dbus-broker" | "dbus-broker-lau" | "dbus-daemon") || p.state == 'Z'
}

/// The processes stopping would end: the account's, but `keep` (this
/// helper's own line of parents and those spared) and the spared kinds.
fn to_stop(procs: &[Proc], uid: u32, keep: &HashSet<i32>) -> Vec<i32> {
    procs.iter().filter(|p| p.uid == uid && !keep.contains(&p.pid) && !spared_kind(p)).map(|p| p.pid).collect()
}

/// This process and its parents, up to the first not the account's (sshd's
/// own): what stopping must never end.
fn own_line(procs: &[Proc], me: i32) -> HashSet<i32> {
    let mut line = HashSet::new();
    let mut at = me;
    while at > 1 && line.insert(at) {
        match procs.iter().find(|p| p.pid == at) {
            Some(p) => at = p.ppid,
            None => break,
        }
    }
    line
}

/// `move stop [--spare PID…]`: the account's processes asked to end, three
/// seconds given, then ended; three rounds (what starts another is ended
/// with it). Prints what was ended and what is left; status 1 if anything is.
fn stop(args: &[String]) -> i32 {
    let mut spare = HashSet::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match (a.as_str(), it.next().and_then(|p| p.parse::<i32>().ok())) {
            ("--spare", Some(pid)) => {
                spare.insert(pid);
            }
            _ => {
                eprintln!("{USAGE}");
                return 2;
            }
        }
    }
    // SAFETY: plain getters.
    let (me, uid) = unsafe { (libc::getpid(), libc::getuid()) };
    let mut keep = own_line(&all_procs(), me);
    keep.extend(spare);
    let mut ended = 0usize;
    for _round in 0..3 {
        let victims = to_stop(&all_procs(), uid, &keep);
        if victims.is_empty() {
            break;
        }
        for &pid in &victims {
            // SAFETY: a signal to a process of the account's own.
            unsafe { libc::kill(pid, libc::SIGTERM) };
        }
        let until = Instant::now() + Duration::from_secs(3);
        let alive = |pid: &i32| proc_of(*pid).is_some_and(|p| p.state != 'Z');
        while Instant::now() < until && victims.iter().any(alive) {
            std::thread::sleep(Duration::from_millis(100));
        }
        for pid in victims.iter().filter(|p| alive(p)) {
            // SAFETY: as above.
            unsafe { libc::kill(*pid, libc::SIGKILL) };
        }
        let until = Instant::now() + Duration::from_secs(1);
        while Instant::now() < until && victims.iter().any(alive) {
            std::thread::sleep(Duration::from_millis(50));
        }
        ended += victims.len();
    }
    let procs = all_procs();
    let left: Vec<serde_json::Value> = to_stop(&procs, uid, &keep)
        .into_iter()
        .filter_map(|pid| procs.iter().find(|p| p.pid == pid))
        .map(|p| serde_json::json!({"pid": p.pid, "name": p.comm, "state": p.state.to_string()}))
        .collect();
    let none_left = left.is_empty();
    println!("{}", serde_json::json!({"ended": ended, "left": left}));
    if none_left { 0 } else { 1 }
}

/// `/etc/os-release`'s fields.
fn os_release(text: &str) -> std::collections::HashMap<String, String> {
    text.lines()
        .filter_map(|l| {
            let (k, v) = l.split_once('=')?;
            let v = v.trim();
            let v = v.strip_prefix('"').and_then(|v| v.strip_suffix('"')).or_else(|| v.strip_prefix('\'').and_then(|v| v.strip_suffix('\''))).unwrap_or(v);
            Some((k.trim().to_owned(), v.replace("\\\"", "\"")))
        })
        .collect()
}

/// Free bytes and inodes where `path` is (its filesystem's, for the
/// account: what isn't kept for root).
fn free_at(path: &Path) -> Option<(u64, u64)> {
    let c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).ok()?;
    // SAFETY: statvfs fills the struct given; the path is NUL-ended.
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    (unsafe { libc::statvfs(c.as_ptr(), &mut st) } == 0).then(|| (st.f_bavail as u64 * st.f_frsize as u64, st.f_favail as u64))
}

/// Whether `name` is a program on the path.
fn on_path(name: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| std::fs::metadata(d.join(name)).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)))
}

/// The account's entry in the system's accounts: its name, home and
/// login shell.
fn account(uid: u32) -> (String, String, String) {
    // SAFETY: getpwuid_r with buffers of ours.
    unsafe {
        let mut pw: libc::passwd = std::mem::zeroed();
        let mut out: *mut libc::passwd = std::ptr::null_mut();
        let mut buf = vec![0 as libc::c_char; 16384];
        if libc::getpwuid_r(uid, &mut pw, buf.as_mut_ptr(), buf.len(), &mut out) == 0 && !out.is_null() {
            let s = |p: *const libc::c_char| if p.is_null() { String::new() } else { std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned() };
            return (s(pw.pw_name), s(pw.pw_dir), s(pw.pw_shell));
        }
    }
    (String::new(), home().display().to_string(), String::new())
}

/// The names of the account's groups.
fn group_names() -> Vec<String> {
    // SAFETY: getgroups into a buffer of ours, then getgrgid_r for each.
    unsafe {
        let n = libc::getgroups(0, std::ptr::null_mut());
        let mut ids = vec![0 as libc::gid_t; n.max(0) as usize + 1];
        let n = libc::getgroups(ids.len() as libc::c_int, ids.as_mut_ptr());
        ids.truncate(n.max(0) as usize);
        ids.push(libc::getgid());
        ids.sort_unstable();
        ids.dedup();
        ids.iter()
            .filter_map(|&g| {
                let mut gr: libc::group = std::mem::zeroed();
                let mut out: *mut libc::group = std::ptr::null_mut();
                let mut buf = vec![0 as libc::c_char; 16384];
                (libc::getgrgid_r(g, &mut gr, buf.as_mut_ptr(), buf.len(), &mut out) == 0 && !out.is_null()).then(|| std::ffi::CStr::from_ptr(gr.gr_name).to_string_lossy().into_owned())
            })
            .collect()
    }
}

/// Ports this account's programs listen on (TCP, v4 and v6), from the
/// kernel's tables.
fn listening(text: &str, uid: u32) -> Vec<u16> {
    text.lines()
        .skip(1)
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            // (local address, state 0A = listening, uid)
            let (local, state, owner) = (f.get(1)?, f.get(3)?, f.get(7)?.parse::<u32>().ok()?);
            (*state == "0A" && owner == uid).then(|| u16::from_str_radix(local.rsplit(':').next()?, 16).ok()).flatten()
        })
        .collect()
}

/// The account's own systemd units (in its own folder, not the system's
/// user units: dbus, gpg-agent, pipewire), and which it has enabled.
fn own_units(dir: &Path) -> (Vec<String>, Vec<String>) {
    let kinds = [".service", ".timer", ".socket", ".path", ".target"];
    let mut units: Vec<String> =
        std::fs::read_dir(dir).map(|d| d.filter_map(|e| e.ok()?.file_name().into_string().ok()).filter(|n| kinds.iter().any(|k| n.ends_with(k))).collect()).unwrap_or_default();
    let mut enabled: Vec<String> = std::fs::read_dir(dir)
        .map(|d| {
            d.filter_map(|e| e.ok())
                .filter(|e| e.file_name().to_str().is_some_and(|n| n.ends_with(".wants")))
                .flat_map(|e| std::fs::read_dir(e.path()).map(|w| w.filter_map(|x| x.ok()?.file_name().into_string().ok()).collect::<Vec<_>>()).unwrap_or_default())
                .collect()
        })
        .unwrap_or_default();
    units.sort();
    enabled.sort();
    enabled.dedup();
    (units, enabled)
}

/// Mount points below `home` (another filesystem the copy doesn't go into).
fn mounts_under(mountinfo: &str, home: &Path) -> Vec<String> {
    mountinfo.lines().filter_map(|l| l.split_whitespace().nth(4)).map(|m| m.replace("\\040", " ")).filter(|m| Path::new(m).starts_with(home) && Path::new(m) != home).collect()
}

/// `move look`: what this host and account are, for a move: system,
/// machine, the account, its home's room, what it may use, its own units,
/// what listens, who else is logged in (JSON). Changes nothing.
fn look() -> i32 {
    // SAFETY: plain getters, and uname into a struct of ours.
    let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
    let machine = unsafe {
        let mut u: libc::utsname = std::mem::zeroed();
        if libc::uname(&mut u) == 0 { std::ffi::CStr::from_ptr(u.machine.as_ptr()).to_string_lossy().into_owned() } else { String::new() }
    };
    let os = os_release(&std::fs::read_to_string("/etc/os-release").or_else(|_| std::fs::read_to_string("/usr/lib/os-release")).unwrap_or_default());
    let (user, home_dir, shell) = account(uid);
    let home_dir = std::env::var("HOME").unwrap_or(home_dir);
    let home_path = PathBuf::from(&home_dir);
    let (free, inodes) = free_at(&home_path).unwrap_or((0, 0));
    let (tmp_free, tmp_inodes) = free_at(Path::new("/var/tmp")).unwrap_or((0, 0));
    let groups = group_names();
    let config = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home_path.join(".config"));
    let (units, enabled) = own_units(&config.join("systemd/user"));
    let tools: Vec<&str> =
        ["crontab", "restorecon", "selinuxenabled", "sudo", "tmux", "screen", "podman", "docker", "git", "zsh", "fish", "bash", "loginctl", "systemctl", "usermod", "dnf", "apt-get"]
            .into_iter()
            .filter(|t| on_path(t))
            .collect();
    let mut ports: Vec<u16> = ["/proc/net/tcp", "/proc/net/tcp6"].iter().flat_map(|f| listening(&std::fs::read_to_string(f).unwrap_or_default(), uid)).collect();
    ports.sort_unstable();
    ports.dedup();
    let procs = all_procs();
    // SAFETY: a getter.
    let line = own_line(&procs, unsafe { libc::getpid() });
    let logins = procs.iter().filter(|p| p.uid == uid && (p.comm == "sshd-session" || p.comm == "sshd") && !line.contains(&p.pid)).count();
    let home_meta = std::fs::metadata(&home_path).ok();
    let ssh_port = std::env::var("SSH_CONNECTION").ok().and_then(|c| c.split_whitespace().nth(3).and_then(|p| p.parse::<u16>().ok()));
    let v = serde_json::json!({
        "system": os.get("ID").cloned().unwrap_or_default(),
        "system_like": os.get("ID_LIKE").cloned().unwrap_or_default(),
        "system_name": os.get("PRETTY_NAME").or_else(|| os.get("NAME")).cloned().unwrap_or_default(),
        "machine": machine,
        "user": user, "uid": uid, "gid": gid, "home": home_dir, "shell": shell,
        "free": free, "free_inodes": inodes, "var_tmp_free": tmp_free, "var_tmp_inodes": tmp_inodes,
        "admin": groups.iter().any(|g| matches!(g.as_str(), "wheel" | "sudo" | "admin")),
        "groups": groups,
        "tools": tools,
        "selinux": Path::new("/sys/fs/selinux/enforce").exists(),
        "linger": !user.is_empty() && Path::new("/var/lib/systemd/linger").join(&user).exists(),
        "units": units, "enabled": enabled,
        "listening": ports,
        "other_logins": logins,
        "mounts": mounts_under(&std::fs::read_to_string("/proc/self/mountinfo").unwrap_or_default(), &home_path),
        "tmux": Path::new(&format!("/tmp/tmux-{uid}")).exists(),
        "screen": Path::new(&format!("/run/screen/S-{user}")).exists() || Path::new(&format!("/var/run/screen/S-{user}")).exists(),
        "machine_id": std::fs::read_to_string("/etc/machine-id").unwrap_or_default().trim(),
        "boot_id": std::fs::read_to_string("/proc/sys/kernel/random/boot_id").unwrap_or_default().trim(),
        "home_dev": home_meta.as_ref().map(std::os::unix::fs::MetadataExt::dev),
        "home_ino": home_meta.as_ref().map(std::os::unix::fs::MetadataExt::ino),
        "ssh_port": ssh_port,
        "now": unix_now(),
        "helper": olo_rex_link::VERSION.to_string(),
    });
    println!("{v}");
    0
}

/// An agent's session files, as it holds them open (Codex's rollouts,
/// archived ones too; agy's conversations, their logs beside them).
fn session_file(home: &Path, target: &Path) -> bool {
    let codex = [home.join(".codex/sessions"), home.join(".codex/archived_sessions")];
    let agy = home.join(".gemini/antigravity-cli/conversations");
    let claude = home.join(".claude/projects");
    let name = target.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    (codex.iter().any(|d| target.starts_with(d)) && name.ends_with(".jsonl"))
        || (target.starts_with(&agy) && (name.ends_with(".db") || name.ends_with(".db-wal") || name.ends_with(".db-shm")))
        || (target.starts_with(&claude) && name.ends_with(".jsonl"))
}

/// One process for `look-pane`.
fn process_json(p: &Proc, home: &Path) -> serde_json::Value {
    let argv: Vec<String> =
        std::fs::read(format!("/proc/{}/cmdline", p.pid)).map(|c| c.split(|&b| b == 0).filter(|w| !w.is_empty()).map(|w| String::from_utf8_lossy(w).into_owned()).collect()).unwrap_or_default();
    let cwd = std::fs::read_link(format!("/proc/{}/cwd", p.pid)).map(|c| c.display().to_string()).unwrap_or_default();
    // (Field 22 counted after the name's closing bracket: a name may hold
    // spaces and brackets of its own.)
    let start: u64 = std::fs::read_to_string(format!("/proc/{}/stat", p.pid)).ok().and_then(|s| s.rsplit_once(')').and_then(|(_, rest)| rest.split_whitespace().nth(19)?.parse().ok())).unwrap_or(0);
    let mut open: Vec<String> = std::fs::read_dir(format!("/proc/{}/fd", p.pid))
        .map(|d| d.filter_map(|e| std::fs::read_link(e.ok()?.path()).ok()).filter(|t| session_file(home, t)).map(|t| t.display().to_string()).collect())
        .unwrap_or_default();
    open.sort();
    open.dedup();
    serde_json::json!({"pid": p.pid, "ppid": p.ppid, "name": p.comm, "argv": argv, "cwd": cwd, "start": start, "open": open})
}

/// Which agent a process is (`claude`, `codex`, `agy`), by its name or
/// its program's.
fn agent_of(argv: &[String], name: &str) -> Option<&'static str> {
    let base = |s: &str| s.rsplit('/').next().unwrap_or(s).to_owned();
    let names: Vec<String> = [name.to_owned()].into_iter().chain(argv.iter().take(2).map(|a| base(a))).collect();
    if names.iter().any(|n| n == "claude") || argv.iter().any(|a| a.contains("@anthropic-ai/claude-code") || a.contains("/claude/versions/")) {
        Some("claude")
    } else if names.iter().any(|n| n == "codex") {
        Some("codex")
    } else if names.iter().any(|n| n == "agy" || n == "antigravity") {
        Some("agy")
    } else {
        None
    }
}

/// The session id on an agent's command line (`--resume ID`, `-r ID`,
/// `--session-id ID`, Codex's `resume ID`, agy's `--conversation ID`).
fn argv_session(agent: &str, argv: &[String]) -> Option<String> {
    let flags: &[&str] = match agent {
        "claude" => &["--resume", "-r", "--session-id"],
        "codex" => &["resume"],
        _ => &["--conversation", "-conversation", "--resume", "-r"],
    };
    argv.windows(2).find(|w| flags.contains(&w[0].as_str()) && !w[1].starts_with('-')).map(|w| w[1].clone())
}

/// A Claude Code session's transcripts, newest first: `(id, written)` in
/// the project folder of `cwd`.
fn claude_transcripts(home: &Path, cwd: &str) -> Vec<(String, std::time::SystemTime)> {
    let project: String = cwd.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let mut all: Vec<(String, std::time::SystemTime)> = std::fs::read_dir(home.join(".claude/projects").join(project))
        .map(|d| {
            d.filter_map(|e| {
                let e = e.ok()?;
                let name = e.file_name().into_string().ok()?;
                Some((name.strip_suffix(".jsonl")?.to_owned(), e.metadata().ok()?.modified().ok()?))
            })
            .collect()
        })
        .unwrap_or_default();
    all.sort_by(|a, b| b.1.cmp(&a.1));
    all
}

/// When a process started, as the clock has it (its start in ticks after
/// boot, and boot's time).
fn started_at(start_ticks: u64) -> Option<std::time::SystemTime> {
    let boot: u64 = std::fs::read_to_string("/proc/stat").ok()?.lines().find_map(|l| l.strip_prefix("btime ")?.trim().parse().ok())?;
    // SAFETY: a getter.
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as u64;
    Some(std::time::UNIX_EPOCH + Duration::from_secs(boot) + Duration::from_millis(start_ticks * 1000 / hz))
}

/// Which agent runs in a pane's tree and the session it is in, and how
/// that was found: the session file it holds open (Codex's, agy's); for
/// Claude Code, the newest transcript of its folder written since it
/// started (a `/resume` inside it moves on from its command line); the
/// command line's; else the newest of its folder, a guess.
fn pane_agent(tree: &[serde_json::Value], home: &Path) -> serde_json::Value {
    let Some((p, agent)) = tree.iter().find_map(|p| {
        let argv: Vec<String> = p["argv"].as_array()?.iter().filter_map(|a| a.as_str().map(str::to_owned)).collect();
        agent_of(&argv, p["name"].as_str().unwrap_or_default()).map(|a| (p, a))
    }) else {
        return serde_json::Value::Null;
    };
    let argv: Vec<String> = p["argv"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect()).unwrap_or_default();
    let cwd = p["cwd"].as_str().unwrap_or_default();
    let open: Option<String> = tree.iter().flat_map(|q| q["open"].as_array().cloned().unwrap_or_default()).filter_map(|f| f.as_str().map(str::to_owned)).find_map(|f| {
        let name = f.rsplit('/').next()?.to_owned();
        for log in [".db", ".db-wal", ".db-shm"] {
            if let Some(stem) = name.strip_suffix(log) {
                return Some(stem.to_owned());
            }
        }
        let stem = name.strip_suffix(".jsonl")?;
        let parts: Vec<&str> = stem.split('-').collect();
        (parts.len() >= 5).then(|| parts[parts.len() - 5..].join("-"))
    });
    let (session, how) = if let Some(id) = open {
        (Some(id), "open")
    } else if agent == "claude"
        && let Some(since) = started_at(p["start"].as_u64().unwrap_or(0))
        && let Some((id, _)) = claude_transcripts(home, cwd).into_iter().find(|(_, written)| *written >= since)
    {
        (Some(id), "written")
    } else if let Some(id) = argv_session(agent, &argv) {
        (Some(id), "argv")
    } else if agent == "claude"
        && let Some((id, _)) = claude_transcripts(home, cwd).into_iter().next()
    {
        (Some(id), "guess")
    } else {
        (None, "none")
    };
    serde_json::json!({"agent": agent, "pid": p["pid"], "argv": argv, "cwd": cwd, "session": session, "how": how})
}

/// `move look-pane PID…`: for each pane's process (Rex's report gives its
/// pid), it and everything under it, parents first, and the agent there
/// with its session ([`pane_agent`]) (JSON).
fn look_pane(args: &[String]) -> i32 {
    let Ok(pids) = args.iter().map(|a| a.parse::<i32>()).collect::<Result<Vec<_>, _>>() else {
        eprintln!("{USAGE}");
        return 2;
    };
    let procs = all_procs();
    let home = home();
    let panes: Vec<serde_json::Value> = pids
        .iter()
        .map(|&root| {
            let mut tree = Vec::new();
            let mut at = vec![root];
            while let Some(pid) = at.pop() {
                if let Some(p) = procs.iter().find(|p| p.pid == pid) {
                    tree.push(process_json(p, &home));
                    let mut children: Vec<i32> = procs.iter().filter(|c| c.ppid == pid).map(|c| c.pid).collect();
                    children.sort_unstable_by(|a, b| b.cmp(a));
                    at.extend(children);
                }
            }
            let agent = pane_agent(&tree, &home);
            serde_json::json!({"pid": root, "tree": tree, "agent": agent})
        })
        .collect();
    println!("{}", serde_json::Value::Array(panes));
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The move's line in `authorized_keys`: in once however often it is
    /// allowed, the others' lines kept whole (a last one without its end
    /// given one), out again by its comment alone.
    #[test]
    fn a_moves_key_is_let_in_and_out() {
        let line = allowed_line("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIA olo-move-x", "m1", Path::new("/home/f/.local/share/olo/rex-link/olo-rex-link-1-x86_64"), "202611261200").unwrap();
        assert_eq!(line, "restrict,expiry-time=\"202611261200\",command=\"/home/f/.local/share/olo/rex-link/olo-rex-link-1-x86_64 move receive\" ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIA olo-move-m1");
        let theirs = "ssh-ed25519 AAAA person@laptop\n# a note\nssh-rsa BBBB ci";
        let once = keys_with(theirs, "m1", Some(&line));
        assert_eq!(once, format!("ssh-ed25519 AAAA person@laptop\n# a note\nssh-rsa BBBB ci\n{line}\n"));
        assert_eq!(keys_with(&once, "m1", Some(&line)), once, "allowed twice: in once");
        assert_eq!(keys_with(&once, "m1", None), "ssh-ed25519 AAAA person@laptop\n# a note\nssh-rsa BBBB ci\n");
        assert_eq!(keys_with(&once, "m2", None), once, "another move's id takes nothing out");
        // (Never a key of another kind, options in the line given, or a
        // helper's path that would end the command's quotes.)
        assert!(allowed_line("ssh-rsa AAAA x", "m1", Path::new("/h"), "1").is_none());
        assert!(allowed_line("command=\"x\" ssh-ed25519 AAAA x", "m1", Path::new("/h"), "1").is_none());
        assert!(allowed_line("ssh-ed25519 AAAA", "m1", Path::new("/h/\"x"), "1").is_none());
        assert!(named("0199a1b2-c3d4") && !named("a b") && !named("") && !named("../x"));
    }

    fn hex(s: &str) -> String {
        s.bytes().map(|b| format!("{b:02x}")).collect()
    }

    /// What the key may ask, and nothing else: a receive that never takes
    /// away (no `--delete`), only where the account may write.
    #[test]
    fn a_moves_key_may_ask_only_its_own() {
        let ask = |json: &str| asked(&format!("olo-rex-link move do {}", hex(json)));
        assert_eq!(ask(r#"{"op":"hello"}"#), Some(Asked::Hello));
        assert_eq!(
            ask(r#"{"op":"sync","root":"/home/f","options":["--scoped","--exclude",".cache"]}"#),
            Some(Asked::Sync { root: "/home/f".into(), options: ["--scoped", "--exclude", ".cache"].map(String::from).to_vec() })
        );
        assert_eq!(ask(r#"{"op":"sync","root":"/home/f","options":["--delete"]}"#), None);
        assert_eq!(ask(r#"{"op":"sync","root":"/home/f","options":["--exclude"]}"#), None);
        assert_eq!(ask(r#"{"op":"forget","root":"/home/f"}"#), Some(Asked::Forget { root: "/home/f".into() }));
        assert_eq!(ask(r#"{"op":"shell","command":"rm -rf ~"}"#), None);
        assert_eq!(asked("rm -rf ~"), None);
        assert_eq!(asked(""), None);
        let home = Path::new("/home/f");
        assert!(writable_root("/home/f", home, 1000) && writable_root("/home/f/proj", home, 1000));
        assert!(!writable_root("/home/f/../g", home, 1000) && !writable_root("/etc", home, 1000) && !writable_root("home/f", home, 1000));
        // (A folder of /var/tmp that isn't there yet may be made; one that
        // is another account's may not.)
        assert!(writable_root("/var/tmp/olo-move-test-none-such/x", home, 1000));
        assert!(!writable_root("/var/tmp", home, 1000));
    }

    /// The key is made once, in the move's folder, private to the account,
    /// and asked again gives the same; its public line is an ed25519 key's.
    #[test]
    fn a_moves_key_is_made_once() {
        let dir = PathBuf::from(format!("/var/tmp/olo-rex-link-move-key-{}", std::process::id())).join("m1");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(key(&[dir.display().to_string()]), 0);
        let public = std::fs::read_to_string(dir.join("key.pub")).unwrap();
        assert!(public.starts_with("ssh-ed25519 AAAA") && public.trim_end().ends_with(" olo-move-m1"), "{public}");
        let private = std::fs::read_to_string(dir.join("key")).unwrap();
        assert!(private.starts_with("-----BEGIN OPENSSH PRIVATE KEY-----"));
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!((mode(&dir), mode(&dir.join("key"))), (0o700, 0o600));
        assert_eq!(key(&[dir.display().to_string()]), 0);
        assert_eq!(std::fs::read_to_string(dir.join("key.pub")).unwrap(), public, "made once");
        let parsed = PrivateKey::from_openssh(&private).unwrap();
        assert_eq!(format!("{}\n", parsed.public_key().to_openssh().unwrap()), public);
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
    }

    /// What `send` is told: the destination by account, name and port
    /// (an address in brackets too), the host key as its type and body.
    #[test]
    fn a_copy_is_told_where_to() {
        let args = |s: &str| s.split(' ').map(String::from).collect::<Vec<_>>();
        let s = sending(
            &args("/var/tmp/x/m1 --to fedora@dest:2222 --host-key ssh-ed25519|AAAA|comment --there /home/fedora --pass 2 --exclude .cache --detach")
                .iter()
                .map(|a| a.replace('|', " "))
                .collect::<Vec<_>>(),
        )
        .unwrap();
        assert_eq!((s.user.as_str(), s.host.as_str(), s.port, s.host_key.as_str(), s.there.as_str(), s.pass, s.detach), ("fedora", "dest", 2222, "ssh-ed25519 AAAA", "/home/fedora", 2, true));
        assert_eq!(s.exclude, [".cache"]);
        let v6 = sending(&args("/var/tmp/x/m1 --to f@[fd00::1]:22 --host-key k|b --there /home/f").iter().map(|a| a.replace('|', " ")).collect::<Vec<_>>()).unwrap();
        assert_eq!((v6.host.as_str(), v6.port), ("fd00::1", 22));
        assert_eq!(sending(&args("/var/tmp/x/m1 --to f@h --host-key k|b --there /home/f").iter().map(|a| a.replace('|', " ")).collect::<Vec<_>>()).unwrap().port, 22);
        assert!(sending(&args("/var/tmp/x/m1 --to f@h --host-key k|b --there home/f").iter().map(|a| a.replace('|', " ")).collect::<Vec<_>>()).is_none(), "a relative root");
        assert!(sending(&args("/var/tmp/x/m1 --to h --host-key k|b --there /home/f").iter().map(|a| a.replace('|', " ")).collect::<Vec<_>>()).is_none(), "no account");
        assert!(sending(&args("/var/tmp/x/a.b --to f@h --host-key k|b --there /home/f").iter().map(|a| a.replace('|', " ")).collect::<Vec<_>>()).is_none(), "a folder not named by a move's id");
        // (Its ask, read back by `receive` as sent.)
        let ask = serde_json::json!({"op": "forget", "root": "/home/f"});
        assert_eq!(asked(&command_for(&ask)), Some(Asked::Forget { root: "/home/f".into() }));
    }

    /// Stopping spares this process's own line of parents and the
    /// account's systemd, and ends the rest of the account's.
    #[test]
    fn stopping_spares_its_own_line() {
        let p = |pid, ppid, uid, comm: &str, state, user_manager| Proc { pid, ppid, uid, comm: comm.into(), state, user_manager };
        let procs = vec![
            p(1, 0, 0, "systemd", 'S', false),
            p(500, 1, 0, "sshd", 'S', false),
            p(600, 500, 1000, "sshd-session", 'S', false),
            p(601, 600, 1000, "sh", 'S', false),
            p(602, 601, 1000, "olo-rex-link", 'R', false),
            p(700, 1, 1000, "systemd", 'S', true),
            p(701, 700, 1000, "(sd-pam)", 'S', false),
            p(702, 700, 1000, "dbus-broker", 'S', false),
            p(800, 700, 1000, "rex", 'S', false),
            p(801, 800, 1000, "claude", 'S', false),
            p(802, 800, 1000, "defunct", 'Z', false),
            p(900, 1, 1001, "bash", 'S', false),
        ];
        let keep = own_line(&procs, 602);
        assert_eq!(keep, HashSet::from([602, 601, 600, 500]));
        assert_eq!(to_stop(&procs, 1000, &keep), vec![800, 801]);
    }

    /// What a host says of itself is read from its own files: its system
    /// (quoted or not), the ports its account listens on, mounts below the
    /// home, the account's own units and those enabled.
    #[test]
    fn a_host_is_read_from_its_own_files() {
        let os = os_release("ID=fedora\nPRETTY_NAME=\"Fedora Linux 44 (Cloud Edition)\"\nID_LIKE='rhel centos'\n");
        assert_eq!((os["ID"].as_str(), os["PRETTY_NAME"].as_str(), os["ID_LIKE"].as_str()), ("fedora", "Fedora Linux 44 (Cloud Edition)", "rhel centos"));
        let tcp = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n   0: 0100007F:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 1 1\n   1: 00000000:0016 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 2 1\n   2: 0100007F:1F91 0100007F:9C40 01 00000000:00000000 00:00000000 00000000  1000        0 3 1\n";
        assert_eq!(listening(tcp, 1000), vec![8080], "its own, listening; not root's sshd, not a connection");
        let mi = "22 1 8:1 / / rw - ext4 /dev/sda1 rw\n40 22 0:40 / /home/f/remote rw - fuse.sshfs h: rw\n41 22 0:41 / /home/f/my\\040stuff rw - nfs x rw\n42 22 0:42 / /home/fx rw - tmpfs t rw\n";
        assert_eq!(mounts_under(mi, Path::new("/home/f")), ["/home/f/remote", "/home/f/my stuff"]);
        let dir = PathBuf::from(format!("/var/tmp/olo-rex-link-move-units-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("default.target.wants")).unwrap();
        for f in ["backup.service", "backup.timer", "notes.txt"] {
            std::fs::write(dir.join(f), "").unwrap();
        }
        std::os::unix::fs::symlink(dir.join("backup.timer"), dir.join("default.target.wants/backup.timer")).unwrap();
        assert_eq!(own_units(&dir), (vec!["backup.service".into(), "backup.timer".into()], vec!["backup.timer".into()]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The agents' session files are told by where they are: Codex's
    /// rollouts (archived too), agy's conversations and their logs.
    #[test]
    fn session_files_are_told_by_their_place() {
        let h = Path::new("/home/f");
        assert!(session_file(h, Path::new("/home/f/.codex/sessions/2026/11/25/rollout-x-ID.jsonl")));
        assert!(session_file(h, Path::new("/home/f/.codex/archived_sessions/rollout-x-ID.jsonl")));
        assert!(session_file(h, Path::new("/home/f/.gemini/antigravity-cli/conversations/ID.db-wal")));
        assert!(!session_file(h, Path::new("/home/f/.codex/config.toml")));
        assert!(!session_file(h, Path::new("/home/g/.codex/sessions/a.jsonl")));
    }

    /// A pane's process read whole: its command line with a newline and a
    /// space in its arguments, its start time though its name has a space
    /// and a bracket.
    #[test]
    fn a_panes_process_is_read_whole() {
        // (A process that names itself with a space and brackets, as a
        // program may: its name in /proc/PID/stat is that.)
        let mut child = std::process::Command::new("bash").args(["-c", "printf 'odd (name) x' > /proc/self/comm; sleep 30 & wait"]).spawn().unwrap();
        let until = Instant::now() + Duration::from_secs(5);
        while proc_of(child.id() as i32).is_none_or(|p| p.comm != "odd (name) x") && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
        }
        let p = proc_of(child.id() as i32).unwrap();
        assert_eq!(p.comm, "odd (name) x");
        let v = process_json(&p, &home());
        assert_eq!(v["argv"][1], "-c");
        assert!(v["start"].as_u64().unwrap() > 0, "{v}");
        let _ = child.kill();
        let _ = child.wait();
        let mut child = std::process::Command::new("bash").args(["-c", "sleep 30 & wait", "a word\nwith a line"]).spawn().unwrap();
        std::thread::sleep(Duration::from_millis(100));
        let v = process_json(&proc_of(child.id() as i32).unwrap(), &home());
        assert_eq!(v["argv"][3], "a word\nwith a line");
        let _ = child.kill();
        let _ = child.wait();
    }

    /// The agent in a pane's tree and its session: the file it holds open
    /// first, its command line's next; never a program that isn't one.
    #[test]
    fn a_panes_agent_and_session_are_found() {
        let home = Path::new("/var/tmp/olo-rex-link-no-such-home");
        let p = |pid: u32, argv: &[&str], open: &[&str]| serde_json::json!({"pid": pid, "name": argv[0], "argv": argv, "cwd": "/home/u/p", "start": 0, "open": open});
        let codex = [p(1, &["bash"], &[]), p(2, &["codex", "resume", "OLD", "--yolo"], &["/home/u/.codex/sessions/2026/11/25/rollout-2026-11-25T01-02-03-0199a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b.jsonl"])];
        let v = pane_agent(&codex, home);
        assert_eq!((v["agent"].as_str(), v["session"].as_str(), v["how"].as_str()), (Some("codex"), Some("0199a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b"), Some("open")));
        let agy = [p(5, &["agy", "--conversation", "C1"], &[])];
        assert_eq!(pane_agent(&agy, home)["session"], "C1");
        let claude = [p(7, &["/home/u/.local/bin/claude", "--dangerously-skip-permissions", "--resume", "S1"], &[])];
        let v = pane_agent(&claude, home);
        assert_eq!((v["agent"].as_str(), v["session"].as_str(), v["how"].as_str()), (Some("claude"), Some("S1"), Some("argv")));
        assert!(pane_agent(&[p(9, &["tail", "-f", "todo.txt"], &[])], home).is_null());
        assert_eq!(argv_session("codex", &["codex".into(), "resume".into(), "--last".into()]), None);
    }
}
