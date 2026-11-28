//! Olo's kit: what every Olo app is built on.
//!
//! The Interface Kit ([`olo_interface`], re-exported whole: windows on
//! loopers of their own, views, elements, drawing) with Olo's look
//! ([`theme`], [`style`]: the palette, and the BeOS drawing primitives
//! on device pixels at every scale), the BeOS controls, and what every
//! app does the same way.

pub mod agent;
pub mod ago;
mod app;
pub mod app_menu;
pub mod brand;
pub mod card;
pub mod chosen;
pub mod code;
pub mod copy;
pub mod cut;
pub mod dnd;
pub mod due;
pub mod editor;
pub mod file_panel;
pub mod glance;
pub mod go_to;
pub mod history;
pub mod icons;
mod kept;
pub mod keys;
pub mod links;
#[cfg(feature = "lint")]
pub mod lint;
mod lock;
pub mod menu;
pub mod open_with;
pub mod para;
pub mod term_keys;
pub mod unread;

/// An emoji the person picks, for an app (a reaction): Olo's emoji
/// picker shown over everything for it (`olo-emoji-picker --pick`), its
/// choice read back. None if it's put away, or can't start. Waits for
/// the person: call it off the UI thread (`Cx::work`).
pub fn pick_emoji() -> Option<String> {
    let out = std::process::Command::new(olo_xdg::find_program("olo-emoji-picker").unwrap_or_else(|| "olo-emoji-picker".into()))
        .arg("--pick")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let picked = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    (!picked.is_empty()).then_some(picked)
}
pub mod person;
pub mod pictures;
pub mod review;
pub mod search;
pub mod spell;
pub mod spotlight;
pub mod style;
pub mod theme;
pub mod tip;
pub mod underline;
pub mod undo;
mod upgrade;
pub mod when;
pub mod widgets;
pub mod words;

pub use app::{
    FIRST_SHOW_WAIT, FirstShowHold, Frame, SMALLEST_SCREEN_HEIGHT, SMALLEST_SCREEN_WIDTH, WindowOptions, app_id, begin_move, display_height, hold_first_show, message_handler, on_displays_changed,
    open_or_raise, open_window, open_window_here, proxy, raise, run, screen_room, screen_room_across, set_content_height, stay_resident, tab_step,
};
pub use lock::{lock_session, unlock_session};
pub use menu::{Menu, MenuBar, MenuItem};
pub use olo_app as app_kit;
pub use olo_interface::*;
// (`clipboard` is the Interface Kit's: what a program with windows copies and
// pastes through. One with none holds the clipboard itself: `held_clipboard`.)
pub use olo_kit_base::clipboard as held_clipboard;
pub use olo_kit_base::{Proxy, airplay, clock, commands, files, instance, metrics, moving, notify, places, restore, settings, store, tracker, watch, workspaces};
/// A sentence in the language chosen with its parts put in:
/// `olo_kit::tr!("Updated {} ago", when)` (the kit's words, `words`);
/// one for another program to show, kept in English: `olo_kit::en!`.
pub use olo_kit_base::{en, tr};
pub use olo_theme as palette;
pub use olo_xdg::{config_dir, state_dir};
pub use upgrade::{WindowMove, on_move_window, on_upgrade, on_upgrade_due, refuse_moves_while, restored_state, upgrade_waits_while};
