#![warn(rust_2018_idioms)]
#![allow(clippy::collapsible_match)]
// If no backend is enabled, a large portion of the codebase is unused.
// So silence this useless warning for the CI.
#![cfg_attr(
    not(any(feature = "winit", feature = "udev")),
    allow(dead_code, unused_imports)
)]

#[cfg(any(feature = "udev", feature = "xwayland"))]
pub mod cursor;
pub mod drawing;
pub mod focus;
pub mod input_handler;
#[cfg(feature = "libei")]
pub mod libei;
pub mod render;
pub mod shell;
pub mod state;
#[cfg(feature = "udev")]
pub mod udev;
#[cfg(feature = "udev")]
pub mod headless;
#[cfg(feature = "winit")]
pub mod winit;

pub mod desktop;
pub mod identify;
pub mod ipc;
pub mod keymap;
pub mod layout;
pub mod lock;
pub mod momentum;
pub mod night;
pub mod osd;
pub mod power;
pub mod saver;
pub mod screenshot;
pub mod sockfile;
pub mod text;
pub mod theme;
pub mod twitcher;
pub mod wallpaper;

pub use state::{AnvilState, ClientState};

/// Keep an inherited fd from leaking into the programs we start.
pub fn set_cloexec(fd: i32) {
    // SAFETY: plain fcntl on an fd we own.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFD);
        if flags >= 0 {
            libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC);
        }
    }
}

/// Stop the main loop cleanly on SIGTERM/SIGINT/SIGHUP (tabula-session asks
/// us to stop this way before starting a new app_server), so the window
/// layout is saved for it.
pub fn stop_on_signals(running: std::sync::Arc<std::sync::atomic::AtomicBool>) {
    for sig in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
        let running = running.clone();
        // SAFETY: the handler only stores to an atomic.
        let _ = unsafe {
            signal_hook::low_level::register(sig, move || running.store(false, std::sync::atomic::Ordering::SeqCst))
        };
    }
}
