//! The Mac keyboard, natively. ⌘ is the command key: apps see it as Ctrl
//! (so ⌘C copies everywhere, GTK and Firefox included), while app_server
//! keeps a few ⌘ chords for itself: ⌘Tab / ⌘` (Twitcher), ⌘Space and a
//! tap of ⌘ alone (Find), ⌘ arrows and ⌘⌫ (line and document navigation)
//! and Ctrl+⌘Q (lock). Caps Lock is another Ctrl. Terminals keep physical
//! Ctrl for the tty, so ⌘ reaches them as Ctrl+Shift.
//!
//! This sits between the input devices and the seat: physical evdev codes
//! go in, the codes clients should see (plus app_server actions) come out.
//! It is pure state, so it is tested without a compositor.

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

/// evdev key codes used here.
pub mod code {
    pub const BACKSPACE: u32 = 14;
    pub const TAB: u32 = 15;
    pub const Q: u32 = 16;
    pub const U: u32 = 22;
    pub const O: u32 = 24;
    pub const LEFTCTRL: u32 = 29;
    pub const GRAVE: u32 = 41;
    pub const LEFTSHIFT: u32 = 42;
    pub const RIGHTSHIFT: u32 = 54;
    pub const LEFTALT: u32 = 56;
    pub const SPACE: u32 = 57;
    pub const CAPSLOCK: u32 = 58;
    pub const RIGHTCTRL: u32 = 97;
    pub const RIGHTALT: u32 = 100;
    pub const HOME: u32 = 102;
    pub const UP: u32 = 103;
    pub const LEFT: u32 = 105;
    pub const RIGHT: u32 = 106;
    pub const END: u32 = 107;
    pub const DOWN: u32 = 108;
    pub const LEFTMETA: u32 = 125;
    pub const RIGHTMETA: u32 = 126;
}
use code::*;

/// A ⌘ press and release with nothing in between, shorter than this, is a
/// tap (Find).
pub const TAP: Duration = Duration::from_millis(350);

/// The virtual Ctrl that stands for ⌘. Physical Ctrl and Caps Lock use the
/// left one; keys are reference-counted, so sharing is harmless.
const CMD_CTRL: u32 = RIGHTCTRL;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// ⌘Tab (`back` with Shift): the next app in Twitcher.
    NextApp { back: bool },
    /// ⌘`: the next window of the focused app.
    NextWindow { back: bool },
    /// ⌘ released after switching: commit Twitcher's choice.
    SwitchDone,
    /// Tap ⌘, or ⌘Space.
    Find,
    /// Ctrl+⌘Q.
    Lock,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Out {
    /// An evdev code clients should see pressed (`true`) or released.
    Key(u32, bool),
    Action(Action),
}

#[derive(Debug, Clone, Copy)]
pub struct Options {
    pub command_key: bool,
    pub caps_lock_control: bool,
    pub tap_command_find: bool,
}

impl From<&tabula_protocol::KeyboardSettings> for Options {
    fn from(k: &tabula_protocol::KeyboardSettings) -> Self {
        Options {
            command_key: k.command_key,
            caps_lock_control: k.caps_lock_control,
            tap_command_find: k.tap_command_find,
        }
    }
}

/// What a physical key that is down produced, so its release undoes exactly
/// that, whatever has changed since.
#[derive(Debug, Default)]
struct Held {
    /// Emitted keys to release, in order.
    release: Vec<u32>,
    /// It is a ⌘ key.
    cmd: bool,
    /// It lifted ⌘'s Ctrl (⌘← is Home, not Ctrl+Home): put it back after.
    restore_ctrl: bool,
}

#[derive(Debug, Default)]
pub struct Keymap {
    held: HashMap<u32, Held>,
    /// How many holders each emitted key has.
    down: HashMap<u32, u32>,
    /// ⌘ keys physically down.
    cmd_keys: u32,
    /// ⌘'s Ctrl is currently pressed.
    cmd_ctrl: bool,
    /// Something was typed with ⌘ (so releasing it is not a tap).
    cmd_used: bool,
    cmd_since: Option<Instant>,
    switching: bool,
}

impl Keymap {
    /// Feed one physical key. `terminal` says the focused window keeps
    /// physical Ctrl for itself.
    pub fn feed(&mut self, key: u32, pressed: bool, now: Instant, opts: Options, terminal: bool) -> Vec<Out> {
        let mut out = Vec::new();
        if pressed {
            self.press(key, now, opts, terminal, &mut out);
        } else {
            self.release(key, now, opts, &mut out);
        }
        out
    }

    /// Forget everything (session switched away, keyboard focus reset):
    /// releases whatever is still down.
    pub fn reset(&mut self) -> Vec<Out> {
        let mut out: Vec<Out> = self.down.keys().map(|&k| Out::Key(k, false)).collect();
        out.sort_by_key(|o| match o {
            Out::Key(k, _) => *k,
            Out::Action(_) => 0,
        });
        if self.switching {
            out.push(Out::Action(Action::SwitchDone));
        }
        *self = Keymap::default();
        out
    }

    fn emit_press(&mut self, key: u32, out: &mut Vec<Out>) {
        let n = self.down.entry(key).or_insert(0);
        *n += 1;
        if *n == 1 {
            out.push(Out::Key(key, true));
        }
    }

    fn emit_release(&mut self, key: u32, out: &mut Vec<Out>) {
        match self.down.get_mut(&key) {
            Some(n) if *n > 1 => *n -= 1,
            Some(_) => {
                self.down.remove(&key);
                out.push(Out::Key(key, false));
            }
            None => {}
        }
    }

    fn is_held(&self, keys: &[u32]) -> bool {
        keys.iter().any(|k| self.held.contains_key(k))
    }

    fn shift(&self) -> bool {
        self.is_held(&[LEFTSHIFT, RIGHTSHIFT])
    }

    /// Physical Ctrl (or Caps Lock acting as it).
    fn ctrl(&self) -> bool {
        self.is_held(&[LEFTCTRL, RIGHTCTRL]) || self.held.get(&CAPSLOCK).is_some_and(|h| h.release == [LEFTCTRL])
    }

    fn alt(&self) -> bool {
        self.is_held(&[LEFTALT, RIGHTALT])
    }

    /// Lift ⌘'s Ctrl for a key that stands for an unmodified one.
    fn lift_ctrl(&mut self, h: &mut Held, out: &mut Vec<Out>) {
        if self.cmd_ctrl {
            self.cmd_ctrl = false;
            self.emit_release(CMD_CTRL, out);
            h.restore_ctrl = true;
        }
    }

    /// Tap `key` (press and release), wrapped in Shift unless it's down.
    fn tap_shifted(&mut self, key: u32, out: &mut Vec<Out>) {
        let shift = !self.shift();
        if shift {
            self.emit_press(LEFTSHIFT, out);
        }
        self.emit_press(key, out);
        self.emit_release(key, out);
        if shift {
            self.emit_release(LEFTSHIFT, out);
        }
    }

    fn press(&mut self, key: u32, now: Instant, opts: Options, terminal: bool, out: &mut Vec<Out>) {
        if self.held.contains_key(&key) {
            // A duplicate press (two devices, or a stuck state): ignore.
            return;
        }
        let mut h = Held::default();
        if opts.command_key && (key == LEFTMETA || key == RIGHTMETA) {
            self.cmd_keys += 1;
            if self.cmd_keys == 1 {
                self.cmd_used = false;
                self.cmd_since = Some(now);
            }
            if !self.cmd_ctrl {
                self.cmd_ctrl = true;
                self.emit_press(CMD_CTRL, out);
            }
            h.cmd = true;
            self.held.insert(key, h);
            return;
        }
        if opts.caps_lock_control && key == CAPSLOCK {
            self.emit_press(LEFTCTRL, out);
            h.release.push(LEFTCTRL);
            self.held.insert(key, h);
            return;
        }
        if self.cmd_keys > 0 {
            self.cmd_used = true;
            let (shift, ctrl) = (self.shift(), self.ctrl());
            let handled = match key {
                Q if ctrl => {
                    out.push(Out::Action(Action::Lock));
                    true
                }
                TAB if !ctrl => {
                    self.switching = true;
                    out.push(Out::Action(Action::NextApp { back: shift }));
                    true
                }
                GRAVE if !ctrl => {
                    self.switching = true;
                    out.push(Out::Action(Action::NextWindow { back: shift }));
                    true
                }
                SPACE if !ctrl => {
                    out.push(Out::Action(Action::Find));
                    true
                }
                // keyd's spelling of a ⌘ tap (Ctrl+Alt+Super+O), in case it
                // is still running underneath.
                O if ctrl && self.alt() => {
                    out.push(Out::Action(Action::Find));
                    true
                }
                LEFT | RIGHT if !ctrl => {
                    // Start and end of the line.
                    self.lift_ctrl(&mut h, out);
                    let to = if key == LEFT { HOME } else { END };
                    self.emit_press(to, out);
                    h.release.push(to);
                    true
                }
                UP | DOWN if !ctrl => {
                    let to = if key == UP { HOME } else { END };
                    if terminal {
                        // Shift+Home/End: the top and bottom of the
                        // scrollback.
                        self.lift_ctrl(&mut h, out);
                        self.tap_shifted(to, out);
                    } else {
                        // Ctrl+Home/End: the start and end of the document.
                        self.emit_press(to, out);
                        h.release.push(to);
                    }
                    true
                }
                BACKSPACE if !ctrl => {
                    if terminal {
                        // Ctrl+U: the shell's "delete to start of line".
                        self.emit_press(U, out);
                        h.release.push(U);
                    } else {
                        // Select to the start of the line, then delete it.
                        self.lift_ctrl(&mut h, out);
                        self.tap_shifted(HOME, out);
                        self.emit_press(BACKSPACE, out);
                        h.release.push(BACKSPACE);
                    }
                    true
                }
                _ if terminal && !shift && !ctrl => {
                    // ⌘C in a terminal is Ctrl+Shift+C (copy), not ^C.
                    self.emit_press(LEFTSHIFT, out);
                    self.emit_press(key, out);
                    h.release.extend([key, LEFTSHIFT]);
                    true
                }
                _ => false,
            };
            if handled {
                self.held.insert(key, h);
                return;
            }
        }
        self.emit_press(key, out);
        h.release.push(key);
        self.held.insert(key, h);
    }

    fn release(&mut self, key: u32, now: Instant, opts: Options, out: &mut Vec<Out>) {
        let Some(h) = self.held.remove(&key) else {
            // Pressed before we were watching: pass it through.
            out.push(Out::Key(key, false));
            return;
        };
        for k in h.release {
            self.emit_release(k, out);
        }
        if h.cmd {
            self.cmd_keys = self.cmd_keys.saturating_sub(1);
            if self.cmd_keys == 0 {
                if self.cmd_ctrl {
                    self.cmd_ctrl = false;
                    self.emit_release(CMD_CTRL, out);
                }
                if std::mem::take(&mut self.switching) {
                    out.push(Out::Action(Action::SwitchDone));
                }
                let tap = !self.cmd_used && self.cmd_since.is_some_and(|t| now.duration_since(t) < TAP);
                if tap && opts.tap_command_find {
                    out.push(Out::Action(Action::Find));
                }
                self.cmd_since = None;
            }
        } else if h.restore_ctrl && self.cmd_keys > 0 && !self.cmd_ctrl {
            self.cmd_ctrl = true;
            self.emit_press(CMD_CTRL, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPTS: Options = Options { command_key: true, caps_lock_control: true, tap_command_find: true };
    const C: u32 = 46;

    struct Rig {
        k: Keymap,
        t: Instant,
        terminal: bool,
    }

    impl Rig {
        fn new() -> Self {
            Rig { k: Keymap::default(), t: Instant::now(), terminal: false }
        }
        fn down(&mut self, key: u32) -> Vec<Out> {
            self.t += Duration::from_millis(10);
            self.k.feed(key, true, self.t, OPTS, self.terminal)
        }
        fn up(&mut self, key: u32) -> Vec<Out> {
            self.t += Duration::from_millis(10);
            self.k.feed(key, false, self.t, OPTS, self.terminal)
        }
    }

    use Out::Key;

    #[test]
    fn command_is_ctrl() {
        let mut r = Rig::new();
        assert_eq!(r.down(LEFTMETA), [Key(RIGHTCTRL, true)]);
        assert_eq!(r.down(C), [Key(C, true)]);
        assert_eq!(r.up(C), [Key(C, false)]);
        assert_eq!(r.up(LEFTMETA), [Key(RIGHTCTRL, false)]);
    }

    #[test]
    fn caps_lock_is_ctrl_and_plain_keys_pass() {
        let mut r = Rig::new();
        assert_eq!(r.down(CAPSLOCK), [Key(LEFTCTRL, true)]);
        assert_eq!(r.up(CAPSLOCK), [Key(LEFTCTRL, false)]);
        assert_eq!(r.down(C), [Key(C, true)]);
        assert_eq!(r.up(C), [Key(C, false)]);
        // Physical Ctrl is untouched.
        assert_eq!(r.down(LEFTCTRL), [Key(LEFTCTRL, true)]);
        assert_eq!(r.up(LEFTCTRL), [Key(LEFTCTRL, false)]);
    }

    #[test]
    fn tap_finds_but_a_chord_does_not() {
        let mut r = Rig::new();
        r.down(LEFTMETA);
        assert_eq!(r.up(LEFTMETA), [Key(RIGHTCTRL, false), Out::Action(Action::Find)]);
        r.down(LEFTMETA);
        r.down(C);
        r.up(C);
        assert_eq!(r.up(LEFTMETA), [Key(RIGHTCTRL, false)]);
        // Held too long: not a tap.
        r.down(LEFTMETA);
        r.t += TAP;
        assert_eq!(r.up(LEFTMETA), [Key(RIGHTCTRL, false)]);
    }

    #[test]
    fn twitcher() {
        let mut r = Rig::new();
        r.down(LEFTMETA);
        assert_eq!(r.down(TAB), [Out::Action(Action::NextApp { back: false })]);
        assert_eq!(r.up(TAB), []);
        r.down(LEFTSHIFT);
        assert_eq!(r.down(TAB), [Out::Action(Action::NextApp { back: true })]);
        r.up(TAB);
        r.up(LEFTSHIFT);
        assert_eq!(r.down(GRAVE), [Out::Action(Action::NextWindow { back: false })]);
        r.up(GRAVE);
        assert_eq!(r.up(LEFTMETA), [Key(RIGHTCTRL, false), Out::Action(Action::SwitchDone)]);
    }

    #[test]
    fn arrows_navigate() {
        let mut r = Rig::new();
        r.down(LEFTMETA);
        // ⌘← is Home without Ctrl; Ctrl comes back afterwards.
        assert_eq!(r.down(LEFT), [Key(RIGHTCTRL, false), Key(HOME, true)]);
        assert_eq!(r.up(LEFT), [Key(HOME, false), Key(RIGHTCTRL, true)]);
        // ⌘↓ is Ctrl+End.
        assert_eq!(r.down(DOWN), [Key(END, true)]);
        assert_eq!(r.up(DOWN), [Key(END, false)]);
        // ⌘⌫ selects to the line start and deletes.
        assert_eq!(
            r.down(BACKSPACE),
            [
                Key(RIGHTCTRL, false),
                Key(LEFTSHIFT, true),
                Key(HOME, true),
                Key(HOME, false),
                Key(LEFTSHIFT, false),
                Key(BACKSPACE, true)
            ]
        );
        assert_eq!(r.up(BACKSPACE), [Key(BACKSPACE, false), Key(RIGHTCTRL, true)]);
        // Released ⌘ before the arrow: Ctrl stays up.
        r.down(RIGHT);
        assert_eq!(r.up(LEFTMETA), []);
        assert_eq!(r.up(RIGHT), [Key(END, false)]);
        assert!(r.k.down.is_empty());
    }

    #[test]
    fn terminals_keep_ctrl() {
        let mut r = Rig::new();
        r.terminal = true;
        r.down(LEFTMETA);
        assert_eq!(r.down(C), [Key(LEFTSHIFT, true), Key(C, true)]);
        assert_eq!(r.up(C), [Key(C, false), Key(LEFTSHIFT, false)]);
        assert_eq!(r.down(BACKSPACE), [Key(U, true)]);
        r.up(BACKSPACE);
        r.up(LEFTMETA);
        // Physical Ctrl+C is ^C.
        assert_eq!(r.down(LEFTCTRL), [Key(LEFTCTRL, true)]);
        assert_eq!(r.down(C), [Key(C, true)]);
    }

    #[test]
    fn lock_and_shared_ctrl() {
        let mut r = Rig::new();
        r.down(CAPSLOCK);
        r.down(LEFTMETA);
        assert_eq!(r.down(Q), [Out::Action(Action::Lock)]);
        assert_eq!(r.up(Q), []);
        r.up(LEFTMETA);
        // Physical Ctrl and Caps Lock share the left Ctrl.
        assert_eq!(r.down(LEFTCTRL), []);
        assert_eq!(r.up(CAPSLOCK), []);
        assert_eq!(r.up(LEFTCTRL), [Key(LEFTCTRL, false)]);
    }

    #[test]
    fn disabled_passes_through() {
        let mut k = Keymap::default();
        let off = Options { command_key: false, caps_lock_control: false, tap_command_find: false };
        let t = Instant::now();
        assert_eq!(k.feed(LEFTMETA, true, t, off, false), [Key(LEFTMETA, true)]);
        assert_eq!(k.feed(TAB, true, t, off, false), [Key(TAB, true)]);
        assert_eq!(k.feed(CAPSLOCK, true, t, off, false), [Key(CAPSLOCK, true)]);
    }

    #[test]
    fn reset_releases_everything() {
        let mut r = Rig::new();
        r.down(LEFTMETA);
        r.down(LEFTSHIFT);
        r.down(TAB);
        assert_eq!(
            r.k.reset(),
            [Key(LEFTSHIFT, false), Key(RIGHTCTRL, false), Out::Action(Action::SwitchDone)]
        );
        assert_eq!(r.up(TAB), [Key(TAB, false)]);
    }
}
