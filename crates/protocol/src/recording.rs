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
    /// How much it had recorded (milliseconds, pauses left out) when
    /// `as_of` (milliseconds since the epoch) was: its clock, for the
    /// controls on the screen (`recorded_ms`).
    #[serde(default)]
    pub recorded_ms: u64,
    #[serde(default)]
    pub as_of: u64,
    /// The recorder's own arguments: what Restart starts again.
    #[serde(default)]
    pub args: Vec<String>,
}

impl Going {
    /// How long it has recorded by now (milliseconds), pauses left out.
    pub fn recorded_now(&self) -> u64 {
        if !self.recording || self.paused || self.as_of == 0 {
            return self.recorded_ms;
        }
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
        self.recorded_ms + now.saturating_sub(self.as_of)
    }
}

/// The namespace of the recording's controls on the screen (a layer
/// surface): app_server leaves them out of every capture, so they're on
/// the screen but never in the recording (or a share).
pub const CONTROLS_NAMESPACE: &str = "olo-recording-controls";

/// Beside the status file while a recording is being discarded
/// (`--discard`): the recorder, finishing, deletes what it made.
pub fn discard_path_for(display: &str) -> PathBuf {
    path_for(display).with_extension("discard")
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
