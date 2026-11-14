//! AirPlay in an app's menus: the receivers paired with, for Play On ▸
//! and Show on AirPlay ▸ (`Menu::airplay`), as the pairings file says
//! them. Read once, then followed (inotify, the app's one watcher): a
//! menu built each time the window draws costs nothing more, and a
//! receiver paired with in Preferences shows in every app's menus the
//! next time they're drawn.

use std::sync::{Arc, Mutex, OnceLock};

use olo_protocol::airplay::{self as ap, Paired};

/// The list, and the watch that keeps it (the app's for as long as it
/// runs).
type Followed = (Arc<Mutex<Vec<Paired>>>, Option<crate::watch::Watch>);

/// The receivers paired with, by name.
pub fn paired() -> Vec<Paired> {
    static PAIRED: OnceLock<Followed> = OnceLock::new();
    let (list, _) = PAIRED.get_or_init(|| {
        let list = Arc::new(Mutex::new(ap::paired()));
        let into = list.clone();
        let watch = crate::watch::watch([ap::pairings_path()], move |_| {
            *into.lock().unwrap_or_else(|e| e.into_inner()) = ap::paired();
        });
        (list, watch)
    });
    list.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Do what a menu's AirPlay item says (`Menu::airplay`'s commands, with
/// any prefix taken off): play `what` (a file, or an address on the web)
/// on the receiver it names, or open AirPlay's preferences. False if
/// `command` isn't one of these.
pub fn play_from_menu(command: &str, what: &str) -> bool {
    match ap::menu_command(command) {
        Some(Some(receiver)) => {
            if let Err(e) = ap::play_on(receiver, what) {
                tracing::warn!("couldn't play {what} on AirPlay: {e}");
            }
            true
        }
        Some(None) => {
            let _ = ap::open_settings();
            true
        }
        None => false,
    }
}

/// The same for a screen shown live (a window, a display, a workspace).
pub fn show_from_menu(command: &str, what: &ap::Shown) -> bool {
    match ap::menu_command(command) {
        Some(Some(receiver)) => {
            if let Err(e) = ap::show_on(receiver, what) {
                tracing::warn!("couldn't show it on AirPlay: {e}");
            }
            true
        }
        Some(None) => {
            let _ = ap::open_settings();
            true
        }
        None => false,
    }
}
