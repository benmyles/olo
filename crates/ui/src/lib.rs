//! Olo's Interface Kit on Olo's GPUI (gpui/): what every Olo app is
//! built on.
//!
//! An app calls [`run`] with its id and builds its windows with
//! [`open_window`]: each is the app's GPUI view inside a [`Frame`], which
//! paints the panel behind it, sets Olo's font and follows the desktop's
//! look ([`theme`]). Widgets draw in Olo's style ([`style`], on device
//! pixels at every scale) and say what they are to agents ([`agent`]),
//! who read them, and each window's frame times, through the socket
//! every app answers on (`olo_kit_base::commands`).

pub mod agent;
mod app;
pub mod brand;
pub mod copy;
pub mod cut;
pub mod dnd;
pub mod editor;
pub mod file_panel;
mod lock;
pub mod icons;
pub mod keys;
#[cfg(feature = "lint")]
pub mod lint;
pub mod menu;
pub mod open_with;
pub mod part;
pub mod pictures;
pub mod review;
pub mod search;
pub mod spell;
pub mod spotlight;
pub mod style;
pub mod theme;
pub mod tip;
mod upgrade;
pub mod widgets;
pub mod workspaces;

pub use app::{on_displays_changed, FocusNext, FocusPrev, Frame, WindowOptions, app_id, begin_move, message_handler, open_window, proxy, run, stay_resident, tab_step};
pub use lock::{lock_session, unlock_session};
pub use menu::{Menu, MenuBar, MenuItem};
pub use upgrade::{on_upgrade, on_upgrade_due, restored_state, upgrade_waits_while};
pub use gpui;
pub use olo_kit_base::{Proxy, clipboard, commands, files, instance, metrics, notify, places, settings, tracker, watch};
pub use olo_theme as palette;
pub use olo_xdg::{config_dir, state_dir};
