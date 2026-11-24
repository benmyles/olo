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
