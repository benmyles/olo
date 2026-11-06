//! A screen recording going (olo-screenshot's recorder, recording.rs):
//! kept in `$XDG_RUNTIME_DIR/olo-recording-<display>.json` while it
//! runs, one a display (an Agent Screen's and the person's apart), for
//! `--stop`, `--pause` and `--resume`, Deskbar's tray (a dot while
//! recording, Pause and Stop Recording) and agents.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// What's being recorded on a display.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Going {
    pub pid: u32,
    pub path: PathBuf,
    /// Seconds since the epoch.
    pub started: u64,
    /// What's recorded: "display", "region" or "window N".
    pub what: String,
    /// Paused (`--pause`): nothing recorded till `--resume`, the time
    /// paused left out of the video.
    #[serde(default)]
    pub paused: bool,
    /// Its clock has started (the encoder is open: NVENC takes a couple
    /// of seconds); before, nothing is recorded yet.
    #[serde(default = "yes")]
    pub recording: bool,
}

fn yes() -> bool {
    true
}

/// The display a program is on (`WAYLAND_DISPLAY`'s name).
pub fn this_display() -> String {
    let d = std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "wayland-0".into());
    // (A display given as a path: its file's name.)
    Path::new(&d).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or(d)
}

/// Where the recording going on `display` is said.
pub fn path_for(display: &str) -> PathBuf {
    let run = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/tmp"));
    run.join(format!("olo-recording-{display}.json"))
}

/// The recording going on `display`, if one is (its process alive).
pub fn going_on(display: &str) -> Option<Going> {
    let g: Going = serde_json::from_slice(&std::fs::read(path_for(display)).ok()?).ok()?;
    Path::new(&format!("/proc/{}", g.pid)).exists().then_some(g)
}

/// The recording going on this program's display.
pub fn going() -> Option<Going> {
    going_on(&this_display())
}
