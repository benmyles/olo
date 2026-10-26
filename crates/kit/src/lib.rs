//! Tabula's Interface Kit.
//!
//! Apps implement [`App`] and call [`run`]. The kit owns the Wayland
//! connection, draws each window with egui on the GPU (wgpu/Vulkan), renders
//! the BeOS menu bar and popup menus, and keeps every window in step with
//! the desktop's light/dark theme.

mod app;
pub mod emoji;
pub use tabula_xdg::{apps, desktop_entry, mime, open};
pub mod file_panel;
pub mod files;
pub mod icons;
/// Decoding pictures (see the `tabula-images` crate).
pub use tabula_images as images;
mod input;
pub mod instance;
pub mod menu;
pub mod metrics;
mod platform;
pub mod style;
mod theme_watch;
pub mod widgets;

pub use app::{App, Clip, Command, Cx, Keyboard, Layer, LayerOptions, Proxy, SessionLock, WindowId, WindowKind, WindowOptions};
pub use egui;
pub use menu::{Action, Menu, MenuItem};
pub use platform::run;
pub use tabula_theme as theme;

pub use tabula_xdg::{config_dir, find_program, spawn_detached, user_dir};
