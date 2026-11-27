//! Shared Agent Screen pane, source abstraction, and screen backends.
//!
//! Embedded by `apps/screen` (Agent Screens) and designed for `apps/rex`
//! (Rex's Activity sidebar) as its second caller so every local and remote Rex
//! host screen shares one Live / History / Stats / Passwords UI.

mod clip;
pub mod export;
pub mod history;
pub mod local;
pub mod moving;
pub mod pane;
pub mod paths;
mod picture;
pub mod record;
mod replay;
pub mod rex_client;
pub mod rex_sock;
pub mod screens;
pub mod shares;
pub mod source;
mod stats;
pub mod stream;

pub use local::LocalScreenSource;
pub use olo_screen_codec::{EncodedDelta, Frame, FrameIndex, FrameMark, TileEncoding, TileRect};
pub use pane::{HEADER_H, ScreenMenuState, ScreenPane, ScreenPaneEvent, Tab, capture_stop_expectations, header_bar};
pub use picture::Shot;
pub use rex_client::{OpenHostError, RexSockClient, RexSockOpener};
pub use shares::Padded;
pub use source::{
    ExportCancel, HistoryEvent, HistoryHandle, LiveEvent, LiveHandle, LiveStatus, NoopHandle, PaneCapabilities, PasswordsMode, ScreenId, ScreenInfo, ScreenLocation, ScreenSource, ScreenState,
};
pub use stream::{CommandOutput, HostCapabilities, HostProtocolCache, LiveStream, ReconnectPolicy, StreamOpener, StreamScreenSource};
