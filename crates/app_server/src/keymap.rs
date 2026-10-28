//! The Mac keyboard, natively. ⌘ is the command key: apps see it as Ctrl
//! (so ⌘C copies everywhere, GTK and Firefox included), while app_server
//! keeps some ⌘ keys for itself: ⌘Tab / ⌘\` (Twitcher), ⌘ arrows and ⌘⌫
//! (line and document navigation), a tap of ⌘ alone (the command palette, if chosen in
//! Preferences), Control pressed twice quickly (keyboard hints, if
//! chosen), and first of all the shortcuts in `Settings::shortcuts`
//! (olo_protocol::shortcuts: tiling, workspaces, Find, lock,
//! screenshots, …). Caps Lock is another Ctrl. Terminals keep physical
//! Ctrl for the tty, so ⌘ reaches them as Ctrl+Shift. Next / Previous
//! Tab (⌥Tab, ⌥⇧Tab) reach the app as Ctrl+Tab / Ctrl+Shift+Tab.
//!
//! This sits between the input devices and the seat: physical evdev codes
//! go in, the codes clients should see (plus app_server actions) come out.
//! It is pure state, so it is tested without a compositor.

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use olo_protocol::{Chord, Shortcut};

/// evdev key codes used here.
pub mod code {
    pub const ESC: u32 = 1;
    pub const KEY3: u32 = 4;
    pub const KEY4: u32 = 5;
    pub const KEY5: u32 = 6;
    pub const BACKSPACE: u32 = 14;
    pub const TAB: u32 = 15;
    pub const Q: u32 = 16;
    pub const A: u32 = 30;
    pub const U: u32 = 22;
    pub const O: u32 = 24;
    pub const LEFTCTRL: u32 = 29;
    pub const GRAVE: u32 = 41;
    pub const LEFTSHIFT: u32 = 42;
    pub const RIGHTSHIFT: u32 = 54;
    pub const LEFTALT: u32 = 56;
    pub const SPACE: u32 = 57;
    pub const CAPSLOCK: u32 = 58;
    pub const SYSRQ: u32 = 99;
    pub const RIGHTCTRL: u32 = 97;
    pub const RIGHTALT: u32 = 100;
    pub const HOME: u32 = 102;
    pub const UP: u32 = 103;
    pub const LEFT: u32 = 105;
    pub const RIGHT: u32 = 106;
    pub const END: u32 = 107;
    pub const DOWN: u32 = 108;
    pub const DELETE: u32 = 111;
    pub const LEFTMETA: u32 = 125;
    pub const RIGHTMETA: u32 = 126;
}
use code::*;

/// A ⌘ press and release with nothing in between, shorter than this, is a
/// tap (the command palette).
pub const TAP: Duration = Duration::from_millis(350);

/// The virtual Ctrl that stands for ⌘. Physical Ctrl and Caps Lock use the
/// left one; keys are reference-counted, so sharing is harmless.
const CMD_CTRL: u32 = RIGHTCTRL;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// ⌘Tab (`back` with Shift): the next app in Twitcher.
    NextApp { back: bool },
    /// ⌘\`: the next window of the focused app.
    NextWindow { back: bool },
    /// ⌘ released after switching: commit Twitcher's choice.
    SwitchDone,
    /// An arrow while switching: along the row (←/→) or to another
    /// workspace's row (↑/↓).
    SwitchMove { dx: i32, dy: i32 },
    /// A letter while switching, with keyboard hints on: the cell it marks.
    SwitchPick(char),
    /// Esc while switching: everything back as it was.
    SwitchCancel,
    /// A tap of ⌘ (if chosen), or keyd's spelling of one.
    Palette,
    /// Control pressed twice quickly (if chosen): keyboard hints.
    Hints,
    /// One of the shortcuts in the settings.
    Shortcut(Shortcut),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Out {
    /// An evdev code clients should see pressed (`true`) or released.
    Key(u32, bool),
    Action(Action),
    /// The key pressed while recording a shortcut (None: cancelled with
    /// Escape; `Some(None)`: no keys, Backspace or Delete).
    Recorded(Option<Option<Chord>>),
}

#[derive(Debug, Clone, Copy)]
pub struct Options {
    pub command_key: bool,
    pub caps_lock_control: bool,
    pub tap_command_find: bool,
    pub hints: bool,
    /// How quickly a double press must come: the second press within this
    /// of the first, each press shorter than it.
    pub double_press: Duration,
}

impl From<&olo_protocol::KeyboardSettings> for Options {
    fn from(k: &olo_protocol::KeyboardSettings) -> Self {
        Options {
            command_key: k.command_key,
            caps_lock_control: k.caps_lock_control,
            tap_command_find: k.find_on_command_tap,
            hints: k.hints,
            double_press: Duration::from_millis(k.double_press_ms.clamp(100, 1000) as u64),
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
    /// The shortcuts (from the settings).
    bindings: Vec<(Chord, Shortcut)>,
    /// The next key is a shortcut being recorded.
    recording: bool,
    /// Control going down and up on its own, for a double press
    /// (keyboard hints).
    ctrl_presses: Option<CtrlPresses>,
    /// Modifiers let go of for a key that stood for another chord (⌥ for
    /// Next Tab's Ctrl+Tab), kept up until another key needs them.
    lifted: Vec<u32>,
}

/// The modifier keys clients see.
const MODIFIERS: [u32; 8] = [LEFTCTRL, RIGHTCTRL, LEFTSHIFT, RIGHTSHIFT, LEFTALT, RIGHTALT, LEFTMETA, RIGHTMETA];

#[derive(Debug)]
struct CtrlPresses {
    /// The first press.
    since: Instant,
    /// The press going on now.
    down: Instant,
    /// Presses already let go.
    done: u8,
}

/// Physical Control (or Caps Lock as one), for the hints' double press.
fn ctrl_key(key: u32, opts: Options) -> bool {
    matches!(key, LEFTCTRL | RIGHTCTRL) || (opts.caps_lock_control && key == CAPSLOCK)
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
            Out::Action(_) | Out::Recorded(_) => 0,
        });
        if self.switching {
            out.push(Out::Action(Action::SwitchDone));
        }
        let (bindings, recording) = (std::mem::take(&mut self.bindings), self.recording);
        *self = Keymap { bindings, recording, ..Keymap::default() };
        out
    }

    pub fn set_bindings(&mut self, bindings: Vec<(Chord, Shortcut)>) {
        self.bindings = bindings;
    }

    /// Take the next key as a shortcut being recorded (or stop).
    pub fn record(&mut self, on: bool) {
        self.recording = on;
    }

    /// A ⌘ key is held (one standing for the command key).
    pub fn command_held(&self) -> bool {
        self.cmd_keys > 0
    }

    /// Something was done with ⌘ held other than typing (a click, a
    /// ⌘-drag of a tab): letting go of it is no tap.
    pub fn command_used(&mut self) {
        if self.cmd_keys > 0 {
            self.cmd_used = true;
        }
    }

    pub fn recording(&self) -> bool {
        self.recording
    }

    /// The key with the modifiers held now.
    fn chord(&self, key: u32) -> Chord {
        Chord { ctrl: self.ctrl(), alt: self.alt(), shift: self.shift(), cmd: self.cmd_keys > 0, key }
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

    /// `h`'s key stands for Ctrl+`to` (Ctrl+Shift+`to` with `shift`):
    /// the modifiers clients see become just those. Ctrl and Shift go
    /// down before the others are let go of, and those stay up until
    /// another key needs them (`restore_lifted`), so ⌥Tab never looks
    /// like a tap of ⌥ (Firefox shows its menu bar on one).
    fn stand_for(&mut self, h: &mut Held, to: u32, shift: bool, out: &mut Vec<Out>) {
        let down = |k: &Keymap, keys: [u32; 2]| keys.iter().any(|x| k.down.contains_key(x));
        let mut pressed = Vec::new();
        if !down(self, [LEFTCTRL, RIGHTCTRL]) {
            pressed.push(LEFTCTRL);
        }
        if shift && !down(self, [LEFTSHIFT, RIGHTSHIFT]) {
            pressed.push(LEFTSHIFT);
        }
        for &k in &pressed {
            self.emit_press(k, out);
        }
        let keep = |k: u32| matches!(k, LEFTCTRL | RIGHTCTRL) || (shift && matches!(k, LEFTSHIFT | RIGHTSHIFT));
        for k in MODIFIERS.into_iter().filter(|&k| !keep(k)) {
            if self.down.remove(&k).is_some() {
                out.push(Out::Key(k, false));
                self.lifted.push(k);
            }
        }
        self.emit_press(to, out);
        h.release.push(to);
        h.release.extend(pressed.into_iter().rev());
    }

    /// Put back the modifiers `stand_for` let go of that are still held.
    fn restore_lifted(&mut self, out: &mut Vec<Out>) {
        for k in std::mem::take(&mut self.lifted) {
            let holders = self.held.values().filter(|h| h.release.contains(&k)).count() as u32;
            if holders > 0 && !self.down.contains_key(&k) {
                self.down.insert(k, holders);
                out.push(Out::Key(k, true));
            }
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
        if opts.hints {
            // Control on its own, nothing else down (⌘, Shift, a letter),
            // starts a double press or is its second.
            self.ctrl_presses = match self.ctrl_presses.take() {
                _ if !ctrl_key(key, opts) || !self.held.is_empty() => None,
                Some(p) if p.done == 1 && now.duration_since(p.since) < opts.double_press => Some(CtrlPresses { down: now, ..p }),
                _ => Some(CtrlPresses { since: now, down: now, done: 0 }),
            };
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
        if self.recording && !olo_protocol::shortcuts::MODIFIER_KEYS.contains(&key) {
            // Swallowed, press and release.
            self.recording = false;
            let chord = self.chord(key);
            let plain = Chord::new(key) == chord;
            out.push(Out::Recorded(match key {
                ESC if plain => None,
                BACKSPACE | DELETE if plain => Some(None),
                _ => Some(Some(chord)),
            }));
            self.held.insert(key, h);
            return;
        }
        if self.cmd_keys > 0 {
            self.cmd_used = true;
        }
        if self.switching && self.cmd_keys > 0 && !self.ctrl() && !self.alt() {
            let action = match key {
                LEFT => Some(Action::SwitchMove { dx: -1, dy: 0 }),
                RIGHT => Some(Action::SwitchMove { dx: 1, dy: 0 }),
                UP => Some(Action::SwitchMove { dx: 0, dy: -1 }),
                DOWN => Some(Action::SwitchMove { dx: 0, dy: 1 }),
                ESC => {
                    self.switching = false;
                    Some(Action::SwitchCancel)
                }
                _ if opts.hints && !self.shift() => crate::hints::letter(key).map(Action::SwitchPick),
                _ => None,
            };
            if let Some(action) = action {
                out.push(Out::Action(action));
                self.held.insert(key, h);
                return;
            }
        }
        let chord = self.chord(key);
        let shortcut = self.bindings.iter().find(|(c, _)| *c == chord).map(|&(_, s)| s);
        let tab = matches!(shortcut, Some(Shortcut::NextTab | Shortcut::PreviousTab));
        if !tab && !olo_protocol::shortcuts::MODIFIER_KEYS.contains(&key) {
            self.restore_lifted(out);
        }
        if let Some(shortcut) = shortcut {
            if tab {
                // What every toolkit takes for the next / previous tab.
                self.stand_for(&mut h, TAB, shortcut == Shortcut::PreviousTab, out);
            } else {
                out.push(Out::Action(Action::Shortcut(shortcut)));
            }
            self.held.insert(key, h);
            return;
        }
        if self.cmd_keys > 0 {
            let (shift, ctrl) = (self.shift(), self.ctrl());
            let handled = match key {
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
                // keyd's spelling of a ⌘ tap (Ctrl+Alt+Super+O), in case it
                // is still running underneath.
                O if ctrl && self.alt() => {
                    out.push(Out::Action(Action::Palette));
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
                    out.push(Out::Action(Action::Palette));
                }
                self.cmd_since = None;
            }
        } else if h.restore_ctrl && self.cmd_keys > 0 && !self.cmd_ctrl {
            self.cmd_ctrl = true;
            self.emit_press(CMD_CTRL, out);
        }
        // Control let go, quickly and with nothing else pressed: the
        // second time, that's a double press.
        if ctrl_key(key, opts)
            && let Some(mut p) = self.ctrl_presses.take()
            && opts.hints
            && now.duration_since(p.down) < opts.double_press
        {
            p.done += 1;
            if p.done == 2 {
                out.push(Out::Action(Action::Hints));
            } else {
                self.ctrl_presses = Some(p);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPTS: Options = Options { command_key: true, caps_lock_control: true, tap_command_find: true, hints: true, double_press: Duration::from_millis(400) };
    const C: u32 = 46;

    struct Rig {
        k: Keymap,
        t: Instant,
        terminal: bool,
    }

    impl Rig {
        fn new() -> Self {
            let mut k = Keymap::default();
            k.set_bindings(olo_protocol::shortcuts::bindings(&Default::default()));
            Rig { k, t: Instant::now(), terminal: false }
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
    fn option_tab_is_ctrl_tab() {
        let mut r = Rig::new();
        assert_eq!(r.down(LEFTALT), [Key(LEFTALT, true)]);
        // Ctrl goes down before ⌥ comes up, so ⌥ was never tapped alone.
        assert_eq!(r.down(TAB), [Key(LEFTCTRL, true), Key(LEFTALT, false), Key(TAB, true)]);
        assert_eq!(r.up(TAB), [Key(TAB, false), Key(LEFTCTRL, false)]);
        // Again, ⌥ still held.
        assert_eq!(r.down(TAB), [Key(LEFTCTRL, true), Key(TAB, true)]);
        assert_eq!(r.up(TAB), [Key(TAB, false), Key(LEFTCTRL, false)]);
        // ⇧ as well: the previous tab.
        assert_eq!(r.down(LEFTSHIFT), [Key(LEFTSHIFT, true)]);
        assert_eq!(r.down(TAB), [Key(LEFTCTRL, true), Key(TAB, true)]);
        assert_eq!(r.up(TAB), [Key(TAB, false), Key(LEFTCTRL, false)]);
        assert_eq!(r.up(LEFTSHIFT), [Key(LEFTSHIFT, false)]);
        // ⌥ is already up for the app.
        assert_eq!(r.up(LEFTALT), []);
    }

    #[test]
    fn shift_option_tab_is_ctrl_shift_tab() {
        let mut r = Rig::new();
        r.down(LEFTSHIFT);
        r.down(RIGHTALT);
        assert_eq!(r.down(TAB), [Key(LEFTCTRL, true), Key(RIGHTALT, false), Key(TAB, true)]);
        assert_eq!(r.up(TAB), [Key(TAB, false), Key(LEFTCTRL, false)]);
    }

    #[test]
    fn option_comes_back_after_option_tab() {
        const F: u32 = 33;
        let mut r = Rig::new();
        r.down(LEFTALT);
        r.down(TAB);
        r.up(TAB);
        // ⌥F is still ⌥F.
        assert_eq!(r.down(F), [Key(LEFTALT, true), Key(F, true)]);
        assert_eq!(r.up(F), [Key(F, false)]);
        assert_eq!(r.up(LEFTALT), [Key(LEFTALT, false)]);
        // Let go of first: nothing comes back.
        r.down(LEFTALT);
        r.down(TAB);
        r.up(TAB);
        assert_eq!(r.up(LEFTALT), []);
        assert_eq!(r.down(F), [Key(F, true)]);
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
        assert_eq!(r.up(LEFTMETA), [Key(RIGHTCTRL, false), Out::Action(Action::Palette)]);
        r.down(LEFTMETA);
        r.down(C);
        r.up(C);
        assert_eq!(r.up(LEFTMETA), [Key(RIGHTCTRL, false)]);
        // Held too long: not a tap.
        r.down(LEFTMETA);
        r.t += TAP;
        assert_eq!(r.up(LEFTMETA), [Key(RIGHTCTRL, false)]);
        // A ⌘-click (⌘-dragging a tab) in between: not a tap either.
        r.down(LEFTMETA);
        assert!(r.k.command_held());
        r.k.command_used();
        assert_eq!(r.up(LEFTMETA), [Key(RIGHTCTRL, false)]);
        assert!(!r.k.command_held());
    }

    #[test]
    fn control_pressed_twice_shows_hints() {
        let hints = |outs: Vec<Out>| outs.contains(&Out::Action(Action::Hints));
        let mut r = Rig::new();
        r.down(LEFTCTRL);
        assert!(!hints(r.up(LEFTCTRL)));
        r.down(LEFTCTRL);
        assert!(hints(r.up(LEFTCTRL)));
        // Either Control, or Caps Lock as one.
        r.t += Duration::from_secs(1);
        r.down(CAPSLOCK);
        r.up(CAPSLOCK);
        r.down(RIGHTCTRL);
        assert!(hints(r.up(RIGHTCTRL)));
        // A third press starts again.
        r.down(LEFTCTRL);
        assert!(!hints(r.up(LEFTCTRL)));
        // Too slow between the presses: the second is a first.
        r.t += Duration::from_millis(400);
        r.down(LEFTCTRL);
        assert!(!hints(r.up(LEFTCTRL)));
        r.down(LEFTCTRL);
        assert!(hints(r.up(LEFTCTRL)));
        // Held too long: not a press.
        r.t += Duration::from_secs(1);
        r.down(LEFTCTRL);
        r.up(LEFTCTRL);
        r.down(LEFTCTRL);
        r.t += Duration::from_millis(400);
        assert!(!hints(r.up(LEFTCTRL)));
        // Something typed with it (^C, then Control): not a double press.
        r.t += Duration::from_secs(1);
        r.down(LEFTCTRL);
        r.down(C);
        r.up(C);
        r.up(LEFTCTRL);
        r.down(LEFTCTRL);
        assert!(!hints(r.up(LEFTCTRL)));
        // A key between the presses spoils it too.
        r.t += Duration::from_secs(1);
        r.down(LEFTCTRL);
        r.up(LEFTCTRL);
        r.down(C);
        r.up(C);
        r.down(LEFTCTRL);
        assert!(!hints(r.up(LEFTCTRL)));
        // With Shift or ⌘ down: no.
        r.t += Duration::from_secs(1);
        r.down(LEFTSHIFT);
        r.down(LEFTCTRL);
        r.up(LEFTCTRL);
        r.down(LEFTCTRL);
        assert!(!hints(r.up(LEFTCTRL)));
        r.up(LEFTSHIFT);
        // The speed is a setting.
        let slow = Options { double_press: Duration::from_millis(800), ..OPTS };
        let mut k = Keymap::default();
        let t = Instant::now();
        k.feed(LEFTCTRL, true, t, slow, false);
        k.feed(LEFTCTRL, false, t + Duration::from_millis(100), slow, false);
        k.feed(LEFTCTRL, true, t + Duration::from_millis(600), slow, false);
        assert!(hints(k.feed(LEFTCTRL, false, t + Duration::from_millis(700), slow, false)));
        // Off in the settings: nothing.
        let off = Options { hints: false, ..OPTS };
        let mut k = Keymap::default();
        for _ in 0..2 {
            k.feed(LEFTCTRL, true, t, off, false);
            assert!(!hints(k.feed(LEFTCTRL, false, t, off, false)));
        }
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
    fn twitcher_arrows_letters_and_esc() {
        let mut r = Rig::new();
        r.down(LEFTMETA);
        // Not switching yet: ⌘← is Home.
        assert!(!r.down(LEFT).contains(&Out::Action(Action::SwitchMove { dx: -1, dy: 0 })));
        r.up(LEFT);
        r.down(TAB);
        r.up(TAB);
        assert_eq!(r.down(DOWN), [Out::Action(Action::SwitchMove { dx: 0, dy: 1 })]);
        assert_eq!(r.up(DOWN), []);
        assert_eq!(r.down(LEFT), [Out::Action(Action::SwitchMove { dx: -1, dy: 0 })]);
        r.up(LEFT);
        // Letters only with hints on.
        let off = Options { hints: false, ..OPTS };
        assert!(!r.k.feed(A, true, r.t, off, false).iter().any(|o| matches!(o, Out::Action(_))));
        r.k.feed(A, false, r.t, off, false);
        assert_eq!(r.down(A), [Out::Action(Action::SwitchPick('a'))]);
        r.up(A);
        assert_eq!(r.down(ESC), [Out::Action(Action::SwitchCancel)]);
        r.up(ESC);
        // Cancelled: letting go of ⌘ commits nothing.
        assert_eq!(r.up(LEFTMETA), [Key(RIGHTCTRL, false)]);
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
        assert_eq!(r.down(Q), [Out::Action(Action::Shortcut(Shortcut::Lock))]);
        assert_eq!(r.up(Q), []);
        assert_eq!(r.down(A), [Out::Action(Action::Shortcut(Shortcut::Caffeinate))]);
        assert_eq!(r.up(A), []);
        r.up(LEFTMETA);
        // Physical Ctrl and Caps Lock share the left Ctrl.
        assert_eq!(r.down(LEFTCTRL), []);
        assert_eq!(r.up(CAPSLOCK), []);
        assert_eq!(r.up(LEFTCTRL), [Key(LEFTCTRL, false)]);
    }

    #[test]
    fn screenshots() {
        let mut r = Rig::new();
        r.down(LEFTMETA);
        r.down(LEFTSHIFT);
        assert_eq!(r.down(KEY3), [Out::Action(Action::Shortcut(Shortcut::ScreenshotScreen))]);
        assert_eq!(r.up(KEY3), []);
        assert_eq!(r.down(KEY4), [Out::Action(Action::Shortcut(Shortcut::ScreenshotRegion))]);
        r.up(KEY4);
        assert_eq!(r.down(KEY5), [Out::Action(Action::Shortcut(Shortcut::ScreenshotApp))]);
        r.up(KEY5);
        r.up(LEFTSHIFT);
        // ⌘3 without Shift is the app's.
        assert_eq!(r.down(KEY3), [Key(KEY3, true)]);
        r.up(KEY3);
        r.up(LEFTMETA);
        assert_eq!(r.down(SYSRQ), [Out::Action(Action::Shortcut(Shortcut::ScreenshotScreen))]);
        assert_eq!(r.up(SYSRQ), []);
        r.down(LEFTALT);
        assert_eq!(r.down(SYSRQ), [Out::Action(Action::Shortcut(Shortcut::ScreenshotWindow))]);
    }

    #[test]
    fn tiling_and_find() {
        use olo_protocol::Tile;
        let mut r = Rig::new();
        r.down(LEFTMETA);
        r.down(LEFTSHIFT);
        assert_eq!(r.down(LEFT), [Out::Action(Action::Shortcut(Shortcut::Tile(Tile::Left)))]);
        assert_eq!(r.up(LEFT), []);
        assert_eq!(r.down(RIGHT), [Out::Action(Action::Shortcut(Shortcut::Tile(Tile::Right)))]);
        r.up(RIGHT);
        r.up(LEFTSHIFT);
        assert_eq!(r.down(SPACE), [Out::Action(Action::Shortcut(Shortcut::Palette))]);
        r.up(SPACE);
        // ⌘M hides the window in front, whatever the app.
        assert_eq!(r.down(50), [Out::Action(Action::Shortcut(Shortcut::Hide))]);
        assert_eq!(r.up(50), []);
        // ⌘← is still Home.
        assert_eq!(r.down(LEFT), [Key(RIGHTCTRL, false), Key(HOME, true)]);
        r.up(LEFT);
        // A tap of ⌘ alone is nothing unless chosen.
        r.up(LEFTMETA);
        r.down(LEFTMETA);
        let off = Options { tap_command_find: false, ..OPTS };
        r.t += Duration::from_millis(10);
        assert_eq!(r.k.feed(LEFTMETA, false, r.t, off, false), [Key(RIGHTCTRL, false)]);
    }

    #[test]
    fn records_a_shortcut() {
        let mut r = Rig::new();
        r.k.record(true);
        r.down(LEFTCTRL);
        r.down(LEFTMETA);
        // Not acted on (Ctrl+⌘Q locks), just recorded.
        let lock_keys = r.down(Q);
        assert_eq!(lock_keys, [Out::Recorded(Some(Some("Ctrl+Cmd+Q".parse().unwrap())))]);
        assert_eq!(r.up(Q), []);
        r.up(LEFTMETA);
        r.up(LEFTCTRL);
        r.k.record(true);
        assert_eq!(r.down(ESC), [Out::Recorded(None)]);
        r.up(ESC);
        r.k.record(true);
        assert_eq!(r.down(BACKSPACE), [Out::Recorded(Some(None))]);
        r.up(BACKSPACE);
        // Recording ends with the one key.
        assert_eq!(r.down(C), [Key(C, true)]);
    }

    #[test]
    fn disabled_passes_through() {
        let mut k = Keymap::default();
        let off = Options { command_key: false, caps_lock_control: false, tap_command_find: false, hints: false, double_press: Duration::from_millis(400) };
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
