#![warn(rust_2018_idioms)]
#![allow(clippy::collapsible_match)]
// If no backend is enabled, a large portion of the codebase is unused.
// So silence this useless warning for the CI.
#![cfg_attr(
    not(any(feature = "winit", feature = "udev")),
    allow(dead_code, unused_imports)
)]

#[cfg(any(feature = "udev", feature = "xwayland"))]
pub mod cursor;
pub mod drawing;
pub mod focus;
pub mod input_handler;
#[cfg(feature = "libei")]
pub mod libei;
pub mod render;
pub mod shell;
pub mod state;
#[cfg(feature = "udev")]
pub mod udev;
#[cfg(feature = "udev")]
pub mod headless;
#[cfg(feature = "winit")]
pub mod winit;

pub mod desktop;
pub mod ipc;
pub mod layout;
pub mod text;
pub mod theme;

pub use state::{AnvilState, ClientState};
