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
/// [`take`], the file gone; and waits, on a thread, for the children the
/// previous self left (see [`reap_inherited`]).
pub fn claim() {
    slot();
    reap_inherited();
}

/// A program re-executed in place (an upgrade, the compositor or the GPU
/// lost) keeps its pid, so the children its previous self started are
/// still its own, but nothing in it waits for them: Activity Monitor's
/// helper, whose input pipe closed with the exec, ended and stayed a
/// zombie, one an upgrade (eleven in a day). A program starts with none
/// of its own, so whatever children it has as it starts are those: a
/// thread waits for each (a few microseconds; nothing when there are
/// none, the usual start).
fn reap_inherited() {
    // (Each of our threads' children: after an exec there is one thread.)
    let Ok(tasks) = std::fs::read_dir("/proc/self/task") else { return };
    let pids: Vec<libc::pid_t> =
        tasks.flatten().filter_map(|t| std::fs::read_to_string(t.path().join("children")).ok()).flat_map(|list| list.split_whitespace().filter_map(|p| p.parse().ok()).collect::<Vec<_>>()).collect();
    if pids.is_empty() {
        return;
    }
    tracing::info!(?pids, "waiting for the children the previous version left");
    let _ = std::thread::Builder::new().name("reap-inherited".into()).spawn(move || {
        for pid in pids {
            // SAFETY: waits for a child of ours that nothing else waits for.
            unsafe { libc::waitpid(pid, std::ptr::null_mut(), 0) };
        }
    });
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
    let file = olo_xdg::runtime_dir().join(format!("olo-restore-{}.json", std::process::id()));
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

/// For a program with a loop of its own (polling its input): a file
/// that becomes readable (a byte in it) when olo-session says the
/// program's file was replaced (SIGURG, ignored by default), for it to
/// become the new version ([`exec_new_version`]) when it's between
/// things. Once per program; None if it can't be had.
pub fn upgrade_signal() -> Option<std::fs::File> {
    use std::os::fd::FromRawFd as _;
    let mut fds = [0; 2];
    // SAFETY: a pipe, and a handler that only writes to it.
    unsafe {
        if libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) != 0 {
            return None;
        }
        UPGRADE_PIPE.store(fds[1], std::sync::atomic::Ordering::Relaxed);
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = on_upgrade_signal as *const () as usize;
        action.sa_flags = libc::SA_RESTART;
        libc::sigemptyset(&mut action.sa_mask);
        libc::sigaction(libc::SIGURG, &action, std::ptr::null_mut());
        // SAFETY: the read end, ours alone.
        Some(std::fs::File::from_raw_fd(fds[0]))
    }
}

/// Becomes the new version of this program, with the same arguments
/// and what's open without close-on-exec (stdin, stdout and stderr: a
/// server over them carries on); returns only if it couldn't.
pub fn exec_new_version() -> std::io::Error {
    use std::os::unix::process::CommandExt as _;
    // (The file it ran from was replaced: /proc names it "(deleted)".)
    let exe = match std::env::current_exe() {
        Ok(exe) => PathBuf::from(exe.to_string_lossy().trim_end_matches(" (deleted)")),
        Err(e) => return e,
    };
    Command::new(&exe).args(std::env::args_os().skip(1)).exec()
}

/// For a program with no kit loop (a session service's watcher, a
/// browser's messaging host): `then` runs, on a thread of its own, when
/// olo-session says the program's file was replaced (SIGURG, ignored by
/// default), to become the new version ([`exec_on_upgrade`]) or make
/// way for it.
pub fn on_upgrade(then: impl FnOnce() + Send + 'static) {
    let Some(mut signals) = upgrade_signal() else { return };
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
        let e = exec_new_version();
        eprintln!("couldn't become the new version: {e}");
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

    /// A child the previous self left (forgotten, as an exec forgets it)
    /// is waited for, so it doesn't stay a zombie.
    #[test]
    fn inherited_children_are_reaped() {
        let child = Command::new("true").spawn().expect("true runs");
        let pid = child.id();
        std::mem::forget(child);
        reap_inherited();
        let gone = (0..300).any(|_| {
            std::thread::sleep(std::time::Duration::from_millis(10));
            !std::path::Path::new(&format!("/proc/{pid}")).exists()
        });
        assert!(gone, "{pid} is still there");
    }
}
