use std::{
    cell::{Cell, Ref, RefCell, RefMut},
    ffi::c_void,
    ptr::NonNull,
    rc::Rc,
    sync::Arc,
};

use blade_graphics as gpu;
use calloop::LoopHandle;
use collections::HashMap;
use futures::channel::oneshot::Receiver;

use raw_window_handle as rwh;
use wayland_backend::client::ObjectId;
use wayland_client::WEnum;
use anyhow::Context as _;
use wayland_client::{
    Proxy,
    protocol::{wl_output, wl_surface},
};
use wayland_protocols::wp::viewporter::client::wp_viewport;
use wayland_protocols::xdg::decoration::zv1::client::zxdg_toplevel_decoration_v1;
use wayland_protocols::xdg::shell::client::xdg_surface;
use wayland_protocols::xdg::shell::client::xdg_toplevel::{self};
use wayland_protocols::xdg::shell::client::{xdg_popup, xdg_positioner};
use wayland_protocols::{
    wp::fractional_scale::v1::client::wp_fractional_scale_v1,
    xdg::shell::client::xdg_toplevel::XdgToplevel,
};
use wayland_protocols_plasma::blur::client::org_kde_kwin_blur;
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};
use wayland_protocols::ext::session_lock::v1::client::{ext_session_lock_surface_v1, ext_session_lock_v1};

use crate::{
    AnyWindowHandle, Bounds, Decorations, DevicePixels, Globals, GpuSpecs, Modifiers, Output, Pixels,
    PlatformDisplay, PlatformInput, Point, PromptButton, PromptLevel, RequestFrameOptions,
    ResizeEdge, Size, Tiling, WaylandClientStatePtr, WindowAppearance, WindowBackgroundAppearance,
    WindowBounds, WindowControlArea, WindowControls, WindowDecorations, WindowParams, px, size,
};
use crate::{
    Capslock,
    platform::{
        PlatformAtlas, PlatformInputHandler, PlatformWindow,
        blade::{BladeContext, BladeRenderer, BladeSurfaceConfig},
        linux::wayland::{display::WaylandDisplay, serial::SerialKind},
    },
};
use crate::{WindowKind, scene::Scene};

#[derive(Default)]
pub(crate) struct Callbacks {
    request_frame: Option<Box<dyn FnMut(RequestFrameOptions) -> bool>>,
    input: Option<Box<dyn FnMut(crate::PlatformInput) -> crate::DispatchEventResult>>,
    active_status_change: Option<Box<dyn FnMut(bool)>>,
    hover_status_change: Option<Box<dyn FnMut(bool)>>,
    resize: Option<Box<dyn FnMut(Size<Pixels>, f32)>>,
    moved: Option<Box<dyn FnMut()>>,
    should_close: Option<Box<dyn FnMut() -> bool>>,
    close: Option<Box<dyn FnOnce()>>,
    appearance_changed: Option<Box<dyn FnMut()>>,
}

struct RawWindow {
    window: *mut c_void,
    display: *mut c_void,
}

impl rwh::HasWindowHandle for RawWindow {
    fn window_handle(&self) -> Result<rwh::WindowHandle<'_>, rwh::HandleError> {
        let window = NonNull::new(self.window).unwrap();
        let handle = rwh::WaylandWindowHandle::new(window);
        Ok(unsafe { rwh::WindowHandle::borrow_raw(handle.into()) })
    }
}
impl rwh::HasDisplayHandle for RawWindow {
    fn display_handle(&self) -> Result<rwh::DisplayHandle<'_>, rwh::HandleError> {
        let display = NonNull::new(self.display).unwrap();
        let handle = rwh::WaylandDisplayHandle::new(display);
        Ok(unsafe { rwh::DisplayHandle::borrow_raw(handle.into()) })
    }
}

#[derive(Debug)]
struct InProgressConfigure {
    size: Option<Size<Pixels>>,
    fullscreen: bool,
    maximized: bool,
    resizing: bool,
    tiling: Tiling,
}

pub struct WaylandWindowState {
    /// A window's or popup's (None for a layer surface).
    xdg_surface: Option<xdg_surface::XdgSurface>,
    /// Olo: a layer surface's (`WindowKind::LayerShell`), and where it
    /// sits.
    layer_surface: Option<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1>,
    layer_shell: Option<crate::LayerShellOptions>,
    /// Olo: a lock surface's (`WindowKind::SessionLock`).
    lock_surface: Option<ext_session_lock_surface_v1::ExtSessionLockSurfaceV1>,
    acknowledged_first_configure: bool,
    pub surface: wl_surface::WlSurface,
    decoration: Option<zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1>,
    app_id: Option<String>,
    appearance: WindowAppearance,
    blur: Option<org_kde_kwin_blur::OrgKdeKwinBlur>,
    /// Olo: the screen kept awake while it shows.
    inhibitor: Option<wayland_protocols::wp::idle_inhibit::zv1::client::zwp_idle_inhibitor_v1::ZwpIdleInhibitorV1>,
    /// Olo: its scale's object and, for a dialog, the dialog's, destroyed
    /// with it (left alive, they name a surface or toplevel that's gone,
    /// whose id goes to something else: olo-wlproxy making them again
    /// after an app_server restart would name that).
    fractional_scale: Option<wp_fractional_scale_v1::WpFractionalScaleV1>,
    dialog: Option<wayland_protocols::xdg::dialog::v1::client::xdg_dialog_v1::XdgDialogV1>,
    /// Olo: the other program's window it's over (xdg-foreign), let go
    /// with it.
    imported: Option<wayland_protocols::xdg::foreign::zv2::client::zxdg_imported_v2::ZxdgImportedV2>,
    /// A window's role (None for a popup, which has `popup`).
    toplevel: Option<xdg_toplevel::XdgToplevel>,
    popup: Option<xdg_popup::XdgPopup>,
    /// Olo: the popup this one opened over (a submenu's menu), if any.
    popup_of: Option<xdg_surface::XdgSurface>,
    /// Olo: a popup that grabbed the pointer and keyboard (a menu; not a
    /// tip).
    grabbing: bool,
    viewport: Option<wp_viewport::WpViewport>,
    outputs: HashMap<ObjectId, Output>,
    display: Option<(ObjectId, Output)>,
    /// Olo: the displays it may open on (those at the scale the
    /// compositor gave it), its text drawn for until it's on one.
    may_open_on: Vec<(ObjectId, Output)>,
    globals: Globals,
    renderer: BladeRenderer,
    bounds: Bounds<Pixels>,
    scale: f32,
    input_handler: Option<PlatformInputHandler>,
    decorations: WindowDecorations,
    background_appearance: WindowBackgroundAppearance,
    fullscreen: bool,
    maximized: bool,
    tiling: Tiling,
    window_bounds: Bounds<Pixels>,
    client: WaylandClientStatePtr,
    handle: AnyWindowHandle,
    active: bool,
    hovered: bool,
    in_progress_configure: Option<InProgressConfigure>,
    resize_throttle: bool,
    /// Olo: being resized by hand (xdg's `resizing`): its swapchain has
    /// room to grow, and is made to fit once the size rests.
    resizing: bool,
    /// Olo: the part of its buffer the viewport shows (device pixels), as
    /// last set.
    viewport_source: Option<Size<DevicePixels>>,
    in_progress_window_controls: Option<WindowControls>,
    window_controls: WindowControls,
    client_inset: Option<Pixels>,
}

#[derive(Clone)]
pub struct WaylandWindowStatePtr {
    state: Rc<RefCell<WaylandWindowState>>,
    callbacks: Rc<RefCell<Callbacks>>,
    frames: Rc<FrameLoop>,
}

/// Olo: a window draws only when something changed. While it animates,
/// each frame asks the compositor for the next (a frame callback, sent
/// with the commit that shows it); when a frame has nothing to draw,
/// nothing is committed and no callback comes, so an idle window costs
/// nothing. Something changing while idle draws at once (on the event
/// loop's next turn); while a committed callback is on its way, it waits
/// for that instead, so frames stay paced by the display.
struct FrameLoop {
    callback: Cell<FrameCallback>,
    /// A draw is queued on the event loop.
    queued: Cell<bool>,
    /// A configure was acknowledged: commit even if nothing is drawn.
    needs_commit: Cell<bool>,
    /// When the frame last committed (a callback that never comes, as
    /// for a hidden window, holds changes back for a second at most).
    committed_at: Cell<std::time::Instant>,
    /// Draw everything again (the text is to be drawn another way).
    force: Cell<bool>,
    /// How text is drawn, for the display the window is on, and the
    /// generation of the person's choices it was chosen with.
    text_rendering: Cell<crate::TextRendering>,
    text_generation: Cell<u64>,
    handle: LoopHandle<'static, WaylandClientStatePtr>,
    /// Wakes the loop for the frame queued: GPUI runs the main thread's
    /// tasks in idle callbacks, and calloop doesn't wake for an idle
    /// queued from one (a task's `refresh`), so without it the frame
    /// would wait for the next input.
    signal: calloop::LoopSignal,
    /// A wheel's notches being eased in.
    wheel: RefCell<Option<WheelEase>>,
    /// Olo: the window is gone (its surface destroyed): a frame queued
    /// before draws nothing.
    closed: Cell<bool>,
    /// A frame is being drawn, and something asked for another meanwhile
    /// (an animation's next frame): it comes with the display's next
    /// callback if this one commits, else at once.
    in_frame: Cell<bool>,
    wanted: Cell<bool>,
}

/// What is left to scroll of a wheel's notches, and where.
struct WheelEase {
    left: Point<f32>,
    position: Point<Pixels>,
    modifiers: Modifiers,
    /// When the last step was.
    last: Option<std::time::Instant>,
}

/// The time a wheel's notch takes to ease most of the way in: 90% in a
/// tenth of a second, as the egui kit's.
const WHEEL_EASE: f64 = 0.1 / std::f64::consts::LN_10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FrameCallback {
    None,
    /// Asked for, but not committed yet: comes after the next commit.
    Asked,
    /// Committed: comes when the compositor shows the frame.
    Committed,
}

impl WaylandWindowState {
    pub(crate) fn new(
        handle: AnyWindowHandle,
        surface: wl_surface::WlSurface,
        xdg_surface: Option<xdg_surface::XdgSurface>,
        layer_surface: Option<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1>,
        toplevel: Option<xdg_toplevel::XdgToplevel>,
        popup: Option<xdg_popup::XdgPopup>,
        decoration: Option<zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1>,
        appearance: WindowAppearance,
        viewport: Option<wp_viewport::WpViewport>,
        client: WaylandClientStatePtr,
        globals: Globals,
        gpu_context: &BladeContext,
        options: WindowParams,
        scale: Option<f32>,
    ) -> anyhow::Result<Self> {
        let renderer = {
            let raw_window = RawWindow {
                window: surface.id().as_ptr().cast::<c_void>(),
                display: surface
                    .backend()
                    .upgrade()
                    .unwrap()
                    .display_ptr()
                    .cast::<c_void>(),
            };
            // Olo: made at the size and transparency it will be drawn at
            // (its device size, once the scale is known; opaque as
            // `update_window` will have it, menus too): a swapchain made
            // again costs ~3 ms, and with another alpha its pipelines too,
            // ~10 ms more, each window opened.
            let device = options.bounds.size.to_device_pixels(scale.unwrap_or(1.0));
            let opaque = options.decorations == WindowDecorations::Server && options.background == WindowBackgroundAppearance::Opaque;
            let config = BladeSurfaceConfig {
                size: gpu::Extent {
                    width: device.width.0 as u32,
                    height: device.height.0 as u32,
                    depth: 1,
                },
                transparent: !opaque,
            };
            BladeRenderer::new(gpu_context, &raw_window, config)?
        };

        Ok(Self {
            xdg_surface,
            layer_surface,
            layer_shell: options.layer_shell.clone(),
            lock_surface: None,
            acknowledged_first_configure: false,
            surface,
            decoration,
            app_id: None,
            blur: None,
            inhibitor: None,
            fractional_scale: None,
            dialog: None,
            imported: None,
            toplevel,
            popup,
            popup_of: None,
            grabbing: false,
            viewport,
            globals,
            outputs: HashMap::default(),
            display: None,
            may_open_on: Vec::new(),
            renderer,
            bounds: options.bounds,
            scale: 1.0,
            input_handler: None,
            decorations: WindowDecorations::Client,
            background_appearance: WindowBackgroundAppearance::Opaque,
            fullscreen: false,
            maximized: false,
            tiling: Tiling::default(),
            window_bounds: options.bounds,
            in_progress_configure: None,
            resize_throttle: false,
            resizing: false,
            viewport_source: None,
            client,
            appearance,
            handle,
            active: false,
            hovered: false,
            in_progress_window_controls: None,
            window_controls: WindowControls::default(),
            client_inset: None,
        })
    }

    pub fn is_transparent(&self) -> bool {
        self.decorations == WindowDecorations::Client
            || self.background_appearance != WindowBackgroundAppearance::Opaque
    }

    /// How its text is drawn: for the display it's on, or before it's on
    /// one (its first frame), as all those it may open on would (else as
    /// by default).
    fn text_rendering(&self) -> crate::TextRendering {
        // LCD coverage needs what's under the text to be what shows (a
        // window's frame is app_server's, once it has said so).
        let opaque = self.background_appearance == WindowBackgroundAppearance::Opaque
            && (self.decorations == WindowDecorations::Server || (self.display.is_none() && self.decoration.is_some()));
        let lcd_possible = opaque && self.renderer.supports_subpixel_text();
        if let Some((_, output)) = self.display.as_ref() {
            return output.text_rendering(lcd_possible);
        }
        let mut each = self.may_open_on.iter().map(|(_, output)| output.text_rendering(lcd_possible));
        match each.next() {
            Some(first) if each.all(|r| r == first) => first,
            _ => crate::TextRendering::default(),
        }
    }

    pub fn primary_output_scale(&mut self) -> i32 {
        let mut scale = 1;
        let mut current_output = self.display.take();
        for (id, output) in self.outputs.iter() {
            if let Some((_, output_data)) = &current_output {
                if output.scale > output_data.scale {
                    current_output = Some((id.clone(), output.clone()));
                }
            } else {
                current_output = Some((id.clone(), output.clone()));
            }
            scale = scale.max(output.scale);
        }
        self.display = current_output;
        scale
    }

    pub fn inset(&self) -> Pixels {
        match self.decorations {
            WindowDecorations::Server => px(0.0),
            WindowDecorations::Client => self.client_inset.unwrap_or(px(0.0)),
        }
    }
}

pub(crate) struct WaylandWindow(pub WaylandWindowStatePtr);

struct WeakWindowPtr {
    state: std::rc::Weak<RefCell<WaylandWindowState>>,
    callbacks: std::rc::Weak<RefCell<Callbacks>>,
    frames: std::rc::Weak<FrameLoop>,
}

impl WeakWindowPtr {
    fn upgrade(&self) -> Option<WaylandWindowStatePtr> {
        Some(WaylandWindowStatePtr {
            state: self.state.upgrade()?,
            callbacks: self.callbacks.upgrade()?,
            frames: self.frames.upgrade()?,
        })
    }
}
pub enum ImeInput {
    InsertText(String),
    SetMarkedText(String),
    UnmarkText,
    DeleteText,
}

impl Drop for WaylandWindow {
    fn drop(&mut self) {
        self.0.frames.closed.set(true);
        // Olo: what the window's callbacks hold (its views, through
        // GPUI's handlers and an app's close-from-its-tab) goes with it,
        // not once the task below runs: nothing may come after that to
        // release them (a dialog's view, answering no as it goes, didn't
        // until the next event). The close callback that task runs stays.
        if let Ok(mut callbacks) = self.0.callbacks.try_borrow_mut() {
            let close = callbacks.close.take();
            *callbacks = Callbacks { close, ..Callbacks::default() };
        }
        let mut state = self.0.state.borrow_mut();
        let surface_id = state.surface.id();
        let client = state.client.clone();

        state.renderer.destroy();
        if let Some(decoration) = &state.decoration {
            decoration.destroy();
        }
        if let Some(dialog) = state.dialog.take() {
            dialog.destroy();
        }
        if let Some(imported) = state.imported.take() {
            imported.destroy();
        }
        if let Some(scale) = state.fractional_scale.take() {
            scale.destroy();
        }
        if let Some(inhibitor) = state.inhibitor.take() {
            inhibitor.destroy();
        }
        if let Some(blur) = &state.blur {
            blur.release();
        }
        if let Some(toplevel) = &state.toplevel {
            toplevel.destroy();
        }
        if let Some(popup) = &state.popup {
            // Olo: a submenu still open over it goes first: destroying a
            // popup that isn't the topmost one is a protocol error, which
            // ends the program's connection (Deskbar, deaf to clicks after
            // closing a menu whose submenu was open, 2026-11-01).
            if let Some(xdg_surface) = &state.xdg_surface {
                client.withdraw_popups_over(xdg_surface);
            }
            popup.destroy();
        }
        if let Some(viewport) = &state.viewport {
            viewport.destroy();
        }
        if let Some(xdg_surface) = &state.xdg_surface {
            xdg_surface.destroy();
        }
        if let Some(layer_surface) = &state.layer_surface {
            layer_surface.destroy();
        }
        if let Some(lock_surface) = &state.lock_surface {
            lock_surface.destroy();
        }
        state.surface.destroy();

        let state_ptr = self.0.clone();
        state
            .globals
            .executor
            .spawn(async move {
                state_ptr.close();
                client.drop_window(&surface_id)
            })
            .detach();
        drop(state);
    }
}

impl WaylandWindow {
    fn borrow(&self) -> Ref<'_, WaylandWindowState> {
        self.0.state.borrow()
    }

    fn borrow_mut(&self) -> RefMut<'_, WaylandWindowState> {
        self.0.state.borrow_mut()
    }

    pub fn new(
        handle: AnyWindowHandle,
        globals: Globals,
        gpu_context: &BladeContext,
        client: WaylandClientStatePtr,
        params: WindowParams,
        appearance: WindowAppearance,
        parent: Option<XdgToplevel>,
        loop_handle: LoopHandle<'static, WaylandClientStatePtr>,
        signal: calloop::LoopSignal,
        popup_parent: Option<(PopupParent, u32)>,
        layer_output: Option<wl_output::WlOutput>,
        session_lock: Option<(ext_session_lock_v1::ExtSessionLockV1, wl_output::WlOutput)>,
        (scale, displays): (Option<f32>, Vec<(ObjectId, Output)>),
    ) -> anyhow::Result<(Self, ObjectId)> {
        let surface = globals.compositor.create_surface(&globals.qh, ());
        // Olo: a lock surface covers its output while the session is
        // locked; the compositor sizes it.
        if let Some((lock, output)) = session_lock {
            let lock_surface = lock.get_lock_surface(&surface, &output, &globals.qh, surface.id());
            let made = Self::finish(handle, globals, gpu_context, client, params, appearance, loop_handle, signal, surface, None, None, None, None, (scale, displays))?;
            made.0.0.state.borrow_mut().lock_surface = Some(lock_surface);
            return Ok(made);
        }
        // Olo: a layer surface (Deskbar, the desktop, notifications, an
        // overlay) has no xdg surface.
        if params.kind == WindowKind::LayerShell {
            let options = params.layer_shell.clone().context("a layer surface without its options")?;
            let shell = globals.layer_shell.as_ref().context("the compositor has no layer shell")?;
            let layer = shell.get_layer_surface(
                &surface,
                layer_output.as_ref(),
                match options.layer {
                    crate::LayerShellLayer::Background => zwlr_layer_shell_v1::Layer::Background,
                    crate::LayerShellLayer::Bottom => zwlr_layer_shell_v1::Layer::Bottom,
                    crate::LayerShellLayer::Top => zwlr_layer_shell_v1::Layer::Top,
                    crate::LayerShellLayer::Overlay => zwlr_layer_shell_v1::Layer::Overlay,
                },
                options.namespace.clone(),
                &globals.qh,
                surface.id(),
            );
            place_layer(&layer, &options, params.bounds.size);
            layer.set_keyboard_interactivity(layer_keyboard(&layer, options.keyboard));
            return Self::finish(handle, globals, gpu_context, client, params, appearance, loop_handle, signal, surface, None, Some(layer), None, None, (scale, displays));
        }
        let xdg_surface = globals
            .wm_base
            .get_xdg_surface(&surface, &globals.qh, surface.id());
        let mut dialog_object = None;
        let mut imported = None;
        let popup_of = match &popup_parent {
            Some((PopupParent::Xdg(parent), _)) => Some(parent.clone()),
            _ => None,
        };
        let (toplevel, popup) = match (params.kind, popup_parent) {
            (kind @ (WindowKind::Popup { .. } | WindowKind::Tip { .. }), Some((parent_surface, serial))) => {
                // Olo: a tip, below what it tells about (a gap away), or
                // above; taking no input, grabbing nothing.
                const TIP_GAP: f32 = 4.0;
                let (anchor, corner, offset, tip) = match kind {
                    WindowKind::Popup { anchor, corner, offset, .. } => (anchor, corner, offset, false),
                    WindowKind::Tip { anchor, .. } => {
                        let a = Bounds { origin: crate::point(anchor.origin.x, anchor.origin.y - px(TIP_GAP)), size: size(anchor.size.width, anchor.size.height + px(2.0 * TIP_GAP)) };
                        (a, crate::Corner::BottomLeft, Point::default(), true)
                    }
                    _ => unreachable!(),
                };
                // Olo: where it opens, and how the compositor may move it
                // to keep it on screen.
                let positioner = globals.wm_base.create_positioner(&globals.qh, ());
                let size = params.bounds.size;
                positioner.set_size(
                    (size.width.0 as i32).max(1),
                    (size.height.0 as i32).max(1),
                );
                positioner.set_anchor_rect(
                    anchor.origin.x.0 as i32,
                    anchor.origin.y.0 as i32,
                    (anchor.size.width.0 as i32).max(1),
                    (anchor.size.height.0 as i32).max(1),
                );
                positioner.set_anchor(match corner {
                    crate::Corner::TopLeft => xdg_positioner::Anchor::TopLeft,
                    crate::Corner::TopRight => xdg_positioner::Anchor::TopRight,
                    crate::Corner::BottomLeft => xdg_positioner::Anchor::BottomLeft,
                    crate::Corner::BottomRight => xdg_positioner::Anchor::BottomRight,
                });
                positioner.set_gravity(xdg_positioner::Gravity::BottomRight);
                let slide = xdg_positioner::ConstraintAdjustment::SlideX
                    | xdg_positioner::ConstraintAdjustment::SlideY;
                if offset == Point::default() {
                    positioner.set_constraint_adjustment(
                        slide
                            | xdg_positioner::ConstraintAdjustment::FlipX
                            | xdg_positioner::ConstraintAdjustment::FlipY,
                    );
                } else {
                    positioner.set_offset(offset.x.0.round() as i32, offset.y.0.round() as i32);
                    positioner.set_constraint_adjustment(slide);
                }
                let popup = match &parent_surface {
                    PopupParent::Xdg(parent) => {
                        xdg_surface.get_popup(Some(parent), &positioner, &globals.qh, surface.id())
                    }
                    // Over a layer surface (Deskbar's menus): made
                    // without a parent, then given to it.
                    PopupParent::Layer(parent) => {
                        let popup = xdg_surface.get_popup(None, &positioner, &globals.qh, surface.id());
                        parent.get_popup(&popup);
                        popup
                    }
                };
                positioner.destroy();
                if tip {
                    let region = globals.compositor.create_region(&globals.qh, ());
                    surface.set_input_region(Some(&region));
                    region.destroy();
                } else {
                    popup.grab(&globals.seat, serial);
                }
                (None, Some(popup))
            }
            _ => {
                let toplevel = xdg_surface.get_toplevel(&globals.qh, surface.id());
                // Olo: over another program's window (by the handle it
                // exported: a portal's dialog over the app asking), or
                // over ours.
                if let (Some(handle), Some(importer)) = (params.foreign_parent.as_deref().filter(|h| !h.is_empty()), &globals.xdg_importer) {
                    let i = importer.import_toplevel(handle.to_owned(), &globals.qh, ());
                    i.set_parent_of(&surface);
                    imported = Some(i);
                } else if params.kind == WindowKind::Floating || params.dialog.is_some() {
                    toplevel.set_parent(parent.as_ref());
                }
                if let Some(size) = params.window_min_size {
                    toplevel.set_min_size(size.width.0 as i32, size.height.0 as i32);
                }
                // Olo: a window of fixed size (an alert) is never resized.
                if !params.is_resizable {
                    let (w, h) = (params.bounds.size.width.0 as i32, params.bounds.size.height.0 as i32);
                    toplevel.set_min_size(w, h);
                    toplevel.set_max_size(w, h);
                }
                // Olo: a dialog, never a tab; an alert, modal.
                if let (Some(dialog), Some(wm)) = (params.dialog, &globals.wm_dialog) {
                    let d = wm.get_xdg_dialog(&toplevel, &globals.qh, ());
                    if dialog == crate::Dialog::Modal {
                        d.set_modal();
                    }
                    dialog_object = Some(d);
                }
                if let (Some(tags), Some(tag)) = (&globals.toplevel_tags, &params.toplevel_tag) {
                    tags.set_toplevel_tag(&toplevel, tag.clone());
                }
                (Some(toplevel), None)
            }
        };

        let grabbing = popup.is_some() && matches!(params.kind, WindowKind::Popup { .. });
        let made = Self::finish(handle, globals, gpu_context, client, params, appearance, loop_handle, signal, surface, Some(xdg_surface), None, toplevel, popup, (scale, displays))?;
        let mut state = made.0.0.state.borrow_mut();
        state.dialog = dialog_object;
        state.imported = imported;
        state.popup_of = popup_of;
        state.grabbing = grabbing;
        drop(state);
        Ok(made)
    }

    /// The rest of making a window, whatever its role.
    #[allow(clippy::too_many_arguments)]
    fn finish(
        handle: AnyWindowHandle,
        globals: Globals,
        gpu_context: &BladeContext,
        client: WaylandClientStatePtr,
        params: WindowParams,
        appearance: WindowAppearance,
        loop_handle: LoopHandle<'static, WaylandClientStatePtr>,
        signal: calloop::LoopSignal,
        surface: wl_surface::WlSurface,
        xdg_surface: Option<xdg_surface::XdgSurface>,
        layer_surface: Option<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1>,
        toplevel: Option<xdg_toplevel::XdgToplevel>,
        popup: Option<xdg_popup::XdgPopup>,
        (scale, displays): (Option<f32>, Vec<(ObjectId, Output)>),
    ) -> anyhow::Result<(Self, ObjectId)> {
        let lock_surface = params.kind == WindowKind::SessionLock;
        // Olo: the scale it's drawn at (a popup's, its window's), asked
        // before its first frame (else drawn at 1 first and again when the
        // scale came: 6–8 ms wasted opening a window, glyphs drawn for no
        // scale it's on), and its renderer made at it.
        let probe = match scale {
            Some(_) => None,
            None => globals
                .fractional_scale_manager
                .as_ref()
                .and_then(|manager| ScaleProbe::ask(manager, &surface)),
        };

        // Attempt to set up window decorations based on the requested configuration
        let decoration = toplevel.as_ref().and_then(|toplevel| {
            globals.decoration_manager.as_ref().map(|decoration_manager| {
                decoration_manager.get_toplevel_decoration(toplevel, &globals.qh, surface.id())
            })
        });

        let viewport = globals
            .viewporter
            .as_ref()
            .map(|viewporter| viewporter.get_viewport(&surface, &globals.qh, ()));

        // Kick things off, before the GPU work of the renderer, so the
        // compositor's first configure is on its way meanwhile (a lock
        // surface is configured without it, and mustn't commit before
        // acknowledging that).
        if !lock_surface {
            surface.commit();
            if let Some(backend) = surface.backend().upgrade() {
                let _ = backend.flush();
            }
        }

        // (The probe's answer is a roundtrip, 0.1–0.2 ms: had first, the
        // renderer is made at the scale.)
        let scale = scale.or_else(|| probe.and_then(ScaleProbe::answer));
        let mut state = WaylandWindowState::new(
            handle,
            surface.clone(),
            xdg_surface,
            layer_surface,
            toplevel,
            popup,
            decoration,
            appearance,
            viewport,
            client,
            globals,
            gpu_context,
            params,
            scale,
        )?;
        // (wl_output's scales are whole: a 1.5× display's is 2.)
        let at_scale = |(_, output): &(ObjectId, Output)| scale.is_none_or(|scale| output.scale == scale.ceil() as i32);
        state.may_open_on = if displays.iter().any(at_scale) { displays.into_iter().filter(at_scale).collect() } else { displays };
        let text_rendering = state.text_rendering();
        if let Some(fractional_scale_manager) = state.globals.fractional_scale_manager.as_ref() {
            state.fractional_scale = Some(fractional_scale_manager.get_fractional_scale(&surface, &state.globals.qh, surface.id()));
        }
        let this = Self(WaylandWindowStatePtr {
            state: Rc::new(RefCell::new(state)),
            callbacks: Rc::new(RefCell::new(Callbacks::default())),
            frames: Rc::new(FrameLoop {
                callback: Cell::new(FrameCallback::None),
                queued: Cell::new(false),
                needs_commit: Cell::new(false),
                committed_at: Cell::new(std::time::Instant::now()),
                force: Cell::new(false),
                text_rendering: Cell::new(text_rendering),
                text_generation: Cell::new(0),
                handle: loop_handle,
                signal,
                wheel: RefCell::new(None),
                closed: Cell::new(false),
                in_frame: Cell::new(false),
                wanted: Cell::new(false),
            }),
        });

        // (As any change of scale: its buffers and viewport for it.)
        if let Some(scale) = scale {
            this.0.rescale(scale);
        }
        Ok((this, surface.id()))
    }
}

impl WaylandWindowStatePtr {
    pub fn handle(&self) -> AnyWindowHandle {
        self.state.borrow().handle
    }

    pub fn surface(&self) -> wl_surface::WlSurface {
        self.state.borrow().surface.clone()
    }

    pub fn toplevel(&self) -> Option<xdg_toplevel::XdgToplevel> {
        self.state.borrow().toplevel.clone()
    }

    /// Olo: the scale it's drawn at and the display it's on (if it's on
    /// one yet): a popup's over it.
    pub fn scale_and_display(&self) -> (f32, Option<(ObjectId, Output)>) {
        let state = self.state.borrow();
        (state.scale, state.display.clone())
    }

    /// Olo: whether this is a popup open over `parent`'s surface.
    pub fn is_popup_over(&self, parent: &xdg_surface::XdgSurface) -> bool {
        // (The window going holds its own state: not over itself.)
        let Ok(state) = self.state.try_borrow() else { return false };
        state.popup.is_some() && state.popup_of.as_ref() == Some(parent)
    }

    /// Olo: its popup role taken down now (and those over it first), its
    /// window going later: a popup it opened over is going.
    pub fn withdraw_popup(&self, client: &WaylandClientStatePtr) {
        let Ok(state) = self.state.try_borrow() else { return };
        let (xdg, popup) = (state.xdg_surface.clone(), state.popup.clone());
        drop(state);
        if let Some(xdg) = &xdg {
            client.withdraw_popups_over(xdg);
        }
        if let Some(popup) = popup {
            // (Destroyed again with its window: nothing, once dead.)
            popup.destroy();
        }
    }

    /// Olo: its xdg surface (a window's or a popup's), and the popup it
    /// opened over, if it is one.
    pub fn xdg_surface(&self) -> Option<xdg_surface::XdgSurface> {
        self.state.try_borrow().ok()?.xdg_surface.clone()
    }

    pub fn popup_of(&self) -> Option<xdg_surface::XdgSurface> {
        self.state.try_borrow().ok()?.popup_of.clone()
    }

    /// Olo: a menu showing (a grabbing popup not yet taken down).
    pub fn is_grabbing_popup(&self) -> bool {
        self.state.try_borrow().is_ok_and(|s| s.grabbing && s.popup.as_ref().is_some_and(|p| p.is_alive()))
    }

    /// What a popup opens over, on this window.
    pub fn popup_parent(&self) -> Option<PopupParent> {
        let state = self.state.borrow();
        match (&state.xdg_surface, &state.layer_surface) {
            (Some(x), _) => Some(PopupParent::Xdg(x.clone())),
            (None, Some(l)) => Some(PopupParent::Layer(l.clone())),
            _ => None,
        }
    }

    /// Olo: a lock surface's size (its output's), to acknowledge and draw.
    pub fn handle_lock_surface_event(&self, event: ext_session_lock_surface_v1::Event) {
        if let ext_session_lock_surface_v1::Event::Configure { serial, width, height } = event {
            let to = size(px(width as f32), px(height as f32));
            if width > 0 && height > 0 && (to != self.state.borrow().bounds.size || !self.state.borrow().acknowledged_first_configure) {
                self.resize(to);
            }
            let mut state = self.state.borrow_mut();
            if let Some(lock_surface) = &state.lock_surface {
                lock_surface.ack_configure(serial);
            }
            let first = !state.acknowledged_first_configure;
            state.acknowledged_first_configure = true;
            drop(state);
            self.frames.needs_commit.set(true);
            if first {
                self.frame();
            } else {
                self.schedule_frame();
            }
        }
    }

    /// Olo: a layer surface's events: its size (0 along an axis it
    /// stretches across: the output's, ours to keep otherwise), or that
    /// it was closed (its output went): it closes.
    pub fn handle_layer_surface_event(&self, event: zwlr_layer_surface_v1::Event) -> bool {
        match event {
            zwlr_layer_surface_v1::Event::Configure { serial, width, height } => {
                let now = self.state.borrow().bounds.size;
                let to = size(
                    if width > 0 { px(width as f32) } else { now.width },
                    if height > 0 { px(height as f32) } else { now.height },
                );
                if to != now || !self.state.borrow().acknowledged_first_configure {
                    self.resize(to);
                }
                let mut state = self.state.borrow_mut();
                if let Some(layer) = &state.layer_surface {
                    layer.ack_configure(serial);
                }
                let first = !state.acknowledged_first_configure;
                state.acknowledged_first_configure = true;
                drop(state);
                self.frames.needs_commit.set(true);
                if first {
                    self.frame();
                } else {
                    self.schedule_frame();
                }
                false
            }
            zwlr_layer_surface_v1::Event::Closed => true,
            _ => false,
        }
    }

    /// Olo: a popup's events: where the compositor put it, or that it was
    /// dismissed (a click outside): it closes.
    pub fn handle_popup_event(&self, event: xdg_popup::Event) -> bool {
        match event {
            xdg_popup::Event::Configure { width, height, .. } => {
                if width > 0 && height > 0 {
                    let mut state = self.state.borrow_mut();
                    state.in_progress_configure = Some(InProgressConfigure {
                        size: Some(size(px(width as f32), px(height as f32))),
                        fullscreen: false,
                        maximized: false,
                        resizing: false,
                        tiling: Tiling::default(),
                    });
                }
                false
            }
            xdg_popup::Event::PopupDone => true,
            _ => false,
        }
    }

    pub fn ptr_eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.state, &other.state)
    }

    /// The compositor's frame callback came: the last frame is showing.
    pub fn frame_done(&self) {
        self.frames.callback.set(FrameCallback::None);
        self.frame();
    }

    /// Draws a frame if anything changed (and asks for the next).
    pub fn frame(&self) {
        let frames = &self.frames;
        // Nothing may be attached before the surface's first configure is
        // acknowledged (a protocol error that ends the program): a scale
        // the compositor sends a new surface first asks for a frame too
        // early. That configure draws it.
        if frames.closed.get() || !self.state.borrow().acknowledged_first_configure {
            frames.queued.set(false);
            return;
        }
        // (Still queued while the wheel's step goes in: what it changes
        // is drawn by this frame, not one more.)
        frames.queued.set(true);
        let mut state = self.state.borrow_mut();
        if frames.callback.get() == FrameCallback::None {
            state.surface.frame(&state.globals.qh, state.surface.id());
            frames.callback.set(FrameCallback::Asked);
        }
        state.resize_throttle = false;
        drop(state);
        let wheel = self.wheel_step();
        if let Some(step) = wheel.clone() {
            self.handle_input(PlatformInput::ScrollWheel(step));
        }
        frames.queued.set(false);

        let mut cb = self.callbacks.borrow_mut();
        let options = RequestFrameOptions {
            force_render: frames.force.take(),
            ..Default::default()
        };
        frames.in_frame.set(true);
        let drew = match cb.request_frame.as_mut() {
            Some(fun) => fun(options),
            None => false,
        };
        frames.in_frame.set(false);
        drop(cb);
        log::trace!(target: "olo_frames", "frame: drew {drew}, callback {:?}", frames.callback.get());
        if wheel.is_some() && !drew {
            // Nothing moved (an edge): the rest goes.
            frames.wheel.take();
        }
        if !drew && frames.needs_commit.get() {
            self.state.borrow().surface.commit();
        }
        if drew || frames.needs_commit.get() {
            frames.needs_commit.set(false);
            frames.callback.set(FrameCallback::Committed);
            frames.committed_at.set(std::time::Instant::now());
        }
        // Another frame asked for while drawing this one: with the next
        // callback (paced by the display) if this one committed, else now.
        if frames.wanted.take() {
            self.schedule_frame();
        }
    }

    /// Olo: a wheel's notches (`delta`, in points) to ease in over the
    /// frames to come, scrolling what is at `position`.
    pub fn add_wheel(&self, delta: Point<f32>, position: Point<Pixels>, modifiers: Modifiers) {
        let mut wheel = self.frames.wheel.borrow_mut();
        match wheel.as_mut() {
            Some(w) => {
                w.left = Point { x: w.left.x + delta.x, y: w.left.y + delta.y };
                w.position = position;
                w.modifiers = modifiers;
            }
            None => *wheel = Some(WheelEase { left: delta, position, modifiers, last: None }),
        }
        drop(wheel);
        self.schedule_frame();
    }

    /// This frame's share of the wheel's notches: what is left shrinks
    /// by the time since the last step (a frame's worth from a rest),
    /// and the last of it is under a device pixel rather than a jump.
    fn wheel_step(&self) -> Option<crate::ScrollWheelEvent> {
        let mut slot = self.frames.wheel.borrow_mut();
        let w = slot.as_mut()?;
        let now = std::time::Instant::now();
        let interval = self.frame_interval();
        let dt = w.last.map_or(interval, |t| (now - t).as_secs_f64().min(4.0 * interval));
        w.last = Some(now);
        let k = (1.0 - (-dt / WHEEL_EASE).exp()) as f32;
        let mut step = Point { x: w.left.x * k, y: w.left.y * k };
        // Less than half a device pixel would be left: no more frames
        // for moves that don't show.
        let half_pixel = 0.5 / self.state.borrow().scale;
        if (w.left.x - step.x).hypot(w.left.y - step.y) < half_pixel {
            step = w.left;
        }
        w.left = Point { x: w.left.x - step.x, y: w.left.y - step.y };
        log::trace!(target: "olo_frames", "wheel: step {:.2} after {:.1} ms, {:.2} left", step.y, dt * 1000.0, w.left.y);
        let event = crate::ScrollWheelEvent {
            position: w.position,
            delta: crate::ScrollDelta::Pixels(Point { x: px(step.x), y: px(step.y) }),
            modifiers: w.modifiers,
            touch_phase: crate::TouchPhase::Moved,
            kind: crate::ScrollKind::Wheel,
        };
        if w.left.x == 0.0 && w.left.y == 0.0 {
            *slot = None;
        }
        Some(event)
    }

    /// How long a frame shows on the window's display (60 Hz until it
    /// says).
    fn frame_interval(&self) -> f64 {
        let mhz = self.state.borrow().display.as_ref().map_or(0, |(_, o)| o.refresh_mhz);
        if mhz >= 10_000 { 1000.0 / f64::from(mhz) } else { 1.0 / 60.0 }
    }

    /// Something changed: draw it now if idle, else with the next frame.
    pub fn schedule_frame(&self) {
        let frames = &self.frames;
        log::trace!(target: "olo_frames", "schedule: queued {}, callback {:?}", frames.queued.get(), frames.callback.get());
        if frames.queued.get() || frames.closed.get() {
            return;
        }
        if frames.in_frame.get() {
            frames.wanted.set(true);
            return;
        }
        if frames.callback.get() == FrameCallback::Committed {
            if frames.committed_at.get().elapsed() < std::time::Duration::from_secs(1) {
                return;
            }
            frames.callback.set(FrameCallback::Asked);
        }
        frames.queued.set(true);
        let this = self.downgrade();
        frames.handle.insert_idle(move |_| {
            if let Some(this) = this.upgrade() {
                if this.frames.queued.get() {
                    this.frame();
                }
            }
        });
        frames.signal.wakeup();
    }

    fn downgrade(&self) -> WeakWindowPtr {
        WeakWindowPtr {
            state: Rc::downgrade(&self.state),
            callbacks: Rc::downgrade(&self.callbacks),
            frames: Rc::downgrade(&self.frames),
        }
    }

    pub fn handle_xdg_surface_event(&self, event: xdg_surface::Event) {
        if let xdg_surface::Event::Configure { serial } = event {
            {
                let mut state = self.state.borrow_mut();
                if let Some(window_controls) = state.in_progress_window_controls.take() {
                    state.window_controls = window_controls;

                    drop(state);
                    let mut callbacks = self.callbacks.borrow_mut();
                    if let Some(appearance_changed) = callbacks.appearance_changed.as_mut() {
                        appearance_changed();
                    }
                }
            }
            {
                let mut state = self.state.borrow_mut();

                if let Some(mut configure) = state.in_progress_configure.take() {
                    let got_unmaximized = state.maximized && !configure.maximized;
                    state.fullscreen = configure.fullscreen;
                    state.maximized = configure.maximized;
                    state.tiling = configure.tiling;
                    // Olo: a resize by hand starts (the press, before
                    // anything moves): room to grow made now, so the
                    // sizes it goes through cost nothing; it ends: made
                    // to fit once the size has rested.
                    if configure.resizing != state.resizing {
                        state.resizing = configure.resizing;
                        if configure.resizing {
                            if state.viewport.is_some() {
                                let size = state.bounds.to_device_pixels(state.scale).size;
                                let room = resize_room(&state, size);
                                state.renderer.update_drawable_size_within(size, room);
                            }
                        } else {
                            drop(state);
                            self.snug_later();
                            state = self.state.borrow_mut();
                        }
                    }
                    // Limit interactive resizes to once per vblank
                    if configure.resizing && state.resize_throttle {
                        return;
                    } else if configure.resizing {
                        state.resize_throttle = true;
                    }
                    if !configure.fullscreen && !configure.maximized {
                        configure.size = if got_unmaximized {
                            Some(state.window_bounds.size)
                        } else {
                            compute_outer_size(state.inset(), configure.size, state.tiling)
                        };
                        if let Some(size) = configure.size {
                            state.window_bounds = Bounds {
                                origin: Point::default(),
                                size,
                            };
                        }
                    }
                    drop(state);
                    if let Some(size) = configure.size {
                        self.resize(size);
                    }
                }
            }
            let mut state = self.state.borrow_mut();
            let Some(xdg_surface) = state.xdg_surface.clone() else { return };
            xdg_surface.ack_configure(serial);

            let window_geometry = inset_by_tiling(
                state.bounds.map_origin(|_| px(0.0)),
                state.inset(),
                state.tiling,
            )
            .map(|v| v.0 as i32)
            .map_size(|v| if v <= 0 { 1 } else { v });

            xdg_surface.set_window_geometry(
                window_geometry.origin.x,
                window_geometry.origin.y,
                window_geometry.size.width,
                window_geometry.size.height,
            );

            let request_frame_callback = !state.acknowledged_first_configure;
            state.acknowledged_first_configure = true;
            drop(state);
            // (What was acknowledged applies with the next commit.)
            self.frames.needs_commit.set(true);
            if request_frame_callback {
                self.frame();
            } else {
                self.schedule_frame();
            }
        }
    }

    /// One of the displays changed (its scale, its subpixel layout).
    pub fn output_changed(&self, id: ObjectId, output: &Output) {
        let mut state = self.state.borrow_mut();
        if let Some(known) = state.outputs.get_mut(&id) {
            *known = output.clone();
            // (The display it's drawn for is a copy: kept up too.)
            if let Some((shown, data)) = state.display.as_mut()
                && *shown == id
            {
                *data = output.clone();
            }
            state.primary_output_scale();
            drop(state);
            self.update_text_rendering();
        }
    }

    /// Chooses how the window's text is drawn, for the display it's on
    /// (the one with the highest scale, which its buffers are for), and
    /// draws it again if that changed.
    pub fn update_text_rendering(&self) {
        let rendering = self.state.borrow().text_rendering();
        if rendering != self.frames.text_rendering.replace(rendering) {
            self.frames.force.set(true);
            self.schedule_frame();
        }
    }

    pub fn handle_toplevel_decoration_event(&self, event: zxdg_toplevel_decoration_v1::Event) {
        if let zxdg_toplevel_decoration_v1::Event::Configure { mode } = event {
            match mode {
                WEnum::Value(zxdg_toplevel_decoration_v1::Mode::ServerSide) => {
                    self.state.borrow_mut().decorations = WindowDecorations::Server;
                    self.update_text_rendering();
                    if let Some(mut appearance_changed) =
                        self.callbacks.borrow_mut().appearance_changed.as_mut()
                    {
                        appearance_changed();
                    }
                }
                WEnum::Value(zxdg_toplevel_decoration_v1::Mode::ClientSide) => {
                    self.state.borrow_mut().decorations = WindowDecorations::Client;
                    self.update_text_rendering();
                    // Update background to be transparent
                    if let Some(mut appearance_changed) =
                        self.callbacks.borrow_mut().appearance_changed.as_mut()
                    {
                        appearance_changed();
                    }
                }
                WEnum::Value(_) => {
                    log::warn!("Unknown decoration mode");
                }
                WEnum::Unknown(v) => {
                    log::warn!("Unknown decoration mode: {}", v);
                }
            }
        }
    }

    pub fn handle_fractional_scale_event(&self, event: wp_fractional_scale_v1::Event) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event {
            self.rescale(scale as f32 / 120.0);
        }
    }

    pub fn handle_toplevel_event(&self, event: xdg_toplevel::Event) -> bool {
        match event {
            xdg_toplevel::Event::Configure {
                width,
                height,
                states,
            } => {
                let mut size = if width == 0 || height == 0 {
                    None
                } else {
                    Some(size(px(width as f32), px(height as f32)))
                };

                let states = extract_states::<xdg_toplevel::State>(&states);

                let mut tiling = Tiling::default();
                let mut fullscreen = false;
                let mut maximized = false;
                let mut resizing = false;

                for state in states {
                    match state {
                        xdg_toplevel::State::Maximized => {
                            maximized = true;
                        }
                        xdg_toplevel::State::Fullscreen => {
                            fullscreen = true;
                        }
                        xdg_toplevel::State::Resizing => resizing = true,
                        xdg_toplevel::State::TiledTop => {
                            tiling.top = true;
                        }
                        xdg_toplevel::State::TiledLeft => {
                            tiling.left = true;
                        }
                        xdg_toplevel::State::TiledRight => {
                            tiling.right = true;
                        }
                        xdg_toplevel::State::TiledBottom => {
                            tiling.bottom = true;
                        }
                        _ => {
                            // noop
                        }
                    }
                }

                if fullscreen || maximized {
                    tiling = Tiling::tiled();
                }

                let mut state = self.state.borrow_mut();
                state.in_progress_configure = Some(InProgressConfigure {
                    size,
                    fullscreen,
                    maximized,
                    resizing,
                    tiling,
                });

                false
            }
            xdg_toplevel::Event::Close => {
                let mut cb = self.callbacks.borrow_mut();
                if let Some(mut should_close) = cb.should_close.take() {
                    let result = (should_close)();
                    cb.should_close = Some(should_close);
                    if result {
                        drop(cb);
                        self.close();
                    }
                    result
                } else {
                    true
                }
            }
            xdg_toplevel::Event::WmCapabilities { capabilities } => {
                let mut window_controls = WindowControls::default();

                let states = extract_states::<xdg_toplevel::WmCapabilities>(&capabilities);

                for state in states {
                    match state {
                        xdg_toplevel::WmCapabilities::Maximize => {
                            window_controls.maximize = true;
                        }
                        xdg_toplevel::WmCapabilities::Minimize => {
                            window_controls.minimize = true;
                        }
                        xdg_toplevel::WmCapabilities::Fullscreen => {
                            window_controls.fullscreen = true;
                        }
                        xdg_toplevel::WmCapabilities::WindowMenu => {
                            window_controls.window_menu = true;
                        }
                        _ => {}
                    }
                }

                let mut state = self.state.borrow_mut();
                state.in_progress_window_controls = Some(window_controls);
                false
            }
            _ => false,
        }
    }

    #[allow(clippy::mutable_key_type)]
    pub fn handle_surface_event(
        &self,
        event: wl_surface::Event,
        outputs: HashMap<ObjectId, Output>,
    ) {
        let mut state = self.state.borrow_mut();

        match event {
            wl_surface::Event::Enter { output } => {
                let id = output.id();

                let Some(output) = outputs.get(&id) else {
                    return;
                };

                state.outputs.insert(id, output.clone());

                let scale = state.primary_output_scale();
                drop(state);
                self.update_text_rendering();
                let state = self.state.borrow_mut();

                // We use `PreferredBufferScale` instead to set the scale if it's available
                // (Olo: and the fractional scale before either: on a
                // wl_compositor before version 6, as app_server's, entering
                // a 1.5x display set 2 over 1.5, and app_server shrank the
                // 2x buffers to fit, every line smeared over two pixels).
                if state.globals.fractional_scale_manager.is_none() && state.surface.version() < wl_surface::EVT_PREFERRED_BUFFER_SCALE_SINCE {
                    state.surface.set_buffer_scale(scale);
                    drop(state);
                    self.rescale(scale as f32);
                }
            }
            wl_surface::Event::Leave { output } => {
                state.outputs.remove(&output.id());

                let scale = state.primary_output_scale();
                drop(state);
                self.update_text_rendering();
                let state = self.state.borrow_mut();

                // We use `PreferredBufferScale` instead to set the scale if it's available
                // (Olo: and the fractional scale before either: on a
                // wl_compositor before version 6, as app_server's, entering
                // a 1.5x display set 2 over 1.5, and app_server shrank the
                // 2x buffers to fit, every line smeared over two pixels).
                if state.globals.fractional_scale_manager.is_none() && state.surface.version() < wl_surface::EVT_PREFERRED_BUFFER_SCALE_SINCE {
                    state.surface.set_buffer_scale(scale);
                    drop(state);
                    self.rescale(scale as f32);
                }
            }
            wl_surface::Event::PreferredBufferScale { factor } => {
                // We use `WpFractionalScale` instead to set the scale if it's available
                if state.globals.fractional_scale_manager.is_none() {
                    state.surface.set_buffer_scale(factor);
                    drop(state);
                    self.rescale(factor as f32);
                }
            }
            _ => {}
        }
    }

    pub fn handle_ime(&self, ime: ImeInput) {
        let mut state = self.state.borrow_mut();
        if let Some(mut input_handler) = state.input_handler.take() {
            drop(state);
            match ime {
                ImeInput::InsertText(text) => {
                    input_handler.replace_text_in_range(None, &text);
                }
                ImeInput::SetMarkedText(text) => {
                    input_handler.replace_and_mark_text_in_range(None, &text, None);
                }
                ImeInput::UnmarkText => {
                    input_handler.unmark_text();
                }
                ImeInput::DeleteText => {
                    if let Some(marked) = input_handler.marked_text_range() {
                        input_handler.replace_text_in_range(Some(marked), "");
                    }
                }
            }
            self.state.borrow_mut().input_handler = Some(input_handler);
        }
    }

    pub fn get_ime_area(&self) -> Option<Bounds<Pixels>> {
        let mut state = self.state.borrow_mut();
        let mut bounds: Option<Bounds<Pixels>> = None;
        if let Some(mut input_handler) = state.input_handler.take() {
            drop(state);
            if let Some(selection) = input_handler.marked_text_range() {
                bounds = input_handler.bounds_for_range(selection.start..selection.start);
            }
            self.state.borrow_mut().input_handler = Some(input_handler);
        }
        bounds
    }

    pub fn set_size_and_scale(&self, size: Option<Size<Pixels>>, scale: Option<f32>) {
        // Olo: a resize queued for a window since closed (Deskbar's bars
        // as the displays went with a restarting app_server) would make
        // a swapchain for a surface that is gone: libwayland segfaulted
        // in wl_proxy_create_wrapper (2026-11-01).
        if self.frames.closed.get() {
            return;
        }
        let (size, scale) = {
            let mut state = self.state.borrow_mut();
            if size.is_none_or(|size| size == state.bounds.size)
                && scale.is_none_or(|scale| scale == state.scale)
            {
                return;
            }
            if let Some(size) = size {
                state.bounds.size = size;
            }
            if let Some(scale) = scale {
                state.scale = scale;
            }
            let device_bounds = state.bounds.to_device_pixels(state.scale);
            // (Resized by hand, within its room; else just as big.)
            if state.resizing && state.viewport.is_some() && scale.is_none() {
                let room = resize_room(&state, device_bounds.size);
                state.renderer.update_drawable_size_within(device_bounds.size, room);
            } else {
                state.renderer.update_drawable_size(device_bounds.size);
            }
            (state.bounds.size, state.scale)
        };

        if let Some(ref mut fun) = self.callbacks.borrow_mut().resize {
            fun(size, scale);
        }

        {
            let state = self.state.borrow();
            if let Some(viewport) = &state.viewport {
                viewport.set_destination(size.width.0 as i32, size.height.0 as i32);
            }
        }
    }

    pub fn resize(&self, size: Size<Pixels>) {
        self.set_size_and_scale(Some(size), None);
    }

    /// Olo: a resize by hand ended: once the size has rested, the
    /// swapchain is made to fit (its room is GPU memory: three screens'
    /// worth for a window grown to its screen's size and back).
    fn snug_later(&self) {
        let this = self.downgrade();
        let _ = self.frames.handle.insert_source(calloop::timer::Timer::from_duration(SNUG_AFTER), move |_, _, _| {
            if let Some(this) = this.upgrade() {
                this.snug();
            }
            calloop::timer::TimeoutAction::Drop
        });
    }

    fn snug(&self) {
        if self.frames.closed.get() {
            return;
        }
        {
            let mut state = self.state.borrow_mut();
            if state.resizing || !state.renderer.has_room() {
                return;
            }
            let size = state.renderer.drawn_size();
            state.renderer.update_drawable_size(size);
        }
        // (Its next frame goes in the new one.)
        self.frames.force.set(true);
        self.schedule_frame();
    }

    pub fn rescale(&self, scale: f32) {
        self.set_size_and_scale(None, Some(scale));
    }

    pub fn close(&self) {
        let mut callbacks = self.callbacks.borrow_mut();
        if let Some(fun) = callbacks.close.take() {
            fun()
        }
    }

    pub fn handle_input(&self, input: PlatformInput) {
        if let Some(ref mut fun) = self.callbacks.borrow_mut().input
            && !fun(input.clone()).propagate
        {
            return;
        }
        if let PlatformInput::KeyDown(event) = input
            && event.keystroke.modifiers.is_subset_of(&Modifiers::shift())
            && let Some(key_char) = &event.keystroke.key_char
        {
            let mut state = self.state.borrow_mut();
            if let Some(mut input_handler) = state.input_handler.take() {
                drop(state);
                input_handler.replace_text_in_range(None, key_char);
                self.state.borrow_mut().input_handler = Some(input_handler);
            }
        }
    }

    pub fn set_focused(&self, focus: bool) {
        self.state.borrow_mut().active = focus;
        if let Some(ref mut fun) = self.callbacks.borrow_mut().active_status_change {
            fun(focus);
        }
    }

    pub fn set_hovered(&self, focus: bool) {
        if let Some(ref mut fun) = self.callbacks.borrow_mut().hover_status_change {
            fun(focus);
        }
    }

    pub fn set_appearance(&mut self, appearance: WindowAppearance) {
        self.state.borrow_mut().appearance = appearance;

        let mut callbacks = self.callbacks.borrow_mut();
        if let Some(ref mut fun) = callbacks.appearance_changed {
            (fun)()
        }
    }

    pub fn primary_output_scale(&self) -> i32 {
        self.state.borrow_mut().primary_output_scale()
    }
}

fn extract_states<'a, S: TryFrom<u32> + 'a>(states: &'a [u8]) -> impl Iterator<Item = S> + 'a
where
    <S as TryFrom<u32>>::Error: 'a,
{
    states
        .chunks_exact(4)
        .flat_map(TryInto::<[u8; 4]>::try_into)
        .map(u32::from_ne_bytes)
        .flat_map(S::try_from)
}

impl rwh::HasWindowHandle for WaylandWindow {
    fn window_handle(&self) -> Result<rwh::WindowHandle<'_>, rwh::HandleError> {
        let surface = self.0.surface().id().as_ptr() as *mut libc::c_void;
        let c_ptr = NonNull::new(surface).ok_or(rwh::HandleError::Unavailable)?;
        let handle = rwh::WaylandWindowHandle::new(c_ptr);
        let raw_handle = rwh::RawWindowHandle::Wayland(handle);
        Ok(unsafe { rwh::WindowHandle::borrow_raw(raw_handle) })
    }
}

impl rwh::HasDisplayHandle for WaylandWindow {
    fn display_handle(&self) -> Result<rwh::DisplayHandle<'_>, rwh::HandleError> {
        let display = self
            .0
            .surface()
            .backend()
            .upgrade()
            .ok_or(rwh::HandleError::Unavailable)?
            .display_ptr() as *mut libc::c_void;

        let c_ptr = NonNull::new(display).ok_or(rwh::HandleError::Unavailable)?;
        let handle = rwh::WaylandDisplayHandle::new(c_ptr);
        let raw_handle = rwh::RawDisplayHandle::Wayland(handle);
        Ok(unsafe { rwh::DisplayHandle::borrow_raw(raw_handle) })
    }
}

impl PlatformWindow for WaylandWindow {
    fn bounds(&self) -> Bounds<Pixels> {
        self.borrow().bounds
    }

    fn is_maximized(&self) -> bool {
        self.borrow().maximized
    }

    fn window_bounds(&self) -> WindowBounds {
        let state = self.borrow();
        if state.fullscreen {
            WindowBounds::Fullscreen(state.window_bounds)
        } else if state.maximized {
            WindowBounds::Maximized(state.window_bounds)
        } else {
            drop(state);
            WindowBounds::Windowed(self.bounds())
        }
    }

    fn inner_window_bounds(&self) -> WindowBounds {
        let state = self.borrow();
        if state.fullscreen {
            WindowBounds::Fullscreen(state.window_bounds)
        } else if state.maximized {
            WindowBounds::Maximized(state.window_bounds)
        } else {
            let inset = state.inset();
            drop(state);
            WindowBounds::Windowed(self.bounds().inset(inset))
        }
    }

    fn content_size(&self) -> Size<Pixels> {
        self.borrow().bounds.size
    }

    fn resize(&mut self, size: Size<Pixels>) {
        let state = self.borrow();
        let state_ptr = self.0.clone();
        let dp_size = size.to_device_pixels(self.scale_factor());

        if let Some(xdg_surface) = &state.xdg_surface {
            xdg_surface.set_window_geometry(
                state.bounds.origin.x.0 as i32,
                state.bounds.origin.y.0 as i32,
                dp_size.width.0,
                dp_size.height.0,
            );
        }
        // Olo: a layer surface asks for its new size.
        if let (Some(layer), Some(options)) = (&state.layer_surface, &state.layer_shell) {
            place_layer(layer, options, size);
            state.surface.commit();
        }

        state
            .globals
            .executor
            .spawn(async move { state_ptr.resize(size) })
            .detach();
    }

    fn scale_factor(&self) -> f32 {
        self.borrow().scale
    }

    fn appearance(&self) -> WindowAppearance {
        self.borrow().appearance
    }

    fn display(&self) -> Option<Rc<dyn PlatformDisplay>> {
        let state = self.borrow();
        // Olo: before it's on one, the one it will open on, if that's
        // known (the only display; a popup's window's).
        let only = || match state.may_open_on.as_slice() {
            [one] => Some(one),
            _ => None,
        };
        state.display.as_ref().or_else(only).map(|(id, display)| {
            Rc::new(WaylandDisplay {
                id: id.clone(),
                name: display.name.clone(),
                bounds: if display.logical.is_some() { display.bounds_in_points() } else { display.bounds.to_pixels(state.scale) },
            }) as Rc<dyn PlatformDisplay>
        })
    }

    fn mouse_position(&self) -> Point<Pixels> {
        self.borrow()
            .client
            .get_client()
            .borrow()
            .mouse_location
            .unwrap_or_default()
    }

    fn modifiers(&self) -> Modifiers {
        self.borrow().client.get_client().borrow().modifiers
    }

    fn capslock(&self) -> Capslock {
        self.borrow().client.get_client().borrow().capslock
    }

    fn set_input_handler(&mut self, input_handler: PlatformInputHandler) {
        self.borrow_mut().input_handler = Some(input_handler);
    }

    fn take_input_handler(&mut self) -> Option<PlatformInputHandler> {
        self.borrow_mut().input_handler.take()
    }

    fn prompt(
        &self,
        _level: PromptLevel,
        _msg: &str,
        _detail: Option<&str>,
        _answers: &[PromptButton],
    ) -> Option<Receiver<usize>> {
        None
    }

    fn activate(&self) {
        // Try to request an activation token. Even though the activation is likely going to be rejected,
        // KWin and Mutter can use the app_id to visually indicate we're requesting attention.
        let state = self.borrow();
        if let (Some(activation), Some(app_id)) = (&state.globals.activation, state.app_id.clone())
        {
            state.client.set_pending_activation(state.surface.id());
            let token = activation.get_activation_token(&state.globals.qh, ());
            // The serial isn't exactly important here, since the activation is probably going to be rejected anyway.
            let serial = state.client.get_serial(SerialKind::MousePress);
            token.set_app_id(app_id);
            token.set_serial(serial, &state.globals.seat);
            token.set_surface(&state.surface);
            token.commit();
        }
    }

    fn activate_with_token(&self, token: &str) {
        let state = self.borrow();
        match &state.globals.activation {
            Some(activation) => activation.activate(token.to_owned(), &state.surface),
            None => {
                drop(state);
                self.activate();
            }
        }
    }

    fn is_active(&self) -> bool {
        self.borrow().active
    }

    fn is_hovered(&self) -> bool {
        self.borrow().hovered
    }

    fn set_title(&mut self, title: &str) {
        if let Some(toplevel) = &self.borrow().toplevel {
            toplevel.set_title(title.to_string());
        }
    }

    fn set_app_id(&mut self, app_id: &str) {
        let mut state = self.borrow_mut();
        if let Some(toplevel) = &state.toplevel {
            toplevel.set_app_id(app_id.to_owned());
        }
        state.app_id = Some(app_id.to_owned());
    }

    fn activation_token(&self, done: Box<dyn FnOnce(Option<String>)>) {
        let state = self.borrow();
        let (client, surface, app_id) = (state.client.clone(), state.surface.clone(), state.app_id.clone());
        drop(state);
        client.request_activation_token(&surface, app_id, done);
    }

    fn set_layer_keyboard(&self, keyboard: crate::LayerKeyboard) {
        let state = self.borrow();
        if let Some(layer) = &state.layer_surface {
            layer.set_keyboard_interactivity(layer_keyboard(layer, keyboard));
            state.surface.commit();
        }
    }

    fn set_drop_accept(&self, accept: Option<crate::DropAccept>) {
        let (client, surface) = {
            let state = self.borrow();
            (state.client.clone(), state.surface.id())
        };
        client.set_drop_accept(&surface, accept);
    }

    fn set_keep_awake(&self, on: bool) {
        let mut state = self.borrow_mut();
        match (on, state.inhibitor.take()) {
            (true, Some(i)) => state.inhibitor = Some(i),
            (true, None) => {
                let made = state.globals.idle_inhibit_manager.as_ref().map(|m| m.create_inhibitor(&state.surface, &state.globals.qh, ()));
                state.inhibitor = made;
            }
            (false, Some(i)) => i.destroy(),
            (false, None) => {}
        }
    }

    fn set_background_appearance(&self, background_appearance: WindowBackgroundAppearance) {
        let mut state = self.borrow_mut();
        state.background_appearance = background_appearance;
        update_window(state);
    }

    fn minimize(&self) {
        if let Some(toplevel) = &self.borrow().toplevel {
            toplevel.set_minimized();
        }
    }

    fn zoom(&self) {
        let state = self.borrow();
        if !state.maximized {
            if let Some(toplevel) = &state.toplevel {
                toplevel.set_maximized();
            }
        } else {
            if let Some(toplevel) = &state.toplevel {
                toplevel.unset_maximized();
            }
        }
    }

    fn toggle_fullscreen(&self) {
        let mut state = self.borrow_mut();
        if !state.fullscreen {
            if let Some(toplevel) = &state.toplevel {
                toplevel.set_fullscreen(None);
            }
        } else {
            if let Some(toplevel) = &state.toplevel {
                toplevel.unset_fullscreen();
            }
        }
    }

    fn is_fullscreen(&self) -> bool {
        self.borrow().fullscreen
    }

    fn on_request_frame(&self, callback: Box<dyn FnMut(RequestFrameOptions) -> bool>) {
        self.0.callbacks.borrow_mut().request_frame = Some(callback);
    }

    fn text_rendering(&self) -> crate::TextRendering {
        let generation = crate::text_system::DISPLAY_TEXT_RENDERING_GENERATION
            .load(std::sync::atomic::Ordering::Relaxed);
        if self.0.frames.text_generation.replace(generation) != generation {
            self.0.update_text_rendering();
        }
        self.0.frames.text_rendering.get()
    }

    fn draw_now(&self) {
        let frames = &self.0.frames;
        if frames.callback.get() == FrameCallback::Committed {
            frames.callback.set(FrameCallback::Asked);
        }
        frames.queued.set(false);
        self.0.schedule_frame();
    }

    fn frame_waker(&self) -> Option<Rc<dyn Fn()>> {
        let this = self.0.downgrade();
        Some(Rc::new(move || {
            if let Some(this) = this.upgrade() {
                this.schedule_frame();
            }
        }))
    }

    fn on_input(&self, callback: Box<dyn FnMut(PlatformInput) -> crate::DispatchEventResult>) {
        self.0.callbacks.borrow_mut().input = Some(callback);
    }

    fn on_active_status_change(&self, callback: Box<dyn FnMut(bool)>) {
        self.0.callbacks.borrow_mut().active_status_change = Some(callback);
    }

    fn on_hover_status_change(&self, callback: Box<dyn FnMut(bool)>) {
        self.0.callbacks.borrow_mut().hover_status_change = Some(callback);
    }

    fn on_resize(&self, callback: Box<dyn FnMut(Size<Pixels>, f32)>) {
        self.0.callbacks.borrow_mut().resize = Some(callback);
    }

    fn on_moved(&self, callback: Box<dyn FnMut()>) {
        self.0.callbacks.borrow_mut().moved = Some(callback);
    }

    fn on_should_close(&self, callback: Box<dyn FnMut() -> bool>) {
        self.0.callbacks.borrow_mut().should_close = Some(callback);
    }

    fn on_close(&self, callback: Box<dyn FnOnce()>) {
        self.0.callbacks.borrow_mut().close = Some(callback);
    }

    fn on_hit_test_window_control(&self, _callback: Box<dyn FnMut() -> Option<WindowControlArea>>) {
    }

    fn on_appearance_changed(&self, callback: Box<dyn FnMut()>) {
        self.0.callbacks.borrow_mut().appearance_changed = Some(callback);
    }

    fn draw(&self, scene: &Scene) {
        // Olo: nothing may be attached before the surface's first configure
        // is acknowledged, whatever asked for the frame (the first draw at
        // a window's opening comes before the configure of a layer
        // surface on a compositor still finding its displays, as
        // Deskbar's did reconnecting after an upgrade: a protocol error
        // that ended it, 2026-11-01). The configure draws it.
        if self.0.frames.closed.get() || !self.0.state.borrow().acknowledged_first_configure {
            log::debug!("a frame before the first configure: not drawn");
            return;
        }
        let mut state = self.borrow_mut();
        // Olo: the part of the buffer drawn in (all of it, or less while
        // it has room from a resize), shown one buffer pixel to a device
        // pixel; set as the frame goes (the present commits it).
        let drawn = state.renderer.drawn_size();
        if state.viewport_source != Some(drawn)
            && let Some(viewport) = &state.viewport
        {
            viewport.set_source(0.0, 0.0, drawn.width.0 as f64, drawn.height.0 as f64);
            state.viewport_source = Some(drawn);
        }
        state.renderer.draw(scene);
    }

    fn completed_frame(&self) {
        // (The frame's present committed it.)
    }

    fn sprite_atlas(&self) -> Arc<dyn PlatformAtlas> {
        let state = self.borrow();
        state.renderer.sprite_atlas().clone()
    }

    fn show_window_menu(&self, position: Point<Pixels>) {
        let state = self.borrow();
        let serial = state.client.get_serial(SerialKind::MousePress);
        if let Some(toplevel) = &state.toplevel {
            toplevel.show_window_menu(
                &state.globals.seat,
                serial,
                position.x.0 as i32,
                position.y.0 as i32,
            );
        }
    }

    fn start_window_move(&self) {
        let state = self.borrow();
        let serial = state.client.get_serial(SerialKind::MousePress);
        if let Some(toplevel) = &state.toplevel {
            toplevel._move(&state.globals.seat, serial);
        }
    }

    fn start_drag(&self, drag: crate::OutgoingDrag, done: futures::channel::oneshot::Sender<Option<crate::DragAction>>) {
        let (client, surface) = {
            let state = self.borrow();
            (state.client.clone(), state.surface.clone())
        };
        client.start_drag(&surface, drag, done);
    }

    fn surface_id(&self) -> Option<u32> {
        Some(self.borrow().surface.id().protocol_id())
    }

    fn start_window_resize(&self, edge: crate::ResizeEdge) {
        let state = self.borrow();
        if let Some(toplevel) = &state.toplevel {
            toplevel.resize(
                &state.globals.seat,
                state.client.get_serial(SerialKind::MousePress),
                edge.to_xdg(),
            )
        }
    }

    fn window_decorations(&self) -> Decorations {
        let state = self.borrow();
        match state.decorations {
            WindowDecorations::Server => Decorations::Server,
            WindowDecorations::Client => Decorations::Client {
                tiling: state.tiling,
            },
        }
    }

    fn request_decorations(&self, decorations: WindowDecorations) {
        let mut state = self.borrow_mut();
        state.decorations = decorations;
        if let Some(decoration) = state.decoration.as_ref() {
            decoration.set_mode(decorations.to_xdg());
            update_window(state);
        }
    }

    fn window_controls(&self) -> WindowControls {
        self.borrow().window_controls
    }

    fn set_client_inset(&self, inset: Pixels) {
        let mut state = self.borrow_mut();
        if Some(inset) != state.client_inset {
            state.client_inset = Some(inset);
            update_window(state);
        }
    }

    fn update_ime_position(&self, bounds: Bounds<Pixels>) {
        let state = self.borrow();
        state.client.update_ime_position(bounds);
    }

    fn gpu_specs(&self) -> Option<GpuSpecs> {
        self.borrow().renderer.gpu_specs().into()
    }
}

fn update_window(mut state: RefMut<WaylandWindowState>) {
    let opaque = !state.is_transparent();

    state.renderer.update_transparency(!opaque);
    let mut opaque_area = state.window_bounds.map(|v| v.0 as i32);
    opaque_area.inset(state.inset().0 as i32);

    let region = state
        .globals
        .compositor
        .create_region(&state.globals.qh, ());
    region.add(
        opaque_area.origin.x,
        opaque_area.origin.y,
        opaque_area.size.width,
        opaque_area.size.height,
    );

    // Note that rounded corners make this rectangle API hard to work with.
    // As this is common when using CSD, let's just disable this API.
    if state.background_appearance == WindowBackgroundAppearance::Opaque
        && state.decorations == WindowDecorations::Server
    {
        // Promise the compositor that this region of the window surface
        // contains no transparent pixels. This allows the compositor to skip
        // updating whatever is behind the surface for better performance.
        state.surface.set_opaque_region(Some(&region));
    } else {
        state.surface.set_opaque_region(None);
    }

    if let Some(ref blur_manager) = state.globals.blur_manager {
        if state.background_appearance == WindowBackgroundAppearance::Blurred {
            if state.blur.is_none() {
                let blur = blur_manager.create(&state.surface, &state.globals.qh, ());
                state.blur = Some(blur);
            }
            state.blur.as_ref().unwrap().commit();
        } else {
            // It probably doesn't hurt to clear the blur for opaque windows
            blur_manager.unset(&state.surface);
            if let Some(b) = state.blur.take() {
                b.release()
            }
        }
    }

    region.destroy();
}

impl WindowDecorations {
    fn to_xdg(self) -> zxdg_toplevel_decoration_v1::Mode {
        match self {
            WindowDecorations::Client => zxdg_toplevel_decoration_v1::Mode::ClientSide,
            WindowDecorations::Server => zxdg_toplevel_decoration_v1::Mode::ServerSide,
        }
    }
}

impl ResizeEdge {
    fn to_xdg(self) -> xdg_toplevel::ResizeEdge {
        match self {
            ResizeEdge::Top => xdg_toplevel::ResizeEdge::Top,
            ResizeEdge::TopRight => xdg_toplevel::ResizeEdge::TopRight,
            ResizeEdge::Right => xdg_toplevel::ResizeEdge::Right,
            ResizeEdge::BottomRight => xdg_toplevel::ResizeEdge::BottomRight,
            ResizeEdge::Bottom => xdg_toplevel::ResizeEdge::Bottom,
            ResizeEdge::BottomLeft => xdg_toplevel::ResizeEdge::BottomLeft,
            ResizeEdge::Left => xdg_toplevel::ResizeEdge::Left,
            ResizeEdge::TopLeft => xdg_toplevel::ResizeEdge::TopLeft,
        }
    }
}

/// The configuration event is in terms of the window geometry, which we are constantly
/// updating to account for the client decorations. But that's not the area we want to render
/// to, due to our intrusize CSD. So, here we calculate the 'actual' size, by adding back in the insets
fn compute_outer_size(
    inset: Pixels,
    new_size: Option<Size<Pixels>>,
    tiling: Tiling,
) -> Option<Size<Pixels>> {
    new_size.map(|mut new_size| {
        if !tiling.top {
            new_size.height += inset;
        }
        if !tiling.bottom {
            new_size.height += inset;
        }
        if !tiling.left {
            new_size.width += inset;
        }
        if !tiling.right {
            new_size.width += inset;
        }

        new_size
    })
}

fn inset_by_tiling(mut bounds: Bounds<Pixels>, inset: Pixels, tiling: Tiling) -> Bounds<Pixels> {
    if !tiling.top {
        bounds.origin.y += inset;
        bounds.size.height -= inset;
    }
    if !tiling.bottom {
        bounds.size.height -= inset;
    }
    if !tiling.left {
        bounds.origin.x += inset;
        bounds.size.width -= inset;
    }
    if !tiling.right {
        bounds.size.width -= inset;
    }

    bounds
}

/// Olo: what a popup opens over: a window's or popup's xdg surface, or a
/// layer surface (Deskbar).
#[derive(Clone)]
pub enum PopupParent {
    Xdg(xdg_surface::XdgSurface),
    Layer(zwlr_layer_surface_v1::ZwlrLayerSurfaceV1),
}

/// Asks for a layer surface's place: its anchors, exclusive zone,
/// margins, and its size (`size`, but none along an axis anchored at both
/// ends: the output's).
fn place_layer(layer: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, options: &crate::LayerShellOptions, size: Size<Pixels>) {
    use zwlr_layer_surface_v1::Anchor;
    let (top, bottom, left, right) = options.anchor;
    let mut anchor = Anchor::empty();
    for (on, edge) in [(top, Anchor::Top), (bottom, Anchor::Bottom), (left, Anchor::Left), (right, Anchor::Right)] {
        if on {
            anchor |= edge;
        }
    }
    layer.set_anchor(anchor);
    let w = if left && right { 0 } else { size.width.0.round() as u32 };
    let h = if top && bottom { 0 } else { size.height.0.round() as u32 };
    layer.set_size(w, h);
    layer.set_exclusive_zone(options.exclusive_zone);
    let (mt, mr, mb, ml) = options.margin;
    layer.set_margin(mt, mr, mb, ml);
}

/// Olo: a new surface's scale, asked of the compositor on a queue of its
/// own (so nothing else is dispatched meanwhile) before it's drawn. The
/// window's own fractional scale object follows it (a surface may have
/// one at a time), which the compositor tells the same scale.
struct ScaleProbe {
    queue: wayland_client::EventQueue<ProbedScale>,
    object: wp_fractional_scale_v1::WpFractionalScaleV1,
}

#[derive(Default)]
struct ProbedScale(Option<u32>);

impl ScaleProbe {
    fn ask(
        manager: &wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1,
        surface: &wl_surface::WlSurface,
    ) -> Option<Self> {
        let connection = wayland_client::Connection::from_backend(surface.backend().upgrade()?);
        let queue = connection.new_event_queue();
        let object = manager.get_fractional_scale(surface, &queue.handle(), ());
        Some(Self { queue, object })
    }

    /// The scale, once the compositor has answered (most likely already).
    fn answer(mut self) -> Option<f32> {
        let mut scale = ProbedScale::default();
        let answered = self.queue.roundtrip(&mut scale);
        self.object.destroy();
        answered.ok()?;
        scale.0.map(|scale| scale as f32 / 120.0)
    }
}

impl wayland_client::Dispatch<wp_fractional_scale_v1::WpFractionalScaleV1, ()> for ProbedScale {
    fn event(
        state: &mut Self,
        _: &wp_fractional_scale_v1::WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        _: &(),
        _: &wayland_client::Connection,
        _: &wayland_client::QueueHandle<Self>,
    ) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event {
            state.0 = Some(scale);
        }
    }
}

/// Olo: a layer surface's keyboard as the protocol has it.
fn layer_keyboard(layer: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, keyboard: crate::LayerKeyboard) -> zwlr_layer_surface_v1::KeyboardInteractivity {
    match keyboard {
        crate::LayerKeyboard::None => zwlr_layer_surface_v1::KeyboardInteractivity::None,
        crate::LayerKeyboard::Exclusive => zwlr_layer_surface_v1::KeyboardInteractivity::Exclusive,
        // (Version 4 on; before, a click gives it nothing.)
        crate::LayerKeyboard::OnDemand if layer.version() >= 4 => zwlr_layer_surface_v1::KeyboardInteractivity::OnDemand,
        crate::LayerKeyboard::OnDemand => zwlr_layer_surface_v1::KeyboardInteractivity::None,
    }
}

/// Olo: how long a window's size rests after a resize by hand before its
/// swapchain is made to fit.
const SNUG_AFTER: std::time::Duration = std::time::Duration::from_millis(1500);

/// Olo: room for a window of `size` resized by hand: as big as its screen
/// (or a quarter more than now), so the rest of the resize costs nothing.
fn resize_room(state: &WaylandWindowState, size: Size<DevicePixels>) -> Size<DevicePixels> {
    match state.display.as_ref().map(|(_, o)| o.bounds.size) {
        Some(screen) if screen.width.0 > 0 && screen.height.0 > 0 => Size { width: DevicePixels(screen.width.0.max(size.width.0)), height: DevicePixels(screen.height.0.max(size.height.0)) },
        _ => {
            let up = |n: i32| ((n as f64 * 1.25 / 64.0).ceil() as i32 * 64).max(n);
            Size { width: DevicePixels(up(size.width.0)), height: DevicePixels(up(size.height.0)) }
        }
    }
}
