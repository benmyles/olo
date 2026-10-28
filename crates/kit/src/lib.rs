//! Olo's Interface Kit.
//!
//! Apps implement [`App`] and call [`run`]. The kit owns the Wayland
//! connection, draws each window with egui on the GPU (wgpu/Vulkan), renders
//! the BeOS menu bar and popup menus, and keeps every window in step with
//! the desktop's light/dark theme.

mod app;
pub mod brand;
pub mod commands;
pub mod dnd;
pub mod emoji;
pub use olo_xdg::{apps, desktop_entry, legacy, mime, open, themes, trash, wallpapers};
pub mod file_panel;
pub mod files;
pub mod icons;
pub mod layout_lint;
/// Decoding pictures (see the `olo-images` crate).
pub use olo_images as images;
mod input;
pub mod instance;
pub mod menu;
pub mod metrics;
pub mod notify;
mod platform;
pub mod settings;
pub mod style;
pub mod text_drag;
mod theme_watch;
pub mod widgets;

pub use app::{App, Clip, Command, Cx, Keyboard, Layer, LayerOptions, ActivationToken, Proxy, SessionLock, WindowId, WindowKind, WindowOptions};
pub use egui;
pub use menu::{Action, Menu, MenuItem};
pub use platform::run;
pub use olo_theme as theme;

pub use olo_xdg::{config_dir, find_program, spawn_detached, state_dir, user_dir};
