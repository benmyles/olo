//! The keyboard profile app_server uses (Preferences ▸ Keyboard: Mac,
//! Windows, BeOS, or one changed from them), as apps need it, whatever
//! kit draws them: a menu item's keys come from the profile
//! ([`olo_protocol::app_keys`], by its command), shown as the platform
//! writes them ("⇧⌘Z", "Ctrl+Y", "Alt+Shift+Z"), and pressed as the app
//! sees them (the command key is always Ctrl to an app). Keys are evdev
//! codes here; each kit turns them into its own.
//!
//! Outside Olo the profile is a Mac's.

use olo_sync::Mutex;
use std::{
    collections::HashMap,
    sync::{
        OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use olo_protocol::{Chord, CommandKey, KeyboardProfile, Platform, app_keys, shortcuts};

static PROFILE: Mutex<Option<KeyboardProfile>> = Mutex::new(None);
/// Bumped when the profile changes, so keys worked out are again.
static GENERATION: AtomicU64 = AtomicU64::new(0);
static COMMAND_KEY: AtomicBool = AtomicBool::new(false);
static APP_ID: OnceLock<String> = OnceLock::new();

/// The app whose own keys the profile has (its id, "olo.pool").
pub fn set_app_id(app_id: &str) {
    let _ = APP_ID.set(app_id.to_owned());
}

/// The program's id ("olo.deskcalc"), as `set_app_id` was given it.
pub fn app_id() -> &'static str {
    APP_ID.get().map_or("", String::as_str)
}

/// app_server's profile (from its settings), as it changes.
pub fn set(profile: &KeyboardProfile) {
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

/// Which platform's keys and labels the profile follows.
pub fn platform() -> Platform {
    PROFILE.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map_or(Platform::Mac, |p| p.platform)
}

/// The command key is one app_server turns into Ctrl (⌘, or BeOS's Alt),
/// sending it to terminals as its right Ctrl with Shift. False outside
/// Olo, and with Windows' keys, where Ctrl is the command key itself.
pub fn command_key() -> bool {
    COMMAND_KEY.load(Ordering::Relaxed)
}

/// A shortcut as the app sees it: an evdev key, with the command key
/// (Ctrl to the app, whatever the profile presses it with), Shift and
/// Alt; or a key of its own (Windows' F3, Delete).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Shortcut {
    pub key: u32,
    pub shift: bool,
    pub command: bool,
    pub alt: bool,
}

impl Shortcut {
    /// The command key and `key`: ⌘W.
    pub const fn command(key: u32) -> Shortcut {
        Shortcut { key, shift: false, command: true, alt: false }
    }

    pub const fn shifted(self) -> Shortcut {
        Shortcut { shift: true, ..self }
    }

    /// All its keys as the profile's platform writes them: "⌘W",
    /// "Ctrl+W", "Alt+W".
    pub fn label(&self) -> String {
        let p = profile();
        physical(self, p.command_key).label(p.platform)
    }
}

/// A chord (physical keys, as profiles write them) as an app sees it: the
/// command key is Ctrl, and on BeOS the Super key (Option) is Alt. In a
/// terminal-style window (`raw`), the command key arrives with Shift. None
/// for one an app never sees (Windows' Super key).
pub fn app_side(c: &Chord, command: CommandKey, raw: bool) -> Option<Shortcut> {
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
    Some(Shortcut { key: c.key, shift, command: cmd, alt })
}

/// The physical chord an app's shortcut is pressed with, on a profile
/// with `command` as its command key (for showing it).
pub fn physical(s: &Shortcut, command: CommandKey) -> Chord {
    let (ctrl, alt, cmd) = match command {
        CommandKey::Command => (false, s.alt, s.command),
        CommandKey::Alt => (false, s.command, s.alt),
        CommandKey::Control => (s.command, s.alt, false),
    };
    Chord { ctrl, alt, shift: s.shift, cmd, key: s.key }
}

/// What the profile has for a command in this app.
#[derive(Clone, Debug)]
pub struct Known {
    /// The app's own entry, not every app's.
    pub own: bool,
    /// Its keys as the app presses them, and as the platform writes them
    /// (None: it has none).
    pub keys: Option<(Shortcut, String)>,
}

/// The keys `command` has in this app (as a terminal-style window, `raw`,
/// presses them): None if the profile doesn't know the command.
pub fn known(command: &str, raw: bool) -> Option<Known> {
    // (A map for each of `raw`, so a command is looked up as it's given:
    // menus ask for every item at every body, and nothing is made to ask.)
    type Cache = (u64, [HashMap<String, Option<Known>>; 2]);
    static CACHE: Mutex<Option<Cache>> = Mutex::new(None);
    let generation = GENERATION.load(Ordering::Relaxed);
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if cache.as_ref().is_none_or(|(g, _)| *g != generation) {
        *cache = Some((generation, Default::default()));
    }
    let (_, maps) = cache.as_mut()?;
    let map = &mut maps[raw as usize];
    if let Some(known) = map.get(command) {
        return known.clone();
    }
    let known = app_keys::app_key(app_id(), command).map(|entry| {
        let p = profile();
        let chords = shortcuts::entry(entry.id).map(|e| e.keys(&p)).unwrap_or_default();
        let keys = chords.iter().find_map(|c| Some((app_side(c, p.command_key, raw)?, c.label(p.platform))));
        Known { own: entry.app.is_some(), keys }
    });
    map.insert(command.to_owned(), known.clone());
    known
}

/// The shortcut `command` has in this app (from the profile), if any.
pub fn shortcut(command: &str) -> Option<Shortcut> {
    known(command, false)?.keys.map(|(s, _)| s)
}

/// App `app`'s own shortcuts in the profile (as a terminal-style window
/// gets them, if `raw`): for an app that sends every other key on
/// (Terminal, to its shell).
pub fn own_shortcuts(app: &str, raw: bool) -> Vec<Shortcut> {
    let p = profile();
    app_keys::APP_KEYS.iter().filter(|e| e.app == Some(app)).flat_map(|e| shortcuts::entry(e.id).map(|x| x.keys(&p)).unwrap_or_default()).filter_map(|c| app_side(&c, p.command_key, raw)).collect()
}

/// A menu item with keys the profile doesn't list: they can't be changed
/// in Preferences, nor shown in the shortcuts sheet. Said once.
pub fn unlisted(command: &str, s: &Shortcut) {
    static SAID: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let mut said = SAID.lock().unwrap_or_else(|e| e.into_inner());
    if !said.iter().any(|c| c == command) {
        said.push(command.to_owned());
        tracing::warn!(app = app_id(), command, keys = s.label(), "menu keys not in olo_protocol::app_keys");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_as_apps_see_them() {
        let c = |s: &str| s.parse::<Chord>().unwrap();
        let s = |key, shift, command, alt| Some(Shortcut { key, shift, command, alt });
        const C: u32 = 46;
        const T: u32 = 20;
        // ⌘C, Ctrl+C and BeOS's Alt+C are all Ctrl+C to the app.
        assert_eq!(app_side(&c("Cmd+C"), CommandKey::Command, false), s(C, false, true, false));
        assert_eq!(app_side(&c("Ctrl+C"), CommandKey::Control, false), s(C, false, true, false));
        assert_eq!(app_side(&c("Alt+C"), CommandKey::Alt, false), s(C, false, true, false));
        // A terminal gets the command key's with Shift; Windows' as written.
        assert_eq!(app_side(&c("Cmd+T"), CommandKey::Command, true), s(T, true, true, false));
        assert_eq!(app_side(&c("Ctrl+Shift+T"), CommandKey::Control, true), s(T, true, true, false));
        // Keys of their own, and Alt's.
        assert_eq!(app_side(&c("F3"), CommandKey::Control, false), s(61, false, false, false));
        assert_eq!(app_side(&c("Alt+Return"), CommandKey::Control, false), s(28, false, false, true));
        // Windows' key never reaches an app.
        assert_eq!(app_side(&c("Cmd+E"), CommandKey::Control, false), None);
        // And back, to show.
        let back = physical(&app_side(&c("Alt+Shift+Z"), CommandKey::Alt, false).unwrap(), CommandKey::Alt);
        assert_eq!(back.label(Platform::Beos), "Alt+Shift+Z");
        assert_eq!(physical(&Shortcut::command(44).shifted(), CommandKey::Command).label(Platform::Mac), "⇧⌘Z");
        assert_eq!(physical(&Shortcut::command(21), CommandKey::Control).label(Platform::Windows), "Ctrl+Y");
    }
}
