//! Menu shortcuts on the kit's keystrokes: the keyboard profile's keys
//! ([`olo_kit_base::keys`], evdev codes) named as a `Keystroke` names
//! them, and
//! whether a keystroke presses one.

use olo_interface::Keystroke;
pub use olo_kit_base::keys::{Shortcut, profile, shortcut};

/// The keys shortcuts name, by their evdev codes.
const KEYS: &[(u32, &str)] = &[
    (30, "a"),
    (48, "b"),
    (46, "c"),
    (32, "d"),
    (18, "e"),
    (33, "f"),
    (34, "g"),
    (35, "h"),
    (23, "i"),
    (36, "j"),
    (37, "k"),
    (38, "l"),
    (50, "m"),
    (49, "n"),
    (24, "o"),
    (25, "p"),
    (16, "q"),
    (19, "r"),
    (31, "s"),
    (20, "t"),
    (22, "u"),
    (47, "v"),
    (17, "w"),
    (45, "x"),
    (21, "y"),
    (44, "z"),
    (1, "escape"),
    (2, "1"),
    (3, "2"),
    (4, "3"),
    (5, "4"),
    (6, "5"),
    (7, "6"),
    (8, "7"),
    (9, "8"),
    (10, "9"),
    (11, "0"),
    (12, "-"),
    (13, "="),
    (14, "backspace"),
    (15, "tab"),
    (26, "["),
    (27, "]"),
    (28, "enter"),
    (39, ";"),
    (40, "'"),
    (41, "`"),
    (43, "\\"),
    (51, ","),
    (52, "."),
    (53, "/"),
    (57, "space"),
    (59, "f1"),
    (60, "f2"),
    (61, "f3"),
    (62, "f4"),
    (63, "f5"),
    (64, "f6"),
    (65, "f7"),
    (66, "f8"),
    (67, "f9"),
    (68, "f10"),
    (87, "f11"),
    (88, "f12"),
    (96, "enter"),
    (102, "home"),
    (103, "up"),
    (104, "pageup"),
    (105, "left"),
    (106, "right"),
    (107, "end"),
    (108, "down"),
    (109, "pagedown"),
    (111, "delete"),
];

/// A keystroke's name for an evdev key (the keys a shortcut can have).
pub fn key_name(code: u32) -> Option<&'static str> {
    KEYS.iter().find(|(c, _)| *c == code).map(|(_, k)| *k)
}

/// The evdev key a keystroke names `key` ("w", "f3", "delete"); 0 if none.
pub fn evdev(key: &str) -> u32 {
    // (Looked up, not searched for: it's asked for every item with a
    // shortcut each time a menu bar is said again, which is every time
    // its window is: 140 times a step of a Tracker window resized.
    // Through `key_name` for each code in turn, as it was, that was up
    // to a hundred passes over the table and a new string each time.)
    static BY_NAME: std::sync::LazyLock<std::collections::HashMap<&'static str, u32>> = std::sync::LazyLock::new(|| {
        let mut by_name = std::collections::HashMap::new();
        for &(code, name) in KEYS {
            let lowest = by_name.entry(name).or_insert(code);
            *lowest = (*lowest).min(code);
        }
        by_name
    });
    match BY_NAME.get(key) {
        Some(code) => *code,
        // (Named in capitals: as its small letters.)
        None if key.bytes().any(|b| b.is_ascii_uppercase()) => BY_NAME.get(key.to_ascii_lowercase().as_str()).copied().unwrap_or(0),
        None => 0,
    }
}

/// Whether `k` presses `s` (the command key is Ctrl to an app). ⇧= types
/// + (and a terminal's ⌘ always brings Shift), so one on = takes + too,
/// as ⌘+ zooms in on a Mac.
/// ⌘ is held with a Mac's keys (app_server sends it as the right Ctrl):
/// in text, ⌘← and ⌘→ are the line's start and end and ⌘⌫ deletes to its
/// start, where the same Ctrl from the left (or BeOS's command key) goes
/// by words. Asked as an action comes, the key that brought it still down.
pub fn mac_command() -> bool {
    olo_kit_base::keys::platform() == olo_protocol::Platform::Mac && olo_kit_base::keys::command_key() && olo_interface::right_control_held()
}

pub fn pressed(s: &Shortcut, k: &Keystroke) -> bool {
    let Some(name) = key_name(s.key) else { return false };
    let key = k.key.to_ascii_lowercase();
    let plus = name == "=" && key == "+";
    // (A keystroke names a shifted symbol as itself, without Shift: ⇧- is "_".)
    let (key, shifted) = unshifted(&key);
    let m = &k.modifiers;
    (key == name || plus) && (s.shift == (m.shift || shifted) || plus) && s.command == m.control && s.alt == m.alt && !m.platform
}

/// US keys' shifted symbols and the keys they're on.
const SHIFTED: &[(char, char)] = &[
    ('!', '1'),
    ('@', '2'),
    ('#', '3'),
    ('$', '4'),
    ('%', '5'),
    ('^', '6'),
    ('&', '7'),
    ('*', '8'),
    ('(', '9'),
    (')', '0'),
    ('_', '-'),
    ('+', '='),
    ('{', '['),
    ('}', ']'),
    ('|', '\\'),
    (':', ';'),
    ('"', '\''),
    ('<', ','),
    ('>', '.'),
    ('?', '/'),
    ('~', '`'),
];

/// The key a keystroke's name `key` is on, and whether it took Shift: it names
/// a shifted symbol as itself and drops Shift from it ("_" for ⇧-), so
/// ("-", true); anything else as it is, (key, false).
pub fn unshifted(key: &str) -> (&str, bool) {
    const BASE: [&str; 21] = ["1", "2", "3", "4", "5", "6", "7", "8", "9", "0", "-", "=", "[", "]", "\\", ";", "'", ",", ".", "/", "`"];
    let mut chars = key.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => match SHIFTED.iter().position(|(s, _)| *s == c) {
            Some(i) => (BASE[i], true),
            None => (key, false),
        },
        _ => (key, false),
    }
}

/// The symbol Shift types on US key `c` ('-' → '_'), if it has one.
pub fn shifted(c: char) -> Option<char> {
    SHIFTED.iter().find(|(_, base)| *base == c).map(|(s, _)| *s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shifted_symbols_are_their_keys_with_shift() {
        assert_eq!(unshifted("_"), ("-", true));
        assert_eq!(unshifted("|"), ("\\", true));
        assert_eq!(unshifted("a"), ("a", false));
        assert_eq!(unshifted("pageup"), ("pageup", false));
        assert_eq!(shifted('2'), Some('@'));
        for (s, base) in SHIFTED {
            assert_eq!(unshifted(&s.to_string()).0, base.to_string());
        }
    }
}

#[cfg(test)]
mod lookup_tests {
    use super::*;

    /// `evdev` is `key_name` the other way round, for every key there
    /// is, whatever its case: as it was when it tried each code in turn.
    #[test]
    fn evdev_is_key_name_backwards() {
        for &(code, name) in KEYS {
            let slow = (1..=111).find(|&c| key_name(c) == Some(name)).unwrap_or(0);
            assert_eq!(evdev(name), slow, "{name}");
            assert_eq!(evdev(&name.to_ascii_uppercase()), slow, "{name} in capitals");
            assert_eq!(key_name(evdev(name)), Some(name));
            assert!((1..=111).contains(&code));
        }
        assert_eq!(evdev("w"), 17);
        assert_eq!(evdev("no such key"), 0);
        assert_eq!(evdev(""), 0);
    }
}
