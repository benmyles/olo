//! A terminal's keys as bytes for its program: xterm's legacy encoding
//! (arrows and their modifiers as `CSI 1;mods X`, Option+←/→ alone a word
//! back and on as macOS Terminal sends them, Ctrl+key as a C0 byte, Alt
//! as ESC before), one for every terminal of Olo's: Terminal's and Rex's
//! panes (Ben, 2026-11-24, of Rex: "using alt+left/right doesn't seem to
//! move back/forward a word properly it is sending some other escape
//! code": it sent ESC before the arrow's own sequence). The kitty keyboard
//! protocol's encoding is the terminal's own, over this.

use crate::Keystroke;

/// A key as the terminal encodes it: its keystroke's name, unshifted (it
/// names shifted symbols as themselves: "_" for ⇧-), and its modifiers,
/// Shift with them again.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Key<'a> {
    pub name: &'a str,
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

impl<'a> Key<'a> {
    pub fn of(k: &'a Keystroke) -> Key<'a> {
        let (name, shifted) = crate::keys::unshifted(&k.key);
        Key { name, shift: k.modifiers.shift || shifted, ctrl: k.modifiers.control, alt: k.modifiers.alt }
    }
}

/// xterm's modifier parameter: 1 + shift + 2·alt + 4·ctrl.
pub fn modifier_param(k: Key) -> u8 {
    1 + k.shift as u8 + 2 * k.alt as u8 + 4 * k.ctrl as u8
}

/// What the program asked of the keys: application cursor keys (DECCKM,
/// `ESC O A`), and the kitty keyboard protocol (its own encoding, by the
/// terminal; here only that Option+←/→ then keep xterm's `CSI 1;3 D`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modes {
    pub app_cursor: bool,
    pub kitty: bool,
}

/// The bytes for a key, or None when it's left to the text it types.
pub fn bytes(k: Key, modes: Modes) -> Option<Vec<u8>> {
    let mp = modifier_param(k);
    let kitty = modes.kitty;
    let csi = |letter: char| -> Vec<u8> {
        if mp > 1 {
            format!("\x1b[1;{mp}{letter}").into_bytes()
        } else if modes.app_cursor {
            format!("\x1bO{letter}").into_bytes()
        } else {
            format!("\x1b[{letter}").into_bytes()
        }
    };
    let tilde = |n: u8| -> Vec<u8> { if mp > 1 { format!("\x1b[{n};{mp}~").into_bytes() } else { format!("\x1b[{n}~").into_bytes() } };
    let ss3 = |letter: char| -> Vec<u8> { if mp > 1 { format!("\x1b[1;{mp}{letter}").into_bytes() } else { format!("\x1bO{letter}").into_bytes() } };
    let esc = |mut b: Vec<u8>| {
        if k.alt {
            b.insert(0, 0x1b);
        }
        b
    };
    let word = k.alt && !k.ctrl && !k.shift && !kitty;
    Some(match k.name {
        "up" => csi('A'),
        "down" => csi('B'),
        // Option+←/→ alone: a word back or forward, as macOS Terminal
        // sends it (readline, zsh and fish all know ESC b / ESC f; bash
        // leaves xterm's CSI 1;3 D unbound). A program that asked for the
        // kitty keyboard protocol (Claude Code, …) gets the protocol's
        // CSI 1;3 D and reads it as Alt+← itself.
        "right" if word => b"\x1bf".to_vec(),
        "left" if word => b"\x1bb".to_vec(),
        "right" => csi('C'),
        "left" => csi('D'),
        "home" => csi('H'),
        "end" => csi('F'),
        "pageup" => tilde(5),
        "pagedown" => tilde(6),
        "insert" => tilde(2),
        "delete" => tilde(3),
        "f1" => ss3('P'),
        "f2" => ss3('Q'),
        "f3" => ss3('R'),
        "f4" => ss3('S'),
        "f5" => tilde(15),
        "f6" => tilde(17),
        "f7" => tilde(18),
        "f8" => tilde(19),
        "f9" => tilde(20),
        "f10" => tilde(21),
        "f11" => tilde(23),
        "f12" => tilde(24),
        "enter" => esc(vec![b'\r']),
        "tab" if k.shift => b"\x1b[Z".to_vec(),
        "tab" => esc(vec![b'\t']),
        "backspace" if k.ctrl => esc(vec![0x08]),
        "backspace" => esc(vec![0x7f]),
        "escape" => esc(vec![0x1b]),
        _ if k.ctrl => esc(vec![ctrl_byte(k)?]),
        // Alt with a character: ESC and the character (the text event's
        // job without Alt).
        name if k.alt => {
            let c = if name == "space" { ' ' } else { single(name)? };
            let c = if k.shift { c.to_ascii_uppercase() } else { c };
            esc(c.to_string().into_bytes())
        }
        _ => return None,
    })
}

/// Whether a key is the kitty keyboard protocol's to encode, for a
/// program that asked for it (`flags`: what it pushed, not 0): not this
/// table's, which is xterm's and cannot say what the protocol exists to
/// say. Ctrl+Enter and Shift+Enter are a bare Return here, where the
/// protocol has `CSI 13;5u` and `CSI 13;2u`; Escape is `CSI 27u`; Ctrl
/// and Alt with a letter have forms of their own. So: Escape, any key
/// of this table pressed with a modifier, and, where the program asked
/// to be told every key as an escape code (flag 8), every key of this
/// table. A terminal with the protocol's encoder (Rex's server, which
/// knows the program's flags as they stand) sends those itself; text
/// typed is text either way.
///
/// (Ben, 2026-11-26: "ctrl+enter doens't seem to work (at least in
/// claude code) in a rex terminal". Claude Code pushes flags 5 at its
/// start, read from what it writes to a Rex pane, and Rex sent it a
/// Return.)
pub fn kitty_encodes(k: Key, flags: u8) -> bool {
    if flags == 0 || bytes(k, Modes { app_cursor: false, kitty: true }).is_none() {
        return false;
    }
    k.ctrl || k.alt || k.shift || k.name == "escape" || flags & 8 != 0
}

/// A one-character key's character.
pub fn single(name: &str) -> Option<char> {
    let mut chars = name.chars();
    let c = chars.next()?;
    chars.next().is_none().then_some(c)
}

/// Ctrl+key as a C0 control character (as xterm's: Ctrl+2 and Ctrl+@ are
/// NUL, Ctrl+3–7 the ESC…US block, Ctrl+8 and Ctrl+? DEL).
fn ctrl_byte(k: Key) -> Option<u8> {
    if k.name == "space" {
        return Some(0);
    }
    let c = single(k.name)?;
    if c.is_ascii_alphabetic() {
        return Some(c.to_ascii_lowercase() as u8 & 0x1f);
    }
    // With Shift: the shifted symbol's (Ctrl+_, Ctrl+^, Ctrl+@, Ctrl+?).
    let c = if k.shift { crate::keys::shifted(c).unwrap_or(c) } else { c };
    Some(match c {
        '2' | '@' => 0,
        '3' | '[' | '{' => 0x1b,
        '4' | '\\' | '|' => 0x1c,
        '5' | ']' | '}' => 0x1d,
        '6' | '^' | '~' => 0x1e,
        '7' | '-' | '_' | '/' => 0x1f,
        '8' | '?' => 0x7f,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the kitty keyboard protocol encodes, for a program that
    /// asked for it: the keys whose modifiers xterm's table drops.
    #[test]
    fn the_protocols_keys_are_the_ones_xterm_cannot_say() {
        let key = |name: &'static str, shift: bool, ctrl: bool, alt: bool| Key { name, shift, ctrl, alt };
        // Nothing, for a program that didn't ask.
        assert!(!kitty_encodes(key("enter", false, true, false), 0));
        // Claude Code's flags (5): Enter with a modifier, Tab with one, Escape, Ctrl and Alt with a letter.
        for k in [
            key("enter", false, true, false),
            key("enter", true, false, false),
            key("enter", false, false, true),
            key("tab", true, false, false),
            key("tab", false, true, false),
            key("escape", false, false, false),
            key("backspace", true, false, false),
            key("c", false, true, false),
            key("m", false, false, true),
            key("up", false, true, false),
        ] {
            assert!(kitty_encodes(k, 5), "{k:?}");
        }
        // A plain Return, Tab, Backspace or arrow is the same either way; text is text.
        for k in [
            key("enter", false, false, false),
            key("tab", false, false, false),
            key("backspace", false, false, false),
            key("up", false, false, false),
            key("a", false, false, false),
            key("a", true, false, false),
            key("space", true, false, false),
        ] {
            assert!(!kitty_encodes(k, 5), "{k:?}");
        }
        // Asked to be told every key as an escape code (8): the plain ones too, never text.
        assert!(kitty_encodes(key("enter", false, false, false), 13));
        assert!(!kitty_encodes(key("a", false, false, false), 13));
        // And what this table sends for Ctrl+Enter, which is why: a Return.
        assert_eq!(bytes(key("enter", false, true, false), Modes::default()), Some(b"\r".to_vec()));
    }
}
