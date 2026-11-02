//! Menu shortcuts on GPUI keystrokes: the keyboard profile's keys
//! ([`olo_kit_base::keys`], evdev codes) named as GPUI names them, and
//! whether a keystroke presses one.

use gpui::Keystroke;
pub use olo_kit_base::keys::{Shortcut, profile, shortcut};

/// GPUI's name for an evdev key (the keys a shortcut can have).
pub fn gpui_key(code: u32) -> Option<&'static str> {
    const KEYS: &[(u32, &str)] = &[
        (30, "a"), (48, "b"), (46, "c"), (32, "d"), (18, "e"), (33, "f"), (34, "g"), (35, "h"), (23, "i"), (36, "j"), (37, "k"), (38, "l"), (50, "m"),
        (49, "n"), (24, "o"), (25, "p"), (16, "q"), (19, "r"), (31, "s"), (20, "t"), (22, "u"), (47, "v"), (17, "w"), (45, "x"), (21, "y"), (44, "z"),
        (1, "escape"), (2, "1"), (3, "2"), (4, "3"), (5, "4"), (6, "5"), (7, "6"), (8, "7"), (9, "8"), (10, "9"), (11, "0"), (12, "-"), (13, "="),
        (14, "backspace"), (15, "tab"), (26, "["), (27, "]"), (28, "enter"), (39, ";"), (40, "'"), (41, "`"), (43, "\\"), (51, ","), (52, "."),
        (53, "/"), (57, "space"), (59, "f1"), (60, "f2"), (61, "f3"), (62, "f4"), (63, "f5"), (64, "f6"), (65, "f7"), (66, "f8"), (67, "f9"),
        (68, "f10"), (87, "f11"), (88, "f12"), (96, "enter"), (102, "home"), (103, "up"), (104, "pageup"), (105, "left"), (106, "right"),
        (107, "end"), (108, "down"), (109, "pagedown"), (111, "delete"),
    ];
    KEYS.iter().find(|(c, _)| *c == code).map(|(_, k)| *k)
}

/// The evdev key GPUI names `key` ("w", "f3", "delete"); 0 if none.
pub fn evdev(key: &str) -> u32 {
    let key = key.to_ascii_lowercase();
    (1..=111).find(|&c| gpui_key(c) == Some(key.as_str())).unwrap_or(0)
}

/// Whether `k` presses `s` (the command key is Ctrl to an app). ⇧= types
/// + (and a terminal's ⌘ always brings Shift), so one on = takes + too,
/// as ⌘+ zooms in on a Mac.
pub fn pressed(s: &Shortcut, k: &Keystroke) -> bool {
    let Some(name) = gpui_key(s.key) else { return false };
    let key = k.key.to_ascii_lowercase();
    let plus = name == "=" && key == "+";
    // (GPUI names a shifted symbol as itself, without Shift: ⇧- is "_".)
    let (key, shifted) = unshifted(&key);
    let m = &k.modifiers;
    (key == name || plus) && (s.shift == (m.shift || shifted) || plus) && s.command == m.control && s.alt == m.alt && !m.platform
}

/// US keys' shifted symbols and the keys they're on.
const SHIFTED: &[(char, char)] = &[
    ('!', '1'), ('@', '2'), ('#', '3'), ('$', '4'), ('%', '5'), ('^', '6'), ('&', '7'), ('*', '8'), ('(', '9'), (')', '0'),
    ('_', '-'), ('+', '='), ('{', '['), ('}', ']'), ('|', '\\'), (':', ';'), ('"', '\''), ('<', ','), ('>', '.'), ('?', '/'), ('~', '`'),
];

/// The key GPUI's name `key` is on, and whether it took Shift: GPUI names
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
