//! The phone service and what it shares with Phone's command line: the
//! service itself (`service`, which `olo-phoned` runs) and the finding
//! of one-time codes in messages (`codes`). Nothing here draws: the
//! window is `olo-phone`'s (main.rs, window.rs).

pub mod codes;
pub mod kept;
pub mod others;
pub mod service;

use olo_protocol::phone::PhoneState;

/// After an upgrade olo-session sends SIGURG to the programs it replaced:
/// the service starts again as the new version (the browsers' hosts
/// connect again; a code waiting is let go).
pub fn upgrade_on_signal() {
    static FD: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);
    extern "C" fn on_signal(_: libc::c_int) {
        let fd = FD.load(std::sync::atomic::Ordering::Relaxed);
        if fd >= 0 {
            // SAFETY: write(2) is async-signal-safe.
            unsafe { libc::write(fd, [1u8].as_ptr().cast(), 1) };
        }
    }
    let mut fds = [0; 2];
    // SAFETY: a pipe and a signal handler that only writes to it.
    unsafe {
        if libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) != 0 {
            return;
        }
        FD.store(fds[1], std::sync::atomic::Ordering::Relaxed);
        libc::signal(libc::SIGURG, on_signal as *const () as libc::sighandler_t);
    }
    let read = fds[0];
    std::thread::Builder::new()
        .name("upgrade".into())
        .spawn(move || {
            loop {
                let mut b = [0u8; 1];
                // SAFETY: reading one byte into our buffer.
                if unsafe { libc::read(read, b.as_mut_ptr().cast(), 1) } != 1 {
                    return;
                }
                let exe = std::fs::read_link("/proc/self/exe").unwrap_or_default();
                let Some(path) = exe.to_str().and_then(|p| p.strip_suffix(" (deleted)")) else { continue };
                tracing::info!("phone: upgraded: starting again as the new version");
                let _ = std::fs::remove_file(PhoneState::socket());
                use std::os::unix::process::CommandExt;
                let e = std::process::Command::new(path).args(std::env::args_os().skip(1)).exec();
                tracing::warn!("phone: couldn't start the new version: {e}");
            }
        })
        .expect("thread");
}
