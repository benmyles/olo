use crate::{
    focus::{KeyboardFocusTarget, PointerFocusTarget},
    shell::WindowElement,
};
use std::{
    collections::HashMap,
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

use tracing::{info, warn};

use smithay::{
    backend::{
        input::TabletToolDescriptor,
        renderer::element::{
            RenderElementStates, default_primary_scanout_output_compare, utils::select_dmabuf_feedback,
        },
    },
    delegate_dispatch2,
    desktop::{
        PopupKind, PopupManager, Space,
        space::SpaceElement,
        utils::{
            OutputPresentationFeedback, surface_presentation_feedback_flags_from_states,
            surface_primary_scanout_output, update_surface_primary_scanout_output,
            with_surfaces_surface_tree,
        },
    },
    input::{
        Seat, SeatHandler, SeatState,
        dnd::{DnDGrab, DndGrabHandler, DndTarget, GrabType, Source},
        keyboard::{Keysym, LedState, XkbConfig},
        pointer::{CursorImageStatus, Focus, PointerHandle},
        tablet::TabletSeatHandler,
    },
    output::Output,
    reexports::{
        calloop::{Interest, LoopHandle, Mode, PostAction, generic::Generic},
        wayland_protocols::xdg::decoration::{
            self as xdg_decoration, zv1::server::zxdg_toplevel_decoration_v1::Mode as DecorationMode,
        },
        wayland_server::{
            Client, Display, DisplayHandle, Resource,
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::wl_surface::WlSurface,
        },
    },
    utils::{Clock, Logical, Monotonic, Point, Rectangle, Serial, Time},
    wayland::{
        commit_timing::{CommitTimerBarrierStateUserData, CommitTimingManagerState},
        compositor::{CompositorClientState, CompositorHandler, CompositorState, get_parent, with_states},
        dmabuf::DmabufFeedback,
        fifo::{FifoBarrierCachedState, FifoManagerState},
        fixes::FixesState,
        fractional_scale::{FractionalScaleHandler, FractionalScaleManagerState, with_fractional_scale},
        image_capture_source::{
            ImageCaptureSource, ImageCaptureSourceHandler, ImageCaptureSourceState,
            OutputCaptureSourceHandler, OutputCaptureSourceState,
        },
        image_copy_capture::{
            BufferConstraints, Frame, ImageCopyCaptureHandler, ImageCopyCaptureState, Session, SessionRef,
        },
        input_method::{InputMethodHandler, InputMethodManagerState, PopupSurface},
        idle_inhibit::{IdleInhibitHandler, IdleInhibitManagerState},
        keyboard_shortcuts_inhibit::{
            KeyboardShortcutsInhibitHandler, KeyboardShortcutsInhibitState, KeyboardShortcutsInhibitor,
        },
        output::{OutputHandler, OutputManagerState},
        pointer_constraints::{
            ConstraintRemove, PointerConstraint, PointerConstraintsHandler, PointerConstraintsState,
            with_pointer_constraint,
        },
        pointer_gestures::PointerGesturesState,
        presentation::PresentationState,
        relative_pointer::RelativePointerManagerState,
        seat::WaylandFocus,
        security_context::{
            SecurityContext, SecurityContextHandler, SecurityContextListenerSource, SecurityContextState,
        },
        selection::{
            SelectionHandler,
            data_device::{DataDeviceHandler, DataDeviceState, WaylandDndGrabHandler, set_data_device_focus},
            primary_selection::{PrimarySelectionHandler, PrimarySelectionState, set_primary_focus},
            wlr_data_control::{DataControlHandler, DataControlState},
        },
        shell::{
            wlr_layer::WlrLayerShellState,
            xdg::{
                ToplevelSurface, XdgShellState,
                decoration::{XdgDecorationHandler, XdgDecorationState},
            },
        },
        shm::{ShmHandler, ShmState},
        single_pixel_buffer::SinglePixelBufferState,
        tablet_manager::TabletManagerState,
        text_input::{TextInputActivation, TextInputManagerState},
        viewporter::ViewporterState,
        virtual_keyboard::VirtualKeyboardManagerState,
        xdg_activation::{
            XdgActivationHandler, XdgActivationState, XdgActivationToken, XdgActivationTokenData,
        },
        xdg_foreign::{XdgForeignHandler, XdgForeignState},
    },
};


#[derive(Debug, Default)]
pub struct ClientState {
    pub compositor_state: CompositorClientState,
    pub security_context: Option<SecurityContext>,
    /// Set when the client goes away (the lock screen's, so a crash is noticed).
    pub gone: Option<Arc<std::sync::atomic::AtomicBool>>,
    /// The program an olo-wlproxy connection speaks for.
    pub pid: Option<i32>,
    /// Through olo-wlproxy, which keeps it through our restart.
    pub proxied: bool,
}

/// The process behind a client: the program itself, also when it comes
/// through olo-wlproxy (whose own pid the socket shows).
pub fn client_pid(client: &Client, dh: &DisplayHandle) -> Option<i32> {
    if let Some(pid) = client.get_data::<ClientState>().and_then(|s| s.pid) {
        return Some(pid);
    }
    client.get_credentials(dh).ok().map(|c| c.pid)
}
impl ClientData for ClientState {
    /// Notification that a client was initialized
    fn initialized(&self, _client_id: ClientId) {}
    /// Notification that a client is disconnected
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {
        if let Some(gone) = &self.gone {
            gone.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }
}

#[derive(Debug)]
pub struct AnvilState<BackendData: Backend + 'static> {
    pub backend_data: BackendData,
    /// The Num Lock setting last applied (None: not yet, at start).
    pub num_lock_applied: Option<bool>,
    pub socket_name: Option<String>,
    pub display_handle: DisplayHandle,
    pub running: Arc<AtomicBool>,
    pub handle: LoopHandle<'static, AnvilState<BackendData>>,

    // desktop
    pub space: Space<WindowElement>,
    pub popups: PopupManager,
    /// Touchpad scrolls gliding on after the fingers lift.
    pub momentum: crate::momentum::Momentum,

    // smithay state
    pub compositor_state: CompositorState,
    pub data_device_state: DataDeviceState,
    pub layer_shell_state: WlrLayerShellState,
    pub output_manager_state: OutputManagerState,
    pub primary_selection_state: PrimarySelectionState,
    pub data_control_state: DataControlState,
    pub seat_state: SeatState<AnvilState<BackendData>>,
    pub keyboard_shortcuts_inhibit_state: KeyboardShortcutsInhibitState,
    pub shm_state: ShmState,
    pub viewporter_state: ViewporterState,
    pub xdg_activation_state: XdgActivationState,
    pub xdg_decoration_state: XdgDecorationState,
    /// KDE's server-decoration protocol, which GTK 3 and 4 use (they have
    /// no xdg-decoration) to let us draw their tab and frame.
    pub kde_decoration_state: smithay::wayland::shell::kde::decoration::KdeDecorationState,
    pub xdg_shell_state: XdgShellState,
    pub presentation_state: PresentationState,
    pub fractional_scale_manager_state: FractionalScaleManagerState,
    pub xdg_foreign_state: XdgForeignState,
    pub single_pixel_buffer_state: SinglePixelBufferState,
    pub fifo_manager_state: FifoManagerState,
    pub commit_timing_manager_state: CommitTimingManagerState,
    pub image_capture_source_state: ImageCaptureSourceState,
    pub output_capture_source_state: OutputCaptureSourceState,
    pub image_copy_capture_state: ImageCopyCaptureState,

    pub dnd_icon: Option<DndIcon>,

    // input-related fields
    pub suppressed_keys: Vec<Keysym>,
    /// ⌘ and friends (see `keymap`).
    pub keymap: crate::keymap::Keymap,
    /// Idle, caffeinated mode, brightness (see `power`).
    pub power: crate::power::Power,
    pub saver: crate::saver::Saver,
    /// The lock screen (see `lock`).
    pub lock: crate::lock::Lock,
    /// Twitcher, while ⌘Tab / ⌘\` is held.
    pub twitch: Option<crate::desktop::Twitch>,
    /// A tiled window's edges being dragged (see `shell::tiling`).
    pub tile_drag: Option<crate::shell::tiling::TileDrag>,
    /// Tiled neighbours following a drag of their shared edge, until
    /// they've drawn the size it left them.
    pub tile_followers: Vec<crate::shell::tiling::Follower>,
    pub twitcher: crate::twitcher::Overlay,
    /// Keyboard hints (⌃⇧ tapped), and their overlay.
    pub hints: crate::hints::Hints,
    pub wallpaper: crate::wallpaper::Loader,
    pub cursor_status: CursorImageStatus,
    pub seat_name: String,
    pub seat: Seat<AnvilState<BackendData>>,
    pub clock: Clock<Monotonic>,
    pub pointer: PointerHandle<AnvilState<BackendData>>,
    pub cursor_position_hint: Option<(WlSurface, Point<f64, Logical>)>,


    #[cfg(feature = "debug")]
    pub renderdoc: Option<renderdoc::RenderDoc<renderdoc::V141>>,

    pub show_window_preview: bool,
    pub parked: crate::desktop::Parked,
    pub workspace: u32,
    pub settings: olo_protocol::Settings,
    pub ipc: crate::ipc::IpcState,
    pub last_window_infos: Vec<olo_protocol::WindowInfo>,
    /// Remembered window places (see `layout`).
    pub layout: Vec<crate::layout::Placement>,
    /// Night-light colour temperature in effect, if any.
    pub night_light: Option<u32>,
    /// The screens' gamma gains now (none until first set), and the fade
    /// toward new ones (see `night`).
    pub white_point: Option<[f64; 3]>,
    pub night_fade: Option<crate::night::Fade>,
    /// Screens that mirror another (out of the Space; see `desktop`).
    pub mirrors: Vec<Output>,
    /// The windows Show Desktop hid (shown again by its next press).
    pub desktop_shown: Vec<WindowElement>,
    /// How the windows were before the last arrangement (Tile All,
    /// Cascade, Fan Out, an app's Show All), for Undo Window Arrangement.
    pub arranged: Vec<crate::shell::tiling::Placed>,
    /// Window groups as of the last loop iteration (`shell::groups`).
    pub groups: crate::shell::groups::Groups,
    /// Counts shortcut recordings (a timer only ends its own).
    pub recording_shortcut: u64,
}

#[derive(Debug)]
pub struct DndIcon {
    pub surface: WlSurface,
    pub offset: Point<i32, Logical>,
}

impl<BackendData: Backend> DataDeviceHandler for AnvilState<BackendData> {
    fn data_device_state(&mut self) -> &mut DataDeviceState {
        &mut self.data_device_state
    }
}

impl<BackendData: Backend> WaylandDndGrabHandler for AnvilState<BackendData> {
    fn dnd_requested<S: Source>(
        &mut self,
        source: S,
        icon: Option<WlSurface>,
        seat: Seat<Self>,
        serial: Serial,
        type_: GrabType,
    ) {
        // The icon was committed (with its hotspot as a buffer offset)
        // before the drag began, while it wasn't known to be the icon:
        // take that offset now, or the pointer holds it by its corner.
        self.dnd_icon = icon.map(|surface| {
            let offset = smithay::wayland::compositor::with_states(&surface, |states| {
                states.cached_state.get::<smithay::wayland::compositor::SurfaceAttributes>().current().buffer_delta.take().unwrap_or_default()
            });
            DndIcon { surface, offset }
        });

        match type_ {
            GrabType::Pointer => {
                let pointer = seat.get_pointer().unwrap();
                let start_data = pointer.grab_start_data().unwrap();
                pointer.set_grab(
                    self,
                    DnDGrab::new_pointer(&self.display_handle, start_data, source, seat),
                    serial,
                    Focus::Keep,
                );
            }
            GrabType::Touch => {
                let touch = seat.get_touch().unwrap();
                let start_data = touch.grab_start_data().unwrap();
                touch.set_grab(
                    self,
                    DnDGrab::new_touch(&self.display_handle, start_data, source, seat),
                    serial,
                );
            }
        }
    }
}

impl<BackendData: Backend> DndGrabHandler for AnvilState<BackendData> {
    fn dropped(
        &mut self,
        _target: Option<DndTarget<'_, Self>>,
        _validated: bool,
        _seat: Seat<Self>,
        _location: Point<f64, Logical>,
    ) {
        self.dnd_icon = None;
    }
}

impl<BackendData: Backend> OutputHandler for AnvilState<BackendData> {}

impl<BackendData: Backend> SelectionHandler for AnvilState<BackendData> {
    type SelectionUserData = ();


}

impl<BackendData: Backend> PrimarySelectionHandler for AnvilState<BackendData> {
    fn primary_selection_state(&mut self) -> &mut PrimarySelectionState {
        &mut self.primary_selection_state
    }
}

impl<BackendData: Backend> DataControlHandler for AnvilState<BackendData> {
    fn data_control_state(&mut self) -> &mut DataControlState {
        &mut self.data_control_state
    }
}

impl<BackendData: Backend> ShmHandler for AnvilState<BackendData> {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

impl<BackendData: Backend> SeatHandler for AnvilState<BackendData> {
    type KeyboardFocus = KeyboardFocusTarget;
    type PointerFocus = PointerFocusTarget;
    type TouchFocus = PointerFocusTarget;

    fn seat_state(&mut self) -> &mut SeatState<AnvilState<BackendData>> {
        &mut self.seat_state
    }

    fn focus_changed(&mut self, seat: &Seat<Self>, target: Option<&KeyboardFocusTarget>) {
        let dh = &self.display_handle;

        let wl_surface = target.and_then(WaylandFocus::wl_surface);

        let focus = wl_surface.and_then(|s| dh.get_client(s.id()).ok());
        set_data_device_focus(dh, seat, focus.clone());
        set_primary_focus(dh, seat, focus);
    }
    fn cursor_image(&mut self, _seat: &Seat<Self>, image: CursorImageStatus) {
        self.cursor_status = image;
    }

    fn led_state_changed(&mut self, _seat: &Seat<Self>, led_state: LedState) {
        self.backend_data.update_led_state(led_state)
    }
}

impl<BackendData: Backend> TabletSeatHandler for AnvilState<BackendData> {
    type ToolFocus = PointerFocusTarget;

    fn tablet_tool_image(&mut self, _tool: &TabletToolDescriptor, image: CursorImageStatus) {
        // TODO: tablet tools should have their own cursors
        self.cursor_status = image;
    }
}

impl<BackendData: Backend> InputMethodHandler for AnvilState<BackendData> {
    fn new_popup(&mut self, surface: PopupSurface) {
        if let Err(err) = self.popups.track_popup(PopupKind::from(surface)) {
            warn!("Failed to track popup: {}", err);
        }
    }

    fn popup_repositioned(&mut self, _: PopupSurface) {}

    fn dismiss_popup(&mut self, surface: PopupSurface) {
        if let Some(parent) = surface.get_parent().map(|parent| parent.surface.clone()) {
            let _ = PopupManager::dismiss_popup(&parent, &PopupKind::from(surface));
        }
    }

    fn parent_geometry(&self, parent: &WlSurface) -> Rectangle<i32, smithay::utils::Logical> {
        self.space
            .elements()
            .find_map(|window| (window.wl_surface().as_deref() == Some(parent)).then(|| window.geometry()))
            .unwrap_or_default()
    }
}

impl<BackendData: Backend> IdleInhibitHandler for AnvilState<BackendData> {
    fn inhibit(&mut self, surface: WlSurface) {
        self.idle_inhibit(surface, true);
    }

    fn uninhibit(&mut self, surface: WlSurface) {
        self.idle_inhibit(surface, false);
    }
}

impl<BackendData: Backend> KeyboardShortcutsInhibitHandler for AnvilState<BackendData> {
    fn keyboard_shortcuts_inhibit_state(&mut self) -> &mut KeyboardShortcutsInhibitState {
        &mut self.keyboard_shortcuts_inhibit_state
    }

    fn new_inhibitor(&mut self, inhibitor: KeyboardShortcutsInhibitor) {
        // Just grant the wish for everyone
        inhibitor.activate();
    }
}

impl<BackendData: Backend> PointerConstraintsHandler for AnvilState<BackendData> {
    fn new_constraint(&mut self, surface: &WlSurface, pointer: &PointerHandle<Self>) {
        // XXX region
        let Some(current_focus) = pointer.current_focus() else {
            return;
        };
        if current_focus.wl_surface().as_deref() == Some(surface) {
            with_pointer_constraint(surface, pointer, |constraint| {
                constraint.unwrap().activate();
            });
        }
    }

    fn remove_constraint(
        &mut self,
        _surface: &WlSurface,
        pointer: &PointerHandle<Self>,
        constraint_remove: ConstraintRemove,
    ) {
        // Clear cursor_position_hint to prevent a oneshot PointerLocked constraint
        // from causing this function to be called again during PointerLeave and
        // unexpectedly changing the cursor position.
        let Some((hint_surface, hint_location)) = self.cursor_position_hint.take() else {
            return;
        };

        match constraint_remove {
            ConstraintRemove::Destroyed(pointer_constraint) => match pointer_constraint {
                PointerConstraint::Confined(_confined_pointer) => {}
                PointerConstraint::Locked(locked_pointer) => {
                    let origin = self
                        .space
                        .elements()
                        .find_map(|window| {
                            (window.wl_surface().as_deref() == Some(&hint_surface)).then(|| window.geometry())
                        })
                        .unwrap_or_default()
                        .loc
                        .to_f64();

                    let surface_location = origin + hint_location;
                    if let Some(region) = locked_pointer.region()
                        && region.contains(hint_location.to_i32_floor())
                    {
                        pointer.set_location(surface_location);
                    } else {
                        pointer.set_location(surface_location);
                    }
                }
            },
            ConstraintRemove::PointerLeave(_region) => {}
        }
    }

    fn cursor_position_hint(
        &mut self,
        surface: &WlSurface,
        pointer: &PointerHandle<Self>,
        location: Point<f64, Logical>,
    ) {
        if with_pointer_constraint(surface, pointer, |constraint| {
            constraint.is_some_and(|c| c.is_active())
        }) {
            self.cursor_position_hint = Some((surface.clone(), location));
        }
    }
}

impl<BackendData: Backend> TextInputActivation for AnvilState<BackendData> {}

impl<BackendData: Backend> AnvilState<BackendData> {
    /// Whether `surface` belongs to one of Olo's own programs.
    fn is_our_program(&self, surface: &WlSurface) -> bool {
        let Some(pid) = surface.client().and_then(|c| client_pid(&c, &self.display_handle)) else {
            return false;
        };
        std::fs::read_link(format!("/proc/{pid}/exe"))
            .ok()
            .and_then(|p| p.file_name()?.to_str().map(|n| n.starts_with("olo-") || n.starts_with("tabula-")))
            .unwrap_or(false)
    }
}

impl<BackendData: Backend> XdgActivationHandler for AnvilState<BackendData> {
    fn activation_state(&mut self) -> &mut XdgActivationState {
        &mut self.xdg_activation_state
    }

    fn token_created(&mut self, _token: XdgActivationToken, _data: XdgActivationTokenData) -> bool {
        // Whether a token may bring a window forward is decided when it's
        // used (request_activation), where we know who asks.
        true
    }

    fn request_activation(
        &mut self,
        _token: XdgActivationToken,
        token_data: XdgActivationTokenData,
        surface: WlSurface,
    ) {
        // A window comes forward only when the user just did something in
        // the app that asks (the token carries a recent input serial from
        // the focused client), or when one of our own programs asks: Olo
        // apps relaunched from Deskbar or Tracker forward the request to
        // the running instance, which has no fresh serial of its own.
        // Anything else (a terminal's bell, an app presenting itself) would
        // steal focus and cover what the user is doing, alerts included.
        let fresh = token_data.serial.as_ref().is_some_and(|(serial, _)| {
            self.seat.get_keyboard().is_some_and(|k| {
                    k.last_enter().is_some_and(|enter| serial.is_no_older_than(&enter))
                    && k.current_focus().and_then(|f| f.wl_surface().map(|s| s.client())).flatten() == surface.client()
            })
        });
        // One of our programs handing its user's click on to another app
        // (a notification clicked brings the app that sent it forward).
        let handed = token_data.serial.is_some() && token_data.surface.as_ref().is_some_and(|s| self.is_our_program(s));
        if !fresh && !handed && !self.is_our_program(&surface) {
            tracing::info!("activation request ignored (no recent input; app {:?})", token_data.app_id);
            return;
        }
        if token_data.timestamp.elapsed().as_secs() < 10 {
            // Just grant the wish
            let w = self
                .space
                .elements()
                .find(|window| window.wl_surface().map(|s| *s == surface).unwrap_or(false))
                .cloned();
            // Parked windows (minimized, other workspace) too.
            let w = w.or_else(|| {
                self.parked
                    .0
                    .borrow()
                    .iter()
                    .find(|window| window.wl_surface().is_some_and(|s| *s == surface))
                    .cloned()
            });
            if let Some(window) = w {
                self.restore(&window);
            }
        }
    }
}

impl<BackendData: Backend> XdgDecorationHandler for AnvilState<BackendData> {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        use xdg_decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode;
        // Olo draws every window's tab and frame itself.
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(Mode::ServerSide);
        });
    }
    fn request_mode(&mut self, toplevel: ToplevelSurface, mode: DecorationMode) {
        use xdg_decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode;

        // BeOS windows all wear the same tab; the compositor has the last word.
        let _ = mode;
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(Mode::ServerSide);
        });

        if toplevel.is_initial_configure_sent() {
            toplevel.send_pending_configure();
        }
    }
    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        use xdg_decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode;
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(Mode::ServerSide);
        });

        if toplevel.is_initial_configure_sent() {
            toplevel.send_pending_configure();
        }
    }
}

/// A surface's choice over KDE's server-decoration protocol: true when we
/// draw its tab and frame.
pub struct KdeServerSide(pub std::sync::atomic::AtomicBool);

/// Whether `surface` asked (over KDE's protocol) for our tab and frame.
pub fn kde_server_side(surface: &WlSurface) -> bool {
    smithay::wayland::compositor::with_states(surface, |states| {
        states.data_map.get::<KdeServerSide>().is_some_and(|k| k.0.load(std::sync::atomic::Ordering::Relaxed))
    })
}

impl<BackendData: Backend> smithay::wayland::shell::kde::decoration::KdeDecorationHandler for AnvilState<BackendData> {
    fn kde_decoration_state(&self) -> &smithay::wayland::shell::kde::decoration::KdeDecorationState {
        &self.kde_decoration_state
    }

    fn new_decoration(
        &mut self,
        surface: &WlSurface,
        decoration: &smithay::reexports::wayland_protocols_misc::server_decoration::server::org_kde_kwin_server_decoration::OrgKdeKwinServerDecoration,
    ) {
        use smithay::reexports::wayland_protocols_misc::server_decoration::server::org_kde_kwin_server_decoration::Mode;
        decoration.mode(Mode::Server);
        self.set_kde_mode(surface, true);
    }

    fn request_mode(
        &mut self,
        surface: &WlSurface,
        decoration: &smithay::reexports::wayland_protocols_misc::server_decoration::server::org_kde_kwin_server_decoration::OrgKdeKwinServerDecoration,
        mode: smithay::reexports::wayland_server::WEnum<
            smithay::reexports::wayland_protocols_misc::server_decoration::server::org_kde_kwin_server_decoration::Mode,
        >,
    ) {
        use smithay::reexports::wayland_protocols_misc::server_decoration::server::org_kde_kwin_server_decoration::Mode;
        // A window that draws its own titlebar (a GTK headerbar) asks for
        // client mode: honour it, or it would wear two.
        let server = !matches!(mode, smithay::reexports::wayland_server::WEnum::Value(Mode::Client | Mode::None));
        decoration.mode(if server { Mode::Server } else { Mode::Client });
        self.set_kde_mode(surface, server);
    }

    fn release(
        &mut self,
        _decoration: &smithay::reexports::wayland_protocols_misc::server_decoration::server::org_kde_kwin_server_decoration::OrgKdeKwinServerDecoration,
        surface: &WlSurface,
    ) {
        self.set_kde_mode(surface, false);
    }
}

impl<BackendData: Backend> AnvilState<BackendData> {
    fn set_kde_mode(&mut self, surface: &WlSurface, server: bool) {
        smithay::wayland::compositor::with_states(surface, |states| {
            states.data_map.insert_if_missing_threadsafe(|| KdeServerSide(std::sync::atomic::AtomicBool::new(false)));
            if let Some(k) = states.data_map.get::<KdeServerSide>() {
                k.0.store(server, std::sync::atomic::Ordering::Relaxed);
            }
        });
        // A window already up changes its frame now.
        if let Some(window) = self.space.elements().find(|w| w.wl_surface().as_deref() == Some(surface)).cloned() {
            window.set_ssd(server);
        }
    }
}

impl<BackendData: Backend> FractionalScaleHandler for AnvilState<BackendData> {
    fn new_fractional_scale(
        &mut self,
        surface: smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    ) {
        // Here we can set the initial fractional scale
        //
        // First we look if the surface already has a primary scan-out output, if not
        // we test if the surface is a subsurface and try to use the primary scan-out output
        // of the root surface. If the root also has no primary scan-out output we just try
        // to use the first output of the toplevel.
        // If the surface is the root we also try to use the first output of the toplevel.
        //
        // If all the above tests do not lead to a output we just use the first output
        // of the space (which in case of anvil will also be the output a toplevel will
        // initially be placed on)
        #[allow(clippy::redundant_clone)]
        let mut root = surface.clone();
        while let Some(parent) = get_parent(&root) {
            root = parent;
        }

        with_states(&surface, |states| {
            let primary_scanout_output = surface_primary_scanout_output(&surface, states)
                .or_else(|| {
                    if root != surface {
                        with_states(&root, |states| {
                            surface_primary_scanout_output(&root, states).or_else(|| {
                                self.window_for_surface(&root).and_then(|window| {
                                    self.space.outputs_for_element(&window).first().cloned()
                                })
                            })
                        })
                    } else {
                        self.window_for_surface(&root)
                            .and_then(|window| self.space.outputs_for_element(&window).first().cloned())
                    }
                })
                .or_else(|| self.space.outputs().next().cloned());
            if let Some(output) = primary_scanout_output {
                with_fractional_scale(states, |fractional_scale| {
                    fractional_scale.set_preferred_scale(output.current_scale().fractional_scale());
                });
            }
        });
    }
}

impl<BackendData: Backend + 'static> SecurityContextHandler for AnvilState<BackendData> {
    fn context_created(&mut self, source: SecurityContextListenerSource, security_context: SecurityContext) {
        self.handle
            .insert_source(source, move |client_stream, _, data| {
                let client_state = ClientState {
                    security_context: Some(security_context.clone()),
                    ..ClientState::default()
                };
                if let Err(err) = data
                    .display_handle
                    .insert_client(client_stream, Arc::new(client_state))
                {
                    warn!("Error adding wayland client: {}", err);
                };
            })
            .expect("Failed to init wayland socket source");
    }
}


impl<BackendData: Backend> XdgForeignHandler for AnvilState<BackendData> {
    fn xdg_foreign_state(&mut self) -> &mut XdgForeignState {
        &mut self.xdg_foreign_state
    }
}

impl<BackendData: Backend> ImageCaptureSourceHandler for AnvilState<BackendData> {
    fn source_destroyed(&mut self, _source: ImageCaptureSource) {
        // Anvil doesn't track sources
    }
}

impl<BackendData: Backend> OutputCaptureSourceHandler for AnvilState<BackendData> {
    fn output_capture_source_state(&mut self) -> &mut OutputCaptureSourceState {
        &mut self.output_capture_source_state
    }

    fn output_source_created(&mut self, source: ImageCaptureSource, output: &Output) {
        source.user_data().insert_if_missing(|| output.downgrade());
    }
}

impl<BackendData: Backend> ImageCopyCaptureHandler for AnvilState<BackendData> {
    fn image_copy_capture_state(&mut self) -> &mut ImageCopyCaptureState {
        &mut self.image_copy_capture_state
    }

    fn capture_constraints(&mut self, source: &ImageCaptureSource) -> Option<BufferConstraints> {
        use smithay::output::WeakOutput;
        let weak_output = source.user_data().get::<WeakOutput>()?;
        let output = weak_output.upgrade()?;
        let mode = output.current_mode()?;

        Some(BufferConstraints {
            size: mode
                .size
                .to_logical(1)
                .to_buffer(1, smithay::utils::Transform::Normal),
            shm: vec![
                smithay::reexports::wayland_server::protocol::wl_shm::Format::Argb8888,
                smithay::reexports::wayland_server::protocol::wl_shm::Format::Xrgb8888,
            ],
            #[cfg(any(feature = "udev", feature = "winit"))]
            dma: None,
        })
    }

    fn new_session(&mut self, _session: Session) {
        // Anvil doesn't track sessions; they clean up on drop
    }

    fn frame(&mut self, _session: &SessionRef, frame: Frame) {
        // Anvil doesn't implement actual capture
        frame.fail(smithay::wayland::image_copy_capture::CaptureFailureReason::Unknown);
    }
}

delegate_dispatch2!(@<BackendData: Backend + 'static> AnvilState<BackendData>);

impl<BackendData: Backend + 'static> AnvilState<BackendData> {
    pub fn init(
        display: Display<AnvilState<BackendData>>,
        handle: LoopHandle<'static, AnvilState<BackendData>>,
        backend_data: BackendData,
        listen_on_socket: bool,
    ) -> AnvilState<BackendData> {
        let dh = display.handle();

        let clock = Clock::new();

        // init wayland clients
        let socket_name = if listen_on_socket && let Some((name, listener)) = inherited_wayland_socket() {
            // olo-session owns the socket, so it outlives us: clients
            // stay connected-to-be across an in-place restart.
            listener.set_nonblocking(true).expect("nonblocking socket");
            handle
                .insert_source(Generic::new(listener, Interest::READ, Mode::Level), |_, listener, data| {
                    loop {
                        match listener.accept() {
                            Ok((stream, _)) => {
                                if let Err(err) =
                                    data.display_handle.insert_client(stream, Arc::new(ClientState::default()))
                                {
                                    warn!("Error adding wayland client: {}", err);
                                }
                            }
                            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                            Err(e) => {
                                warn!(?e, "accept failed");
                                break;
                            }
                        }
                    }
                    Ok(PostAction::Continue)
                })
                .expect("Failed to init inherited wayland socket");
            info!(name, "Listening on wayland socket from olo-session");
            proxied_wayland_socket(&handle);
            Some(name)
        } else if listen_on_socket {
            // Our own socket (not smithay's ListeningSocketSource, which
            // removes its files by name on exit, even once they're
            // another session's).
            let crate::sockfile::Wayland { name: socket_name, listener, _lock, _files } =
                crate::sockfile::bind_wayland().expect("Failed to create a wayland socket");
            listener.set_nonblocking(true).expect("nonblocking socket");
            let files = (_lock, _files);
            handle
                .insert_source(Generic::new(listener, Interest::READ, Mode::Level), move |_, listener, data| {
                    let _ = &files;
                    loop {
                        match listener.accept() {
                            Ok((stream, _)) => {
                                if let Err(err) =
                                    data.display_handle.insert_client(stream, Arc::new(ClientState::default()))
                                {
                                    warn!("Error adding wayland client: {}", err);
                                }
                            }
                            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                            Err(e) => {
                                warn!(?e, "accept failed");
                                break;
                            }
                        }
                    }
                    Ok(PostAction::Continue)
                })
                .expect("Failed to init wayland socket source");
            info!(name = socket_name, "Listening on wayland socket");
            Some(socket_name)
        } else {
            None
        };
        handle
            .insert_source(
                Generic::new(display, Interest::READ, Mode::Level),
                |_, display, data| {
                    profiling::scope!("dispatch_clients");
                    // Safety: we don't drop the display
                    unsafe {
                        display.get_mut().dispatch_clients(data).unwrap();
                    }
                    Ok(PostAction::Continue)
                },
            )
            .expect("Failed to init wayland server source");

        // init globals
        let compositor_state = CompositorState::new::<Self>(&dh);
        let data_device_state = DataDeviceState::new::<Self>(&dh);
        let layer_shell_state = WlrLayerShellState::new::<Self>(&dh);
        let output_manager_state = OutputManagerState::new_with_xdg_output::<Self>(&dh);
        let primary_selection_state = PrimarySelectionState::new::<Self>(&dh);
        let data_control_state =
            DataControlState::new::<Self, _>(&dh, Some(&primary_selection_state), |_| true);
        let mut seat_state = SeatState::new();
        let shm_state = ShmState::new::<Self>(&dh, vec![]);
        let viewporter_state = ViewporterState::new::<Self>(&dh);
        let xdg_activation_state = XdgActivationState::new::<Self>(&dh);
        let xdg_decoration_state = XdgDecorationState::new::<Self>(&dh);
        let kde_decoration_state = smithay::wayland::shell::kde::decoration::KdeDecorationState::new::<Self>(
            &dh,
            smithay::reexports::wayland_protocols_misc::server_decoration::server::org_kde_kwin_server_decoration_manager::Mode::Server,
        );
        let xdg_shell_state = XdgShellState::new::<Self>(&dh);
        let presentation_state = PresentationState::new::<Self>(&dh, clock.id() as u32);
        let fractional_scale_manager_state = FractionalScaleManagerState::new::<Self>(&dh);
        let xdg_foreign_state = XdgForeignState::new::<Self>(&dh);
        let single_pixel_buffer_state = SinglePixelBufferState::new::<Self>(&dh);
        let fifo_manager_state = FifoManagerState::new::<Self>(&dh);
        let commit_timing_manager_state = CommitTimingManagerState::new::<Self>(&dh);
        TextInputManagerState::new::<Self>(&dh);
        InputMethodManagerState::new::<Self, _>(&dh, |_client| true);
        VirtualKeyboardManagerState::new::<Self, _>(&dh, |_client| true);
        // Expose global only if backend supports relative motion events
        if BackendData::HAS_RELATIVE_MOTION {
            RelativePointerManagerState::new::<Self>(&dh);
        }
        PointerConstraintsState::new::<Self>(&dh);
        if BackendData::HAS_GESTURES {
            PointerGesturesState::new::<Self>(&dh);
        }
        TabletManagerState::new::<Self>(&dh);
        // Clients name their cursors; we draw Haiku's.
        smithay::wayland::cursor_shape::CursorShapeManagerState::new::<Self>(&dh);
        SecurityContextState::new::<Self, _>(&dh, |client| {
            client
                .get_data::<ClientState>()
                .is_none_or(|client_state| client_state.security_context.is_none())
        });
        FixesState::new::<Self>(&dh);

        // Image capture protocols (screencopy)
        let image_capture_source_state = ImageCaptureSourceState::new();
        let output_capture_source_state = OutputCaptureSourceState::new::<Self>(&dh);
        let image_copy_capture_state = ImageCopyCaptureState::new::<Self>(&dh);

        // init input
        let seat_name = backend_data.seat_name();
        let mut seat = seat_state.new_wl_seat(&dh, seat_name.clone());

        let pointer = seat.add_pointer();
        seat.add_keyboard(XkbConfig::default(), 200, 25)
            .expect("Failed to initialize the keyboard");

        let keyboard_shortcuts_inhibit_state = KeyboardShortcutsInhibitState::new::<Self>(&dh);
        IdleInhibitManagerState::new::<Self>(&dh);
        let lock = crate::lock::Lock::new::<BackendData>(&dh);



        let mut state = AnvilState {
            backend_data,
            num_lock_applied: None,
            display_handle: dh,
            socket_name,
            running: Arc::new(AtomicBool::new(true)),
            handle,
            space: Space::default(),
            popups: PopupManager::default(),
            momentum: Default::default(),
            compositor_state,
            data_device_state,
            layer_shell_state,
            output_manager_state,
            primary_selection_state,
            data_control_state,
            seat_state,
            keyboard_shortcuts_inhibit_state,
            shm_state,
            viewporter_state,
            xdg_activation_state,
            xdg_decoration_state,
            kde_decoration_state,
            xdg_shell_state,
            presentation_state,
            fractional_scale_manager_state,
            xdg_foreign_state,
            single_pixel_buffer_state,
            fifo_manager_state,
            commit_timing_manager_state,
            image_capture_source_state,
            output_capture_source_state,
            image_copy_capture_state,
            dnd_icon: None,
            suppressed_keys: Vec::new(),
            keymap: Default::default(),
            power: crate::power::Power::new(BackendData::IS_SESSION),
            saver: Default::default(),
            lock,
            twitch: None,
            tile_drag: None,
            tile_followers: Vec::new(),
            twitcher: Default::default(),
            hints: Default::default(),
            wallpaper: Default::default(),
            cursor_status: CursorImageStatus::default_named(),
            seat_name,
            seat,
            pointer,
            cursor_position_hint: None,
            clock,

            #[cfg(feature = "debug")]
            renderdoc: renderdoc::RenderDoc::new().ok(),
            show_window_preview: false,
            parked: Default::default(),
            workspace: 0,
            settings: {
                let settings = crate::desktop::load_settings();
                // `op` is there (or not) as Preferences ▸ Security says.
                olo_xdg::link_op_here(settings.security.passwords.op_cli);
                crate::shell::deco::set_title_chars(settings.tab_title_chars);
                settings
            },
            ipc: Default::default(),
            last_window_infos: Vec::new(),
            layout: crate::layout::load(),
            night_light: None,
            white_point: None,
            night_fade: None,
            mirrors: Vec::new(),
            desktop_shown: Vec::new(),
            arranged: Vec::new(),
            groups: Default::default(),
            recording_shortcut: 0,
        };
        if let Some(name) = state.socket_name.clone() {
            match crate::ipc::init(&state.handle, &name) {
                Ok(ipc) => state.ipc = ipc,
                Err(e) => warn!(?e, "IPC disabled"),
            }
        }
        crate::stop_on_signals(state.running.clone());
        state.apply_input_settings();
        state.update_appearance(true);
        // Re-check the Auto theme / night-light schedule every 30 s.
        let _ = state.handle.insert_source(
            smithay::reexports::calloop::timer::Timer::from_duration(std::time::Duration::from_secs(30)),
            |_, _, state| {
                state.update_appearance(false);
                smithay::reexports::calloop::timer::TimeoutAction::ToDuration(std::time::Duration::from_secs(30))
            },
        );
        let _ = state.handle.insert_source(smithay::reexports::calloop::timer::Timer::from_duration(crate::power::TICK), |_, _, state| {
            state.power_tick();
            smithay::reexports::calloop::timer::TimeoutAction::ToDuration(crate::power::TICK)
        });
        if let Some(feedback) = state.power.volume_feedback.take() {
            let _ = state.handle.insert_source(feedback, |event, _, state| {
                if let smithay::reexports::calloop::channel::Event::Msg((level, muted)) = event {
                    state.show_indicator(crate::osd::Indicator::Volume { level, muted });
                }
            });
        }
        state.keymap.set_bindings(olo_protocol::shortcuts::bindings(&state.settings.shortcuts));
        if let Some(found) = state.power.displays_found.take() {
            let _ = state.handle.insert_source(found, |event, _, state| {
                if let smithay::reexports::calloop::channel::Event::Msg(found) = event {
                    state.displays_found(found);
                }
            });
        }
        state.relock();
        state.start_wallpaper();
        if BackendData::IS_SESSION {
            state.watch_login();
        }
        crate::text::warm_up();
        state
    }

}

impl<BackendData: Backend + 'static> AnvilState<BackendData> {
    pub fn pre_repaint(&mut self, output: &Output, frame_target: impl Into<Time<Monotonic>>) {
        let frame_target = frame_target.into();
        self.update_wallpaper(output);

        #[allow(clippy::mutable_key_type)]
        let mut clients: HashMap<ClientId, Client> = HashMap::new();
        self.space.elements().for_each(|window| {
            window.with_surfaces(|surface, states| {
                if let Some(mut commit_timer_state) = states
                    .data_map
                    .get::<CommitTimerBarrierStateUserData>()
                    .map(|commit_timer| commit_timer.lock().unwrap())
                {
                    commit_timer_state.signal_until(frame_target);
                    let client = surface.client().unwrap();
                    clients.insert(client.id(), client);
                }
            });
        });

        let map = smithay::desktop::layer_map_for_output(output);
        for layer_surface in map.layers() {
            layer_surface.with_surfaces(|surface, states| {
                if let Some(mut commit_timer_state) = states
                    .data_map
                    .get::<CommitTimerBarrierStateUserData>()
                    .map(|commit_timer| commit_timer.lock().unwrap())
                {
                    commit_timer_state.signal_until(frame_target);
                    let client = surface.client().unwrap();
                    clients.insert(client.id(), client);
                }
            });
        }
        // Drop the lock to the layer map before calling blocker_cleared, which might end up
        // calling the commit handler which in turn again could access the layer map.
        std::mem::drop(map);

        if let CursorImageStatus::Surface(ref surface) = self.cursor_status {
            with_surfaces_surface_tree(surface, |surface, states| {
                if let Some(mut commit_timer_state) = states
                    .data_map
                    .get::<CommitTimerBarrierStateUserData>()
                    .map(|commit_timer| commit_timer.lock().unwrap())
                {
                    commit_timer_state.signal_until(frame_target);
                    let client = surface.client().unwrap();
                    clients.insert(client.id(), client);
                }
            });
        }

        if let Some(surface) = self.dnd_icon.as_ref().map(|icon| &icon.surface) {
            with_surfaces_surface_tree(surface, |surface, states| {
                if let Some(mut commit_timer_state) = states
                    .data_map
                    .get::<CommitTimerBarrierStateUserData>()
                    .map(|commit_timer| commit_timer.lock().unwrap())
                {
                    commit_timer_state.signal_until(frame_target);
                    let client = surface.client().unwrap();
                    clients.insert(client.id(), client);
                }
            });
        }

        let dh = self.display_handle.clone();
        for client in clients.into_values() {
            self.client_compositor_state(&client).blocker_cleared(self, &dh);
        }
    }

    pub fn post_repaint(
        &mut self,
        output: &Output,
        time: impl Into<Duration>,
        dmabuf_feedback: Option<SurfaceDmabufFeedback>,
        render_element_states: &RenderElementStates,
    ) {
        let time = time.into();
        let throttle = Some(Duration::from_secs(1));

        #[allow(clippy::mutable_key_type)]
        let mut clients: HashMap<ClientId, Client> = HashMap::new();

        self.space.elements().for_each(|window| {
            window.with_surfaces(|surface, states| {
                let primary_scanout_output = surface_primary_scanout_output(surface, states);

                if let Some(output) = primary_scanout_output.as_ref() {
                    with_fractional_scale(states, |fraction_scale| {
                        fraction_scale.set_preferred_scale(output.current_scale().fractional_scale());
                    });
                }

                if primary_scanout_output
                    .as_ref()
                    .map(|o| o == output)
                    .unwrap_or(true)
                {
                    let fifo_barrier = states
                        .cached_state
                        .get::<FifoBarrierCachedState>()
                        .current()
                        .barrier
                        .take();

                    if let Some(fifo_barrier) = fifo_barrier {
                        fifo_barrier.signal();
                        let client = surface.client().unwrap();
                        clients.insert(client.id(), client);
                    }
                }
            });

            if self.space.outputs_for_element(window).contains(output) {
                window.send_frame(output, time, throttle, surface_primary_scanout_output);
                if let Some(dmabuf_feedback) = dmabuf_feedback.as_ref() {
                    window.send_dmabuf_feedback(output, surface_primary_scanout_output, |surface, _| {
                        select_dmabuf_feedback(
                            surface,
                            render_element_states,
                            &dmabuf_feedback.render_feedback,
                            &dmabuf_feedback.scanout_feedback,
                        )
                    });
                }
            }
        });
        let map = smithay::desktop::layer_map_for_output(output);
        for layer_surface in map.layers() {
            layer_surface.with_surfaces(|surface, states| {
                let primary_scanout_output = surface_primary_scanout_output(surface, states);

                if let Some(output) = primary_scanout_output.as_ref() {
                    with_fractional_scale(states, |fraction_scale| {
                        fraction_scale.set_preferred_scale(output.current_scale().fractional_scale());
                    });
                }

                if primary_scanout_output
                    .as_ref()
                    .map(|o| o == output)
                    .unwrap_or(true)
                {
                    let fifo_barrier = states
                        .cached_state
                        .get::<FifoBarrierCachedState>()
                        .current()
                        .barrier
                        .take();

                    if let Some(fifo_barrier) = fifo_barrier {
                        fifo_barrier.signal();
                        let client = surface.client().unwrap();
                        clients.insert(client.id(), client);
                    }
                }
            });

            layer_surface.send_frame(output, time, throttle, surface_primary_scanout_output);
            if let Some(dmabuf_feedback) = dmabuf_feedback.as_ref() {
                layer_surface.send_dmabuf_feedback(output, surface_primary_scanout_output, |surface, _| {
                    select_dmabuf_feedback(
                        surface,
                        render_element_states,
                        &dmabuf_feedback.render_feedback,
                        &dmabuf_feedback.scanout_feedback,
                    )
                });
            }
        }
        if let Some(surface) = self.lock.surface_for(output) {
            smithay::desktop::utils::send_frames_surface_tree(surface, output, time, throttle, |_, _| Some(output.clone()));
        }
        // Drop the lock to the layer map before calling blocker_cleared, which might end up
        // calling the commit handler which in turn again could access the layer map.
        std::mem::drop(map);

        if let CursorImageStatus::Surface(ref surface) = self.cursor_status {
            with_surfaces_surface_tree(surface, |surface, states| {
                let primary_scanout_output = surface_primary_scanout_output(surface, states);

                if let Some(output) = primary_scanout_output.as_ref() {
                    with_fractional_scale(states, |fraction_scale| {
                        fraction_scale.set_preferred_scale(output.current_scale().fractional_scale());
                    });
                }

                if primary_scanout_output
                    .as_ref()
                    .map(|o| o == output)
                    .unwrap_or(true)
                {
                    let fifo_barrier = states
                        .cached_state
                        .get::<FifoBarrierCachedState>()
                        .current()
                        .barrier
                        .take();

                    if let Some(fifo_barrier) = fifo_barrier {
                        fifo_barrier.signal();
                        let client = surface.client().unwrap();
                        clients.insert(client.id(), client);
                    }
                }
            });
        }

        if let Some(surface) = self.dnd_icon.as_ref().map(|icon| &icon.surface) {
            with_surfaces_surface_tree(surface, |surface, states| {
                let primary_scanout_output = surface_primary_scanout_output(surface, states);

                if let Some(output) = primary_scanout_output.as_ref() {
                    with_fractional_scale(states, |fraction_scale| {
                        fraction_scale.set_preferred_scale(output.current_scale().fractional_scale());
                    });
                }

                if primary_scanout_output
                    .as_ref()
                    .map(|o| o == output)
                    .unwrap_or(true)
                {
                    let fifo_barrier = states
                        .cached_state
                        .get::<FifoBarrierCachedState>()
                        .current()
                        .barrier
                        .take();

                    if let Some(fifo_barrier) = fifo_barrier {
                        fifo_barrier.signal();
                        let client = surface.client().unwrap();
                        clients.insert(client.id(), client);
                    }
                }
            });
        }

        let dh = self.display_handle.clone();
        for client in clients.into_values() {
            self.client_compositor_state(&client).blocker_cleared(self, &dh);
        }
    }
}

pub fn update_primary_scanout_output(
    space: &Space<WindowElement>,
    output: &Output,
    dnd_icon: &Option<DndIcon>,
    cursor_status: &CursorImageStatus,
    render_element_states: &RenderElementStates,
) {
    space.elements().for_each(|window| {
        window.with_surfaces(|surface, states| {
            update_surface_primary_scanout_output(
                surface,
                output,
                states,
                None,
                render_element_states,
                default_primary_scanout_output_compare,
            );
        });
    });
    // A layer surface belongs to one display, and only that display's pass
    // sees it: the default comparison would keep the display it was on
    // before moving (Deskbar and the desktop follow the main display), and
    // its frame callbacks would then come only from the once-a-second
    // fallback, leaving it all but deaf to clicks.
    let map = smithay::desktop::layer_map_for_output(output);
    for layer_surface in map.layers() {
        layer_surface.with_surfaces(|surface, states| {
            update_surface_primary_scanout_output(surface, output, states, None, render_element_states, |_, _, next, _| next);
        });
    }

    if let CursorImageStatus::Surface(surface) = cursor_status {
        with_surfaces_surface_tree(surface, |surface, states| {
            update_surface_primary_scanout_output(
                surface,
                output,
                states,
                None,
                render_element_states,
                default_primary_scanout_output_compare,
            );
        });
    }

    if let Some(surface) = dnd_icon.as_ref().map(|icon| &icon.surface) {
        with_surfaces_surface_tree(surface, |surface, states| {
            update_surface_primary_scanout_output(
                surface,
                output,
                states,
                None,
                render_element_states,
                default_primary_scanout_output_compare,
            );
        });
    }
}

#[derive(Debug, Clone)]
pub struct SurfaceDmabufFeedback {
    pub render_feedback: DmabufFeedback,
    pub scanout_feedback: DmabufFeedback,
}

#[profiling::function]
pub fn take_presentation_feedback(
    output: &Output,
    space: &Space<WindowElement>,
    render_element_states: &RenderElementStates,
) -> OutputPresentationFeedback {
    let mut output_presentation_feedback = OutputPresentationFeedback::new(output);

    space.elements().for_each(|window| {
        if space.outputs_for_element(window).contains(output) {
            window.take_presentation_feedback(
                &mut output_presentation_feedback,
                surface_primary_scanout_output,
                |surface, _| {
                    surface_presentation_feedback_flags_from_states(surface, None, render_element_states)
                },
            );
        }
    });
    let map = smithay::desktop::layer_map_for_output(output);
    for layer_surface in map.layers() {
        layer_surface.take_presentation_feedback(
            &mut output_presentation_feedback,
            surface_primary_scanout_output,
            |surface, _| {
                surface_presentation_feedback_flags_from_states(surface, None, render_element_states)
            },
        );
    }

    output_presentation_feedback
}

pub trait Backend {
    const HAS_RELATIVE_MOTION: bool = false;
    const HAS_GESTURES: bool = false;
    /// Running as the real desktop session (not nested for development).
    /// Only then do we touch user-global settings like the GTK colour scheme.
    const IS_SESSION: bool = false;
    fn seat_name(&self) -> String;
    /// Push touchpad/mouse settings to input devices.
    fn apply_input_settings(&mut self, _settings: &olo_protocol::Settings) {}
    fn reset_buffers(&mut self, output: &Output);
    fn early_import(&mut self, surface: &WlSurface);
    fn update_led_state(&mut self, led_state: LedState);
    /// Switch `output` to `mode` (one of `output.modes()`), updating the
    /// output's current mode on success.
    /// Turn the screens off (DPMS) for idleness, or back on.
    fn set_displays_on(&mut self, _on: bool) {}
    /// Scale the screens' gamma ramps by these red, green, blue gains
    /// (night light; `[1.0; 3]` is neutral).
    fn set_white_point(&mut self, _gains: [f64; 3]) {}
    fn set_mode(&mut self, _output: &Output, _mode: smithay::output::Mode) -> Result<(), String> {
        Err("This display's resolution can't be changed.".into())
    }
}

/// The socket olo-wlproxy connects to for other programs
/// (`OLO_WAYLAND_PROXY_FD`). Each connection opens with a 16-byte
/// hello, "TBPX", version, the program's pid, uid; we answer "TBPX",
/// version, our pid, 0 (so the proxy knows a new app_server from the old
/// one) and the Wayland stream follows.
fn proxied_wayland_socket<BackendData: Backend + 'static>(handle: &LoopHandle<'static, AnvilState<BackendData>>) {
    use std::io::{Read, Write};
    use std::os::fd::FromRawFd;
    let Some(fd) = std::env::var("OLO_WAYLAND_PROXY_FD").ok().and_then(|f| f.parse::<i32>().ok()) else { return };
    // SAFETY: olo-session passes this fd to us alone.
    let listener = unsafe { std::os::unix::net::UnixListener::from_raw_fd(fd) };
    crate::set_cloexec(fd);
    // SAFETY: called during single-threaded start-up.
    unsafe { std::env::remove_var("OLO_WAYLAND_PROXY_FD") };
    listener.set_nonblocking(true).expect("nonblocking socket");
    let accept = handle.insert_source(Generic::new(listener, Interest::READ, Mode::Level), |_, listener, data| {
        loop {
            let stream = match listener.accept() {
                Ok((stream, _)) => stream,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) => {
                    warn!(?e, "accept failed");
                    break;
                }
            };
            let _ = stream.set_nonblocking(true);
            let mut hello = Vec::with_capacity(16);
            let read = data.handle.insert_source(Generic::new(stream, Interest::READ, Mode::Level), move |_, stream, data| {
                let mut buf = [0u8; 16];
                match (&**stream).read(&mut buf[..16 - hello.len()]) {
                    Ok(0) => return Ok(PostAction::Remove),
                    Ok(n) => hello.extend_from_slice(&buf[..n]),
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return Ok(PostAction::Continue),
                    Err(_) => return Ok(PostAction::Remove),
                }
                if hello.len() < 16 {
                    return Ok(PostAction::Continue);
                }
                if &hello[..4] != b"TBPX" {
                    warn!("a proxied connection without the hello");
                    return Ok(PostAction::Remove);
                }
                let pid = i32::from_ne_bytes(hello[8..12].try_into().unwrap());
                let mut reply = Vec::with_capacity(16);
                reply.extend_from_slice(b"TBPX");
                reply.extend_from_slice(&1u32.to_ne_bytes());
                reply.extend_from_slice(&(std::process::id() as i32).to_ne_bytes());
                reply.extend_from_slice(&0u32.to_ne_bytes());
                // A new socket's buffer takes 16 bytes.
                if (&**stream).write_all(&reply).is_err() {
                    return Ok(PostAction::Remove);
                }
                if let Ok(s) = (**stream).try_clone() {
                    let state = ClientState { pid: (pid > 0).then_some(pid), proxied: true, ..Default::default() };
                    if let Err(err) = data.display_handle.insert_client(s, Arc::new(state)) {
                        warn!("Error adding wayland client: {}", err);
                    }
                }
                Ok(PostAction::Remove)
            });
            if let Err(e) = read {
                warn!(?e, "could not wait for a proxied connection's hello");
            }
        }
        Ok(PostAction::Continue)
    });
    match accept {
        Ok(_) => info!("Listening for olo-wlproxy"),
        Err(e) => warn!(?e, "could not listen for olo-wlproxy"),
    }
}

/// The Wayland listening socket handed down by olo-session
/// (`OLO_WAYLAND_FD`, with its name in `WAYLAND_DISPLAY`).
fn inherited_wayland_socket() -> Option<(String, std::os::unix::net::UnixListener)> {
    use std::os::fd::FromRawFd;
    let fd: i32 = std::env::var("OLO_WAYLAND_FD").ok()?.parse().ok()?;
    let name = std::env::var("WAYLAND_DISPLAY").ok()?;
    // SAFETY: olo-session passes this fd to us alone.
    let listener = unsafe { std::os::unix::net::UnixListener::from_raw_fd(fd) };
    crate::set_cloexec(fd);
    // SAFETY: called during single-threaded start-up.
    unsafe { std::env::remove_var("OLO_WAYLAND_FD") };
    Some((name, listener))
}
