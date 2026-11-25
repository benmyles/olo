//! `olo-rex-link move …`: the helper's part in moving an account from one
//! host to another (Ben, 2026-11-25: "machine to machine copy (don't go
//! via the user's desktop)"), driven by the app (`olo_rex_client::
//! migrate`). The same program on Ubuntu, Fedora or anything else (it is
//! static), so nothing of the host's is needed for it: no rsync, tar or
//! zstd (Fedora's cloud image has none of them), no ssh-keygen.
//!
//! - `move key DIR`: a key of the move's own (ed25519, made here) in the
//!   move's folder on the source, its public line printed.
//! - `move allow ID LINE [--root /var/tmp/NAME]…` / `move disallow ID`:
//!   that key let into the destination's account (`~/.ssh/authorized_keys`)
//!   for a day, able to run nothing but this helper's `move receive --id
//!   ID`, writing nothing but the home itself and the folders of `/var/tmp`
//!   named here; and taken out again.
//! - `move receive --id ID`: what that key runs (its forced command): the
//!   one thing asked of it (`SSH_ORIGINAL_COMMAND`), from a short list;
//!   never the destination's own keys, its helper or the move's folder.
//! - `move keep ID` / `move put-back ID`: the destination's own files kept
//!   aside before the copy (hard links), and put back by Go Back.
//! - `move add-keys ID HEX` / `move take-keys ID`: the source's
//!   `authorized_keys` let in at the destination too, and out again.
//! - `move codex-stop`: Codex's daemon stopped as Codex restarts it.
//! - `move send DIR --to USER@HOST[:PORT] --host-key KEY --there ROOT
//!   [--pass N] [--exclude PATTERN]… [--also /var/tmp/NAME]… [--detach]`:
//!   the copy, from the home here to ROOT there (and each folder of
//!   `/var/tmp` named to the same place there), over an SSH connection of its own with the
//!   move's key, the destination's host key pinned (`KEY`, as the app's
//!   own connection checked it: never trusted on first sight); detached,
//!   it goes on with the app gone (its log, pid and `status.json` in DIR).
//!   A second pass sends what changed and takes away there what the first
//!   put there and is gone here.
//! - `move status DIR`: how the copy in the move's folder stands.
//! - `move look [--size] [--exclude PATTERN]…`: what this host and account
//!   are, for a move (JSON); with `--size`, what the home holds that the
//!   copy brings, and the account's folders in `/var/tmp`.
//! - `move fix --from OLD --to NEW [--once FILE]`: a moved home made right
//!   where its path changed (`crate::fix`).
//! - `move look-pane [--screens HEX] PID…`: each pane's programs and the
//!   agent there with its session (`crate::panes`).
//! - `move stop [--spare PID…]`: every process of the account stopped but
//!   this one's own line of parents and the account's own systemd.

use std::collections::HashSet;
use std::io::{self, Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use russh::keys::ssh_key::{LineEnding, PrivateKey, private::Ed25519Keypair};

const USAGE: &str = "usage: olo-rex-link move look [--size] [--exclude PATTERN]...\n       olo-rex-link move codex-stop\n       olo-rex-link move halt DIR\n       olo-rex-link move keep ID [--exclude PATTERN]...\n       olo-rex-link move put-back ID\n       olo-rex-link move add-keys ID HEX\n       olo-rex-link move take-keys ID\n       olo-rex-link move reach DIR --to USER@HOST[:PORT] --host-key KEY --there ROOT\n       olo-rex-link move look-pane [--screens HEX] PID...\n       olo-rex-link move key DIR\n       olo-rex-link move send DIR --to USER@HOST[:PORT] --host-key KEY --there ROOT [--pass N] [--exclude PATTERN]... [--also /var/tmp/NAME]... [--detach]\n       olo-rex-link move allow ID PUBLIC-KEY-LINE [--root /var/tmp/NAME]...\n       olo-rex-link move disallow ID\n       olo-rex-link move receive --id ID   (a move's key's forced command)\n       olo-rex-link move status DIR\n       olo-rex-link move stop [--spare PID]…";

pub fn run(args: Vec<String>) -> ! {
    let code = match args.first().map(String::as_str) {
        Some("key") => key(&args[1..]),
        Some("send") => send(&args[1..]),
        Some("reach") => reach(&args[1..]),
        Some("allow") => allow(&args[1..]),
        Some("disallow") => disallow(&args[1..]),
        Some("receive") => receive(&args[1..]),
        Some("status") => status(&args[1..]),
        Some("stop") => stop(&args[1..]),
        Some("look") => look(&args[1..]),
        Some("codex-stop") => codex_stop(),
        Some("halt") => halt(&args[1..]),
        Some("add-keys") => add_keys(&args[1..]),
        Some("take-keys") => take_keys(&args[1..]),
        Some("keep") => keep_own(&args[1..]),
        Some("put-back") => put_back(&args[1..]),
        Some("look-pane") => crate::panes::look_pane(&args[1..]),
        Some("fix") => crate::fix::run(&args[1..]),
        _ => {
            eprintln!("{USAGE}");
            2
        }
    };
    std::process::exit(code)
}

pub(crate) fn home() -> PathBuf {
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
    // (This process's own: two writing one file at once never share it.)
    let new = path.with_file_name(format!(".{name}.{}.olo-move-new", std::process::id()));
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

/// A day from now, as `authorized_keys`' `expiry-time` has it: the
/// system's own time zone, which is how sshd reads it (a `TZ` of the
/// account's own would put it hours out, even in the past: its zone
/// left out first). The `Z` of UTC would be plainer, but an sshd before
/// 8.7 refuses the line.
fn tomorrow() -> String {
    // SAFETY: `allow` runs this before any thread is started (nothing
    // reads the environment meanwhile); `time` and `localtime_r` with a
    // valid pointer to fill.
    unsafe {
        std::env::remove_var("TZ");
        tzset();
        let now = libc::time(std::ptr::null_mut()) + 24 * 3600;
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&now, &mut tm);
        format!("{:04}{:02}{:02}{:02}{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min)
    }
}

unsafe extern "C" {
    fn tzset();
}

/// The line that lets the move's key in: for a day, nothing but this
/// helper's `move receive` for this move (no terminal, no forwarding of
/// any kind). sshd runs the command with the account's shell, so the
/// helper's path is quoted where it holds more than a path's plain
/// characters (a home with a space in it).
fn allowed_line(public: &str, id: &str, helper: &Path, expiry: &str) -> Option<String> {
    let mut words = public.split_whitespace();
    let (kind, body) = (words.next()?, words.next()?);
    if kind != "ssh-ed25519" || body.is_empty() || !body.bytes().all(|b| b.is_ascii_alphanumeric() || b"+/=".contains(&b)) || !named(id) {
        return None;
    }
    let helper = helper.to_str()?;
    if helper.contains(['"', '\\', '\n', '\'']) || helper.chars().any(char::is_control) {
        return None;
    }
    let plain = helper.bytes().all(|b| b.is_ascii_alphanumeric() || b"/._-+@:,=".contains(&b));
    let helper = if plain { helper.to_owned() } else { format!("'{helper}'") };
    Some(format!("restrict,expiry-time=\"{expiry}\",command=\"{helper} move receive --id {id}\" {kind} {body} {}", comment(id)))
}

/// `move allow ID LINE [--root PATH]…`: the move's key let in (again: a
/// later allow puts its day off, the line once); with the folders of
/// `/var/tmp` it may write besides the home, which this account keeps in
/// the move's folder here, out of the key's reach.
fn allow(args: &[String]) -> i32 {
    let (Some(id), Some(public)) = (args.first(), args.get(1)) else {
        eprintln!("{USAGE}");
        return 2;
    };
    let mut roots = Vec::new();
    let mut it = args[2..].iter();
    while let Some(a) = it.next() {
        match (a.as_str(), it.next()) {
            ("--root", Some(r)) if var_tmp_root(r) => roots.push(r.clone()),
            _ => {
                eprintln!("olo-rex-link move allow: a folder of /var/tmp of its own, by its whole path, after --root");
                return 2;
            }
        }
    }
    if named(id) {
        let dir = home().join(MOVES).join(id);
        let saved = std::fs::create_dir_all(&dir)
            .and_then(|()| std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)))
            .and_then(|()| write_whole(&dir.join("roots"), roots.join("\n").as_bytes(), 0o600));
        if let Err(e) = saved {
            eprintln!("olo-rex-link move allow: {e}");
            return 1;
        }
    }
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

/// The moves' folders, under the home.
const MOVES: &str = ".local/state/olo-rex/move";

/// Whether `rel` (under the home) is one of [`KEPT`], in one, or a folder
/// one is in.
fn kept(rel: &Path) -> bool {
    KEPT.iter().any(|k| rel.starts_with(k) || Path::new(k).starts_with(rel))
}

/// A folder of `/var/tmp` itself (`/var/tmp/NAME`, nothing deeper), by a
/// plain whole path.
fn var_tmp_root(root: &str) -> bool {
    let path = Path::new(root);
    path.parent() == Some(Path::new("/var/tmp")) && path.file_name().and_then(|n| n.to_str()).is_some_and(|n| !n.starts_with('.') && named_file(n))
}

/// A file's name of plain characters, no `..` or `/`.
fn named_file(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains('/') && !name.chars().any(char::is_control)
}

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

pub(crate) fn unhex(s: &str) -> Option<Vec<u8>> {
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

/// Where a move's key may write: the home itself (by its real path:
/// `/home` may be a link to `/var/home`); a folder under it that a part
/// of the copy goes to (sent in parallel), never one on the way to or in
/// what the destination keeps ([`KEPT`]: not `.ssh`, not `.local`), each
/// folder on the way a folder and not a link, itself a folder or not
/// there yet; or a folder of `/var/tmp` the app named for this move when
/// it let the key in (`roots`), the account's own or not there yet, not a
/// link. Nothing else.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Root {
    Home,
    Part,
    VarTmp,
}

fn writable_root(root: &str, home: &Path, uid: u32, roots: &[String]) -> Option<Root> {
    let path = Path::new(root);
    if !path.is_absolute() || path.components().any(|c| !matches!(c, std::path::Component::RootDir | std::path::Component::Normal(_))) {
        return None;
    }
    if let (Ok(real), Ok(home)) = (std::fs::canonicalize(path), std::fs::canonicalize(home))
        && real == home
    {
        return Some(Root::Home);
    }
    if let Ok(rel) = path.strip_prefix(home)
        && !rel.as_os_str().is_empty()
        && rel.components().all(|c| matches!(c, std::path::Component::Normal(_)))
        && !kept(rel)
        && no_link_on_the_way(home, rel)
    {
        return match std::fs::symlink_metadata(path) {
            Ok(m) if m.is_dir() => Some(Root::Part),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Some(Root::Part),
            _ => None,
        };
    }
    if !(var_tmp_root(root) && roots.iter().any(|r| r == root)) {
        return None;
    }
    match std::fs::symlink_metadata(path) {
        Ok(m) => (m.is_dir() && std::os::unix::fs::MetadataExt::uid(&m) == uid).then_some(Root::VarTmp),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Some(Root::VarTmp),
        Err(_) => None,
    }
}

/// The `/var/tmp` folders the app named for move `id` (`allow --root`).
fn allowed_roots(id: &str) -> Vec<String> {
    std::fs::read_to_string(home().join(MOVES).join(id).join("roots")).map(|t| t.lines().filter(|l| var_tmp_root(l)).map(str::to_owned).collect()).unwrap_or_default()
}

/// `move receive --id ID`: what the move's key runs (its forced command
/// names the move).
fn receive(args: &[String]) -> i32 {
    let ["--id", id] = args.iter().map(String::as_str).collect::<Vec<_>>()[..] else {
        eprintln!("olo-rex-link move receive: only as a move's key's command");
        return 2;
    };
    let Ok(command) = std::env::var("SSH_ORIGINAL_COMMAND") else {
        eprintln!("olo-rex-link move receive: only as a move's key's command");
        return 2;
    };
    if !named(id) {
        return 2;
    }
    // SAFETY: a getter.
    let uid = unsafe { libc::getuid() };
    let roots = allowed_roots(id);
    let dir = home().join(MOVES).join(id);
    match asked(&command) {
        Some(Asked::Hello) => {
            println!("{{\"ok\":true,\"helper\":\"{}\"}}", olo_rex_link::VERSION);
            0
        }
        Some(Asked::Sync { root, mut options }) => {
            let Some(kind) = writable_root(&root, &home(), uid, &roots) else {
                eprintln!("olo-rex-link move receive: {root} isn't somewhere this move may write");
                return 2;
            };
            // (What the destination keeps as its own, whatever the sender
            // sends: its keys, its helper, the move's own folder. Its
            // index kept in the move's folder, not the home's cache, where
            // it would stay: one for each root.)
            options.retain(|o| o != "--no-cache");
            let mut args = vec!["receive".to_owned(), root.clone()];
            args.extend(options);
            if kind == Root::Home {
                for p in KEPT {
                    args.extend(["--protect".to_owned(), (*p).to_owned()]);
                }
            }
            let made = match kind {
                Root::Home => Ok(()),
                Root::VarTmp => std::fs::create_dir(&root).or_else(|e| if e.kind() == io::ErrorKind::AlreadyExists { Ok(()) } else { Err(e) }),
                // (Its folders on the way made where the home's own session
                // hasn't yet: they are made right when it gets there.)
                Root::Part => std::fs::create_dir_all(&root),
            };
            if let Err(e) = made {
                eprintln!("olo-rex-link move receive: {root}: {e}");
                return 1;
            }
            let name = if kind == Root::Home { "recv-index".to_owned() } else { format!("recv-index-{}", key_of("r", &root)) };
            args.extend(["--cache".to_owned(), dir.join(name).display().to_string()]);
            crate::sync::run(args)
        }
        Some(Asked::Forget { root }) => match writable_root(&root, &home(), uid, &roots) {
            // (What is under a part is kept as the home's is: by its path
            // from the home.)
            Some(kind) => forget(Path::new(&root), kind, Path::new(&root).strip_prefix(home()).ok().map(Path::to_path_buf)),
            None => {
                eprintln!("olo-rex-link move receive: {root} isn't somewhere this move may write");
                2
            }
        },
        None => {
            eprintln!("olo-rex-link move receive: not something a move's key may ask");
            2
        }
    }
}

/// `forget`: each path on the input (NUL after each, under `root`) taken
/// away, a folder only if it is empty; deepest first. Never one the
/// destination keeps ([`KEPT`]), never through a link (each folder on the
/// way a folder, not a link to one elsewhere), and in a folder that is
/// read-only (Go's module cache: 0555) by opening it to the account a
/// moment, its mode and time put back.
fn forget(root: &Path, kind: Root, under_home: Option<PathBuf>) -> i32 {
    let mut input = Vec::new();
    if io::stdin().read_to_end(&mut input).is_err() {
        return 1;
    }
    let mut paths: Vec<&[u8]> = input.split(|&b| b == 0).filter(|p| !p.is_empty()).collect();
    paths.sort_by_key(|p| std::cmp::Reverse(p.iter().filter(|&&b| b == b'/').count()));
    let (mut gone, mut left) = (0u64, 0u64);
    for p in paths {
        let rel = Path::new(std::ffi::OsStr::from_bytes_compat(p));
        let in_home = under_home.as_ref().filter(|_| kind != Root::VarTmp).map(|h| h.join(rel));
        if rel.is_absolute() || rel.components().any(|c| !matches!(c, std::path::Component::Normal(_))) || in_home.is_some_and(|p| kept(&p)) || !folders_on_the_way(root, rel) {
            left += 1;
            continue;
        }
        if remove_entry(&root.join(rel)) {
            gone += 1;
        } else {
            left += 1;
        }
    }
    println!("{{\"gone\":{gone},\"kept\":{left}}}");
    0
}

/// Whether no folder from `root` down to `rel`'s own is a link or a file:
/// each there a folder, or not there yet (made, as folders, when the part
/// is: the home's own session makes them too, at the same time).
fn no_link_on_the_way(root: &Path, rel: &Path) -> bool {
    if !std::fs::symlink_metadata(root).is_ok_and(|m| m.is_dir()) {
        return false;
    }
    let mut at = root.to_path_buf();
    let parts: Vec<_> = rel.components().collect();
    for part in &parts[..parts.len().saturating_sub(1)] {
        at.push(part);
        match std::fs::symlink_metadata(&at) {
            Ok(m) if m.is_dir() => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => return true,
            _ => return false,
        }
    }
    true
}

/// Whether every folder from `root` down to `rel`'s own is a folder (not a
/// link, which would lead the removal elsewhere).
fn folders_on_the_way(root: &Path, rel: &Path) -> bool {
    if !std::fs::symlink_metadata(root).is_ok_and(|m| m.is_dir()) {
        return false;
    }
    let mut at = root.to_path_buf();
    let parts: Vec<_> = rel.components().collect();
    for part in &parts[..parts.len().saturating_sub(1)] {
        at.push(part);
        if !std::fs::symlink_metadata(&at).is_ok_and(|m| m.is_dir()) {
            return false;
        }
    }
    true
}

/// `path` removed (a folder only if empty); gone already counts. A
/// folder that won't let the account change it (0555) is opened to it a
/// moment and put back as it was, mode and time.
fn remove_entry(path: &Path) -> bool {
    let remove = || match std::fs::symlink_metadata(path) {
        Ok(m) if m.is_dir() => std::fs::remove_dir(path),
        Ok(_) => std::fs::remove_file(path),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    };
    match remove() {
        Ok(()) => true,
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
            let Some(parent) = path.parent() else { return false };
            let Ok(meta) = std::fs::symlink_metadata(parent) else { return false };
            let mode = meta.permissions().mode() & 0o7777;
            if mode & 0o200 != 0 || std::fs::set_permissions(parent, std::fs::Permissions::from_mode(mode | 0o700)).is_err() {
                return false;
            }
            let done = remove().is_ok();
            let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(mode));
            use std::os::unix::fs::MetadataExt;
            let _ = crate::fix::set_times(parent, meta.atime(), meta.atime_nsec(), meta.mtime(), meta.mtime_nsec(), false);
            done
        }
        Err(_) => false,
    }
}

/// The key's place in an `authorized_keys` line, its kind and body (after
/// any options, whose quoted values may hold spaces), and its comment.
fn line_key(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    // (Words, a quoted stretch one with the word it is in.)
    let mut words = Vec::new();
    let (mut word, mut quoted, mut escaped) = (String::new(), false, false);
    for c in line.chars() {
        match c {
            _ if escaped => {
                word.push(c);
                escaped = false;
            }
            '\\' if quoted => {
                word.push(c);
                escaped = true;
            }
            '"' => {
                word.push(c);
                quoted = !quoted;
            }
            c if c.is_whitespace() && !quoted => {
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
            }
            c => word.push(c),
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    let kind = |w: &str| w.starts_with("ssh-") || w.starts_with("ecdsa-sha2-") || w.starts_with("sk-ssh-") || w.starts_with("sk-ecdsa-");
    let at = words.iter().position(|w| kind(w))?;
    Some((words[at].clone(), words.get(at + 1)?.clone()))
}

/// `move add-keys ID HEX`: the source account's `authorized_keys` (HEX:
/// its text, hex-coded) let in here too, a line for each key the
/// destination doesn't have (by kind and body; never a move's own key),
/// as the source had it (its options too): the person's other computers
/// reach the account where it is now. What was added is kept in the
/// move's folder (`take-keys` takes it out again).
fn add_keys(args: &[String]) -> i32 {
    let [id, text] = args else {
        eprintln!("{USAGE}");
        return 2;
    };
    let Some(theirs) = named(id).then(|| unhex(text)).flatten().and_then(|b| String::from_utf8(b).ok()) else {
        eprintln!("olo-rex-link move add-keys: a move's id and a hex-coded authorized_keys");
        return 2;
    };
    let (file, dir) = authorized();
    let mine = std::fs::read_to_string(&file).unwrap_or_default();
    let have: HashSet<(String, String)> = mine.lines().filter_map(line_key).collect();
    let mut seen = have.clone();
    let added: Vec<&str> = theirs.lines().filter(|l| !l.split_whitespace().last().is_some_and(|c| c.starts_with("olo-move-"))).filter(|l| line_key(l).is_some_and(|k| seen.insert(k))).collect();
    let done = (|| -> io::Result<()> {
        if added.is_empty() {
            return Ok(());
        }
        let moves = home().join(MOVES).join(id);
        std::fs::create_dir_all(&moves)?;
        std::fs::set_permissions(&moves, std::fs::Permissions::from_mode(0o700))?;
        let before: String = std::fs::read_to_string(moves.join("added-keys")).unwrap_or_default();
        write_whole(&moves.join("added-keys"), format!("{before}{}", added.iter().map(|l| format!("{l}\n")).collect::<String>()).as_bytes(), 0o600)?;
        if !dir.exists() {
            std::fs::create_dir_all(&dir)?;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
        }
        let mut text = mine.clone();
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        for l in &added {
            text.push_str(l);
            text.push('\n');
        }
        let mode = std::fs::metadata(&file).map(|m| m.permissions().mode() & 0o777).unwrap_or(0o600) & !0o022;
        write_whole(&file, text.as_bytes(), mode)
    })();
    match done {
        Ok(()) => {
            println!("{}", serde_json::json!({"added": added.len()}));
            0
        }
        Err(e) => {
            eprintln!("olo-rex-link move add-keys: {e}");
            1
        }
    }
}

/// `move take-keys ID`: the lines `add-keys` added taken out again (Go
/// Back).
fn take_keys(args: &[String]) -> i32 {
    let [id] = args else {
        eprintln!("{USAGE}");
        return 2;
    };
    if !named(id) {
        return 2;
    }
    let added = std::fs::read_to_string(home().join(MOVES).join(id).join("added-keys")).unwrap_or_default();
    let added: HashSet<&str> = added.lines().collect();
    let (file, _) = authorized();
    let Ok(text) = std::fs::read_to_string(&file) else {
        println!("{}", serde_json::json!({"taken": 0}));
        return 0;
    };
    let kept: Vec<&str> = text.lines().filter(|l| !added.contains(l)).collect();
    let taken = text.lines().count() - kept.len();
    let mode = std::fs::metadata(&file).map(|m| m.permissions().mode() & 0o777).unwrap_or(0o600) & !0o022;
    let out: String = kept.iter().map(|l| format!("{l}\n")).collect();
    match if taken > 0 { write_whole(&file, out.as_bytes(), mode) } else { Ok(()) } {
        Ok(()) => {
            println!("{}", serde_json::json!({"taken": taken}));
            0
        }
        Err(e) => {
            eprintln!("olo-rex-link move take-keys: {e}");
            1
        }
    }
}

/// `move keep ID [--exclude PATTERN]…`: before the first copy, the
/// destination's own files kept aside as they are, by a second name in the
/// move's folder (`before/`: a hard link, no room taken; a symlink made
/// again there), so Go Back can put them back (`put-back`). Only once (a
/// second keep would keep what the copy brought); never what the copy
/// leaves alone (the move's exclusions, the destination's kept files),
/// one filesystem. (A file with a second name is never appended to in
/// place by the copy: replaced, its first version stays here whole.)
fn keep_own(args: &[String]) -> i32 {
    let Some(id) = args.first().filter(|i| named(i)) else {
        eprintln!("{USAGE}");
        return 2;
    };
    let mut exclude = Vec::new();
    let mut it = args[1..].iter();
    while let Some(a) = it.next() {
        match (a.as_str(), it.next()) {
            ("--exclude", Some(p)) => exclude.push(p.clone()),
            _ => {
                eprintln!("{USAGE}");
                return 2;
            }
        }
    }
    let home = home();
    let moves = home.join(MOVES).join(id);
    let farm = moves.join("before");
    let done = moves.join("before.done");
    if done.exists() {
        println!("{}", std::fs::read_to_string(&done).unwrap_or_default().trim());
        return 0;
    }
    use std::os::unix::fs::MetadataExt;
    let filter = olo_rex_sync::index::Filter { exclude, ..olo_rex_sync::index::Filter::default() };
    let dev = std::fs::symlink_metadata(&home).map(|m| m.dev()).unwrap_or(0);
    let (mut kept_files, mut failed) = (0u64, Vec::new());
    let mut stack = vec![(home.clone(), String::new())];
    while let Some((dir, rel)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.filter_map(|e| e.ok()) {
            let Some(name) = e.file_name().to_str().map(str::to_owned) else { continue };
            let rel = if rel.is_empty() { name } else { format!("{rel}/{name}") };
            if kept(Path::new(&rel)) {
                continue;
            }
            let Ok(meta) = std::fs::symlink_metadata(e.path()) else { continue };
            if meta.is_dir() {
                if meta.dev() == dev && filter.allow_dir(&rel) {
                    stack.push((e.path(), rel));
                }
                continue;
            }
            if !filter.matches_entry(&rel) || !(meta.is_file() || meta.file_type().is_symlink()) {
                continue;
            }
            let to = farm.join(&rel);
            let made = std::fs::create_dir_all(to.parent().unwrap_or(&farm))
                .and_then(|()| if meta.is_file() { std::fs::hard_link(e.path(), &to) } else { std::os::unix::fs::symlink(std::fs::read_link(e.path())?, &to) });
            match made {
                Ok(()) => kept_files += 1,
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => kept_files += 1,
                Err(e) => failed.push(format!("{rel}: {e}")),
            }
        }
    }
    let said = serde_json::json!({"kept": kept_files, "failed": failed});
    let _ = std::fs::set_permissions(&moves, std::fs::Permissions::from_mode(0o700));
    if failed.is_empty() {
        let _ = write_whole(&done, said.to_string().as_bytes(), 0o600);
    }
    println!("{said}");
    if failed.is_empty() { 0 } else { 1 }
}

/// `move put-back ID`: what `keep` kept put back where it was (Go Back),
/// each file in its place again; the copy's own files beside them are
/// left (the person's to remove, or to move again).
fn put_back(args: &[String]) -> i32 {
    let [id] = args else {
        eprintln!("{USAGE}");
        return 2;
    };
    if !named(id) {
        return 2;
    }
    let home = home();
    let farm = home.join(MOVES).join(id).join("before");
    let (mut back, mut failed) = (0u64, Vec::new());
    let mut stack = vec![(farm.clone(), String::new())];
    while let Some((dir, rel)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.filter_map(|e| e.ok()) {
            let Some(name) = e.file_name().to_str().map(str::to_owned) else { continue };
            let rel = if rel.is_empty() { name } else { format!("{rel}/{name}") };
            let Ok(meta) = std::fs::symlink_metadata(e.path()) else { continue };
            if meta.is_dir() {
                stack.push((e.path(), rel));
                continue;
            }
            let to = home.join(&rel);
            // (A folder the copy made where a file was: the file can't go
            // back over it; said.)
            let put = std::fs::create_dir_all(to.parent().unwrap_or(&home)).and_then(|()| {
                if std::fs::symlink_metadata(&to).is_ok_and(|m| m.is_dir()) {
                    return Err(io::Error::other("a folder is there now"));
                }
                std::fs::rename(e.path(), &to)
            });
            match put {
                Ok(()) => back += 1,
                Err(e) => failed.push(format!("{rel}: {e}")),
            }
        }
    }
    if failed.is_empty() {
        let _ = std::fs::remove_dir_all(&farm);
        let _ = std::fs::remove_file(home.join(MOVES).join(id).join("before.done"));
    }
    println!("{}", serde_json::json!({"back": back, "failed": failed}));
    if failed.is_empty() { 0 } else { 1 }
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
    /// Folders of `/var/tmp` sent too, each to the same place there.
    also: Vec<String>,
    /// A part's share of the home, where it is told (the rig's: a small
    /// home in parts all the same); else a stream's, a gigabyte or more.
    each: Option<u64>,
    detach: bool,
}

/// `HOST`, `HOST:PORT`, `[V6]`, `[V6]:PORT` or a bare v6 address (two
/// colons or more: no port).
fn host_port(at: &str) -> Option<(String, u16)> {
    if let Some(rest) = at.strip_prefix('[') {
        let (host, after) = rest.split_once(']')?;
        let port = match after.strip_prefix(':') {
            Some(p) => p.parse().ok()?,
            None if after.is_empty() => 22,
            None => return None,
        };
        return Some((host.to_owned(), port));
    }
    if at.matches(':').count() > 1 {
        return Some((at.to_owned(), 22));
    }
    match at.rsplit_once(':') {
        Some((h, p)) => Some((h.to_owned(), p.parse().ok()?)),
        None => Some((at.to_owned(), 22)),
    }
}

fn sending(args: &[String]) -> Option<Sending> {
    let mut s = Sending { port: 22, pass: 1, ..Sending::default() };
    let mut it = args.iter();
    s.dir = PathBuf::from(it.next()?);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--to" => {
                let (user, at) = it.next()?.split_once('@')?;
                let (host, port) = host_port(at)?;
                (s.user, s.host, s.port) = (user.to_owned(), host, port);
            }
            "--also" => {
                let root = it.next()?;
                if !var_tmp_root(root) {
                    return None;
                }
                s.also.push(root.clone());
            }
            "--host-key" => s.host_key = it.next()?.split_whitespace().take(2).collect::<Vec<_>>().join(" "),
            "--there" => s.there = it.next()?.clone(),
            "--pass" => s.pass = it.next()?.parse().ok()?,
            "--exclude" => s.exclude.push(it.next()?.clone()),
            "--detach" => s.detach = true,
            "--each" => s.each = Some(it.next()?.parse().ok()?),
            _ => return None,
        }
    }
    let id = s.dir.file_name()?.to_str()?;
    (named(id) && !s.user.is_empty() && !s.host.is_empty() && !s.host.contains(['[', ']', '/']) && s.host_key.split(' ').count() == 2 && Path::new(&s.there).is_absolute() && s.pass >= 1).then_some(s)
}

/// The move's own folder's lock, held by the copy while it runs: one copy
/// of a move at a time (a second start, the app restarted, is refused),
/// and how `status` knows one runs (a pid could be another's by then).
fn lock(dir: &Path) -> io::Result<Option<std::fs::File>> {
    use std::os::fd::AsRawFd;
    let f = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).mode(0o600).open(dir.join("lock"))?;
    // SAFETY: flock on a descriptor of ours.
    if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        Ok(Some(f))
    } else {
        let e = io::Error::last_os_error();
        if e.kind() == io::ErrorKind::WouldBlock { Ok(None) } else { Err(e) }
    }
}

/// How one part of a pass stands (its session's progress, as the engine
/// says it).
#[derive(Debug, Default, Clone)]
struct PartStanding {
    /// What the split measured of it, before its session planned (a first
    /// pass: everything there is new there).
    measured_files: u64,
    measured_bytes: u64,
    /// "waiting", "indexing", "sending", "done", "failed".
    phase: &'static str,
    files: u64,
    bytes: u64,
    files_total: u64,
    bytes_total: u64,
    planned: bool,
}

/// How the copy stands, as `status.json` has it: its parts summed, and
/// how many sessions run at once (`streams`), how many it aims at (`level`)
/// and how fast it goes (`rate`, bytes a second of the files' own).
#[derive(Debug, Default)]
struct Standing {
    pass: u32,
    phase: &'static str,
    parts: Vec<PartStanding>,
    sent: u64,
    current: String,
    started: u64,
    ended: Option<u64>,
    error: Option<String>,
    report: Option<serde_json::Value>,
    streams: usize,
    level: usize,
    rate: u64,
}

impl Standing {
    /// Files and bytes sent, and of all there is to send (a part not yet
    /// planned counted as the split measured it).
    fn totals(&self) -> (u64, u64, u64, u64) {
        self.parts.iter().fold((0, 0, 0, 0), |(f, b, tf, tb), p| {
            let (pf, pb) = if p.planned { (p.files_total, p.bytes_total) } else { (p.measured_files, p.measured_bytes) };
            (f + p.files, b + p.bytes, tf + pf, tb + pb)
        })
    }

    fn json(&self) -> serde_json::Value {
        let (files, bytes, total_files, total_bytes) = self.totals();
        serde_json::json!({
            "pass": self.pass, "phase": self.phase, "total_files": total_files, "total_bytes": total_bytes,
            "files": files, "bytes": bytes, "sent": self.sent, "current": self.current,
            "started": self.started, "ended": self.ended, "error": self.error, "report": self.report,
            "streams": self.streams, "level": self.level, "rate": self.rate,
            "parts": self.parts.len(), "parts_done": self.parts.iter().filter(|p| p.phase == "done").count(),
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

/// A part of the home (or a folder of /var/tmp) as the engine sends it,
/// keeping the paths it sends (the next pass's: what it put there and is
/// gone here is taken away there), and passing on the engine's single read
/// of new files and its cache.
struct Counted {
    inner: olo_rex_sync::wire::FolderSource,
    keep_paths: PathBuf,
}

impl olo_rex_sync::wire::Source for Counted {
    fn index(&self) -> io::Result<olo_rex_sync::Index> {
        let index = self.inner.index()?;
        keep_paths(&self.keep_paths, &index)?;
        Ok(index)
    }

    fn index_with_progress(&self, on_entry: &mut dyn FnMut(&str, u64)) -> io::Result<olo_rex_sync::Index> {
        let index = self.inner.index_with_progress(on_entry)?;
        keep_paths(&self.keep_paths, &index)?;
        Ok(index)
    }

    fn index_lazy_with_progress(&self, on_entry: &mut dyn FnMut(&str, u64)) -> io::Result<Option<(olo_rex_sync::Index, HashSet<String>)>> {
        let found = self.inner.index_lazy_with_progress(on_entry)?;
        if let Some((index, _)) = &found {
            keep_paths(&self.keep_paths, index)?;
        }
        Ok(found)
    }

    fn hash_entry(&self, path: &str, size: u64) -> io::Result<olo_rex_sync::FileDigest> {
        self.inner.hash_entry(path, size)
    }

    fn wants_cache(&self) -> bool {
        self.inner.wants_cache()
    }

    fn save_cache(&self, index: &olo_rex_sync::Index) -> io::Result<()> {
        self.inner.save_cache(index)
    }

    fn check_unchanged(&self, entry: &olo_rex_sync::Entry) -> bool {
        self.inner.check_unchanged(entry)
    }

    fn extent_at(&self, path: &str, offset: u64, max_len: u64) -> io::Result<(bool, u64)> {
        self.inner.extent_at(path, offset, max_len)
    }

    fn read(&self, path: &str, offset: u64, len: usize) -> io::Result<Vec<u8>> {
        self.inner.read(path, offset, len)
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
    // (Held to the end, by the child when it forks: the descriptor goes
    // with it.)
    // (Half a second's grace: `status` takes the lock a moment to see
    // whether it is held.)
    let mut held = lock(&s.dir);
    for _ in 0..10 {
        if !matches!(held, Ok(None)) {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
        held = lock(&s.dir);
    }
    let _lock = match held {
        Ok(Some(l)) => l,
        Ok(None) => {
            println!("{}", serde_json::json!({"error": "a copy of this move runs already"}));
            return 1;
        }
        Err(e) => {
            eprintln!("olo-rex-link move send: {}: {e}", s.dir.display());
            return 1;
        }
    };
    // (This pass's status from the first: the pass before's "done" never
    // read as this one's.)
    let standing: Shared = std::sync::Arc::new(std::sync::Mutex::new(Standing { pass: s.pass, phase: "starting", started: unix_now(), ..Standing::default() }));
    write_standing(&s.dir, &standing);
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
                // (Its streams never the connection that started it, which
                // would wait for them to close: the log, else nowhere.)
                use std::os::fd::AsRawFd;
                if let Ok(null) = std::fs::OpenOptions::new().read(true).write(true).open("/dev/null") {
                    let log = std::fs::OpenOptions::new().create(true).append(true).mode(0o600).open(s.dir.join("log"));
                    let out = log.as_ref().map_or(null.as_raw_fd(), |l| l.as_raw_fd());
                    // SAFETY: replacing this process's standard streams.
                    unsafe {
                        libc::dup2(null.as_raw_fd(), 0);
                        libc::dup2(out, 1);
                        libc::dup2(out, 2);
                    }
                }
            }
            child => {
                let _ = write_whole(&s.dir.join("pid"), format!("{child}\n").as_bytes(), 0o600);
                println!("{{\"started\":{child}}}");
                return 0;
            }
        }
    } else {
        // SAFETY: a getter.
        let me = unsafe { libc::getpid() };
        let _ = write_whole(&s.dir.join("pid"), format!("{me}\n").as_bytes(), 0o600);
    }
    standing.lock().unwrap_or_else(|e| e.into_inner()).phase = "connecting";
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
    // (Threads for the sessions' connections: each encrypts on its own.)
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(2, 16);
    let ended = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(threads)
        .enable_all()
        .build()
        .map_err(|e| e.to_string())
        .and_then(|rt| rt.block_on(copy(std::sync::Arc::new(s.clone()), standing.clone())));
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

/// The paths a pass sent of a part (by its key), by the pass's number.
fn kept_paths(dir: &Path, key: &str, pass: u32) -> PathBuf {
    dir.join(format!("paths-{key}-{pass}"))
}

/// A short name for a root, for its files in the move's folder.
fn key_of(kind: &str, root: &str) -> String {
    let id = olo_rex_sync::chunk::chunk_id(root.as_bytes());
    format!("{kind}{}", hex(&id[..8]))
}

/// What a pass sent, as a sorted list (NUL after each): its index's
/// paths, and those it couldn't read (still there: never taken away as
/// gone). A pass made again (a cut first pass started over) adds to what
/// the one before it kept, so nothing the first try put there is
/// forgotten.
fn keep_paths(file: &Path, index: &olo_rex_sync::Index) -> io::Result<()> {
    let mut paths: Vec<&str> = index.entries.keys().map(String::as_str).collect();
    let unread: Vec<String> = index.skipped().unread.paths;
    paths.extend(unread.iter().map(String::as_str));
    let before = std::fs::read(file).unwrap_or_default();
    paths.extend(before.split(|&b| b == 0).filter(|p| !p.is_empty()).filter_map(|p| std::str::from_utf8(p).ok()));
    paths.sort_unstable();
    paths.dedup();
    let mut out = Vec::with_capacity(paths.iter().map(|p| p.len() + 1).sum());
    for p in paths {
        out.extend_from_slice(p.as_bytes());
        out.push(0);
    }
    write_whole(file, &out, 0o600)
}

/// What was sent before (a sorted list) and isn't now (another): what to
/// take away there. Never what lies on another filesystem here (the copy
/// doesn't go there: its files aren't gone), nor what this pass couldn't
/// read; `None` when there is no list from before (a first pass, or a
/// move whose first pass was never made).
fn gone_since(before: &Path, now: &Path, mounts: &[String]) -> Option<Vec<u8>> {
    let before = std::fs::read(before).ok()?;
    let now = std::fs::read(now).ok()?;
    let now: HashSet<&[u8]> = now.split(|&b| b == 0).filter(|p| !p.is_empty()).collect();
    let under_mount = |p: &[u8]| mounts.iter().any(|m| p == m.as_bytes() || (p.starts_with(m.as_bytes()) && p.get(m.len()) == Some(&b'/')));
    let mut out = Vec::new();
    for p in before.split(|&b| b == 0).filter(|p| !p.is_empty()) {
        if !now.contains(p) && !under_mount(p) {
            out.extend_from_slice(p);
            out.push(0);
        }
    }
    Some(out)
}

/// The destination reached with the move's key, its host key pinned.
async fn connect(s: &Sending) -> Result<russh::client::Handle<Pinned>, String> {
    let private = std::fs::read_to_string(s.dir.join("key")).map_err(|e| format!("the move's key: {e}"))?;
    let key = PrivateKey::from_openssh(&private).map_err(|e| format!("the move's key: {e}"))?;
    // (The pinned key's kind asked for first: the destination shows the
    // key the app checked, whatever its kinds; no SSH compression, the
    // engine compresses; no waiting to fill packets; a window for a far
    // link's round trips.)
    let mut preferred = russh::Preferred::default();
    if let Some(kind) = s.host_key.split_whitespace().next()
        && let Ok(algorithm) = kind.parse::<russh::keys::Algorithm>()
    {
        let mut kinds: Vec<russh::keys::Algorithm> = if kind == "ssh-rsa" {
            use russh::keys::{Algorithm, HashAlg};
            vec![Algorithm::Rsa { hash: Some(HashAlg::Sha512) }, Algorithm::Rsa { hash: Some(HashAlg::Sha256) }, algorithm]
        } else {
            vec![algorithm]
        };
        kinds.extend(preferred.key.iter().filter(|k| !kinds.contains(k)).cloned().collect::<Vec<_>>());
        preferred.key = std::borrow::Cow::Owned(kinds);
    }
    preferred.compression = std::borrow::Cow::Borrowed(&[russh::compression::NONE]);
    let config =
        std::sync::Arc::new(russh::client::Config { keepalive_interval: Some(Duration::from_secs(15)), keepalive_max: 4, nodelay: true, window_size: 16 << 20, preferred, ..Default::default() });
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

/// What a pass sends in a session (and over a connection) of its own: the
/// home's own (all of it but its parts), a part of it (a folder), or a
/// folder of `/var/tmp`.
#[derive(Clone, Debug, PartialEq, Eq)]
enum PartKind {
    Home,
    Folder,
    VarTmp,
}

#[derive(Clone, Debug)]
struct Part {
    kind: PartKind,
    /// Where it is here, and where it goes there.
    here: PathBuf,
    there: String,
    /// The move's exclusions under it, anchored at it.
    exclude: Vec<String>,
    /// Its files' name in the move's folder.
    key: String,
    /// What the split measured of it.
    files: u64,
    bytes: u64,
}

/// The move's root-anchored exclusions as they are under folder `rel` of
/// the root, anchored at it (a name's pattern, anywhere, as it is).
fn excludes_under(exclude: &[String], rel: &str) -> Vec<String> {
    exclude
        .iter()
        .filter_map(|p| match p.strip_prefix('/') {
            Some(anchored) => anchored.strip_prefix(rel).and_then(|r| r.strip_prefix('/')).filter(|r| !r.is_empty()).map(|r| format!("/{r}")),
            None => Some(p.clone()),
        })
        .collect()
}

/// What [`measure_tree`] found: each folder's files and bytes (of all
/// under it), each folder's folders, and the whole's.
struct Tree {
    sizes: std::collections::HashMap<String, (u64, u64)>,
    children: std::collections::HashMap<String, Vec<String>>,
    files: u64,
    bytes: u64,
}

/// The home walked once, as the copy brings it (`filter`, one filesystem):
/// what each folder holds.
fn measure_tree(home: &Path, filter: &olo_rex_sync::index::Filter) -> Tree {
    use std::os::unix::fs::MetadataExt;
    let dev = std::fs::symlink_metadata(home).map(|m| m.dev()).unwrap_or(0);
    let mut sizes: std::collections::HashMap<String, (u64, u64)> = std::collections::HashMap::new();
    let mut children: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
    let (mut files, mut bytes) = (0u64, 0u64);
    let mut stack = vec![String::new()];
    while let Some(rel) = stack.pop() {
        let dir = if rel.is_empty() { home.to_path_buf() } else { home.join(&rel) };
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.filter_map(|e| e.ok()) {
            let Some(name) = e.file_name().to_str().map(str::to_owned) else { continue };
            let child = if rel.is_empty() { name } else { format!("{rel}/{name}") };
            let Ok(meta) = std::fs::symlink_metadata(e.path()) else { continue };
            if meta.is_dir() {
                if meta.dev() == dev && filter.allow_dir(&child) {
                    children.entry(rel.clone()).or_default().push(child.clone());
                    stack.push(child);
                }
                continue;
            }
            if !filter.matches_entry(&child) {
                continue;
            }
            let size = meta.blocks() * 512;
            files += 1;
            bytes += size;
            // (Its size to each folder it is in.)
            let mut at = child.as_str();
            while let Some((up, _)) = at.rsplit_once('/') {
                match sizes.get_mut(up) {
                    Some(s) => {
                        s.0 += 1;
                        s.1 += size;
                    }
                    None => {
                        sizes.insert(up.to_owned(), (1, size));
                    }
                }
                at = up;
            }
        }
    }
    Tree { sizes, children, files, bytes }
}

/// The home split for sending in parallel: folders of it (by their paths
/// under it), each a session of its own, of about `each` bytes or more;
/// the biggest split into their folders until none is much bigger (a
/// folder's own files stay with the home's session); never a folder on the
/// way to what the destination keeps (`.ssh`, `.local`), nor one the move
/// leaves out (the walk didn't go there). With what each holds, the
/// biggest first, at most `most`.
fn split(tree: &Tree, each: u64, most: usize) -> Vec<(String, u64, u64)> {
    let (sizes, children) = (&tree.sizes, &tree.children);
    let may_be = |rel: &str| !kept(Path::new(rel));
    let size_of = |rel: &str| sizes.get(rel).copied().unwrap_or_default();
    let mut parts: Vec<String> = children.get("").cloned().unwrap_or_default().into_iter().filter(|r| may_be(r)).collect();
    for _ in 0..2000 {
        parts.sort_by_key(|r| std::cmp::Reverse(size_of(r).1));
        let Some(at) = parts.iter().position(|r| size_of(r).1 > each + each / 2 && children.get(r).is_some_and(|c| c.iter().any(|c| may_be(c)))) else { break };
        let big = parts.remove(at);
        parts.extend(children.get(&big).cloned().unwrap_or_default().into_iter().filter(|r| may_be(r)));
    }
    let least = if each < 1 << 30 { each / 4 } else { (each / 4).max(256 << 20) };
    let mut out: Vec<(String, u64, u64)> = parts.into_iter().map(|r| (r.clone(), size_of(&r).0, size_of(&r).1)).filter(|p| p.2 >= least).collect();
    out.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.cmp(&b.0)));
    out.truncate(most);
    out
}

/// How many sessions a pass runs at once: as many as make it go fastest,
/// found as it goes. More while each step up makes it faster; held where
/// it stops; tried one more now and then, and one fewer kept where that
/// is slower. (A session is never cut: fewer means none started until the
/// running are fewer.)
#[derive(Debug)]
struct Streams {
    level: usize,
    most: usize,
    last_bytes: u64,
    last_rate: f64,
    ticks: u32,
    state: Climb,
}

#[derive(Debug, PartialEq)]
enum Climb {
    Rising,
    Holding { until: u32 },
    Trying { from: f64 },
}

impl Streams {
    fn new(level: usize, most: usize) -> Streams {
        Streams { level: level.clamp(1, most.max(1)), most: most.max(1), last_bytes: 0, last_rate: 0.0, ticks: 0, state: Climb::Rising }
    }

    /// What has been sent by now (the files' own bytes), `secs` after the
    /// last look: the level for the next while.
    fn tick(&mut self, bytes: u64, secs: f64) -> usize {
        let rate = bytes.saturating_sub(self.last_bytes) as f64 / secs.max(0.001);
        self.last_bytes = bytes;
        self.ticks += 1;
        // (Nothing sent yet, sessions still looking at their files: no
        // judgment of the level by that.)
        if rate <= 0.0 {
            return self.level;
        }
        match self.state {
            Climb::Rising => {
                if (self.last_rate == 0.0 || rate >= self.last_rate * 1.05) && self.level < self.most {
                    self.level = (self.level + (self.level / 4).max(1)).min(self.most);
                } else {
                    if rate < self.last_rate * 0.85 {
                        self.level = (self.level - 1).max(1);
                    }
                    self.state = Climb::Holding { until: self.ticks + 4 };
                }
            }
            Climb::Holding { until } => {
                if self.ticks >= until && self.level < self.most {
                    self.state = Climb::Trying { from: rate };
                    self.level += 1;
                }
            }
            Climb::Trying { from } => {
                if rate >= from * 1.05 {
                    self.state = Climb::Rising;
                } else {
                    self.level = (self.level - 1).max(1);
                    self.state = Climb::Holding { until: self.ticks + 8 };
                }
            }
        }
        self.last_rate = rate;
        self.level
    }
}

/// The parts a pass sends: the home's split, kept from its first pass in
/// the move's folder (a later pass's, and a first pass's started again, are
/// the same: what each sent is told by it), the home's own session, and
/// each folder of `/var/tmp`.
fn parts_of(s: &Sending, sessions_most: usize) -> io::Result<Vec<Part>> {
    let file = s.dir.join("parts.json");
    let home = home();
    let saved: Option<Vec<(String, u64, u64)>> = std::fs::read(&file).ok().and_then(|b| serde_json::from_slice(&b).ok());
    let (split_parts, total) = match saved {
        Some(parts) => (parts, None),
        None => {
            let filter = olo_rex_sync::index::Filter { exclude: s.exclude.clone(), ..olo_rex_sync::index::Filter::default() };
            // (About three parts a stream, so the number running can be
            // changed as it goes; none smaller than a gigabyte.)
            let tree = measure_tree(&home, &filter);
            let each = s.each.unwrap_or_else(|| (tree.bytes / (3 * sessions_most as u64).max(1)).max(1 << 30));
            let parts = split(&tree, each, 64);
            write_whole(&file, serde_json::to_string(&parts).unwrap_or_default().as_bytes(), 0o600)?;
            (parts, Some((tree.files, tree.bytes)))
        }
    };
    let here = |rel: &str| home.join(rel);
    // (A part no longer a folder here is the home's own session's again.)
    let folders: Vec<&(String, u64, u64)> = split_parts.iter().filter(|(rel, _, _)| std::fs::symlink_metadata(here(rel)).is_ok_and(|m| m.is_dir())).collect();
    let mut home_exclude = s.exclude.clone();
    home_exclude.extend(folders.iter().map(|(rel, _, _)| format!("/{rel}")));
    let (in_parts_files, in_parts_bytes) = folders.iter().fold((0, 0), |(f, b), p| (f + p.1, b + p.2));
    let (home_files, home_bytes) = total.map_or((0, 0), |(f, b)| (f.saturating_sub(in_parts_files), b.saturating_sub(in_parts_bytes)));
    let mut parts = vec![Part { kind: PartKind::Home, here: home.clone(), there: s.there.clone(), exclude: home_exclude, key: "home".into(), files: home_files, bytes: home_bytes }];
    for (rel, files, bytes) in split_parts.iter() {
        let gone = !folders.iter().any(|p| &p.0 == rel);
        parts.push(Part {
            kind: PartKind::Folder,
            here: here(rel),
            there: format!("{}/{rel}", s.there.trim_end_matches('/')),
            exclude: excludes_under(&s.exclude, rel),
            key: key_of("p", rel),
            files: if gone || total.is_none() { 0 } else { *files },
            bytes: if gone || total.is_none() { 0 } else { *bytes },
        });
    }
    for root in &s.also {
        parts.push(Part { kind: PartKind::VarTmp, here: PathBuf::from(root), there: root.clone(), exclude: Vec::new(), key: key_of("t", root), files: 0, bytes: 0 });
    }
    Ok(parts)
}

/// A pass: its parts sent, the home's own session first and then the
/// biggest, as many at once as make it go fastest ([`Streams`]). The home's
/// session failing, or a part's, is the pass's (what the others did is
/// kept: a pass started again finds it there); a folder of `/var/tmp`'s is
/// said in the report.
async fn copy(s: std::sync::Arc<Sending>, standing: Shared) -> Result<(), String> {
    let most = std::thread::available_parallelism().map_or(8, |n| n.get()).clamp(2, 24);
    standing.lock().unwrap_or_else(|e| e.into_inner()).phase = "splitting";
    let parts = {
        let s = s.clone();
        tokio::task::spawn_blocking(move || parts_of(&s, most)).await.map_err(|e| e.to_string())?.map_err(|e| format!("the home's split: {e}"))?
    };
    {
        let mut st = standing.lock().unwrap_or_else(|e| e.into_inner());
        st.parts = parts.iter().map(|p| PartStanding { measured_files: p.files, measured_bytes: p.bytes, phase: "waiting", ..PartStanding::default() }).collect();
        st.phase = "copying";
    }
    let mut whole = serde_json::json!({"same": 0, "copied": 0, "patched": 0, "failed": 0, "failures": [], "wire": 0, "gone": 0,
        "skipped": {"unread": {"count": 0, "paths": []}, "odd_names": {"count": 0, "paths": []}, "special": {"count": 0, "paths": []}, "mounts": {"count": 0, "paths": []}}});
    let mut waiting: std::collections::VecDeque<usize> = (0..parts.len()).collect();
    let mut running = tokio::task::JoinSet::new();
    let mut streams = Streams::new(4, most);
    let mut failed: Option<String> = None;
    let every = Duration::from_secs(15);
    let mut ticker = tokio::time::interval_at(tokio::time::Instant::now() + every, every);
    loop {
        while failed.is_none()
            && running.len() < streams.level
            && let Some(k) = waiting.pop_front()
        {
            running.spawn(send_part(s.clone(), parts[k].clone(), k, standing.clone()));
        }
        {
            let mut st = standing.lock().unwrap_or_else(|e| e.into_inner());
            (st.streams, st.level) = (running.len(), streams.level);
        }
        if running.is_empty() {
            break;
        }
        tokio::select! {
            Some(done) = running.join_next() => match done {
                Ok((k, Ok(report))) => {
                    add_report(&mut whole, &report.0);
                    whole["gone"] = (whole["gone"].as_u64().unwrap_or(0) + report.1).into();
                    standing.lock().unwrap_or_else(|e| e.into_inner()).parts[k].phase = "done";
                }
                Ok((k, Err(e))) => {
                    standing.lock().unwrap_or_else(|e| e.into_inner()).parts[k].phase = "failed";
                    if parts[k].kind == PartKind::VarTmp {
                        let failure = serde_json::json!({"path": parts[k].there, "why": e});
                        match whole["also_failed"].as_array_mut() {
                            Some(list) => list.push(failure),
                            None => whole["also_failed"] = serde_json::json!([failure]),
                        }
                    } else if failed.is_none() {
                        failed = Some(if parts[k].kind == PartKind::Home { e } else { format!("{}: {e}", parts[k].here.display()) });
                    }
                }
                Err(e) => failed = failed.or(Some(e.to_string())),
            },
            _ = ticker.tick() => {
                let sent = standing.lock().unwrap_or_else(|e| e.into_inner()).totals().1;
                streams.tick(sent, every.as_secs_f64());
                standing.lock().unwrap_or_else(|e| e.into_inner()).rate = streams.last_rate as u64;
            }
        }
        standing.lock().unwrap_or_else(|e| e.into_inner()).report = Some(whole.clone());
    }
    match failed {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// One part sent in a session of its own, over a connection of its own:
/// its report and how many of the destination's it took away (a later
/// pass's: what the passes before put there and is gone here).
async fn send_part(s: std::sync::Arc<Sending>, part: Part, k: usize, standing: Shared) -> (usize, Result<(olo_rex_sync::wire::Report, u64), String>) {
    let ended = async {
        let session = connect(&s).await?;
        let mut gone_count = 0u64;
        let here_is = std::fs::symlink_metadata(&part.here).is_ok_and(|m| m.is_dir());
        let report = if here_is {
            standing.lock().unwrap_or_else(|e| e.into_inner()).parts[k].phase = "indexing";
            // The engine's receiving side there.
            let channel = session.channel_open_session().await.map_err(|e| e.to_string())?;
            let ask = serde_json::json!({"op": "sync", "root": part.there, "options": ["--scoped"]});
            channel.exec(true, command_for(&ask)).await.map_err(|e| e.to_string())?;
            let filter = olo_rex_sync::index::Filter { exclude: part.exclude.clone(), ..olo_rex_sync::index::Filter::default() };
            let source = Counted {
                inner: olo_rex_sync::wire::FolderSource { root: part.here.clone(), filter: filter.clone(), cache_path: Some(s.dir.join(format!("index-cache-{}", part.key))) },
                keep_paths: kept_paths(&s.dir, &part.key, s.pass),
            };
            let shared = standing.clone();
            let progress: olo_rex_sync::wire::ProgressCallback = std::sync::Arc::new(move |p: &olo_rex_sync::wire::Progress| {
                let mut st = shared.lock().unwrap_or_else(|e| e.into_inner());
                let mine = &mut st.parts[k];
                if p.phase == olo_rex_sync::wire::Phase::Indexing {
                    mine.phase = "indexing";
                } else {
                    (mine.phase, mine.planned) = ("sending", true);
                    (mine.files, mine.bytes, mine.files_total, mine.bytes_total) = (p.files, p.bytes, p.files_total, p.bytes_total);
                }
                if !p.current.is_empty() {
                    p.current.clone_into(&mut st.current);
                }
            });
            let io = Blocking { stream: channel.into_stream(), handle: tokio::runtime::Handle::current(), standing: standing.clone() };
            let options = olo_rex_sync::wire::Options { filter, progress: Some(progress), ..olo_rex_sync::wire::Options::default() };
            tokio::task::spawn_blocking(move || olo_rex_sync::wire::send(io, &source, options)).await.map_err(|e| e.to_string())?.map_err(|e| match e.kind() {
                io::ErrorKind::UnexpectedEof | io::ErrorKind::BrokenPipe | io::ErrorKind::ConnectionReset => format!("the copy ended early ({e})"),
                _ => e.to_string(),
            })?
        } else {
            olo_rex_sync::wire::Report::default()
        };
        // A later pass: what the passes before put there and is gone here
        // taken away there (and nothing else of the destination's); a part
        // gone here whole, all it sent, and its folder.
        if s.pass > 1 {
            let before = kept_paths(&s.dir, &part.key, s.pass - 1);
            let gone = if here_is { gone_since(&before, &kept_paths(&s.dir, &part.key, s.pass), &report.skipped.mounts.paths) } else { std::fs::read(&before).ok() };
            if let Some(gone) = gone.filter(|g| !g.is_empty()) {
                gone_count += forget_there(&session, &part.there, &gone).await?;
            }
            if !here_is
                && part.kind == PartKind::Folder
                && let Some(rel) = part.there.strip_prefix(&format!("{}/", s.there.trim_end_matches('/')))
            {
                let mut list = rel.as_bytes().to_vec();
                list.push(0);
                gone_count += forget_there(&session, &s.there, &list).await?;
            }
        }
        let _ = session.disconnect(russh::Disconnect::ByApplication, "", "").await;
        Ok((report, gone_count))
    }
    .await;
    (k, ended)
}

/// The paths of `list` (NUL after each) taken away under `root` there: how
/// many went.
async fn forget_there(session: &russh::client::Handle<Pinned>, root: &str, list: &[u8]) -> Result<u64, String> {
    let channel = session.channel_open_session().await.map_err(|e| e.to_string())?;
    channel.exec(true, command_for(&serde_json::json!({"op": "forget", "root": root}))).await.map_err(|e| e.to_string())?;
    let mut stream = channel.into_stream();
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    stream.write_all(list).await.map_err(|e| e.to_string())?;
    stream.shutdown().await.map_err(|e| e.to_string())?;
    let mut said = String::new();
    stream.read_to_string(&mut said).await.map_err(|e| e.to_string())?;
    let v: serde_json::Value = said.lines().rev().find_map(|l| serde_json::from_str(l).ok()).unwrap_or_default();
    Ok(v["gone"].as_u64().unwrap_or(0))
}

/// A root's report added to the pass's: counts summed, what failed and
/// what was passed over named (the first 200 of each).
fn add_report(whole: &mut serde_json::Value, report: &olo_rex_sync::wire::Report) {
    let add = |v: &mut serde_json::Value, n: u64| *v = (v.as_u64().unwrap_or(0) + n).into();
    add(&mut whole["same"], report.same);
    add(&mut whole["copied"], report.copied + report.new);
    add(&mut whole["patched"], report.patched + report.appended);
    add(&mut whole["failed"], report.errors.len() as u64);
    add(&mut whole["wire"], report.wire_bytes());
    if let Some(list) = whole["failures"].as_array_mut() {
        list.extend(report.errors.iter().take(200usize.saturating_sub(list.len())).map(|e| serde_json::json!({"path": e.path, "why": e.reason})));
    }
    // (What the engine passed over, counted and named: files it couldn't
    // read, names that aren't UTF-8, sockets and FIFOs, other filesystems.)
    let skipped = &report.skipped;
    for (key, kind) in [("unread", &skipped.unread), ("odd_names", &skipped.non_utf8), ("special", &skipped.special), ("mounts", &skipped.mounts)] {
        let to = &mut whole["skipped"][key];
        add(&mut to["count"], kind.count);
        if let Some(list) = to["paths"].as_array_mut() {
            list.extend(kind.paths.iter().take(200usize.saturating_sub(list.len())).map(|p| serde_json::Value::String(p.clone())));
        }
    }
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
    // (Running while its lock is held: a pid could be another's by now.)
    let running = dir.join("lock").exists() && matches!(lock(&dir), Ok(None));
    v["running"] = serde_json::Value::Bool(running);
    println!("{v}");
    0
}

/// A process of this host: its parent, its name, its state.
#[derive(Clone, Debug)]
pub(crate) struct Proc {
    pub pid: i32,
    pub ppid: i32,
    uid: u32,
    pub comm: String,
    state: char,
    user_manager: bool,
}

pub(crate) fn proc_of(pid: i32) -> Option<Proc> {
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

pub(crate) fn all_procs() -> Vec<Proc> {
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

/// Whether `name` is a program here: on the path, or where an SSH
/// command's path leaves out (Ubuntu's has no `/usr/sbin`, where `usermod`
/// and `restorecon` are; `~/.local/bin`, the account's own).
fn on_path(name: &str) -> bool {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    dirs.extend(["/usr/local/sbin", "/usr/sbin", "/sbin", "/usr/local/bin", "/usr/bin", "/bin"].map(PathBuf::from));
    dirs.push(home().join(".local/bin"));
    dirs.iter().any(|d| std::fs::metadata(d.join(name)).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0))
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

/// The services Podman makes of the account's Quadlets
/// (`~/.config/containers/systemd`: `web.container` is `web.service`,
/// `data.volume` `data-volume.service`): enabled by being there, so
/// stopped and masked rather than disabled.
fn quadlets(dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(dir)
        .map(|d| {
            d.filter_map(|e| {
                let name = e.ok()?.file_name().into_string().ok()?;
                let (stem, kind) = name.rsplit_once('.')?;
                Some(match kind {
                    "container" | "kube" => format!("{stem}.service"),
                    "volume" | "network" | "pod" | "image" | "build" => format!("{stem}-{kind}.service"),
                    _ => return None,
                })
            })
            .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

/// The account's own systemd units (in its own folder, not the system's
/// user units: dbus, gpg-agent, pipewire), and which it has enabled (a
/// template's instances by their own names: `worker@1.service`).
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

/// What a home holds that a move brings: its files' room on disk (their
/// blocks: a sparse file's holes stay holes there) and how many, passed
/// over as the copy passes them over (`filter`, one filesystem), and its
/// biggest folders two deep (the first `top`).
fn measure(root: &Path, filter: &olo_rex_sync::index::Filter, top: usize) -> (u64, u64, Vec<(String, u64)>) {
    use std::os::unix::fs::MetadataExt;
    let dev = std::fs::symlink_metadata(root).map(|m| m.dev()).unwrap_or(0);
    let (mut bytes, mut files) = (0u64, 0u64);
    let mut folders: std::collections::HashMap<String, u64> = std::collections::HashMap::new();
    let mut stack = vec![(root.to_path_buf(), String::new())];
    while let Some((dir, rel)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.filter_map(|e| e.ok()) {
            let Some(name) = e.file_name().to_str().map(str::to_owned) else { continue };
            let rel = if rel.is_empty() { name } else { format!("{rel}/{name}") };
            let Ok(meta) = std::fs::symlink_metadata(e.path()) else { continue };
            if meta.is_dir() {
                if meta.dev() == dev && filter.allow_dir(&rel) {
                    stack.push((e.path(), rel));
                }
                continue;
            }
            if !filter.matches_entry(&rel) {
                continue;
            }
            let size = meta.blocks() * 512;
            bytes += size;
            files += 1;
            let two: String = rel.splitn(3, '/').take(2).collect::<Vec<_>>().join("/");
            *folders.entry(two).or_default() += size;
        }
    }
    let mut big: Vec<(String, u64)> = folders.into_iter().collect();
    big.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    big.truncate(top);
    (bytes, files, big)
}

/// The account's own folders in `/var/tmp` (its scratch: agents keep
/// theirs there), each with its room and files; a folder looked into no
/// further than a few hundred thousand entries (`more`).
fn var_tmp_folders(uid: u32) -> (Vec<serde_json::Value>, u64) {
    use std::os::unix::fs::MetadataExt;
    let (mut out, mut loose) = (Vec::new(), 0u64);
    let Ok(rd) = std::fs::read_dir("/var/tmp") else { return (out, loose) };
    // (Every folder counted whole, a minute at most for them all: past
    // it, what is left is `more`.)
    let until = Instant::now() + Duration::from_secs(60);
    for e in rd.filter_map(|e| e.ok()) {
        let Some(name) = e.file_name().to_str().map(str::to_owned) else { continue };
        let Ok(meta) = std::fs::symlink_metadata(e.path()) else { continue };
        if meta.uid() != uid {
            continue;
        }
        if !meta.is_dir() || !var_tmp_root(&format!("/var/tmp/{name}")) {
            // (A file of the account's straight in /var/tmp: not carried.)
            loose += 1;
            continue;
        }
        let (mut bytes, mut files, mut more) = (0u64, 0u64, false);
        let mut stack = vec![e.path()];
        while let Some(dir) = stack.pop() {
            if Instant::now() > until {
                more = true;
                break;
            }
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            for f in rd.filter_map(|f| f.ok()) {
                let Ok(m) = std::fs::symlink_metadata(f.path()) else { continue };
                if m.is_dir() {
                    if m.dev() == meta.dev() {
                        stack.push(f.path());
                    }
                } else {
                    bytes += m.blocks() * 512;
                    files += 1;
                }
            }
        }
        out.push(serde_json::json!({"path": format!("/var/tmp/{name}"), "bytes": bytes, "files": files, "more": more}));
    }
    out.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
    (out, loose)
}

/// Codex's home (`CODEX_HOME`, else `~/.codex`).
pub(crate) fn codex_home() -> PathBuf {
    std::env::var_os("CODEX_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".codex"))
}

/// Codex's daemon (`codex app-server`), as its pid file names it (JSON:
/// its pid and start), if that process is still it.
fn codex_daemon() -> Option<i32> {
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(codex_home().join("app-server-daemon/daemon.pid")).ok()?).ok()?;
    let pid = i32::try_from(v["pid"].as_i64()?).ok()?;
    let ticks = v["processIdentity"]["startTicks"].as_u64()?;
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let start: u64 = stat.rsplit_once(')')?.1.split_whitespace().nth(19)?.parse().ok()?;
    let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    (start == ticks && cmdline.split(|&b| b == 0).any(|w| w == b"app-server")).then_some(pid)
}

/// `move halt DIR`: the move's copy that runs from DIR ended (a move given
/// up): by its pid file, once that process is seen to be that copy (its
/// command line `move send` for DIR), asked first, then ended.
fn halt(args: &[String]) -> i32 {
    let [dir] = args else {
        eprintln!("{USAGE}");
        return 2;
    };
    let dir = PathBuf::from(dir);
    let Some(pid) = std::fs::read_to_string(dir.join("pid")).ok().and_then(|s| s.trim().parse::<i32>().ok()) else {
        println!("{}", serde_json::json!({"halted": false}));
        return 0;
    };
    let ours = std::fs::read(format!("/proc/{pid}/cmdline")).is_ok_and(|c| {
        let words: Vec<&[u8]> = c.split(|&b| b == 0).collect();
        words.contains(&b"move".as_slice()) && words.contains(&b"send".as_slice()) && words.iter().any(|w| Path::new(std::ffi::OsStr::from_bytes_compat(w)) == dir)
    });
    if !ours {
        println!("{}", serde_json::json!({"halted": false}));
        return 0;
    }
    // SAFETY: a signal to the move's own copy, checked above.
    unsafe { libc::kill(pid, libc::SIGTERM) };
    let until = Instant::now() + Duration::from_secs(5);
    while Instant::now() < until && proc_of(pid).is_some_and(|p| p.state != 'Z') {
        std::thread::sleep(Duration::from_millis(100));
    }
    if proc_of(pid).is_some_and(|p| p.state != 'Z') {
        // SAFETY: as above.
        unsafe { libc::kill(pid, libc::SIGKILL) };
    }
    println!("{}", serde_json::json!({"halted": true}));
    0
}

/// `move codex-stop`: Codex's daemon asked to end, and ten seconds given
/// (after its windows: a live one starts it again), as Codex's own restart
/// does; what is left of it `move stop` ends.
fn codex_stop() -> i32 {
    let Some(pid) = codex_daemon() else {
        println!("{}", serde_json::json!({"stopped": false, "running": false}));
        return 0;
    };
    // SAFETY: a signal to the account's own Codex daemon, checked above.
    unsafe { libc::kill(pid, libc::SIGTERM) };
    let until = Instant::now() + Duration::from_secs(10);
    while Instant::now() < until && proc_of(pid).is_some_and(|p| p.state != 'Z') {
        std::thread::sleep(Duration::from_millis(100));
    }
    let gone = !proc_of(pid).is_some_and(|p| p.state != 'Z');
    println!("{}", serde_json::json!({"stopped": gone, "running": !gone, "pid": pid}));
    0
}

/// Whether process `p` is an SSH login with a terminal (a person's, or an
/// agent's `ssh host`), not a command's (Rex's own connections, this one).
fn interactive_login(p: &Proc) -> bool {
    (p.comm == "sshd-session" || p.comm == "sshd") && std::fs::read(format!("/proc/{}/cmdline", p.pid)).is_ok_and(|c| String::from_utf8_lossy(&c).contains("@pts/"))
}

/// `move look [--size] [--exclude PATTERN]…`: what this host and account
/// are, for a move: system, machine, the account, its home's room, what it
/// may use, its own units, what listens, who else is logged in (JSON).
/// With `--size`, what its home holds that the copy brings (the exclusions
/// the copy's), its biggest folders and its folders in `/var/tmp`.
/// Changes nothing.
fn look(args: &[String]) -> i32 {
    let (mut sized, mut exclude) = (false, Vec::new());
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match (a.as_str(), a == "--exclude") {
            ("--size", _) => sized = true,
            (_, true) => match it.next() {
                Some(p) => exclude.push(p.clone()),
                None => {
                    eprintln!("{USAGE}");
                    return 2;
                }
            },
            _ => {
                eprintln!("{USAGE}");
                return 2;
            }
        }
    }
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
    let quadlet_services = quadlets(&config.join("containers/systemd"));
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
    // (With a terminal: Rex's own connections, this one among them, have
    // none.)
    let logins = procs.iter().filter(|p| p.uid == uid && !line.contains(&p.pid) && interactive_login(p)).count();
    let shells: Vec<String> = std::fs::read_to_string("/etc/shells").map(|t| t.lines().map(str::trim).filter(|l| l.starts_with('/')).map(str::to_owned).collect()).unwrap_or_default();
    let home_meta = std::fs::metadata(&home_path).ok();
    let ssh_port = std::env::var("SSH_CONNECTION").ok().and_then(|c| c.split_whitespace().nth(3).and_then(|p| p.parse::<u16>().ok()));
    let mut v = serde_json::json!({
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
        "units": units, "enabled": enabled, "quadlets": quadlet_services,
        "listening": ports,
        "other_logins": logins,
        "mounts": mounts_under(&std::fs::read_to_string("/proc/self/mountinfo").unwrap_or_default(), &home_path),
        "tmux": Path::new(&format!("/tmp/tmux-{uid}")).exists(),
        "screen": Path::new(&format!("/run/screen/S-{user}")).exists() || Path::new(&format!("/var/run/screen/S-{user}")).exists(),
        "machine_id": std::fs::read_to_string("/etc/machine-id").unwrap_or_default().trim(),
        "boot_id": std::fs::read_to_string("/proc/sys/kernel/random/boot_id").unwrap_or_default().trim(),
        "home_dev": home_meta.as_ref().map(std::os::unix::fs::MetadataExt::dev),
        "var_tmp_dev": std::fs::metadata("/var/tmp").ok().as_ref().map(std::os::unix::fs::MetadataExt::dev),
        "home_ino": home_meta.as_ref().map(std::os::unix::fs::MetadataExt::ino),
        "ssh_port": ssh_port,
        "now": unix_now(),
        "helper": olo_rex_link::VERSION.to_string(),
        "shells": shells,
        "codex_daemon": codex_daemon().is_some(),
    });
    if sized {
        let filter = olo_rex_sync::index::Filter { exclude, ..olo_rex_sync::index::Filter::default() };
        let (bytes, files, big) = measure(&home_path, &filter, 8);
        v["home_bytes"] = bytes.into();
        v["home_files"] = files.into();
        v["big"] = big.into_iter().map(|(path, bytes)| serde_json::json!({"path": path, "bytes": bytes})).collect();
        let (folders, loose) = var_tmp_folders(uid);
        v["var_tmp"] = folders.into();
        v["var_tmp_loose"] = loose.into();
    }
    println!("{v}");
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
        assert_eq!(
            line,
            "restrict,expiry-time=\"202611261200\",command=\"/home/f/.local/share/olo/rex-link/olo-rex-link-1-x86_64 move receive --id m1\" ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIA olo-move-m1"
        );
        // (A home with a space: the helper's path quoted for the shell.)
        assert!(allowed_line("ssh-ed25519 AAAA x", "m1", Path::new("/home/a b/h"), "1").unwrap().contains("command=\"'/home/a b/h' move receive --id m1\""));
        assert!(allowed_line("ssh-ed25519 AAAA x", "m1", Path::new("/home/a'b/h"), "1").is_none());
        assert!(allowed_line("ssh-ed25519 AAAA x", "m 1", Path::new("/h"), "1").is_none());
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
        // (The home itself, by its real path; a folder under it a part of
        // the copy goes to, never one through `..`.)
        let home = PathBuf::from(format!("/var/tmp/olo-rex-link-roots-{}", std::process::id()));
        std::fs::create_dir_all(home.join("proj")).unwrap();
        let h = home.display().to_string();
        // SAFETY: a getter.
        let uid = unsafe { libc::getuid() };
        assert_eq!(writable_root(&h, &home, uid, &[]), Some(Root::Home));
        assert_eq!(writable_root(&format!("{h}/proj"), &home, uid, &[]), Some(Root::Part));
        assert_eq!(writable_root(&format!("{h}/proj/.."), &home, uid, &[]), None);
        assert_eq!(writable_root("/etc", &home, uid, &[]), None);
        assert_eq!(writable_root("var/tmp/x", &home, uid, &[]), None);
        // (A folder of /var/tmp the app named for the move: one not there
        // yet may be made; one not named, deeper, or another's may not.)
        let named = ["/var/tmp/olo-move-test-none-such".to_owned()];
        assert_eq!(writable_root("/var/tmp/olo-move-test-none-such", &home, uid, &named), Some(Root::VarTmp));
        assert_eq!(writable_root("/var/tmp/olo-move-test-none-such", &home, uid, &[]), None);
        assert_eq!(writable_root("/var/tmp/olo-move-test-none-such/x", &home, uid, &named), None);
        assert_eq!(writable_root("/var/tmp", &home, uid, &named), None);
        assert!(kept(Path::new(".ssh/authorized_keys")) && kept(Path::new(".ssh")) && kept(Path::new(".local/state/olo-rex/move/m1/key")) && !kept(Path::new(".ssh/config")));
        let _ = std::fs::remove_dir_all(&home);
    }

    /// `forget` takes away what it is given, but never through a link, and
    /// opens a read-only folder a moment (its mode put back).
    #[test]
    fn forgetting_goes_through_no_link() {
        let home = PathBuf::from(format!("/var/tmp/olo-rex-link-forget-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let outside = home.with_extension("outside");
        let _ = std::fs::remove_dir_all(&outside);
        std::fs::create_dir_all(home.join("ro/sub")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(home.join("ro/sub/f"), "x").unwrap();
        std::fs::write(home.join("gone.txt"), "x").unwrap();
        std::fs::write(outside.join("victim"), "x").unwrap();
        std::os::unix::fs::symlink(&outside, home.join("link")).unwrap();
        std::fs::set_permissions(home.join("ro/sub"), std::fs::Permissions::from_mode(0o555)).unwrap();
        assert!(!folders_on_the_way(&home, Path::new("link/victim")));
        assert!(folders_on_the_way(&home, Path::new("ro/sub/f")));
        assert!(remove_entry(&home.join("ro/sub/f")) && !home.join("ro/sub/f").exists());
        assert_eq!(std::fs::metadata(home.join("ro/sub")).unwrap().permissions().mode() & 0o777, 0o555, "its mode put back");
        assert!(remove_entry(&home.join("gone.txt")) && !home.join("gone.txt").exists());
        assert!(outside.join("victim").exists());
        std::fs::set_permissions(home.join("ro/sub"), std::fs::Permissions::from_mode(0o755)).unwrap();
        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::remove_dir_all(&outside);
    }

    /// A key's kind and body are found after options whose quoted values
    /// hold spaces; comments and blank lines have none.
    #[test]
    fn a_keys_line_is_read_past_its_options() {
        assert_eq!(line_key("ssh-ed25519 AAAA me@laptop"), Some(("ssh-ed25519".into(), "AAAA".into())));
        assert_eq!(line_key("command=\"echo a b\",no-pty ssh-rsa BBBB ci"), Some(("ssh-rsa".into(), "BBBB".into())));
        assert_eq!(line_key("from=\"10.0.0.1\" ecdsa-sha2-nistp256 CCCC"), Some(("ecdsa-sha2-nistp256".into(), "CCCC".into())));
        assert_eq!(line_key("# ssh-ed25519 AAAA"), None);
        assert_eq!(line_key("   "), None);
    }

    /// A part's exclusions are the move's, anchored at the part; a name's
    /// anywhere stays as it is; none that is the part itself.
    #[test]
    fn a_parts_exclusions_are_anchored_at_it() {
        let ex = ["/.cache", "/src/olo/target", "/src/olo", "*.swp", "/src/olo/x/*.tmp"].map(String::from);
        assert_eq!(excludes_under(&ex, "src/olo"), ["/target", "*.swp", "/x/*.tmp"]);
        assert_eq!(excludes_under(&ex, "docs"), ["*.swp"]);
    }

    /// The home is split by size: the biggest folders split into theirs
    /// until none is much bigger than a part's share; never a folder on the
    /// way to what the destination keeps; tiny ones left to the home's own
    /// session.
    #[test]
    fn the_home_is_split_by_size() {
        let home = PathBuf::from(format!("/var/tmp/olo-rex-link-split-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let file = |rel: &str, mb: usize| {
            let p = home.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, vec![7u8; mb << 20]).unwrap();
        };
        file("big/a/f", 300);
        file("big/b/f", 300);
        file("big/top", 1);
        file("mid/f", 300);
        file("small/f", 1);
        file(".local/share/x/f", 300);
        let tree = measure_tree(&home, &olo_rex_sync::index::Filter::default());
        let parts = split(&tree, 300 << 20, 64);
        let names: Vec<&str> = parts.iter().map(|p| p.0.as_str()).collect();
        // (A share of 300 MB, and none smaller than 256 MB: `big` split in
        // two, `mid` whole; `small` the home's; nothing of `.local`.)
        assert_eq!(names, ["big/a", "big/b", "mid"], "{parts:?}");
        assert!(tree.bytes >= 1200 << 20);
        let _ = std::fs::remove_dir_all(&home);
    }

    /// A folder under the home is a root a part may go to; never one on
    /// the way to what the destination keeps, nor one reached through a
    /// link.
    #[test]
    fn a_part_goes_only_where_nothing_kept_is() {
        let home = PathBuf::from(format!("/var/tmp/olo-rex-link-part-root-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join("src/olo")).unwrap();
        std::fs::create_dir_all(home.with_extension("elsewhere")).unwrap();
        std::os::unix::fs::symlink(home.with_extension("elsewhere"), home.join("link")).unwrap();
        let h = home.display().to_string();
        // SAFETY: a getter.
        let uid = unsafe { libc::getuid() };
        assert_eq!(writable_root(&format!("{h}/src/olo"), &home, uid, &[]), Some(Root::Part));
        assert_eq!(writable_root(&format!("{h}/src/new"), &home, uid, &[]), Some(Root::Part), "not there yet");
        assert_eq!(writable_root(&format!("{h}/data/b"), &home, uid, &[]), Some(Root::Part), "its folder not there yet either");
        for refused in [".ssh", ".local", ".local/share/olo", ".local/share/olo/rex-link", "link", "link/x"] {
            assert_eq!(writable_root(&format!("{h}/{refused}"), &home, uid, &[]), None, "{refused}");
        }
        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::remove_dir_all(home.with_extension("elsewhere"));
    }

    /// The streams go up while each step makes the copy faster, hold where
    /// it doesn't, try one more after a while and keep one fewer where
    /// that was slower; nothing is judged while nothing is sent yet.
    #[test]
    fn the_streams_find_the_fastest() {
        let mut st = Streams::new(4, 24);
        let mut sent = 0u64;
        let mut at = |rate: u64, st: &mut Streams| {
            sent += rate * 15;
            st.tick(sent, 15.0)
        };
        assert_eq!(Streams::new(4, 24).tick(0, 15.0), 4, "nothing sent: no judgment");
        // (Faster with each step up: 4, 5, 6, 7, 8, 10…)
        let ups: Vec<usize> = [100, 120, 140, 160, 180].iter().map(|r| at(*r << 20, &mut st)).collect();
        assert_eq!(ups, [5, 6, 7, 8, 10]);
        // (No faster at 10: held there.)
        assert_eq!(at(181 << 20, &mut st), 10);
        assert_eq!(st.state, Climb::Holding { until: st.ticks + 4 });
        for _ in 0..3 {
            assert_eq!(at(181 << 20, &mut st), 10);
        }
        // (Then one more tried; slower with it, so one fewer kept.)
        assert_eq!(at(181 << 20, &mut st), 11);
        assert_eq!(at(170 << 20, &mut st), 10);
        assert!(matches!(st.state, Climb::Holding { .. }));
        // (Never past the most, never under one.)
        let mut one = Streams::new(1, 2);
        let mut b = 0u64;
        for r in [10, 20, 40, 80] {
            b += r * 15;
            assert!(one.tick(b, 15.0) <= 2);
        }
    }

    /// What a later pass takes away: what the passes before sent and this
    /// one hasn't, but nothing on another filesystem here.
    #[test]
    fn a_later_pass_forgets_only_what_is_gone() {
        let dir = PathBuf::from(format!("/var/tmp/olo-rex-link-gone-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let list = |name: &str, paths: &[&str]| std::fs::write(dir.join(name), paths.iter().flat_map(|p| p.bytes().chain([0])).collect::<Vec<u8>>()).unwrap();
        list("before", &["a", "a/x", "b", "mnt/f", "mnt2"]);
        list("now", &["a", "mnt2"]);
        let gone = gone_since(&dir.join("before"), &dir.join("now"), &["mnt".to_owned()]).unwrap();
        assert_eq!(gone, b"a/x\0b\0");
        assert!(gone_since(&dir.join("none"), &dir.join("now"), &[]).is_none(), "no list from before: nothing taken away");
        let _ = std::fs::remove_dir_all(&dir);
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
        let v6 = sending(&args("/var/tmp/x/m1 --to f@[fd00::1]:2200 --host-key k|b --there /home/f").iter().map(|a| a.replace('|', " ")).collect::<Vec<_>>()).unwrap();
        assert_eq!((v6.host.as_str(), v6.port), ("fd00::1", 2200));
        assert_eq!(host_port("fd00::1"), Some(("fd00::1".to_owned(), 22)), "a bare v6 address has no port");
        assert_eq!(host_port("[fd00::1]"), Some(("fd00::1".to_owned(), 22)));
        assert_eq!(host_port("h:x"), None);
        let also = sending(&args("/var/tmp/x/m1 --to f@h --host-key k|b --there /home/f --also /var/tmp/scratch").iter().map(|a| a.replace('|', " ")).collect::<Vec<_>>()).unwrap();
        assert_eq!(also.also, ["/var/tmp/scratch"]);
        assert!(sending(&args("/var/tmp/x/m1 --to f@h --host-key k|b --there /home/f --also /etc").iter().map(|a| a.replace('|', " ")).collect::<Vec<_>>()).is_none(), "only a folder of /var/tmp");
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
}
