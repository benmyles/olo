#![warn(rust_2018_idioms)]
#![allow(clippy::collapsible_match)]
// If no backend is enabled, a large portion of the codebase is unused.
// So silence this useless warning for the CI.
#![cfg_attr(not(any(feature = "winit", feature = "udev")), allow(dead_code, unused_imports))]

#[cfg(feature = "udev")]
pub mod capture;
pub mod color;
pub mod contacts;
pub mod cursor;
pub mod drawing;
pub mod focus;
pub mod grabs;
#[cfg(feature = "udev")]
pub mod headless;
pub mod hints;
pub mod icon_theme;
pub mod input_counts;
pub mod input_handler;
#[cfg(feature = "libei")]
pub mod libei;
pub mod render;
pub mod shell;
pub mod state;
pub mod touch_health;
#[cfg(feature = "udev")]
pub mod udev;
#[cfg(feature = "winit")]
pub mod winit;

pub mod approve;
pub mod desktop;
pub mod dmabuf_check;
pub mod frames;
pub mod handover;
pub mod identify;
pub mod insert;
pub mod ipc;
pub mod keymap;
pub mod layout;
pub mod lock;
pub mod momentum;
pub mod night;
pub mod osd;
pub mod overview;
pub mod pace;
pub mod power;
pub mod saver;
pub mod screenshot;
pub mod scroll;
pub mod sockfile;
pub mod spotlight;
pub mod tabtip;
pub mod text;
pub mod theme;
pub mod tiles;
pub mod timings;
pub mod toolkits;
pub mod touch_filter;
pub mod twitcher;
pub mod wallpaper;
pub mod window_drop;
pub mod xcursor;

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

/// Stop the main loop cleanly on SIGTERM/SIGINT/SIGHUP (olo-session asks
/// us to stop this way before starting a new app_server), so the window
/// layout is saved for it.
pub fn stop_on_signals(running: std::sync::Arc<std::sync::atomic::AtomicBool>) {
    for sig in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
        let running = running.clone();
        // SAFETY: the handler only stores to an atomic.
        let _ = unsafe { signal_hook::low_level::register(sig, move || running.store(false, std::sync::atomic::Ordering::SeqCst)) };
    }
}
