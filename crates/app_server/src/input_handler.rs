use std::{convert::TryInto, process::Command, sync::atomic::Ordering};

use crate::{AnvilState, focus::PointerFocusTarget, shell::FullscreenSurface};

#[cfg(feature = "udev")]
use crate::udev::UdevData;
#[cfg(feature = "udev")]
use smithay::{backend::renderer::DebugFlags, input::tablet, reexports::wayland_server::Resource};

use smithay::{
    backend::input::{
        self, Axis, AxisSource, Device, DeviceCapability, Event, GestureBeginEvent, GestureEndEvent,
        GesturePinchUpdateEvent as _, GestureSwipeUpdateEvent as _, InputBackend, InputEvent, InputTime,
        KeyState, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent, TouchEvent,
    },
    desktop::{WindowSurfaceType, layer_map_for_output, utils::under_from_surface_tree},
    input::{
        keyboard::{FilterResult, Keysym, ModifiersState, keysyms as xkb},
        pointer::{AxisFrame, ButtonEvent, MotionEvent},
        pointer::{
            GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent, GesturePinchEndEvent,
            GesturePinchUpdateEvent, GestureSwipeBeginEvent, GestureSwipeEndEvent, GestureSwipeUpdateEvent,
        },
        tablet::{TabletDescriptor, TabletSeatTrait},
        touch::{DownEvent, UpEvent},
    },
    output::Scale,
    reexports::{
        wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1,
        wayland_server::protocol::wl_pointer,
    },
    utils::{Logical, Point, SERIAL_COUNTER as SCOUNTER, Serial, Transform},
    wayland::{
        input_method::InputMethodSeat,
        keyboard_shortcuts_inhibit::KeyboardShortcutsInhibitorSeat,
        shell::wlr_layer::{KeyboardInteractivity, Layer as WlrLayer},
        virtual_keyboard::{VirtualKeyboardBackend, VirtualKeyboardHandler, VirtualKeyboardSpecialEvent},
    },
};

use smithay::backend::input::AbsolutePositionEvent;

#[cfg(feature = "winit")]
use smithay::output::Output;
use tracing::{debug, error, info, warn};

use crate::state::Backend;
#[cfg(feature = "udev")]
use smithay::{
    backend::{
        input::{
            PointerMotionEvent, ProximityState, TabletToolButtonEvent, TabletToolEvent,
            TabletToolProximityEvent, TabletToolTipEvent, TabletToolTipState,
        },
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

                if let Err(e) = Command::new(&cmd)
                    .envs(self.socket_name.clone().map(|v| ("WAYLAND_DISPLAY", v)))
                    .spawn()
                {
                    error!(cmd, err = %e, "Failed to start program");
                }
            }

            KeyAction::ToggleDecorations => {
                for element in self.space.elements() {
                    #[allow(irrefutable_let_patterns)]
                    if let Some(toplevel) = element.0.toplevel() {
                        let mode_changed = toplevel.with_pending_state(|state| {
                            if let Some(current_mode) = state.decoration_mode {
                                let new_mode =
                                    if current_mode == zxdg_toplevel_decoration_v1::Mode::ClientSide {
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

            _ => unreachable!(
                "Common key action handler encountered backend specific action {:?}",
                action
            ),
        }
    }

    fn keyboard_key_to_action<B: InputBackend>(&mut self, evt: B::KeyboardKeyEvent) -> KeyAction {
        let key = evt.key_code().raw().saturating_sub(8);
        let pressed = evt.state() == KeyState::Pressed;
        self.physical_key(key, pressed, Event::time(&evt))
    }

    /// A key from a device (or a test): map it (⌘, Caps Lock, …), act on
    /// app_server's own chords and send the rest to the focused client.
    fn physical_key(&mut self, key: u32, pressed: bool, time: InputTime) -> KeyAction {
        if self.media_key(key, pressed) {
            return KeyAction::None;
        }
        if self.hints_key(key, pressed) || self.overview_key(key, pressed) {
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
                crate::keymap::Out::Grabbed(chord, pressed) if !self.lock.locked() => self.key_grabbed(chord, pressed),
                crate::keymap::Out::Grabbed(..) => {}
            }
        }
        action
    }

    fn keymap_action(&mut self, action: crate::keymap::Action) {
        use crate::keymap::Action as A;
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
            S::Tile(_)
            | S::Zoom
            | S::ZoomVertical
            | S::ZoomHorizontal
            | S::Restore
            | S::RestoreOrHide
            | S::Center
            | S::Hide
            | S::NextDisplay
            | S::PreviousDisplay => {
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
            let exclusive = layer.with_cached_state(|data| {
                data.keyboard_interactivity == KeyboardInteractivity::Exclusive
                    && (data.layer == WlrLayer::Top || data.layer == WlrLayer::Overlay)
            });
            if exclusive {
                let surface = self.space.outputs().find_map(|o| {
                    let map = layer_map_for_output(o);
                    map.layers().find(|l| l.layer_surface() == &layer).cloned()
                });
                if let Some(surface) = surface {
                    keyboard.set_focus(self, Some(surface.into()), serial);
                    keyboard.input::<(), _>(self, keycode, state, serial, time, |_, _, _| {
                        FilterResult::Forward
                    });
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

                        action
                            .map(FilterResult::Intercept)
                            .unwrap_or(FilterResult::Forward)
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
        self.stop_glide();
        let serial = SCOUNTER.next_serial();
        let button = evt.button_code();

        let state = wl_pointer::ButtonState::from(evt.state());
        if self.overview_button(button, state == wl_pointer::ButtonState::Pressed) || self.twitch_button(button, state == wl_pointer::ButtonState::Pressed) {
            return;
        }

        if wl_pointer::ButtonState::Pressed == state {
            // Clicking puts keyboard hints away.
            if self.hints.active() {
                self.hints.close();
            }
            // A ⌘-click: letting go of ⌘ after isn't a tap of it.
            self.keymap.command_used();
            self.update_keyboard_focus(self.pointer.current_location(), serial);
        };
        let pointer = self.pointer.clone();
        pointer.button(
            self,
            &ButtonEvent {
                button,
                state: state.try_into().unwrap(),
                serial,
                time: evt.time(),
            },
        );
        pointer.frame(self);
    }

    fn on_gesture_swipe_begin<B: InputBackend>(&mut self, evt: B::GestureSwipeBeginEvent) {
        self.stop_glide();
        let serial = SCOUNTER.next_serial();
        let pointer = self.pointer.clone();
        pointer.gesture_swipe_begin(
            self,
            &GestureSwipeBeginEvent {
                serial,
                time: evt.time(),
                fingers: evt.fingers(),
            },
        );
    }

    fn on_gesture_swipe_update<B: InputBackend>(&mut self, evt: B::GestureSwipeUpdateEvent) {
        let pointer = self.pointer.clone();
        pointer.gesture_swipe_update(
            self,
            &GestureSwipeUpdateEvent {
                time: evt.time(),
                delta: evt.delta(),
            },
        );
    }

    fn on_gesture_swipe_end<B: InputBackend>(&mut self, evt: B::GestureSwipeEndEvent) {
        let serial = SCOUNTER.next_serial();
        let pointer = self.pointer.clone();
        pointer.gesture_swipe_end(
            self,
            &GestureSwipeEndEvent {
                serial,
                time: evt.time(),
                cancelled: evt.cancelled(),
            },
        );
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
        pointer.gesture_pinch_begin(
            self,
            &GesturePinchBeginEvent {
                serial,
                time: evt.time(),
                fingers: evt.fingers(),
            },
        );
    }

    fn on_gesture_pinch_update<B: InputBackend>(&mut self, evt: B::GesturePinchUpdateEvent) {
        if !self.pinching {
            return;
        }
        let pointer = self.pointer.clone();
        pointer.gesture_pinch_update(
            self,
            &GesturePinchUpdateEvent {
                time: evt.time(),
                delta: evt.delta(),
                scale: evt.scale(),
                rotation: evt.rotation(),
            },
        );
    }

    fn on_gesture_pinch_end<B: InputBackend>(&mut self, evt: B::GesturePinchEndEvent) {
        if !std::mem::take(&mut self.pinching) {
            return;
        }
        let serial = SCOUNTER.next_serial();
        let pointer = self.pointer.clone();
        pointer.gesture_pinch_end(
            self,
            &GesturePinchEndEvent {
                serial,
                time: evt.time(),
                cancelled: evt.cancelled(),
            },
        );
    }

    fn on_gesture_hold_begin<B: InputBackend>(&mut self, evt: B::GestureHoldBeginEvent) {
        // Fingers resting on the touchpad.
        self.stop_glide();
        let serial = SCOUNTER.next_serial();
        let pointer = self.pointer.clone();
        pointer.gesture_hold_begin(
            self,
            &GestureHoldBeginEvent {
                serial,
                time: evt.time(),
                fingers: evt.fingers(),
            },
        );
    }

    fn on_gesture_hold_end<B: InputBackend>(&mut self, evt: B::GestureHoldEndEvent) {
        let serial = SCOUNTER.next_serial();
        let pointer = self.pointer.clone();
        pointer.gesture_hold_end(
            self,
            &GestureHoldEndEvent {
                serial,
                time: evt.time(),
                cancelled: evt.cancelled(),
            },
        );
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
        if !self.pointer.is_grabbed()
            && (!keyboard.is_grabbed() || input_method.keyboard_grabbed())
            && !touch.map(|touch| touch.is_grabbed()).unwrap_or(false)
        {
            let output = self.space.output_under(location).next().cloned();
            if let Some(output) = output.as_ref() {
                let output_geo = self.space.output_geometry(output).unwrap();
                if let Some(window) = output
                    .user_data()
                    .get::<FullscreenSurface>()
                    .and_then(|f| f.get())
                    && let Some((_, _)) =
                        window.surface_under(location - output_geo.loc.to_f64(), WindowSurfaceType::ALL)
                    {
                        keyboard.set_focus(self, Some(window.into()), serial);
                        return;
                    }

                let layers = layer_map_for_output(output);
                if let Some(layer) = layers
                    .layer_under(WlrLayer::Overlay, location - output_geo.loc.to_f64())
                    .or_else(|| layers.layer_under(WlrLayer::Top, location - output_geo.loc.to_f64()))
                    && let Some((_, _)) = layer.surface_under(
                        location
                            - output_geo.loc.to_f64()
                            - layers.layer_geometry(layer).unwrap().loc.to_f64(),
                        WindowSurfaceType::ALL,
                    )
                {
                    // One that takes the keyboard gets it; one that never
                    // does (Deskbar) leaves it where it is: a click on
                    // Deskbar went through to the desktop under it, which
                    // took the keyboard from the window being typed into.
                    if layer.can_receive_keyboard_focus() {
                        keyboard.set_focus(self, Some(layer.clone().into()), serial);
                    }
                    return;
                }
                drop(layers);
                // A desktop menu, over the windows: not the window under it.
                if self.lower_layer_menu_under(output, location).is_some() {
                    return;
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
                if let Some(layer) = layers
                    .layer_under(WlrLayer::Bottom, location - output_geo.loc.to_f64())
                    .or_else(|| layers.layer_under(WlrLayer::Background, location - output_geo.loc.to_f64()))
                    && layer.can_receive_keyboard_focus()
                        && let Some((_, _)) = layer.surface_under(
                            location
                                - output_geo.loc.to_f64()
                                - layers.layer_geometry(layer).unwrap().loc.to_f64(),
                            WindowSurfaceType::ALL,
                        ) {
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

    pub fn surface_under(
        &self,
        pos: Point<f64, Logical>,
    ) -> Option<(PointerFocusTarget, Point<f64, Logical>)> {
        if self.lock.locked() {
            return self.lock_surface_under(pos).map(|(s, loc)| (PointerFocusTarget::from(s), loc));
        }
        let output = self.space.outputs().find(|o| {
            let geometry = self.space.output_geometry(o).unwrap();
            geometry.contains(pos.to_i32_round())
        })?;
        let output_geo = self.space.output_geometry(output).unwrap();
        // (Before the layer map is taken: it takes it too.)
        let menu = self.lower_layer_menu_under(output, pos);
        let layers = layer_map_for_output(output);
        let layer_under = |layer| {
            layers.layer_under(layer, pos - output_geo.loc.to_f64()).and_then(|layer| {
                let layer_loc = layers.layer_geometry(layer).unwrap().loc;
                layer
                    .surface_under(pos - output_geo.loc.to_f64() - layer_loc.to_f64(), WindowSurfaceType::ALL)
                    .map(|(surface, loc)| (PointerFocusTarget::from(surface), loc + layer_loc + output_geo.loc))
            })
        };

        // The overview out: only what is over it (Deskbar) takes the
        // pointer; windows and the desktop under it don't.
        if self.overview.as_ref().is_some_and(|o| !o.leaving()) {
            return layer_under(WlrLayer::Overlay).or_else(|| layer_under(WlrLayer::Top)).map(|(s, l)| (s, l.to_f64()));
        }
        let mut under = None;
        if let Some((surface, loc)) = output
            .user_data()
            .get::<FullscreenSurface>()
            .and_then(|f| f.get())
            .and_then(|w| w.surface_under(pos - output_geo.loc.to_f64(), WindowSurfaceType::ALL))
        {
            under = Some((surface, loc + output_geo.loc));
        } else if let Some(focus) = layer_under(WlrLayer::Overlay)
            .or_else(|| menu.map(|(surface, loc)| (PointerFocusTarget::from(surface), loc)))
            .or_else(|| layer_under(WlrLayer::Top))
        {
            under = Some(focus)
        } else if let Some(focus) = self.space.element_under(pos).and_then(|(window, loc)| {
            window
                .surface_under(pos - loc.to_f64(), WindowSurfaceType::ALL)
                .map(|(surface, surf_loc)| (surface, surf_loc + loc))
        }) {
            under = Some(focus);
        } else if let Some(focus) = layer_under(WlrLayer::Bottom).or_else(|| layer_under(WlrLayer::Background)) {
            under = Some(focus)
        };
        under.map(|(s, l)| (s, l.to_f64()))
    }

    /// The surface of a desktop menu at `pos` (global), and where it is
    /// (`shell::lower_layer_menus`). Takes `output`'s layer map.
    fn lower_layer_menu_under(
        &self,
        output: &smithay::output::Output,
        pos: Point<f64, Logical>,
    ) -> Option<(smithay::reexports::wayland_server::protocol::wl_surface::WlSurface, Point<i32, Logical>)> {
        let origin = self.space.output_geometry(output)?.loc;
        crate::shell::lower_layer_menus(output)
            .into_iter()
            .find_map(|(menu, at)| under_from_surface_tree(&menu, pos, origin + at, WindowSurfaceType::ALL))
    }

    fn on_pointer_axis<B: InputBackend>(&mut self, evt: B::PointerAxisEvent) {
        // Nothing under the overview scrolls.
        if self.overview.as_ref().is_some_and(|o| !o.leaving()) {
            return;
        }
        let horizontal_amount = evt
            .amount(input::Axis::Horizontal)
            .unwrap_or_else(|| evt.amount_v120(input::Axis::Horizontal).unwrap_or(0.0) * 15.0 / 120.);
        let vertical_amount = evt
            .amount(input::Axis::Vertical)
            .unwrap_or_else(|| evt.amount_v120(input::Axis::Vertical).unwrap_or(0.0) * 15.0 / 120.);
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
                // Sent on the frame clock (`pace.rs`), the stop too.
                let now = std::time::Instant::now();
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

    fn touch_location_transformed<B: InputBackend, E: AbsolutePositionEvent<B>>(
        &self,
        evt: &E,
    ) -> Option<Point<f64, Logical>> {
        let output = self
            .space
            .outputs()
            .find(|output| output.name().starts_with("eDP"))
            .or_else(|| self.space.outputs().next());

        let output = output?;
        let output_geometry = self.space.output_geometry(output)?;

        let transform = output.current_transform();
        let size = transform.invert().transform_size(output_geometry.size);
        Some(
            transform.transform_point_in(evt.position_transformed(size), &size.to_f64())
                + output_geometry.loc.to_f64(),
        )
    }

    fn on_touch_down<B: InputBackend>(&mut self, evt: B::TouchDownEvent) {
        let Some(handle) = self.seat.get_touch() else {
            return;
        };

        let Some(touch_location) = self.touch_location_transformed(&evt) else {
            return;
        };

        let serial = SCOUNTER.next_serial();
        self.update_keyboard_focus(touch_location, serial);

        let under = self.surface_under(touch_location);
        handle.down(
            self,
            under,
            &DownEvent {
                slot: evt.slot(),
                location: touch_location,
                serial,
                time: evt.time(),
            },
        );
    }

    fn on_touch_up<B: InputBackend>(&mut self, evt: B::TouchUpEvent) {
        let Some(handle) = self.seat.get_touch() else {
            return;
        };
        let serial = SCOUNTER.next_serial();
        handle.up(
            self,
            &UpEvent {
                slot: evt.slot(),
                serial,
                time: evt.time(),
            },
        )
    }

    fn on_touch_motion<B: InputBackend>(&mut self, evt: B::TouchMotionEvent) {
        let Some(handle) = self.seat.get_touch() else {
            return;
        };
        let Some(touch_location) = self.touch_location_transformed(&evt) else {
            return;
        };

        let under = self.surface_under(touch_location);
        handle.motion(
            self,
            under,
            &smithay::input::touch::MotionEvent {
                slot: evt.slot(),
                location: touch_location,
                time: evt.time(),
            },
        );
    }

    fn on_touch_frame<B: InputBackend>(&mut self, _evt: B::TouchFrameEvent) {
        let Some(handle) = self.seat.get_touch() else {
            return;
        };
        handle.frame(self);
    }

    fn on_touch_cancel<B: InputBackend>(&mut self, _evt: B::TouchCancelEvent) {
        let Some(handle) = self.seat.get_touch() else {
            return;
        };
        handle.cancel(self);
    }

    fn on_device_added<B: InputBackend>(&mut self, device: B::Device) {
        let dh = &self.display_handle;
        if device.has_capability(DeviceCapability::TabletTool) {
            self.seat
                .tablet_seat()
                .add_wp_tablet(dh, &TabletDescriptor::from(&device));
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
                    let output = self
                        .space
                        .outputs()
                        .find(|o| o.name() == output_name)
                        .unwrap()
                        .clone();

                    let current_scale = output.current_scale().fractional_scale();
                    let new_scale = current_scale + 0.25;
                    output.change_current_state(None, None, Some(Scale::Fractional(new_scale)), None);

                    crate::shell::fixup_positions(&mut self.space, &self.settings.display, self.pointer.current_location());
                    self.backend_data.reset_buffers(&output);
                }

                KeyAction::ScaleDown => {
                    let output = self
                        .space
                        .outputs()
                        .find(|o| o.name() == output_name)
                        .unwrap()
                        .clone();

                    let current_scale = output.current_scale().fractional_scale();
                    let new_scale = f64::max(1.0, current_scale - 0.25);
                    output.change_current_state(None, None, Some(Scale::Fractional(new_scale)), None);

                    crate::shell::fixup_positions(&mut self.space, &self.settings.display, self.pointer.current_location());
                    self.backend_data.reset_buffers(&output);
                }

                KeyAction::RotateOutput => {
                    let output = self
                        .space
                        .outputs()
                        .find(|o| o.name() == output_name)
                        .unwrap()
                        .clone();

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
                    KeyAction::None
                    | KeyAction::Quit
                    | KeyAction::Run(_)
                    | KeyAction::ToggleDecorations => self.process_common_key_action(action),

                    _ => tracing::warn!(
                        ?action,
                        output_name,
                        "Key action unsupported on on output backend.",
                    ),
                },
            },

            InputEvent::PointerMotionAbsolute { event } => {
                let output = self
                    .space
                    .outputs()
                    .find(|o| o.name() == output_name)
                    .unwrap()
                    .clone();
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

    fn on_pointer_move_absolute_windowed<B: InputBackend>(
        &mut self,
        evt: B::PointerMotionAbsoluteEvent,
        output: &Output,
    ) {
        let output_geo = self.space.output_geometry(output).unwrap();

        let pos = evt.position_transformed(output_geo.size) + output_geo.loc.to_f64();
        let serial = SCOUNTER.next_serial();

        let pointer = self.pointer.clone();
        let under = self.surface_under(pos);
        self.pointer_track.note(std::time::Duration::from_micros(evt.time().micros()), pos);
        pointer.motion(
            self,
            under,
            &MotionEvent {
                location: pos,
                serial,
                time: evt.time(),
            },
        );
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

    pub fn synthetic_input(&mut self, event: olo_protocol::SyntheticEvent) {
        use olo_protocol::SyntheticEvent as S;
        // As real input wakes the displays (`process_input_event`).
        let before = self.pointer.current_location();
        let pointing = matches!(event, S::Move { .. } | S::Scroll { .. });
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
            S::Scroll { dx, dy } => {
                // As from the touchpad: 0, 0 is the fingers lifting.
                let _ = (pointer, time);
                let now = std::time::Instant::now();
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
            keyboard.input(
                self,
                keycode,
                KeyState::Released,
                SCOUNTER.next_serial(),
                InputTime::now(),
                |_, _, _| FilterResult::Forward::<bool>,
            );
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
                    let geometry = self
                        .space
                        .outputs()
                        .nth(num)
                        .map(|o| self.space.output_geometry(o).unwrap());

                    if let Some(geometry) = geometry {
                        let x = geometry.loc.x as f64 + geometry.size.w as f64 / 2.0;
                        let y = geometry.size.h as f64 / 2.0;
                        let location = (x, y).into();
                        let pointer = self.pointer.clone();
                        let under = self.surface_under(location);
                        pointer.motion(
                            self,
                            under,
                            &MotionEvent {
                                location,
                                serial: SCOUNTER.next_serial(),
                                time: InputTime::now(),
                            },
                        );
                        pointer.frame(self);
                    }
                }
                KeyAction::ScaleUp => {
                    let pos = self.pointer.current_location().to_i32_round();
                    let output = self
                        .space
                        .outputs()
                        .find(|o| self.space.output_geometry(o).unwrap().contains(pos))
                        .cloned();

                    if let Some(output) = output {
                        let (output_location, scale) = (
                            self.space.output_geometry(&output).unwrap().loc,
                            output.current_scale().fractional_scale(),
                        );
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
                        pointer.motion(
                            self,
                            under,
                            &MotionEvent {
                                location: pointer_location,
                                serial: SCOUNTER.next_serial(),
                                time: InputTime::now(),
                            },
                        );
                        pointer.frame(self);
                        self.backend_data.reset_buffers(&output);
                    }
                }
                KeyAction::ScaleDown => {
                    let pos = self.pointer.current_location().to_i32_round();
                    let output = self
                        .space
                        .outputs()
                        .find(|o| self.space.output_geometry(o).unwrap().contains(pos))
                        .cloned();

                    if let Some(output) = output {
                        let (output_location, scale) = (
                            self.space.output_geometry(&output).unwrap().loc,
                            output.current_scale().fractional_scale(),
                        );
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
                        pointer.motion(
                            self,
                            under,
                            &MotionEvent {
                                location: pointer_location,
                                serial: SCOUNTER.next_serial(),
                                time: InputTime::now(),
                            },
                        );
                        pointer.frame(self);
                        self.backend_data.reset_buffers(&output);
                    }
                }
                KeyAction::RotateOutput => {
                    let pos = self.pointer.current_location().to_i32_round();
                    let output = self
                        .space
                        .outputs()
                        .find(|o| self.space.output_geometry(o).unwrap().contains(pos))
                        .cloned();

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
                    KeyAction::None
                    | KeyAction::Quit
                    | KeyAction::Run(_)
                    | KeyAction::ToggleDecorations => self.process_common_key_action(action),

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
        let mut pointer_location = self.pointer.current_location();
        let serial = SCOUNTER.next_serial();

        let pointer = self.pointer.clone();
        let under = self.surface_under(pointer_location);

        let mut pointer_locked = false;
        let mut pointer_confined = false;
        let mut confine_region = None;
        if let Some((surface, surface_loc)) = under
            .as_ref()
            .and_then(|(target, l)| Some((target.wl_surface()?, l)))
        {
            with_pointer_constraint(&surface, &pointer, |constraint| match constraint {
                Some(constraint) if constraint.is_active() => {
                    // Constraint does not apply if not within region
                    if !constraint
                        .region()
                        .is_none_or(|x| x.contains((pointer_location - *surface_loc).to_i32_round()))
                    {
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

        pointer.relative_motion(
            self,
            under.clone(),
            &RelativeMotionEvent {
                delta: evt.delta(),
                delta_unaccel: evt.delta_unaccel(),
                time: evt.time(),
            },
        );

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
                && let Some(region) = &confine_region {
                    // Clamp delta.x
                    if !region.contains(
                        (pointer_location + Point::new(delta.x, 0f64) - *surface_loc).to_i32_round(),
                    ) {
                        delta.x = 0f64;
                    }

                    // Clamp delta.y
                    if !region.contains(
                        (pointer_location + Point::new(0f64, delta.y) - *surface_loc).to_i32_round(),
                    ) {
                        delta.y = 0f64;
                    }
                }

        pointer_location += delta;

        let new_under = self.surface_under(pointer_location);

        // If confined, don't move pointer if it would go outside surface
        if pointer_confined
            && let Some((surface, _)) = &under
                && new_under.as_ref().and_then(|(under, _)| under.wl_surface()) != surface.wl_surface() {
                    pointer.frame(self);
                    return;
                }

        // The surface where the pointer is now, not where it was before
        // this motion (a window just entered gets the pointer with this
        // motion, not the next).
        self.pointer_track.note(std::time::Duration::from_micros(evt.time().micros()), pointer_location);
        pointer.motion(
            self,
            new_under.clone(),
            &MotionEvent {
                location: pointer_location,
                serial,
                time: evt.time(),
            },
        );
        pointer.frame(self);
        self.overview_pointer();
        self.twitch_pointer();

        // If pointer is now in a constraint region, activate it
        // TODO Anywhere else pointer is moved needs to do this
        if let Some((under, surface_location)) =
            new_under.and_then(|(target, loc)| Some((target.wl_surface()?.into_owned(), loc)))
        {
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

    fn on_pointer_move_absolute<B: InputBackend>(
        &mut self,
        _dh: &DisplayHandle,
        evt: B::PointerMotionAbsoluteEvent,
    ) {
        let serial = SCOUNTER.next_serial();

        // An absolute device (a tablet, a VM's pointer) spans every screen.
        let Some(all) = self.space.outputs().filter_map(|o| self.space.output_geometry(o)).reduce(|a, b| a.merge(b)) else {
            return;
        };
        let mut pointer_location: Point<f64, Logical> =
            (all.loc.x as f64 + evt.x_transformed(all.size.w), all.loc.y as f64 + evt.y_transformed(all.size.h)).into();

        // clamp to screen limits
        pointer_location = self.clamp_coords(pointer_location);

        let pointer = self.pointer.clone();
        let under = self.surface_under(pointer_location);

        pointer.motion(
            self,
            under,
            &MotionEvent {
                location: pointer_location,
                serial,
                time: evt.time(),
            },
        );
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

            pointer.motion(
                self,
                under.clone(),
                &MotionEvent {
                    location: pointer_location,
                    serial: SCOUNTER.next_serial(),
                    time,
                },
            );

            if let Some(tool) = tool {
                let frame = tablet::tool::AxisFrame {
                    pressure: evt.pressure_has_changed().then(|| evt.pressure()),
                    distance: evt.distance_has_changed().then(|| evt.distance()),
                    tilt: evt.tilt_has_changed().then(|| evt.tilt()),
                    rotation: evt.rotation_has_changed().then(|| evt.rotation()),
                    slider: evt.slider_has_changed().then(|| evt.slider_position()),
                    wheel: evt
                        .wheel_has_changed()
                        .then(|| (evt.wheel_delta(), evt.wheel_delta_discrete())),
                };

                tool.axis(self, frame);

                tool.motion(
                    self,
                    under,
                    &tablet::tool::MotionEvent {
                        location: pointer_location,
                        serial: SCOUNTER.next_serial(),
                        time,
                    },
                );

                tool.frame(self, time);
            }

            pointer.frame(self);
        }
    }

    fn on_tablet_tool_proximity<B: InputBackend>(
        &mut self,
        dh: &DisplayHandle,
        evt: B::TabletToolProximityEvent,
    ) {
        let tablet_seat = self.seat.tablet_seat();

        if let Some(pointer_location) = self.touch_location_transformed(&evt) {
            let tool = evt.tool();

            let pointer = self.pointer.clone();
            let under = self.surface_under(pointer_location);
            let tablet = tablet_seat.get_tablet(&TabletDescriptor::from(&evt.device()));
            let tool = tablet_seat
                .get_tool(&tool)
                .unwrap_or_else(|| tablet_seat.add_wp_tool(self, dh, &tool));

            pointer.motion(
                self,
                under.clone(),
                &MotionEvent {
                    location: pointer_location,
                    serial: SCOUNTER.next_serial(),
                    time: evt.time(),
                },
            );
            pointer.frame(self);

            if let Some(tablet) = tablet {
                let frame = tablet::tool::AxisFrame {
                    pressure: evt.pressure_has_changed().then(|| evt.pressure()),
                    distance: evt.distance_has_changed().then(|| evt.distance()),
                    tilt: evt.tilt_has_changed().then(|| evt.tilt()),
                    rotation: evt.rotation_has_changed().then(|| evt.rotation()),
                    slider: evt.slider_has_changed().then(|| evt.slider_position()),
                    wheel: evt
                        .wheel_has_changed()
                        .then(|| (evt.wheel_delta(), evt.wheel_delta_discrete())),
                };

                match evt.state() {
                    ProximityState::In => {
                        tool.proximity_in(
                            self,
                            under,
                            tablet,
                            &tablet::tool::ProximityInEvent {
                                location: pointer_location,
                                axis: Some(frame),
                                serial: SCOUNTER.next_serial(),
                                time: evt.time(),
                            },
                        );
                    }
                    ProximityState::Out => {
                        tool.proximity_out(
                            self,
                            &tablet::tool::ProximityOutEvent {
                                serial: SCOUNTER.next_serial(),
                                time: evt.time(),
                            },
                        );
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
                    tool.down(
                        self,
                        &tablet::tool::DownEvent {
                            serial,
                            time: evt.time(),
                        },
                    );

                    // change the keyboard focus
                    self.update_keyboard_focus(self.pointer.current_location(), serial);
                }
                TabletToolTipState::Up => {
                    tool.up(
                        self,
                        &tablet::tool::UpEvent {
                            serial,
                            time: evt.time(),
                        },
                    );
                }
            }

            tool.frame(self, evt.time());
        }
    }

    fn on_tablet_button<B: InputBackend>(&mut self, evt: B::TabletToolButtonEvent) {
        let tool = self.seat.tablet_seat().get_tool(&evt.tool());

        if let Some(tool) = tool {
            tool.button(
                self,
                &tablet::tool::ButtonEvent {
                    serial: SCOUNTER.next_serial(),
                    button: evt.button(),
                    state: evt.button_state(),
                    time: evt.time(),
                },
            );

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
            InputEvent::DeviceAdded { device }
            | InputEvent::Special(VirtualKeyboardSpecialEvent::KeymapChanged { device }) => {
                let Some(keymap) = device.keymap() else {
                    return;
                };
                if let Err(err) = keyboard.set_keymap_from_string(self, keymap.to_string()) {
                    warn!(?err, "Failed to apply virtual keyboard keymap");
                }
            }
            InputEvent::Keyboard { event } => {
                match self.keyboard_key_to_action::<VirtualKeyboardBackend>(event) {
                    action @ (KeyAction::None
                    | KeyAction::Quit
                    | KeyAction::Run(_)
                    | KeyAction::ToggleDecorations) => self.process_common_key_action(action),
                    // VT switching and output changes are out-of-scope for virtual keyboards.
                    action => debug!(?action, "ignoring key action from a virtual keyboard"),
                }
            }
            InputEvent::Special(VirtualKeyboardSpecialEvent::Modifiers {
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
                ..
            }) => {
                keyboard.with_xkb_state(self, |mut context| {
                    context.set_modifier_mask(mods_depressed, mods_latched, mods_locked, group)
                });
            }
            _ => (),
        }
    }
}
