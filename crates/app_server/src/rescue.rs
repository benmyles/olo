//! Olo Rescue (`olo_protocol::rescue`): Ctrl+Shift+Super and Delete (or
//! Backspace, a Mac's delete), from anywhere. The chord is read here, on
//! the input thread (app_server's main thread, the compositor's priority),
//! from the keys' physical codes before anything else sees them: before
//! the keymap and its profile (so no remapping, broken or not, hides
//! it), shortcuts other programs took, keyboard hints, the overview, a
//! layer surface that takes every key, or the window with the keyboard.
//!
//! Rescue then, for as long as it lasts:
//!
//! * every window is put away: each display shows a workspace of its
//!   own past the person's (`Rescue::workspace`), and only Rescue's own
//!   layer surfaces are drawn or reached (no Deskbar, desktop, banners,
//!   or a full-screen window, `shows_layer`, `active`);
//! * app_server runs on safe settings (`safe`): each display on its own
//!   at its preferred mode and the scale Olo picks, unrotated, unmirrored,
//!   no VRR, HDR or Vivid colours; night light off, no wallpaper, the
//!   built-in display the main one, a screen too dim to see brightened;
//!   the keyboard's layout and profile kept, its other settings (repeat,
//!   xkb options) and the pointer's as they come; no screen saver;
//! * the person's own settings are what clients read and change
//!   (`told_settings`; `SetSettings` and `ConfigureOutput` change them),
//!   saved as they change, so Preferences shows and fixes what broke,
//!   and they come back, applied, as Rescue ends;
//! * olo-rescue (apps/rescue) covers the displays: a terminal of its
//!   own, the processes, Preferences, and Leave Rescue.
//!
//! Locked, the chord changes the displays (a lock screen must be seen to
//! be unlocked) and olo-rescue comes once the screen is unlocked: Rescue
//! is no way past the lock. Pressed in Rescue, it brings olo-rescue back
//! if it had gone. An app_server restarted in place stays in Rescue
//! (handover.rs), and one started with `OLO_RESCUE` set starts in it.

use std::{
    collections::HashMap,
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
    time::Instant,
};

use olo_protocol::{DisplaySettings, KeyboardSettings, NightLightMode, Settings, Shortcut, rescue::NAMESPACE};
use smithay::{desktop::LayerSurface, output::Output};
use tracing::{info, warn};

use crate::{
    desktop::display_key,
    keymap::Action,
    state::{AnvilState, Backend},
};

/// Rescue is on (for what draws and finds surfaces, which has no state).
static ACTIVE: AtomicBool = AtomicBool::new(false);
/// olo-rescue's process while it runs (0: none).
static CHILD: AtomicU32 = AtomicU32::new(0);
/// app_server was started with `OLO_RESCUE` set (main.rs).
static START: AtomicBool = AtomicBool::new(false);

/// Start in Rescue (main.rs, from `OLO_RESCUE`).
pub fn start_in_rescue() {
    START.store(true, Ordering::Relaxed);
}

pub fn starts_in_rescue() -> bool {
    START.load(Ordering::Relaxed)
}

/// A screen this dim or dimmer (0–1) is brightened in Rescue.
const TOO_DIM: f32 = 0.25;
const BRIGHTENED: f32 = 0.6;

/// Rescue is on.
pub fn active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

/// A display `w`×`h` pixels' scale in Rescue: the largest of Olo's
/// usual ones that leaves it at least 1280×800 points, the smallest
/// screen Olo's windows are made for (2× on a 2560×1600 or larger
/// panel, 1× on a 1920×1080 monitor).
pub fn safe_scale(w: i32, h: i32) -> f64 {
    let (w, h) = (w.max(h) as f64, w.min(h) as f64);
    [2.0, 1.5].into_iter().find(|s| w / s >= 1280.0 && h / s >= 800.0).unwrap_or(1.0)
}

/// Whether `layer` is drawn and takes the pointer and keyboard now: any
/// is, but in Rescue only Rescue's own.
pub fn shows_layer(layer: &LayerSurface) -> bool {
    !active() || layer.namespace() == NAMESPACE
}

/// The keys' evdev codes.
const KEY_LEFTCTRL: u32 = 29;
const KEY_RIGHTCTRL: u32 = 97;
const KEY_LEFTSHIFT: u32 = 42;
const KEY_RIGHTSHIFT: u32 = 54;
const KEY_LEFTMETA: u32 = 125;
const KEY_RIGHTMETA: u32 = 126;
const KEY_DELETE: u32 = 111;
const KEY_BACKSPACE: u32 = 14;

/// The modifiers held, physically, each of its own bit.
fn bit(key: u32) -> Option<u8> {
    Some(match key {
        KEY_LEFTCTRL => 1,
        KEY_RIGHTCTRL => 2,
        KEY_LEFTSHIFT => 4,
        KEY_RIGHTSHIFT => 8,
        KEY_LEFTMETA => 16,
        KEY_RIGHTMETA => 32,
        _ => return None,
    })
}

/// What a key is to the chord.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fed {
    /// Nothing: on to the rest.
    Pass,
    /// The chord's Delete going up: nobody sees it.
    Swallow,
    /// The chord: Rescue (and nobody sees the Delete).
    Rescue,
}

/// The chord, followed from the keys' physical codes.
#[derive(Debug, Default)]
pub struct Chord {
    held: u8,
    /// The chord's Delete (or Backspace), still down.
    swallowing: Option<u32>,
}

impl Chord {
    pub fn feed(&mut self, key: u32, pressed: bool) -> Fed {
        if let Some(b) = bit(key) {
            if pressed {
                self.held |= b;
            } else {
                self.held &= !b;
            }
            return Fed::Pass;
        }
        if !pressed && self.swallowing == Some(key) {
            self.swallowing = None;
            return Fed::Swallow;
        }
        let all = self.held & 3 != 0 && self.held & 12 != 0 && self.held & 48 != 0;
        if pressed && all && matches!(key, KEY_DELETE | KEY_BACKSPACE) {
            self.swallowing = Some(key);
            return Fed::Rescue;
        }
        Fed::Pass
    }

    /// Every key up (the session given back after another took the
    /// seat: what was held then isn't now).
    pub fn reset(&mut self) {
        *self = Chord::default();
    }
}

/// What a keymap action becomes in Rescue: none of what would leave its
/// workspace (another workspace, the overview, Show Desktop, hiding) or
/// open what it doesn't show (the palette, keyboard hints, the emoji
/// picker, Find, a screenshot's selection); switching apps among
/// Rescue's own.
pub fn in_rescue(action: Action) -> Option<Action> {
    use Shortcut as S;
    Some(match action {
        Action::NextApp { back, .. } => Action::NextApp { back, all: false },
        Action::NextWindow { .. } | Action::SwitchDone | Action::SwitchMove { .. } | Action::SwitchPick(_) | Action::SwitchCancel => action,
        Action::Palette | Action::Hints | Action::HintsAhead => return None,
        Action::Shortcut(s) => match s {
            S::Overview
            | S::Palette
            | S::EmojiPicker
            | S::Find
            | S::NextWorkspace
            | S::PreviousWorkspace
            | S::Workspace(_)
            | S::WindowToNextWorkspace
            | S::WindowToPreviousWorkspace
            | S::WindowToWorkspace(_)
            | S::ShowDesktop
            | S::Hide
            | S::HideApp
            | S::RestoreOrHide
            | S::SmartTile
            | S::ScreenshotRegion
            | S::ScreenshotWindow
            | S::ScreenshotApp => return None,
            S::SwitchApps => Action::Shortcut(S::SwitchAppsHere),
            _ => action,
        },
    })
}

/// The settings Rescue runs on, from the person's `real` ones: what can
/// leave a screen dark, garbled or unreadable, or the keyboard or
/// pointer unusable, as Olo comes; the rest (the look, the keyboard's
/// layout and profile, so it types and its shortcuts are where the hands
/// know them) kept. `main`: the display Rescue shows on; `workspace`:
/// Rescue's own (one past the person's).
pub fn safe(real: &Settings, main: Option<String>, scales: &[(String, f64)], workspace: u32) -> Settings {
    let d = Settings::default();
    let mut s = real.clone();
    // Each display on its own, at its preferred mode (udev and
    // `enter_rescue`) and the scale Olo picks, unrotated, no VRR, HDR or
    // Vivid; brightness is kept as it is (`set_settings`).
    s.display = DisplaySettings { scale: d.display.scale, main, outputs: Default::default() };
    for (key, scale) in scales {
        s.display.outputs.entry(key.clone()).or_default().scale = Some(*scale);
    }
    s.appearance.night_light.mode = NightLightMode::Off;
    s.wallpaper = None;
    let k = &real.keyboard;
    let mut terminal_apps = k.terminal_apps.clone();
    // Its terminal's ⌃C is the shell's, ⌘C copies.
    terminal_apps.push(olo_protocol::rescue::APP_ID.into());
    s.keyboard = KeyboardSettings {
        layout: k.layout.clone(),
        variant: k.variant.clone(),
        profile: k.profile.clone(),
        profiles: k.profiles.clone(),
        swap_alt_super: k.swap_alt_super.clone(),
        terminal_apps,
        browser_apps: k.browser_apps.clone(),
        ..d.keyboard
    };
    s.touchpad = d.touchpad;
    s.mouse_speed = d.mouse_speed;
    s.mice = d.mice;
    s.focus_follows_mouse = d.focus_follows_mouse;
    s.tiling.default_mode = d.tiling.default_mode;
    s.tiling.modes.retain(|m| m.workspace != workspace);
    s.workspace_count = workspace + 1;
    s.screen_saver.enabled = false;
    s
}

/// Rescue, while it's on.
#[derive(Debug)]
pub struct Rescue {
    /// The person's own settings: what clients read and change meanwhile
    /// (saved as they change), and what comes back.
    pub real: Settings,
    /// Rescue's workspace, one past the person's.
    pub workspace: u32,
    /// The workspace each display showed, by its key.
    pub(crate) before: HashMap<String, u32>,
    pub since: Instant,
}

impl<B: Backend + 'static> AnvilState<B> {
    /// The chord: Rescue, or (in it) olo-rescue back if it had gone.
    pub fn rescue_chord(&mut self) {
        info!("the Rescue keys");
        if self.rescue.is_some() {
            if !self.lock.locked() {
                self.start_rescue_app();
            }
            return;
        }
        self.enter_rescue();
    }

    pub fn enter_rescue(&mut self) {
        self.enter_rescue_from(None);
    }

    /// Into Rescue, with the workspace each display showed before it
    /// (`before`: as the app_server before this one had it), or as they
    /// are now.
    pub fn enter_rescue_from(&mut self, before: Option<HashMap<String, u32>>) {
        if self.rescue.is_some() {
            return;
        }
        let t = Instant::now();
        if self.overview.is_some() {
            self.close_overview(None);
        }
        self.hints.close();
        let real = self.settings.clone();
        let workspace = real.workspace_count.max(1);
        let outputs: Vec<Output> = self.space.outputs().cloned().collect();
        let before = before.unwrap_or_else(|| outputs.iter().map(|o| (display_key(o), self.showing(o))).collect());
        // On the built-in display, if there's one.
        let main = outputs.iter().find(|o| crate::power::is_internal(&o.name())).or(outputs.first()).map(display_key);
        // Its preferred mode first, and the scale for it.
        for o in &outputs {
            self.set_rescue_mode(o, None);
        }
        let scales: Vec<(String, f64)> = outputs.iter().filter_map(|o| Some((display_key(o), o.current_mode().map(|m| safe_scale(m.size.w, m.size.h))?))).collect();
        self.apply_settings(safe(&real, main, &scales, workspace));
        for o in &outputs {
            // Every window put away: a workspace of nothing.
            self.switch_workspace_on(o, workspace);
            self.set_safe_color(o);
        }
        ACTIVE.store(true, Ordering::Relaxed);
        self.rescue = Some(Rescue { real, workspace, before, since: Instant::now() });
        self.brighten_for_rescue();
        self.tell_workspaces();
        self.power_changed();
        self.ipc.broadcast(olo_protocol::Event::OutputsChanged { outputs: self.output_infos() });
        self.backend_data.wake(None);
        if !self.lock.locked() {
            self.start_rescue_app();
        }
        info!(took_ms = t.elapsed().as_secs_f64() * 1e3, displays = outputs.len(), "Olo Rescue");
    }

    pub fn leave_rescue(&mut self) {
        let Some(r) = self.rescue.take() else { return };
        ACTIVE.store(false, Ordering::Relaxed);
        stop_rescue_app();
        let outputs: Vec<Output> = self.space.outputs().cloned().collect();
        let last = r.real.workspace_count.max(1) - 1;
        for o in &outputs {
            let back = r.before.get(&display_key(o)).copied().unwrap_or(0).min(last);
            // What was opened in Rescue goes along, onto the workspace the
            // display showed.
            let here: Vec<_> = self.space.elements().filter(|w| self.window_output(w).as_ref() == Some(o) && !w.meta().pip.get()).cloned().collect();
            for w in &here {
                self.move_to_workspace(w, back);
            }
            self.switch_workspace_on(o, back);
        }
        self.apply_settings(r.real);
        for o in &outputs {
            let saved = self.settings.display.config(&display_key(o), &o.name()).and_then(|c| c.mode);
            self.set_rescue_mode(o, saved);
            if let Err(e) = self.apply_color(o) {
                warn!(output = o.name(), "colours: {e}");
            }
            if self.settings.display.config(&display_key(o), &o.name()).and_then(|c| c.vrr) == Some(true)
                && let Err(e) = self.backend_data.set_vrr(o, true)
            {
                warn!(output = o.name(), "VRR: {e}");
            }
        }
        let pointer = self.pointer.current_location();
        crate::shell::fixup_positions(&mut self.space, &self.settings.display, pointer);
        self.rehome_windows();
        // (With the brightness Rescue left.)
        crate::desktop::save_settings(&self.settings);
        self.tell_workspaces();
        self.power_changed();
        self.ipc.broadcast(olo_protocol::Event::SettingsChanged { settings: self.told_settings() });
        self.ipc.broadcast(olo_protocol::Event::OutputsChanged { outputs: self.output_infos() });
        self.backend_data.wake(None);
        info!(secs = r.since.elapsed().as_secs(), "left Olo Rescue");
    }

    /// `o` at `mode` (None: its preferred one), if it isn't already.
    fn set_rescue_mode(&mut self, o: &Output, mode: Option<olo_protocol::Mode>) {
        let want = match mode {
            Some(m) => smithay::output::Mode { size: (m.width, m.height).into(), refresh: m.refresh },
            None => match o.preferred_mode() {
                Some(m) => m,
                None => return,
            },
        };
        if o.current_mode() == Some(want) || !o.modes().contains(&want) {
            return;
        }
        match self.backend_data.set_mode(o, want) {
            Ok(()) => self.backend_data.reset_buffers(o),
            Err(e) => warn!(output = o.name(), "its mode: {e}"),
        }
    }

    /// `o` in SDR, Accurate, without VRR (the safe settings have no
    /// colours of their own: `apply_color` reads them so).
    fn set_safe_color(&mut self, o: &Output) {
        if let Err(e) = self.apply_color(o) {
            warn!(output = o.name(), "colours: {e}");
        }
        if self.backend_data.vrr(o) == Some(true)
            && let Err(e) = self.backend_data.set_vrr(o, false)
        {
            warn!(output = o.name(), "VRR: {e}");
        }
    }

    /// A screen too dim to see, brightened (kept so after Rescue too).
    fn brighten_for_rescue(&mut self) {
        if self.power.brightness.is_some_and(|b| b <= TOO_DIM) {
            self.set_brightness(BRIGHTENED);
        }
        let names: Vec<String> = self.space.outputs().map(|o| o.name()).filter(|n| !crate::power::is_internal(n)).collect();
        for n in names {
            if self.power.display_brightness(&n).is_some_and(|b| b <= TOO_DIM)
                && let Err(e) = self.set_output_brightness(&n, BRIGHTENED)
            {
                warn!(output = n, "brightness: {e}");
            }
        }
    }

    /// Start olo-rescue, unless it's running. It asks rtkit for Rescue's
    /// priority itself as it starts (`RESCUE_NICE`), before its threads.
    pub fn start_rescue_app(&mut self) {
        if CHILD.load(Ordering::Relaxed) != 0 {
            return;
        }
        let program = olo_protocol::rescue::PROGRAM;
        let Some(path) = olo_xdg::find_program(program) else {
            warn!(program, "not installed: Rescue has its safe settings, and nothing to show");
            return;
        };
        let mut cmd = std::process::Command::new(path);
        if let Some(socket) = &self.socket_name {
            cmd.env("WAYLAND_DISPLAY", socket);
        }
        match cmd.spawn() {
            Ok(mut child) => {
                let pid = child.id();
                CHILD.store(pid, Ordering::Relaxed);
                info!(pid, "olo-rescue started");
                let reaped = std::thread::Builder::new().name("olo-rescue".into()).spawn(move || {
                    let status = child.wait();
                    let _ = CHILD.compare_exchange(pid, 0, Ordering::Relaxed, Ordering::Relaxed);
                    info!(pid, ?status, "olo-rescue ended");
                });
                if let Err(e) = reaped {
                    warn!("olo-rescue's watcher: {e}");
                }
            }
            Err(e) => warn!(program, %e, "could not start"),
        }
    }

    /// The settings clients are told and read: the person's own, in
    /// Rescue too, with the look as it is now (what's worked out of it).
    pub fn told_settings(&self) -> Settings {
        match &self.rescue {
            None => self.settings.clone(),
            Some(r) => {
                let mut s = r.real.clone();
                s.appearance.effective_theme = self.settings.appearance.effective_theme;
                s.appearance.night_light_active = self.settings.appearance.night_light_active;
                s
            }
        }
    }

    /// The person's settings saved (in Rescue, their own, not Rescue's)
    /// and told.
    pub fn settings_changed(&mut self) {
        crate::desktop::save_settings(self.rescue.as_ref().map_or(&self.settings, |r| &r.real));
        self.ipc.broadcast(olo_protocol::Event::SettingsChanged { settings: self.told_settings() });
    }
}

/// olo-rescue asked to go (Rescue over).
fn stop_rescue_app() {
    let pid = CHILD.load(Ordering::Relaxed);
    if pid != 0 {
        // SAFETY: a signal to our own child (reaped by its watcher, so
        // the pid is still its until then).
        unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_chord_from_any_side_and_its_delete_unseen() {
        let mut c = Chord::default();
        // Delete alone, or with only some of them: nothing.
        assert_eq!(c.feed(KEY_DELETE, true), Fed::Pass);
        assert_eq!(c.feed(KEY_DELETE, false), Fed::Pass);
        c.feed(KEY_LEFTCTRL, true);
        c.feed(KEY_RIGHTSHIFT, true);
        assert_eq!(c.feed(KEY_DELETE, true), Fed::Pass);
        assert_eq!(c.feed(KEY_DELETE, false), Fed::Pass);
        // With Super too (either side): Rescue, and its Delete's going
        // up seen by nobody.
        c.feed(KEY_RIGHTMETA, true);
        assert_eq!(c.feed(KEY_DELETE, true), Fed::Rescue);
        assert_eq!(c.feed(KEY_DELETE, false), Fed::Swallow);
        // A Mac's delete (Backspace) does too.
        assert_eq!(c.feed(KEY_BACKSPACE, true), Fed::Rescue);
        assert_eq!(c.feed(KEY_BACKSPACE, false), Fed::Swallow);
        // Let go of Shift: not any more.
        c.feed(KEY_RIGHTSHIFT, false);
        assert_eq!(c.feed(KEY_DELETE, true), Fed::Pass);
    }

    #[test]
    fn safe_settings_keep_the_keyboard_layout_and_drop_what_breaks_a_screen() {
        let mut real = Settings::default();
        real.keyboard.layout = "de".into();
        real.keyboard.options = "caps:none".into();
        real.keyboard.repeat_delay_ms = 1;
        real.appearance.night_light.mode = NightLightMode::On;
        real.wallpaper = Some("/nowhere.png".into());
        real.workspace_count = 4;
        let c = real.display.config_mut("Some Monitor 1", "DP-1");
        c.scale = Some(4.0);
        c.rotation = Some(90);
        c.hdr = Some(true);
        c.mirror = Some("eDP-1".into());
        let s = safe(&real, Some("eDP-1".into()), &[("eDP-1".into(), 2.0)], 4);
        assert_eq!(s.keyboard.layout, "de");
        assert_eq!(s.keyboard.options, KeyboardSettings::default().options);
        assert_eq!(s.keyboard.repeat_delay_ms, KeyboardSettings::default().repeat_delay_ms);
        assert!(s.keyboard.terminal_apps.iter().any(|a| a == olo_protocol::rescue::APP_ID));
        assert_eq!(s.appearance.night_light.mode, NightLightMode::Off);
        assert_eq!(s.wallpaper, None);
        assert_eq!(s.display.outputs.len(), 1);
        assert_eq!(s.display.scale_for("Some Monitor 1", "DP-1"), Settings::default().display.scale);
        assert_eq!(s.display.rotation_for("Some Monitor 1", "DP-1"), 0);
        assert_eq!(s.display.config("Some Monitor 1", "DP-1").and_then(|c| c.hdr), None);
        assert_eq!(s.display.main.as_deref(), Some("eDP-1"));
        assert_eq!(s.workspace_count, 5);
        assert!(!s.screen_saver.enabled);
        // What has nothing to do with seeing and typing is the person's.
        assert_eq!(s.appearance.theme, real.appearance.theme);
        assert_eq!(s.security, real.security);
    }

    /// What every key and every frame pay for Rescue: the chord followed
    /// (each key), and whether it's on (each layer surface, each frame
    /// and pointer move). `cargo test --release -p app_server --lib
    /// rescue_cost -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn rescue_cost() {
        let keys = [KEY_LEFTCTRL, 30, 31, KEY_LEFTSHIFT, 32, KEY_DELETE, 14, 57];
        let mut c = Chord::default();
        let n = 10_000_000u32;
        let t = std::time::Instant::now();
        let mut fired = 0u32;
        for i in 0..n {
            let k = keys[i as usize % keys.len()];
            fired += (std::hint::black_box(&mut c).feed(std::hint::black_box(k), i % 2 == 0) == Fed::Rescue) as u32;
        }
        eprintln!("a key through the chord: {:.2} ns ({fired})", t.elapsed().as_nanos() as f64 / n as f64);
        let t = std::time::Instant::now();
        let mut on = 0u32;
        for _ in 0..n {
            on += std::hint::black_box(active()) as u32;
        }
        eprintln!("whether Rescue is on: {:.2} ns ({on})", t.elapsed().as_nanos() as f64 / n as f64);
    }

    #[test]
    fn each_display_at_a_scale_that_fits_rescue() {
        assert_eq!(safe_scale(3840, 2400), 2.0);
        assert_eq!(safe_scale(2560, 1600), 2.0);
        assert_eq!(safe_scale(5120, 2880), 2.0);
        assert_eq!(safe_scale(2560, 1440), 1.5);
        assert_eq!(safe_scale(1920, 1080), 1.0);
        // Turned on its side too.
        assert_eq!(safe_scale(1600, 2560), 2.0);
    }

    #[test]
    fn rescue_keeps_to_its_workspace() {
        assert_eq!(in_rescue(Action::Shortcut(Shortcut::NextWorkspace)), None);
        assert_eq!(in_rescue(Action::Shortcut(Shortcut::Overview)), None);
        assert_eq!(in_rescue(Action::Palette), None);
        assert_eq!(in_rescue(Action::NextApp { back: false, all: true }), Some(Action::NextApp { back: false, all: false }));
        assert_eq!(in_rescue(Action::Shortcut(Shortcut::CloseWindow)), Some(Action::Shortcut(Shortcut::CloseWindow)));
    }
}
