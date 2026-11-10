//! Olo's kit: what every Olo app is built on.
//!
//! The Interface Kit ([`olo_interface`], re-exported whole: windows on
//! loopers of their own, views, elements, drawing) with Olo's look
//! ([`theme`], [`style`]: the palette, and the BeOS drawing primitives
//! on device pixels at every scale), the BeOS controls, and what every
//! app does the same way.

pub mod agent;
mod app;
pub mod cut;
pub mod icons;
pub mod review;
pub mod style;
pub mod theme;
pub mod tip;
pub mod widgets;

pub use app::{Frame, WindowOptions, open_window, run};
pub use olo_app as app_kit;
pub use olo_interface::*;
pub use olo_kit_base::{Proxy, clipboard, clock, commands, files, instance, metrics, moving, notify, places, restore, settings, store, tracker, watch};
pub use olo_theme as palette;
pub use olo_xdg::{config_dir, state_dir};
