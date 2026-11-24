//! The login session's control groups: the desktop and background work
//! as two groups the kernel can tell apart (kernel/TUNING.md,
//! "Background work kept to the memory that is to be had for nothing").
//!
//! logind gives a login one group for everything in it (`session-N.scope`):
//! the compositor, every app, a terminal's shells and every compiler an
//! agent starts. Olo's login divides it before the desktop starts:
//!
//! ```text
//! session-N.scope/     systemd's, root's; delegated (a drop-in of Doctor's)
//!   leader/            the login's own worker (root): the person can't move it
//!   olo/               nothing in it itself
//!     desk/            the session: everything, as it comes
//!     bg/              background work, idle on the processors against desk
//! ```
//!
//! The person gets only what moving a process between `desk` and `bg`
//! takes (their `cgroup.procs`, and `olo`'s, the two's common ancestor),
//! `bg`'s allowance of memory (`memory.high`) and, where the kernel has
//! them, having `bg`'s memory taken back (`memory.reclaim`: how the
//! keeper takes back what the group holds over its allowance,
//! `background`) and how much of it may be in swap (`memory.swap.max`:
//! how the kernel itself keeps the group's own memory out of swap where
//! the keeper's rule says so); it is their own background work's
//! memory, which the allowance already let them have taken back and
//! swapped; the directories and everything else stay root's. (That is the account's, not one
//! session's: the same person logged in a second time can move what is
//! in the first's two groups. Nobody else's processes can come in or
//! go out: the scope's own list stays root's.) Inside the scope a
//! process is still of its login session to logind, systemd-oomd isn't
//! watching (it watches the person's manager's slices), and `bg` is idle
//! against the desktop itself, not against a manager's other slices.
//!
//! **Made by root at login, so nothing here may keep anyone out**:
//! [`prepare`] checks everything it relies on and changes nothing until
//! it has; what it then makes it notes step by step, so that a step
//! failing takes apart exactly what was made and says whether that
//! worked; [`place`] puts the session's first process in `desk` while it
//! waits, and failing that [`abandon`] takes the division apart again.
//! The caller goes on without groups whatever these say.

use std::{
    ffi::CString,
    fs,
    io::Write as _,
    os::unix::ffi::OsStrExt as _,
    path::{Path, PathBuf},
};

/// Where the kernel's groups are.
pub const MOUNT: &str = "/sys/fs/cgroup";

/// The drop-in that has systemd leave a login session's scope to what is
/// made inside it (Doctor writes it; in force for logins after it).
pub const DELEGATE_CONF: &str = "etc/systemd/system/session-.scope.d/50-olo-groups.conf";

/// What that drop-in says. Only the two kinds of resource the groups
/// divide: a scope given more is left more alone than it needs to be.
pub const DELEGATE_CONF_TEXT: &str = "# Olo: its login divides a login session into the desktop and background work (two groups inside the session's own),\n# which systemd leaves alone only where the session's scope is delegated. Olo Doctor wrote this.\n[Scope]\nDelegate=cpu memory\n";

/// The group a process is in, as the kernel says it (`/proc/PID/cgroup`,
/// the one hierarchy's line): a path from the mount, "/user.slice/…".
pub fn of(pid: u32) -> Option<String> {
    parse(&fs::read_to_string(format!("/proc/{pid}/cgroup")).ok()?).map(str::to_owned)
}

/// This process's group.
pub fn own() -> Option<String> {
    parse(&fs::read_to_string("/proc/self/cgroup").ok()?).map(str::to_owned)
}

fn parse(text: &str) -> Option<&str> {
    text.lines().find_map(|l| l.strip_prefix("0::")).map(str::trim).filter(|p| p.starts_with('/'))
}

/// What [`prepare`] made, and how to take it apart again.
#[derive(Debug)]
pub struct Made {
    /// Where the session's first process goes ([`place`]).
    pub desk: PathBuf,
    pub bg: PathBuf,
    scope: PathBuf,
    steps: Steps,
}

/// What was done to a scope, in the order it was done.
#[derive(Debug, Default)]
struct Steps {
    /// Directories made.
    dirs: Vec<PathBuf>,
    /// The worker moved out of the scope's own group.
    moved: bool,
    /// Controllers handed down: where, and which.
    handed: Vec<(PathBuf, String)>,
}

fn write(path: &Path, value: &str) -> Result<(), String> {
    fs::OpenOptions::new().write(true).open(path).and_then(|mut f| f.write_all(value.as_bytes())).map_err(|e| format!("{}: {e}", path.display()))
}

fn read(path: &Path) -> Result<String, String> {
    fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Whether systemd has handed `dir` over (it marks what it delegates:
/// `user.delegate`, and `trusted.delegate`). Root's to set on a root's
/// directory, so the person can't make it so.
fn delegated(dir: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let Ok(metadata) = fs::symlink_metadata(dir) else { return false };
    // The worker is root. A scope that another account can write is not its boundary to divide.
    if !metadata.is_dir() || metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o022 != 0 {
        return false;
    }
    let Ok(path) = CString::new(dir.as_os_str().as_bytes()) else { return false };
    [c"user.delegate", c"trusted.delegate"].iter().any(|name| {
        let mut value = [0u8; 8];
        // SAFETY: our buffer and its length.
        let n = unsafe { libc::getxattr(path.as_ptr(), name.as_ptr(), value.as_mut_ptr().cast(), value.len()) };
        n == 1 && value[0] == b'1'
    })
}

/// The login session's scope divided, by root, before the session
/// starts: `own` is the caller's group ([`own`]), which must be the
/// scope of the session logind just made (`session`, PAM's
/// `XDG_SESSION_ID`) with the caller (`me`) alone in it. `uid` and `gid`
/// are the person's. An error says why nothing was made, and whether
/// what had been made by then was taken apart again.
pub fn prepare(mount: &Path, own: &str, session: &str, me: u32, uid: u32, gid: u32) -> Result<Made, String> {
    // What logind made for this login and nothing else: a name can't
    // climb out, and another session's scope isn't ours to divide.
    if session.is_empty() || !session.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return Err(format!("no session to divide ({session:?})"));
    }
    if own.split('/').any(|part| part == "..") || own.rsplit('/').next() != Some(format!("session-{session}.scope").as_str()) {
        return Err(format!("{own} isn't session {session}'s scope"));
    }
    let scope = mount.join(own.trim_start_matches('/'));
    if !delegated(&scope) {
        return Err("the session's scope isn't delegated (Olo Doctor's drop-in isn't in force for this login)".into());
    }
    // Alone in it, and nothing made in it yet: a helper some PAM module
    // left, or groups of someone else's making, and it isn't ours.
    let occupants = read(&scope.join("cgroup.procs"))?;
    if occupants.split_whitespace().collect::<Vec<_>>() != [me.to_string().as_str()] {
        return Err(format!("the scope holds more than the login's worker ({})", occupants.split_whitespace().collect::<Vec<_>>().join(" ")));
    }
    let scan_error = |e: std::io::Error| format!("{}: {e}", scope.display());
    for entry in fs::read_dir(&scope).map_err(scan_error)? {
        if entry.map_err(scan_error)?.file_type().map_err(scan_error)?.is_dir() {
            return Err("the scope already has groups inside".into());
        }
    }
    let controllers = read(&scope.join("cgroup.controllers"))?;
    let has = |name: &str| controllers.split_whitespace().any(|c| c == name);
    if !has("memory") {
        return Err("the scope has no say over memory".into());
    }
    // (Handed down already, by whoever: not ours to take back later.)
    let handed = read(&scope.join("cgroup.subtree_control"))?;
    let wanted: Vec<&str> = ["memory", "cpu"].into_iter().filter(|c| has(c) && !handed.split_whitespace().any(|h| h == *c)).collect();
    let mut made = Made { desk: scope.join("olo/desk"), bg: scope.join("olo/bg"), scope, steps: Steps::default() };
    match build(&mut made, me, uid, gid, &wanted) {
        Ok(()) => Ok(made),
        Err(why) => Err(match take_apart(&made.scope, &made.steps, &[me]) {
            Ok(()) => format!("{why} (undone)"),
            Err(left) => format!("{why}; and not all of it could be undone: {left}"),
        }),
    }
}

fn build(made: &mut Made, me: u32, uid: u32, gid: u32, controllers: &[&str]) -> Result<(), String> {
    let scope = made.scope.clone();
    let (leader, olo) = (scope.join("leader"), scope.join("olo"));
    let (desk, bg) = (made.desk.clone(), made.bg.clone());
    for dir in [&leader, &olo, &desk, &bg] {
        fs::create_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        made.steps.dirs.push(dir.clone());
    }
    // Out of the scope's own group first: a group that hands a resource
    // down may hold no process itself.
    write(&leader.join("cgroup.procs"), &me.to_string())?;
    made.steps.moved = true;
    if !read(&scope.join("cgroup.procs"))?.trim().is_empty() {
        return Err("the scope didn't empty".into());
    }
    // (One at a time: a write naming two fails whole if either does.)
    for dir in [&scope, &olo] {
        for c in controllers {
            write(&dir.join("cgroup.subtree_control"), &format!("+{c}"))?;
            made.steps.handed.push((dir.clone(), (*c).to_owned()));
        }
    }
    for file in [bg.join("memory.high"), bg.join("memory.current"), bg.join("cgroup.events"), desk.join("memory.pressure")] {
        if !file.exists() {
            return Err(format!("{} isn't there", file.display()));
        }
    }
    // Idle against the desktop: whatever is in `bg` runs when nothing of
    // `desk`'s wants the processor, and gives way at once when it does.
    if bg.join("cpu.idle").exists() {
        write(&bg.join("cpu.idle"), "1")?;
    }
    // The person's: moving between the two leaves (the leaves' lists and
    // their common ancestor's), and the background's allowance. Nothing
    // else: the leader's group is out of their reach by the scope's own
    // list, which stays root's.
    for file in [olo.join("cgroup.procs"), desk.join("cgroup.procs"), bg.join("cgroup.procs"), bg.join("memory.high")] {
        std::os::unix::fs::chown(&file, Some(uid), Some(gid)).map_err(|e| format!("{}: {e}", file.display()))?;
    }
    // And, for its keeper, the background's memory taken back and how
    // much of it may be in swap. Not a reason to make nothing: a kernel
    // without a file, or the hand-over refused, leaves the keeper the
    // allowance alone.
    for file in [bg.join("memory.reclaim"), bg.join("memory.swap.max")] {
        if file.exists() {
            let _ = std::os::unix::fs::chown(&file, Some(uid), Some(gid));
        }
    }
    Ok(())
}

/// Exactly what `steps` says was done, undone in the opposite order:
/// what was handed down taken back, `pids` put back in the scope's own
/// group, the directories removed. Every step is tried; what couldn't be
/// done is said.
fn take_apart(scope: &Path, steps: &Steps, pids: &[u32]) -> Result<(), String> {
    let mut left = Vec::new();
    for (dir, controller) in steps.handed.iter().rev() {
        if let Err(e) = write(&dir.join("cgroup.subtree_control"), &format!("-{controller}")) {
            left.push(e);
        }
    }
    if steps.moved {
        for pid in pids {
            if let Err(e) = write(&scope.join("cgroup.procs"), &pid.to_string()) {
                left.push(e);
            }
        }
    }
    for dir in steps.dirs.iter().rev() {
        if let Err(e) = fs::remove_dir(dir) {
            left.push(format!("{}: {e}", dir.display()));
        }
    }
    if left.is_empty() { Ok(()) } else { Err(left.join("; ")) }
}

/// The session's first process (`child`: just made, still root, waiting
/// to be let go) put in `desk` by the login's worker, which then knows
/// whether it is there: everything the session starts is born where it
/// is.
pub fn place(made: &Made, child: u32) -> Result<(), String> {
    write(&made.desk.join("cgroup.procs"), &child.to_string())?;
    let there = read(&made.desk.join("cgroup.procs"))?;
    if there.split_whitespace().any(|pid| pid == child.to_string()) { Ok(()) } else { Err("the session's first process isn't in its desktop group".into()) }
}

/// The division taken apart again (the session's first process couldn't
/// be put in `desk`: a session that ran where the worker is would be
/// one group that looks like two). `me` and the waiting `child` go back
/// to the scope's own group. Ok: the scope is as it was found.
pub fn abandon(made: Made, me: u32, child: Option<u32>) -> Result<(), String> {
    let pids: Vec<u32> = std::iter::once(me).chain(child).collect();
    take_apart(&made.scope, &made.steps, &pids)
}

/// The two groups as a process of the session's finds them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Groups {
    pub desk: PathBuf,
    pub bg: PathBuf,
}

/// The groups of the session a process in `own` is in: where it is in
/// its `desk` and may move processes to `bg` and set `bg`'s allowance.
/// None in a session Olo's login didn't divide.
pub fn find(mount: &Path, own: &str) -> Option<Groups> {
    let olo = own.strip_suffix("/olo/desk").filter(|scope| scope.rsplit('/').next().is_some_and(|s| s.starts_with("session-") && s.ends_with(".scope")))?;
    let olo = mount.join(olo.trim_start_matches('/')).join("olo");
    let (desk, bg) = (olo.join("desk"), olo.join("bg"));
    // SAFETY: a plain system call on our string.
    let ours = |file: PathBuf| CString::new(file.as_os_str().as_bytes()).is_ok_and(|p| unsafe { libc::access(p.as_ptr(), libc::W_OK) } == 0);
    (ours(olo.join("cgroup.procs")) && ours(desk.join("cgroup.procs")) && ours(bg.join("cgroup.procs")) && ours(bg.join("memory.high"))).then_some(Groups { desk, bg })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_kernels_line() {
        assert_eq!(parse("0::/user.slice/user-1000.slice/session-3.scope\n"), Some("/user.slice/user-1000.slice/session-3.scope"));
        assert_eq!(parse("1:name=systemd:/x\n0::/a/b\n"), Some("/a/b"));
        assert_eq!(parse("0::relative\n"), None);
        assert_eq!(parse(""), None);
    }

    /// A made-up tree of plain files: what `prepare` refuses before it
    /// changes anything, and that a step failing leaves nothing behind.
    /// (What the kernel does at each write is tried in a guest, as root:
    /// `tools/dev/kernel-check session-groups`.)
    fn tree(procs: &str, controllers: &str) -> (tempfile::TempDir, String) {
        let dir = tempfile::Builder::new().prefix("olo-session-groups-").tempdir_in("/var/tmp").unwrap();
        let own = "/user.slice/user-1000.slice/session-7.scope";
        let scope = dir.path().join(own.trim_start_matches('/'));
        fs::create_dir_all(&scope).unwrap();
        // (As the kernel makes a scope, whatever the umask: under 0002 it was
        // writable by its group, and refused as not the worker's boundary.)
        fs::set_permissions(&scope, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        fs::write(scope.join("cgroup.procs"), procs).unwrap();
        fs::write(scope.join("cgroup.controllers"), controllers).unwrap();
        fs::write(scope.join("cgroup.subtree_control"), "").unwrap();
        (dir, own.to_owned())
    }

    fn mark_delegated(dir: &Path, own: &str) -> bool {
        let path = CString::new(dir.join(own.trim_start_matches('/')).as_os_str().as_bytes()).unwrap();
        // SAFETY: our strings.
        unsafe { libc::setxattr(path.as_ptr(), c"user.delegate".as_ptr(), b"1".as_ptr().cast(), 1, 0) == 0 }
    }

    #[test]
    fn refuses_what_isnt_this_logins_scope_alone_and_delegated() {
        let (dir, own) = tree("41\n", "cpu io memory pids\n");
        let before = || fs::read_dir(dir.path().join(own.trim_start_matches('/'))).unwrap().count();
        let files = before();
        // Not delegated.
        assert!(prepare(dir.path(), &own, "7", 41, 1000, 1000).unwrap_err().contains("isn't delegated"));
        // Another session's scope, a name that climbs, no session.
        assert!(prepare(dir.path(), &own, "8", 41, 1000, 1000).unwrap_err().contains("isn't session 8's scope"));
        assert!(prepare(dir.path(), "/user.slice/../session-7.scope", "7", 41, 1000, 1000).is_err());
        assert!(prepare(dir.path(), &own, "", 41, 1000, 1000).is_err());
        assert!(prepare(dir.path(), &own, "7/..", 41, 1000, 1000).is_err());
        if !mark_delegated(dir.path(), &own) {
            // (A filesystem without attributes of a person's: nothing more
            // to try here.)
            return;
        }
        // Someone else in it; a group already in it; no say over memory.
        let scope = dir.path().join(own.trim_start_matches('/'));
        fs::write(scope.join("cgroup.procs"), "41\n977\n").unwrap();
        assert!(prepare(dir.path(), &own, "7", 41, 1000, 1000).unwrap_err().contains("more than the login's worker"));
        fs::write(scope.join("cgroup.procs"), "41\n").unwrap();
        assert!(prepare(dir.path(), &own, "7", 40, 1000, 1000).unwrap_err().contains("more than the login's worker"));
        fs::create_dir(scope.join("other")).unwrap();
        assert!(prepare(dir.path(), &own, "7", 41, 1000, 1000).unwrap_err().contains("already has groups"));
        fs::remove_dir(scope.join("other")).unwrap();
        fs::write(scope.join("cgroup.controllers"), "cpu io pids\n").unwrap();
        assert!(prepare(dir.path(), &own, "7", 41, 1000, 1000).unwrap_err().contains("no say over memory"));
        assert_eq!(before(), files);
        // Everything as it should be found: plain files don't do what the
        // kernel's do (a directory made here has no list to write to), so
        // the making stops at its first write and is taken apart: only
        // what was made, and said to be undone.
        fs::write(scope.join("cgroup.controllers"), "cpu io memory pids\n").unwrap();
        let why = prepare(dir.path(), &own, "7", 41, 1000, 1000).unwrap_err();
        assert!(why.ends_with("(undone)"), "{why}");
        assert_eq!(before(), files);
        assert!(!scope.join("leader").exists() && !scope.join("olo").exists());
        assert_eq!(fs::read_to_string(scope.join("cgroup.subtree_control")).unwrap(), "");
    }

    /// Taken apart: only what the steps say was done, in the opposite
    /// order, and what can't be undone is said, not passed over.
    #[test]
    fn takes_apart_only_what_was_made_and_says_what_it_couldnt() {
        let (dir, own) = tree("", "cpu memory\n");
        let scope = dir.path().join(own.trim_start_matches('/'));
        // A directory that was there before isn't touched.
        fs::create_dir(scope.join("theirs")).unwrap();
        let mut steps = Steps::default();
        for name in ["leader", "olo"] {
            fs::create_dir(scope.join(name)).unwrap();
            steps.dirs.push(scope.join(name));
        }
        steps.moved = true;
        assert_eq!(take_apart(&scope, &steps, &[41, 42]), Ok(()));
        assert!(scope.join("theirs").exists() && !scope.join("leader").exists() && !scope.join("olo").exists());
        // (Plain files keep the last write: the second of the two.)
        assert_eq!(fs::read_to_string(scope.join("cgroup.procs")).unwrap(), "42");
        // Something in the way: said.
        fs::create_dir_all(scope.join("olo/desk")).unwrap();
        let steps = Steps { dirs: vec![scope.join("olo")], ..Steps::default() };
        assert!(take_apart(&scope, &steps, &[41]).unwrap_err().contains("olo"));
    }

    #[test]
    fn finds_the_groups_only_from_a_divided_sessions_desk() {
        let dir = tempfile::Builder::new().prefix("olo-session-groups-").tempdir_in("/var/tmp").unwrap();
        let olo = dir.path().join("user.slice/user-1000.slice/session-7.scope/olo");
        for leaf in ["desk", "bg"] {
            fs::create_dir_all(olo.join(leaf)).unwrap();
            fs::write(olo.join(leaf).join("cgroup.procs"), "").unwrap();
        }
        fs::write(olo.join("cgroup.procs"), "").unwrap();
        let own = "/user.slice/user-1000.slice/session-7.scope/olo/desk";
        // (No allowance to set yet: not ours.)
        assert_eq!(find(dir.path(), own), None);
        fs::write(olo.join("bg/memory.high"), "max\n").unwrap();
        assert_eq!(find(dir.path(), own), Some(Groups { desk: olo.join("desk"), bg: olo.join("bg") }));
        // As a session is today, in its background group, in a group of
        // the person's manager, or under another name.
        for elsewhere in ["/user.slice/user-1000.slice/session-7.scope", "/user.slice/user-1000.slice/session-7.scope/olo/bg", "/user.slice/user-1000.slice/user@1000.service/olo/desk", "/x/olo/desk"]
        {
            assert_eq!(find(dir.path(), elsewhere), None, "{elsewhere}");
        }
    }
}
