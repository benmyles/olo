//! Olo's Interface Kit on Olo's GPUI (gpui/): the kit apps move to from
//! the egui one (`olo-kit`), app by app (TODO, "The kit").
//!
//! An app calls [`run`] with its id and builds its windows with
//! [`open_window`]: each is the app's GPUI view inside a [`Frame`], which
//! paints the panel behind it, sets Olo's font and follows the desktop's
//! look ([`theme`]). Widgets draw in Olo's style ([`style`], on device
//! pixels at every scale) and say what they are to agents ([`agent`]),
//! who read them, and each window's frame times, through the same socket
//! as the egui kit's apps (`olo_kit_base::commands`).

pub mod agent;
mod app;
mod lock;
pub mod icons;
pub mod menu;
pub mod review;
pub mod style;
pub mod theme;
mod upgrade;
pub mod widgets;

pub use app::{Frame, WindowOptions, app_id, message_handler, open_window, proxy, run, stay_resident};
pub use lock::{lock_session, unlock_session};
pub use menu::{Menu, MenuItem};
pub use upgrade::{on_upgrade, restored_state};
pub use gpui;
pub use olo_kit_base::{Proxy, commands, files, instance, metrics, notify, places, settings, tracker, watch};
pub use olo_theme as palette;
pub use olo_xdg::{config_dir, state_dir};
