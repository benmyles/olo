//! Tabula's Interface Kit.
//!
//! Apps implement [`App`] and call [`run`]. The kit owns the Wayland
//! connection, draws each window with egui on the GPU (wgpu/Vulkan), renders
//! the BeOS menu bar and popup menus, and keeps every window in step with
//! the desktop's light/dark theme.

mod app;
pub mod desktop_entry;
pub mod icons;
mod input;
pub mod menu;
mod platform;
pub mod style;
mod theme_watch;
pub mod widgets;

pub use app::{App, Command, Cx, Layer, LayerOptions, Proxy, WindowId, WindowKind, WindowOptions};
pub use egui;
pub use menu::{Action, Menu, MenuItem};
pub use platform::run;
pub use tabula_theme as theme;

/// Run a shell command detached in its own session (no inherited stdio),
/// as Deskbar and Tracker launch things. The child is reaped on a helper
/// thread so it never lingers as a zombie.
pub fn spawn_detached(cmd: &str) -> std::io::Result<()> {
    use std::os::unix::process::CommandExt;
    let mut c = std::process::Command::new("/bin/sh");
    c.arg("-c").arg(cmd);
    c.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    // SAFETY: setsid is async-signal-safe.
    unsafe {
        c.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let mut child = c.spawn()?;
    std::thread::spawn(move || child.wait());
    Ok(())
}
