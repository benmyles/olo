//! The one capability Olo's login hands on, and how it goes from hand to
//! hand. Intel's graphics drivers give a program's work the GPU's high
//! priority only where the program has `CAP_SYS_NICE` (Mesa asks the
//! kernel for the ordinary one otherwise, and says nothing): so the
//! person's own compositor, and nothing else, is to have it.
//!
//! Capabilities don't survive a change of user or a program's start
//! unless each step says so:
//! - the login's worker, root, about to become the person: [`keep`]
//!   before the change of user, [`narrow`] after it (that one alone, of
//!   all root had), [`hand_on`] so the program it starts has it;
//! - `olo-session`, started with it: [`held`], then [`hand_on_nothing`]
//!   so nothing it starts has it, and [`hand_on`] again in the one child
//!   that is the person's compositor (and before it starts itself again
//!   for an upgrade);
//! - `olo-app-server`: [`hand_on_nothing`] first thing, so what it
//!   starts has nothing.
//!
//! Every call here is a plain system call (safe between `fork` and
//! `exec`), and a failure leaves the process with less, never more: a
//! program started by an account that isn't root keeps only what was
//! handed on, whatever its parent still held.

/// `CAP_SYS_NICE`.
pub const SYS_NICE: u32 = 23;

/// Where the computer's owner says that a person's Olo session is to
/// have the capability (from the root of the filesystem). It is theirs
/// to say, not Olo's: the programs it is handed to are the person's own
/// files, so it is the account that gets it, and what it allows
/// (any priority, realtime among them, and the scheduling of any
/// program on the computer) is the owner's to give. Olo Doctor offers
/// it in those words and writes this; without it the login hands on
/// nothing.
pub const CHOICE: &str = "etc/olo/compositor-first";

/// What that file says (only its being there counts).
pub const CHOICE_TEXT: &str = "# Olo: this computer's owner chose that a person's Olo session gets CAP_SYS_NICE at login, for its compositor\n# (the graphics driver gives the GPU's high priority only with it). Olo Doctor wrote this; remove it to take the choice back.\n";

/// Whether the owner made that choice, under `root`: the file is there,
/// a plain file (not a link to one) that is `owner`'s (root's) and that
/// nobody else may write, in a directory the same. Anything else is no
/// choice.
pub fn chosen(root: &std::path::Path, owner: u32) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    let file = root.join(CHOICE);
    let theirs = |m: std::fs::Metadata| m.uid() == owner && m.mode() & 0o022 == 0;
    std::fs::symlink_metadata(&file).is_ok_and(|m| m.is_file() && theirs(m)) && file.parent().and_then(|dir| std::fs::symlink_metadata(dir).ok()).is_some_and(|m| m.is_dir() && theirs(m))
}

/// Whether process `pid` may use `cap` now, as anyone can read it (its
/// status: a process that holds a capability can't otherwise be looked
/// into by one that doesn't).
pub fn held_by(pid: u32, cap: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/status")).ok().is_some_and(|status| effective_in(&status, cap))
}

/// [`held_by`], from a process's status as read.
pub fn effective_in(status: &str, cap: u32) -> bool {
    status.lines().find_map(|l| l.strip_prefix("CapEff:")).and_then(|hex| u64::from_str_radix(hex.trim(), 16).ok()).is_some_and(|held| held & (1 << cap) != 0)
}

/// `_LINUX_CAPABILITY_VERSION_3`: two words of each set.
const VERSION: u32 = 0x2008_0522;

#[repr(C)]
struct Header {
    version: u32,
    pid: i32,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Data {
    effective: u32,
    permitted: u32,
    inheritable: u32,
}

fn sets() -> Option<[Data; 2]> {
    let mut header = Header { version: VERSION, pid: 0 };
    let mut data = [Data::default(); 2];
    // SAFETY: the kernel's structures, ours to fill.
    (unsafe { libc::syscall(libc::SYS_capget, &mut header as *mut Header, data.as_mut_ptr()) } == 0).then_some(data)
}

/// Whether this process may use `cap` now.
pub fn held(cap: u32) -> bool {
    sets().is_some_and(|d| d[(cap / 32) as usize].effective & (1 << (cap % 32)) != 0)
}

/// What this process holds stays through a change of user (cleared by
/// the kernel at the next program's start).
pub fn keep() {
    // SAFETY: a plain system call.
    unsafe { libc::prctl(libc::PR_SET_KEEPCAPS, 1, 0, 0, 0) };
}

/// Everything this process holds given up but `cap`, which it may use
/// and hand on. False (and nothing held at all, if that can be had)
/// where the kernel refused.
pub fn narrow(cap: u32) -> bool {
    let mut data = [Data::default(); 2];
    let bit = 1 << (cap % 32);
    data[(cap / 32) as usize] = Data { effective: bit, permitted: bit, inheritable: bit };
    let mut header = Header { version: VERSION, pid: 0 };
    // SAFETY: the kernel's structures, read by it.
    if unsafe { libc::syscall(libc::SYS_capset, &mut header as *mut Header, data.as_ptr()) } == 0 {
        return true;
    }
    let none = [Data::default(); 2];
    let mut header = Header { version: VERSION, pid: 0 };
    // SAFETY: as above.
    unsafe { libc::syscall(libc::SYS_capset, &mut header as *mut Header, none.as_ptr()) };
    false
}

/// The next program this process starts (itself, by `exec`) has `cap`.
pub fn hand_on(cap: u32) -> bool {
    // SAFETY: a plain system call.
    unsafe { libc::prctl(libc::PR_CAP_AMBIENT, libc::PR_CAP_AMBIENT_RAISE as libc::c_ulong, libc::c_ulong::from(cap), 0, 0) == 0 }
}

/// Nothing this process starts has any capability of its.
pub fn hand_on_nothing() {
    // SAFETY: a plain system call.
    unsafe { libc::prctl(libc::PR_CAP_AMBIENT, libc::PR_CAP_AMBIENT_CLEAR_ALL as libc::c_ulong, 0, 0, 0) };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An ordinary account's process holds nothing and can hand nothing
    /// on; asking changes nothing (tests don't run as root).
    #[test]
    fn nothing_to_hand_on_without_it() {
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        assert!(!held(SYS_NICE));
        assert!(!hand_on(SYS_NICE));
        hand_on_nothing();
        assert!(!held(SYS_NICE));
        assert!(sets().is_some());
        assert!(!held_by(std::process::id(), SYS_NICE));
        assert!(effective_in("Name:\tx\nCapEff:\t0000000000800000\n", SYS_NICE) && !effective_in("CapEff:\t0000000000000000\n", SYS_NICE) && !effective_in("", SYS_NICE));
    }

    /// The owner's choice is a plain file of theirs that nobody else may
    /// write, in a directory the same: anything else is no choice.
    #[test]
    fn the_choice_is_the_owners_file_and_nothing_else() {
        use std::os::unix::fs::PermissionsExt as _;
        let root = std::path::PathBuf::from(format!("/var/tmp/olo-caps-choice-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        // SAFETY: getuid has no preconditions.
        let me = unsafe { libc::getuid() };
        assert!(!chosen(&root, me));
        let file = root.join(CHOICE);
        let dir = file.parent().unwrap();
        std::fs::create_dir_all(dir).unwrap();
        let mode = |p: &std::path::Path, m: u32| std::fs::set_permissions(p, std::fs::Permissions::from_mode(m)).unwrap();
        mode(dir, 0o755);
        std::fs::write(&file, CHOICE_TEXT).unwrap();
        mode(&file, 0o644);
        assert!(chosen(&root, me));
        // Someone else's; writable by others; in a directory others may
        // write; a link to a file.
        assert!(!chosen(&root, me + 1));
        mode(&file, 0o664);
        assert!(!chosen(&root, me));
        mode(&file, 0o644);
        mode(dir, 0o777);
        assert!(!chosen(&root, me));
        mode(dir, 0o755);
        std::fs::rename(&file, dir.join("real")).unwrap();
        std::os::unix::fs::symlink(dir.join("real"), &file).unwrap();
        assert!(!chosen(&root, me));
        std::fs::remove_dir_all(&root).unwrap();
    }
}
