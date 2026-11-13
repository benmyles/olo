use std::cell::RefCell;

use smithay::{
    backend::input::InputTime,
    desktop::{PopupKeyboardGrab, PopupKind, PopupPointerGrab, PopupUngrabStrategy, Space, Window, WindowSurfaceType, find_popup_root_surface, get_popup_toplevel_coords, layer_map_for_output},
    input::{
        Seat,
        pointer::{Focus, MotionEvent},
    },
    output::Output,
    reexports::{
        wayland_protocols::xdg::{decoration as xdg_decoration, shell::server::xdg_toplevel},
        wayland_server::{
            Resource,
            protocol::{wl_output, wl_seat, wl_surface::WlSurface},
        },
    },
    utils::{IsAlive, Logical, Point, Serial},
    wayland::{
        compositor::{self, with_states},
        seat::WaylandFocus,
        shell::xdg::{Configure, PopupSurface, PositionerState, ToplevelCachedState, ToplevelSurface, XdgShellHandler, XdgShellState},
    },
};
use tracing::{trace, warn};

/// The app ids of clipboard tools' windows (`clipboard_helper`).
const CLIPBOARD_HELPERS: &[&str] = &["io.github.bugaevc.wl-clipboard"];

use crate::{
    focus::KeyboardFocusTarget,
    shell::{TouchMoveSurfaceGrab, TouchResizeSurfaceGrab},
    state::{AnvilState, Backend},
};

use super::{FullscreenSurface, PointerMoveSurfaceGrab, PointerResizeSurfaceGrab, ResizeData, ResizeEdge, ResizeState, SurfaceData, WindowElement, place_new_window};

impl<BackendData: Backend> XdgShellHandler for AnvilState<BackendData> {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        let t = std::time::Instant::now();
        // Do not send a configure here, the initial configure
        // of a xdg_surface has to be sent during the commit if
        // the surface is not already configured
        let window = WindowElement(Window::new_wayland_window(surface.clone()));
        let client = surface.wl_surface().client();
        let pid = client.as_ref().and_then(|c| crate::state::client_pid(c, &self.display_handle));
        if let (Some(pid), Some(c)) = (pid, &client) {
            *window.meta().owner.borrow_mut() = Some((pid, c.id()));
        }
        // New windows come up focused, as on BeOS (and active as they're
        // put in their place). One that has them only for having just
        // opened (not shown yet) passes on what it had.
        let before = match self.focused_window().filter(|w| !w.meta().placed.get()) {
            Some(new) => new.meta().focus_before.borrow().clone(),
            None => (self.seat.get_keyboard().and_then(|k| k.current_focus()), self.space.elements().find(|w| w.decoration().focused).cloned()),
        };
        let opener = before.1.clone();
        *window.meta().focus_before.borrow_mut() = before;
        let near = self.new_window_near(client.as_ref().map(|c| c.id()));
        place_new_window(&mut self.space, near, &window, true);
        // Opened from one of its app's floating windows (Preferences ▸
        // Sound's Try It, from the Sound panel), it floats too, over it:
        // zoomed by Focus or tiled, it would be under the window it was
        // opened from (Ben, 2026-11-06).
        if let (Some(opener), Some(client)) = (opener, &client)
            && opener.meta().owner.borrow().as_ref().is_some_and(|(_, c)| *c == client.id())
            && self.floats(&opener)
            && self.window_output(&window).is_some_and(|o| self.mode_now(&o) != olo_protocol::WindowingMode::Standard)
        {
            tracing::info!(window = window.id(), opener = opener.id(), "opened from its app's floating window: it floats too");
            Self::restore_float_override(&window, Some(true));
        }
        self.focus_window(&window);

        compositor::add_post_commit_hook(surface.wl_surface(), |state: &mut Self, _, surface| {
            if state.clipboard_helper(surface) {
                return;
            }
            let t = std::time::Instant::now();
            // (Where it goes once it has content: `place_on_content`.)
            // Picture in picture is placed by itself.
            state.pip_claim(surface);
            // A window coming back asks for its old size in its first
            // configure (sent just below).
            state.claim_placement(surface);
            // Auto-tiled: a new window opens in its tile; in Focus, zoomed.
            state.auto_claim(surface);
            state.focus_claim(surface);
            handle_toplevel_commit(&mut state.space, surface);
            state.timings.within(|| "a window's placing after a commit".into(), t);
        });
        self.timings.within(|| "a new window".into(), t);
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        // A clipboard tool's window, done: the keyboard back where it was.
        if let Some(i) = self.clipboard_helpers.iter().position(|(w, _)| w.0.toplevel().is_some_and(|t| t == &surface)) {
            let (_, before) = self.clipboard_helpers.remove(i);
            // (Once the helper's client is gone, if it's going: see below.)
            self.handle.insert_idle(move |state| match before.filter(|w| state.space.element_location(w).is_some()) {
                Some(w) => state.focus_window(&w),
                None => state.focus_topmost(),
            });
            return;
        }
        // When the focused window goes, the next one down takes focus (an
        // alert closing hands the keyboard back to its document).
        let Some(window) = self.all_windows().into_iter().find(|w| w.0.toplevel().is_some_and(|t| t == &surface)) else {
            return;
        };
        self.remember_placement(&window);
        if self.space.element_location(&window).is_none() {
            return; // parked: not on screen, nothing to refocus
        }
        let keyboard = self.seat.get_keyboard().unwrap();
        let had_focus = match keyboard.current_focus() {
            Some(KeyboardFocusTarget::Window(w)) => w == window.0,
            None => true,
            _ => false,
        };
        self.space.unmap_elem(&window);
        // The pointer, if it was on it, is on what was behind it.
        self.repoint_soon();
        if had_focus {
            if surface.wl_surface().client().is_some() {
                self.focus_topmost();
            } else {
                // Its client disconnected, its windows being destroyed one
                // by one: moving the focus now would send the keyboard's
                // leave to the client going away (wayland-backend aborts
                // for that, in a destructor). Once it's gone.
                self.handle.insert_idle(|state| state.focus_topmost());
            }
        }
    }

    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        // Do not send a configure here, the initial configure
        // of a xdg_surface has to be sent during the commit if
        // the surface is not already configured
        let t = std::time::Instant::now();

        self.unconstrain_popup(&surface);

        if let Err(err) = self.popups.track_popup(PopupKind::from(surface)) {
            warn!("Failed to track popup: {}", err);
        }
        self.timings.within(|| "a new popup".into(), t);
    }

    fn reposition_request(&mut self, surface: PopupSurface, positioner: PositionerState, token: u32) {
        surface.with_pending_state(|state| {
            let geometry = positioner.get_geometry();
            state.geometry = geometry;
            state.positioner = positioner;
        });
        self.unconstrain_popup(&surface);
        surface.send_repositioned(token);
    }

    fn move_request(&mut self, surface: ToplevelSurface, seat: wl_seat::WlSeat, serial: Serial) {
        let seat: Seat<AnvilState<BackendData>> = Seat::from_resource(&seat).unwrap();
        self.move_request_xdg(&surface, &seat, serial)
    }

    fn resize_request(&mut self, surface: ToplevelSurface, seat: wl_seat::WlSeat, serial: Serial, edges: xdg_toplevel::ResizeEdge) {
        let seat: Seat<AnvilState<BackendData>> = Seat::from_resource(&seat).unwrap();
        self.resize_request_xdg(&surface, &seat, serial, edges.into());
    }

    fn ack_configure(&mut self, surface: WlSurface, configure: Configure) {
        if let Configure::Toplevel(configure) = configure {
            if let Some(serial) = with_states(&surface, |states| {
                if let Some(data) = states.data_map.get::<RefCell<SurfaceData>>()
                    && let ResizeState::WaitingForFinalAck(_, serial) = data.borrow().resize_state
                {
                    return Some(serial);
                }

                None
            }) {
                // When the resize grab is released the surface
                // resize state will be set to WaitingForFinalAck
                // and the client will receive a configure request
                // without the resize state to inform the client
                // resizing has finished. Here we will wait for
                // the client to acknowledge the end of the
                // resizing. To check if the surface was resizing
                // before sending the configure we need to use
                // the current state as the received acknowledge
                // will no longer have the resize state set
                let is_resizing = with_states(&surface, |states| {
                    states.cached_state.get::<ToplevelCachedState>().current().last_acked.as_ref().is_some_and(|c| c.state.states.contains(xdg_toplevel::State::Resizing))
                });

                if configure.serial >= serial && is_resizing {
                    with_states(&surface, |states| {
                        let mut data = states.data_map.get::<RefCell<SurfaceData>>().unwrap().borrow_mut();
                        if let ResizeState::WaitingForFinalAck(resize_data, _) = data.resize_state {
                            data.resize_state = ResizeState::WaitingForCommit(resize_data);
                        } else {
                            unreachable!()
                        }
                    });
                }
            }

            let window = self.space.elements().find(|element| element.wl_surface().as_deref() == Some(&surface));
            if let Some(window) = window {
                use xdg_decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode;
                let is_ssd = configure.state.decoration_mode.map(|mode| mode == Mode::ServerSide).unwrap_or(false) || crate::state::kde_server_side(&surface);
                window.set_ssd(is_ssd);
            }
        }
    }

    fn fullscreen_request(&mut self, surface: ToplevelSurface, mut wl_output: Option<wl_output::WlOutput>) {
        // NOTE: This is only one part of the solution. We can set the
        // location and configure size here, but the surface should be rendered fullscreen
        // independently from its buffer size
        let wl_surface = surface.wl_surface();
        let window = self.space.elements().find(|window| window.wl_surface().map(|s| &*s == wl_surface).unwrap_or(false)).cloned();
        // The display asked for, else the one the window is on (where most
        // of it is: Quick Look on the display it opened on), else the one
        // the person is at. Its size and the display it fills are the same
        // one's.
        let output = wl_output
            .as_ref()
            .and_then(Output::from_resource)
            .or_else(|| window.as_ref().and_then(|w| self.display_of(w)))
            .or_else(|| self.space.output_under(self.pointer.current_location()).next().cloned())
            .or_else(|| self.space.outputs().next().cloned());
        let geometry = output.as_ref().and_then(|o| self.space.output_geometry(o));

        if let (Some(geometry), Some(output), Some(window)) = (geometry, output, window) {
            let client = match self.display_handle.get_client(wl_surface.id()) {
                Ok(client) => client,
                Err(_) => return,
            };
            for output in output.client_outputs(&client) {
                wl_output = Some(output);
            }
            // Full screen on one display only: off any other it filled.
            for o in self.space.outputs() {
                if let Some(f) = o.user_data().get::<FullscreenSurface>()
                    && f.get().is_some_and(|w| w == window)
                    && *o != output
                {
                    f.clear();
                }
            }
            // (Its place and size kept the first time, not again from full
            // screen.)
            if !window.meta().fullscreen.get()
                && let Some(at) = self.space.element_location(&window)
            {
                window.meta().before_fullscreen.set(Some(smithay::utils::Rectangle::new(at, window.0.geometry().size)));
            }
            window.meta().fullscreen.set(true);

            surface.with_pending_state(|state| {
                state.states.set(xdg_toplevel::State::Fullscreen);
                state.size = Some(geometry.size);
                state.fullscreen_output = wl_output;
            });
            output.user_data().insert_if_missing(FullscreenSurface::default);
            output.user_data().get::<FullscreenSurface>().unwrap().set(window.clone());
            trace!("Fullscreening: {:?}", window);
        }

        // The protocol demands us to always reply with a configure,
        // regardless of we fulfilled the request or not
        if surface.is_initial_configure_sent() {
            surface.send_configure();
        } else {
            // Will be sent during initial configure
        }
    }

    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        // Back where it was and as big, as it went full screen.
        let window = self.space.elements().find(|w| w.wl_surface().as_deref() == Some(surface.wl_surface())).cloned();
        let before = window.as_ref().and_then(|w| w.meta().before_fullscreen.take());
        let ret = surface.with_pending_state(|state| {
            state.states.unset(xdg_toplevel::State::Fullscreen);
            state.size = before.map(|r| r.size);
            state.fullscreen_output.take()
        });
        if let Some(w) = &window {
            w.meta().fullscreen.set(false);
            if let Some(r) = before {
                self.space.map_element(w.clone(), r.loc, false);
            }
        }
        if let Some(output) = ret {
            let output = Output::from_resource(&output).unwrap();
            if let Some(fullscreen) = output.user_data().get::<FullscreenSurface>() {
                trace!("Unfullscreening: {:?}", fullscreen.get());
                fullscreen.clear();
                self.backend_data.reset_buffers(&output);
            }
        }

        // The protocol demands us to always reply with a configure,
        // regardless of we fulfilled the request or not
        if surface.is_initial_configure_sent() {
            surface.send_configure();
        } else {
            // Will be sent during initial configure
        }
    }

    fn maximize_request(&mut self, surface: ToplevelSurface) {
        if let Some(window) = self.window_for_surface(surface.wl_surface()) {
            self.zoom_window(&window);
        }
        // The protocol demands us to always reply with a configure.
        if surface.is_initial_configure_sent() {
            surface.send_configure();
        }
    }

    fn unmaximize_request(&mut self, surface: ToplevelSurface) {
        if let Some(window) = self.window_for_surface(surface.wl_surface()) {
            self.unzoom_window(&window);
        } else {
            surface.with_pending_state(|state| {
                state.states.unset(xdg_toplevel::State::Maximized);
                state.size = None;
            });
        }
        if surface.is_initial_configure_sent() {
            surface.send_configure();
        }
    }

    fn popup_destroyed(&mut self, surface: PopupSurface) {
        // Smithay's popup keyboard grab only hands the keyboard back to the
        // window on the next key press, so a menu's window would show no
        // caret (and a text field no selection) until then. Give it back,
        // ending the grab, as soon as the last popup of the grab is gone
        // (once the popup manager has seen it go: before, the grab hands
        // the keyboard back but stays, and refuses the next menu of any
        // other program, as Deskbar's right after a window's tab menu
        // put away with Escape).
        let gone = surface.wl_surface().clone();
        let root = self.popup_root_focus(&PopupKind::Xdg(surface));
        self.handle.insert_idle(move |state| {
            state.popups.cleanup();
            // A parent menu still open keeps its grab (and the keyboard).
            let open = root.as_ref().and_then(|r| r.wl_surface()).is_some_and(|w| smithay::desktop::PopupManager::popups_for_surface(&w).any(|(p, _)| p.alive() && *p.wl_surface() != gone));
            if let Some(keyboard) = state.seat.get_keyboard()
                && keyboard.is_grabbed()
                && !open
            {
                // Back to what had the keyboard as the menu opened (the
                // window clicked for it; for Deskbar's, which never takes
                // the keyboard, the window typed into), else the menu's
                // window (a popup gone can't always say whose it was: then
                // the grab's parent); not if its window went with it (the
                // tab menu's layer: the keyboard stays where its choice put
                // it).
                let before = state.keyboard_before_popup.take();
                let start = keyboard.with_grab(|_, grab| grab.start_data().focus.clone()).flatten();
                let takes = |f: &KeyboardFocusTarget| f.alive() && !matches!(f, KeyboardFocusTarget::LayerSurface(l) if !l.can_receive_keyboard_focus());
                let back = match &root {
                    Some(r) if !r.alive() => None,
                    _ => before.filter(takes).or(root.clone().filter(takes)).or(start.filter(takes)),
                };
                // The grab first: it ignores focus changes until it thinks
                // itself over, which is only once the popup's surface has
                // gone too (a kit app keeps it a while), so the window
                // never got the keyboard back.
                if keyboard.with_grab(|_, grab| grab.is::<PopupKeyboardGrab<Self>>()).unwrap_or(false) {
                    keyboard.unset_grab(state);
                }
                if let Some(back) = back.filter(|r| r.alive()) {
                    keyboard.set_focus(state, Some(back), smithay::utils::SERIAL_COUNTER.next_serial());
                }
            }
            // Its window draws itself without the menu (a menu bar's title
            // let go; the window menu's clear layer closing) on its next
            // frame, which Olo's apps wait for: one now, not at whatever
            // next changes on screen, which may be a while (a click
            // meanwhile went to the window menu's layer, still there).
            if let Some(root) = root.as_ref().filter(|r| r.alive())
                && let Some(surface) = root.wl_surface()
                && let Some(output) = state.space.outputs().next().cloned()
            {
                let time: std::time::Duration = state.clock.now().into();
                smithay::desktop::utils::send_frames_surface_tree(&surface, &output, time, None, |_, _| Some(output.clone()));
            }
            // The pointer's popup grab likewise ends only on the next motion,
            // and then enters the window where the pointer was before that
            // motion (on the chosen item), dropping the motion itself: a
            // click on a menu title right after choosing an item would land
            // on the text under the item instead. Move the pointer where it
            // is now, which ends the grab and enters whatever is under it.
            let pointer = state.pointer.clone();
            let location = pointer.current_location();
            let under = state.surface_under(location);
            pointer.motion(state, under, &MotionEvent { location, serial: smithay::utils::SERIAL_COUNTER.next_serial(), time: InputTime::now() });
            pointer.frame(state);
        });
    }

    fn grab(&mut self, surface: PopupSurface, seat: wl_seat::WlSeat, serial: Serial) {
        let t = std::time::Instant::now();
        self.grab_inner(surface, seat, serial);
        self.timings.within(|| "a popup's grab".into(), t);
    }
}

impl<BackendData: Backend + 'static> AnvilState<BackendData> {
    fn grab_inner(&mut self, surface: PopupSurface, seat: wl_seat::WlSeat, serial: Serial) {
        let seat: Seat<AnvilState<BackendData>> = Seat::from_resource(&seat).unwrap();
        let kind = PopupKind::Xdg(surface);
        if let Some(root) = self.popup_root_focus(&kind) {
            // A window's menu (its tab right-clicked: `window_menu`) asks
            // as the button may still be down, with no press of its own to
            // name (the press was app_server's, on the tab); refused, it
            // showed with no grab, and its clear layer outlived it when put
            // away with Escape, taking the next click (Deskbar's). It takes
            // the pointer from that press instead.
            let window_menu = matches!(&root, crate::focus::KeyboardFocusTarget::LayerSurface(l) if l.namespace() == "window-menu");
            let Ok(mut grab) = self.popups.grab_popup(root, kind, &seat, serial) else { return };
            let (keyboard, pointer) = (seat.get_keyboard(), seat.get_pointer());
            // Asked with a press the seat's grabs don't know: refused, both
            // (before either is taken).
            let previous = grab.previous_serial();
            let refused = keyboard.as_ref().is_some_and(|k| k.is_grabbed() && !(k.has_grab(serial) || k.has_grab(previous.unwrap_or(serial))))
                || pointer.as_ref().is_some_and(|p| p.is_grabbed() && !(p.has_grab(serial) || p.has_grab(previous.unwrap_or_else(|| grab.serial()))));
            if refused && !window_menu {
                grab.ungrab(PopupUngrabStrategy::All);
                return;
            }
            if let Some(keyboard) = &keyboard {
                let menu_open = keyboard.with_grab(|_, g| g.is::<PopupKeyboardGrab<Self>>()).unwrap_or(false);
                // A menu's first popup (not a submenu): what has the
                // keyboard now gets it back when the menu goes.
                if !menu_open {
                    self.keyboard_before_popup = keyboard.current_focus();
                }
                // The menu open (the one this replaces, or opens from)
                // has the keyboard by a grab that its pointer grab ends
                // as it's replaced below, and with it any keyboard grab of
                // the same press: this one's, when it was set first. A
                // menu opened from the keyboard (← → along a menu bar, a
                // submenu by →) asks with the press that opened the first,
                // so it showed and took no keys. Its grab ends here, and
                // this one's is set after the pointer's.
                if menu_open {
                    keyboard.unset_grab(self);
                }
            }
            if let Some(pointer) = &pointer {
                pointer.set_grab(self, PopupPointerGrab::new(&grab), serial, Focus::Keep);
            }
            if let Some(keyboard) = &keyboard {
                keyboard.set_focus(self, grab.current_grab(), serial);
                keyboard.set_grab(self, PopupKeyboardGrab::new(&grab), serial);
            }
        }
    }
}

impl<BackendData: Backend> AnvilState<BackendData> {
    /// A window clipboard tools make only to set the clipboard (wl-copy,
    /// wl-paste: a 1x1 surface to get the keyboard with, as Wayland
    /// asks, when a program can't use data-control): never shown, tiled,
    /// decorated or listed; it has the keyboard for its moment, and gives
    /// it back when it goes (`toplevel_destroyed`). True for one (on any
    /// commit), so it is left out of the rest.
    fn clipboard_helper(&mut self, surface: &WlSurface) -> bool {
        if self.clipboard_helpers.iter().any(|(w, _)| w.wl_surface().as_deref() == Some(surface)) {
            return true;
        }
        let Some(window) = self.space.elements().find(|w| w.wl_surface().as_deref() == Some(surface)).cloned() else { return false };
        if !CLIPBOARD_HELPERS.contains(&window.app_id().as_str()) {
            return false;
        }
        let before = self.focused_window().filter(|w| *w != window);
        self.space.unmap_elem(&window);
        if let Some(t) = window.0.toplevel()
            && !t.is_initial_configure_sent()
        {
            t.send_configure();
        }
        let serial = smithay::utils::SERIAL_COUNTER.next_serial();
        if let Some(keyboard) = self.seat.get_keyboard() {
            keyboard.set_focus(self, Some(KeyboardFocusTarget::from(window.clone())), serial);
        }
        tracing::info!(app_id = window.app_id(), "a clipboard tool's window, kept out of sight");
        self.clipboard_helpers.push((window, before));
        true
    }

    /// The window (or layer surface) a popup belongs to, as a keyboard focus.
    fn popup_root_focus(&self, popup: &PopupKind) -> Option<KeyboardFocusTarget> {
        let root = find_popup_root_surface(popup).ok()?;
        self.space.elements().find(|w| w.wl_surface().map(|s| *s == root).unwrap_or(false)).cloned().map(KeyboardFocusTarget::from).or_else(|| {
            self.space
                .outputs()
                .find_map(|o| {
                    let map = layer_map_for_output(o);
                    map.layer_for_surface(&root, WindowSurfaceType::TOPLEVEL).cloned()
                })
                .map(KeyboardFocusTarget::LayerSurface)
        })
    }

    pub fn resize_request_xdg(&mut self, surface: &ToplevelSurface, seat: &Seat<Self>, serial: Serial, edges: ResizeEdge) {
        if let Some(touch) = seat.get_touch()
            && touch.has_grab(serial)
        {
            let start_data = touch.grab_start_data().unwrap();
            tracing::info!(?start_data);

            // If the client disconnects after requesting a move
            // we can just ignore the request
            let Some(window) = self.window_for_surface(surface.wl_surface()) else {
                tracing::info!("no window");
                return;
            };

            // If the focus was for a different surface, ignore the request.
            if start_data.focus.is_none() || !start_data.focus.as_ref().unwrap().0.same_client_as(&surface.wl_surface().id()) {
                tracing::info!("different surface");
                return;
            }
            let geometry = window.0.geometry();
            let loc = self.space.element_location(&window).unwrap();
            let (initial_window_location, initial_window_size) = (loc, geometry.size);

            with_states(surface.wl_surface(), move |states| {
                states.data_map.get::<RefCell<SurfaceData>>().unwrap().borrow_mut().resize_state = ResizeState::Resizing(ResizeData { edges, initial_window_location, initial_window_size });
            });

            surface.with_pending_state(|state| {
                state.states.set(xdg_toplevel::State::Resizing);
            });
            surface.send_pending_configure();

            // Taken in hand: no longer on its way to a tile it was asked into.
            self.unfollow(&window);
            window.meta().moved_on_purpose.set(true);
            let grab = TouchResizeSurfaceGrab { start_data, window, edges, initial_window_location, initial_window_size, last_window_size: initial_window_size };

            touch.set_grab(self, grab, serial);
            return;
        }

        let pointer = seat.get_pointer().unwrap();

        // Check that this surface has a click grab.
        if !pointer.has_grab(serial) {
            return;
        }

        let start_data = pointer.grab_start_data().unwrap();

        let window = self.window_for_surface(surface.wl_surface()).unwrap();

        // If the focus was for a different surface, ignore the request.
        if start_data.focus.is_none() || !start_data.focus.as_ref().unwrap().0.same_client_as(&surface.wl_surface().id()) {
            return;
        }

        let geometry = window.0.geometry();
        let loc = self.space.element_location(&window).unwrap();
        let (initial_window_location, initial_window_size) = (loc, geometry.size);

        with_states(surface.wl_surface(), move |states| {
            states.data_map.get::<RefCell<SurfaceData>>().unwrap().borrow_mut().resize_state = ResizeState::Resizing(ResizeData { edges, initial_window_location, initial_window_size });
        });

        // Resizing from the press, before it moves: a client can get
        // ready (the kit makes room in its swapchain, which costs a frame,
        // while nothing moves yet).
        surface.with_pending_state(|state| {
            state.states.set(xdg_toplevel::State::Resizing);
        });
        surface.send_pending_configure();

        // Taken in hand: no longer on its way to a tile it was asked into.
        self.unfollow(&window);
        window.meta().moved_on_purpose.set(true);
        let grab = PointerResizeSurfaceGrab { start_data, window, edges, initial_window_location, initial_window_size, last_window_size: initial_window_size };

        pointer.set_grab(self, grab, serial, Focus::Clear);
    }

    /// `window` unzoomed as it's taken to be moved (once a press on its
    /// tab has moved past the drag distance): true if it was zoomed, and
    /// is now under the pointer at its own size.
    pub fn unzoom_to_move(&mut self, window: &WindowElement) -> bool {
        let Some(surface) = window.0.toplevel() else { return false };
        let changed = surface.with_pending_state(|state| {
            if state.states.unset(xdg_toplevel::State::Maximized) {
                state.size = None;
                true
            } else {
                false
            }
        });
        if changed {
            surface.send_configure();
        }
        changed
    }

    pub fn move_request_xdg(&mut self, surface: &ToplevelSurface, seat: &Seat<Self>, serial: Serial) {
        if let Some(touch) = seat.get_touch()
            && touch.has_grab(serial)
        {
            let start_data = touch.grab_start_data().unwrap();

            // If the client disconnects after requesting a move
            // we can just ignore the request
            let Some(window) = self.window_for_surface(surface.wl_surface()) else {
                return;
            };

            // If the focus was for a different surface, ignore the request.
            if start_data.focus.is_none() || !start_data.focus.as_ref().unwrap().0.same_client_as(&surface.wl_surface().id()) {
                return;
            }

            // (Zoomed, it unzooms once the finger has moved it: a tap
            // leaves it as it is, `TouchMoveSurfaceGrab::started`.)
            let initial_window_location = self.space.element_location(&window).unwrap();

            // Taken in hand: no longer on its way to a tile it was asked into.
            self.unfollow(&window);
            window.meta().moved_on_purpose.set(true);
            let grab = TouchMoveSurfaceGrab { start_data, window, initial_window_location, started: false };

            touch.set_grab(self, grab, serial);
            return;
        }

        let pointer = seat.get_pointer().unwrap();

        // Check that this surface has a click grab.
        if !pointer.has_grab(serial) {
            return;
        }

        let start_data = pointer.grab_start_data().unwrap();

        // If the client disconnects after requesting a move
        // we can just ignore the request
        let Some(window) = self.window_for_surface(surface.wl_surface()) else {
            return;
        };
        // Not shown yet (its first frame's buffer may come after this, as
        // Vulkan's present commits it on its own): once it is in its place.
        if !window.meta().placed.get() {
            window.meta().pending_move.set(Some(serial));
            return;
        }

        // If the focus was for a different surface, ignore the request.
        if start_data.focus.is_none() || !start_data.focus.as_ref().unwrap().0.same_client_as(&surface.wl_surface().id()) {
            return;
        }

        // (Zoomed, it unzooms once the pointer has moved it: a click
        // leaves it as it is, `PointerMoveSurfaceGrab::started`.)
        let mut initial_window_location = self.space.element_location(&window).unwrap();

        // Moved with a press on another of its app's windows (a tab torn
        // off its strip, now a window under the pointer): from here, not
        // from where that press was.
        let mut start_data = start_data;
        let pressed_on_it = match start_data.focus.as_ref().map(|(f, _)| f) {
            Some(crate::focus::PointerFocusTarget::SSD(ssd)) => ssd.0 == window,
            Some(f) => f.wl_surface().is_some_and(|s| *s == *surface.wl_surface()),
            None => true,
        };
        if !pressed_on_it {
            start_data.location = pointer.current_location();
            initial_window_location = self.space.element_location(&window).unwrap_or(initial_window_location);
        }

        // Taken in hand: no longer on its way to a tile it was asked into.
        self.unfollow(&window);
        window.meta().moved_on_purpose.set(true);
        let grab = PointerMoveSurfaceGrab { start_data, window, initial_window_location, over: None, started: false, unzoomed: false, back_to: None };

        pointer.set_grab(self, grab, serial, Focus::Clear);
    }

    pub(crate) fn unconstrain_popup(&self, popup: &PopupSurface) {
        let Ok(root) = find_popup_root_surface(&PopupKind::Xdg(popup.clone())) else {
            return;
        };
        let popup_offset = get_popup_toplevel_coords(&PopupKind::Xdg(popup.clone()));

        // Popups of layer surfaces (Deskbar's menus): keep them on the
        // layer's output.
        for output in self.space.outputs() {
            let map = layer_map_for_output(output);
            let Some(layer) = map.layer_for_surface(&root, WindowSurfaceType::TOPLEVEL) else {
                continue;
            };
            let Some(layer_geo) = map.layer_geometry(layer) else {
                return;
            };
            let Some(out_geo) = self.space.output_geometry(output) else {
                return;
            };
            let mut target = smithay::utils::Rectangle::from_size(out_geo.size);
            target.loc -= layer_geo.loc;
            target.loc -= popup_offset;
            popup.with_pending_state(|state| {
                state.geometry = state.positioner.get_unconstrained_geometry(target);
            });
            return;
        }

        let Some(window) = self.window_for_surface(&root) else {
            return;
        };

        let mut outputs_for_window = self.space.outputs_for_element(&window);
        if outputs_for_window.is_empty() {
            return;
        }

        // Get a union of all outputs' geometries.
        let mut outputs_geo = self.space.output_geometry(&outputs_for_window.pop().unwrap()).unwrap();
        for output in outputs_for_window {
            outputs_geo = outputs_geo.merge(self.space.output_geometry(&output).unwrap());
        }

        let window_geo = self.space.element_geometry(&window).unwrap();

        // The target geometry for the positioner should be relative to its
        // parent's geometry: the client's window geometry, which sits
        // inside our decoration frame.
        let mut target = outputs_geo;
        target.loc -= popup_offset;
        target.loc -= window_geo.loc;
        target.loc -= window.content_offset();

        popup.with_pending_state(|state| {
            state.geometry = state.positioner.get_unconstrained_geometry(target);
        });
    }
}

/// Should be called on `WlSurface::commit` of xdg toplevel
fn handle_toplevel_commit(space: &mut Space<WindowElement>, surface: &WlSurface) -> Option<()> {
    let window = space.elements().find(|w| w.wl_surface().as_deref() == Some(surface)).cloned()?;

    let mut window_loc = space.element_location(&window)?;
    let geometry = window.0.geometry();

    let new_loc: Point<Option<i32>, Logical> = with_states(window.wl_surface().as_deref()?, |states| {
        let data = states.data_map.get::<RefCell<SurfaceData>>()?.borrow_mut();

        if let ResizeState::Resizing(resize_data) = data.resize_state {
            let edges = resize_data.edges;
            let loc = resize_data.initial_window_location;
            let size = resize_data.initial_window_size;

            // If the window is being resized by top or left, its location must be adjusted
            // accordingly.
            edges.intersects(ResizeEdge::TOP_LEFT).then(|| {
                let new_x = edges.intersects(ResizeEdge::LEFT).then_some(loc.x + (size.w - geometry.size.w));

                let new_y = edges.intersects(ResizeEdge::TOP).then_some(loc.y + (size.h - geometry.size.h));

                (new_x, new_y).into()
            })
        } else {
            None
        }
    })?;

    if let Some(new_x) = new_loc.x {
        window_loc.x = new_x;
    }
    if let Some(new_y) = new_loc.y {
        window_loc.y = new_y;
    }

    if new_loc.x.is_some() || new_loc.y.is_some() {
        // If TOP or LEFT side of the window got resized, we have to move it
        space.map_element(window, window_loc, false);
    }

    Some(())
}

/// Whether the surface has a buffer (has been mapped with content), so its
/// app id and title have been set.
fn surface_initial_mapped(surface: &WlSurface) -> bool {
    smithay::backend::renderer::utils::with_renderer_surface_state(surface, |s| s.buffer().is_some()).unwrap_or(false)
}

impl<BackendData: Backend> AnvilState<BackendData> {
    /// A window's first commit with content: put it where it goes (back
    /// where it was, or over its parent) before it is drawn. From the
    /// compositor's commit handler, once it has taken the buffer in and the
    /// window knows its size: post-commit hooks run before that, and a
    /// window placed there was placed a commit late (a frame of it in its
    /// cascade slot first: Quick Look's flash) or, placed by a size not
    /// known yet, in the wrong place.
    pub(crate) fn place_on_content(&mut self, surface: &WlSurface) {
        if surface_initial_mapped(surface) && !self.place_pip(surface) && self.restore_placement(surface) {
            self.place_over_parent(surface);
            self.place_alert(surface);
            self.place_under_pointer(surface);
            // New (not one coming back as it was): it glows a moment, if asked.
            let a = &self.settings.appearance;
            if a.glow_new_windows
                && a.glow_seconds > 0
                && let Some(w) = self.space.elements().find(|w| w.wl_surface().as_deref() == Some(surface))
            {
                w.meta().glow.set(Some((std::time::Instant::now(), std::time::Duration::from_secs(a.glow_seconds.min(60) as u64))));
            }
        }
        // A move asked for before it showed: now, from where it is.
        let window = self.space.elements().find(|w| w.wl_surface().as_deref() == Some(surface)).cloned();
        if let Some(window) = window
            && window.meta().placed.get()
            && let Some(serial) = window.meta().pending_move.take()
            && let Some(toplevel) = window.0.toplevel().cloned()
        {
            let seat = self.seat.clone();
            self.move_request_xdg(&toplevel, &seat, serial);
        }
    }
}
