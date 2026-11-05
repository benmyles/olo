use std::{convert::TryInto, process::Command, sync::atomic::Ordering};

use crate::{AnvilState, focus::PointerFocusTarget, shell::FullscreenSurface};

#[cfg(feature = "udev")]
use crate::udev::UdevData;
#[cfg(feature = "udev")]
use smithay::{backend::renderer::DebugFlags, input::tablet, reexports::wayland_server::Resource};

use smithay::{
    backend::input::{
        self, Axis, AxisSource, Device, DeviceCapability, Event, GestureBeginEvent, GestureEndEvent, GesturePinchUpdateEvent as _, GestureSwipeUpdateEvent as _, InputBackend, InputEvent, InputTime,
        KeyState, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent, TouchEvent,
    },
    desktop::{WindowSurfaceType, layer_map_for_output, utils::under_from_surface_tree},
    input::{
        keyboard::{FilterResult, Keysym, ModifiersState, keysyms as xkb},
        pointer::{AxisFrame, ButtonEvent, MotionEvent},
        pointer::{
            GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent, GesturePinchEndEvent, GesturePinchUpdateEvent, GestureSwipeBeginEvent, GestureSwipeEndEvent, GestureSwipeUpdateEvent,
        },
        tablet::{TabletDescriptor, TabletSeatTrait},
        touch::{DownEvent, UpEvent},
    },
    output::Scale,
    reexports::{wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1, wayland_server::protocol::wl_pointer},
    utils::{Logical, Point, SERIAL_COUNTER as SCOUNTER, Serial, Transform},
    wayland::{
        input_method::InputMethodSeat,
        keyboard_shortcuts_inhibit::KeyboardShortcutsInhibitorSeat,
        shell::wlr_layer::{KeyboardInteractivity, Layer as WlrLayer},
        virtual_keyboard::{VirtualKeyboardBackend, VirtualKeyboardHandler, VirtualKeyboardSpecialEvent},
    },
};

use smithay::backend::input::AbsolutePositionEvent;

/// The left button (linux/input-event-codes.h), a finger on the touch
/// screen being the pointer presses.
const BTN_LEFT: u32 = 0x110;

#[cfg(feature = "winit")]
use smithay::output::Output;
use tracing::{debug, error, info, warn};

use crate::state::Backend;
#[cfg(feature = "udev")]
use smithay::{
    backend::{
        input::{PointerMotionEvent, ProximityState, TabletToolButtonEvent, TabletToolEvent, TabletToolProximityEvent, TabletToolTipEvent, TabletToolTipState},
        session::Session,
    },
    input::pointer::RelativeMotionEvent,
    reexports::wayland_server::DisplayHandle,
    wayland::{
        pointer_constraints::{PointerConstraint, with_pointer_constraint},
        seat::WaylandFocus,
    },
};

impl<BackendData: Backend> AnvilState<BackendData> {
    // Allow in this method because of existing usage
    #[allow(clippy::uninlined_format_args)]
    fn process_common_key_action(&mut self, action: KeyAction) {
        match action {
            KeyAction::None => (),

            KeyAction::Quit => {
                info!("Quitting.");
                self.running.store(false, Ordering::SeqCst);
            }

            KeyAction::Run(cmd) => {
                info!(cmd, "Starting program");

                if let Err(e) = Command::new(&cmd).envs(self.socket_name.clone().map(|v| ("WAYLAND_DISPLAY", v))).spawn() {
                    error!(cmd, err = %e, "Failed to start program");
                }
            }

            KeyAction::ToggleDecorations => {
                for element in self.space.elements() {
                    #[allow(irrefutable_let_patterns)]
                    if let Some(toplevel) = element.0.toplevel() {
                        let mode_changed = toplevel.with_pending_state(|state| {
                            if let Some(current_mode) = state.decoration_mode {
                                let new_mode = if current_mode == zxdg_toplevel_decoration_v1::Mode::ClientSide {
                                    zxdg_toplevel_decoration_v1::Mode::ServerSide
                                } else {
                                    zxdg_toplevel_decoration_v1::Mode::ClientSide
                                };
                                state.decoration_mode = Some(new_mode);
                                true
                            } else {
                                false
                            }
                        });

                        if mode_changed && toplevel.is_initial_configure_sent() {
                            toplevel.send_pending_configure();
                        }
                    }
                }
            }

            _ => unreachable!("Common key action handler encountered backend specific action {:?}", action),
        }
    }

    fn keyboard_key_to_action<B: InputBackend>(&mut self, evt: B::KeyboardKeyEvent) -> KeyAction {
        let key = evt.key_code().raw().saturating_sub(8);
        // A security key's typing goes to nobody, but three touches in a
        // row (security_key.rs).
        if !self.security_key_ids.is_empty() && evt.device().usb_id().is_some_and(|id| self.security_key_ids.contains(&id)) {
            let pressed = evt.state() == KeyState::Pressed;
            let typing = self.settings.security.keys.three_touches_type;
            if let Some(keys) = self.security_key_taps.feed(key, pressed, std::time::Instant::now(), typing) {
                let time = Event::time(&evt);
                for (k, p) in keys {
                    let _ = self.physical_key(k, p, time);
                }
            }
            return KeyAction::None;
        }
        // A Magic Keyboard: its top row (F-keys first, if chosen for it),
        // its Do Not Disturb key, its Globe key alone (apple_keys.rs).
        let key = if evt.device().usb_id().is_some_and(crate::apple_keys::is_magic_keyboard) {
            let pressed = evt.state() == KeyState::Pressed;
            if self.globe.feed(key, pressed, std::time::Instant::now()) && self.settings.keyboard.globe_emoji {
                self.spawn_program("olo-emoji-picker", &["--toggle"]);
            }
            if key == crate::apple_keys::KEY_FN {
                return KeyAction::None;
            }
            let fkeys = &self.settings.keyboard.fkeys_first;
            let key = if !fkeys.is_empty() && fkeys.contains(&evt.device().name()) { crate::apple_keys::fkeys_first(key) } else { key };
            if key == crate::apple_keys::KEY_SLEEP {
                if pressed {
                    self.toggle_do_not_disturb();
                }
                return KeyAction::None;
            }
            key
        } else {
            key
        };
        // A keyboard whose Alt and Super change places (its own setting:
        // a PC keyboard's, not an Apple one's on the same computer).
        let swap = &self.settings.keyboard.swap_alt_super;
        let key = if !swap.is_empty() && matches!(key, 56 | 100 | 125 | 126) && swap.contains(&evt.device().name()) { crate::keymap::swap_alt_super(key) } else { key };
        let pressed = evt.state() == KeyState::Pressed;
        self.physical_key(key, pressed, Event::time(&evt))
    }

    /// A ring where the pointer is, when clicks are shown (`showing`).
    fn show_click(&mut self) {
        self.zoom_click();
        if !crate::showing::clicks() {
            return;
        }
        let at = self.pointer.current_location();
        if let Some(o) = self.space.output_under(at).next()
            && let Some(g) = self.space.output_geometry(o)
        {
            crate::showing::click(o, at - g.loc.to_f64());
        }
        self.backend_data.wake(None);
    }

    /// A press, for a recording's zoom that follows clicks (`zoom`): in
    /// around it, and the displays woken once the hold is up (a timer at
    /// a time, put off while clicks and keys hold it).
    fn zoom_click(&mut self) {
        let at = self.pointer.current_location();
        let Some((o, g)) = self.space.output_under(at).next().and_then(|o| self.space.output_geometry(o).map(|g| (o.clone(), g))) else { return };
        if !crate::zoom::click(&o.name(), at - g.loc.to_f64(), g.size.to_f64()) {
            return;
        }
        self.backend_data.wake(None);
        if !self.zoom_timer {
            self.zoom_timer = true;
            use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
            let _ = self.handle.insert_source(Timer::from_duration(crate::zoom::HOLD), |_, _, state| match crate::zoom::hold_left() {
                Some(left) => TimeoutAction::ToDuration(left + std::time::Duration::from_millis(10)),
                None => {
                    state.zoom_timer = false;
                    state.backend_data.wake(None);
                    TimeoutAction::Drop
                }
            });
        }
    }

    /// Physical `key` pressed while keys are shown (`showing`): its chord
    /// put up as the keyboard profile writes it, if it's one (⌃, ⌥ or ⌘
    /// held) or a key shown alone; typing never.
    fn show_chord(&mut self, key: u32) {
        let chord = self.keymap.chord_now(key);
        let modified = chord.ctrl || chord.alt || chord.cmd;
        let modifier = matches!(key, 29 | 97 | 42 | 54 | 56 | 100 | 125 | 126 | 58);
        if modifier || !olo_protocol::Chord::known_key(key) || !(modified || crate::showing::shown_alone(key)) {
            return;
        }
        let label = chord.label(self.settings.keyboard.profile.platform);
        let at = self.pointer.current_location();
        if let Some(o) = self.space.output_under(at).next().or_else(|| self.space.outputs().next()).cloned() {
            crate::showing::chord(&o, label);
        }
        self.backend_data.wake(None);
    }

    /// A key from a device (or a test): map it (⌘, Caps Lock, …), act on
    /// app_server's own chords and send the rest to the focused client.
    fn physical_key(&mut self, key: u32, pressed: bool, time: InputTime) -> KeyAction {
        if pressed {
            crate::contacts::note_key();
            if crate::showing::keys() {
                self.show_chord(key);
            }
            crate::zoom::key();
        }
        // Escape while something is dragged: the drag ends, nothing
        // dropped (and the Escape reaches nobody).
        const KEY_ESC: u32 = 1;
        if key == KEY_ESC {
            if pressed && self.escape_drag() {
                self.escaped = true;
                return KeyAction::None;
            }
            if !pressed && std::mem::take(&mut self.escaped) {
                return KeyAction::None;
            }
        }
        // Olo Rescue's keys, before anything else sees them (rescue.rs).
        match self.rescue_chord.feed(key, pressed) {
            crate::rescue::Fed::Pass => {}
            crate::rescue::Fed::Swallow => return KeyAction::None,
            crate::rescue::Fed::Rescue => {
                // (Super let go now is no tap: no palette.)
                self.keymap.command_used();
                self.rescue_chord();
                return KeyAction::None;
            }
        }
        if self.media_key(key, pressed) {
            return KeyAction::None;
        }
        if self.rescue.is_none() && (self.hints_key(key, pressed) || self.overview_key(key, pressed)) {
            return KeyAction::None;
        }
        let opts = crate::keymap::Options::from(&self.settings.keyboard);
        let focus = self.focused_window().map_or(crate::keymap::Focus::Olo, |w| {
            use crate::keymap::Focus;
            let (id, keyboard) = (w.app_id(), &self.settings.keyboard);
            if keyboard.terminal_apps.contains(&id) {
                Focus::Terminal
            } else if keyboard.browser_apps.contains(&id) {
                Focus::Browser
            } else if id.starts_with("olo.") {
                Focus::Olo
            } else {
                Focus::Other
            }
        });
        let outs = self.keymap.feed(key, pressed, std::time::Instant::now(), opts, focus);
        // ⌘ on a tab's cut title: its tip at once (`tabtip`).
        if pressed && self.keymap.command_held() {
            self.tab_tip_now();
        }
        let mut action = KeyAction::None;
        for out in outs {
            match out {
                crate::keymap::Out::Key(k, p) => {
                    let state = if p { KeyState::Pressed } else { KeyState::Released };
                    match self.client_key((k + 8).into(), state, time) {
                        KeyAction::None => {}
                        a => action = a,
                    }
                }
                // Switching apps, Find and the like stay off while locked.
                crate::keymap::Out::Action(a) if !self.lock.locked() => self.keymap_action(a),
                crate::keymap::Out::Action(_) => {}
                crate::keymap::Out::Recorded(chord) => {
                    let keys = chord.map(|c| c.map(|c| c.to_string()).unwrap_or_default());
                    self.ipc.broadcast(olo_protocol::Event::ShortcutRecorded { keys });
                }
                // (Nor in Rescue: what took them is put away.)
                crate::keymap::Out::Grabbed(chord, pressed) if !self.lock.locked() && self.rescue.is_none() => self.key_grabbed(chord, pressed),
                crate::keymap::Out::Grabbed(..) => {}
            }
        }
        action
    }

    /// Escape, with something dragged: a drag-and-drop ends with nothing
    /// dropped (its source told it was cancelled, the window it was over
    /// that it left), a window being moved goes back where it was. False
    /// when nothing is.
    fn escape_drag(&mut self) -> bool {
        let pointer = self.pointer.clone();
        let moving = pointer.with_grab(|_, g| g.downcast_ref::<crate::shell::PointerMoveSurfaceGrab<BackendData>>().is_some_and(|m| m.started)).unwrap_or(false);
        if !(moving || self.dragging) {
            return false;
        }
        info!(moving, "Escape: the drag ends");
        self.move_cancelled = moving;
        pointer.unset_grab(self, SCOUNTER.next_serial(), InputTime::now());
        true
    }

    fn keymap_action(&mut self, action: crate::keymap::Action) {
        use crate::keymap::Action as A;
        let action = match (self.rescue.is_some(), crate::rescue::in_rescue(action)) {
            (false, _) => action,
            (true, Some(a)) => a,
            (true, None) => {
                debug!(?action, "not in Olo Rescue");
                return;
            }
        };
        debug!(?action, "keymap action");
        match action {
            A::NextApp { back, all } => self.twitch_step(true, all, back),
            A::NextWindow { back } => self.twitch_step(false, true, back),
            A::SwitchDone => self.twitch_done(),
            A::SwitchMove { dx, dy } => self.twitch_move(dx, dy),
            A::SwitchPick(c) => self.twitch_pick(c),
            A::SwitchCancel => self.twitch_cancel(),
            A::Palette => self.shortcut(olo_protocol::Shortcut::Palette),
            A::Shortcut(s) => self.shortcut(s),
            A::Hints => self.toggle_hints(),
            A::HintsAhead => self.hints_ahead(),
        }
    }

    /// One of the shortcuts in the settings.
    fn shortcut(&mut self, shortcut: olo_protocol::Shortcut) {
        self.run_shortcut(shortcut, None);
    }

    /// A shortcut, done to `target` (or the focused window).
    pub fn run_shortcut(&mut self, shortcut: olo_protocol::Shortcut, target: Option<crate::shell::WindowElement>) {
        use olo_protocol::Shortcut as S;
        let focused = target.or_else(|| self.focused_window());
        if let Some(w) = &focused {
            w.meta().moved_on_purpose.set(true);
        }
        let workspaces = self.settings.workspace_count.max(1);
        // Arranging leaves Focus mode (which zooms every window) first.
        if matches!(
            shortcut,
            S::Tile(_)
                | S::ZoomVertical
                | S::ZoomHorizontal
                | S::Restore
                | S::RestoreOrHide
                | S::Center
                | S::TileAll
                | S::SmartTile
                | S::ZoomAll
                | S::CascadeAll
                | S::FanOut
                | S::CascadeOthers
                | S::FanOutOthers
        ) {
            self.arranging(focused.as_ref().and_then(|w| self.window_output(w)));
        }
        match shortcut {
            // Resident: a second start hands --toggle to the running one.
            S::Palette => self.spawn_program("olo-palette", &["--toggle"]),
            S::EmojiPicker => self.spawn_program("olo-emoji-picker", &["--toggle"]),
            S::Find => {
                // Bring back an open Find window rather than stacking more.
                let open = self.all_windows().into_iter().find(|w| w.app_id() == "olo.tracker" && w.title() == "Find");
                match open {
                    Some(w) => self.restore(&w),
                    None => self.spawn_program("olo-tracker", &["--find"]),
                }
            }
            S::Lock => self.lock_screen(),
            S::Overview => self.toggle_overview(),
            // From the palette or an agent, with no keys to hold: a quick
            // press and release, to the app (or window) before.
            S::SwitchApps | S::SwitchAppsHere | S::SwitchWindows => {
                self.twitch_step(shortcut != S::SwitchWindows, shortcut != S::SwitchAppsHere, false);
                self.twitch_done();
            }
            S::ShortcutHelp => self.spawn_program("olo-palette", &["--shortcuts"]),
            S::ActivityMonitor => {
                let open = self.all_windows().into_iter().find(|w| w.app_id() == "olo.activity");
                match open {
                    Some(w) => self.restore(&w),
                    None => self.spawn_program("olo-activity", &[]),
                }
            }
            S::HideApp => {
                // Its windows on this display: those on another are
                // theirs to hide.
                let Some(focused) = focused else { return };
                let (app, here) = (focused.app_id(), self.window_output(&focused));
                let windows: Vec<_> = self.all_windows().into_iter().filter(|w| w.app_id() == app && self.shows(w) && self.window_output(w) == here).collect();
                for w in &windows {
                    self.minimize_window(w);
                }
            }
            S::SendToBack => {
                if let Some(w) = focused {
                    self.send_to_back(&w);
                }
            }
            S::CloseWindow => {
                if let Some(w) = focused {
                    self.close_window(&w);
                }
            }
            S::Caffeinate => {
                let on = !self.power.caffeinated();
                self.set_caffeinated(on, None, false);
                let detail = if on { "Until you turn it off" } else { "Sleeps when idle" };
                self.show_indicator(crate::osd::Indicator::Caffeinated { on, detail: detail.into() });
            }
            S::ScreenshotScreen => self.spawn_program("olo-screenshot", &["--silent"]),
            // The target by id: from the palette, the palette is active.
            S::ScreenshotWindow => match &focused {
                Some(w) => self.spawn_program("olo-screenshot", &["--silent", "--window", &w.meta().id.to_string()]),
                None => self.spawn_program("olo-screenshot", &["--silent", "--window"]),
            },
            S::ScreenshotRegion => self.spawn_program("olo-screenshot", &["--region"]),
            // Started, or the one going finished (olo-screenshot knows).
            S::ScreenRecording => self.spawn_program("olo-screenshot", &["--toggle-record"]),
            S::ScreenshotApp => {
                let open = self.all_windows().into_iter().find(|w| w.app_id() == "olo.screenshot");
                match open {
                    Some(w) => self.restore(&w),
                    None => self.spawn_program("olo-screenshot", &[]),
                }
            }
            S::TileAll => {
                if !self.auto_balance() {
                    self.tile_all();
                }
            }
            S::AutoTile | S::FocusMode => {
                use olo_protocol::WindowingMode as M;
                let mode = if shortcut == S::AutoTile { M::AutoTiling } else { M::Focus };
                let Some(o) = self.active_output() else { return };
                let to = if self.mode_now(&o) == mode { M::Standard } else { mode };
                let now = self.set_windowing_mode(to, None, Some(o));
                self.show_indicator(crate::osd::Indicator::Mode(now));
            }
            // Slow (a model looks at the screen): its own process, which
            // shows progress and sends Arrange when it has an answer.
            S::SmartTile => self.spawn_program("olo-smart-tile", &[]),
            S::ZoomAll => self.zoom_all(),
            // Arrangements that aren't tiles: auto-tiling off there first.
            S::CascadeAll | S::FanOut | S::CascadeOthers | S::FanOutOthers => {
                let keep = matches!(shortcut, S::CascadeOthers | S::FanOutOthers).then_some(focused).flatten();
                let output = keep.as_ref().and_then(|w| self.window_output(w)).or_else(|| self.active_output());
                if let Some(o) = output.filter(|o| self.auto_on(o, self.showing(o))) {
                    self.set_auto_tiling(Some(false), None, Some(o));
                    self.show_indicator(crate::osd::Indicator::Mode(olo_protocol::WindowingMode::Standard));
                }
                match shortcut {
                    S::CascadeAll | S::CascadeOthers => self.cascade_all(keep.as_ref()),
                    _ => self.fan_out(keep.as_ref()),
                }
            }
            S::UndoArrangement => {
                if !self.auto_undo() {
                    self.undo_arrangement();
                }
            }
            S::ShowDesktop => self.show_desktop(),
            // The active display's workspaces; the others stay as they are.
            S::NextWorkspace => self.switch_workspace((self.workspace() + 1).min(workspaces - 1)),
            S::PreviousWorkspace => self.switch_workspace(self.workspace().saturating_sub(1)),
            S::Workspace(i) => self.switch_workspace(u32::from(i)),
            // Along its display's workspaces.
            S::WindowToNextWorkspace | S::WindowToPreviousWorkspace => {
                let Some(w) = focused else { return };
                let here = self.workspace_of(&w);
                let to = if shortcut == S::WindowToNextWorkspace { (here + 1).min(workspaces - 1) } else { here.saturating_sub(1) };
                if to != here {
                    self.move_to_workspace(&w, to);
                    self.restore(&w);
                }
            }
            S::WindowToWorkspace(i) => {
                let Some(w) = focused else { return };
                let to = u32::from(i);
                if to < workspaces && to != self.workspace_of(&w) {
                    self.move_to_workspace(&w, to);
                    self.restore(&w);
                }
            }
            S::RollUp => {
                if let Some(w) = focused {
                    self.toggle_rolled(&w);
                }
            }
            S::NextTab | S::PreviousTab => {
                // From the palette or an agent: the keys the shortcut
                // stands for (keymap.rs), typed into the window.
                let Some(w) = focused else { return };
                if self.focused_window().as_ref() != Some(&w) {
                    self.focus_window(&w);
                }
                use crate::keymap::code::{LEFTCTRL, LEFTSHIFT, TAB};
                let keys: &[u32] = if shortcut == S::PreviousTab { &[LEFTCTRL, LEFTSHIFT, TAB] } else { &[LEFTCTRL, TAB] };
                let time = InputTime::now();
                for (&k, pressed) in keys.iter().map(|k| (k, true)).chain(keys.iter().rev().map(|k| (k, false))) {
                    let state = if pressed { KeyState::Pressed } else { KeyState::Released };
                    self.client_key((k + 8).into(), state, time);
                }
            }
            S::Float => {
                let Some(w) = focused else { return };
                let floats = self.floats(&w);
                self.set_floating(&w, Some(!floats));
            }
            S::Tile(_) | S::Zoom | S::ZoomVertical | S::ZoomHorizontal | S::Restore | S::RestoreOrHide | S::Center | S::Hide | S::NextDisplay | S::PreviousDisplay => {
                let Some(w) = focused else { return };
                // Windows' ⊞↓: a window arranged goes back to its own
                // size, one at its own size hides.
                let shortcut = match shortcut {
                    S::RestoreOrHide if self.arranged(&w) => S::Restore,
                    S::RestoreOrHide => S::Hide,
                    s => s,
                };
                if shortcut != S::Hide {
                    self.front_of_group(&w);
                }
                match shortcut {
                    S::Tile(tile) => {
                        if !self.auto_tile_window(&w, tile) {
                            self.tile_window(&w, tile);
                        }
                    }
                    S::Zoom => {
                        // Unzooming puts it back in its tile, if it was
                        // tiled (Restore goes back to its own frame).
                        if self.is_zoomed(&w) {
                            self.unzoom_window(&w);
                        } else {
                            self.zoom_window(&w);
                        }
                    }
                    S::ZoomVertical | S::ZoomHorizontal => {
                        let axis = if shortcut == S::ZoomVertical { crate::shell::tiling::Axis::Vertical } else { crate::shell::tiling::Axis::Horizontal };
                        if !self.auto_zoom_along(&w, axis) {
                            self.toggle_zoom_along(&w, axis);
                        }
                    }
                    // A tile given its own frame floats over the tiles.
                    S::Restore | S::Center => {
                        if self.auto_tiled(&w) {
                            self.set_floating(&w, Some(true));
                        }
                        if shortcut == S::Restore {
                            self.restore_frame(&w);
                        } else {
                            self.center_window(&w);
                        }
                    }
                    S::Hide => self.minimize_window(&w),
                    S::NextDisplay => self.window_to_next_output(&w, false),
                    S::PreviousDisplay => self.window_to_next_output(&w, true),
                    _ => {}
                }
            }
        }
    }

    fn client_key(&mut self, keycode: smithay::input::keyboard::Keycode, state: KeyState, time: InputTime) -> KeyAction {
        debug!(?keycode, ?state, "key");
        let serial = SCOUNTER.next_serial();
        let mut suppressed_keys = self.suppressed_keys.clone();
        let keyboard = self.seat.get_keyboard().unwrap();

        // Locked: keys go to the lock screen and nowhere else.
        if self.lock.locked() {
            self.lock_guard_focus();
            keyboard.input::<(), _>(self, keycode, state, serial, time, |_, _, _| FilterResult::Forward);
            return KeyAction::None;
        }

        for layer in self.layer_shell_state.layer_surfaces().rev() {
            let exclusive = layer.with_cached_state(|data| data.keyboard_interactivity == KeyboardInteractivity::Exclusive && (data.layer == WlrLayer::Top || data.layer == WlrLayer::Overlay));
            if exclusive {
                let surface = self.space.outputs().find_map(|o| {
                    let map = layer_map_for_output(o);
                    map.layers().find(|l| l.layer_surface() == &layer).cloned()
                });
                if let Some(surface) = surface.filter(crate::rescue::shows_layer) {
                    keyboard.set_focus(self, Some(surface.into()), serial);
                    keyboard.input::<(), _>(self, keycode, state, serial, time, |_, _, _| FilterResult::Forward);
                    return KeyAction::None;
                };
            }
        }

        let inhibited = self
            .space
            .element_under(self.pointer.current_location())
            .and_then(|(window, _)| {
                let surface = window.wl_surface()?;
                self.seat.keyboard_shortcuts_inhibitor_for_surface(&surface)
            })
            .map(|inhibitor| inhibitor.is_active())
            .unwrap_or(false);

        let action = keyboard
            .input(self, keycode, state, serial, time, |_, modifiers, handle| {
                let keysym = handle.modified_sym();

                debug!(
                    ?state,
                    mods = ?modifiers,
                    keysym = ::xkbcommon::xkb::keysym_get_name(keysym),
                    "keysym"
                );

                // If the key is pressed and triggered a action
                // we will not forward the key to the client.
                // Additionally add the key to the suppressed keys
                // so that we can decide on a release if the key
                // should be forwarded to the client or not.
                if let KeyState::Pressed = state {
                    if !inhibited {
                        let action = process_keyboard_shortcut(*modifiers, keysym);

                        if action.is_some() {
                            suppressed_keys.push(keysym);
                        }

                        action.map(FilterResult::Intercept).unwrap_or(FilterResult::Forward)
                    } else {
                        FilterResult::Forward
                    }
                } else {
                    let suppressed = suppressed_keys.contains(&keysym);
                    if suppressed {
                        suppressed_keys.retain(|k| *k != keysym);
                        FilterResult::Intercept(KeyAction::None)
                    } else {
                        FilterResult::Forward
                    }
                }
            })
            .unwrap_or(KeyAction::None);

        self.suppressed_keys = suppressed_keys;
        action
    }

    fn on_pointer_button<B: InputBackend>(&mut self, evt: B::PointerButtonEvent) {
        self.pointer_button(evt.button_code(), wl_pointer::ButtonState::from(evt.state()), evt.time());
    }

    /// A pointer button pressed or let go: a mouse's, a touchpad's, or a
    /// finger on the touch screen being the pointer.
    fn pointer_button(&mut self, button: u32, state: wl_pointer::ButtonState, time: InputTime) {
        self.stop_glide();
        let serial = SCOUNTER.next_serial();

        if self.overview_button(button, state == wl_pointer::ButtonState::Pressed) || self.twitch_button(button, state == wl_pointer::ButtonState::Pressed) {
            return;
        }

        if wl_pointer::ButtonState::Pressed == state {
            self.show_click();
            // Clicking puts keyboard hints away.
            if self.hints.active() {
                self.hints.close();
            }
            // A ⌘-click: letting go of ⌘ after isn't a tap of it.
            self.keymap.command_used();
            self.update_keyboard_focus(self.pointer.current_location(), serial);
        };
        let pointer = self.pointer.clone();
        pointer.button(self, &ButtonEvent { button, state: state.try_into().unwrap(), serial, time });
        pointer.frame(self);
    }

    fn on_gesture_swipe_begin<B: InputBackend>(&mut self, evt: B::GestureSwipeBeginEvent) {
        self.stop_glide();
        let serial = SCOUNTER.next_serial();
        let pointer = self.pointer.clone();
        pointer.gesture_swipe_begin(self, &GestureSwipeBeginEvent { serial, time: evt.time(), fingers: evt.fingers() });
    }

    fn on_gesture_swipe_update<B: InputBackend>(&mut self, evt: B::GestureSwipeUpdateEvent) {
        let pointer = self.pointer.clone();
        pointer.gesture_swipe_update(self, &GestureSwipeUpdateEvent { time: evt.time(), delta: evt.delta() });
    }

    fn on_gesture_swipe_end<B: InputBackend>(&mut self, evt: B::GestureSwipeEndEvent) {
        let serial = SCOUNTER.next_serial();
        let pointer = self.pointer.clone();
        pointer.gesture_swipe_end(self, &GestureSwipeEndEvent { serial, time: evt.time(), cancelled: evt.cancelled() });
    }

    /// A pinch reaches the window under the pointer only with
    /// Preferences ▸ Touchpad's "Pinch to zoom" on: off, two fingers
    /// that come together a little while scrolling don't zoom a page.
    fn on_gesture_pinch_begin<B: InputBackend>(&mut self, evt: B::GesturePinchBeginEvent) {
        self.stop_glide();
        self.pinching = self.settings.touchpad.pinch_zoom;
        if !self.pinching {
            return;
        }
        let serial = SCOUNTER.next_serial();
        let pointer = self.pointer.clone();
        pointer.gesture_pinch_begin(self, &GesturePinchBeginEvent { serial, time: evt.time(), fingers: evt.fingers() });
    }

    fn on_gesture_pinch_update<B: InputBackend>(&mut self, evt: B::GesturePinchUpdateEvent) {
        if !self.pinching {
            return;
        }
        let pointer = self.pointer.clone();
        pointer.gesture_pinch_update(self, &GesturePinchUpdateEvent { time: evt.time(), delta: evt.delta(), scale: evt.scale(), rotation: evt.rotation() });
    }

    fn on_gesture_pinch_end<B: InputBackend>(&mut self, evt: B::GesturePinchEndEvent) {
        if !std::mem::take(&mut self.pinching) {
            return;
        }
        let serial = SCOUNTER.next_serial();
        let pointer = self.pointer.clone();
        pointer.gesture_pinch_end(self, &GesturePinchEndEvent { serial, time: evt.time(), cancelled: evt.cancelled() });
    }

    fn on_gesture_hold_begin<B: InputBackend>(&mut self, evt: B::GestureHoldBeginEvent) {
        // Fingers resting on the touchpad.
        self.stop_glide();
        let serial = SCOUNTER.next_serial();
        let pointer = self.pointer.clone();
        pointer.gesture_hold_begin(self, &GestureHoldBeginEvent { serial, time: evt.time(), fingers: evt.fingers() });
    }

    fn on_gesture_hold_end<B: InputBackend>(&mut self, evt: B::GestureHoldEndEvent) {
        let serial = SCOUNTER.next_serial();
        let pointer = self.pointer.clone();
        pointer.gesture_hold_end(self, &GestureHoldEndEvent { serial, time: evt.time(), cancelled: evt.cancelled() });
    }

    fn update_keyboard_focus(&mut self, location: Point<f64, Logical>, serial: Serial) {
        // A press on a display makes it the active one (while no window
        // has the keyboard: a workspace switched from there).
        if let Some(o) = self.space.output_under(location).next() {
            self.last_display = Some(o.name());
        }
        if self.lock.locked() {
            self.lock_guard_focus();
            return;
        }
        let keyboard = self.seat.get_keyboard().unwrap();
        let touch = self.seat.get_touch();
        let input_method = self.seat.input_method();
        // change the keyboard focus unless the pointer or keyboard is grabbed
        // We test for any matching surface type here but always use the root
        // (in case of a window the toplevel) surface for the focus.
        // So for example if a user clicks on a subsurface or popup the toplevel
        // will receive the keyboard focus. Directly assigning the focus to the
        // matching surface leads to issues with clients dismissing popups and
        // subsurface menus (for example firefox-wayland).
        // see here for a discussion about that issue:
        // https://gitlab.freedesktop.org/wayland/wayland/-/issues/294
        if !self.pointer.is_grabbed() && (!keyboard.is_grabbed() || input_method.keyboard_grabbed()) && !touch.map(|touch| touch.is_grabbed()).unwrap_or(false) {
            let output = self.space.output_under(location).next().cloned();
            if let Some(output) = output.as_ref() {
                let output_geo = self.space.output_geometry(output).unwrap();
                if let Some(window) = output.user_data().get::<FullscreenSurface>().and_then(|f| f.get()).filter(|_| !crate::rescue::active())
                    && let Some((_, _)) = window.surface_under(location - output_geo.loc.to_f64(), WindowSurfaceType::ALL)
                {
                    keyboard.set_focus(self, Some(window.into()), serial);
                    return;
                }

                // The front layer surface of `layer` at `location`, its
                // menus too or (they're over it all: `layer_menu_under`) not.
                let front = |layer, kind| {
                    let at = location - output_geo.loc.to_f64();
                    let layers = layer_map_for_output(output);
                    layers.layers_on(layer).rev().find(|l| crate::rescue::shows_layer(l) && layers.layer_geometry(l).is_some_and(|g| l.surface_under(at - g.loc.to_f64(), kind).is_some())).cloned()
                };
                // One that takes the keyboard gets it; one that never does
                // (Deskbar) leaves it where it is: a click on Deskbar went
                // through to the desktop under it, which took the keyboard
                // from the window being typed into.
                let take = |state: &mut Self, layer: smithay::desktop::LayerSurface| {
                    if layer.can_receive_keyboard_focus() {
                        keyboard.set_focus(state, Some(layer.into()), serial);
                    }
                };
                if let Some(layer) = front(WlrLayer::Overlay, WindowSurfaceType::ALL) {
                    return take(self, layer);
                }
                // A layer surface's menu (Deskbar's, the desktop's), over
                // the windows and the Top layer: not what is under it.
                if self.layer_menu_under(output, location).is_some() {
                    return;
                }
                if let Some(layer) = front(WlrLayer::Top, WindowSurfaceType::ALL - WindowSurfaceType::POPUP) {
                    return take(self, layer);
                }
            }

            if let Some((window, _)) = self.space.element_under(location).map(|(w, p)| (w.clone(), p)) {
                self.space.raise_element(&window, true);
                self.keep_children_above();
                keyboard.set_focus(self, Some(window.into()), serial);
                return;
            }

            if let Some(output) = output.as_ref() {
                let output_geo = self.space.output_geometry(output).unwrap();
                let layers = layer_map_for_output(output);
                let at = location - output_geo.loc.to_f64();
                let under = |layer| layers.layers_on(layer).rev().find(|l| crate::rescue::shows_layer(l) && layers.layer_geometry(l).is_some_and(|g| g.to_f64().contains(at)));
                if let Some(layer) = under(WlrLayer::Bottom).or_else(|| under(WlrLayer::Background))
                    && layer.can_receive_keyboard_focus()
                    && let Some((_, _)) = layer.surface_under(location - output_geo.loc.to_f64() - layers.layer_geometry(layer).unwrap().loc.to_f64(), WindowSurfaceType::ALL)
                {
                    keyboard.set_focus(self, Some(layer.clone().into()), serial);
                    return;
                }
                drop(layers);
                // Nothing there takes it: the bare desktop of another
                // display than the focused window's is the one in use now
                // (as the main display's desktop takes the keyboard), so
                // no window keeps it, and that display is the active one.
                if self.focused_window().is_some_and(|w| self.window_output(&w).as_ref() != Some(output)) {
                    keyboard.set_focus(self, None, serial);
                }
            };
        }
    }

    pub fn surface_under(&self, pos: Point<f64, Logical>) -> Option<(PointerFocusTarget, Point<f64, Logical>)> {
        if self.lock.locked() {
            return self.lock_surface_under(pos).map(|(s, loc)| (PointerFocusTarget::from(s), loc));
        }
        let output = self.space.outputs().find(|o| {
            let geometry = self.space.output_geometry(o).unwrap();
            geometry.contains(pos.to_i32_round())
        })?;
        let output_geo = self.space.output_geometry(output).unwrap();
        // (Before the layer map is taken: it takes it too.)
        let menu = self.layer_menu_under(output, pos).map(|(surface, loc)| (PointerFocusTarget::from(surface), loc));
        let layers = layer_map_for_output(output);
        // The front surface of `layer` at `pos`: the Overlay layer's with
        // its menus, the rest without (`menu` has them, over everything).
        let layer_under = |layer| {
            let kind = if layer == WlrLayer::Overlay { WindowSurfaceType::ALL } else { WindowSurfaceType::ALL - WindowSurfaceType::POPUP };
            let at = pos - output_geo.loc.to_f64();
            layers.layers_on(layer).rev().filter(|l| crate::rescue::shows_layer(l)).find_map(|l| {
                let layer_loc = layers.layer_geometry(l)?.loc;
                l.surface_under(at - layer_loc.to_f64(), kind).map(|(surface, loc)| (PointerFocusTarget::from(surface), loc + layer_loc + output_geo.loc))
            })
        };

        // The overview out: only what is over it (Deskbar) takes the
        // pointer; windows and the desktop under it don't.
        if self.overview.as_ref().is_some_and(|o| !o.leaving()) {
            return layer_under(WlrLayer::Overlay).or(menu).or_else(|| layer_under(WlrLayer::Top)).map(|(s, l)| (s, l.to_f64()));
        }
        let mut under = None;
        // (In Rescue a full-screen window is put away with the rest.)
        let fullscreen = output.user_data().get::<FullscreenSurface>().and_then(|f| f.get()).filter(|_| !crate::rescue::active());
        if let Some((surface, loc)) = fullscreen.and_then(|w| w.surface_under(pos - output_geo.loc.to_f64(), WindowSurfaceType::ALL)) {
            under = Some((surface, loc + output_geo.loc));
        } else if let Some(focus) = layer_under(WlrLayer::Overlay)
            .or(menu)
            // A window's menus, drawn over Deskbar (`menu_elements`).
            .or_else(|| self.window_menu_under(pos))
            .or_else(|| layer_under(WlrLayer::Top))
        {
            under = Some(focus)
        } else if let Some(focus) =
            self.space.element_under(pos).and_then(|(window, loc)| window.surface_under(pos - loc.to_f64(), WindowSurfaceType::ALL).map(|(surface, surf_loc)| (surface, surf_loc + loc)))
        {
            under = Some(focus);
        } else if let Some(focus) = layer_under(WlrLayer::Bottom).or_else(|| layer_under(WlrLayer::Background)) {
            under = Some(focus)
        };
        under.map(|(s, l)| (s, l.to_f64()))
    }

    /// A window's menu at `pos` (global): its surface there and where it
    /// is, the front window's first.
    fn window_menu_under(&self, pos: Point<f64, Logical>) -> Option<(PointerFocusTarget, Point<i32, Logical>)> {
        self.space.elements().rev().find_map(|w| {
            let at = self.space.element_location(w)? - smithay::desktop::space::SpaceElement::geometry(w).loc;
            w.menu_under(pos - at.to_f64()).map(|(s, loc)| (s, loc + at))
        })
    }

    /// The surface of a layer surface's menu at `pos` (global), Deskbar's
    /// or the desktop's, and where it is (`shell::layer_menus`). Takes
    /// `output`'s layer map.
    fn layer_menu_under(&self, output: &smithay::output::Output, pos: Point<f64, Logical>) -> Option<(smithay::reexports::wayland_server::protocol::wl_surface::WlSurface, Point<i32, Logical>)> {
        let origin = self.space.output_geometry(output)?.loc;
        crate::shell::layer_menus(output, &crate::shell::MENU_LAYERS).into_iter().find_map(|(menu, at)| under_from_surface_tree(&menu, pos, origin + at, WindowSurfaceType::ALL))
    }

    fn on_pointer_axis<B: InputBackend>(&mut self, evt: B::PointerAxisEvent) {
        // Nothing under the overview scrolls.
        if self.overview.as_ref().is_some_and(|o| !o.leaving()) {
            return;
        }
        let horizontal_amount = evt.amount(input::Axis::Horizontal).unwrap_or_else(|| evt.amount_v120(input::Axis::Horizontal).unwrap_or(0.0) * 15.0 / 120.);
        let vertical_amount = evt.amount(input::Axis::Vertical).unwrap_or_else(|| evt.amount_v120(input::Axis::Vertical).unwrap_or(0.0) * 15.0 / 120.);
        let horizontal_amount_discrete = evt.amount_v120(input::Axis::Horizontal);
        let vertical_amount_discrete = evt.amount_v120(input::Axis::Vertical);
        // Preferences ▸ Touchpad & Mouse ▸ Scrolling speed, before momentum
        // samples it so a glide goes as far again.
        let (horizontal_amount, vertical_amount) = match evt.source() {
            AxisSource::Finger => {
                let k = self.settings.touchpad.scroll_speed.clamp(0.25, 4.0);
                (horizontal_amount * k, vertical_amount * k)
            }
            _ => (horizontal_amount, vertical_amount),
        };

        {
            let mut frame = AxisFrame::new(evt.time()).source(evt.source());
            if horizontal_amount != 0.0 {
                frame = frame.relative_direction(Axis::Horizontal, evt.relative_direction(Axis::Horizontal));
                frame = frame.value(Axis::Horizontal, horizontal_amount);
                if let Some(discrete) = horizontal_amount_discrete {
                    frame = frame.v120(Axis::Horizontal, discrete as i32);
                }
            }
            if vertical_amount != 0.0 {
                frame = frame.relative_direction(Axis::Vertical, evt.relative_direction(Axis::Vertical));
                frame = frame.value(Axis::Vertical, vertical_amount);
                if let Some(discrete) = vertical_amount_discrete {
                    frame = frame.v120(Axis::Vertical, discrete as i32);
                }
            }
            if evt.source() == AxisSource::Finger {
                // Sent on the frame clock (`pace.rs`), the stop too, by
                // when the pad reported it.
                let now = crate::pace::reported(evt.time().micros());
                let (h, v) = (evt.amount(Axis::Horizontal), evt.amount(Axis::Vertical));
                if h.unwrap_or(0.0) == 0.0 && v.unwrap_or(0.0) == 0.0 {
                    self.pacer.lifted(now);
                } else {
                    self.scroll_direction = (evt.relative_direction(Axis::Horizontal), evt.relative_direction(Axis::Vertical));
                    let out = self.pacer.fingers(now, horizontal_amount, vertical_amount);
                    self.send_paced(out);
                }
                return;
            }
            self.stop_glide();
            let pointer = self.pointer.clone();
            pointer.axis(self, frame);
            pointer.frame(self);
        }
    }

    /// Fingers back on the touchpad, a click or another scroll: a glide
    /// stops where it is (what the fingers scrolled is sent first).
    pub fn stop_glide(&mut self) {
        self.send_held_stop();
        let out = self.pacer.stop();
        self.send_paced(out);
    }

    /// Chromium flings on its own when a finger scroll stops within
    /// 200 ms of its last move, however long the fingers rested, where
    /// Olo's apps don't glide (`momentum.rs`, RESTED): so its stop waits
    /// until then (or until whatever comes next). Whether it was held.
    fn hold_stop(&mut self) -> bool {
        let Some(PointerFocusTarget::WlSurface(surface)) = self.pointer.current_focus() else {
            return false;
        };
        let now = std::time::Instant::now();
        let due = match self.last_paced_move {
            Some(t) if t + crate::scroll::CHROMIUM_FLINGS_WITHIN > now => t + crate::scroll::CHROMIUM_FLINGS_WITHIN,
            _ => return false,
        };
        let chromium = surface.client().and_then(|c| crate::state::client_pid(&c, &self.display_handle));
        if chromium.map(crate::scroll::toolkit) != Some(crate::scroll::Toolkit::Chromium) {
            return false;
        }
        self.held_stop = Some((surface, due));
        let timer = smithay::reexports::calloop::timer::Timer::from_deadline(due);
        let _ = self.handle.insert_source(timer, |_, _, state| {
            if state.held_stop.as_ref().is_some_and(|(_, due)| *due <= std::time::Instant::now()) {
                state.send_held_stop();
            }
            smithay::reexports::calloop::timer::TimeoutAction::Drop
        });
        true
    }

    /// The held stop, now, if the pointer is still on its window.
    fn send_held_stop(&mut self) {
        let Some((surface, _)) = self.held_stop.take() else { return };
        if !matches!(self.pointer.current_focus(), Some(PointerFocusTarget::WlSurface(s)) if s == surface) {
            return;
        }
        let frame = AxisFrame::new(InputTime::now()).source(AxisSource::Finger).stop(Axis::Horizontal).stop(Axis::Vertical);
        let pointer = self.pointer.clone();
        pointer.axis(self, frame);
        pointer.frame(self);
    }

    /// A frame of `output`: if the pointer is on it, the scroll that
    /// goes with it (`pace.rs`), before clients are told to draw.
    pub fn pace_scroll(&mut self, output: &smithay::output::Output) {
        let pointer = self.pointer.current_location();
        let ours = match self.space.output_under(pointer).next() {
            Some(o) => o == output,
            None => self.space.outputs().next() == Some(output),
        };
        if ours {
            let out = self.pacer.tick(std::time::Instant::now());
            self.send_paced(out);
        }
    }

    fn send_paced(&mut self, out: Vec<crate::pace::Out>) {
        if out.is_empty() {
            return;
        }
        // A held stop goes before what comes after it.
        self.send_held_stop();
        // Stamped with when the fingers were where it says.
        let time = InputTime::from_micros(InputTime::now().micros().saturating_sub(self.pacer.delay().as_micros() as u64));
        let pointer = self.pointer.clone();
        let (h_dir, v_dir) = self.scroll_direction;
        for o in out {
            let mut frame = AxisFrame::new(time).source(AxisSource::Finger);
            match o {
                crate::pace::Out::Move { dx, dy, glide } => {
                    self.pacer.sending_glide = glide;
                    if dx != 0.0 {
                        frame = frame.relative_direction(Axis::Horizontal, h_dir).value(Axis::Horizontal, dx);
                    }
                    if dy != 0.0 {
                        frame = frame.relative_direction(Axis::Vertical, v_dir).value(Axis::Vertical, dy);
                    }
                    self.last_paced_move = Some(std::time::Instant::now());
                }
                crate::pace::Out::Stop { glide } => {
                    self.pacer.sending_glide = glide;
                    if !glide && self.hold_stop() {
                        continue;
                    }
                    frame = frame.stop(Axis::Horizontal).stop(Axis::Vertical);
                }
            }
            pointer.axis(self, frame);
            pointer.frame(self);
        }
        self.pacer.sending_glide = false;
    }

    fn touch_location_transformed<B: InputBackend, E: AbsolutePositionEvent<B>>(&self, evt: &E) -> Option<Point<f64, Logical>> {
        let output = self.space.outputs().find(|output| output.name().starts_with("eDP")).or_else(|| self.space.outputs().next());

        let output = output?;
        let output_geometry = self.space.output_geometry(output)?;

        let transform = output.current_transform();
        let size = transform.invert().transform_size(output_geometry.size);
        Some(transform.transform_point_in(evt.position_transformed(size), &size.to_f64()) + output_geometry.loc.to_f64())
    }

    /// Whether what a finger came down on takes touch: a client's
    /// surface whose client has `wl_touch` (Chrome, GTK). Not app_server's
    /// own frames and tabs, nor an app without it (every GPUI app, so
    /// every Olo app): there the finger is the pointer.
    fn takes_touch(&self, under: &Option<(PointerFocusTarget, Point<f64, Logical>)>) -> bool {
        use smithay::reexports::wayland_server::Resource as _;
        let (Some(touch), Some((PointerFocusTarget::WlSurface(surface), _))) = (self.seat.get_touch(), under) else { return false };
        surface.client().is_some_and(|client| touch.client_touch(&client).next().is_some())
    }

    /// The touch screen's finger as the pointer, moved to `at`, as a
    /// mouse's would be (window moves and resizes, menus, the overview).
    fn touch_pointer_to(&mut self, at: Point<f64, Logical>, time: InputTime) {
        // (On the touch screen's display: no clamping needed.)
        let under = self.surface_under(at);
        let pointer = self.pointer.clone();
        pointer.motion(self, under, &MotionEvent { location: at, serial: SCOUNTER.next_serial(), time });
        pointer.frame(self);
        self.overview_pointer();
        self.twitch_pointer();
    }

    fn on_touch_down<B: InputBackend>(&mut self, evt: B::TouchDownEvent) {
        let Some(touch_location) = self.touch_location_transformed(&evt) else {
            return;
        };
        self.touch_down(evt.slot(), touch_location, evt.time());
    }

    fn on_touch_up<B: InputBackend>(&mut self, evt: B::TouchUpEvent) {
        self.touch_up(evt.slot(), evt.time());
    }

    fn on_touch_motion<B: InputBackend>(&mut self, evt: B::TouchMotionEvent) {
        let Some(touch_location) = self.touch_location_transformed(&evt) else {
            return;
        };
        self.touch_motion(evt.slot(), touch_location, evt.time());
    }

    /// A finger down on the touch screen at `touch_location` (a real one,
    /// or `SyntheticEvent::Touch`).
    fn touch_down(&mut self, touch_slot: input::TouchSlot, touch_location: Point<f64, Logical>, time: InputTime) {
        let slot = i32::from(touch_slot);
        // (The pointer hides while a finger works: until a mouse moves.)
        self.touch_hides_pointer = true;
        // A second finger while one is the pointer: the two scroll (the
        // button, if the first had pressed it, let go first).
        if let Some(mut tp) = self.touch_pointer {
            if tp.scroll.is_none() && !tp.lifting {
                if tp.pressed {
                    self.pointer_button(BTN_LEFT, wl_pointer::ButtonState::Released, time);
                    tp.pressed = false;
                }
                tp.scroll = Some([(tp.slot, tp.at), (slot, touch_location)]);
                tp.started = [tp.at, touch_location];
                tp.two = TwoFingers::Undecided;
                self.touch_pointer = Some(tp);
            }
            return;
        }
        let under = self.surface_under(touch_location);
        // A finger on what takes no touch is the pointer, moved there; its
        // button pressed only once it moves (a drag) or at the lift (a
        // tap), so a second finger first scrolls instead.
        if !self.takes_touch(&under) {
            self.touch_pointer =
                Some(TouchPointer { slot, down_at: touch_location, at: touch_location, pressed: false, scroll: None, lifting: false, started: [touch_location; 2], two: TwoFingers::Undecided });
            self.touch_pointer_to(touch_location, time);
            return;
        }
        let Some(handle) = self.seat.get_touch() else {
            return;
        };
        let serial = SCOUNTER.next_serial();
        self.update_keyboard_focus(touch_location, serial);
        handle.down(self, under, &DownEvent { slot: touch_slot, location: touch_location, serial, time });
    }

    fn touch_up(&mut self, slot: input::TouchSlot, time: InputTime) {
        let id = i32::from(slot);
        if let Some(mut tp) = self.touch_pointer {
            if let Some(fingers) = tp.scroll {
                if fingers.iter().any(|(s, _)| *s == id) {
                    // One of the two lifts: the scroll or the pinch ends,
                    // and the other does nothing until it lifts too.
                    if !tp.lifting {
                        let pointer = self.pointer.clone();
                        match tp.two {
                            TwoFingers::Scroll => {
                                let frame = AxisFrame::new(time).source(AxisSource::Finger).stop(Axis::Horizontal).stop(Axis::Vertical);
                                pointer.axis(self, frame);
                                pointer.frame(self);
                            }
                            TwoFingers::Pinch { .. } => {
                                let serial = SCOUNTER.next_serial();
                                pointer.gesture_pinch_end(self, &GesturePinchEndEvent { serial, time, cancelled: false });
                            }
                            TwoFingers::Undecided => {}
                        }
                        tp.lifting = true;
                        tp.scroll = Some(fingers.map(|f| if f.0 == id { (-1, f.1) } else { f }));
                        self.touch_pointer = Some(tp);
                    } else {
                        self.touch_pointer = None;
                    }
                }
                return;
            }
            if tp.slot == id {
                // A tap: pressed and let go where it was.
                if !tp.pressed {
                    self.pointer_button(BTN_LEFT, wl_pointer::ButtonState::Pressed, time);
                }
                self.pointer_button(BTN_LEFT, wl_pointer::ButtonState::Released, time);
                self.touch_pointer = None;
            }
            return;
        }
        let Some(handle) = self.seat.get_touch() else {
            return;
        };
        let serial = SCOUNTER.next_serial();
        handle.up(self, &UpEvent { slot, serial, time })
    }

    fn touch_motion(&mut self, slot: input::TouchSlot, touch_location: Point<f64, Logical>, time: InputTime) {
        let id = i32::from(slot);
        if let Some(mut tp) = self.touch_pointer {
            match tp.scroll {
                Some(mut fingers) => {
                    if tp.lifting {
                        return;
                    }
                    let Some(i) = fingers.iter().position(|(s, _)| *s == id) else { return };
                    let middle = |a: Point<f64, Logical>, b: Point<f64, Logical>| (a + b).downscale(2.0);
                    let apart = |a: Point<f64, Logical>, b: Point<f64, Logical>| (a.x - b.x).hypot(a.y - b.y);
                    let before = middle(fingers[0].1, fingers[1].1);
                    fingers[i].1 = touch_location;
                    let now = middle(fingers[0].1, fingers[1].1);
                    let moved = now - before;
                    tp.scroll = Some(fingers);
                    // What the two do, once they've moved: closer or
                    // further apart is a pinch (the window's, as a
                    // touchpad's), together a scroll.
                    if tp.two == TwoFingers::Undecided {
                        let start = middle(tp.started[0], tp.started[1]);
                        let travel = (now.x - start.x).hypot(now.y - start.y);
                        let spread = (apart(fingers[0].1, fingers[1].1) - apart(tp.started[0], tp.started[1])).abs();
                        if travel.max(spread) < TOUCH_DECIDE {
                            self.touch_pointer = Some(tp);
                            return;
                        }
                        tp.two = if spread > travel { TwoFingers::Pinch { apart: apart(tp.started[0], tp.started[1]).max(1.0) } } else { TwoFingers::Scroll };
                        if matches!(tp.two, TwoFingers::Pinch { .. }) {
                            let serial = SCOUNTER.next_serial();
                            let pointer = self.pointer.clone();
                            pointer.gesture_pinch_begin(self, &GesturePinchBeginEvent { serial, time, fingers: 2 });
                        }
                    }
                    self.touch_pointer = Some(tp);
                    if let TwoFingers::Pinch { apart: from } = tp.two {
                        let scale = apart(fingers[0].1, fingers[1].1) / from;
                        let pointer = self.pointer.clone();
                        pointer.gesture_pinch_update(self, &GesturePinchUpdateEvent { time, delta: (moved.x, moved.y).into(), scale, rotation: 0.0 });
                        return;
                    }
                    // The content follows the fingers (a touch screen's
                    // scroll is direct: inverted, as natural scrolling).
                    let mut frame = AxisFrame::new(time).source(AxisSource::Finger);
                    if moved.x != 0.0 {
                        frame = frame.relative_direction(Axis::Horizontal, smithay::backend::input::AxisRelativeDirection::Inverted).value(Axis::Horizontal, -moved.x);
                    }
                    if moved.y != 0.0 {
                        frame = frame.relative_direction(Axis::Vertical, smithay::backend::input::AxisRelativeDirection::Inverted).value(Axis::Vertical, -moved.y);
                    }
                    let pointer = self.pointer.clone();
                    pointer.axis(self, frame);
                    pointer.frame(self);
                }
                None if tp.slot == id => {
                    tp.at = touch_location;
                    // Moved past a few points: a drag, pressed where the
                    // finger came down.
                    if !tp.pressed && (touch_location - tp.down_at).to_f64().x.hypot((touch_location - tp.down_at).y) > TOUCH_DRAG {
                        tp.pressed = true;
                        self.touch_pointer = Some(tp);
                        self.touch_pointer_to(tp.down_at, time);
                        self.pointer_button(BTN_LEFT, wl_pointer::ButtonState::Pressed, time);
                    }
                    self.touch_pointer = Some(tp);
                    self.touch_pointer_to(touch_location, time);
                }
                None => {}
            }
            return;
        }
        let Some(handle) = self.seat.get_touch() else {
            return;
        };
        let under = self.surface_under(touch_location);
        handle.motion(self, under, &smithay::input::touch::MotionEvent { slot, location: touch_location, time });
    }

    fn on_touch_frame<B: InputBackend>(&mut self, _evt: B::TouchFrameEvent) {
        let Some(handle) = self.seat.get_touch() else {
            return;
        };
        handle.frame(self);
    }

    fn on_touch_cancel<B: InputBackend>(&mut self, _evt: B::TouchCancelEvent) {
        // (The finger being the pointer lets go.)
        if self.touch_pointer.take().is_some_and(|tp| tp.pressed) {
            self.pointer_button(BTN_LEFT, wl_pointer::ButtonState::Released, InputTime::now());
        }
        let Some(handle) = self.seat.get_touch() else {
            return;
        };
        handle.cancel(self);
    }

    fn on_device_added<B: InputBackend>(&mut self, device: B::Device) {
        // A security key's keyboard (its one-time codes): known by the
        // device, its typing taken from now on.
        if device.has_capability(DeviceCapability::Keyboard)
            && let Some(id) = device.usb_id()
            && olo_protocol::keyboards::security_key(&device.name(), id.1)
            && !self.security_key_ids.contains(&id)
        {
            info!(name = device.name(), "a security key's keyboard: its touches type nothing on their own");
            self.security_key_ids.push(id);
        }
        let dh = &self.display_handle;
        if device.has_capability(DeviceCapability::TabletTool) {
            self.seat.tablet_seat().add_wp_tablet(dh, &TabletDescriptor::from(&device));
        }
        if device.has_capability(DeviceCapability::Touch) && self.seat.get_touch().is_none() {
            self.seat.add_touch();
        }
    }

    fn on_device_removed<B: InputBackend>(&mut self, device: B::Device) {
        if device.has_capability(DeviceCapability::TabletTool) {
            let tablet_seat = self.seat.tablet_seat();

            tablet_seat.remove_tablet(&TabletDescriptor::from(&device));

            // If there are no tablets in seat we can remove all tools
            if tablet_seat.count_tablets() == 0 {
                tablet_seat.clear_tools();
            }
        }
    }
}

#[cfg(feature = "winit")]
impl<BackendData: Backend> AnvilState<BackendData> {
    pub fn process_input_event_windowed<B: InputBackend>(&mut self, event: InputEvent<B>, output_name: &str) {
        self.backend_data.wake(None);
        self.power_activity();
        match event {
            InputEvent::Keyboard { event } => match self.keyboard_key_to_action::<B>(event) {
                KeyAction::ScaleUp => {
                    let output = self.space.outputs().find(|o| o.name() == output_name).unwrap().clone();

                    let current_scale = output.current_scale().fractional_scale();
                    let new_scale = current_scale + 0.25;
                    output.change_current_state(None, None, Some(Scale::Fractional(new_scale)), None);

                    crate::shell::fixup_positions(&mut self.space, &self.settings.display, self.pointer.current_location());
                    self.backend_data.reset_buffers(&output);
                }

                KeyAction::ScaleDown => {
                    let output = self.space.outputs().find(|o| o.name() == output_name).unwrap().clone();

                    let current_scale = output.current_scale().fractional_scale();
                    let new_scale = f64::max(1.0, current_scale - 0.25);
                    output.change_current_state(None, None, Some(Scale::Fractional(new_scale)), None);

                    crate::shell::fixup_positions(&mut self.space, &self.settings.display, self.pointer.current_location());
                    self.backend_data.reset_buffers(&output);
                }

                KeyAction::RotateOutput => {
                    let output = self.space.outputs().find(|o| o.name() == output_name).unwrap().clone();

                    let current_transform = output.current_transform();
                    let new_transform = match current_transform {
                        Transform::Normal => Transform::_90,
                        Transform::_90 => Transform::_180,
                        Transform::_180 => Transform::_270,
                        Transform::_270 => Transform::Flipped,
                        Transform::Flipped => Transform::Flipped90,
                        Transform::Flipped90 => Transform::Flipped180,
                        Transform::Flipped180 => Transform::Flipped270,
                        Transform::Flipped270 => Transform::Normal,
                    };
                    tracing::info!(?current_transform, ?new_transform, output = ?output.name(), "changing output transform");
                    output.change_current_state(None, Some(new_transform), None, None);
                    crate::shell::fixup_positions(&mut self.space, &self.settings.display, self.pointer.current_location());
                    self.backend_data.reset_buffers(&output);
                }

                action => match action {
                    KeyAction::None | KeyAction::Quit | KeyAction::Run(_) | KeyAction::ToggleDecorations => self.process_common_key_action(action),

                    _ => tracing::warn!(?action, output_name, "Key action unsupported on on output backend.",),
                },
            },

            InputEvent::PointerMotionAbsolute { event } => {
                let output = self.space.outputs().find(|o| o.name() == output_name).unwrap().clone();
                self.on_pointer_move_absolute_windowed::<B>(event, &output)
            }
            InputEvent::PointerButton { event } => self.on_pointer_button::<B>(event),
            InputEvent::PointerAxis { event } => self.on_pointer_axis::<B>(event),

            InputEvent::GestureSwipeBegin { event, .. } => self.on_gesture_swipe_begin::<B>(event),
            InputEvent::GestureSwipeUpdate { event, .. } => self.on_gesture_swipe_update::<B>(event),
            InputEvent::GestureSwipeEnd { event, .. } => self.on_gesture_swipe_end::<B>(event),
            InputEvent::GesturePinchBegin { event, .. } => self.on_gesture_pinch_begin::<B>(event),
            InputEvent::GesturePinchUpdate { event, .. } => self.on_gesture_pinch_update::<B>(event),
            InputEvent::GesturePinchEnd { event, .. } => self.on_gesture_pinch_end::<B>(event),
            InputEvent::GestureHoldBegin { event, .. } => self.on_gesture_hold_begin::<B>(event),
            InputEvent::GestureHoldEnd { event, .. } => self.on_gesture_hold_end::<B>(event),

            InputEvent::TouchDown { event } => self.on_touch_down::<B>(event),
            InputEvent::TouchUp { event } => self.on_touch_up::<B>(event),
            InputEvent::TouchMotion { event } => self.on_touch_motion::<B>(event),
            InputEvent::TouchFrame { event } => self.on_touch_frame::<B>(event),
            InputEvent::TouchCancel { event } => self.on_touch_cancel::<B>(event),
            InputEvent::DeviceAdded { device } => self.on_device_added::<B>(device),
            InputEvent::DeviceRemoved { device } => self.on_device_removed::<B>(device),
            _ => (), // other events are not handled in anvil (yet)
        }
    }

    fn on_pointer_move_absolute_windowed<B: InputBackend>(&mut self, evt: B::PointerMotionAbsoluteEvent, output: &Output) {
        let output_geo = self.space.output_geometry(output).unwrap();

        let pos = evt.position_transformed(output_geo.size) + output_geo.loc.to_f64();
        let serial = SCOUNTER.next_serial();

        let pointer = self.pointer.clone();
        let under = self.surface_under(pos);
        self.pointer_track.note(std::time::Duration::from_micros(evt.time().micros()), pos);
        pointer.motion(self, under, &MotionEvent { location: pos, serial, time: evt.time() });
        pointer.frame(self);
        self.overview_pointer();
        self.twitch_pointer();
    }

    /// Input injected over IPC by UI tests.
    /// Take the next key as a shortcut (Preferences is recording one), for
    /// up to 30 s.
    pub fn record_shortcut(&mut self, on: bool) {
        self.keymap.record(on);
        self.recording_shortcut += 1;
        if !on {
            return;
        }
        let ticket = self.recording_shortcut;
        let timer = smithay::reexports::calloop::timer::Timer::from_duration(std::time::Duration::from_secs(30));
        let _ = self.handle.insert_source(timer, move |_, _, state| {
            if state.recording_shortcut == ticket && state.keymap.recording() {
                state.keymap.record(false);
                state.ipc.broadcast(olo_protocol::Event::ShortcutRecorded { keys: None });
            }
            smithay::reexports::calloop::timer::TimeoutAction::Drop
        });
    }

    /// Move the pointer to `pos` (its display went away).
    pub fn warp_pointer(&mut self, pos: Point<f64, Logical>) {
        let under = self.surface_under(pos);
        let pointer = self.pointer.clone();
        pointer.motion(self, under, &MotionEvent { location: pos, serial: SCOUNTER.next_serial(), time: InputTime::now() });
        pointer.frame(self);
    }

    /// The displays under the pointer, where it was (`before`) and is, look
    /// again if resting (`Backend::wake`).
    pub fn wake_under_pointer(&mut self, before: Point<f64, Logical>) {
        let now = self.pointer.current_location();
        let mut under: Vec<Output> = self.space.output_under(before).cloned().collect();
        for o in self.space.output_under(now) {
            if !under.contains(o) {
                under.push(o.clone());
            }
        }
        if under.is_empty() {
            self.backend_data.wake(None);
        } else {
            self.backend_data.wake(Some(&under));
        }
    }

    /// Physical keys pressed together and let go, as a keyboard would
    /// (`insert::press_keys`): through the profile, its shortcuts too.
    pub(crate) fn press_physical(&mut self, keys: &[u32]) {
        let time = InputTime::now();
        for &k in keys {
            let _ = self.physical_key(k, true, time);
        }
        for &k in keys.iter().rev() {
            let _ = self.physical_key(k, false, time);
        }
    }

    pub fn synthetic_input(&mut self, event: olo_protocol::SyntheticEvent) {
        use olo_protocol::SyntheticEvent as S;
        // As real input wakes the displays (`process_input_event`).
        let before = self.pointer.current_location();
        let pointing = matches!(event, S::Move { .. } | S::Scroll { .. } | S::Touch { .. });
        // (Counted as real input is, for Activity Monitor ▸ Inputs.)
        let counted = match &event {
            S::Button { pressed: true, .. } | S::Touch { phase: olo_protocol::TouchPhase::Down, .. } => Some(crate::input_counts::Input::Click),
            S::Key { pressed: true, .. } => Some(crate::input_counts::Input::Key),
            S::Scroll { .. } | S::Wheel { .. } => Some(crate::input_counts::Input::Scroll),
            _ => None,
        };
        if let Some(what) = counted {
            self.input_counts.note(what, std::time::Instant::now());
        }
        self.synthetic_input_inner(event);
        if pointing {
            self.wake_under_pointer(before);
        } else {
            self.backend_data.wake(None);
        }
    }

    fn synthetic_input_inner(&mut self, event: olo_protocol::SyntheticEvent) {
        use olo_protocol::SyntheticEvent as S;
        self.power_activity();
        let serial = SCOUNTER.next_serial();
        let time = InputTime::now();
        let pointer = self.pointer.clone();
        match event {
            S::Move { x, y } => {
                let pos: Point<f64, Logical> = (x, y).into();
                let under = self.surface_under(pos);
                pointer.motion(self, under, &MotionEvent { location: pos, serial, time });
                pointer.frame(self);
                self.overview_pointer();
                self.twitch_pointer();
            }
            S::Button { button, pressed } => {
                if self.overview_button(button, pressed) || self.twitch_button(button, pressed) {
                    return;
                }
                if pressed {
                    // (An agent's clicks shown too: its demo's.)
                    self.show_click();
                    self.update_keyboard_focus(pointer.current_location(), serial);
                }
                let state = if pressed { smithay::backend::input::ButtonState::Pressed } else { smithay::backend::input::ButtonState::Released };
                pointer.button(self, &ButtonEvent { button, state, serial, time });
                pointer.frame(self);
            }
            S::Key { key, pressed } => {
                let _ = serial;
                let action = self.physical_key(key, pressed, time);
                if let KeyAction::Quit = action {
                    self.process_common_key_action(action);
                }
            }
            S::Wheel { dx, dy } => {
                // As libinput reports a wheel: 15° and 120 a notch.
                self.stop_glide();
                let mut frame = AxisFrame::new(time).source(AxisSource::Wheel);
                if dx != 0 {
                    frame = frame.value(Axis::Horizontal, dx as f64 * 15.0).v120(Axis::Horizontal, dx * 120);
                }
                if dy != 0 {
                    frame = frame.value(Axis::Vertical, dy as f64 * 15.0).v120(Axis::Vertical, dy * 120);
                }
                pointer.axis(self, frame);
                pointer.frame(self);
            }
            S::Touch { slot, phase, x, y } => {
                // As a finger on a touch screen (agents, tests).
                let _ = pointer;
                let slot = input::TouchSlot::from(Some(slot));
                let at: Point<f64, Logical> = (x, y).into();
                match phase {
                    olo_protocol::TouchPhase::Down => {
                        self.touch_down(slot, at, time);
                        if let Some(t) = self.seat.get_touch() {
                            t.frame(self);
                        }
                    }
                    olo_protocol::TouchPhase::Motion => {
                        self.touch_motion(slot, at, time);
                        if let Some(t) = self.seat.get_touch() {
                            t.frame(self);
                        }
                    }
                    olo_protocol::TouchPhase::Up => {
                        self.touch_up(slot, time);
                        if let Some(t) = self.seat.get_touch() {
                            t.frame(self);
                        }
                    }
                }
            }
            S::Contacts { contacts } => {
                // As a MacBook pad's report would say (testing the palm
                // trainer headless).
                let _ = (pointer, time);
                if self.ipc.wanted(olo_protocol::EventKind::TouchContacts) {
                    let pad = olo_protocol::touchpad::PadAxes::macbook();
                    self.ipc.broadcast(olo_protocol::Event::TouchContacts { pad, time_us: crate::contacts::now_us(), contacts, since_key_us: crate::contacts::since_key_us() });
                }
            }
            S::Scroll { dx, dy, at_us } => {
                // As from the touchpad: 0, 0 is the fingers lifting.
                let _ = (pointer, time);
                let now = at_us.map_or_else(std::time::Instant::now, crate::pace::reported);
                if dx == 0.0 && dy == 0.0 {
                    self.pacer.lifted(now);
                } else {
                    let out = self.pacer.fingers(now, dx, dy);
                    self.send_paced(out);
                }
            }
        }
    }

    pub fn release_all_keys(&mut self) {
        let keyboard = self.seat.get_keyboard().unwrap();
        for keycode in keyboard.pressed_keys() {
            keyboard.input(self, keycode, KeyState::Released, SCOUNTER.next_serial(), InputTime::now(), |_, _, _| FilterResult::Forward::<bool>);
        }
    }
}

#[cfg(feature = "udev")]
impl AnvilState<UdevData> {
    pub fn process_input_event<B: InputBackend>(&mut self, dh: &DisplayHandle, event: InputEvent<B>) {
        // The pointer moving or scrolling changes only what is under it
        // (the pointer drawn, a scroll paced there): those displays look.
        // Anything else may show something of ours anywhere: every one.
        let pointing = matches!(event, InputEvent::PointerMotion { .. } | InputEvent::PointerMotionAbsolute { .. } | InputEvent::PointerAxis { .. });
        let before = self.pointer.current_location();
        if !pointing {
            self.backend_data.wake(None);
        }
        let start = std::time::Instant::now();
        {
            use crate::input_counts::Input;
            let counted = match &event {
                InputEvent::Keyboard { event } if event.state() == KeyState::Pressed => Some(Input::Key),
                InputEvent::PointerButton { event } if event.state() == smithay::backend::input::ButtonState::Pressed => Some(Input::Click),
                InputEvent::PointerAxis { .. } => Some(Input::Scroll),
                InputEvent::GestureSwipeBegin { .. } | InputEvent::GesturePinchBegin { .. } | InputEvent::GestureHoldBegin { .. } => Some(Input::Gesture),
                _ => None,
            };
            if let Some(what) = counted {
                self.input_counts.note(what, start);
            }
        }
        let kind = match &event {
            InputEvent::Keyboard { .. } => "key",
            InputEvent::PointerMotion { .. } | InputEvent::PointerMotionAbsolute { .. } => "pointer motion",
            InputEvent::PointerButton { .. } => "pointer button",
            InputEvent::PointerAxis { .. } => "scroll",
            InputEvent::DeviceAdded { .. } | InputEvent::DeviceRemoved { .. } => "device",
            _ => "gesture or touch",
        };
        self.handle_input_event(dh, event);
        if pointing {
            self.wake_under_pointer(before);
        }
        self.timings.handled(|| format!("input {kind}"), start);
        self.timings.phase("input", start);
    }

    fn handle_input_event<B: InputBackend>(&mut self, dh: &DisplayHandle, event: InputEvent<B>) {
        if !matches!(event, InputEvent::DeviceAdded { .. } | InputEvent::DeviceRemoved { .. }) {
            self.power_activity();
        }
        match event {
            InputEvent::Keyboard { event, .. } => match self.keyboard_key_to_action::<B>(event) {
                #[cfg(feature = "udev")]
                KeyAction::VtSwitch(vt) => {
                    info!(to = vt, "Trying to switch vt");
                    if let Err(err) = self.backend_data.session.change_vt(vt) {
                        error!(vt, "Error switching vt: {}", err);
                    }
                }
                KeyAction::Screen(num) => {
                    let geometry = self.space.outputs().nth(num).map(|o| self.space.output_geometry(o).unwrap());

                    if let Some(geometry) = geometry {
                        let x = geometry.loc.x as f64 + geometry.size.w as f64 / 2.0;
                        let y = geometry.size.h as f64 / 2.0;
                        let location = (x, y).into();
                        let pointer = self.pointer.clone();
                        let under = self.surface_under(location);
                        pointer.motion(self, under, &MotionEvent { location, serial: SCOUNTER.next_serial(), time: InputTime::now() });
                        pointer.frame(self);
                    }
                }
                KeyAction::ScaleUp => {
                    let pos = self.pointer.current_location().to_i32_round();
                    let output = self.space.outputs().find(|o| self.space.output_geometry(o).unwrap().contains(pos)).cloned();

                    if let Some(output) = output {
                        let (output_location, scale) = (self.space.output_geometry(&output).unwrap().loc, output.current_scale().fractional_scale());
                        let new_scale = scale + 0.25;
                        output.change_current_state(None, None, Some(Scale::Fractional(new_scale)), None);

                        let rescale = scale / new_scale;
                        let output_location = output_location.to_f64();
                        let mut pointer_output_location = self.pointer.current_location() - output_location;
                        pointer_output_location.x *= rescale;
                        pointer_output_location.y *= rescale;
                        let pointer_location = output_location + pointer_output_location;

                        crate::shell::fixup_positions(&mut self.space, &self.settings.display, pointer_location);
                        let pointer = self.pointer.clone();
                        let under = self.surface_under(pointer_location);
                        pointer.motion(self, under, &MotionEvent { location: pointer_location, serial: SCOUNTER.next_serial(), time: InputTime::now() });
                        pointer.frame(self);
                        self.backend_data.reset_buffers(&output);
                    }
                }
                KeyAction::ScaleDown => {
                    let pos = self.pointer.current_location().to_i32_round();
                    let output = self.space.outputs().find(|o| self.space.output_geometry(o).unwrap().contains(pos)).cloned();

                    if let Some(output) = output {
                        let (output_location, scale) = (self.space.output_geometry(&output).unwrap().loc, output.current_scale().fractional_scale());
                        let new_scale = f64::max(1.0, scale - 0.25);
                        output.change_current_state(None, None, Some(Scale::Fractional(new_scale)), None);

                        let rescale = scale / new_scale;
                        let output_location = output_location.to_f64();
                        let mut pointer_output_location = self.pointer.current_location() - output_location;
                        pointer_output_location.x *= rescale;
                        pointer_output_location.y *= rescale;
                        let pointer_location = output_location + pointer_output_location;

                        crate::shell::fixup_positions(&mut self.space, &self.settings.display, pointer_location);
                        let pointer = self.pointer.clone();
                        let under = self.surface_under(pointer_location);
                        pointer.motion(self, under, &MotionEvent { location: pointer_location, serial: SCOUNTER.next_serial(), time: InputTime::now() });
                        pointer.frame(self);
                        self.backend_data.reset_buffers(&output);
                    }
                }
                KeyAction::RotateOutput => {
                    let pos = self.pointer.current_location().to_i32_round();
                    let output = self.space.outputs().find(|o| self.space.output_geometry(o).unwrap().contains(pos)).cloned();

                    if let Some(output) = output {
                        let current_transform = output.current_transform();
                        let new_transform = match current_transform {
                            Transform::Normal => Transform::_90,
                            Transform::_90 => Transform::_180,
                            Transform::_180 => Transform::_270,
                            Transform::_270 => Transform::Flipped,
                            Transform::Flipped => Transform::Flipped90,
                            Transform::Flipped90 => Transform::Flipped180,
                            Transform::Flipped180 => Transform::Flipped270,
                            Transform::Flipped270 => Transform::Normal,
                        };
                        output.change_current_state(None, Some(new_transform), None, None);
                        crate::shell::fixup_positions(&mut self.space, &self.settings.display, self.pointer.current_location());
                        self.backend_data.reset_buffers(&output);
                    }
                }
                KeyAction::ToggleTint => {
                    let mut debug_flags = self.backend_data.debug_flags();
                    debug_flags.toggle(DebugFlags::TINT);
                    self.backend_data.set_debug_flags(debug_flags);
                }

                action => match action {
                    KeyAction::None | KeyAction::Quit | KeyAction::Run(_) | KeyAction::ToggleDecorations => self.process_common_key_action(action),

                    _ => unreachable!(),
                },
            },
            InputEvent::PointerMotion { event, .. } => self.on_pointer_move::<B>(dh, event),
            InputEvent::PointerMotionAbsolute { event, .. } => self.on_pointer_move_absolute::<B>(dh, event),
            InputEvent::PointerButton { event, .. } => self.on_pointer_button::<B>(event),
            InputEvent::PointerAxis { event, .. } => self.on_pointer_axis::<B>(event),
            InputEvent::TabletToolAxis { event, .. } => self.on_tablet_tool_axis::<B>(event),
            InputEvent::TabletToolProximity { event, .. } => self.on_tablet_tool_proximity::<B>(dh, event),
            InputEvent::TabletToolTip { event, .. } => self.on_tablet_tool_tip::<B>(event),
            InputEvent::TabletToolButton { event, .. } => self.on_tablet_button::<B>(event),
            InputEvent::GestureSwipeBegin { event, .. } => self.on_gesture_swipe_begin::<B>(event),
            InputEvent::GestureSwipeUpdate { event, .. } => self.on_gesture_swipe_update::<B>(event),
            InputEvent::GestureSwipeEnd { event, .. } => self.on_gesture_swipe_end::<B>(event),
            InputEvent::GesturePinchBegin { event, .. } => self.on_gesture_pinch_begin::<B>(event),
            InputEvent::GesturePinchUpdate { event, .. } => self.on_gesture_pinch_update::<B>(event),
            InputEvent::GesturePinchEnd { event, .. } => self.on_gesture_pinch_end::<B>(event),
            InputEvent::GestureHoldBegin { event, .. } => self.on_gesture_hold_begin::<B>(event),
            InputEvent::GestureHoldEnd { event, .. } => self.on_gesture_hold_end::<B>(event),

            InputEvent::TouchDown { event } => self.on_touch_down::<B>(event),
            InputEvent::TouchUp { event } => self.on_touch_up::<B>(event),
            InputEvent::TouchMotion { event } => self.on_touch_motion::<B>(event),
            InputEvent::TouchFrame { event } => self.on_touch_frame::<B>(event),
            InputEvent::TouchCancel { event } => self.on_touch_cancel::<B>(event),

            InputEvent::DeviceAdded { device } => self.on_device_added::<B>(device),
            InputEvent::DeviceRemoved { device } => self.on_device_removed::<B>(device),
            _ => {
                // other events are not handled in anvil (yet)
            }
        }
    }

    fn on_pointer_move<B: InputBackend>(&mut self, _dh: &DisplayHandle, evt: B::PointerMotionEvent) {
        self.stop_glide();
        // (A mouse or a touchpad: the pointer shows again.)
        self.touch_hides_pointer = false;
        let mut pointer_location = self.pointer.current_location();
        let serial = SCOUNTER.next_serial();

        let pointer = self.pointer.clone();
        let under = self.surface_under(pointer_location);

        let mut pointer_locked = false;
        let mut pointer_confined = false;
        let mut confine_region = None;
        if let Some((surface, surface_loc)) = under.as_ref().and_then(|(target, l)| Some((target.wl_surface()?, l))) {
            with_pointer_constraint(&surface, &pointer, |constraint| match constraint {
                Some(constraint) if constraint.is_active() => {
                    // Constraint does not apply if not within region
                    if !constraint.region().is_none_or(|x| x.contains((pointer_location - *surface_loc).to_i32_round())) {
                        return;
                    }
                    match &*constraint {
                        PointerConstraint::Locked(_locked) => {
                            pointer_locked = true;
                        }
                        PointerConstraint::Confined(confine) => {
                            pointer_confined = true;
                            confine_region = confine.region().cloned();
                        }
                    }
                }
                _ => {}
            });
        }

        pointer.relative_motion(self, under.clone(), &RelativeMotionEvent { delta: evt.delta(), delta_unaccel: evt.delta_unaccel(), time: evt.time() });

        // If pointer is locked, only emit relative motion
        if pointer_locked {
            pointer.frame(self);
            return;
        }

        // Clamp delta, such that the pointer never moves out of bounds
        let mut delta = self.clamp_coords(pointer_location + evt.delta()) - pointer_location;

        // Clamp the individual x and y components of delta to respect confinement regions
        if pointer_confined
            && let Some((_, surface_loc)) = &under
            && let Some(region) = &confine_region
        {
            // Clamp delta.x
            if !region.contains((pointer_location + Point::new(delta.x, 0f64) - *surface_loc).to_i32_round()) {
                delta.x = 0f64;
            }

            // Clamp delta.y
            if !region.contains((pointer_location + Point::new(0f64, delta.y) - *surface_loc).to_i32_round()) {
                delta.y = 0f64;
            }
        }

        pointer_location += delta;

        let new_under = self.surface_under(pointer_location);

        // If confined, don't move pointer if it would go outside surface
        if pointer_confined
            && let Some((surface, _)) = &under
            && new_under.as_ref().and_then(|(under, _)| under.wl_surface()) != surface.wl_surface()
        {
            pointer.frame(self);
            return;
        }

        // The surface where the pointer is now, not where it was before
        // this motion (a window just entered gets the pointer with this
        // motion, not the next).
        self.pointer_track.note(std::time::Duration::from_micros(evt.time().micros()), pointer_location);
        pointer.motion(self, new_under.clone(), &MotionEvent { location: pointer_location, serial, time: evt.time() });
        pointer.frame(self);
        self.overview_pointer();
        self.twitch_pointer();

        // If pointer is now in a constraint region, activate it
        // TODO Anywhere else pointer is moved needs to do this
        if let Some((under, surface_location)) = new_under.and_then(|(target, loc)| Some((target.wl_surface()?.into_owned(), loc))) {
            with_pointer_constraint(&under, &pointer, |constraint| match constraint {
                Some(constraint) if !constraint.is_active() => {
                    let point = (pointer_location - surface_location).to_i32_round();
                    if constraint.region().is_none_or(|region| region.contains(point)) {
                        constraint.activate();
                    }
                }
                _ => {}
            });
        }
    }

    fn on_pointer_move_absolute<B: InputBackend>(&mut self, _dh: &DisplayHandle, evt: B::PointerMotionAbsoluteEvent) {
        self.touch_hides_pointer = false;
        let serial = SCOUNTER.next_serial();

        // An absolute device (a tablet, a VM's pointer) spans every screen.
        let Some(all) = self.space.outputs().filter_map(|o| self.space.output_geometry(o)).reduce(|a, b| a.merge(b)) else {
            return;
        };
        let mut pointer_location: Point<f64, Logical> = (all.loc.x as f64 + evt.x_transformed(all.size.w), all.loc.y as f64 + evt.y_transformed(all.size.h)).into();

        // clamp to screen limits
        pointer_location = self.clamp_coords(pointer_location);

        let pointer = self.pointer.clone();
        let under = self.surface_under(pointer_location);

        pointer.motion(self, under, &MotionEvent { location: pointer_location, serial, time: evt.time() });
        pointer.frame(self);
        self.overview_pointer();
        self.twitch_pointer();
    }

    fn on_tablet_tool_axis<B: InputBackend>(&mut self, evt: B::TabletToolAxisEvent) {
        let tablet_seat = self.seat.tablet_seat();

        if let Some(pointer_location) = self.touch_location_transformed(&evt) {
            let pointer = self.pointer.clone();
            let under = self.surface_under(pointer_location);
            let tool = tablet_seat.get_tool(&evt.tool());
            let time = InputTime::now();

            pointer.motion(self, under.clone(), &MotionEvent { location: pointer_location, serial: SCOUNTER.next_serial(), time });

            if let Some(tool) = tool {
                let frame = tablet::tool::AxisFrame {
                    pressure: evt.pressure_has_changed().then(|| evt.pressure()),
                    distance: evt.distance_has_changed().then(|| evt.distance()),
                    tilt: evt.tilt_has_changed().then(|| evt.tilt()),
                    rotation: evt.rotation_has_changed().then(|| evt.rotation()),
                    slider: evt.slider_has_changed().then(|| evt.slider_position()),
                    wheel: evt.wheel_has_changed().then(|| (evt.wheel_delta(), evt.wheel_delta_discrete())),
                };

                tool.axis(self, frame);

                tool.motion(self, under, &tablet::tool::MotionEvent { location: pointer_location, serial: SCOUNTER.next_serial(), time });

                tool.frame(self, time);
            }

            pointer.frame(self);
        }
    }

    fn on_tablet_tool_proximity<B: InputBackend>(&mut self, dh: &DisplayHandle, evt: B::TabletToolProximityEvent) {
        let tablet_seat = self.seat.tablet_seat();

        if let Some(pointer_location) = self.touch_location_transformed(&evt) {
            let tool = evt.tool();

            let pointer = self.pointer.clone();
            let under = self.surface_under(pointer_location);
            let tablet = tablet_seat.get_tablet(&TabletDescriptor::from(&evt.device()));
            let tool = tablet_seat.get_tool(&tool).unwrap_or_else(|| tablet_seat.add_wp_tool(self, dh, &tool));

            pointer.motion(self, under.clone(), &MotionEvent { location: pointer_location, serial: SCOUNTER.next_serial(), time: evt.time() });
            pointer.frame(self);

            if let Some(tablet) = tablet {
                let frame = tablet::tool::AxisFrame {
                    pressure: evt.pressure_has_changed().then(|| evt.pressure()),
                    distance: evt.distance_has_changed().then(|| evt.distance()),
                    tilt: evt.tilt_has_changed().then(|| evt.tilt()),
                    rotation: evt.rotation_has_changed().then(|| evt.rotation()),
                    slider: evt.slider_has_changed().then(|| evt.slider_position()),
                    wheel: evt.wheel_has_changed().then(|| (evt.wheel_delta(), evt.wheel_delta_discrete())),
                };

                match evt.state() {
                    ProximityState::In => {
                        tool.proximity_in(self, under, tablet, &tablet::tool::ProximityInEvent { location: pointer_location, axis: Some(frame), serial: SCOUNTER.next_serial(), time: evt.time() });
                    }
                    ProximityState::Out => {
                        tool.proximity_out(self, &tablet::tool::ProximityOutEvent { serial: SCOUNTER.next_serial(), time: evt.time() });
                    }
                }

                // Doing this in an idle handler would allow other events (e.g. buttons) to be
                // sent as part of the same frame, which is closer to what the protocol
                // expect, and let well behaved clients accumulate events.
                tool.frame(self, evt.time());
            }
        }
    }

    fn on_tablet_tool_tip<B: InputBackend>(&mut self, evt: B::TabletToolTipEvent) {
        let tool = self.seat.tablet_seat().get_tool(&evt.tool());

        if let Some(tool) = tool {
            let serial = SCOUNTER.next_serial();

            match evt.tip_state() {
                TabletToolTipState::Down => {
                    tool.down(self, &tablet::tool::DownEvent { serial, time: evt.time() });

                    // change the keyboard focus
                    self.update_keyboard_focus(self.pointer.current_location(), serial);
                }
                TabletToolTipState::Up => {
                    tool.up(self, &tablet::tool::UpEvent { serial, time: evt.time() });
                }
            }

            tool.frame(self, evt.time());
        }
    }

    fn on_tablet_button<B: InputBackend>(&mut self, evt: B::TabletToolButtonEvent) {
        let tool = self.seat.tablet_seat().get_tool(&evt.tool());

        if let Some(tool) = tool {
            tool.button(self, &tablet::tool::ButtonEvent { serial: SCOUNTER.next_serial(), button: evt.button(), state: evt.button_state(), time: evt.time() });

            tool.frame(self, evt.time());
        }
    }

    /// Keep the pointer on a screen: a point off every screen goes to the
    /// nearest point of the nearest one, so it slides along edges and
    /// crosses wherever screens touch, however they are arranged.
    fn clamp_coords(&self, pos: Point<f64, Logical>) -> Point<f64, Logical> {
        let screens: Vec<smithay::utils::Rectangle<i32, Logical>> = self.space.outputs().filter_map(|o| self.space.output_geometry(o)).collect();
        clamp_to_screens(&screens, pos)
    }
}

/// `pos` if it is on one of `screens`, else the nearest point that is.
fn clamp_to_screens(screens: &[smithay::utils::Rectangle<i32, Logical>], pos: Point<f64, Logical>) -> Point<f64, Logical> {
    screens
        .iter()
        .map(|g| {
            // The last pixel's far edge is the next screen's.
            let x = pos.x.clamp(g.loc.x as f64, (g.loc.x + g.size.w) as f64 - 0.001);
            let y = pos.y.clamp(g.loc.y as f64, (g.loc.y + g.size.h) as f64 - 0.001);
            let d = (x - pos.x).powi(2) + (y - pos.y).powi(2);
            (d, Point::from((x, y)))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map_or(pos, |(_, p)| p)
}

#[cfg(test)]
mod clamp_tests {
    use super::*;
    use smithay::utils::Rectangle;

    #[test]
    fn pointer_stays_on_screens() {
        // Dell at (0, 180), built-in at (4480, 180), Studio at (1920, 0).
        let screens = [Rectangle::new((0, 180).into(), (1920, 1080).into()), Rectangle::new((1920, 0).into(), (2560, 1440).into()), Rectangle::new((4480, 180).into(), (1728, 1080).into())];
        let at = |x: f64, y: f64| clamp_to_screens(&screens, (x, y).into());
        // On a screen: unchanged.
        assert_eq!(at(5000.0, 500.0), (5000.0, 500.0).into());
        // Across the Dell's right edge onto the Studio (which once couldn't be reached).
        assert_eq!(at(1925.0, 100.0), (1925.0, 100.0).into());
        // Above the Dell (no screen there): onto its top edge, not lost.
        assert_eq!(at(100.0, 50.0), (100.0, 180.0).into());
        // Far right, below everything: the built-in's corner.
        let p = at(9000.0, 5000.0);
        assert!(p.x < 6208.0 && p.x > 6207.9 && p.y < 1260.0 && p.y > 1259.9);
        // No screens: as it is.
        assert_eq!(clamp_to_screens(&[], (5.0, 5.0).into()), (5.0, 5.0).into());
    }
}

/// Possible results of a keyboard action
#[allow(dead_code)] // some of these are only read if udev is enabled
#[derive(Debug)]
enum KeyAction {
    /// Quit the compositor
    Quit,
    /// Trigger a vt-switch
    VtSwitch(i32),
    /// run a command
    Run(String),
    /// Switch the current screen
    Screen(usize),
    ScaleUp,
    ScaleDown,
    RotateOutput,
    ToggleTint,
    ToggleDecorations,
    /// Do nothing more
    None,
}

fn process_keyboard_shortcut(modifiers: ModifiersState, keysym: Keysym) -> Option<KeyAction> {
    // The ⌘ chords live in `keymap`; Super never reaches here as a command
    // key, so these are only the emergency exits.
    if modifiers.ctrl && modifiers.alt && keysym == Keysym::BackSpace {
        Some(KeyAction::Quit)
    } else if (xkb::KEY_XF86Switch_VT_1..=xkb::KEY_XF86Switch_VT_12).contains(&keysym.raw()) {
        Some(KeyAction::VtSwitch((keysym.raw() - xkb::KEY_XF86Switch_VT_1 + 1) as i32))
    } else {
        None
    }
}

impl<BackendData: Backend> VirtualKeyboardHandler for AnvilState<BackendData> {
    fn process_virtual_keyboard_event(&mut self, event: InputEvent<VirtualKeyboardBackend>) {
        let keyboard = self.seat.get_keyboard().unwrap();

        match event {
            // The device's keycodes only mean anything under its own keymap, so
            // activate it before processing events. Anvil never switches back;
            // a real compositor should track which device the seat keymap came
            // from and restore its configured one when a physical key arrives.
            InputEvent::DeviceAdded { device } | InputEvent::Special(VirtualKeyboardSpecialEvent::KeymapChanged { device }) => {
                let Some(keymap) = device.keymap() else {
                    return;
                };
                if let Err(err) = keyboard.set_keymap_from_string(self, keymap.to_string()) {
                    warn!(?err, "Failed to apply virtual keyboard keymap");
                }
            }
            InputEvent::Keyboard { event } => {
                match self.keyboard_key_to_action::<VirtualKeyboardBackend>(event) {
                    action @ (KeyAction::None | KeyAction::Quit | KeyAction::Run(_) | KeyAction::ToggleDecorations) => self.process_common_key_action(action),
                    // VT switching and output changes are out-of-scope for virtual keyboards.
                    action => debug!(?action, "ignoring key action from a virtual keyboard"),
                }
            }
            InputEvent::Special(VirtualKeyboardSpecialEvent::Modifiers { mods_depressed, mods_latched, mods_locked, group, .. }) => {
                keyboard.with_xkb_state(self, |mut context| context.set_modifier_mask(mods_depressed, mods_latched, mods_locked, group));
            }
            _ => (),
        }
    }
}

/// How far (points) a finger being the pointer moves before it's a drag
/// (its button pressed where it came down), not a tap.
const TOUCH_DRAG: f64 = 6.0;

/// How far (points) two fingers move before what they do is decided.
const TOUCH_DECIDE: f64 = 8.0;

/// What two fingers on what takes no touch do.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TwoFingers {
    Undecided,
    Scroll,
    /// Their distance apart as it began.
    Pinch {
        apart: f64,
    },
}

/// The touch screen's fingers being the pointer (`AnvilState::touch_pointer`).
#[derive(Debug, Clone, Copy)]
pub struct TouchPointer {
    /// The finger that is the pointer.
    pub slot: i32,
    /// Where it came down, and where it is.
    pub down_at: Point<f64, Logical>,
    pub at: Point<f64, Logical>,
    /// Its button is pressed (it moved: a drag).
    pub pressed: bool,
    /// A second finger came: the two scroll, each one's slot and place.
    pub scroll: Option<[(i32, Point<f64, Logical>); 2]>,
    /// One of the two lifted: nothing until the other does.
    pub lifting: bool,
    /// Where the two were as the second came, and what they do.
    pub started: [Point<f64, Logical>; 2],
    pub two: TwoFingers,
}
