//! `olo-rex-link move …`: the helper's part in moving an account from one
//! host to another (Ben, 2026-11-25: "machine to machine copy (don't go
//! via the user's desktop)"), driven by the app (`olo_rex_client::
//! migrate`). The same program on Ubuntu, Fedora or anything else (it is
//! static); file copies run `rsync` over SSH in parallel streams with a
//! per-move ephemeral key and pinned host key.
//!
//! - `move key DIR`: a key of the move's own (ed25519, made here) in the
//!   move's folder on the source, its public line printed.
//! - `move allow ID LINE [--var-tmp]` / `move disallow ID`:
//!   that key let into the destination's account (`~/.ssh/authorized_keys`)
//!   for a day, able to run nothing but this helper's `move receive --id
//!   ID`, writing nothing but the home itself (and `/var/tmp` when
//!   `--var-tmp` is given); and taken out again.
//! - `move receive --id ID`: what that key runs (its forced command): the
//!   one thing asked of it (`SSH_ORIGINAL_COMMAND`), either `hello`,
//!   `forget`, or a validated receiving `rsync --server` (never `--sender`,
//!   never `--delete`, never the destination's own keys, its helper or the
//!   move's folder).
//! - `move keep ID` / `move put-back ID`: the destination's own files kept
//!   aside before the copy (hard links), and put back by Go Back.
//! - `move add-keys ID HEX` / `move take-keys ID`: the source's
//!   `authorized_keys` let in at the destination too, and out again.
//! - `move codex-stop`: Codex's daemon stopped as Codex restarts it.
//! - `move send DIR --to USER@HOST[:PORT] --host-key KEY --there ROOT
//!   [--pass N] [--exclude PATTERN]… [--var-tmp] [--each BYTES] [--detach]`:
//!   the copy, from the home here to ROOT there (and the account's own
//!   entries in `/var/tmp` when `--var-tmp` is set), over parallel `rsync`
//!   streams using an isolated SSH config with the move's key and pinned
//!   host key; detached, it goes on with the app gone (its log, pid and
//!   `status.json` in DIR). A second pass sends what changed and takes away
//!   there what the first put there and is gone here.
//! - `move status DIR`: how the copy in the move's folder stands.
//! - `move look [--size] [--exclude PATTERN]…`: what this host and account
//!   are, for a move (JSON); with `--size`, what the home holds that the
//!   copy brings, and a summary of the account's entries in `/var/tmp`.
//! - `move fix --from OLD --to NEW [--once FILE] [--var-tmp]`: a moved home
//!   made right where its path changed (`crate::fix`).
//! - `move look-pane [--screens HEX] PID…`: each pane's programs and the
//!   agent there with its session (`crate::panes`).
//! - `move stop [--spare PID…]`: every process of the account stopped but
//!   this one's own line of parents and the account's own systemd.

pub(crate) use crate::procs::{Proc, all_procs, codex_home, home, proc_of, unhex};
use std::collections::HashSet;
use std::io::{self, Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use russh::keys::ssh_key::{LineEnding, PrivateKey, private::Ed25519Keypair};

const USAGE: &str = "usage: olo-rex-link move look [--size] [--exclude PATTERN]...\n       olo-rex-link move codex-stop\n       olo-rex-link move halt DIR\n       olo-rex-link move keep ID [--exclude PATTERN]...\n       olo-rex-link move put-back ID\n       olo-rex-link move add-keys ID HEX\n       olo-rex-link move take-keys ID\n       olo-rex-link move reach DIR --to USER@HOST[:PORT] --host-key KEY --there ROOT\n       olo-rex-link move look-pane [--screens HEX] PID...\n       olo-rex-link move key DIR\n       olo-rex-link move send DIR --to USER@HOST[:PORT] --host-key KEY --there ROOT [--pass N] [--exclude PATTERN]... [--var-tmp] [--each BYTES] [--detach]\n       olo-rex-link move allow ID PUBLIC-KEY-LINE [--var-tmp]\n       olo-rex-link move disallow ID\n       olo-rex-link move receive --id ID   (a move's key's forced command)\n       olo-rex-link move status DIR\n       olo-rex-link move stop [--spare PID]…";

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

/// `move allow ID LINE [--var-tmp]`: the move's key let in (again: a
/// later allow puts its day off, the line once); with `--var-tmp` when
/// `/var/tmp` is also allowed for this move, kept in the move's folder
/// here, out of the key's reach.
fn allow(args: &[String]) -> i32 {
    let (Some(id), Some(public)) = (args.first(), args.get(1)) else {
        eprintln!("{USAGE}");
        return 2;
    };
    let mut var_tmp = false;
    for a in &args[2..] {
        if a == "--var-tmp" {
            var_tmp = true;
        } else {
            eprintln!("olo-rex-link move allow: unknown option {a} (expected --var-tmp)");
            return 2;
        }
    }
    if named(id) {
        let dir = home().join(MOVES).join(id);
        let marker = dir.join("var_tmp");
        let saved = std::fs::create_dir_all(&dir).and_then(|()| std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))).and_then(|()| {
            if var_tmp {
                write_whole(&marker, b"1\n", 0o600)
            } else {
                match std::fs::remove_file(&marker) {
                    Ok(()) => Ok(()),
                    Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
                    Err(e) => Err(e),
                }
            }
        });
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

/// What a move's key may ask through `olo-rex-link move do HEX`.
#[derive(Debug, PartialEq, Eq)]
enum Asked {
    /// Can it be reached, by which helper, and is `rsync` present.
    Hello,
    /// The paths named on the input (NUL after each, under `root`) taken
    /// away: the second pass's, of what the first put there and the
    /// source no longer has.
    Forget { root: String },
}

/// The ask in a command's last word (`SSH_ORIGINAL_COMMAND`).
fn asked(command: &str) -> Option<Asked> {
    let mut words = command.split_whitespace();
    if (words.next(), words.next(), words.next()) != (Some("olo-rex-link"), Some("move"), Some("do")) {
        return None;
    }
    let word = words.next()?;
    if words.next().is_some() {
        return None;
    }
    let v: serde_json::Value = serde_json::from_slice(&unhex(word)?).ok()?;
    let root = || v["root"].as_str().map(str::to_owned);
    match v["op"].as_str()? {
        "hello" => Some(Asked::Hello),
        "forget" => Some(Asked::Forget { root: root()? }),
        _ => None,
    }
}

/// Splits a POSIX shell-escaped command (`SSH_ORIGINAL_COMMAND` from rsync
/// with `--no-secluded-args`) into literal arguments, refusing unquoted
/// shell metacharacters, newlines and NULs.
fn split_shell_words(s: &str) -> Option<Vec<String>> {
    if s.bytes().any(|b| b == 0 || b == b'\n' || b == b'\r') {
        return None;
    }
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' => {
                if in_word {
                    words.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            '\\' => {
                let next = chars.next()?;
                cur.push(next);
                in_word = true;
            }
            '\'' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(ch) => cur.push(ch),
                        None => return None,
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => {
                            let next = chars.next()?;
                            cur.push(next);
                        }
                        Some('$' | '`') | None => return None,
                        Some(ch) => cur.push(ch),
                    }
                }
            }
            ';' | '|' | '&' | '$' | '`' | '<' | '>' | '(' | ')' => return None,
            _ => {
                cur.push(c);
                in_word = true;
            }
        }
    }
    if in_word {
        words.push(cur);
    }
    Some(words)
}

/// A validated receiving `rsync --server` command from
/// `SSH_ORIGINAL_COMMAND`: never `--sender` (read-only is refused), never
/// `--delete*`, never options that run programs or read files on the
/// destination, and exactly one target path after `.`. Returns the
/// arguments to pass to `rsync` (`words[1..]`) and the destination path.
fn asked_rsync(command: &str) -> Option<(Vec<String>, String)> {
    let words = split_shell_words(command)?;
    if words.len() < 4 || words[0] != "rsync" || words[1] != "--server" {
        return None;
    }
    let dot = words[2..].iter().position(|w| w == ".")? + 2;
    if dot + 2 != words.len() {
        return None;
    }
    for opt in &words[2..dot] {
        if let Some(long) = opt.strip_prefix("--") {
            let ok = matches!(long, "partial" | "no-implied-dirs" | "ignore-missing-args" | "stats" | "preallocate" | "8-bit-output")
                || long.strip_prefix("log-format=").is_some_and(|v| !v.is_empty() && !v.chars().any(char::is_control))
                || long.strip_prefix("out-format=").is_some_and(|v| !v.is_empty() && !v.chars().any(char::is_control))
                || long.strip_prefix("timeout=").is_some_and(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
                || long.strip_prefix("compress-choice=").is_some_and(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b','))
                || long.strip_prefix("checksum-choice=").is_some_and(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b','))
                || long.strip_prefix("compress-level=").is_some_and(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit() || b == b'-'))
                || long.strip_prefix("skip-compress=").is_some_and(|v| !v.chars().any(char::is_control));
            if !ok {
                return None;
            }
        } else {
            let short = opt.strip_prefix('-')?;
            if short.is_empty() {
                return None;
            }
            let (flags, compat) = match short.split_once("e.") {
                Some((f, c)) => (f, Some(c)),
                None => (short, None),
            };
            if !flags.bytes().all(|b| matches!(b, b'l' | b'H' | b't' | b'p' | b'r' | b'R' | b'x' | b'S' | b'z' | b'8' | b'v' | b'q' | b'u' | b'W' | b'I' | b'd')) {
                return None;
            }
            if let Some(c) = compat
                && (c.is_empty() || !c.bytes().all(|b| b.is_ascii_alphanumeric()))
            {
                return None;
            }
        }
    }
    let dest = words[dot + 1].clone();
    Some((words[1..].to_vec(), dest))
}

/// Where a move's key may write: the home itself (by its real path:
/// `/home` may be a link to `/var/home`); a folder under it that a part
/// of the copy goes to (sent in parallel), never one on the way to or in
/// what the destination keeps ([`KEPT`]: not `.ssh`, not `.local`), each
/// folder on the way a folder and not a link, itself a folder or not
/// there yet; or `/var/tmp` when `allow --var-tmp` permitted it for this
/// move (the sticky bit keeps every other account's files untouched).
/// Nothing else.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Root {
    Home,
    Part,
    VarTmp,
}

fn writable_root(root: &str, home: &Path, var_tmp_ok: bool) -> Option<Root> {
    let trimmed = root.trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    let path = Path::new(trimmed);
    if !path.is_absolute() || path.components().any(|c| !matches!(c, std::path::Component::RootDir | std::path::Component::Normal(_))) {
        return None;
    }
    let canon_home = std::fs::canonicalize(home).ok();
    if let (Ok(real), Some(ch)) = (std::fs::canonicalize(path), &canon_home)
        && real == *ch
    {
        return Some(Root::Home);
    }
    let part_rel = path.strip_prefix(home).ok().map(|r| (home, r)).or_else(|| canon_home.as_deref().and_then(|ch| path.strip_prefix(ch).ok().map(|r| (ch, r))));
    if let Some((base, rel)) = part_rel
        && !rel.as_os_str().is_empty()
        && rel.components().all(|c| matches!(c, std::path::Component::Normal(_)))
        && !kept(rel)
        && no_link_on_the_way(base, rel)
    {
        return match std::fs::symlink_metadata(path) {
            Ok(m) if m.is_dir() => Some(Root::Part),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Some(Root::Part),
            _ => None,
        };
    }
    if trimmed == "/var/tmp" && var_tmp_ok {
        return Some(Root::VarTmp);
    }
    None
}

/// Whether `allow --var-tmp` permitted `/var/tmp` for move `id`.
fn var_tmp_allowed(id: &str) -> bool {
    home().join(MOVES).join(id).join("var_tmp").exists()
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
    let var_tmp_ok = var_tmp_allowed(id);
    if let Some(ask) = asked(&command) {
        return match ask {
            Asked::Hello => {
                println!("{{\"ok\":true,\"helper\":\"{}\",\"rsync\":{}}}", olo_rex_link::VERSION, find_program("rsync").is_some());
                0
            }
            Asked::Forget { root } => match writable_root(&root, &home(), var_tmp_ok) {
                Some(kind) => {
                    let trimmed = root.trim_end_matches('/');
                    forget(Path::new(trimmed), kind, Path::new(trimmed).strip_prefix(home()).ok().map(Path::to_path_buf), uid)
                }
                None => {
                    eprintln!("olo-rex-link move receive: {root} isn't somewhere this move may write");
                    2
                }
            },
        };
    }
    let Some((rsync_args, dest)) = asked_rsync(&command) else {
        eprintln!("olo-rex-link move receive: not something a move's key may ask");
        return 2;
    };
    let Some(kind) = writable_root(&dest, &home(), var_tmp_ok) else {
        eprintln!("olo-rex-link move receive: {dest} isn't somewhere this move may write");
        return 2;
    };
    if kind == Root::Part
        && let Err(e) = std::fs::create_dir_all(dest.trim_end_matches('/'))
    {
        eprintln!("olo-rex-link move receive: {dest}: {e}");
        return 1;
    }
    let Some(rsync) = find_program("rsync") else {
        eprintln!("olo-rex-link move receive: rsync isn't installed on this host");
        return 127;
    };
    use std::os::unix::process::CommandExt;
    let err = std::process::Command::new(rsync).args(&rsync_args).exec();
    eprintln!("olo-rex-link move receive: rsync: {err}");
    127
}

/// `forget`: each path on the input (NUL after each, under `root`) taken
/// away, a folder only if it is empty; deepest first. Never one the
/// destination keeps ([`KEPT`]), never through a link (each folder on the
/// way a folder, not a link to one elsewhere), for `/var/tmp` only entries
/// whose top-level entry is owned by `uid`, and in a folder that is
/// read-only (Go's module cache: 0555) by opening it to the account a
/// moment, its mode and time put back.
fn forget(root: &Path, kind: Root, under_home: Option<PathBuf>, uid: u32) -> i32 {
    use std::os::unix::fs::MetadataExt;
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
        if kind == Root::VarTmp {
            let Some(first) = rel.components().next() else {
                left += 1;
                continue;
            };
            if !std::fs::symlink_metadata(root.join(first)).is_ok_and(|m| m.uid() == uid) {
                left += 1;
                continue;
            }
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
    /// Also copy this account's entries in `/var/tmp` to `/var/tmp` there.
    var_tmp: bool,
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
            "--var-tmp" => s.var_tmp = true,
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

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The command a move's key sends for `ask` (its forced command reads it).
fn command_for(ask: &serde_json::Value) -> String {
    format!("olo-rex-link move do {}", hex(ask.to_string().as_bytes()))
}

/// Writes `known_hosts` and an isolated `ssh_config` in `s.dir` so every
/// `ssh` and `rsync` stream uses only the move's key and pinned host key,
/// ignoring `~/.ssh/config` and `~/.ssh/known_hosts`.
fn write_ssh_files(s: &Sending) -> Result<(), String> {
    std::fs::create_dir_all(&s.dir).and_then(|()| std::fs::set_permissions(&s.dir, std::fs::Permissions::from_mode(0o700))).map_err(|e| format!("{}: {e}", s.dir.display()))?;
    let abs = std::fs::canonicalize(&s.dir).unwrap_or_else(|_| s.dir.clone());
    let abs_str = abs.to_str().ok_or_else(|| "the move's folder path isn't UTF-8".to_owned())?;
    for (what, val) in [("host", s.host.as_str()), ("user", s.user.as_str()), ("host key", s.host_key.as_str()), ("move folder", abs_str)] {
        if val.contains(['"', '\n', '\r']) || val.chars().any(char::is_control) {
            return Err(format!("invalid character in {what}"));
        }
    }
    if s.host.chars().any(char::is_whitespace) || s.user.chars().any(char::is_whitespace) {
        return Err("whitespace in host or user".into());
    }
    let known = format!("olo-move-dest {}\n", s.host_key);
    write_whole(&abs.join("known_hosts"), known.as_bytes(), 0o600).map_err(|e| format!("known_hosts: {e}"))?;
    // (OpenSSH expands `%` tokens in IdentityFile / UserKnownHostsFile.)
    let esc_dir = abs_str.replace('%', "%%");
    let config = format!(
        "Host dest\n\
         \tHostName {host}\n\
         \tPort {port}\n\
         \tUser {user}\n\
         \tHostKeyAlias olo-move-dest\n\
         \tIdentityFile \"{esc_dir}/key\"\n\
         \tIdentitiesOnly yes\n\
         \tUserKnownHostsFile \"{esc_dir}/known_hosts\"\n\
         \tGlobalKnownHostsFile /dev/null\n\
         \tStrictHostKeyChecking yes\n\
         \tUpdateHostKeys no\n\
         \tBatchMode yes\n\
         \tCompression no\n\
         \tServerAliveInterval 15\n\
         \tServerAliveCountMax 4\n\
         \tConnectTimeout 30\n\
         \tClearAllForwardings yes\n\
         \tControlMaster no\n",
        host = s.host,
        port = s.port,
        user = s.user,
    );
    write_whole(&abs.join("ssh_config"), config.as_bytes(), 0o600).map_err(|e| format!("ssh_config: {e}"))
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
    // with it. Half a second's grace: `status` takes the lock a moment.)
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
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(2, 8);
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

/// A short name for a root, for its files in the move's folder.
fn key_of(kind: &str, root: &str) -> String {
    let id = olo_rex_sync::chunk::chunk_id(root.as_bytes());
    format!("{kind}{}", hex(&id[..8]))
}

/// `move reach DIR --to … --host-key …`: whether the destination can be
/// reached from here with the move's key over `ssh` (the exact path
/// `move send` uses), and `rsync` is installed on both hosts.
fn reach(args: &[String]) -> i32 {
    let Some(s) = sending(args) else {
        eprintln!("{USAGE}");
        return 2;
    };
    let said = (|| -> Result<serde_json::Value, String> {
        let ssh = find_program("ssh").ok_or_else(|| "ssh isn't installed on this host".to_owned())?;
        if find_program("rsync").is_none() {
            return Err("rsync isn't installed on this host".into());
        }
        write_ssh_files(&s)?;
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
        rt.block_on(async {
            let mut cmd = tokio::process::Command::new(&ssh);
            cmd.current_dir(&s.dir)
                .args(["-F", "ssh_config", "dest", &command_for(&serde_json::json!({"op": "hello"}))])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(true);
            let out = tokio::time::timeout(Duration::from_secs(25), cmd.output()).await.map_err(|_| format!("{}:{} didn't answer in 25 seconds", s.host, s.port))?.map_err(|e| format!("ssh: {e}"))?;
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            if !out.status.success() {
                let why = stderr.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("ssh failed").trim();
                return Err(why.to_owned());
            }
            let v: serde_json::Value = stdout.lines().rev().find_map(|l| serde_json::from_str(l.trim()).ok()).unwrap_or_default();
            if v["ok"].as_bool() != Some(true) {
                return Err(format!("the destination's helper didn't answer ({})", stdout.trim()));
            }
            if v["rsync"].as_bool() == Some(false) {
                return Err("rsync isn't installed on the new host".into());
            }
            Ok(v)
        })
    })();
    match said {
        Ok(v) => {
            println!("{}", serde_json::json!({"ok": true, "helper": v["helper"], "rsync": true}));
            0
        }
        Err(e) => {
            println!("{}", serde_json::json!({"ok": false, "error": e}));
            1
        }
    }
}

/// What a pass sends in an `rsync` stream of its own: the home's own (all
/// of it but its split parts), a split folder of the home, or a balanced
/// bucket of the account's entries in `/var/tmp`.
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
    /// The move's exclusions under it, anchored at it (for Home / Folder).
    exclude: Vec<String>,
    /// For `/var/tmp` buckets: NUL-separated top-level entry names (`--files-from`).
    files_from: Option<PathBuf>,
    /// Its files' name in the move's folder.
    key: String,
    /// What the split measured of it (logical file bytes, matching rsync progress).
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

/// Escapes wildcard characters in a literal folder path for rsync's
/// `--exclude-from` so split folder names containing `[` or `*` match
/// literally.
fn rsync_escape_pattern(rel: &str) -> String {
    let mut out = String::with_capacity(rel.len() + 1);
    out.push('/');
    for c in rel.chars() {
        if matches!(c, '\\' | '*' | '?' | '[' | ']') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// What [`measure_tree`] found: each folder's files and bytes (of all
/// under it), each folder's folders, which folders have only owner-writable
/// non-symlink ancestors, and the whole's.
struct Tree {
    sizes: std::collections::HashMap<String, (u64, u64)>,
    children: std::collections::HashMap<String, Vec<String>>,
    writable: HashSet<String>,
    files: u64,
    bytes: u64,
}

/// The home walked once, as the copy brings it (`filter`, one filesystem):
/// what each folder holds (logical bytes `meta.len()`, matching rsync's
/// progress counter).
fn measure_tree(home: &Path, filter: &olo_rex_sync::index::Filter) -> Tree {
    use std::os::unix::fs::MetadataExt;
    let dev = std::fs::symlink_metadata(home).map(|m| m.dev()).unwrap_or(0);
    let mut sizes: std::collections::HashMap<String, (u64, u64)> = std::collections::HashMap::new();
    let mut children: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
    let mut writable = HashSet::new();
    writable.insert(String::new());
    let (mut files, mut bytes) = (0u64, 0u64);
    let mut stack = vec![(String::new(), true)];
    while let Some((rel, parent_writable)) = stack.pop() {
        let dir = if rel.is_empty() { home.to_path_buf() } else { home.join(&rel) };
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.filter_map(|e| e.ok()) {
            let Some(name) = e.file_name().to_str().map(str::to_owned) else { continue };
            let child = if rel.is_empty() { name } else { format!("{rel}/{name}") };
            let Ok(meta) = std::fs::symlink_metadata(e.path()) else { continue };
            if meta.is_dir() {
                if meta.dev() == dev && filter.allow_dir(&child) {
                    let is_w = parent_writable && (meta.permissions().mode() & 0o200 != 0);
                    if is_w {
                        writable.insert(child.clone());
                    }
                    children.entry(rel.clone()).or_default().push(child.clone());
                    stack.push((child, is_w));
                }
                continue;
            }
            if !filter.matches_entry(&child) {
                continue;
            }
            let size = if meta.is_file() { meta.len() } else { 0 };
            files += 1;
            bytes += size;
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
    Tree { sizes, children, writable, files, bytes }
}

/// The home split for sending in parallel: folders of it (by their paths
/// under it), each an `rsync` stream of its own, of about `each` bytes or
/// more; the biggest split into their folders until none is much bigger;
/// never a folder on the way to what the destination keeps (`.ssh`,
/// `.local`), nor one under a read-only (`0555`) parent that a parallel
/// stream couldn't `mkdir` under. Biggest first, at most `most`.
fn split(tree: &Tree, each: u64, most: usize) -> Vec<(String, u64, u64)> {
    let (sizes, children, writable) = (&tree.sizes, &tree.children, &tree.writable);
    let may_be = |rel: &str| !kept(Path::new(rel)) && writable.contains(rel);
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

/// Scans `root` (ordinarily `/var/tmp`) for top-level entries owned by `uid`
/// and packs them into 1..=8 balanced buckets (greedy largest-first), each
/// written as a NUL-separated `--files-from` list in `dir`. Zero entry names
/// ever appear on a command line.
fn var_tmp_buckets_in(root: &Path, uid: u32, dir: &Path, pass: u32, each: u64) -> io::Result<Vec<Part>> {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;
    let Ok(rd) = std::fs::read_dir(root) else { return Ok(Vec::new()) };
    let root_dev = std::fs::symlink_metadata(root).map(|m| m.dev()).unwrap_or(0);
    let mut items: Vec<(Vec<u8>, u64, u64)> = Vec::new();
    let until = Instant::now() + Duration::from_secs(30);
    for e in rd.filter_map(|e| e.ok()) {
        let name = e.file_name();
        let raw = name.as_bytes();
        if raw.is_empty() || raw == b"." || raw == b".." || raw.contains(&0) || raw.contains(&b'/') {
            continue;
        }
        let Ok(meta) = std::fs::symlink_metadata(e.path()) else { continue };
        if meta.uid() != uid {
            continue;
        }
        let (mut files, mut bytes) = (0u64, 0u64);
        if meta.is_dir() {
            let mut stack = vec![e.path()];
            while let Some(d) = stack.pop() {
                if Instant::now() > until {
                    break;
                }
                let Ok(sub) = std::fs::read_dir(&d) else { continue };
                for f in sub.filter_map(|f| f.ok()) {
                    let Ok(m) = std::fs::symlink_metadata(f.path()) else { continue };
                    if m.is_dir() {
                        if m.dev() == root_dev {
                            stack.push(f.path());
                        }
                    } else {
                        files += 1;
                        if m.is_file() {
                            bytes += m.len();
                        }
                    }
                }
            }
        } else {
            files = 1;
            if meta.is_file() {
                bytes = meta.len();
            }
        }
        items.push((raw.to_vec(), files, bytes));
    }
    if items.is_empty() {
        return Ok(Vec::new());
    }
    // (Largest first so greedy bin-packing balances byte size across buckets.)
    items.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.cmp(&b.0)));
    let total_bytes: u64 = items.iter().map(|x| x.2).sum();
    let target_buckets = if each == 0 || total_bytes < each { 1 } else { ((total_bytes / each) as usize).clamp(2, 8) }.min(items.len());
    let mut buckets: Vec<(Vec<u8>, u64, u64)> = (0..target_buckets).map(|_| (Vec::new(), 0, 0)).collect();
    for (name, f, b) in items {
        let idx = buckets.iter().enumerate().min_by_key(|(_, x)| (x.2, x.1)).map_or(0, |(i, _)| i);
        buckets[idx].0.extend_from_slice(&name);
        buckets[idx].0.push(0);
        buckets[idx].1 += f;
        buckets[idx].2 += b;
    }
    let mut parts = Vec::new();
    for (i, (list_bytes, files, bytes)) in buckets.into_iter().enumerate() {
        if list_bytes.is_empty() {
            continue;
        }
        let list_path = dir.join(format!("vartmp-{pass}-{i}.list"));
        write_whole(&list_path, &list_bytes, 0o600)?;
        parts.push(Part {
            kind: PartKind::VarTmp,
            here: root.to_path_buf(),
            there: root.display().to_string(),
            exclude: Vec::new(),
            files_from: Some(list_path),
            key: format!("vt{i}"),
            files: if pass == 1 { files } else { 0 },
            bytes: if pass == 1 { bytes } else { 0 },
        });
    }
    Ok(parts)
}

/// How many sessions a pass runs at once: as many as make it go fastest,
/// found as it goes. More while each step up makes it faster; held where
/// it stops; tried one more now and then, and one fewer kept where that
/// is slower. (A stream is never cut: fewer means none started until the
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
/// the same), the home's own session, and (when `--var-tmp`) 1..=8 balanced
/// buckets of this account's entries in `/var/tmp`.
fn parts_of(s: &Sending, sessions_most: usize) -> io::Result<Vec<Part>> {
    let file = s.dir.join("parts.json");
    let home = home();
    let mut base_exclude: Vec<String> = KEPT.iter().map(|k| format!("/{k}")).collect();
    for e in &s.exclude {
        if !base_exclude.contains(e) {
            base_exclude.push(e.clone());
        }
    }
    type SavedSplit = (Vec<(String, u64, u64)>, u64, u64);
    let saved: Option<SavedSplit> = std::fs::read(&file).ok().and_then(|b| {
        serde_json::from_slice::<SavedSplit>(&b).ok().or_else(|| {
            // (Older format: just the split vec.)
            serde_json::from_slice::<Vec<(String, u64, u64)>>(&b).ok().map(|v| (v, 0, 0))
        })
    });
    let (split_parts, total_files, total_bytes, each_bytes) = match saved {
        Some((parts, tf, tb)) => {
            let each = s.each.unwrap_or_else(|| (tb / (3 * sessions_most as u64).max(1)).max(1 << 30));
            (parts, tf, tb, each)
        }
        None => {
            let filter = olo_rex_sync::index::Filter { exclude: base_exclude.clone(), ..olo_rex_sync::index::Filter::default() };
            let tree = measure_tree(&home, &filter);
            let each = s.each.unwrap_or_else(|| (tree.bytes / (3 * sessions_most as u64).max(1)).max(1 << 30));
            let parts = split(&tree, each, 64);
            let payload = (&parts, tree.files, tree.bytes);
            write_whole(&file, serde_json::to_string(&payload).unwrap_or_default().as_bytes(), 0o600)?;
            (parts, tree.files, tree.bytes, each)
        }
    };
    let here = |rel: &str| home.join(rel);
    let folders: Vec<&(String, u64, u64)> = split_parts.iter().filter(|(rel, _, _)| std::fs::symlink_metadata(here(rel)).is_ok_and(|m| m.is_dir())).collect();
    let mut home_exclude = base_exclude.clone();
    home_exclude.extend(folders.iter().map(|(rel, _, _)| rsync_escape_pattern(rel)));
    let (in_parts_files, in_parts_bytes) = folders.iter().fold((0u64, 0u64), |(f, b), p| (f + p.1, b + p.2));
    let (home_files, home_bytes) = if s.pass == 1 && total_bytes > 0 { (total_files.saturating_sub(in_parts_files), total_bytes.saturating_sub(in_parts_bytes)) } else { (0, 0) };
    let mut parts = vec![Part { kind: PartKind::Home, here: home.clone(), there: s.there.clone(), exclude: home_exclude, files_from: None, key: "home".into(), files: home_files, bytes: home_bytes }];
    for (rel, files, bytes) in &split_parts {
        let gone = !folders.iter().any(|p| &p.0 == rel);
        parts.push(Part {
            kind: PartKind::Folder,
            here: here(rel),
            there: format!("{}/{rel}", s.there.trim_end_matches('/')),
            exclude: excludes_under(&base_exclude, rel),
            files_from: None,
            key: key_of("p", rel),
            files: if gone || s.pass > 1 { 0 } else { *files },
            bytes: if gone || s.pass > 1 { 0 } else { *bytes },
        });
    }
    if s.var_tmp {
        // SAFETY: a getter.
        let uid = unsafe { libc::getuid() };
        parts.extend(var_tmp_buckets_in(Path::new("/var/tmp"), uid, &s.dir, s.pass, each_bytes)?);
    }
    Ok(parts)
}

/// Decodes rsync's `\#OOO` octal escapes (emitted with `--8-bit-output` for
/// control characters) back to raw path bytes.
fn unescape_rsync(s: &str) -> Vec<u8> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if i + 4 < bytes.len() && bytes[i] == b'\\' && bytes[i + 1] == b'#' && bytes[i + 2..i + 5].iter().all(|b| (b'0'..=b'7').contains(b)) {
            let v = ((bytes[i + 2] - b'0') as u16) * 64 + ((bytes[i + 3] - b'0') as u16) * 8 + ((bytes[i + 4] - b'0') as u16);
            if v <= 255 {
                out.push(v as u8);
                i += 5;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

/// Parses an `rsync --info=progress2` status line (`120,000 100% 83.19MB/s
/// 0:00:00 (xfr#2, to-chk=0/4)`), returning `(bytes_transferred, xfr_count)`.
fn parse_progress2(line: &str) -> Option<(u64, Option<u64>)> {
    if !line.contains('%') || !(line.contains("/s") || line.contains("xfr#")) {
        return None;
    }
    let mut words = line.split_whitespace();
    let first = words.next()?;
    let second = words.next()?;
    if !second.ends_with('%') {
        return None;
    }
    let digits: String = first.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    let bytes = digits.parse::<u64>().ok()?;
    let xfr = line.split_once("xfr#").and_then(|(_, rest)| {
        let num: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        num.parse::<u64>().ok()
    });
    Some((bytes, xfr))
}

/// What one `rsync` part produced.
#[derive(Default)]
struct PartResult {
    copied: u64,
    wire: u64,
    gone: u64,
    /// Skipped on a Pass 1 retry because `done-1-{key}` already existed.
    skipped_done: bool,
    warnings: Vec<String>,
}

/// Whether `stderr` from an exit-23 `rsync` shows a fatal destination write
/// error (disk full, read-only fs, quota, refusal to write) rather than a
/// live-source vanished/unreadable file.
fn fatal_dest_error(stderr: &str) -> bool {
    let low = stderr.to_ascii_lowercase();
    for needle in [
        "no space left on device",
        "read-only file system",
        "disk quota exceeded",
        "input/output error",
        "mkstemp",
        "isn't somewhere this move may write",
        "not something a move's key may ask",
        "host key verification failed",
        "permission denied (publickey",
    ] {
        if low.contains(needle) {
            return true;
        }
    }
    false
}

/// A pass: its parts sent in parallel over `rsync`, with adaptive stream
/// concurrency ([`Streams`]), staggered starts (never hammering sshd's
/// `MaxStartups`), and live rolling throughput/ETA. Pass 1 resumes by
/// skipping finished parts and using `--partial`; Pass 2 syncs every part
/// and forgets paths sent in Pass 1 that no longer exist on the source.
async fn copy(s: std::sync::Arc<Sending>, standing: Shared) -> Result<(), String> {
    let ssh = find_program("ssh").ok_or_else(|| "ssh isn't installed on this host".to_owned())?;
    let rsync = find_program("rsync").ok_or_else(|| "rsync isn't installed on this host".to_owned())?;
    write_ssh_files(&s)?;
    let most = std::thread::available_parallelism().map_or(8, |n| n.get()).clamp(2, 12);
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
    let mut whole = serde_json::json!({"copied": 0, "failed": 0, "failures": [], "wire": 0, "gone": 0});
    let mut waiting: std::collections::VecDeque<usize> = (0..parts.len()).collect();
    let mut running = tokio::task::JoinSet::new();
    let mut streams = Streams::new(4, most);
    let mut failed: Option<String> = None;
    let climb_every = Duration::from_secs(15);
    let rate_every = Duration::from_secs(2);
    let mut climb_tick = tokio::time::interval_at(tokio::time::Instant::now() + climb_every, climb_every);
    let mut rate_tick = tokio::time::interval_at(tokio::time::Instant::now() + rate_every, rate_every);
    let mut samples: std::collections::VecDeque<(Instant, u64)> = std::collections::VecDeque::from([(Instant::now(), 0)]);
    let mut skipped_bytes = 0u64;
    let mut last_spawn = Instant::now() - Duration::from_secs(1);
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();
    let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt()).ok();
    loop {
        while failed.is_none()
            && running.len() < streams.level
            && let Some(k) = waiting.pop_front()
        {
            // (Stagger new SSH handshakes by 150 ms so sshd never sees a
            // MaxStartups spike.)
            let since = last_spawn.elapsed();
            if since < Duration::from_millis(150) {
                tokio::time::sleep(Duration::from_millis(150) - since).await;
            }
            last_spawn = Instant::now();
            running.spawn(send_part(s.clone(), ssh.clone(), rsync.clone(), parts[k].clone(), k, standing.clone()));
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
                Ok((k, Ok(res))) => {
                    whole["copied"] = (whole["copied"].as_u64().unwrap_or(0) + res.copied).into();
                    whole["wire"] = (whole["wire"].as_u64().unwrap_or(0) + res.wire).into();
                    whole["gone"] = (whole["gone"].as_u64().unwrap_or(0) + res.gone).into();
                    if !res.warnings.is_empty()
                        && let Some(list) = whole["failures"].as_array_mut()
                    {
                        for w in res.warnings.iter().take(200usize.saturating_sub(list.len())) {
                            list.push(serde_json::json!({"path": parts[k].there, "why": w}));
                        }
                    }
                    let mut st = standing.lock().unwrap_or_else(|e| e.into_inner());
                    st.sent += res.wire;
                    let mine = &mut st.parts[k];
                    mine.phase = "done";
                    if s.pass == 1 && mine.measured_bytes > 0 {
                        if res.skipped_done {
                            skipped_bytes = skipped_bytes.saturating_add(mine.measured_bytes);
                        }
                        mine.bytes = mine.bytes.max(mine.measured_bytes);
                        mine.bytes_total = mine.bytes;
                        mine.files = mine.files.max(mine.measured_files);
                        mine.files_total = mine.files;
                        mine.planned = true;
                    }
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
            _ = rate_tick.tick() => {
                let now = Instant::now();
                let bytes = standing.lock().unwrap_or_else(|e| e.into_inner()).totals().1.saturating_sub(skipped_bytes);
                samples.push_back((now, bytes));
                while samples.len() > 2 && samples.front().is_some_and(|(t, _)| now.duration_since(*t) > Duration::from_secs(12)) {
                    samples.pop_front();
                }
                if let Some((t0, b0)) = samples.front() {
                    let dt = now.duration_since(*t0).as_secs_f64();
                    if dt >= 0.5 && bytes >= *b0 {
                        standing.lock().unwrap_or_else(|e| e.into_inner()).rate = ((bytes - *b0) as f64 / dt) as u64;
                    }
                }
            }
            _ = climb_tick.tick() => {
                let bytes = standing.lock().unwrap_or_else(|e| e.into_inner()).totals().1.saturating_sub(skipped_bytes);
                streams.tick(bytes, climb_every.as_secs_f64());
            }
            _ = async { if let Some(sig) = &mut sigterm { sig.recv().await } else { std::future::pending().await } } => {
                running.abort_all();
                return Err("the copy was stopped".into());
            }
            _ = async { if let Some(sig) = &mut sigint { sig.recv().await } else { std::future::pending().await } } => {
                running.abort_all();
                return Err("the copy was stopped".into());
            }
        }
        standing.lock().unwrap_or_else(|e| e.into_inner()).report = Some(whole.clone());
    }
    // Pass 2: remove `/var/tmp` files sent in Pass 1 that no longer exist on
    // the source (checked across all `/var/tmp` buckets at once).
    if failed.is_none() && s.pass > 1 && s.var_tmp {
        let gone = vanished_in_sent_prefix(&s.dir, "sent-vt", Path::new("/var/tmp"));
        if !gone.is_empty()
            && let Ok(n) = forget_there(&s, &ssh, "/var/tmp", &gone).await
        {
            whole["gone"] = (whole["gone"].as_u64().unwrap_or(0) + n).into();
            standing.lock().unwrap_or_else(|e| e.into_inner()).report = Some(whole);
        }
    }
    match failed {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// Collects NUL-separated paths from `file` (deduplicated) that no longer
/// exist under `root` (`NotFound`), or all of them when `root` itself is gone.
fn vanished_in_file(file: &Path, root: &Path, root_exists: bool) -> Vec<u8> {
    let Ok(raw) = std::fs::read(file) else { return Vec::new() };
    let mut seen: HashSet<&[u8]> = HashSet::new();
    let mut out = Vec::new();
    for p in raw.split(|&b| b == 0).filter(|p| !p.is_empty()) {
        if !seen.insert(p) {
            continue;
        }
        let gone = !root_exists || matches!(std::fs::symlink_metadata(root.join(std::ffi::OsStr::from_bytes_compat(p))), Err(e) if e.kind() == io::ErrorKind::NotFound);
        if gone {
            out.extend_from_slice(p);
            out.push(0);
        }
    }
    out
}

/// Like [`vanished_in_file`], merged across every file in `dir` whose name
/// starts with `prefix` (all `/var/tmp` buckets from Pass 1).
fn vanished_in_sent_prefix(dir: &Path, prefix: &str, root: &Path) -> Vec<u8> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut seen: HashSet<Vec<u8>> = HashSet::new();
    let mut out = Vec::new();
    for e in rd.filter_map(|e| e.ok()) {
        if !e.file_name().to_str().is_some_and(|n| n.starts_with(prefix)) {
            continue;
        }
        let Ok(raw) = std::fs::read(e.path()) else { continue };
        for p in raw.split(|&b| b == 0).filter(|p| !p.is_empty()) {
            if !seen.insert(p.to_vec()) {
                continue;
            }
            if matches!(std::fs::symlink_metadata(root.join(std::ffi::OsStr::from_bytes_compat(p))), Err(e) if e.kind() == io::ErrorKind::NotFound) {
                out.extend_from_slice(p);
                out.push(0);
            }
        }
    }
    out
}

/// Appends NUL-separated paths recorded by Pass 1 to `path` (`0600`).
fn append_sent_paths(path: &Path, buf: &[u8]) {
    if buf.is_empty() {
        return;
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).mode(0o600).open(path) {
        let _ = f.write_all(buf);
    }
}

/// One part sent by `rsync` over SSH (using `DIR/ssh_config`), with retries
/// on transient SSH/network drops, live progress parsing, Pass 1 resume,
/// and Pass 2 deletion of files vanished since Pass 1.
async fn send_part(s: std::sync::Arc<Sending>, ssh: PathBuf, rsync: PathBuf, part: Part, k: usize, standing: Shared) -> (usize, Result<PartResult, String>) {
    let ended = async {
        let done_marker = s.dir.join(format!("done-1-{}", part.key));
        if s.pass == 1 && part.kind != PartKind::VarTmp && done_marker.exists() {
            return Ok(PartResult { skipped_done: true, ..PartResult::default() });
        }
        let here_is = std::fs::symlink_metadata(&part.here).is_ok_and(|m| m.is_dir());
        let mut res = PartResult::default();
        if here_is {
            let exclude_file = if !part.exclude.is_empty() {
                let p = s.dir.join(format!("exclude-{}", part.key));
                let body: String = part.exclude.iter().flat_map(|e| [e.as_str(), "\n"]).collect();
                write_whole(&p, body.as_bytes(), 0o600).map_err(|e| format!("exclude file: {e}"))?;
                Some(p)
            } else {
                None
            };
            let sent_file = s.dir.join(format!("sent-{}", part.key));
            let rsh = format!("{} -F ssh_config", ssh.display());
            let src_arg = format!("{}/", part.here.display().to_string().trim_end_matches('/'));
            let dst_arg = format!("dest:{}/", part.there.trim_end_matches('/'));
            let mut attempt = 0u32;
            let backoffs = [2u64, 5, 10];
            loop {
                attempt += 1;
                standing.lock().unwrap_or_else(|e| e.into_inner()).parts[k].phase = "sending";
                let mut cmd = tokio::process::Command::new(&rsync);
                cmd.current_dir(&s.dir).env("LC_ALL", "C").args([
                    "-rlptHSxz",
                    "--partial",
                    "--8-bit-output",
                    "--no-secluded-args",
                    "--info=progress2",
                    "--stats",
                    "--out-format=OLO_ITEM\t%i\t%n",
                    "--timeout=120",
                    "-e",
                    &rsh,
                ]);
                if let Some(ex) = &exclude_file {
                    cmd.arg("--exclude-from").arg(ex);
                }
                if let Some(ff) = &part.files_from {
                    cmd.arg("--from0").arg("--files-from").arg(ff).arg("--no-implied-dirs").arg("--ignore-missing-args");
                }
                cmd.arg(&src_arg).arg(&dst_arg).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).kill_on_drop(true);
                let mut child = cmd.spawn().map_err(|e| format!("rsync: {e}"))?;
                let mut stdout = child.stdout.take().ok_or_else(|| "rsync stdout missing".to_owned())?;
                let mut stderr = child.stderr.take().ok_or_else(|| "rsync stderr missing".to_owned())?;
                let stderr_task = tokio::spawn(async move {
                    use tokio::io::AsyncReadExt;
                    let mut out = Vec::new();
                    let mut chunk = [0u8; 4096];
                    loop {
                        match stderr.read(&mut chunk).await {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                if out.len() < 256 * 1024 {
                                    let take = n.min(256 * 1024 - out.len());
                                    out.extend_from_slice(&chunk[..take]);
                                }
                            }
                        }
                    }
                    String::from_utf8_lossy(&out).into_owned()
                });
                let mut buf = [0u8; 8192];
                let mut pending: Vec<u8> = Vec::new();
                let mut sent_buf: Vec<u8> = Vec::new();
                let mut wire = 0u64;
                let mut copied = 0u64;
                let base_bytes = standing.lock().unwrap_or_else(|e| e.into_inner()).parts[k].bytes;
                let base_files = standing.lock().unwrap_or_else(|e| e.into_inner()).parts[k].files;
                use tokio::io::AsyncReadExt;
                loop {
                    let n = stdout.read(&mut buf).await.map_err(|e| format!("rsync output: {e}"))?;
                    if n == 0 {
                        break;
                    }
                    pending.extend_from_slice(&buf[..n]);
                    let mut start = 0;
                    for i in 0..pending.len() {
                        if pending[i] == b'\n' || pending[i] == b'\r' {
                            let line = String::from_utf8_lossy(&pending[start..i]);
                            start = i + 1;
                            if let Some(rest) = line.strip_prefix("OLO_ITEM\t") {
                                if let Some((item, raw_path)) = rest.split_once('\t') {
                                    let rel = raw_path.trim_end_matches('/');
                                    if !rel.is_empty() {
                                        if s.pass == 1 && item.starts_with(['<', '>', 'c', 'h']) {
                                            sent_buf.extend_from_slice(&unescape_rsync(rel));
                                            sent_buf.push(0);
                                            if sent_buf.len() >= 65536 {
                                                append_sent_paths(&sent_file, &sent_buf);
                                                sent_buf.clear();
                                            }
                                        }
                                        let mut st = standing.lock().unwrap_or_else(|e| e.into_inner());
                                        rel.clone_into(&mut st.current);
                                    }
                                }
                            } else if let Some(rest) = line.strip_prefix("Total bytes sent: ") {
                                let d: String = rest.chars().filter(|c| c.is_ascii_digit()).collect();
                                if let Ok(v) = d.parse::<u64>() {
                                    wire = v;
                                }
                            } else if let Some(rest) = line.strip_prefix("Number of regular files transferred: ") {
                                let d: String = rest.chars().filter(|c| c.is_ascii_digit()).collect();
                                if let Ok(v) = d.parse::<u64>() {
                                    copied = v;
                                }
                            } else if let Some((bytes, xfr)) = parse_progress2(&line) {
                                let mut st = standing.lock().unwrap_or_else(|e| e.into_inner());
                                let mine = &mut st.parts[k];
                                let total_b = base_bytes.saturating_add(bytes);
                                mine.bytes = mine.bytes.max(total_b);
                                if let Some(x) = xfr {
                                    mine.files = mine.files.max(base_files.saturating_add(x));
                                }
                                if s.pass == 1 {
                                    mine.bytes_total = mine.measured_bytes.max(mine.bytes);
                                    mine.files_total = mine.measured_files.max(mine.files);
                                } else {
                                    mine.bytes_total = mine.bytes;
                                    mine.files_total = mine.files;
                                }
                                mine.planned = true;
                            }
                        }
                    }
                    if start > 0 {
                        pending.drain(..start);
                    }
                }
                if !sent_buf.is_empty() {
                    append_sent_paths(&sent_file, &sent_buf);
                }
                let status = child.wait().await.map_err(|e| format!("rsync wait: {e}"))?;
                let err_text = stderr_task.await.unwrap_or_default();
                let code = status.code().unwrap_or(-1);
                res.wire += wire;
                res.copied += copied;
                match code {
                    0 => break,
                    24 => {
                        // (Vanished source files: normal while the source is still running.)
                        if !err_text.trim().is_empty() {
                            res.warnings.push("some files vanished during copy".into());
                        }
                        break;
                    }
                    23 if !fatal_dest_error(&err_text) => {
                        // (Live source: unreadable or mid-change source file; fatal destination errors still abort above.)
                        let msg = err_text.lines().find(|l| !l.trim().is_empty()).unwrap_or("some files could not be read").trim();
                        res.warnings.push(msg.to_owned());
                        break;
                    }
                    10 | 12 | 30 | 35 | 255 if (attempt as usize) <= backoffs.len() => {
                        let wait = backoffs[(attempt - 1) as usize];
                        eprintln!("olo-rex-link move send: {} attempt {attempt} failed (exit {code}), retrying in {wait}s: {}", part.key, err_text.lines().last().unwrap_or(""));
                        tokio::time::sleep(Duration::from_secs(wait)).await;
                        continue;
                    }
                    _ => {
                        let why = err_text
                            .lines()
                            .rev()
                            .find(|l| {
                                let t = l.trim();
                                !t.is_empty() && !t.starts_with("rsync error:")
                            })
                            .or_else(|| err_text.lines().rev().find(|l| !l.trim().is_empty()))
                            .map(str::trim)
                            .unwrap_or("rsync failed");
                        return Err(format!("{why} (rsync exit {code})"));
                    }
                }
            }
            if s.pass == 1 && part.kind != PartKind::VarTmp {
                let _ = write_whole(&done_marker, b"1\n", 0o600);
            }
        }
        // Pass 2 for Home / Folder parts: remove files Pass 1 put there that
        // no longer exist on the source (never destination's pre-existing files).
        if s.pass > 1 && part.kind != PartKind::VarTmp {
            let sent_file = s.dir.join(format!("sent-{}", part.key));
            let gone = vanished_in_file(&sent_file, &part.here, here_is);
            if !gone.is_empty() {
                res.gone += forget_there(&s, &ssh, &part.there, &gone).await?;
            }
            if !here_is
                && part.kind == PartKind::Folder
                && let Some(rel) = part.there.strip_prefix(&format!("{}/", s.there.trim_end_matches('/')))
            {
                let mut list = rel.as_bytes().to_vec();
                list.push(0);
                res.gone += forget_there(&s, &ssh, &s.there, &list).await?;
            }
        }
        Ok(res)
    }
    .await;
    (k, ended)
}

/// The paths of `list` (NUL after each) taken away under `root` there over
/// `ssh -F ssh_config dest`: how many went.
async fn forget_there(s: &Sending, ssh: &Path, root: &str, list: &[u8]) -> Result<u64, String> {
    use tokio::io::AsyncWriteExt;
    let mut cmd = tokio::process::Command::new(ssh);
    cmd.current_dir(&s.dir)
        .args(["-F", "ssh_config", "dest", &command_for(&serde_json::json!({"op": "forget", "root": root}))])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd.spawn().map_err(|e| format!("ssh forget: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(list).await.map_err(|e| format!("ssh forget stdin: {e}"))?;
        let _ = stdin.shutdown().await;
    }
    let out = child.wait_with_output().await.map_err(|e| format!("ssh forget wait: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(stderr.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("forget failed").trim().to_owned());
    }
    let said = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = said.lines().rev().find_map(|l| serde_json::from_str(l.trim()).ok()).unwrap_or_default();
    Ok(v["gone"].as_u64().unwrap_or(0))
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

/// What stopping leaves alone of the account's: the account's own
/// systemd and what it keeps for it (its PAM half, its bus: `systemctl
/// --user` must still answer, to put things back), standard system user
/// daemons (ssh-agent, gpg-agent, accessibility/audio), SSH sessions
/// (`under_sshd`), and what is gone already (a zombie).
fn spared_kind(p: &Proc) -> bool {
    p.user_manager
        || p.comm == "(sd-pam)"
        || matches!(
            p.comm.as_str(),
            "dbus-broker"
                | "dbus-broker-lau"
                | "dbus-daemon"
                | "sshd"
                | "sshd-session"
                | "ssh-agent"
                | "gpg-agent"
                | "dirmngr"
                | "keyboxd"
                | "at-spi-bus-laun"
                | "pipewire"
                | "wireplumber"
                | "pulseaudio"
        )
        || p.state == 'Z'
}

/// Whether `pid` is an SSH server session (`sshd` / `sshd-session`) or a
/// live descendant of one: a move stops Rex, agents, services and
/// background work on the old host, never SSH logins or connections to it.
fn under_sshd(procs: &[Proc], mut pid: i32) -> bool {
    let mut seen = HashSet::new();
    while pid > 1 && seen.insert(pid) {
        let Some(p) = procs.iter().find(|p| p.pid == pid) else { break };
        if matches!(p.comm.as_str(), "sshd" | "sshd-session") {
            return true;
        }
        pid = p.ppid;
    }
    false
}

/// The processes stopping would end: the account's, but `keep` (this
/// helper's own line of parents and those spared), active SSH sessions
/// (`under_sshd`), and the spared kinds.
fn to_stop(procs: &[Proc], uid: u32, keep: &HashSet<i32>) -> Vec<i32> {
    procs.iter().filter(|p| p.uid == uid && !keep.contains(&p.pid) && !spared_kind(p) && !under_sshd(procs, p.pid)).map(|p| p.pid).collect()
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

/// Where `name` lives on this host: on `PATH`, or in standard system/user
/// folders an SSH command's path leaves out (`/usr/sbin`, `~/.local/bin`).
fn find_program(name: &str) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    dirs.extend(["/usr/local/sbin", "/usr/sbin", "/sbin", "/usr/local/bin", "/usr/bin", "/bin"].map(PathBuf::from));
    dirs.push(home().join(".local/bin"));
    dirs.into_iter().map(|d| d.join(name)).find(|p| std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0))
}

fn on_path(name: &str) -> bool {
    find_program(name).is_some()
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

/// Summary of the account's own entries in `/var/tmp` (both directories
/// and files owned by `uid`): `(total_bytes, total_files, total_count,
/// more, top_8_entries)` — bounded JSON even when `/var/tmp` has thousands
/// of folders.
fn var_tmp_summary(uid: u32) -> (u64, u64, u64, bool, Vec<serde_json::Value>) {
    use std::os::unix::fs::MetadataExt;
    let Ok(rd) = std::fs::read_dir("/var/tmp") else { return (0, 0, 0, false, Vec::new()) };
    let root_dev = std::fs::symlink_metadata("/var/tmp").map(|m| m.dev()).unwrap_or(0);
    let until = Instant::now() + Duration::from_secs(60);
    let (mut total_bytes, mut total_files, mut total_count, mut any_more) = (0u64, 0u64, 0u64, false);
    let mut entries: Vec<(String, u64, u64, bool)> = Vec::new();
    for e in rd.filter_map(|e| e.ok()) {
        let Some(name) = e.file_name().to_str().map(str::to_owned) else { continue };
        let Ok(meta) = std::fs::symlink_metadata(e.path()) else { continue };
        if meta.uid() != uid {
            continue;
        }
        total_count += 1;
        let (mut bytes, mut files, mut more) = (0u64, 0u64, false);
        if meta.is_dir() {
            let mut stack = vec![e.path()];
            while let Some(dir) = stack.pop() {
                if Instant::now() > until {
                    more = true;
                    any_more = true;
                    break;
                }
                let Ok(rd) = std::fs::read_dir(&dir) else { continue };
                for f in rd.filter_map(|f| f.ok()) {
                    let Ok(m) = std::fs::symlink_metadata(f.path()) else { continue };
                    if m.is_dir() {
                        if m.dev() == root_dev {
                            stack.push(f.path());
                        }
                    } else {
                        bytes += m.blocks() * 512;
                        files += 1;
                    }
                }
            }
        } else {
            bytes = meta.blocks() * 512;
            files = 1;
        }
        total_bytes += bytes;
        total_files += files;
        entries.push((format!("/var/tmp/{name}"), bytes, files, more));
    }
    entries.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    entries.truncate(8);
    let top = entries.into_iter().map(|(path, bytes, files, more)| serde_json::json!({"path": path, "bytes": bytes, "files": files, "more": more})).collect();
    (total_bytes, total_files, total_count, any_more, top)
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
/// command line `move send` for DIR), signaling its whole process group
/// (when detached via `setsid`) so child `rsync` and `ssh` processes stop
/// with it.
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
    // (When detached via `setsid`, `pid` is its own process group leader:
    // `-pid` terminates the coordinator and every child `rsync`/`ssh`.)
    // SAFETY: `getpgid` and `kill` on the verified move-send process.
    let target = unsafe { if libc::getpgid(pid) == pid { -pid } else { pid } };
    unsafe { libc::kill(target, libc::SIGTERM) };
    let until = Instant::now() + Duration::from_secs(5);
    while Instant::now() < until && proc_of(pid).is_some_and(|p| p.state != 'Z') {
        std::thread::sleep(Duration::from_millis(100));
    }
    if proc_of(pid).is_some_and(|p| p.state != 'Z') {
        // SAFETY: as above.
        unsafe { libc::kill(target, libc::SIGKILL) };
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
        ["rsync", "ssh", "crontab", "restorecon", "selinuxenabled", "sudo", "tmux", "screen", "podman", "docker", "git", "zsh", "fish", "bash", "loginctl", "systemctl", "usermod", "dnf", "apt-get"]
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
        let (vt_bytes, vt_files, vt_count, vt_more, top) = var_tmp_summary(uid);
        v["var_tmp_bytes"] = vt_bytes.into();
        v["var_tmp_files"] = vt_files.into();
        v["var_tmp_count"] = vt_count.into();
        v["var_tmp_more"] = vt_more.into();
        v["var_tmp"] = top.into();
        v["var_tmp_loose"] = 0u64.into();
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

    /// What the key may ask, and nothing else: `hello`, `forget`, or a
    /// receiving `rsync --server` (never `--sender`, never `--delete`,
    /// never shell metacharacters or dangerous flags), only where the
    /// account may write.
    #[test]
    fn a_moves_key_may_ask_only_its_own() {
        let ask = |json: &str| asked(&format!("olo-rex-link move do {}", hex(json)));
        assert_eq!(ask(r#"{"op":"hello"}"#), Some(Asked::Hello));
        assert_eq!(ask(r#"{"op":"forget","root":"/home/f"}"#), Some(Asked::Forget { root: "/home/f".into() }));
        assert_eq!(ask(r#"{"op":"shell","command":"rm -rf ~"}"#), None);
        assert_eq!(asked("rm -rf ~"), None);
        assert_eq!(asked(""), None);
        // (Validated receiving `rsync --server` commands, including escaped spaces.)
        let (args, dest) = asked_rsync("rsync --server -lHtprxSze.iLsfxCIvu --log-format=%i --timeout=120 --stats --partial . /home/a\\ b/dst/").unwrap();
        assert_eq!(dest, "/home/a b/dst/");
        assert_eq!(args[0], "--server");
        let (args_vt, dest_vt) = asked_rsync("rsync --server -lHtprRxSze.iLsfxCIvu --log-format=%i --timeout=120 --stats --partial --no-implied-dirs . /var/tmp/").unwrap();
        assert_eq!(dest_vt, "/var/tmp/");
        assert!(args_vt.contains(&"--no-implied-dirs".to_owned()));
        // (Refused rsync commands: sender, delete, remote shell, files-from, shell injection, extra paths.)
        for bad in [
            "rsync --server --sender -lHtprxSze.iLsfxCIvu . /home/f/",
            "rsync --server -lHtprxSze.iLsfxCIvu --delete . /home/f/",
            "rsync --server -lHtprxSze.iLsfxCIvu --delete-before . /home/f/",
            "rsync --server -e sh . /home/f/",
            "rsync --server -lHtprxSze.iLsfxCIvu --files-from=/etc/passwd . /home/f/",
            "rsync --server -lHtprxSze.iLsfxCIvu --rsh=sh . /home/f/",
            "rsync --server -lHtprxSze.iLsfxCIvu . /home/f/; rm -rf ~",
            "rsync --server -lHtprxSze.iLsfxCIvu . /home/f/ /tmp/",
            "rsync --server -lHtprxSze.iLsfxCIvu . $(id)",
        ] {
            assert_eq!(asked_rsync(bad), None, "must refuse: {bad}");
        }
        // (The home itself, by its real path; a folder under it a part of
        // the copy goes to, never one through `..`; `/var/tmp` only when allowed.)
        let home = PathBuf::from(format!("/var/tmp/olo-rex-link-roots-{}", std::process::id()));
        std::fs::create_dir_all(home.join("proj")).unwrap();
        let h = home.display().to_string();
        assert_eq!(writable_root(&h, &home, false), Some(Root::Home));
        assert_eq!(writable_root(&format!("{h}/"), &home, false), Some(Root::Home));
        assert_eq!(writable_root(&format!("{h}/proj"), &home, false), Some(Root::Part));
        assert_eq!(writable_root(&format!("{h}/proj/"), &home, false), Some(Root::Part));
        assert_eq!(writable_root(&format!("{h}/proj/.."), &home, false), None);
        assert_eq!(writable_root("/etc", &home, false), None);
        assert_eq!(writable_root("var/tmp/x", &home, false), None);
        assert_eq!(writable_root("/var/tmp", &home, true), Some(Root::VarTmp));
        assert_eq!(writable_root("/var/tmp/", &home, true), Some(Root::VarTmp));
        assert_eq!(writable_root("/var/tmp", &home, false), None);
        assert_eq!(writable_root("/var/tmp/olo-move-test-none-such", &home, true), None);
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
        assert_eq!(writable_root(&format!("{h}/src/olo"), &home, false), Some(Root::Part));
        assert_eq!(writable_root(&format!("{h}/src/new"), &home, false), Some(Root::Part), "not there yet");
        assert_eq!(writable_root(&format!("{h}/data/b"), &home, false), Some(Root::Part), "its folder not there yet either");
        for refused in [".ssh", ".local", ".local/share/olo", ".local/share/olo/rex-link", "link", "link/x"] {
            assert_eq!(writable_root(&format!("{h}/{refused}"), &home, false), None, "{refused}");
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

    /// What a later pass takes away: paths recorded by Pass 1 that no
    /// longer exist on the source (deduplicated); rsync progress2 parsing,
    /// octal unescaping, exclude escaping, and `/var/tmp` bucketing even
    /// with thousands of entries.
    #[test]
    fn a_later_pass_forgets_only_what_is_gone() {
        let dir = PathBuf::from(format!("/var/tmp/olo-rex-link-gone-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let root = dir.join("root");
        std::fs::create_dir_all(root.join("a")).unwrap();
        std::fs::write(root.join("a/stay"), "x").unwrap();
        std::fs::write(dir.join("sent"), b"a/stay\0a/gone\0b\0b\0").unwrap();
        assert_eq!(vanished_in_file(&dir.join("sent"), &root, true), b"a/gone\0b\0");
        assert_eq!(vanished_in_file(&dir.join("sent"), &root, false), b"a/stay\0a/gone\0b\0");
        assert!(vanished_in_file(&dir.join("none"), &root, true).is_empty());
        // (Progress2 parsing, octal unescaping, pattern escaping.)
        assert_eq!(parse_progress2("      1,081,344  34% 1024.00kB/s    0:00:02  "), Some((1_081_344, None)));
        assert_eq!(parse_progress2("        120,000 100%   83.19MB/s    0:00:00 (xfr#2, to-chk=0/4)"), Some((120_000, Some(2))));
        assert_eq!(parse_progress2("OLO_ITEM\t<f+++++++++\ta.txt"), None);
        assert_eq!(unescape_rsync(r"a\#011b/café.txt"), b"a\tb/caf\xc3\xa9.txt");
        assert_eq!(rsync_escape_pattern("dir/[a]*?"), r"/dir/\[a\]\*\?");
        // (Thousands of `/var/tmp` entries pack into at most 8 buckets.)
        let vt = dir.join("vt");
        std::fs::create_dir_all(&vt).unwrap();
        for i in 0..2000 {
            std::fs::write(vt.join(format!("e-{i:04}")), "x").unwrap();
        }
        // SAFETY: a getter.
        let uid = unsafe { libc::getuid() };
        let parts = var_tmp_buckets_in(&vt, uid, &dir, 1, 100).unwrap();
        assert!((2..=8).contains(&parts.len()), "expected 2..=8 buckets, got {}", parts.len());
        let listed: usize = parts.iter().map(|p| std::fs::read(p.files_from.as_ref().unwrap()).unwrap().split(|&b| b == 0).filter(|s| !s.is_empty()).count()).sum();
        assert_eq!(listed, 2000);
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
        let vt = sending(&args("/var/tmp/x/m1 --to f@h --host-key k|b --there /home/f --var-tmp").iter().map(|a| a.replace('|', " ")).collect::<Vec<_>>()).unwrap();
        assert!(vt.var_tmp);
        assert!(sending(&args("/var/tmp/x/m1 --to f@h --host-key k|b --there /home/f --also /etc").iter().map(|a| a.replace('|', " ")).collect::<Vec<_>>()).is_none());
        assert_eq!(sending(&args("/var/tmp/x/m1 --to f@h --host-key k|b --there /home/f").iter().map(|a| a.replace('|', " ")).collect::<Vec<_>>()).unwrap().port, 22);
        assert!(sending(&args("/var/tmp/x/m1 --to f@h --host-key k|b --there home/f").iter().map(|a| a.replace('|', " ")).collect::<Vec<_>>()).is_none(), "a relative root");
        assert!(sending(&args("/var/tmp/x/m1 --to h --host-key k|b --there /home/f").iter().map(|a| a.replace('|', " ")).collect::<Vec<_>>()).is_none(), "no account");
        assert!(sending(&args("/var/tmp/x/a.b --to f@h --host-key k|b --there /home/f").iter().map(|a| a.replace('|', " ")).collect::<Vec<_>>()).is_none(), "a folder not named by a move's id");
        // (Its ask, read back by `receive` as sent.)
        let ask = serde_json::json!({"op": "forget", "root": "/home/f"});
        assert_eq!(asked(&command_for(&ask)), Some(Asked::Forget { root: "/home/f".into() }));
    }

    /// Stopping spares this process's own line of parents, the account's
    /// systemd and session daemons, and any other SSH login sessions (and
    /// their children), and ends Rex, agents, and stray background work.
    #[test]
    fn stopping_spares_its_own_line() {
        let p = |pid, ppid, uid, comm: &str, state, user_manager| Proc { pid, ppid, uid, comm: comm.into(), state, user_manager };
        let procs = vec![
            p(1, 0, 0, "systemd", 'S', false),
            p(500, 1, 0, "sshd", 'S', false),
            p(600, 500, 1000, "sshd-session", 'S', false),
            p(601, 600, 1000, "sh", 'S', false),
            p(602, 601, 1000, "olo-rex-link", 'R', false),
            // Another live SSH session and its shell/command: must NOT be stopped.
            p(650, 500, 1000, "sshd-session", 'S', false),
            p(651, 650, 1000, "bash", 'S', false),
            p(652, 651, 1000, "vim", 'S', false),
            p(700, 1, 1000, "systemd", 'S', true),
            p(701, 700, 1000, "(sd-pam)", 'S', false),
            p(702, 700, 1000, "dbus-broker", 'S', false),
            p(703, 700, 1000, "ssh-agent", 'S', false),
            p(704, 700, 1000, "at-spi-bus-laun", 'S', false),
            p(800, 700, 1000, "rex", 'S', false),
            p(801, 800, 1000, "claude", 'S', false),
            p(802, 800, 1000, "defunct", 'Z', false),
            p(803, 700, 1000, "python3", 'S', false),
            p(900, 1, 1001, "bash", 'S', false),
        ];
        let keep = own_line(&procs, 602);
        assert_eq!(keep, HashSet::from([602, 601, 600, 500]));
        assert_eq!(to_stop(&procs, 1000, &keep), vec![800, 801, 803]);
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
