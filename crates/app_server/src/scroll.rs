//! The same scroll goes the same distance in every program.
//!
//! Olo's apps scroll a touchpad's axis values as logical pixels and a
//! wheel notch 48 of them. Chromium (Chrome, Chromium and Electron apps)
//! scrolls a touchpad's values twelve times as far and a notch 120
//! pixels; GTK 4 a touchpad's values two and a half times as far (a notch
//! the same 48). Their scrolls are scaled here so they match (measured
//! with a ruler page and a numbered file; see HACKING.md, "Scrolling").

use std::{collections::HashMap, path::Path, sync::Mutex};

use smithay::{backend::input::AxisSource, input::pointer::AxisFrame};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Toolkit {
    /// Scrolls the values as given (Olo's kit, and the default).
    AsGiven,
    Chromium,
    Gtk4,
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
            Some(dir) if is_chromium(dir) => Toolkit::Chromium,
            _ if maps(pid, "/libgtk-4.so") => Toolkit::Gtk4,
            _ => Toolkit::AsGiven,
        }
    })
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

/// `frame` as `toolkit` should get it.
pub fn adjust(mut frame: AxisFrame, toolkit: Toolkit) -> AxisFrame {
    let scale = match (toolkit, frame.source) {
        (Toolkit::AsGiven, _) => return frame,
        (Toolkit::Chromium, Some(AxisSource::Wheel | AxisSource::WheelTilt)) => 48.0 / 120.0,
        (Toolkit::Chromium, _) => 1.0 / 12.0,
        (Toolkit::Gtk4, Some(AxisSource::Wheel | AxisSource::WheelTilt)) => return frame,
        (Toolkit::Gtk4, _) => 1.0 / 2.5,
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
