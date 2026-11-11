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
pub mod file_panel;
pub mod icons;
mod kept;
pub mod keys;
mod lock;
pub mod menu;
pub mod open_with;
pub mod pictures;
pub mod review;
pub mod style;
pub mod theme;
pub mod tip;
pub mod undo;
mod upgrade;
pub mod widgets;

pub use app::{
    FIRST_SHOW_WAIT, FirstShowHold, Frame, WindowOptions, app_id, begin_move, hold_first_show, message_handler, on_displays_changed, open_or_raise, open_window, open_window_here, proxy, raise, run,
    screen_room, set_content_height, stay_resident, tab_step,
};
pub use lock::{lock_session, unlock_session};
pub use menu::{Menu, MenuBar, MenuItem};
pub use olo_app as app_kit;
pub use olo_interface::*;
// (`clipboard` is the Interface Kit's: what a program with windows copies and
// pastes through. One with none holds the clipboard itself: `held_clipboard`.)
pub use olo_kit_base::clipboard as held_clipboard;
pub use olo_kit_base::{Proxy, clock, commands, files, instance, metrics, moving, notify, places, restore, settings, store, tracker, watch};
pub use olo_theme as palette;
pub use olo_xdg::{config_dir, state_dir};
pub use upgrade::{on_upgrade, on_upgrade_due, restored_state, upgrade_waits_while};
