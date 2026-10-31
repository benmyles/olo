//! The same scroll goes the same distance in every program.
//!
//! Olo's apps scroll a touchpad's axis values as logical pixels and a
//! wheel notch 48 of them. Chromium (Chrome, Chromium and Electron apps)
//! scrolls a touchpad's values twelve times as far and a notch 120
//! pixels; GTK 4 a touchpad's values two and a half times as far (a notch
//! the same 48); GTK 3 a tenth of either value times its scrolled area's
//! height to the power ⅔ (`get_scroll_unit`), which app_server knows only
//! as the window's. Their scrolls are scaled here so they match (measured
//! with a ruler page and numbered lines; see HACKING.md, "Scrolling").

use std::{collections::HashMap, path::Path, sync::Mutex, time::Duration};

use smithay::{backend::input::AxisSource, input::pointer::AxisFrame};

/// Chromium starts a fling of its own when a finger scroll stops within
/// 200 ms of its last move (`kFlingStartTimeoutMs`); this, with room for
/// it to read them late.
pub const CHROMIUM_FLINGS_WITHIN: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Toolkit {
    /// Scrolls the values as given (the default).
    AsGiven,
    /// Olo's kit: as given, and it takes a glide as one (`pace.rs`).
    Olo,
    Chromium,
    Gtk4,
    Gtk3,
}

/// What draws `pid`'s windows, as far as scrolling goes (remembered per
/// process; a pid is reused only after its process is gone).
pub fn toolkit(pid: i32) -> Toolkit {
    static SEEN: Mutex<Option<HashMap<i32, Toolkit>>> = Mutex::new(None);
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    let seen = seen.get_or_insert_with(HashMap::new);
    if seen.len() > 512 {
        seen.clear();
    }
    *seen.entry(pid).or_insert_with(|| {
        let exe = std::fs::read_link(format!("/proc/{pid}/exe")).ok();
        match exe.as_deref().and_then(Path::parent) {
            // Before the socket: olo-chrome answers on one named after
            // Chrome's pid too (its page trees).
            Some(dir) if is_chromium(dir) => Toolkit::Chromium,
            // Every kit app answers on a socket named after its pid
            // (olo_kit::commands).
            _ if runtime_dir().join(format!("olo-menus-{pid}.sock")).exists() => Toolkit::Olo,
            _ if maps(pid, "/libgtk-4.so") => Toolkit::Gtk4,
            _ if maps(pid, "/libgtk-3.so") => Toolkit::Gtk3,
            _ => Toolkit::AsGiven,
        }
    })
}

fn runtime_dir() -> std::path::PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR").map_or_else(|| format!("/run/user/{}", unsafe { libc::getuid() }).into(), Into::into)
}

/// Chromium's builds keep these beside the program (Chrome, Chromium,
/// Electron apps alike).
fn is_chromium(dir: &Path) -> bool {
    dir.join("icudtl.dat").exists() && (dir.join("v8_context_snapshot.bin").exists() || dir.join("snapshot_blob.bin").exists())
}

/// Whether `pid` has a library whose path contains `lib` loaded.
fn maps(pid: i32, lib: &str) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/maps")).is_ok_and(|m| m.contains(lib))
}

/// `frame` as `toolkit` should get it, in a window `height` logical
/// pixels high (what GTK 3 goes by).
pub fn adjust(mut frame: AxisFrame, toolkit: Toolkit, height: Option<i32>) -> AxisFrame {
    let wheel = matches!(frame.source, Some(AxisSource::Wheel | AxisSource::WheelTilt));
    let scale = match (toolkit, wheel) {
        (Toolkit::AsGiven | Toolkit::Olo, _) => return frame,
        (Toolkit::Chromium, true) => 48.0 / 120.0,
        (Toolkit::Chromium, false) => 1.0 / 12.0,
        (Toolkit::Gtk4, true) => return frame,
        (Toolkit::Gtk4, false) => 1.0 / 2.5,
        (Toolkit::Gtk3, _) => {
            // It goes value / 10 × page^⅔: a finger's values as they are,
            // a notch (15° of value) 48. Its value, not its notches, is
            // what it reads.
            let unit = (height.unwrap_or(600).max(50) as f64).powf(2.0 / 3.0) / 10.0;
            let scale = if wheel { 48.0 / 15.0 / unit } else { 1.0 / unit };
            frame.axis = (frame.axis.0 * scale, frame.axis.1 * scale);
            return frame;
        }
    };
    frame.axis = (frame.axis.0 * scale, frame.axis.1 * scale);
    frame.v120 = frame.v120.map(|(h, v)| ((h as f64 * scale).round() as i32, (v as f64 * scale).round() as i32));
    frame
}

#[cfg(test)]
mod tests {
    use super::*;
    use smithay::backend::input::{Axis, InputTime};

    #[test]
    fn chromium_matches_olo() {
        let finger = AxisFrame::new(InputTime::now()).source(AxisSource::Finger).value(Axis::Vertical, 24.0);
        assert_eq!(adjust(finger, Toolkit::Chromium).axis.1, 2.0);
        assert_eq!(adjust(finger, Toolkit::AsGiven).axis.1, 24.0);
        let wheel = AxisFrame::new(InputTime::now()).source(AxisSource::Wheel).value(Axis::Vertical, 15.0).v120(Axis::Vertical, 120);
        assert_eq!(adjust(wheel, Toolkit::Chromium).v120, Some((0, 48)));
    }
}
