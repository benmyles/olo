//! What a few programs need in their environment to work their best in
//! Olo, given as Olo starts them (`spawn`, `spawn_app`,
//! `spawn_detached`):
//!
//! - Zoom's own app offers screen sharing on Wayland only when its
//!   `XDG_CURRENT_DESKTOP` is GNOME (it checks before asking the
//!   portal, which serves it as any app); xdg-desktop-portal picks its
//!   backends by its own environment, not Zoom's, so Olo's still answer.
//!   (Zoom in Chrome needs nothing.)

use std::{ffi::OsStr, path::Path};

/// Program `program`'s quirks, onto `cmd`.
pub fn apply(cmd: &mut std::process::Command, program: &OsStr) {
    if is_zoom(program) {
        // (Exactly: Olo's "Olo:GNOME" may not be taken for it.)
        cmd.env("XDG_CURRENT_DESKTOP", "GNOME");
    }
}

/// Zoom's app: `zoom` (its launcher), or anything in /opt/zoom.
fn is_zoom(program: &OsStr) -> bool {
    let p = Path::new(program);
    p.file_name().is_some_and(|n| n == "zoom" || n == "ZoomLauncher") || p.starts_with("/opt/zoom")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_told_it_is_on_gnome() {
        let mut cmd = std::process::Command::new("zoom");
        apply(&mut cmd, OsStr::new("/usr/bin/zoom"));
        let set = cmd.get_envs().find(|(k, _)| *k == "XDG_CURRENT_DESKTOP").and_then(|(_, v)| v.map(|v| v.to_string_lossy().into_owned()));
        assert_eq!(set.as_deref(), Some("GNOME"));
        let mut other = std::process::Command::new("gedit");
        apply(&mut other, OsStr::new("gedit"));
        assert!(other.get_envs().next().is_none());
    }
}
