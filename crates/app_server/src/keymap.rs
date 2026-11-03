//! The keyboard as the profile has it (olo_protocol::shortcuts::KeyboardProfile:
//! Mac, Windows or BeOS). The command key (⌘ on a Mac, Alt on BeOS) reaches
//! apps as Ctrl, so ⌘C copies everywhere, GTK and Firefox included; on
//! Windows the command key is Ctrl itself and nothing is changed. First of
//! all app_server takes the profile's shortcuts (olo_protocol::shortcuts:
//! tiling, workspaces, Twitcher, Find, lock, screenshots, …): Twitcher's
//! (⌘Tab, Alt+Tab, Ctrl+Tab) stay up while their modifiers are held, Shift
//! going back. A tap of the Super key alone opens the command palette, if
//! the profile says so (the Windows key), and Control pressed twice
//! quickly shows keyboard hints, if chosen. Caps Lock is another Ctrl if
//! chosen. Terminals keep physical Ctrl for the tty, so the command key
//! reaches them as Ctrl+Shift. Next / Previous Tab (⌃Tab, ⌃⇧Tab, or
//! whatever they are set to) reach the app as Ctrl+Tab / Ctrl+Shift+Tab.
//!
//! With ⌘ as the command key (Mac), the rest of the Mac: ⌘ arrows and
//! ⌘⌫ are line and document navigation (Olo's apps get them as they are,
//! and the kit does that in text fields), and programs that aren't Olo's
//! get the Mac's keys in their own terms ([`Focus`]): ⌥ moves and deletes
//! by word (Ctrl there), ⌘[ / ⌘] go back and forward (Alt+← / Alt+→:
//! Chrome, Slack, GTK's Files), and in browsers ⌘⇧[ / ⌘⇧] change tabs.
//! ⌘← / ⌘→ never navigate: they are the line's start and end everywhere.
//! With Alt (BeOS), Alt arrows are Ctrl arrows already (a word, the
//! document, as on BeOS), the Super key is Option (Alt for apps), and
//! the left Alt alone is the command key (the right one stays AltGr).
//!
//! This sits between the input devices and the seat: physical evdev codes
//! go in, the codes clients should see (plus app_server actions) come out.
//! It is pure state, so it is tested without a compositor.

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use olo_protocol::{Chord, Shortcut, shortcuts::CommandKey};

/// evdev key codes used here.
pub mod code {
    pub const ESC: u32 = 1;
    pub const KEY3: u32 = 4;
    pub const KEY4: u32 = 5;
    pub const KEY5: u32 = 6;
    pub const BACKSPACE: u32 = 14;
    pub const TAB: u32 = 15;
    pub const Q: u32 = 16;
    pub const LEFTBRACE: u32 = 26;
    pub const RIGHTBRACE: u32 = 27;
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
    pub const PAGEUP: u32 = 104;
    pub const LEFT: u32 = 105;
    pub const RIGHT: u32 = 106;
    pub const END: u32 = 107;
    pub const DOWN: u32 = 108;
    pub const PAGEDOWN: u32 = 109;
    pub const DELETE: u32 = 111;
    pub const LEFTMETA: u32 = 125;
    pub const RIGHTMETA: u32 = 126;
}
use code::*;

/// A Super press and release with nothing in between, shorter than this,
/// is a tap (the command palette).
pub const TAP: Duration = Duration::from_millis(350);

/// The virtual Ctrl that stands for the command key. Physical Ctrl
/// (either) and Caps Lock use the left one, so a client that cares (a terminal) can tell
/// them apart; keys are reference-counted, so sharing is harmless.
const CMD_CTRL: u32 = RIGHTCTRL;

/// What has the keyboard, as far as the keys it should see go.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Focus {
    /// Olo's own apps (and Deskbar, the desktop), which know the Mac
    /// keyboard themselves: ⌥← is a word back there already.
    #[default]
    Olo,
    /// A terminal (`terminal_apps`): ⌘ reaches it as Ctrl+Shift.
    Terminal,
    /// A web browser (`browser_apps`): as `Other`, and ⌘⇧[ / ⌘⇧] and
    /// ⌘⌥← / ⌘⌥→ are the tab before / after.
    Browser,
    /// Any other program (GTK, Qt, Electron): ⌥←/⌥→ and ⌥⌫/⌥⌦ move and
    /// delete by word, which is Ctrl there (Alt+← is Back), and ⌘[ / ⌘]
    /// are Back / Forward, which is Alt+← / Alt+→ there.
    Other,
}

/// Modifier pairs, the one pressed for a chord first.
const CTRL: [u32; 2] = [LEFTCTRL, RIGHTCTRL];
const SHIFT: [u32; 2] = [LEFTSHIFT, RIGHTSHIFT];
const ALT: [u32; 2] = [LEFTALT, RIGHTALT];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Switch Applications (⌘Tab; `back` with Shift): the next app in
    /// Twitcher, on any workspace (`all`); Switch Applications Here
    /// (⌥Tab): on those showing.
    NextApp { back: bool, all: bool },
    /// Switch Windows (⌘\`): the next window of the focused app.
    NextWindow { back: bool },
    /// The switcher's modifiers released: commit Twitcher's choice.
    SwitchDone,
    /// An arrow while switching: along the row (←/→) or to another
    /// workspace's row (↑/↓).
    SwitchMove { dx: i32, dy: i32 },
    /// A letter while switching, with keyboard hints on: the cell it marks.
    SwitchPick(char),
    /// Esc while switching: everything back as it was.
    SwitchCancel,
    /// A tap of the Super key (if chosen), or keyd's spelling of one.
    Palette,
    /// Control pressed twice quickly (if chosen): keyboard hints.
    Hints,
    /// Control pressed and let go once on its own (keyboard hints
    /// chosen): the second time may well follow.
    HintsAhead,
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
    /// A chord another program took (`Request::GrabKeys`) went down or up.
    Grabbed(Chord, bool),
}

#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// The key apps take as the command key (they see Ctrl).
    pub command: CommandKey,
    pub caps_lock_control: bool,
    /// A tap of the Super key on its own opens the command palette.
    pub palette_on_tap: bool,
    pub hints: bool,
    /// How quickly a double press must come: the second press within this
    /// of the first, each press shorter than it.
    pub double_press: Duration,
}

impl From<&olo_protocol::KeyboardSettings> for Options {
    fn from(k: &olo_protocol::KeyboardSettings) -> Self {
        Options {
            command: k.profile.command_key,
            caps_lock_control: k.profile.caps_lock_control,
            palette_on_tap: k.profile.palette_on_tap,
            hints: k.hints,
            double_press: Duration::from_millis(k.double_press_ms.clamp(100, 1000) as u64),
        }
    }
}

impl Options {
    /// The Mac's keys: ⌘ is the command key.
    fn mac(self) -> bool {
        self.command == CommandKey::Command
    }
}

/// The physical command key.
fn command_key(key: u32, opts: Options) -> bool {
    match opts.command {
        CommandKey::Command => matches!(key, LEFTMETA | RIGHTMETA),
        CommandKey::Alt => key == LEFTALT,
        CommandKey::Control => false,
    }
}

fn super_key(key: u32) -> bool {
    matches!(key, LEFTMETA | RIGHTMETA)
}

/// What a physical key that is down produced, so its release undoes exactly
/// that, whatever has changed since.
#[derive(Debug, Default)]
struct Held {
    /// Emitted keys to release, in order.
    release: Vec<u32>,
    /// It is a command key.
    cmd: bool,
    /// It is a Super key.
    sup: bool,
    /// It lifted ⌘'s Ctrl (⌘← is Home, not Ctrl+Home): put it back after.
    restore_ctrl: bool,
    /// It pressed another program's grabbed chord: its release goes there.
    grab: Option<Chord>,
}

#[derive(Debug, Default)]
pub struct Keymap {
    held: HashMap<u32, Held>,
    /// How many holders each emitted key has.
    down: HashMap<u32, u32>,
    /// Command keys physically down.
    cmd_keys: u32,
    /// The command key's Ctrl is currently pressed.
    cmd_ctrl: bool,
    /// Super keys physically down, for a tap.
    super_keys: u32,
    /// Something was typed with Super (so releasing it is not a tap).
    super_used: bool,
    super_since: Option<Instant>,
    /// Twitcher is up, while its modifiers are held.
    switching: Option<Switching>,
    /// The shortcuts (from the settings).
    bindings: Vec<(Chord, Shortcut)>,
    /// Chords other programs took (their global shortcuts).
    grabs: Vec<Chord>,
    /// The next key is a shortcut being recorded.
    recording: bool,
    /// Control going down and up on its own, for a double press
    /// (keyboard hints).
    ctrl_presses: Option<CtrlPresses>,
    /// Modifiers let go of for a key that stood for another chord (⌥ for
    /// Next Tab set to ⌥Tab, which is Ctrl+Tab), kept up until another
    /// key needs them.
    lifted: Vec<u32>,
}

/// Modifiers physically held (Shift aside).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Mods {
    ctrl: bool,
    alt: bool,
    cmd: bool,
}

/// What Twitcher was brought up with, and is held for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Switching {
    /// The modifiers of the shortcut (⌘ for ⌘Tab): letting go of one
    /// commits.
    hold: Mods,
    /// Its key (Tab): again for the next, with Shift the one before.
    key: u32,
    action: Shortcut,
}

/// One of Twitcher's shortcuts.
fn switcher(s: Shortcut) -> bool {
    matches!(s, Shortcut::SwitchApps | Shortcut::SwitchAppsHere | Shortcut::SwitchWindows)
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
pub(crate) fn ctrl_key(key: u32, opts: Options) -> bool {
    matches!(key, LEFTCTRL | RIGHTCTRL) || (opts.caps_lock_control && key == CAPSLOCK)
}

impl Keymap {
    /// Feed one physical key, for `focus`.
    pub fn feed(&mut self, key: u32, pressed: bool, now: Instant, opts: Options, focus: Focus) -> Vec<Out> {
        let mut out = Vec::new();
        if pressed {
            self.press(key, now, opts, focus, &mut out);
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
            Out::Action(_) | Out::Recorded(_) | Out::Grabbed(..) => 0,
        });
        // A taken chord still down is let go of too.
        out.extend(self.held.values().filter_map(|h| h.grab).map(|c| Out::Grabbed(c, false)));
        if self.switching.is_some() {
            out.push(Out::Action(Action::SwitchDone));
        }
        let (bindings, grabs, recording) = (std::mem::take(&mut self.bindings), std::mem::take(&mut self.grabs), self.recording);
        *self = Keymap { bindings, grabs, recording, ..Keymap::default() };
        out
    }

    pub fn set_bindings(&mut self, bindings: Vec<(Chord, Shortcut)>) {
        self.bindings = bindings;
    }

    /// The chords other programs have taken.
    pub fn set_grabs(&mut self, grabs: Vec<Chord>) {
        self.grabs = grabs;
    }

    /// Whether `chord` is one of app_server's own shortcuts.
    pub fn is_bound(&self, chord: Chord) -> bool {
        self.bindings.iter().any(|(c, _)| *c == chord)
    }

    /// Take the next key as a shortcut being recorded (or stop).
    pub fn record(&mut self, on: bool) {
        self.recording = on;
    }

    /// A command key is held.
    pub fn command_held(&self) -> bool {
        self.cmd_keys > 0
    }

    /// Something was done with a modifier held other than typing (a
    /// click, a ⌘-drag of a tab): letting go of Super is no tap.
    pub fn command_used(&mut self) {
        if self.super_keys > 0 {
            self.super_used = true;
        }
    }

    pub fn recording(&self) -> bool {
        self.recording
    }

    /// The shortcut `key` makes with the modifiers held now, if any.
    pub fn shortcut_for(&self, key: u32) -> Option<Shortcut> {
        let chord = self.chord(key);
        self.bindings.iter().find(|(c, _)| *c == chord).map(|&(_, s)| s)
    }

    /// The key with the modifiers held now (physically: Cmd is Super, Alt
    /// is Alt, whichever is the command key).
    fn chord(&self, key: u32) -> Chord {
        Chord { ctrl: self.ctrl(), alt: self.alt(), shift: self.shift(), cmd: self.is_held(&[LEFTMETA, RIGHTMETA]), key }
    }

    fn mods(&self) -> Mods {
        let c = self.chord(0);
        Mods { ctrl: c.ctrl, alt: c.alt, cmd: c.cmd }
    }

    /// Every modifier of `hold` is still down.
    fn holding(&self, hold: Mods) -> bool {
        let now = self.mods();
        (!hold.ctrl || now.ctrl) && (!hold.alt || now.alt) && (!hold.cmd || now.cmd)
    }

    /// Option: ⌥ on a Mac, the Super key on BeOS.
    fn option(&self, opts: Options) -> bool {
        match opts.command {
            CommandKey::Command => self.alt(),
            CommandKey::Alt => self.is_held(&[LEFTMETA, RIGHTMETA]),
            CommandKey::Control => false,
        }
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

    /// `h`'s key stands for `to` with the modifiers in `with` (Ctrl+Tab,
    /// Alt+←): the modifiers clients see become just those. They go down
    /// before the others are let go of, and those stay up until another
    /// key needs them (`restore_lifted`), so a ⌥ chord never looks like a
    /// tap of ⌥ (Firefox shows its menu bar on one). ⌘'s Ctrl, when it isn't
    /// wanted, comes back as this key is let go of.
    fn stand_for(&mut self, h: &mut Held, with: &[[u32; 2]], to: u32, out: &mut Vec<Out>) {
        let wanted = |k: u32| with.iter().any(|pair| pair.contains(&k));
        if !wanted(CMD_CTRL) {
            self.lift_ctrl(h, out);
        }
        let mut pressed = Vec::new();
        for pair in with {
            if !pair.iter().any(|k| self.down.contains_key(k)) {
                pressed.push(pair[0]);
            }
        }
        for &k in &pressed {
            self.emit_press(k, out);
        }
        for k in MODIFIERS.into_iter().filter(|&k| !wanted(k)) {
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

    /// The chord a key is in `focus`'s terms, if it isn't itself there:
    /// the modifiers wanted and the key. (Only with a command key that
    /// isn't Ctrl: on Windows, every program has the same keys already.)
    fn translate(&self, key: u32, focus: Focus, opts: Options) -> Option<(&'static [[u32; 2]], u32)> {
        if opts.command == CommandKey::Control {
            return None;
        }
        let (cmd, ctrl, opt, shift) = (self.cmd_keys > 0, self.ctrl(), self.option(opts), self.shift());
        let foreign = matches!(focus, Focus::Browser | Focus::Other);
        let word = opts.mac() && foreign && opt && !cmd && !ctrl;
        let browser = focus == Focus::Browser && cmd && !ctrl;
        let history = foreign && cmd && !ctrl && !opt && !shift;
        Some(match key {
            // A word back or on (selecting with ⇧), or deleted.
            LEFT | RIGHT if word && shift => (&[CTRL, SHIFT], key),
            LEFT | RIGHT | BACKSPACE | DELETE if word => (&[CTRL], key),
            // Back and Forward.
            LEFTBRACE if history => (&[ALT], LEFT),
            RIGHTBRACE if history => (&[ALT], RIGHT),
            // The tab before and after.
            LEFTBRACE if browser && !opt => (&[CTRL], PAGEUP),
            RIGHTBRACE if browser && !opt => (&[CTRL], PAGEDOWN),
            LEFT if browser && opt && !shift => (&[CTRL], PAGEUP),
            RIGHT if browser && opt && !shift => (&[CTRL], PAGEDOWN),
            _ => return None,
        })
    }

    fn press(&mut self, key: u32, now: Instant, opts: Options, focus: Focus, out: &mut Vec<Out>) {
        let terminal = focus == Focus::Terminal;
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
        if super_key(key) {
            self.super_keys += 1;
            if self.super_keys == 1 {
                self.super_used = false;
                self.super_since = Some(now);
            }
        } else if self.super_keys > 0 {
            self.super_used = true;
        }
        let mut h = Held { sup: super_key(key), ..Held::default() };
        if command_key(key, opts) {
            self.cmd_keys += 1;
            if !self.cmd_ctrl {
                self.cmd_ctrl = true;
                self.emit_press(CMD_CTRL, out);
            }
            h.cmd = true;
            self.held.insert(key, h);
            return;
        }
        if super_key(key) && opts.command == CommandKey::Alt {
            // BeOS's Option, Alt for apps.
            self.emit_press(LEFTALT, out);
            h.release.push(LEFTALT);
            self.held.insert(key, h);
            return;
        }
        if opts.caps_lock_control && key == CAPSLOCK {
            self.emit_press(LEFTCTRL, out);
            h.release.push(LEFTCTRL);
            self.held.insert(key, h);
            return;
        }
        if opts.command != CommandKey::Control && key == RIGHTCTRL {
            // The right Ctrl clients see is the command key's alone, so a
            // terminal can tell ⌘- (Smaller) from the shell's Ctrl+_ (the
            // kit's `theme_watch::command_key`).
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
        let chord = self.chord(key);
        if let Some(sw) = self.switching.filter(|sw| self.holding(sw.hold)) {
            // Twitcher is up: its key again (with Shift, back), and with
            // just its modifiers the arrows, Esc and the hints' letters.
            let alone = self.mods() == sw.hold;
            let action = match key {
                _ if key == sw.key => Some(self.switch_step(sw.action, chord.shift)),
                LEFT if alone => Some(Action::SwitchMove { dx: -1, dy: 0 }),
                RIGHT if alone => Some(Action::SwitchMove { dx: 1, dy: 0 }),
                UP if alone => Some(Action::SwitchMove { dx: 0, dy: -1 }),
                DOWN if alone => Some(Action::SwitchMove { dx: 0, dy: 1 }),
                ESC if alone => {
                    self.switching = None;
                    Some(Action::SwitchCancel)
                }
                _ if alone && opts.hints && !chord.shift => crate::hints::letter(key).map(Action::SwitchPick),
                _ => None,
            };
            if let Some(action) = action {
                out.push(Out::Action(action));
                self.held.insert(key, h);
                return;
            }
        }
        let mut shortcut = self.bindings.iter().find(|(c, _)| *c == chord).map(|&(_, s)| s);
        let mut back = false;
        if shortcut.is_none() && chord.shift {
            // A switcher's keys with Shift: the one before.
            let plain = Chord { shift: false, ..chord };
            if let Some(&(_, s)) = self.bindings.iter().find(|(c, s)| *c == plain && switcher(*s)) {
                (shortcut, back) = (Some(s), true);
            }
        }
        if let Some(s) = shortcut.filter(|s| switcher(*s)) {
            // Up while its modifiers are held; the app keeps them down, as
            // for any switcher, and loses the focus to the window chosen.
            let hold = Mods { ctrl: chord.ctrl, alt: chord.alt, cmd: chord.cmd };
            out.push(Out::Action(self.switch_step(s, back)));
            if hold == Mods::default() {
                // Nothing to hold: just the one step.
                out.push(Out::Action(Action::SwitchDone));
            } else {
                self.switching = Some(Switching { hold, key, action: s });
            }
            self.held.insert(key, h);
            return;
        }
        // Another program's global shortcut (`GrabKeys`): its press and
        // release go to that program, not to the focused window.
        if shortcut.is_none() && self.grabs.contains(&chord) {
            out.push(Out::Grabbed(chord, true));
            h.grab = Some(chord);
            self.held.insert(key, h);
            return;
        }
        let tab = matches!(shortcut, Some(Shortcut::NextTab | Shortcut::PreviousTab));
        let translated = if shortcut.is_none() { self.translate(key, focus, opts) } else { None };
        // (⌥ stays up through a run of ⌥← words, as through Next Tabs.)
        if !tab && translated.is_none() && !olo_protocol::shortcuts::MODIFIER_KEYS.contains(&key) {
            self.restore_lifted(out);
        }
        if let Some((with, to)) = translated {
            self.stand_for(&mut h, with, to, out);
            self.held.insert(key, h);
            return;
        }
        if let Some(shortcut) = shortcut {
            if tab {
                // What every toolkit takes for the next / previous tab.
                let with: &[[u32; 2]] = if shortcut == Shortcut::PreviousTab { &[CTRL, SHIFT] } else { &[CTRL] };
                self.stand_for(&mut h, with, TAB, out);
            } else {
                out.push(Out::Action(Action::Shortcut(shortcut)));
            }
            self.held.insert(key, h);
            return;
        }
        if self.cmd_keys > 0 {
            let (shift, ctrl) = (self.shift(), self.ctrl());
            // Olo's apps take ⌘ arrows and ⌘⌫ as ⌘ chords (Open Parent,
            // Move to Trash), and as the line's keys in a text field (the
            // kit's `mac_text_keys`).
            let mac = opts.mac() && focus != Focus::Olo;
            let handled = match key {
                // keyd's spelling of a ⌘ tap (Ctrl+Alt+Super+O), in case it
                // is still running underneath.
                O if opts.mac() && ctrl && self.alt() => {
                    out.push(Out::Action(Action::Palette));
                    true
                }
                LEFT | RIGHT if mac && !ctrl => {
                    // Start and end of the line.
                    self.lift_ctrl(&mut h, out);
                    let to = if key == LEFT { HOME } else { END };
                    self.emit_press(to, out);
                    h.release.push(to);
                    true
                }
                UP | DOWN if mac && !ctrl => {
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
                BACKSPACE if mac && !ctrl => {
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
                // (BeOS: the command key's arrows and ⌫ are Ctrl's, a word
                // in the shell too.)
                LEFT | RIGHT | UP | DOWN | HOME | END | PAGEUP | PAGEDOWN | BACKSPACE | DELETE if !opts.mac() => false,
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

    /// Twitcher's step for one of its shortcuts.
    fn switch_step(&self, s: Shortcut, back: bool) -> Action {
        match s {
            Shortcut::SwitchWindows => Action::NextWindow { back },
            Shortcut::SwitchAppsHere => Action::NextApp { back, all: false },
            _ => Action::NextApp { back, all: true },
        }
    }

    fn release(&mut self, key: u32, now: Instant, opts: Options, out: &mut Vec<Out>) {
        let Some(h) = self.held.remove(&key) else {
            // Pressed before we were watching: pass it through.
            out.push(Out::Key(key, false));
            return;
        };
        if let Some(chord) = h.grab {
            out.push(Out::Grabbed(chord, false));
        }
        for k in h.release {
            self.emit_release(k, out);
        }
        if h.cmd {
            self.cmd_keys = self.cmd_keys.saturating_sub(1);
            if self.cmd_keys == 0 && self.cmd_ctrl {
                self.cmd_ctrl = false;
                self.emit_release(CMD_CTRL, out);
            }
        } else if h.restore_ctrl && self.cmd_keys > 0 && !self.cmd_ctrl {
            self.cmd_ctrl = true;
            self.emit_press(CMD_CTRL, out);
        }
        if let Some(sw) = self.switching
            && !self.holding(sw.hold)
        {
            self.switching = None;
            out.push(Out::Action(Action::SwitchDone));
        }
        if h.sup {
            self.super_keys = self.super_keys.saturating_sub(1);
            if self.super_keys == 0 {
                let tap = !self.super_used && self.super_since.is_some_and(|t| now.duration_since(t) < TAP);
                if tap && opts.palette_on_tap {
                    out.push(Out::Action(Action::Palette));
                }
                self.super_since = None;
            }
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
                out.push(Out::Action(Action::HintsAhead));
                self.ctrl_presses = Some(p);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use olo_protocol::shortcuts::{KeyboardProfile, Platform};

    const OPTS: Options = Options { command: CommandKey::Command, caps_lock_control: true, palette_on_tap: true, hints: true, double_press: Duration::from_millis(400) };
    const C: u32 = 46;
    const MINUS: u32 = 12;
    const F9: u32 = 67;

    struct Rig {
        k: Keymap,
        t: Instant,
        focus: Focus,
        opts: Options,
    }

    impl Rig {
        fn new() -> Self {
            Rig::with(&KeyboardProfile::default())
        }
        /// With a profile's keys and command key (the Mac's otherwise).
        fn with(profile: &KeyboardProfile) -> Self {
            let mut k = Keymap::default();
            k.set_bindings(olo_protocol::shortcuts::bindings(profile));
            let opts = Options { command: profile.command_key, palette_on_tap: true, ..OPTS };
            Rig { k, t: Instant::now(), focus: Focus::Olo, opts }
        }
        fn down(&mut self, key: u32) -> Vec<Out> {
            self.t += Duration::from_millis(10);
            self.k.feed(key, true, self.t, self.opts, self.focus)
        }
        fn up(&mut self, key: u32) -> Vec<Out> {
            self.t += Duration::from_millis(10);
            self.k.feed(key, false, self.t, self.opts, self.focus)
        }
    }

    use Out::Key;

    #[test]
    fn grabbed_chords_go_to_their_program() {
        let mut r = Rig::new();
        let push = "Ctrl+Alt+F9".parse::<Chord>().unwrap();
        r.k.set_grabs(vec![push]);
        r.focus = Focus::Other;
        r.down(LEFTCTRL);
        r.down(LEFTALT);
        // Its press and release go to the program, not the focused window.
        assert_eq!(r.down(F9), [Out::Grabbed(push, true)]);
        assert_eq!(r.up(F9), [Out::Grabbed(push, false)]);
        r.up(LEFTALT);
        r.up(LEFTCTRL);
        // Other chords type as ever.
        assert_eq!(r.down(F9), [Key(F9, true)]);
        r.up(F9);
        // Still down when the keyboard resets: let go of.
        r.down(LEFTCTRL);
        r.down(LEFTALT);
        r.down(F9);
        assert!(r.k.reset().contains(&Out::Grabbed(push, false)));
    }

    #[test]
    fn command_is_ctrl() {
        let mut r = Rig::new();
        assert_eq!(r.down(LEFTMETA), [Key(RIGHTCTRL, true)]);
        assert_eq!(r.down(C), [Key(C, true)]);
        assert_eq!(r.up(C), [Key(C, false)]);
        assert_eq!(r.up(LEFTMETA), [Key(RIGHTCTRL, false)]);
    }

    /// Next / Previous Tab set to ⌥Tab / ⌥⇧Tab, as they once were.
    fn option_tab_for_tabs() -> Rig {
        let mut p = KeyboardProfile::default();
        olo_protocol::shortcuts::assign(&mut p, "next-tab", "Alt+Tab".parse().ok());
        olo_protocol::shortcuts::assign(&mut p, "previous-tab", "Alt+Shift+Tab".parse().ok());
        Rig::with(&p)
    }

    #[test]
    fn control_tab_is_the_next_tab() {
        let mut r = Rig::new();
        r.down(LEFTCTRL);
        assert_eq!(r.down(TAB), [Key(TAB, true)]);
        assert_eq!(r.up(TAB), [Key(TAB, false)]);
        r.down(LEFTSHIFT);
        assert_eq!(r.down(TAB), [Key(TAB, true)]);
        r.up(TAB);
        r.up(LEFTSHIFT);
        // Caps Lock as Ctrl too.
        r.up(LEFTCTRL);
        r.down(CAPSLOCK);
        assert_eq!(r.down(TAB), [Key(TAB, true)]);
    }

    #[test]
    fn option_tab_is_ctrl_tab() {
        let mut r = option_tab_for_tabs();
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
        let mut r = option_tab_for_tabs();
        r.down(LEFTSHIFT);
        r.down(RIGHTALT);
        assert_eq!(r.down(TAB), [Key(LEFTCTRL, true), Key(RIGHTALT, false), Key(TAB, true)]);
        assert_eq!(r.up(TAB), [Key(TAB, false), Key(LEFTCTRL, false)]);
    }

    #[test]
    fn option_comes_back_after_option_tab() {
        const F: u32 = 33;
        let mut r = option_tab_for_tabs();
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
        // (Let go on its own, it may be the first of two for hints.)
        assert_eq!(r.down(CAPSLOCK), [Key(LEFTCTRL, true)]);
        assert_eq!(r.up(CAPSLOCK), [Key(LEFTCTRL, false), Out::Action(Action::HintsAhead)]);
        assert_eq!(r.down(C), [Key(C, true)]);
        assert_eq!(r.up(C), [Key(C, false)]);
        // Physical Ctrl is untouched.
        r.t += Duration::from_secs(1);
        assert_eq!(r.down(LEFTCTRL), [Key(LEFTCTRL, true)]);
        assert_eq!(r.up(LEFTCTRL), [Key(LEFTCTRL, false), Out::Action(Action::HintsAhead)]);
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
        // The first time asks the apps ahead.
        let first = r.up(LEFTCTRL);
        assert!(first.contains(&Out::Action(Action::HintsAhead)) && !hints(first));
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
        k.feed(LEFTCTRL, true, t, slow, Focus::Olo);
        k.feed(LEFTCTRL, false, t + Duration::from_millis(100), slow, Focus::Olo);
        k.feed(LEFTCTRL, true, t + Duration::from_millis(600), slow, Focus::Olo);
        assert!(hints(k.feed(LEFTCTRL, false, t + Duration::from_millis(700), slow, Focus::Olo)));
        // Off in the settings: nothing.
        let off = Options { hints: false, ..OPTS };
        let mut k = Keymap::default();
        for _ in 0..2 {
            k.feed(LEFTCTRL, true, t, off, Focus::Olo);
            assert!(!hints(k.feed(LEFTCTRL, false, t, off, Focus::Olo)));
        }
    }

    #[test]
    fn twitcher() {
        let mut r = Rig::new();
        r.down(LEFTMETA);
        assert_eq!(r.down(TAB), [Out::Action(Action::NextApp { back: false, all: true })]);
        assert_eq!(r.up(TAB), []);
        r.down(LEFTSHIFT);
        assert_eq!(r.down(TAB), [Out::Action(Action::NextApp { back: true, all: true })]);
        r.up(TAB);
        r.up(LEFTSHIFT);
        assert_eq!(r.down(GRAVE), [Out::Action(Action::NextWindow { back: false })]);
        r.up(GRAVE);
        assert_eq!(r.up(LEFTMETA), [Key(RIGHTCTRL, false), Out::Action(Action::SwitchDone)]);
    }

    #[test]
    fn option_tab_is_twitcher_for_this_workspace() {
        let mut r = Rig::new();
        assert_eq!(r.down(LEFTALT), [Key(LEFTALT, true)]);
        // The app keeps ⌥; the Tab is Twitcher's.
        assert_eq!(r.down(TAB), [Out::Action(Action::NextApp { back: false, all: false })]);
        assert_eq!(r.up(TAB), []);
        r.down(LEFTSHIFT);
        assert_eq!(r.down(TAB), [Out::Action(Action::NextApp { back: true, all: false })]);
        r.up(TAB);
        r.up(LEFTSHIFT);
        // While ⌥ is held: the arrows, Esc.
        assert_eq!(r.down(DOWN), [Out::Action(Action::SwitchMove { dx: 0, dy: 1 })]);
        r.up(DOWN);
        // Letting go of ⌥ commits.
        assert_eq!(r.up(LEFTALT), [Key(LEFTALT, false), Out::Action(Action::SwitchDone)]);
        // ⌥ again, without Tab: just ⌥.
        r.down(RIGHTALT);
        assert_eq!(r.down(DOWN), [Key(DOWN, true)]);
        r.up(DOWN);
        assert_eq!(r.up(RIGHTALT), [Key(RIGHTALT, false)]);
        // Both ⌥ keys: done when the last is let go of.
        r.down(LEFTALT);
        r.down(RIGHTALT);
        r.down(TAB);
        r.up(TAB);
        assert_eq!(r.up(LEFTALT), [Key(LEFTALT, false)]);
        assert_eq!(r.up(RIGHTALT), [Key(RIGHTALT, false), Out::Action(Action::SwitchDone)]);
        // Esc: cancelled, nothing to commit.
        r.down(LEFTALT);
        r.down(TAB);
        r.up(TAB);
        assert_eq!(r.down(ESC), [Out::Action(Action::SwitchCancel)]);
        r.up(ESC);
        assert_eq!(r.up(LEFTALT), [Key(LEFTALT, false)]);
    }

    #[test]
    fn command_and_option_keep_their_own_switching() {
        let mut r = Rig::new();
        // ⌘Tab: ⌥ going up meanwhile commits nothing.
        r.down(LEFTALT);
        r.up(LEFTALT);
        r.down(LEFTMETA);
        r.down(TAB);
        r.up(TAB);
        r.down(LEFTALT);
        assert!(!r.up(LEFTALT).contains(&Out::Action(Action::SwitchDone)));
        assert!(r.up(LEFTMETA).contains(&Out::Action(Action::SwitchDone)));
        // ⌥ down while ⌘Tab is up: Tab is still ⌘Tab's, not ⌥Tab's.
        r.down(LEFTMETA);
        r.down(TAB);
        r.up(TAB);
        r.down(LEFTALT);
        assert_eq!(r.down(TAB), [Out::Action(Action::NextApp { back: false, all: true })]);
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
        assert!(!r.k.feed(A, true, r.t, off, Focus::Olo).iter().any(|o| matches!(o, Out::Action(_))));
        r.k.feed(A, false, r.t, off, Focus::Olo);
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
        r.focus = Focus::Other;
        r.down(LEFTMETA);
        // ⌘← is Home without Ctrl; Ctrl comes back afterwards.
        assert_eq!(r.down(LEFT), [Key(RIGHTCTRL, false), Key(HOME, true)]);
        assert_eq!(r.up(LEFT), [Key(HOME, false), Key(RIGHTCTRL, true)]);
        // ⌘↓ is Ctrl+End.
        assert_eq!(r.down(DOWN), [Key(END, true)]);
        assert_eq!(r.up(DOWN), [Key(END, false)]);
        // ⌘⌫ selects to the line start and deletes.
        assert_eq!(r.down(BACKSPACE), [Key(RIGHTCTRL, false), Key(LEFTSHIFT, true), Key(HOME, true), Key(HOME, false), Key(LEFTSHIFT, false), Key(BACKSPACE, true)]);
        assert_eq!(r.up(BACKSPACE), [Key(BACKSPACE, false), Key(RIGHTCTRL, true)]);
        // Released ⌘ before the arrow: Ctrl stays up.
        r.down(RIGHT);
        assert_eq!(r.up(LEFTMETA), []);
        assert_eq!(r.up(RIGHT), [Key(END, false)]);
        assert!(r.k.down.is_empty());
        // Olo's apps get ⌘↑ and ⌘⌫ as they are (Open Parent, Move to
        // Trash; the kit makes them the line's keys in a text field).
        r.focus = Focus::Olo;
        r.down(LEFTMETA);
        assert_eq!(r.down(UP), [Key(UP, true)]);
        r.up(UP);
        assert_eq!(r.down(BACKSPACE), [Key(BACKSPACE, true)]);
    }

    #[test]
    fn other_apps_move_by_word_with_option() {
        let mut r = Rig::new();
        // Olo's apps take ⌥← as it is.
        r.down(LEFTALT);
        assert_eq!(r.down(LEFT), [Key(LEFT, true)]);
        r.up(LEFT);
        r.up(LEFTALT);
        r.focus = Focus::Other;
        assert_eq!(r.down(LEFTALT), [Key(LEFTALT, true)]);
        // Elsewhere it's Ctrl+←, without the Alt (Back, in a browser)…
        assert_eq!(r.down(LEFT), [Key(LEFTCTRL, true), Key(LEFTALT, false), Key(LEFT, true)]);
        assert_eq!(r.up(LEFT), [Key(LEFT, false), Key(LEFTCTRL, false)]);
        // …which stays up for the next word.
        assert_eq!(r.down(LEFT), [Key(LEFTCTRL, true), Key(LEFT, true)]);
        r.up(LEFT);
        // ⌥⇧→ selects a word on; ⌥⌫ deletes one.
        r.down(LEFTSHIFT);
        assert_eq!(r.down(RIGHT), [Key(LEFTCTRL, true), Key(RIGHT, true)]);
        r.up(RIGHT);
        r.up(LEFTSHIFT);
        assert_eq!(r.down(BACKSPACE), [Key(LEFTCTRL, true), Key(BACKSPACE, true)]);
        r.up(BACKSPACE);
        assert_eq!(r.up(LEFTALT), []);
        assert_eq!(r.down(C), [Key(C, true)]);
        r.up(C);
        assert!(r.k.down.is_empty());
    }

    #[test]
    fn browsers_go_back_and_change_tabs() {
        let mut r = Rig::new();
        r.focus = Focus::Browser;
        r.down(LEFTMETA);
        // ⌘[ is Alt+← (Back), with ⌘'s Ctrl let go of meanwhile.
        assert_eq!(r.down(LEFTBRACE), [Key(RIGHTCTRL, false), Key(LEFTALT, true), Key(LEFT, true)]);
        assert_eq!(r.up(LEFTBRACE), [Key(LEFT, false), Key(LEFTALT, false), Key(RIGHTCTRL, true)]);
        // ⌘⇧] is Ctrl+PgDn (the next tab), without the Shift (that moves the tab).
        r.down(LEFTSHIFT);
        assert_eq!(r.down(RIGHTBRACE), [Key(LEFTSHIFT, false), Key(PAGEDOWN, true)]);
        assert_eq!(r.up(RIGHTBRACE), [Key(PAGEDOWN, false)]);
        assert_eq!(r.up(LEFTSHIFT), []);
        // ⌘⌥← is Ctrl+PgUp.
        r.down(LEFTALT);
        assert_eq!(r.down(LEFT), [Key(LEFTALT, false), Key(PAGEUP, true)]);
        r.up(LEFT);
        r.up(LEFTALT);
        // ⌘C still copies.
        assert_eq!(r.down(C), [Key(C, true)]);
        r.up(C);
        r.up(LEFTMETA);
        assert!(r.k.down.is_empty());
        // Any other program's Back and Forward are Alt+← / Alt+→ too
        // (Slack's); ⌘← is still the line's start.
        r.focus = Focus::Other;
        r.down(LEFTMETA);
        assert_eq!(r.down(LEFTBRACE), [Key(RIGHTCTRL, false), Key(LEFTALT, true), Key(LEFT, true)]);
        r.up(LEFTBRACE);
        assert_eq!(r.down(RIGHTBRACE), [Key(RIGHTCTRL, false), Key(LEFTALT, true), Key(RIGHT, true)]);
        r.up(RIGHTBRACE);
        assert_eq!(r.down(LEFT), [Key(RIGHTCTRL, false), Key(HOME, true)]);
        r.up(LEFT);
        r.up(LEFTMETA);
        assert!(r.k.down.is_empty());
        // Olo's apps take ⌘[ as it is.
        r.focus = Focus::Olo;
        r.down(LEFTMETA);
        assert_eq!(r.down(LEFTBRACE), [Key(LEFTBRACE, true)]);
    }

    #[test]
    fn terminals_keep_ctrl() {
        let mut r = Rig::new();
        r.focus = Focus::Terminal;
        r.down(LEFTMETA);
        assert_eq!(r.down(C), [Key(LEFTSHIFT, true), Key(C, true)]);
        assert_eq!(r.up(C), [Key(C, false), Key(LEFTSHIFT, false)]);
        assert_eq!(r.down(BACKSPACE), [Key(U, true)]);
        r.up(BACKSPACE);
        r.up(LEFTMETA);
        // Physical Ctrl+C is ^C.
        assert_eq!(r.down(LEFTCTRL), [Key(LEFTCTRL, true)]);
        assert_eq!(r.down(C), [Key(C, true)]);
        r.up(C);
        r.up(LEFTCTRL);
        // Either physical Ctrl is the left one, so Ctrl+Shift+- (Ctrl+_)
        // isn't taken for ⌘-, which is the right one.
        assert_eq!(r.down(RIGHTCTRL), [Key(LEFTCTRL, true)]);
        assert_eq!(r.down(LEFTSHIFT), [Key(LEFTSHIFT, true)]);
        assert_eq!(r.down(MINUS), [Key(MINUS, true)]);
        r.up(MINUS);
        r.up(LEFTSHIFT);
        assert_eq!(r.up(RIGHTCTRL), [Key(LEFTCTRL, false)]);
        r.down(LEFTMETA);
        assert_eq!(r.down(MINUS), [Key(LEFTSHIFT, true), Key(MINUS, true)]);
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
        r.focus = Focus::Other;
        // ⌃⌥ arrows tile (as Rectangle's); ⇧⌘ arrows select text.
        r.down(LEFTCTRL);
        r.down(LEFTALT);
        assert_eq!(r.down(LEFT), [Out::Action(Action::Shortcut(Shortcut::Tile(Tile::Left)))]);
        assert_eq!(r.up(LEFT), []);
        assert_eq!(r.down(RIGHT), [Out::Action(Action::Shortcut(Shortcut::Tile(Tile::Right)))]);
        r.up(RIGHT);
        r.up(LEFTALT);
        r.up(LEFTCTRL);
        r.down(LEFTMETA);
        r.down(LEFTSHIFT);
        assert_eq!(r.down(LEFT), [Key(RIGHTCTRL, false), Key(HOME, true)]);
        r.up(LEFT);
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
        let off = Options { palette_on_tap: false, ..OPTS };
        r.t += Duration::from_millis(10);
        assert_eq!(r.k.feed(LEFTMETA, false, r.t, off, Focus::Olo), [Key(RIGHTCTRL, false)]);
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
        let off = Options { command: CommandKey::Control, caps_lock_control: false, palette_on_tap: false, hints: false, double_press: Duration::from_millis(400) };
        let t = Instant::now();
        assert_eq!(k.feed(LEFTMETA, true, t, off, Focus::Olo), [Key(LEFTMETA, true)]);
        assert_eq!(k.feed(TAB, true, t, off, Focus::Olo), [Key(TAB, true)]);
        assert_eq!(k.feed(CAPSLOCK, true, t, off, Focus::Olo), [Key(CAPSLOCK, true)]);
    }

    #[test]
    fn windows_keys() {
        use olo_protocol::Tile;
        let mut r = Rig::with(&KeyboardProfile::builtin(Platform::Windows));
        // Ctrl is Ctrl, the command key itself.
        assert_eq!(r.down(LEFTCTRL), [Key(LEFTCTRL, true)]);
        assert_eq!(r.down(C), [Key(C, true)]);
        r.up(C);
        r.up(LEFTCTRL);
        // The Windows key: Snap, and a tap alone is the palette (Start).
        assert_eq!(r.down(LEFTMETA), [Key(LEFTMETA, true)]);
        assert_eq!(r.down(LEFT), [Out::Action(Action::Shortcut(Shortcut::Tile(Tile::Left)))]);
        r.up(LEFT);
        assert_eq!(r.up(LEFTMETA), [Key(LEFTMETA, false)]);
        r.down(LEFTMETA);
        assert_eq!(r.up(LEFTMETA), [Key(LEFTMETA, false), Out::Action(Action::Palette)]);
        // Alt+Tab is Twitcher while Alt is held; Alt+Shift+Tab goes back.
        r.down(LEFTALT);
        assert_eq!(r.down(TAB), [Out::Action(Action::NextApp { back: false, all: true })]);
        r.up(TAB);
        r.down(LEFTSHIFT);
        assert_eq!(r.down(TAB), [Out::Action(Action::NextApp { back: true, all: true })]);
        r.up(TAB);
        r.up(LEFTSHIFT);
        assert_eq!(r.down(RIGHT), [Out::Action(Action::SwitchMove { dx: 1, dy: 0 })]);
        r.up(RIGHT);
        assert_eq!(r.up(LEFTALT), [Key(LEFTALT, false), Out::Action(Action::SwitchDone)]);
        // Home is Home: nothing is translated, for any program.
        r.focus = Focus::Other;
        r.down(LEFTALT);
        assert_eq!(r.down(LEFT), [Key(LEFT, true)]);
        r.up(LEFT);
        r.up(LEFTALT);
        // Ctrl+Shift+Esc: Task Manager, Activity Monitor here.
        r.down(LEFTCTRL);
        r.down(LEFTSHIFT);
        assert_eq!(r.down(ESC), [Out::Action(Action::Shortcut(Shortcut::ActivityMonitor))]);
    }

    #[test]
    fn beos_keys() {
        const F1: u32 = 59;
        let mut r = Rig::with(&KeyboardProfile::builtin(Platform::Beos));
        // The left Alt is the command key: Ctrl for apps.
        assert_eq!(r.down(LEFTALT), [Key(RIGHTCTRL, true)]);
        assert_eq!(r.down(C), [Key(C, true)]);
        r.up(C);
        // Alt+← is Ctrl+←, a word back, as it is on BeOS.
        assert_eq!(r.down(LEFT), [Key(LEFT, true)]);
        r.up(LEFT);
        // Alt+F1: workspace 1.
        assert_eq!(r.down(F1), [Out::Action(Action::Shortcut(Shortcut::Workspace(0)))]);
        r.up(F1);
        assert_eq!(r.up(LEFTALT), [Key(RIGHTCTRL, false)]);
        // The Super key is Option: Alt for apps.
        assert_eq!(r.down(LEFTMETA), [Key(LEFTALT, true)]);
        assert_eq!(r.up(LEFTMETA), [Key(LEFTALT, false), Out::Action(Action::Palette)]);
        // Ctrl+Tab is Twitcher while Ctrl is held.
        r.down(LEFTCTRL);
        assert_eq!(r.down(TAB), [Out::Action(Action::NextApp { back: false, all: true })]);
        r.up(TAB);
        assert_eq!(r.up(LEFTCTRL), [Key(LEFTCTRL, false), Out::Action(Action::SwitchDone)]);
        // Ctrl+Alt+Z zooms, Ctrl+Alt+Del is Team Monitor (Activity Monitor).
        r.t += Duration::from_secs(1);
        r.down(LEFTCTRL);
        r.down(LEFTALT);
        assert_eq!(r.down(44), [Out::Action(Action::Shortcut(Shortcut::Zoom))]);
        r.up(44);
        assert_eq!(r.down(DELETE), [Out::Action(Action::Shortcut(Shortcut::ActivityMonitor))]);
        r.up(DELETE);
        r.up(LEFTALT);
        r.up(LEFTCTRL);
        // A terminal: Alt+C is Ctrl+Shift+C (copy); Alt+⌫ a word.
        r.focus = Focus::Terminal;
        r.down(LEFTALT);
        assert_eq!(r.down(C), [Key(LEFTSHIFT, true), Key(C, true)]);
        r.up(C);
        assert_eq!(r.down(BACKSPACE), [Key(BACKSPACE, true)]);
    }

    #[test]
    fn reset_releases_everything() {
        let mut r = Rig::new();
        r.down(LEFTMETA);
        r.down(LEFTSHIFT);
        r.down(TAB);
        assert_eq!(r.k.reset(), [Key(LEFTSHIFT, false), Key(RIGHTCTRL, false), Out::Action(Action::SwitchDone)]);
        assert_eq!(r.up(TAB), [Key(TAB, false)]);
    }
}
