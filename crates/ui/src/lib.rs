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
pub mod code;
pub mod copy;
pub mod cut;
pub mod dnd;
pub mod due;
pub mod editor;
pub mod file_panel;
pub mod icons;
mod kept;
pub mod keys;
#[cfg(feature = "lint")]
pub mod lint;
mod lock;
pub mod menu;
pub mod open_with;
pub mod part;
pub mod pictures;
pub mod review;
pub mod rich_text;
pub mod search;
pub mod spell;
pub mod spotlight;
pub mod style;
pub mod theme;
pub mod tip;
pub mod undo;
mod upgrade;
pub mod widgets;
pub mod workspaces;

pub use app::{
    FIRST_SHOW_WAIT, FirstShowHold, FocusNext, FocusPrev, Frame, WindowOptions, app_id, begin_move, hold_first_show, message_handler, on_displays_changed, open_or_raise, open_window, proxy, raise,
    run, screen_room, set_content_height, stay_resident, tab_step,
};
pub use gpui;
pub use lock::{lock_session, unlock_session};
pub use menu::{Menu, MenuBar, MenuItem};
pub use olo_kit_base::{Proxy, clipboard, clock, commands, files, instance, metrics, moving, notify, places, restore, settings, store, tracker, watch};
pub use olo_theme as palette;
pub use olo_xdg::{config_dir, state_dir};
pub use upgrade::{WindowMove, on_move_window, on_upgrade, on_upgrade_due, refuse_moves_while, restored_state, upgrade_waits_while};
