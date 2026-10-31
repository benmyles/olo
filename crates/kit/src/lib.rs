//! Olo's Interface Kit.
//!
//! Apps implement [`App`] and call [`run`]. The kit owns the Wayland
//! connection, draws each window with egui on the GPU (wgpu/Vulkan), renders
//! the BeOS menu bar and popup menus, and keeps every window in step with
//! the desktop's light/dark theme.

mod app;
pub mod big_text;
pub mod brand;
pub mod clipboard;
pub mod commands;
pub mod dnd;
pub mod editor;
pub mod emoji;
pub use olo_xdg::{apps, desktop_entry, legacy, mime, open, themes, trash, wallpapers};
pub mod file_panel;
pub mod files;
pub mod dashboard;
pub mod graph;
pub mod icons;
pub mod layout_lint;
pub mod layout_review;
pub mod masonry;
/// Decoding pictures (see the `olo-images` crate).
pub use olo_images as images;
mod input;
pub mod instance;
pub mod keyboard;
pub mod menu;
pub mod metrics;
pub mod notify;
pub mod places;
mod platform;
pub mod recorded;
pub mod search;
pub mod settings;
pub mod sidebar;
pub mod spell;
pub mod style;
pub mod textures;
pub mod text_drag;
pub mod tip;
pub mod tracker;
pub mod watch;
mod theme_watch;
pub mod widgets;
mod workspaces;

pub use app::{App, Clip, Command, Cx, Keyboard, Layer, LayerOptions, ActivationToken, Pasted, Proxy, SessionLock, WindowId, WindowKind, WindowOptions};
pub use egui;
pub use menu::{Action, Menu, MenuItem};
pub use platform::run;
pub use olo_theme as theme;

pub use olo_xdg::{config_dir, display_name, find_program, spawn_detached, spawn_detached_app, state_dir, upgrade_env, user_dir};
