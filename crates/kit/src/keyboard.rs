//! The keyboard profile app_server uses (Preferences ▸ Keyboard: Mac,
//! Windows, BeOS, or one changed from them), as apps need it: a menu
//! item's keys come from the profile ([`olo_protocol::app_keys`], by its
//! command), shown as the platform writes them ("⇧⌘Z", "Ctrl+Y",
//! "Alt+Shift+Z"), and pressed as the app sees them (the command key is
//! always Ctrl to an app). Apps that take a command's keys outside a menu
//! ask [`pressed`] / [`consume`] rather than naming keys.
//!
//! Outside Olo the profile is a Mac's.

use std::{
    collections::HashMap,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use egui::{Key, Modifiers};
use olo_protocol::{Chord, CommandKey, KeyboardProfile, Platform, app_keys, shortcuts};

use crate::menu::{Menu, MenuItem, Shortcut};

static PROFILE: Mutex<Option<KeyboardProfile>> = Mutex::new(None);
/// Bumped when the profile changes, so cached keys are made again.
static GENERATION: AtomicU64 = AtomicU64::new(0);
static COMMAND_KEY: AtomicBool = AtomicBool::new(false);
static APP_ID: OnceLock<String> = OnceLock::new();

pub(crate) fn set_app_id(app_id: &str) {
    let _ = APP_ID.set(app_id.to_owned());
}

fn app_id() -> &'static str {
    APP_ID.get().map_or("", String::as_str)
}

/// app_server's profile (from its settings), as it changes.
pub(crate) fn set(profile: &KeyboardProfile) {
    COMMAND_KEY.store(profile.command_key != CommandKey::Control, Ordering::Relaxed);
    let mut p = PROFILE.lock().unwrap_or_else(|e| e.into_inner());
    if p.as_ref() != Some(profile) {
        *p = Some(profile.clone());
        GENERATION.fetch_add(1, Ordering::Relaxed);
    }
}

/// The profile in use (a Mac's outside Olo).
pub fn profile() -> KeyboardProfile {
    PROFILE.lock().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_default()
}

pub fn platform() -> Platform {
    PROFILE.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map_or(Platform::Mac, |p| p.platform)
}

/// The command key is one app_server turns into Ctrl (⌘, or BeOS's Alt),
/// sending it to terminals as its right Ctrl with Shift and keeping the
/// left Ctrl for the physical key: in a terminal-style window only the
/// right one is the command key. False outside Olo, and with Windows'
/// keys, where Ctrl is the command key itself.
pub fn command_key() -> bool {
    COMMAND_KEY.load(Ordering::Relaxed)
}

/// A chord (physical keys, as profiles write them) as an app sees it: the
/// command key is Ctrl, and on BeOS the Super key (Option) is Alt. In a
/// terminal-style window (`raw`), the command key arrives with Shift. None
/// for one an app never sees (Windows' Super key).
pub fn app_side(c: &Chord, command: CommandKey, raw: bool) -> Option<Shortcut> {
    let key = egui_key(c.key)?;
    let (cmd, alt) = match command {
        CommandKey::Command => (c.cmd || c.ctrl, c.alt),
        CommandKey::Alt => (c.alt || c.ctrl, c.cmd),
        CommandKey::Control if c.cmd => return None,
        CommandKey::Control => (c.ctrl, c.alt),
    };
    // (A terminal gets the command key's chords with Shift.)
    let by_command_key = match command {
        CommandKey::Command => c.cmd,
        CommandKey::Alt => c.alt,
        CommandKey::Control => false,
    };
    let shift = c.shift || (raw && by_command_key && !c.ctrl);
    Some(Shortcut { key, shift, command: cmd, alt })
}

/// The physical chord an app's shortcut is pressed with, on a profile with
/// `command` as its command key (for showing it).
pub fn physical(s: &Shortcut, command: CommandKey) -> Chord {
    let key = evdev_key(s.key);
    let (ctrl, alt, cmd) = match command {
        CommandKey::Command => (false, s.alt, s.command),
        CommandKey::Alt => (false, s.command, s.alt),
        CommandKey::Control => (s.command, s.alt, false),
    };
    Chord { ctrl, alt, shift: s.shift, cmd, key }
}

/// A shortcut as the profile's platform writes it.
pub fn label(s: &Shortcut) -> String {
    let p = profile();
    physical(s, p.command_key).label(p.platform)
}

/// What the profile has for a command in this app.
#[derive(Clone)]
struct Known {
    /// The app's own entry, not every app's.
    own: bool,
    /// Its keys as the app presses them, and as the platform writes them
    /// (None: it has none).
    keys: Option<(Shortcut, String)>,
}

/// The keys `command` has in this app (as a terminal-style window, `raw`,
/// presses them): None if the profile doesn't know the command.
fn keys_of(command: &str, raw: bool) -> Option<Known> {
    type Cache = (u64, HashMap<(String, bool), Option<Known>>);
    static CACHE: Mutex<Option<Cache>> = Mutex::new(None);
    let generation = GENERATION.load(Ordering::Relaxed);
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if cache.as_ref().is_none_or(|(g, _)| *g != generation) {
        *cache = Some((generation, HashMap::new()));
    }
    let (_, map) = cache.as_mut()?;
    map.entry((command.to_owned(), raw))
        .or_insert_with(|| {
            let entry = app_keys::app_key(app_id(), command)?;
            let p = profile();
            let chords = shortcuts::entry(entry.id).map(|e| e.keys(&p)).unwrap_or_default();
            let keys = chords.iter().find_map(|c| Some((app_side(c, p.command_key, raw)?, c.label(p.platform))));
            Some(Known { own: entry.app.is_some(), keys })
        })
        .clone()
}

/// Give `menus`' items the keys their commands have in the profile, as the
/// platform writes them; a terminal-style window's (`raw`) as it presses
/// them. The kit does this to every menu bar and context menu. Every
/// app's keys (Copy, Close) go to items the app gave keys; the app's own
/// in the profile go to its items whether or not it did.
pub fn resolve(menus: &mut [Menu], raw: bool) {
    for m in menus {
        resolve_items(&mut m.items, raw);
    }
}

fn resolve_items(items: &mut [MenuItem], raw: bool) {
    for item in items {
        match item {
            MenuItem::Action(a) => match keys_of(&a.command, raw) {
                Some(known) if known.own || a.shortcut.is_some() => {
                    a.shortcut = known.keys.as_ref().map(|(s, _)| *s);
                    a.keys = known.keys.map(|(_, label)| label);
                }
                Some(_) => {}
                None => {
                    if let Some(s) = a.shortcut.filter(|_| a.keys.is_none()) {
                        a.keys = Some(label(&s));
                        if cfg!(debug_assertions) {
                            unlisted(&a.command, &s);
                        }
                    }
                }
            },
            MenuItem::Submenu(m) => resolve_items(&mut m.items, raw),
            MenuItem::Separator => {}
        }
    }
}

/// A menu item with keys the profile doesn't list: they can't be changed
/// in Preferences, nor shown in the shortcuts sheet. Said once.
fn unlisted(command: &str, s: &Shortcut) {
    static SAID: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let mut said = SAID.lock().unwrap_or_else(|e| e.into_inner());
    if !said.iter().any(|c| c == command) {
        said.push(command.to_owned());
        tracing::warn!(app = app_id(), command, keys = label(s), "menu keys not in olo_protocol::app_keys");
    }
}

/// The shortcut `command` has in this app (from the profile), if any.
pub fn shortcut(command: &str) -> Option<Shortcut> {
    keys_of(command, false)?.keys.map(|(s, _)| s)
}

/// `command`'s keys were pressed this frame (its shortcut in the profile,
/// for a command an app takes outside its menus).
pub fn pressed(i: &egui::InputState, command: &str) -> bool {
    let Some(s) = shortcut(command) else { return false };
    i.events.iter().any(|e| matches!(e, egui::Event::Key { key, pressed: true, modifiers, .. } if s.pressed_by(*key, *modifiers)))
}

/// As [`pressed`], taking the key press from the frame.
pub fn consume(i: &mut egui::InputState, command: &str) -> bool {
    let Some(s) = shortcut(command) else { return false };
    let mut mods = Modifiers::NONE;
    mods.command = s.command;
    mods.ctrl = s.command;
    mods.alt = s.alt;
    mods.shift = s.shift;
    i.consume_key(mods, s.key) || (s.key == Key::Equals && i.consume_key(mods, Key::Plus))
}

/// Whether `key` with `modifiers` is one of app `app`'s own shortcuts in
/// the profile (as a terminal-style window gets them, if `raw`): for an
/// app that sends every other key on (Terminal, to its shell). Its own
/// are the ones [`app_keys`] lists for it by name.
pub fn is_own_shortcut(app: &str, key: Key, modifiers: Modifiers, raw: bool) -> bool {
    let p = profile();
    app_keys::APP_KEYS.iter().filter(|e| e.app == Some(app)).any(|e| {
        let keys = shortcuts::entry(e.id).map(|x| x.keys(&p)).unwrap_or_default();
        keys.iter().filter_map(|c| app_side(c, p.command_key, raw)).any(|s| s.pressed_by(key, modifiers))
    })
}

/// egui's key for an evdev code (the keys a shortcut can have).
pub fn egui_key(code: u32) -> Option<Key> {
    use Key::*;
    const LETTERS: [(u32, Key); 26] = [
        (30, A), (48, B), (46, C), (32, D), (18, E), (33, F), (34, G), (35, H), (23, I), (36, J), (37, K), (38, L), (50, M),
        (49, N), (24, O), (25, P), (16, Q), (19, R), (31, S), (20, T), (22, U), (47, V), (17, W), (45, X), (21, Y), (44, Z),
    ];
    const OTHERS: [(u32, Key); 48] = [
        (1, Escape), (2, Num1), (3, Num2), (4, Num3), (5, Num4), (6, Num5), (7, Num6), (8, Num7), (9, Num8), (10, Num9),
        (11, Num0), (12, Minus), (13, Equals), (14, Backspace), (15, Tab), (26, OpenBracket), (27, CloseBracket),
        (28, Enter), (39, Semicolon), (40, Quote), (41, Backtick), (43, Backslash), (51, Comma), (52, Period),
        (53, Slash), (57, Space), (59, F1), (60, F2), (61, F3), (62, F4), (63, F5), (64, F6), (65, F7), (66, F8),
        (67, F9), (68, F10), (87, F11), (88, F12), (96, Enter), (102, Home), (103, ArrowUp), (104, PageUp),
        (105, ArrowLeft), (106, ArrowRight), (107, End), (108, ArrowDown), (109, PageDown), (111, Delete),
    ];
    LETTERS.iter().chain(OTHERS.iter()).find(|(c, _)| *c == code).map(|(_, k)| *k)
}

/// The evdev code for egui's key (0 if none).
pub fn evdev_key(key: Key) -> u32 {
    let key = if key == Key::Plus { Key::Equals } else { key };
    (1..=111).find(|&c| egui_key(c) == Some(key)).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_as_apps_see_them() {
        let c = |s: &str| s.parse::<Chord>().unwrap();
        let s = |key, shift, command, alt| Some(Shortcut { key, shift, command, alt });
        // ⌘C, Ctrl+C and BeOS's Alt+C are all Ctrl+C to the app.
        assert_eq!(app_side(&c("Cmd+C"), CommandKey::Command, false), s(Key::C, false, true, false));
        assert_eq!(app_side(&c("Ctrl+C"), CommandKey::Control, false), s(Key::C, false, true, false));
        assert_eq!(app_side(&c("Alt+C"), CommandKey::Alt, false), s(Key::C, false, true, false));
        // A terminal gets the command key's with Shift; Windows' as written.
        assert_eq!(app_side(&c("Cmd+T"), CommandKey::Command, true), s(Key::T, true, true, false));
        assert_eq!(app_side(&c("Ctrl+Shift+T"), CommandKey::Control, true), s(Key::T, true, true, false));
        // Keys of their own, and Alt's.
        assert_eq!(app_side(&c("F3"), CommandKey::Control, false), s(Key::F3, false, false, false));
        assert_eq!(app_side(&c("Alt+Return"), CommandKey::Control, false), s(Key::Enter, false, false, true));
        assert_eq!(app_side(&c("Alt+Cmd+F"), CommandKey::Command, false), s(Key::F, false, true, true));
        // Windows' key never reaches an app.
        assert_eq!(app_side(&c("Cmd+E"), CommandKey::Control, false), None);
        // And back, to show.
        let back = physical(&app_side(&c("Alt+Shift+Z"), CommandKey::Alt, false).unwrap(), CommandKey::Alt);
        assert_eq!(back.label(Platform::Beos), "Alt+Shift+Z");
        assert_eq!(physical(&Shortcut::command(Key::Z).shifted(), CommandKey::Command).label(Platform::Mac), "⇧⌘Z");
        assert_eq!(physical(&Shortcut::command(Key::Y), CommandKey::Control).label(Platform::Windows), "Ctrl+Y");
        for code in 1..=111 {
            if let Some(k) = egui_key(code)
                && code != 96
            {
                assert_eq!(evdev_key(k), code, "{k:?}");
            }
        }
    }
}
