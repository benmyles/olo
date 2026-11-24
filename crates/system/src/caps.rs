//! The one capability Olo's login hands on, and how it goes from hand to
//! hand. Intel's graphics drivers give a program's work the GPU's high
//! priority only where the program has `CAP_SYS_NICE` (Mesa asks the
//! kernel for the ordinary one otherwise, and says nothing): so the
//! person's own compositor, and nothing else, is to have it.
//!
//! The login checks the administrator's per-account marker and sets exactly
//! one bit on the change to the selected UID. The session and compositor
//! drop the inheritable and ambient sets before starting work. A fork still
//! copies the calling thread's credentials until they are dropped or the
//! child executes an ordinary file. The session prepares its current thread
//! for an exec only for its seat compositor or its own replacement.

/// `CAP_SYS_NICE`.
pub const SYS_NICE: u32 = 23;

mod choice;
pub use choice::{CHOICE_DIR, OLD_SWITCH, chosen, set_choice};

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
#[derive(Default, Clone, Copy, PartialEq)]
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
    let _ = keep_for_login();
}

pub fn keep_for_login() -> std::io::Result<()> {
    if unsafe { libc::prctl(libc::PR_SET_KEEPCAPS, 1 as libc::c_ulong, 0 as libc::c_ulong, 0 as libc::c_ulong, 0 as libc::c_ulong) } == 0 { Ok(()) } else { Err(std::io::Error::last_os_error()) }
}

fn set_exact(cap: Option<u32>, forward: bool) -> std::io::Result<()> {
    let mut data = [Data::default(); 2];
    if let Some(cap) = cap {
        let bit = 1 << (cap % 32);
        data[(cap / 32) as usize] = Data { effective: bit, permitted: bit, inheritable: if forward { bit } else { 0 } };
    }
    let mut header = Header { version: VERSION, pid: 0 };
    // SAFETY: the two words use the kernel's v3 layout. Dropping I also drops A.
    if unsafe { libc::syscall(libc::SYS_capset, &mut header as *mut Header, data.as_ptr()) } == 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    // Some sandboxes deny even a no-op capset. Continue only when capget
    // independently confirms the requested sets.
    if sets() == Some(data) { Ok(()) } else { Err(error) }
}

/// Keep the exact effective and permitted bit requested, with nothing inheritable or ambient.
pub fn rest(cap: Option<u32>) -> std::io::Result<()> {
    set_exact(cap, false)
}

pub fn narrow(cap: u32) -> bool {
    if set_exact(Some(cap), true).is_ok() {
        return true;
    }
    let _ = rest(None);
    false
}

/// Prepare the current thread for the one intended exec. The caller must
/// restore its resting state on every error, including a returned exec.
pub fn for_exec(cap: u32) -> std::io::Result<()> {
    set_exact(Some(cap), true)?;
    if hand_on(cap) { Ok(()) } else { Err(std::io::Error::last_os_error()) }
}

/// The ambient bit this thread actually received for an ordinary exec.
pub fn ambient(cap: u32) -> bool {
    unsafe { libc::prctl(libc::PR_CAP_AMBIENT, libc::PR_CAP_AMBIENT_IS_SET as libc::c_ulong, libc::c_ulong::from(cap), 0, 0) == 1 }
}

/// The next program this process starts (itself, by `exec`) has `cap`.
pub fn hand_on(cap: u32) -> bool {
    // SAFETY: a plain system call.
    unsafe { libc::prctl(libc::PR_CAP_AMBIENT, libc::PR_CAP_AMBIENT_RAISE as libc::c_ulong, libc::c_ulong::from(cap), 0, 0) == 0 }
}

/// Clear only ambient capabilities. Inheritable, permitted and effective are preserved.
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
        let scratch = tempfile::Builder::new().prefix("olo-caps-choice-").tempdir_in("/var/tmp").unwrap();
        let root = scratch.path();
        // SAFETY: getuid has no preconditions.
        let me = unsafe { libc::getuid() };
        assert!(!chosen(root, me, 1000));
        let file = root.join(CHOICE_DIR).join("1000");
        let dir = file.parent().unwrap();
        std::fs::create_dir_all(dir).unwrap();
        let mode = |p: &std::path::Path, m: u32| std::fs::set_permissions(p, std::fs::Permissions::from_mode(m)).unwrap();
        // (Every folder on the way as root's are, whatever the umask: under
        // 0002, the scratch folder, etc and etc/olo were writable by their
        // group, and refused.)
        for d in [root.to_path_buf(), root.join("etc"), root.join("etc/olo"), dir.to_path_buf()] {
            mode(&d, 0o755);
        }
        set_choice(root, me, 1000, true).unwrap();
        assert!(!chosen(root, me, 1001));
        mode(&file, 0o644);
        assert!(chosen(root, me, 1000));
        // Someone else's; writable by others; in a directory others may
        // write; a link to a file.
        assert!(!chosen(root, me + 1, 1000));
        mode(&file, 0o664);
        assert!(!chosen(root, me, 1000));
        mode(&file, 0o644);
        mode(dir, 0o777);
        assert!(!chosen(root, me, 1000));
        mode(dir, 0o755);
        std::fs::rename(&file, dir.join("real")).unwrap();
        std::os::unix::fs::symlink(dir.join("real"), &file).unwrap();
        assert!(!chosen(root, me, 1000));
        assert!(set_choice(root, me, 1000, true).is_err());
    }
}
