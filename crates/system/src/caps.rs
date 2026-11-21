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
    }
}
