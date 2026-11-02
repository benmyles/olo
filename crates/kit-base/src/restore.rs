//! The state an app hands its new self across an in-place upgrade: the
//! old process writes it to a file in XDG_RUNTIME_DIR and names it in
//! `OLO_RESTORE` for the binary it re-executes as. Both kits claim the
//! file as they start ([`claim`]: read, then removed, asked for or not),
//! so a version that doesn't ask, or asks only on some paths, leaves
//! nothing behind in the runtime directory (memory, on tmpfs).

use olo_sync::Mutex;
use std::{path::PathBuf, process::Command, sync::OnceLock};

/// The variable naming the file.
pub const ENV: &str = "OLO_RESTORE";

fn slot() -> &'static Mutex<Option<String>> {
    static SLOT: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    SLOT.get_or_init(|| {
        let text = std::env::var_os(ENV).and_then(|path| {
            // SAFETY: claimed at start-up (the kits' `run`, first thing),
            // before the app starts threads that read the environment.
            unsafe { std::env::remove_var(ENV) };
            let text = std::fs::read_to_string(&path).ok();
            let _ = std::fs::remove_file(&path);
            text
        });
        Mutex::new(text.filter(|t| !t.is_empty()))
    })
}

/// Takes the file the previous version left, if any, now: kept for
/// [`take`], the file gone.
pub fn claim() {
    slot();
}

/// What the previous version left (once; None on a normal start, or if
/// it left nothing).
pub fn take() -> Option<String> {
    slot().lock().unwrap_or_else(|e| e.into_inner()).take()
}

/// Has `cmd` (the binary re-executing as the new version) carry
/// `state`: a file named for this process (whose pid it keeps), unless
/// there is nothing to carry.
pub fn hand_over(cmd: &mut Command, state: Option<&str>) {
    let Some(state) = state.filter(|s| !s.is_empty()) else { return };
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let file = dir.join(format!("olo-restore-{}.json", std::process::id()));
    if std::fs::write(&file, state).is_ok() {
        cmd.env(ENV, &file);
    }
}

/// Where SIGURG lands, for [`exec_on_upgrade`].
static UPGRADE_PIPE: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);

extern "C" fn on_upgrade_signal(_: libc::c_int) {
    let fd = UPGRADE_PIPE.load(std::sync::atomic::Ordering::Relaxed);
    if fd >= 0 {
        // SAFETY: write(2) is async-signal-safe.
        unsafe { libc::write(fd, [1u8].as_ptr().cast(), 1) };
    }
}

/// For a program with no kit loop (a session service's watcher, a
/// browser's messaging host): `then` runs, on a thread of its own, when
/// olo-session says the program's file was replaced (SIGURG, ignored by
/// default), to become the new version ([`exec_on_upgrade`]) or make
/// way for it.
pub fn on_upgrade(then: impl FnOnce() + Send + 'static) {
    use std::os::fd::FromRawFd as _;
    let mut fds = [0; 2];
    // SAFETY: a pipe, and a handler that only writes to it.
    unsafe {
        if libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) != 0 {
            return;
        }
        UPGRADE_PIPE.store(fds[1], std::sync::atomic::Ordering::Relaxed);
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = on_upgrade_signal as *const () as usize;
        action.sa_flags = libc::SA_RESTART;
        libc::sigemptyset(&mut action.sa_mask);
        libc::sigaction(libc::SIGURG, &action, std::ptr::null_mut());
    }
    // SAFETY: the read end, ours alone.
    let mut signals = unsafe { std::fs::File::from_raw_fd(fds[0]) };
    let _ = std::thread::Builder::new().name("upgrade".into()).spawn(move || {
        use std::io::Read as _;
        let mut b = [0u8; 1];
        if signals.read_exact(&mut b).is_ok() {
            then();
        }
    });
}

/// [`on_upgrade`]: re-executes as the new version with the same
/// arguments. Whatever the program must carry across is to be on disk
/// already.
pub fn exec_on_upgrade() {
    on_upgrade(|| {
        use std::os::unix::process::CommandExt as _;
        // (The file it ran from was replaced: /proc names it "(deleted)".)
        let Ok(exe) = std::env::current_exe() else { return };
        let exe = PathBuf::from(exe.to_string_lossy().trim_end_matches(" (deleted)"));
        let e = Command::new(&exe).args(std::env::args_os().skip(1)).exec();
        eprintln!("{}: couldn't become the new version: {e}", exe.display());
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_to_carry_writes_nothing() {
        let mut cmd = Command::new("true");
        hand_over(&mut cmd, Some(""));
        hand_over(&mut cmd, None);
        assert!(cmd.get_envs().all(|(k, _)| k != ENV));
    }
}
