//! The private IPC between `app_server` and Tabula's own apps (Deskbar,
//! Preferences, ...). Plain Wayland clients never need it.
//!
//! Transport: newline-delimited JSON over the Unix socket named by
//! `$TABULA_SOCKET` (set by app_server for every child it starts). Each line a
//! client writes is a [`Request`]; app_server answers each with exactly one
//! [`Reply`] line. After [`Request::Subscribe`] the connection also receives
//! unsolicited [`Reply::Event`] lines.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub const SOCKET_ENV: &str = "TABULA_SOCKET";

/// Opaque, stable for the lifetime of a window.
pub type WindowId = u64;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    Ping,
    /// Start receiving [`Event`]s on this connection.
    Subscribe,
    ListWindows,
    Activate { id: WindowId },
    /// BeOS "minimize": hide the window; Deskbar shows it greyed out.
    Minimize { id: WindowId },
    Close { id: WindowId },
    ToggleZoom { id: WindowId },
    Workspaces,
    SwitchWorkspace { index: u32 },
    MoveToWorkspace { id: WindowId, index: u32 },
    Outputs,
    ConfigureOutput { name: String, config: OutputConfig },
    GetSettings,
    SetSettings { settings: Settings },
    /// Save a PNG of everything on screen (all outputs, composited).
    Screenshot { path: String },
    Quit,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Reply {
    Ok,
    Pong { version: String },
    Error { message: String },
    Windows { windows: Vec<WindowInfo> },
    Workspaces { current: u32, count: u32 },
    Outputs { outputs: Vec<OutputInfo> },
    Settings { settings: Settings },
    Event { event: Event },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    WindowsChanged { windows: Vec<WindowInfo> },
    WorkspaceChanged { current: u32, count: u32 },
    SettingsChanged { settings: Settings },
    OutputsChanged { outputs: Vec<OutputInfo> },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WindowInfo {
    pub id: WindowId,
    pub app_id: String,
    pub title: String,
    pub pid: Option<i32>,
    pub workspace: u32,
    pub minimized: bool,
    pub focused: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Mode {
    pub width: i32,
    pub height: i32,
    /// millihertz
    pub refresh: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OutputInfo {
    pub name: String,
    pub make: String,
    pub model: String,
    pub physical_size_mm: (i32, i32),
    pub modes: Vec<Mode>,
    pub current_mode: Option<Mode>,
    pub scale: f64,
    pub position: (i32, i32),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct OutputConfig {
    pub mode: Option<Mode>,
    pub scale: Option<f64>,
    pub position: Option<(i32, i32)>,
}

/// Everything Preferences can change live in app_server. Persisted by
/// app_server in `$XDG_CONFIG_HOME/tabula/app_server.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub workspace_count: u32,
    /// Per-workspace desktop colour, BeOS-style (Backgrounds/Workspaces prefs).
    pub workspace_colors: Vec<[u8; 3]>,
    /// Optional wallpaper image drawn by Tracker's desktop window.
    pub wallpaper: Option<String>,
    pub focus_follows_mouse: bool,
    pub touchpad: TouchpadSettings,
    pub keyboard: KeyboardSettings,
    pub mouse_speed: f64,
    /// Font size (points) used for window tab titles.
    pub tab_font_size: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct TouchpadSettings {
    pub tap_to_click: bool,
    pub tap_and_drag: bool,
    pub natural_scroll: bool,
    pub disable_while_typing: bool,
    pub clickfinger: bool,
    /// libinput accel speed, -1.0 ..= 1.0
    pub speed: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct KeyboardSettings {
    pub layout: String,
    pub variant: String,
    pub options: String,
    pub repeat_delay_ms: i32,
    pub repeat_rate_hz: i32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            workspace_count: 4,
            // BeOS R5's default desktop blue, then a few Haiku-ish alternates.
            workspace_colors: vec![[51, 102, 152], [0, 128, 128], [102, 51, 153], [96, 96, 96]],
            wallpaper: None,
            focus_follows_mouse: false,
            touchpad: TouchpadSettings::default(),
            keyboard: KeyboardSettings::default(),
            mouse_speed: 0.0,
            tab_font_size: 9.0,
        }
    }
}

impl Default for TouchpadSettings {
    fn default() -> Self {
        TouchpadSettings {
            tap_to_click: true,
            tap_and_drag: false,
            natural_scroll: false,
            disable_while_typing: true,
            clickfinger: true,
            speed: 0.0,
        }
    }
}

impl Default for KeyboardSettings {
    fn default() -> Self {
        KeyboardSettings {
            layout: "us".into(),
            variant: String::new(),
            options: String::new(),
            repeat_delay_ms: 300,
            repeat_rate_hz: 35,
        }
    }
}

impl Settings {
    pub fn workspace_color(&self, index: u32) -> [u8; 3] {
        self.workspace_colors
            .get(index as usize)
            .or_else(|| self.workspace_colors.first())
            .copied()
            .unwrap_or([51, 102, 152])
    }
}

pub fn socket_path() -> Option<PathBuf> {
    std::env::var_os(SOCKET_ENV).map(PathBuf::from)
}

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("{SOCKET_ENV} is not set; not running under Tabula's app_server")]
    NoSocket,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("app_server: {0}")]
    Server(String),
    #[error("app_server closed the connection")]
    Closed,
}

/// A blocking connection to app_server.
pub struct Client {
    writer: UnixStream,
    reader: BufReader<UnixStream>,
}

impl Client {
    pub fn connect() -> Result<Self, ClientError> {
        let path = socket_path().ok_or(ClientError::NoSocket)?;
        let stream = UnixStream::connect(path)?;
        Ok(Client { reader: BufReader::new(stream.try_clone()?), writer: stream })
    }

    pub fn request(&mut self, req: &Request) -> Result<Reply, ClientError> {
        self.send(req)?;
        loop {
            match self.read()? {
                // Events can interleave with replies on a subscribed connection.
                Reply::Event { .. } => continue,
                Reply::Error { message } => return Err(ClientError::Server(message)),
                reply => return Ok(reply),
            }
        }
    }

    pub fn send(&mut self, req: &Request) -> Result<(), ClientError> {
        let mut line = serde_json::to_vec(req)?;
        line.push(b'\n');
        self.writer.write_all(&line)?;
        Ok(())
    }

    /// Read the next line (a reply or, when subscribed, an event).
    pub fn read(&mut self) -> Result<Reply, ClientError> {
        let mut line = String::new();
        if self.reader.read_line(&mut line)? == 0 {
            return Err(ClientError::Closed);
        }
        Ok(serde_json::from_str(&line)?)
    }

    /// Consume this client as an event stream (after sending Subscribe).
    pub fn into_events(mut self) -> Result<impl Iterator<Item = Result<Event, ClientError>>, ClientError> {
        match self.request(&Request::Subscribe)? {
            Reply::Ok => {}
            other => return Err(ClientError::Server(format!("unexpected reply {other:?}"))),
        }
        Ok(std::iter::from_fn(move || match self.read() {
            Ok(Reply::Event { event }) => Some(Ok(event)),
            Ok(_) => None,
            Err(ClientError::Closed) => None,
            Err(e) => Some(Err(e)),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_wire_format_is_tagged() {
        let s = serde_json::to_string(&Request::Activate { id: 7 }).unwrap();
        assert_eq!(s, r#"{"type":"activate","id":7}"#);
        let back: Request = serde_json::from_str(&s).unwrap();
        assert_eq!(back, Request::Activate { id: 7 });
    }

    #[test]
    fn settings_tolerate_missing_fields() {
        let s: Settings = serde_json::from_str(r#"{"workspace_count": 8}"#).unwrap();
        assert_eq!(s.workspace_count, 8);
        assert!(s.touchpad.tap_to_click);
    }
}
