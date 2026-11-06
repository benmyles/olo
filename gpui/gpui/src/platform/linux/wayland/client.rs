use anyhow::Context as _;
use std::{
    cell::{RefCell, RefMut},
    hash::Hash,
    os::fd::{AsRawFd, BorrowedFd},
    path::PathBuf,
    rc::{Rc, Weak},
    time::{Duration, Instant},
};

use ashpd::WindowIdentifier;
use ashpd::url::Url;
use calloop::{
    EventLoop, LoopHandle,
    timer::{TimeoutAction, Timer},
};
use calloop_wayland_source::WaylandSource;
use collections::HashMap;
use filedescriptor::Pipe;
use smallvec::SmallVec;
use util::ResultExt;
use wayland_backend::client::ObjectId;
use wayland_backend::protocol::WEnum;
use wayland_client::event_created_child;
use wayland_client::globals::{GlobalList, GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_callback::{self, WlCallback};
use wayland_client::protocol::wl_data_device_manager::DndAction;
use wayland_client::protocol::wl_data_offer::WlDataOffer;
use wayland_client::protocol::wl_pointer::AxisSource;
use wayland_client::protocol::{
    wl_data_device, wl_data_device_manager, wl_data_offer, wl_data_source, wl_output, wl_region,
};
use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle, delegate_noop,
    protocol::{
        wl_buffer, wl_compositor, wl_keyboard, wl_pointer, wl_registry, wl_seat, wl_shm,
        wl_shm_pool, wl_subcompositor, wl_subsurface, wl_surface,
    },
};
use wayland_protocols::ext::session_lock::v1::client::{
    ext_session_lock_manager_v1, ext_session_lock_surface_v1, ext_session_lock_v1,
};
use wayland_protocols::wp::cursor_shape::v1::client::{
    wp_cursor_shape_device_v1, wp_cursor_shape_manager_v1,
};
use wayland_protocols::wp::fractional_scale::v1::client::{
    wp_fractional_scale_manager_v1, wp_fractional_scale_v1,
};
use wayland_protocols::wp::idle_inhibit::zv1::client::{
    zwp_idle_inhibit_manager_v1, zwp_idle_inhibitor_v1,
};
use wayland_protocols::wp::linux_dmabuf::zv1::client::{
    zwp_linux_buffer_params_v1, zwp_linux_dmabuf_v1,
};
use wayland_protocols::wp::pointer_gestures::zv1::client::{
    zwp_pointer_gesture_pinch_v1, zwp_pointer_gestures_v1,
};
use wayland_protocols::wp::primary_selection::zv1::client::zwp_primary_selection_offer_v1::{
    self, ZwpPrimarySelectionOfferV1,
};
use wayland_protocols::wp::primary_selection::zv1::client::{
    zwp_primary_selection_device_manager_v1, zwp_primary_selection_device_v1,
    zwp_primary_selection_source_v1,
};
use wayland_protocols::wp::text_input::zv3::client::zwp_text_input_v3::{
    ContentHint, ContentPurpose,
};
use wayland_protocols::wp::text_input::zv3::client::{
    zwp_text_input_manager_v3, zwp_text_input_v3,
};
use wayland_protocols::wp::viewporter::client::{wp_viewport, wp_viewporter};
use wayland_protocols::xdg::activation::v1::client::{xdg_activation_token_v1, xdg_activation_v1};
use wayland_protocols::xdg::decoration::zv1::client::{
    zxdg_decoration_manager_v1, zxdg_toplevel_decoration_v1,
};
use wayland_protocols::xdg::dialog::v1::client::{xdg_dialog_v1, xdg_wm_dialog_v1};
use wayland_protocols::xdg::foreign::zv2::client::{
    zxdg_exported_v2, zxdg_exporter_v2, zxdg_imported_v2, zxdg_importer_v2,
};
use wayland_protocols::xdg::shell::client::{
    xdg_popup, xdg_positioner, xdg_surface, xdg_toplevel, xdg_wm_base,
};
use wayland_protocols::xdg::toplevel_tag::v1::client::xdg_toplevel_tag_manager_v1;
use wayland_protocols::xdg::xdg_output::zv1::client::{zxdg_output_manager_v1, zxdg_output_v1};
use wayland_protocols_plasma::blur::client::{org_kde_kwin_blur, org_kde_kwin_blur_manager};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};
use xkbcommon::xkb::ffi::XKB_KEYMAP_FORMAT_TEXT_V1;
use xkbcommon::xkb::{self, KEYMAP_COMPILE_NO_FLAGS, Keycode};

use super::{
    display::WaylandDisplay,
    window::{ImeInput, WaylandWindowStatePtr},
};

use crate::platform::{PlatformWindow, blade::BladeContext};
use crate::{
    AnyWindowHandle, Bounds, Capslock, CursorStyle, DOUBLE_CLICK_INTERVAL, DevicePixels, DisplayId,
    FileDropEvent, ForegroundExecutor, KeyDownEvent, KeyUpEvent, Keystroke, LinuxCommon,
    LinuxKeyboardLayout, Modifiers, ModifiersChangedEvent, MouseButton, MouseDownEvent,
    MouseExitEvent, MouseMoveEvent, MouseUpEvent, NavigationDirection, Pixels, PlatformDisplay,
    PlatformInput, PlatformKeyboardLayout, Point, SCROLL_LINES, ScrollDelta, ScrollWheelEvent,
    Size, TouchPhase, WindowParams, point, px, size,
};
use crate::{
    SharedString,
    platform::linux::{
        LinuxClient, get_xkb_compose_state, is_within_click_distance, open_uri_internal, read_fd,
        reveal_path_internal,
        wayland::{
            clipboard::{Clipboard, DataOffer, FILE_LIST_MIME_TYPE, TEXT_MIME_TYPES},
            cursor::Cursor,
            serial::{SerialKind, SerialTracker},
            window::WaylandWindow,
        },
    },
};

/// Used to convert evdev scancode to xkb scancode
const MIN_KEYCODE: u32 = 8;

const UNKNOWN_KEYBOARD_LAYOUT_NAME: SharedString = SharedString::new_static("unknown");

/// Olo: how far a wheel's notch scrolls, in points (the egui kit's).
const WHEEL_NOTCH: f32 = 48.0;

#[derive(Clone)]
pub struct Globals {
    pub qh: QueueHandle<WaylandClientStatePtr>,
    pub activation: Option<xdg_activation_v1::XdgActivationV1>,
    pub compositor: wl_compositor::WlCompositor,
    pub cursor_shape_manager: Option<wp_cursor_shape_manager_v1::WpCursorShapeManagerV1>,
    pub data_device_manager: Option<wl_data_device_manager::WlDataDeviceManager>,
    pub primary_selection_manager:
        Option<zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1>,
    pub wm_base: xdg_wm_base::XdgWmBase,
    pub shm: wl_shm::WlShm,
    pub seat: wl_seat::WlSeat,
    pub viewporter: Option<wp_viewporter::WpViewporter>,
    pub fractional_scale_manager:
        Option<wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1>,
    pub decoration_manager: Option<zxdg_decoration_manager_v1::ZxdgDecorationManagerV1>,
    /// Olo: tags for windows (`floating`, `under-pointer`), which
    /// app_server places and tiles by.
    pub toplevel_tags: Option<xdg_toplevel_tag_manager_v1::XdgToplevelTagManagerV1>,
    /// Olo: locking the session (the lock screen).
    pub session_lock_manager: Option<ext_session_lock_manager_v1::ExtSessionLockManagerV1>,
    /// Olo: dialogs (alerts, panels): never tabs, modal or not.
    pub wm_dialog: Option<xdg_wm_dialog_v1::XdgWmDialogV1>,
    /// Olo: other programs' windows, to open ours over (a portal's
    /// dialogs).
    pub xdg_importer: Option<zxdg_importer_v2::ZxdgImporterV2>,
    /// Olo: ours, for another program's windows to open over (a file
    /// panel's look, `Window::export_handle`).
    pub xdg_exporter: Option<zxdg_exporter_v2::ZxdgExporterV2>,
    /// Olo: surfaces of the desktop's own (Deskbar, notifications).
    pub layer_shell: Option<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
    pub blur_manager: Option<org_kde_kwin_blur_manager::OrgKdeKwinBlurManager>,
    pub text_input_manager: Option<zwp_text_input_manager_v3::ZwpTextInputManagerV3>,
    /// Olo: keeping the screen awake while a window shows (a video).
    pub idle_inhibit_manager: Option<zwp_idle_inhibit_manager_v1::ZwpIdleInhibitManagerV1>,
    /// Olo: pinches (a touchpad's, a touch screen's as app_server makes
    /// them).
    pub pointer_gestures: Option<zwp_pointer_gestures_v1::ZwpPointerGesturesV1>,
    /// Olo: each display's size in points (a fractional scale's too).
    pub xdg_output_manager: Option<zxdg_output_manager_v1::ZxdgOutputManagerV1>,
    /// Olo: other programs' pictures shown over a window, in GPU memory
    /// (`Window::place_external`).
    pub subcompositor: Option<wl_subcompositor::WlSubcompositor>,
    pub linux_dmabuf: Option<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1>,
    pub executor: ForegroundExecutor,
}

impl Globals {
    fn new(
        globals: GlobalList,
        executor: ForegroundExecutor,
        qh: QueueHandle<WaylandClientStatePtr>,
        seat: wl_seat::WlSeat,
    ) -> Self {
        Globals {
            activation: globals.bind(&qh, 1..=1, ()).ok(),
            compositor: globals
                .bind(
                    &qh,
                    wl_surface::REQ_SET_BUFFER_SCALE_SINCE
                        ..=wl_surface::EVT_PREFERRED_BUFFER_SCALE_SINCE,
                    (),
                )
                .unwrap(),
            cursor_shape_manager: globals.bind(&qh, 1..=1, ()).ok(),
            data_device_manager: globals
                .bind(
                    &qh,
                    WL_DATA_DEVICE_MANAGER_VERSION..=WL_DATA_DEVICE_MANAGER_VERSION,
                    (),
                )
                .ok(),
            primary_selection_manager: globals.bind(&qh, 1..=1, ()).ok(),
            shm: globals.bind(&qh, 1..=1, ()).unwrap(),
            seat,
            wm_base: globals.bind(&qh, 2..=5, ()).unwrap(),
            viewporter: globals.bind(&qh, 1..=1, ()).ok(),
            fractional_scale_manager: globals.bind(&qh, 1..=1, ()).ok(),
            decoration_manager: globals.bind(&qh, 1..=1, ()).ok(),
            toplevel_tags: globals.bind(&qh, 1..=1, ()).ok(),
            layer_shell: globals.bind(&qh, 1..=4, ()).ok(),
            wm_dialog: globals.bind(&qh, 1..=1, ()).ok(),
            xdg_importer: globals.bind(&qh, 1..=1, ()).ok(),
            xdg_exporter: globals.bind(&qh, 1..=1, ()).ok(),
            session_lock_manager: globals.bind(&qh, 1..=1, ()).ok(),
            blur_manager: globals.bind(&qh, 1..=1, ()).ok(),
            text_input_manager: globals.bind(&qh, 1..=1, ()).ok(),
            idle_inhibit_manager: globals.bind(&qh, 1..=1, ()).ok(),
            pointer_gestures: globals.bind(&qh, 1..=3, ()).ok(),
            xdg_output_manager: globals.bind(&qh, 3..=3, ()).ok(),
            subcompositor: globals.bind(&qh, 1..=1, ()).ok(),
            // (Version 3: its formats come as events, each with its
            // modifiers.)
            linux_dmabuf: globals.bind(&qh, 3..=3, ()).ok(),
            executor,
            qh,
        }
    }
}

#[derive(Default, Debug, Clone, PartialEq, Eq, Hash)]
pub struct InProgressOutput {
    name: Option<String>,
    scale: Option<i32>,
    position: Option<Point<DevicePixels>>,
    size: Option<Size<DevicePixels>>,
    physical_mm: Size<i32>,
    subpixel: Option<wl_output::Subpixel>,
    transform: Option<wl_output::Transform>,
    refresh_mhz: i32,
    logical_position: Option<(i32, i32)>,
    logical_size: Option<(i32, i32)>,
}

impl InProgressOutput {
    fn complete(&self) -> Option<Output> {
        if let Some((position, size)) = self.position.zip(self.size) {
            let scale = self.scale.unwrap_or(1);
            Some(Output {
                name: self.name.clone(),
                scale,
                bounds: Bounds::new(position, size),
                physical_mm: self.physical_mm,
                subpixel: self.subpixel,
                transform: self.transform,
                refresh_mhz: self.refresh_mhz,
                logical: self
                    .logical_position
                    .zip(self.logical_size)
                    .map(|((x, y), (w, h))| [x, y, w, h]),
            })
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct Output {
    pub name: Option<String>,
    pub scale: i32,
    pub bounds: Bounds<DevicePixels>,
    /// Olo: its size in millimetres (0 if unknown), its colour stripes'
    /// order (as app_server knows or guesses it) and its rotation: what
    /// a window's text is drawn for.
    pub physical_mm: Size<i32>,
    pub subpixel: Option<wl_output::Subpixel>,
    pub transform: Option<wl_output::Transform>,
    /// Olo: its refresh rate in millihertz (0 if unknown): how long a
    /// frame shows, for easing a wheel's notches in.
    pub refresh_mhz: i32,
    /// Olo: where it is and how big, in points (x, y, width, height),
    /// as xdg-output says: at a fractional scale, not its mode over the
    /// whole number wl_output gives.
    pub logical: Option<[i32; 4]>,
}

impl Output {
    /// Where it is and how big, in points.
    pub fn bounds_in_points(&self) -> Bounds<Pixels> {
        match self.logical {
            Some([x, y, w, h]) => Bounds::new(
                point(px(x as f32), px(y as f32)),
                size(px(w as f32), px(h as f32)),
            ),
            None => self.bounds.to_pixels(self.scale as f32),
        }
    }
}

impl Output {
    /// How text is best drawn on it, as Olo decides for every display
    /// (`olo_protocol::panels::text_mode`: Preferences ▸ Display shows the
    /// same choice and why), unless the person chose otherwise
    /// (`crate::set_display_text_rendering`). `OLO_TEXT_RENDERING`
    /// (`hinted`, `unhinted`, `rgb`, `bgr`) says otherwise, for tests.
    pub fn text_rendering(&self, lcd_possible: bool) -> crate::TextRendering {
        use crate::{SubpixelLayout, TextRendering};
        use olo_protocol::panels::{self, Subpixel, TextMode};
        let chosen = std::env::var("OLO_TEXT_RENDERING")
            .ok()
            .and_then(|forced| match forced.as_str() {
                "hinted" => Some(TextRendering::Hinted),
                "unhinted" => Some(TextRendering::Unhinted),
                "rgb" => Some(TextRendering::Subpixel(SubpixelLayout::Rgb)),
                "bgr" => Some(TextRendering::Subpixel(SubpixelLayout::Bgr)),
                _ => None,
            })
            .or_else(|| {
                self.name
                    .as_deref()
                    .and_then(crate::text_system::display_text_rendering)
            });
        let rendering = chosen.unwrap_or_else(|| {
            let (rotation, mirrored) = match self.transform {
                Some(wl_output::Transform::_90) => (90, false),
                Some(wl_output::Transform::_180) => (180, false),
                Some(wl_output::Transform::_270) => (270, false),
                Some(wl_output::Transform::Flipped) => (0, true),
                Some(wl_output::Transform::Flipped90) => (90, true),
                Some(wl_output::Transform::Flipped180) => (180, true),
                Some(wl_output::Transform::Flipped270) => (270, true),
                _ => (0, false),
            };
            // (Mirrored across, a display's stripes run the other way.)
            let subpixel = match (self.subpixel, mirrored) {
                (Some(wl_output::Subpixel::HorizontalRgb), false) => Subpixel::Rgb,
                (Some(wl_output::Subpixel::HorizontalRgb), true) => Subpixel::Bgr,
                (Some(wl_output::Subpixel::HorizontalBgr), false) => Subpixel::Bgr,
                (Some(wl_output::Subpixel::HorizontalBgr), true) => Subpixel::Rgb,
                (Some(wl_output::Subpixel::VerticalRgb), _) => Subpixel::VerticalRgb,
                (Some(wl_output::Subpixel::VerticalBgr), _) => Subpixel::VerticalBgr,
                (Some(wl_output::Subpixel::None), _) => Subpixel::None,
                _ => Subpixel::Unknown,
            };
            let ppi = panels::ppi(self.bounds.size.width.0, self.physical_mm.width);
            match panels::text_mode(ppi, f64::from(self.scale), subpixel, rotation).0 {
                TextMode::Hinted | TextMode::Auto => TextRendering::Hinted,
                TextMode::Unhinted => TextRendering::Unhinted,
                TextMode::Subpixel(Subpixel::Bgr) => TextRendering::Subpixel(SubpixelLayout::Bgr),
                TextMode::Subpixel(_) => TextRendering::Subpixel(SubpixelLayout::Rgb),
            }
        });
        match rendering {
            TextRendering::Subpixel(_) if !lcd_possible => TextRendering::Hinted,
            rendering => rendering,
        }
    }
}

pub(crate) struct WaylandClientState {
    serial_tracker: SerialTracker,
    globals: Globals,
    /// Olo: the session's lock while this app holds it, and the
    /// connection (to send its end at once).
    session_lock: Option<ext_session_lock_v1::ExtSessionLockV1>,
    connection: Connection,
    /// Olo: the compositor's connection is gone (told once).
    lost: bool,
    /// Olo: made with the first window (an agent with none, the polkit
    /// agent's resident self, holds no GPU device: no driver threads, a
    /// third of the memory).
    gpu_context: Option<BladeContext>,
    /// Olo: the context being made on a thread of its own since the
    /// client started (`BladeContext::ahead`), for the first window.
    gpu_context_ahead: Option<std::thread::JoinHandle<anyhow::Result<BladeContext>>>,
    /// Olo: the modifiers the compositor takes dmabufs of each format
    /// with (linux-dmabuf's events), for external pictures.
    dmabuf_modifiers: HashMap<u32, Vec<u64>>,
    wl_seat: wl_seat::WlSeat, // TODO: Multi seat support
    wl_pointer: Option<wl_pointer::WlPointer>,
    /// Olo: the pointer's pinches, and the last scale one said.
    pinch: Option<zwp_pointer_gesture_pinch_v1::ZwpPointerGesturePinchV1>,
    pinch_scale: f32,
    wl_keyboard: Option<wl_keyboard::WlKeyboard>,
    cursor_shape_device: Option<wp_cursor_shape_device_v1::WpCursorShapeDeviceV1>,
    data_device: Option<wl_data_device::WlDataDevice>,
    primary_selection: Option<zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1>,
    text_input: Option<zwp_text_input_v3::ZwpTextInputV3>,
    pre_edit_text: Option<String>,
    ime_pre_edit: Option<String>,
    composing: bool,
    // Surface to Window mapping
    windows: HashMap<ObjectId, WaylandWindowStatePtr>,
    // Output to scale mapping
    outputs: HashMap<ObjectId, Output>,
    /// Olo: the outputs themselves, for a layer surface to sit on one.
    wl_outputs: HashMap<ObjectId, wl_output::WlOutput>,
    in_progress_outputs: HashMap<ObjectId, InProgressOutput>,
    keyboard_layout: LinuxKeyboardLayout,
    keymap_state: Option<xkb::State>,
    compose_state: Option<xkb::compose::State>,
    drag: DragState,
    /// Olo: a drag out of one of our windows, while it lasts.
    drag_out: Option<DragOut>,
    click: ClickState,
    repeat: KeyRepeat,
    pub modifiers: Modifiers,
    pub capslock: Capslock,
    axis_source: AxisSource,
    /// Olo: what the scroll going on comes from (fingers, their glide).
    scroll_kind: crate::ScrollKind,
    pub mouse_location: Option<Point<Pixels>>,
    continuous_scroll_delta: Option<Point<Pixels>>,
    /// A wheel's notches this pointer frame.
    discrete_scroll_delta: Option<Point<f32>>,
    vertical_modifier: f32,
    horizontal_modifier: f32,
    scroll_event_received: bool,
    /// Olo: the fingers (or the glide after them) stopped this frame.
    scroll_stopped: bool,
    /// Olo: fingers on the touchpad (or a glide) between a first move
    /// and a stop: their scrolls are one gesture.
    scrolling: bool,
    enter_token: Option<()>,
    button_pressed: Option<MouseButton>,
    mouse_focused_window: Option<WaylandWindowStatePtr>,
    keyboard_focused_window: Option<WaylandWindowStatePtr>,
    loop_handle: LoopHandle<'static, WaylandClientStatePtr>,
    cursor_style: Option<CursorStyle>,
    clipboard: Clipboard,
    data_offers: Vec<DataOffer<WlDataOffer>>,
    /// Olo: what each drag's source allows (its offer's source_actions).
    offer_actions: HashMap<ObjectId, DndAction>,
    primary_data_offer: Option<DataOffer<ZwpPrimarySelectionOfferV1>>,
    cursor: Cursor,
    pending_activation: Option<PendingActivation>,
    /// Olo: tokens asked for with `Window::activation_token`, by request.
    token_requests: HashMap<u64, Box<dyn FnOnce(Option<String>)>>,
    next_token_request: u64,
    /// Olo: those waiting for a window's exported handle
    /// (`Window::export_handle`), by its surface.
    export_requests: HashMap<ObjectId, Vec<Box<dyn FnOnce(Option<String>)>>>,
    event_loop: Option<EventLoop<'static, WaylandClientStatePtr>>,
    common: LinuxCommon,
    /// Olo: the clipboard's source while what's on it is ours.
    selection_source: Option<wl_data_source::WlDataSource>,
    /// Olo: quit while holding the clipboard: running on, windowless,
    /// until something else is copied (or ours is taken off).
    lingering: bool,
}

/// Olo: a drag out of one of our windows (`PlatformWindow::start_drag`):
/// its source, data, the action the place under it would take, the
/// surface showing its picture (with the buffer and pool behind it), and
/// who to tell when it's over.
struct DragOut {
    source: wl_data_source::WlDataSource,
    types: Vec<(String, std::sync::Arc<[u8]>)>,
    /// What it lets a drop do.
    allowed: DndAction,
    action: Option<crate::DragAction>,
    icon: Option<(
        wl_surface::WlSurface,
        wl_buffer::WlBuffer,
        wl_shm_pool::WlShmPool,
    )>,
    done: Option<futures::channel::oneshot::Sender<Option<crate::DragAction>>>,
}

pub struct DragState {
    data_offer: Option<wl_data_offer::WlDataOffer>,
    window: Option<WaylandWindowStatePtr>,
    position: Point<Pixels>,
    /// Olo: what's offered (its types and what its source allows), the
    /// enter's serial, whether it's our own drag out, and where the
    /// window takes it (None: the platform's own way, files copied;
    /// Some(None): nowhere).
    types: Vec<String>,
    source_actions: DndAction,
    serial: u32,
    from_us: bool,
    accept: Option<Option<crate::DropAccept>>,
}

/// Olo: a surface showing a drag's picture, from shared memory: the
/// surface, its buffer and the pool (kept until the drag is over).
fn drag_icon(
    globals: &Globals,
    image: &crate::DragImage,
) -> Option<(
    wl_surface::WlSurface,
    wl_buffer::WlBuffer,
    wl_shm_pool::WlShmPool,
)> {
    use std::{
        io::Write as _,
        os::fd::{AsFd as _, FromRawFd as _, OwnedFd},
    };
    let (w, h) = (image.width as usize, image.height as usize);
    if w == 0 || h == 0 || image.rgba.len() < w * h * 4 {
        return None;
    }
    // SAFETY: a new anonymous file, owned from here.
    let fd = unsafe {
        let fd = libc::memfd_create(c"olo-drag-icon".as_ptr(), libc::MFD_CLOEXEC);
        if fd < 0 {
            return None;
        }
        OwnedFd::from_raw_fd(fd)
    };
    // Premultiplied BGRA in memory, as ARGB8888 is.
    let bgra: Vec<u8> = image.rgba[..w * h * 4]
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [p[2], p[1], p[0], p[3]])
        .collect();
    let mut file = std::fs::File::from(fd);
    file.write_all(&bgra).ok()?;
    let size = (w * h * 4) as i32;
    let pool = globals.shm.create_pool(file.as_fd(), size, &globals.qh, ());
    let buffer = pool.create_buffer(
        0,
        w as i32,
        h as i32,
        w as i32 * 4,
        wl_shm::Format::Argb8888,
        &globals.qh,
        (),
    );
    let surface = globals.compositor.create_surface(&globals.qh, ());
    surface.set_buffer_scale(image.scale.max(1) as i32);
    // The pointer's place on it: the picture is drawn up and left of it.
    let (dx, dy) = (
        -(f32::from(image.hotspot.x).round() as i32),
        -(f32::from(image.hotspot.y).round() as i32),
    );
    if surface.version() >= 5 {
        surface.attach(Some(&buffer), 0, 0);
        surface.offset(dx, dy);
    } else {
        surface.attach(Some(&buffer), dx, dy);
    }
    surface.damage_buffer(0, 0, w as i32, h as i32);
    surface.commit();
    Some((surface, buffer, pool))
}

pub struct ClickState {
    last_mouse_button: Option<MouseButton>,
    last_click: Instant,
    last_location: Point<Pixels>,
    current_count: usize,
}

pub(crate) struct KeyRepeat {
    characters_per_second: u32,
    delay: Duration,
    current_id: u64,
    current_keycode: Option<xkb::Keycode>,
}

pub(crate) enum PendingActivation {
    /// URI to open in the web browser.
    Uri(String),
    /// Path to open in the file explorer.
    Path(PathBuf),
    /// A window from ourselves to raise.
    Window(ObjectId),
}

/// This struct is required to conform to Rust's orphan rules, so we can dispatch on the state but hand the
/// window to GPUI.
#[derive(Clone)]
pub struct WaylandClientStatePtr(Weak<RefCell<WaylandClientState>>);

impl WaylandClientStatePtr {
    /// Olo: the popups open over `parent` (a menu's submenus) taken down
    /// first, before it is: only the topmost popup may be destroyed.
    pub fn withdraw_popups_over(&self, parent: &xdg_surface::XdgSurface) {
        let Some(client) = self.0.upgrade() else {
            return;
        };
        let over: Vec<WaylandWindowStatePtr> = match client.try_borrow() {
            Ok(state) => state
                .windows
                .values()
                .filter(|w| w.is_popup_over(parent))
                .cloned()
                .collect(),
            Err(_) => return,
        };
        for w in over {
            w.withdraw_popup(self);
        }
    }

    /// Olo: before a menu opens over `parent`: our menus showing anywhere
    /// but along `parent`'s own chain (`parent`, the menu it opened over,
    /// …) go first. xdg-shell lets a grabbing popup open only on the
    /// topmost one, and the compositor ends the connection of a program
    /// that breaks that; an app that forgot a menu it had open (Olo's
    /// menu bar, switching menus, 2026-11-01) loses that menu, not its
    /// windows. Each goes now from the screen, its window as the loop
    /// next turns.
    pub fn withdraw_stray_menus(&self, parent: &xdg_surface::XdgSurface) {
        let Some(client) = self.0.upgrade() else {
            return;
        };
        let (windows, executor) = match client.try_borrow() {
            Ok(state) => (
                state.windows.values().cloned().collect::<Vec<_>>(),
                state.common.foreground_executor.clone(),
            ),
            Err(_) => return,
        };
        let mut chain = vec![parent.clone()];
        while let Some(up) = windows
            .iter()
            .find(|w| w.xdg_surface().as_ref() == chain.last())
            .and_then(|w| w.popup_of())
        {
            if chain.contains(&up) {
                break;
            }
            chain.push(up);
        }
        for w in windows {
            if !w.is_grabbing_popup() || w.xdg_surface().is_some_and(|x| chain.contains(&x)) {
                continue;
            }
            log::warn!("a menu was still showing where another opened: taking it down first");
            w.withdraw_popup(self);
            executor.spawn(async move { w.close() }).detach();
        }
    }

    pub fn get_client(&self) -> Rc<RefCell<WaylandClientState>> {
        self.0
            .upgrade()
            .expect("The pointer should always be valid when dispatching in wayland")
    }

    /// Olo: drags `drag` out of `origin` (see `PlatformWindow::start_drag`).
    pub fn start_drag(
        &self,
        origin: &wl_surface::WlSurface,
        drag: crate::OutgoingDrag,
        done: futures::channel::oneshot::Sender<Option<crate::DragAction>>,
    ) {
        let client = self.get_client();
        let mut state = client.borrow_mut();
        let (Some(manager), Some(device)) = (
            state.globals.data_device_manager.clone(),
            state.data_device.clone(),
        ) else {
            return;
        };
        if state.drag_out.is_some() || drag.types.is_empty() {
            return;
        }
        let qh = state.globals.qh.clone();
        let source = manager.create_data_source(&qh, ());
        for (mime, _) in &drag.types {
            source.offer(mime.clone());
        }
        let mut actions = DndAction::empty();
        if drag.copy {
            actions |= DndAction::Copy;
        }
        if drag.move_ {
            actions |= DndAction::Move;
        }
        if drag.ask {
            actions |= DndAction::Ask;
        }
        if source.version() >= 3 {
            source.set_actions(if actions.is_empty() {
                DndAction::Copy
            } else {
                actions
            });
        }
        let icon = drag
            .icon
            .as_ref()
            .and_then(|image| drag_icon(&state.globals, image));
        let serial = state.serial_tracker.get(SerialKind::MousePress);
        device.start_drag(
            Some(&source),
            origin,
            icon.as_ref().map(|(surface, ..)| surface),
            serial,
        );
        state.drag_out = Some(DragOut {
            source,
            types: drag.types,
            allowed: actions,
            action: None,
            icon,
            done: Some(done),
        });
    }

    /// Olo: where window `surface` takes what's dragged over it (None:
    /// nowhere), told to the compositor (the drop's type and action).
    pub fn set_drop_accept(&self, surface: &ObjectId, accept: Option<crate::DropAccept>) {
        let client = self.get_client();
        let mut state = client.borrow_mut();
        let ours = state
            .drag
            .window
            .as_ref()
            .is_some_and(|w| w.surface().id() == *surface);
        let Some(offer) = state.drag.data_offer.clone().filter(|_| ours) else {
            return;
        };
        let serial = state.drag.serial;
        match &accept {
            Some(a) => {
                offer.accept(serial, Some(a.mime.clone()));
                if offer.version() >= 3 {
                    let action = match a.action {
                        crate::DragAction::Copy => DndAction::Copy,
                        crate::DragAction::Move => DndAction::Move,
                        crate::DragAction::Ask => DndAction::Ask,
                    };
                    offer.set_actions(action, action);
                }
            }
            None => {
                offer.accept(serial, None);
                if offer.version() >= 3 {
                    offer.set_actions(DndAction::empty(), DndAction::empty());
                }
            }
        }
        state.drag.accept = Some(accept);
    }

    pub fn get_serial(&self, kind: SerialKind) -> u32 {
        self.0.upgrade().unwrap().borrow().serial_tracker.get(kind)
    }

    pub fn set_pending_activation(&self, window: ObjectId) {
        self.0.upgrade().unwrap().borrow_mut().pending_activation =
            Some(PendingActivation::Window(window));
    }

    pub fn enable_ime(&self) {
        let client = self.get_client();
        let mut state = client.borrow_mut();
        let Some(mut text_input) = state.text_input.take() else {
            return;
        };

        text_input.enable();
        text_input.set_content_type(ContentHint::None, ContentPurpose::Normal);
        if let Some(window) = state.keyboard_focused_window.clone() {
            drop(state);
            if let Some(area) = window.get_ime_area() {
                text_input.set_cursor_rectangle(
                    area.origin.x.0 as i32,
                    area.origin.y.0 as i32,
                    area.size.width.0 as i32,
                    area.size.height.0 as i32,
                );
            }
            state = client.borrow_mut();
        }
        text_input.commit();
        state.text_input = Some(text_input);
    }

    pub fn disable_ime(&self) {
        let client = self.get_client();
        let mut state = client.borrow_mut();
        state.composing = false;
        if let Some(text_input) = &state.text_input {
            text_input.disable();
            text_input.commit();
        }
    }

    pub fn update_ime_position(&self, bounds: Bounds<Pixels>) {
        let client = self.get_client();
        let mut state = client.borrow_mut();
        if state.composing || state.text_input.is_none() || state.pre_edit_text.is_some() {
            return;
        }

        let text_input = state.text_input.as_ref().unwrap();
        text_input.set_cursor_rectangle(
            bounds.origin.x.0 as i32,
            bounds.origin.y.0 as i32,
            bounds.size.width.0 as i32,
            bounds.size.height.0 as i32,
        );
        text_input.commit();
    }

    pub fn handle_keyboard_layout_change(&self) {
        let client = self.get_client();
        let mut state = client.borrow_mut();
        let changed = if let Some(keymap_state) = &state.keymap_state {
            let layout_idx = keymap_state.serialize_layout(xkbcommon::xkb::STATE_LAYOUT_EFFECTIVE);
            let keymap = keymap_state.get_keymap();
            let layout_name = keymap.layout_get_name(layout_idx);
            let changed = layout_name != state.keyboard_layout.name();
            if changed {
                state.keyboard_layout = LinuxKeyboardLayout::new(layout_name.to_string().into());
            }
            changed
        } else {
            let changed = &UNKNOWN_KEYBOARD_LAYOUT_NAME != state.keyboard_layout.name();
            if changed {
                state.keyboard_layout = LinuxKeyboardLayout::new(UNKNOWN_KEYBOARD_LAYOUT_NAME);
            }
            changed
        };

        if changed && let Some(mut callback) = state.common.callbacks.keyboard_layout_change.take()
        {
            drop(state);
            callback();
            state = client.borrow_mut();
            state.common.callbacks.keyboard_layout_change = Some(callback);
        }
    }

    pub fn drop_window(&self, surface_id: &ObjectId) {
        let mut client = self.get_client();
        let mut state = client.borrow_mut();
        let closed_window = state.windows.remove(surface_id).unwrap();
        if let Some(window) = state.mouse_focused_window.take()
            && !window.ptr_eq(&closed_window)
        {
            state.mouse_focused_window = Some(window);
        }
        if let Some(window) = state.keyboard_focused_window.take()
            && !window.ptr_eq(&closed_window)
        {
            state.keyboard_focused_window = Some(window);
        }
        // (Olo: an app with no windows left decides itself whether it
        // ends: olo-ui quits unless it stays resident, as an agent does.)
    }
}

#[derive(Clone)]
pub struct WaylandClient(Rc<RefCell<WaylandClientState>>);

impl Drop for WaylandClient {
    fn drop(&mut self) {
        let mut state = self.0.borrow_mut();
        state.windows.clear();

        if let Some(wl_pointer) = &state.wl_pointer {
            wl_pointer.release();
        }
        if let Some(cursor_shape_device) = &state.cursor_shape_device {
            cursor_shape_device.destroy();
        }
        if let Some(data_device) = &state.data_device {
            data_device.release();
        }
        if let Some(text_input) = &state.text_input {
            text_input.destroy();
        }
    }
}

const WL_DATA_DEVICE_MANAGER_VERSION: u32 = 3;

fn wl_seat_version(version: u32) -> u32 {
    // We rely on the wl_pointer.frame event
    const WL_SEAT_MIN_VERSION: u32 = 5;
    const WL_SEAT_MAX_VERSION: u32 = 9;

    if version < WL_SEAT_MIN_VERSION {
        panic!(
            "wl_seat below required version: {} < {}",
            version, WL_SEAT_MIN_VERSION
        );
    }

    version.clamp(WL_SEAT_MIN_VERSION, WL_SEAT_MAX_VERSION)
}

fn wl_output_version(version: u32) -> u32 {
    const WL_OUTPUT_MIN_VERSION: u32 = 2;
    const WL_OUTPUT_MAX_VERSION: u32 = 4;

    if version < WL_OUTPUT_MIN_VERSION {
        panic!(
            "wl_output below required version: {} < {}",
            version, WL_OUTPUT_MIN_VERSION
        );
    }

    version.clamp(WL_OUTPUT_MIN_VERSION, WL_OUTPUT_MAX_VERSION)
}

impl WaylandClient {
    pub(crate) fn new() -> Self {
        // Olo: a compositor restarting in place (an upgrade) is back
        // within a second or two; a program re-executing to reconnect
        // waits for it rather than fail, and connects again if it goes
        // once more before answering (an upgrade restarting olo-wlproxy
        // and app_server one after the other: Telegram's panic on
        // `registry_queue_init`, 2026-11-02).
        let (conn, globals, mut event_queue) = {
            let mut tries = 0;
            loop {
                let made = connect().map_err(|e| e.to_string()).and_then(|c| {
                    match registry_queue_init::<WaylandClientStatePtr>(&c) {
                        Ok((globals, queue)) => Ok((c, globals, queue)),
                        Err(e) => Err(e.to_string()),
                    }
                });
                match made {
                    Ok(made) => break made,
                    Err(e) if tries < 150 => {
                        if tries == 0 {
                            log::warn!("no compositor yet ({e}); waiting for it");
                        }
                        tries += 1;
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    }
                    Err(e) => panic!("could not connect to the compositor: {e}"),
                }
            }
        };
        let qh = event_queue.handle();

        let mut seat: Option<wl_seat::WlSeat> = None;
        #[allow(clippy::mutable_key_type)]
        let mut in_progress_outputs = HashMap::default();
        let mut wl_outputs = Vec::new();
        globals.contents().with_list(|list| {
            for global in list {
                match &global.interface[..] {
                    "wl_seat" => {
                        seat = Some(globals.registry().bind::<wl_seat::WlSeat, _, _>(
                            global.name,
                            wl_seat_version(global.version),
                            &qh,
                            (),
                        ));
                    }
                    "wl_output" => {
                        let output = globals.registry().bind::<wl_output::WlOutput, _, _>(
                            global.name,
                            wl_output_version(global.version),
                            &qh,
                            (),
                        );
                        in_progress_outputs.insert(output.id(), InProgressOutput::default());
                        wl_outputs.push(output);
                    }
                    _ => {}
                }
            }
        });

        let event_loop = EventLoop::<WaylandClientStatePtr>::try_new().unwrap();

        let (common, main_receiver) = LinuxCommon::new(event_loop.get_signal());

        let handle = event_loop.handle();
        handle
            .insert_source(main_receiver, {
                let handle = handle.clone();
                move |event, _, _: &mut WaylandClientStatePtr| {
                    if let calloop::channel::Event::Msg(runnable) = event {
                        handle.insert_idle(|_| {
                            runnable.run();
                        });
                    }
                }
            })
            .unwrap();

        let seat = seat.unwrap();
        let globals = Globals::new(
            globals,
            common.foreground_executor.clone(),
            qh.clone(),
            seat.clone(),
        );
        // Olo: each display's size in points, read with the rest below.
        if let Some(manager) = &globals.xdg_output_manager {
            for output in &wl_outputs {
                manager.get_xdg_output(output, &qh, output.id());
            }
        }

        let data_device = globals
            .data_device_manager
            .as_ref()
            .map(|data_device_manager| data_device_manager.get_data_device(&seat, &qh, ()));

        let primary_selection = globals
            .primary_selection_manager
            .as_ref()
            .map(|primary_selection_manager| primary_selection_manager.get_device(&seat, &qh, ()));

        let mut cursor = Cursor::new(&conn, &globals, 24);

        // (Olo: no desktop portal watched for the colour scheme and the
        // cursor's theme and size. olo-ui takes the look from app_server,
        // which draws the pointer; the portal's settings over D-Bus kept a
        // thread of the `blocking` pool waking in every app.)

        let mut state = Rc::new(RefCell::new(WaylandClientState {
            serial_tracker: SerialTracker::new(),
            globals,
            gpu_context: None,
            gpu_context_ahead: Some(BladeContext::ahead()),
            dmabuf_modifiers: HashMap::default(),
            session_lock: None,
            connection: conn.clone(),
            wl_seat: seat,
            wl_pointer: None,
            pinch: None,
            pinch_scale: 1.0,
            wl_keyboard: None,
            cursor_shape_device: None,
            data_device,
            primary_selection,
            text_input: None,
            pre_edit_text: None,
            ime_pre_edit: None,
            composing: false,
            outputs: HashMap::default(),
            wl_outputs: HashMap::default(),
            in_progress_outputs,
            windows: HashMap::default(),
            common,
            keyboard_layout: LinuxKeyboardLayout::new(UNKNOWN_KEYBOARD_LAYOUT_NAME),
            keymap_state: None,
            compose_state: None,
            drag: DragState {
                data_offer: None,
                window: None,
                position: Point::default(),
                types: Vec::new(),
                source_actions: DndAction::empty(),
                serial: 0,
                from_us: false,
                accept: None,
            },
            drag_out: None,
            click: ClickState {
                last_click: Instant::now(),
                last_mouse_button: None,
                last_location: Point::default(),
                current_count: 0,
            },
            repeat: KeyRepeat {
                characters_per_second: 16,
                delay: Duration::from_millis(500),
                current_id: 0,
                current_keycode: None,
            },
            modifiers: Modifiers {
                shift: false,
                control: false,
                alt: false,
                function: false,
                platform: false,
            },
            capslock: Capslock { on: false },
            scroll_event_received: false,
            scroll_stopped: false,
            scrolling: false,
            axis_source: AxisSource::Wheel,
            scroll_kind: crate::ScrollKind::Wheel,
            mouse_location: None,
            continuous_scroll_delta: None,
            discrete_scroll_delta: None,
            vertical_modifier: -1.0,
            horizontal_modifier: -1.0,
            button_pressed: None,
            mouse_focused_window: None,
            keyboard_focused_window: None,
            loop_handle: handle.clone(),
            enter_token: None,
            cursor_style: None,
            clipboard: Clipboard::new(conn.clone(), handle.clone()),
            data_offers: Vec::new(),
            offer_actions: HashMap::default(),
            primary_data_offer: None,
            cursor,
            pending_activation: None,
            token_requests: HashMap::default(),
            next_token_request: 0,
            export_requests: HashMap::default(),
            event_loop: Some(event_loop),
            selection_source: None,
            lingering: false,
            lost: false,
        }));

        CLIENT.with(|c| *c.borrow_mut() = Rc::downgrade(&state));
        // Olo: the displays read now (and the seat), before the first
        // window opens: it draws its first frame for the one it opens on.
        if let Err(e) = event_queue.roundtrip(&mut WaylandClientStatePtr(Rc::downgrade(&state))) {
            log::warn!("reading the displays: {e}");
        }
        WaylandSource::new(conn, event_queue)
            .insert(handle)
            .unwrap();

        Self(state)
    }
}

impl LinuxClient for WaylandClient {
    fn keyboard_layout(&self) -> Box<dyn PlatformKeyboardLayout> {
        Box::new(self.0.borrow().keyboard_layout.clone())
    }

    fn displays(&self) -> Vec<Rc<dyn PlatformDisplay>> {
        self.0
            .borrow()
            .outputs
            .iter()
            .map(|(id, output)| {
                Rc::new(WaylandDisplay {
                    id: id.clone(),
                    name: output.name.clone(),
                    bounds: output.bounds_in_points(),
                }) as Rc<dyn PlatformDisplay>
            })
            .collect()
    }

    fn display(&self, id: DisplayId) -> Option<Rc<dyn PlatformDisplay>> {
        self.0
            .borrow()
            .outputs
            .iter()
            .find_map(|(object_id, output)| {
                (object_id.protocol_id() == id.0).then(|| {
                    Rc::new(WaylandDisplay {
                        id: object_id.clone(),
                        name: output.name.clone(),
                        bounds: output.bounds_in_points(),
                    }) as Rc<dyn PlatformDisplay>
                })
            })
    }

    fn primary_display(&self) -> Option<Rc<dyn PlatformDisplay>> {
        None
    }

    #[cfg(feature = "screen-capture")]
    fn is_screen_capture_supported(&self) -> bool {
        false
    }

    #[cfg(feature = "screen-capture")]
    fn screen_capture_sources(
        &self,
    ) -> futures::channel::oneshot::Receiver<anyhow::Result<Vec<Rc<dyn crate::ScreenCaptureSource>>>>
    {
        // TODO: Get screen capture working on wayland. Be sure to try window resizing as that may
        // be tricky.
        //
        // start_scap_default_target_source()
        let (sources_tx, sources_rx) = futures::channel::oneshot::channel();
        sources_tx
            .send(Err(anyhow::anyhow!(
                "Wayland screen capture not yet implemented."
            )))
            .ok();
        sources_rx
    }

    fn open_window(
        &self,
        handle: AnyWindowHandle,
        params: WindowParams,
    ) -> anyhow::Result<Box<dyn PlatformWindow>> {
        // Olo: a menu opens only over the menus along its own chain.
        if let crate::WindowKind::Popup { parent, .. } = params.kind {
            let over = self
                .0
                .borrow()
                .windows
                .values()
                .find(|w| w.handle() == parent)
                .and_then(|w| w.xdg_surface());
            if let Some(over) = over {
                WaylandClientStatePtr(Rc::downgrade(&self.0)).withdraw_stray_menus(&over);
            }
        }
        let mut state = self.0.borrow_mut();
        // Olo: a window again (launched while lingering): it runs on.
        state.lingering = false;

        let parent = state
            .keyboard_focused_window
            .as_ref()
            .and_then(|w| w.toplevel());
        // Olo: a popup opens over its parent window's surface, grabbing
        // the press that opened it.
        let popup_parent = match params.kind {
            crate::WindowKind::Popup { parent, .. } | crate::WindowKind::Tip { parent, .. } => {
                state
                    .windows
                    .values()
                    .find(|w| w.handle() == parent)
                    .and_then(|w| w.popup_parent())
                    .map(|p| (p, state.serial_tracker.get(SerialKind::MousePress)))
            }
            _ => None,
        };
        // Olo: a lock surface covers an output (by name, or the first)
        // of the session's lock.
        let session_lock = if params.kind == crate::WindowKind::SessionLock {
            let lock = state.session_lock.clone().context("lock_session first")?;
            let output = state
                .outputs
                .iter()
                .find(|(_, o)| {
                    params.session_lock_output.is_none() || o.name == params.session_lock_output
                })
                .and_then(|(id, _)| state.wl_outputs.get(id).cloned())
                .context("no such output")?;
            Some((lock, output))
        } else {
            None
        };
        // Olo: the output a layer surface asked for, by name.
        let layer_output = params
            .layer_shell
            .as_ref()
            .and_then(|l| l.output.as_ref())
            .and_then(|name| {
                state
                    .outputs
                    .iter()
                    .find(|(_, o)| o.name.as_ref() == Some(name))
            })
            .and_then(|(id, _)| state.wl_outputs.get(id).cloned());
        // Olo: a display asked for by name that this connection doesn't
        // know yet (after app_server's restart they come one by one) is
        // refused, not stood in for by the compositor's choice: the bar
        // for the Dell went on the Studio Display under its own
        // (2026-11-01). The app hears of it (`on_displays_changed`) and
        // asks again.
        if let Some(name) = params.layer_shell.as_ref().and_then(|l| l.output.as_ref())
            && layer_output.is_none()
        {
            anyhow::bail!("display {name} isn't known to this connection yet");
        }

        // Olo: the displays it may open on (a layer's or a lock surface's
        // own, a popup's window's, with its scale): it draws its first
        // frame for them.
        let over = match params.kind {
            crate::WindowKind::Popup { parent, .. } | crate::WindowKind::Tip { parent, .. } => {
                state
                    .windows
                    .values()
                    .find(|w| w.handle() == parent)
                    .map(|w| w.scale_and_display())
            }
            _ => None,
        };
        let opens_on = match (
            over,
            layer_output
                .as_ref()
                .or(session_lock.as_ref().map(|(_, o)| o)),
        ) {
            (Some((scale, Some(display))), _) => (Some(scale), vec![display]),
            (Some((scale, None)), _) => (
                Some(scale),
                state
                    .outputs
                    .iter()
                    .map(|(id, o)| (id.clone(), o.clone()))
                    .collect(),
            ),
            (None, Some(output)) => (
                None,
                state
                    .outputs
                    .get(&output.id())
                    .map(|o| (output.id(), o.clone()))
                    .into_iter()
                    .collect(),
            ),
            (None, None) => (
                None,
                state
                    .outputs
                    .iter()
                    .map(|(id, o)| (id.clone(), o.clone()))
                    .collect(),
            ),
        };
        if state.gpu_context.is_none() {
            let ahead = state
                .gpu_context_ahead
                .take()
                .and_then(|t| t.join().ok())
                .and_then(|made| {
                    made.map_err(|e| log::warn!("the GPU context made ahead failed ({e}); again"))
                        .ok()
                });
            state.gpu_context = Some(match ahead {
                Some(context) => context,
                None => BladeContext::new()?,
            });
        }
        let gpu_context = state.gpu_context.as_ref().expect("made above");
        let (window, surface_id) = WaylandWindow::new(
            handle,
            state.globals.clone(),
            gpu_context,
            WaylandClientStatePtr(Rc::downgrade(&self.0)),
            params,
            state.common.appearance,
            parent,
            state.loop_handle.clone(),
            state.common.signal.clone(),
            popup_parent,
            layer_output,
            session_lock,
            opens_on,
        )?;
        state.windows.insert(surface_id, window.0.clone());

        Ok(Box::new(window))
    }

    fn set_cursor_style(&self, style: CursorStyle) {
        let mut state = self.0.borrow_mut();

        let need_update = state.cursor_style != Some(style);

        if need_update {
            let serial = state.serial_tracker.get(SerialKind::MouseEnter);
            state.cursor_style = Some(style);

            if let CursorStyle::None = style {
                let wl_pointer = state
                    .wl_pointer
                    .clone()
                    .expect("window is focused by pointer");
                wl_pointer.set_cursor(serial, None, 0, 0);
            } else if let Some(cursor_shape_device) = &state.cursor_shape_device {
                cursor_shape_device.set_shape(serial, style.to_shape());
            } else if let Some(focused_window) = &state.mouse_focused_window {
                // cursor-shape-v1 isn't supported, set the cursor using a surface.
                let wl_pointer = state
                    .wl_pointer
                    .clone()
                    .expect("window is focused by pointer");
                let scale = focused_window.primary_output_scale();
                state
                    .cursor
                    .set_icon(&wl_pointer, serial, style.to_icon_names(), scale);
            }
        }
    }

    fn open_uri(&self, uri: &str) {
        let mut state = self.0.borrow_mut();
        if let (Some(activation), Some(window)) = (
            state.globals.activation.clone(),
            state.mouse_focused_window.clone(),
        ) {
            state.pending_activation = Some(PendingActivation::Uri(uri.to_string()));
            let token = activation.get_activation_token(&state.globals.qh, ());
            let serial = state.serial_tracker.get(SerialKind::MousePress);
            token.set_serial(serial, &state.wl_seat);
            token.set_surface(&window.surface());
            token.commit();
        } else {
            let executor = state.common.background_executor.clone();
            open_uri_internal(executor, uri, None);
        }
    }

    fn reveal_path(&self, path: PathBuf) {
        let mut state = self.0.borrow_mut();
        if let (Some(activation), Some(window)) = (
            state.globals.activation.clone(),
            state.mouse_focused_window.clone(),
        ) {
            state.pending_activation = Some(PendingActivation::Path(path));
            let token = activation.get_activation_token(&state.globals.qh, ());
            let serial = state.serial_tracker.get(SerialKind::MousePress);
            token.set_serial(serial, &state.wl_seat);
            token.set_surface(&window.surface());
            token.commit();
        } else {
            let executor = state.common.background_executor.clone();
            reveal_path_internal(executor, path, None);
        }
    }

    fn with_common<R>(&self, f: impl FnOnce(&mut LinuxCommon) -> R) -> R {
        f(&mut self.0.borrow_mut().common)
    }

    fn run(&self) {
        let mut event_loop = self
            .0
            .borrow_mut()
            .event_loop
            .take()
            .expect("App is already running");

        event_loop
            .run(
                None,
                &mut WaylandClientStatePtr(Rc::downgrade(&self.0)),
                |ptr| {
                    // Olo: the compositor gone (restarted in place, or
                    // the connection in error) ends nothing by itself:
                    // calloop drops the failing source and the loop
                    // sleeps on with nothing to wake it, windows gone
                    // (Deskbar, 2026-11-01), or spins on a stored
                    // protocol error (Notifications). Seen here, after
                    // each turn, the program hands over to whoever asked
                    // (olo-ui: re-execute with the state saved, and
                    // reconnect), or stops its loop.
                    let Some(state) = ptr.0.upgrade() else { return };
                    compositor_lost_check(&state);
                },
            )
            .log_err();
        // (The loop may end on the error itself, before a turn's check.)
        compositor_lost_check(&self.0);
        // Olo: what the last turn asked (the windows quitting destroyed)
        // reaches the compositor now: a program that carries on after its
        // loop (Screenshot holding the clipboard) leaves nothing behind
        // on the screens.
        let _ = self.0.borrow().connection.flush();
    }

    fn write_to_primary(&self, item: crate::ClipboardItem) {
        let mut state = self.0.borrow_mut();
        let (Some(primary_selection_manager), Some(primary_selection)) = (
            state.globals.primary_selection_manager.clone(),
            state.primary_selection.clone(),
        ) else {
            return;
        };
        if state.mouse_focused_window.is_some() || state.keyboard_focused_window.is_some() {
            state.clipboard.set_primary(item);
            let serial = state.serial_tracker.get(SerialKind::KeyPress);
            let data_source = primary_selection_manager.create_source(&state.globals.qh, ());
            for mime_type in TEXT_MIME_TYPES {
                data_source.offer(mime_type.to_string());
            }
            data_source.offer(state.clipboard.self_mime());
            primary_selection.set_selection(Some(&data_source), serial);
        }
    }

    fn write_to_clipboard(&self, item: crate::ClipboardItem) {
        let mut state = self.0.borrow_mut();
        let (Some(data_device_manager), Some(data_device)) = (
            state.globals.data_device_manager.clone(),
            state.data_device.clone(),
        ) else {
            return;
        };
        if state.mouse_focused_window.is_some() || state.keyboard_focused_window.is_some() {
            // Olo: each form under its own types (text, an image's
            // format, data's MIME type).
            let types = item.mime_types(&TEXT_MIME_TYPES);
            state.clipboard.set(item);
            let serial = state.serial_tracker.get(SerialKind::KeyPress);
            let data_source = data_device_manager.create_data_source(&state.globals.qh, ());
            for mime_type in types {
                data_source.offer(mime_type);
            }
            data_source.offer(state.clipboard.self_mime());
            data_device.set_selection(Some(&data_source), serial);
            state.selection_source = Some(data_source);
        }
    }

    fn withdraw_clipboard(&self, item: &crate::ClipboardItem) {
        let mut state = self.0.borrow_mut();
        let Some(data_device) = state.data_device.clone() else {
            return;
        };
        let ours =
            state.clipboard.holds(item) || (state.lingering && state.clipboard.contents_are(item));
        if ours {
            let serial = state.serial_tracker.get(SerialKind::KeyPress);
            data_device.set_selection(None, serial);
            state.clipboard.forget();
            if let Some(source) = state.selection_source.take() {
                source.destroy();
            }
            if state.lingering {
                state.common.signal.stop();
            }
        }
    }

    fn linger_for_clipboard(&self) -> bool {
        let mut state = self.0.borrow_mut();
        state.lingering = state.selection_source.is_some();
        state.lingering
    }

    fn holds_clipboard(&self) -> bool {
        self.0.borrow().selection_source.is_some()
    }

    fn external_modifiers(&self, fourcc: u32) -> Vec<u64> {
        let state = self.0.borrow();
        let g = &state.globals;
        if g.subcompositor.is_none() || g.viewporter.is_none() || g.linux_dmabuf.is_none() {
            return Vec::new();
        }
        state
            .dmabuf_modifiers
            .get(&fourcc)
            .cloned()
            .unwrap_or_default()
    }

    fn read_from_primary(&self) -> Option<crate::ClipboardItem> {
        self.0.borrow_mut().clipboard.read_primary()
    }

    fn read_from_clipboard(&self) -> Option<crate::ClipboardItem> {
        self.0.borrow_mut().clipboard.read()
    }

    fn own_clipboard(&self) -> Option<crate::ClipboardItem> {
        self.0.borrow().clipboard.own()
    }

    fn clipboard_types(&self) -> Vec<String> {
        self.0.borrow().clipboard.types()
    }

    fn receive_clipboard(&self, mime: &str) -> Option<crate::ClipboardReceipt> {
        self.0.borrow().clipboard.receive(mime)
    }

    fn active_window(&self) -> Option<AnyWindowHandle> {
        self.0
            .borrow_mut()
            .keyboard_focused_window
            .as_ref()
            .map(|window| window.handle())
    }

    fn window_stack(&self) -> Option<Vec<AnyWindowHandle>> {
        None
    }

    fn compositor_name(&self) -> &'static str {
        "Wayland"
    }

    fn window_identifier(&self) -> impl Future<Output = Option<WindowIdentifier>> + Send + 'static {
        async fn inner(surface: Option<wl_surface::WlSurface>) -> Option<WindowIdentifier> {
            if let Some(surface) = surface {
                ashpd::WindowIdentifier::from_wayland(&surface).await
            } else {
                None
            }
        }

        let client_state = self.0.borrow();
        let active_window = client_state.keyboard_focused_window.as_ref();
        inner(active_window.map(|aw| aw.surface()))
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for WaylandClientStatePtr {
    fn event(
        this: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let mut client = this.get_client();
        let mut state = client.borrow_mut();

        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } => match &interface[..] {
                "wl_seat" => {
                    if let Some(wl_pointer) = state.wl_pointer.take() {
                        wl_pointer.release();
                    }
                    if let Some(wl_keyboard) = state.wl_keyboard.take() {
                        wl_keyboard.release();
                    }
                    state.wl_seat.release();
                    state.wl_seat = registry.bind::<wl_seat::WlSeat, _, _>(
                        name,
                        wl_seat_version(version),
                        qh,
                        (),
                    );
                }
                "wl_output" => {
                    let output = registry.bind::<wl_output::WlOutput, _, _>(
                        name,
                        wl_output_version(version),
                        qh,
                        (),
                    );

                    state
                        .in_progress_outputs
                        .insert(output.id(), InProgressOutput::default());
                    if let Some(manager) = &state.globals.xdg_output_manager {
                        manager.get_xdg_output(&output, qh, output.id());
                    }
                }
                _ => {}
            },
            wl_registry::Event::GlobalRemove { name: _ } => {
                // TODO: handle global removal
            }
            _ => {}
        }
    }
}

delegate_noop!(WaylandClientStatePtr: ignore xdg_activation_v1::XdgActivationV1);
delegate_noop!(WaylandClientStatePtr: ignore zwp_pointer_gestures_v1::ZwpPointerGesturesV1);

/// Olo: a pinch, to the window the pointer is on ([`crate::PinchEvent`]).
impl Dispatch<zwp_pointer_gesture_pinch_v1::ZwpPointerGesturePinchV1, ()>
    for WaylandClientStatePtr
{
    fn event(
        this: &mut Self,
        _: &zwp_pointer_gesture_pinch_v1::ZwpPointerGesturePinchV1,
        event: zwp_pointer_gesture_pinch_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use zwp_pointer_gesture_pinch_v1::Event;
        let client = this.get_client();
        let mut state = client.borrow_mut();
        let (Some(window), Some(position)) =
            (state.mouse_focused_window.clone(), state.mouse_location)
        else {
            return;
        };
        let modifiers = state.modifiers;
        let (scale, step, phase) = match event {
            Event::Begin { .. } => {
                state.pinch_scale = 1.0;
                (1.0, 1.0, TouchPhase::Started)
            }
            Event::Update { scale, .. } => {
                let scale = scale as f32;
                let step = if state.pinch_scale > 0.0 {
                    scale / state.pinch_scale
                } else {
                    1.0
                };
                state.pinch_scale = scale;
                (scale, step, TouchPhase::Moved)
            }
            Event::End { .. } => (state.pinch_scale, 1.0, TouchPhase::Ended),
            _ => return,
        };
        drop(state);
        window.handle_input(PlatformInput::Pinch(crate::PinchEvent {
            position,
            scale,
            step,
            phase,
            modifiers,
        }));
    }
}
delegate_noop!(WaylandClientStatePtr: ignore wl_compositor::WlCompositor);
delegate_noop!(WaylandClientStatePtr: ignore wp_cursor_shape_device_v1::WpCursorShapeDeviceV1);
delegate_noop!(WaylandClientStatePtr: ignore wp_cursor_shape_manager_v1::WpCursorShapeManagerV1);
delegate_noop!(WaylandClientStatePtr: ignore wl_data_device_manager::WlDataDeviceManager);
delegate_noop!(WaylandClientStatePtr: ignore zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1);
delegate_noop!(WaylandClientStatePtr: ignore wl_shm::WlShm);
delegate_noop!(WaylandClientStatePtr: ignore wl_shm_pool::WlShmPool);
delegate_noop!(WaylandClientStatePtr: ignore wl_buffer::WlBuffer);
delegate_noop!(WaylandClientStatePtr: ignore wl_region::WlRegion);
delegate_noop!(WaylandClientStatePtr: ignore wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1);
delegate_noop!(WaylandClientStatePtr: ignore zxdg_decoration_manager_v1::ZxdgDecorationManagerV1);
delegate_noop!(WaylandClientStatePtr: ignore zxdg_output_manager_v1::ZxdgOutputManagerV1);

/// Olo: a display's place and size in points (applied at its
/// wl_output's `done`).
impl Dispatch<zxdg_output_v1::ZxdgOutputV1, ObjectId> for WaylandClientStatePtr {
    fn event(
        this: &mut Self,
        _: &zxdg_output_v1::ZxdgOutputV1,
        event: zxdg_output_v1::Event,
        output: &ObjectId,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let client = this.get_client();
        let mut state = client.borrow_mut();
        let Some(in_progress) = state.in_progress_outputs.get_mut(output) else {
            return;
        };
        match event {
            zxdg_output_v1::Event::LogicalPosition { x, y } => {
                in_progress.logical_position = Some((x, y))
            }
            zxdg_output_v1::Event::LogicalSize { width, height } => {
                in_progress.logical_size = Some((width, height))
            }
            _ => {}
        }
    }
}
delegate_noop!(WaylandClientStatePtr: ignore org_kde_kwin_blur_manager::OrgKdeKwinBlurManager);
delegate_noop!(WaylandClientStatePtr: ignore zwp_text_input_manager_v3::ZwpTextInputManagerV3);
delegate_noop!(WaylandClientStatePtr: ignore zwp_idle_inhibit_manager_v1::ZwpIdleInhibitManagerV1);
delegate_noop!(WaylandClientStatePtr: ignore zwp_idle_inhibitor_v1::ZwpIdleInhibitorV1);
delegate_noop!(WaylandClientStatePtr: ignore org_kde_kwin_blur::OrgKdeKwinBlur);
delegate_noop!(WaylandClientStatePtr: ignore wp_viewporter::WpViewporter);
delegate_noop!(WaylandClientStatePtr: ignore wp_viewport::WpViewport);
delegate_noop!(WaylandClientStatePtr: ignore wl_subcompositor::WlSubcompositor);
delegate_noop!(WaylandClientStatePtr: ignore wl_subsurface::WlSubsurface);
delegate_noop!(WaylandClientStatePtr: ignore zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1);

impl Dispatch<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1, ()> for WaylandClientStatePtr {
    fn event(
        this: &mut Self,
        _: &zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1,
        event: zwp_linux_dmabuf_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwp_linux_dmabuf_v1::Event::Modifier {
            format,
            modifier_hi,
            modifier_lo,
        } = event
        {
            let client = this.get_client();
            let mut state = client.borrow_mut();
            let modifier = (u64::from(modifier_hi) << 32) | u64::from(modifier_lo);
            let list = state.dmabuf_modifiers.entry(format).or_default();
            if !list.contains(&modifier) {
                list.push(modifier);
            }
        }
    }
}

/// Olo: an external picture's buffer (`Window::set_external_buffer`):
/// its id, told to its owner when the compositor lets it go.
pub(crate) struct ExternalRelease {
    pub id: u64,
    pub released: std::sync::Arc<dyn Fn(u64) + Send + Sync>,
}

impl Dispatch<wl_buffer::WlBuffer, ExternalRelease> for WaylandClientStatePtr {
    fn event(
        _: &mut Self,
        _: &wl_buffer::WlBuffer,
        event: wl_buffer::Event,
        data: &ExternalRelease,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_buffer::Event::Release = event {
            (data.released)(data.id);
        }
    }
}

impl Dispatch<WlCallback, ObjectId> for WaylandClientStatePtr {
    fn event(
        state: &mut WaylandClientStatePtr,
        _: &wl_callback::WlCallback,
        event: wl_callback::Event,
        surface_id: &ObjectId,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let client = state.get_client();
        let mut state = client.borrow_mut();
        let Some(window) = get_window(&mut state, surface_id) else {
            return;
        };
        drop(state);

        if let wl_callback::Event::Done { .. } = event {
            window.frame_done();
        }
    }
}

fn get_window(
    mut state: &mut RefMut<WaylandClientState>,
    surface_id: &ObjectId,
) -> Option<WaylandWindowStatePtr> {
    state.windows.get(surface_id).cloned()
}

impl Dispatch<wl_surface::WlSurface, ()> for WaylandClientStatePtr {
    fn event(
        this: &mut Self,
        surface: &wl_surface::WlSurface,
        event: <wl_surface::WlSurface as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let mut client = this.get_client();
        let mut state = client.borrow_mut();

        let Some(window) = get_window(&mut state, &surface.id()) else {
            return;
        };
        #[allow(clippy::mutable_key_type)]
        let outputs = state.outputs.clone();
        drop(state);

        window.handle_surface_event(event, outputs);
    }
}

impl Dispatch<wl_output::WlOutput, ()> for WaylandClientStatePtr {
    fn event(
        this: &mut Self,
        output: &wl_output::WlOutput,
        event: <wl_output::WlOutput as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let mut client = this.get_client();
        let mut state = client.borrow_mut();

        let Some(mut in_progress_output) = state.in_progress_outputs.get_mut(&output.id()) else {
            return;
        };

        match event {
            wl_output::Event::Name { name } => {
                in_progress_output.name = Some(name);
            }
            wl_output::Event::Scale { factor } => {
                in_progress_output.scale = Some(factor);
            }
            wl_output::Event::Geometry {
                x,
                y,
                physical_width,
                physical_height,
                subpixel,
                transform,
                ..
            } => {
                in_progress_output.position = Some(point(DevicePixels(x), DevicePixels(y)));
                in_progress_output.physical_mm = size(physical_width, physical_height);
                in_progress_output.subpixel = subpixel.into_result().ok();
                in_progress_output.transform = transform.into_result().ok();
            }
            // Olo: every mode it offers comes; the one it's in is marked.
            wl_output::Event::Mode {
                flags,
                width,
                height,
                refresh,
            } => {
                if flags
                    .into_result()
                    .is_ok_and(|f| f.contains(wl_output::Mode::Current))
                {
                    in_progress_output.size = Some(size(DevicePixels(width), DevicePixels(height)));
                    in_progress_output.refresh_mhz = refresh;
                }
            }
            // (Olo: kept, so later changes are followed too.)
            wl_output::Event::Done => {
                if let Some(complete) = in_progress_output.complete() {
                    state.outputs.insert(output.id(), complete.clone());
                    state.wl_outputs.insert(output.id(), output.clone());
                    // Olo: a new output must be covered while locked.
                    if state.session_lock.is_some() {
                        state
                            .common
                            .foreground_executor
                            .spawn(
                                async move { session_lock_event(crate::SessionLockEvent::Outputs) },
                            )
                            .detach();
                    }
                    let windows: Vec<_> = state.windows.values().cloned().collect();
                    drop(state);
                    for window in windows {
                        window.output_changed(output.id(), &complete);
                    }
                    // Olo: whoever waits for a display (a bar for it).
                    let displays_changed =
                        client.borrow_mut().common.callbacks.displays_changed.take();
                    if let Some(mut f) = displays_changed {
                        f();
                        client.borrow_mut().common.callbacks.displays_changed = Some(f);
                    }
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<xdg_surface::XdgSurface, ObjectId> for WaylandClientStatePtr {
    fn event(
        state: &mut Self,
        _: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        surface_id: &ObjectId,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let client = state.get_client();
        let mut state = client.borrow_mut();
        let Some(window) = get_window(&mut state, surface_id) else {
            return;
        };
        drop(state);
        window.handle_xdg_surface_event(event);
    }
}

impl Dispatch<xdg_toplevel::XdgToplevel, ObjectId> for WaylandClientStatePtr {
    fn event(
        this: &mut Self,
        _: &xdg_toplevel::XdgToplevel,
        event: <xdg_toplevel::XdgToplevel as Proxy>::Event,
        surface_id: &ObjectId,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let client = this.get_client();
        let mut state = client.borrow_mut();
        let Some(window) = get_window(&mut state, surface_id) else {
            return;
        };

        drop(state);
        let should_close = window.handle_toplevel_event(event);

        if should_close {
            // The close logic will be handled in drop_window()
            window.close();
        }
    }
}

impl Dispatch<xdg_popup::XdgPopup, ObjectId> for WaylandClientStatePtr {
    fn event(
        this: &mut Self,
        _: &xdg_popup::XdgPopup,
        event: xdg_popup::Event,
        surface_id: &ObjectId,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let client = this.get_client();
        let mut state = client.borrow_mut();
        let Some(window) = get_window(&mut state, surface_id) else {
            return;
        };
        drop(state);
        if window.handle_popup_event(event) {
            window.close();
        }
    }
}

delegate_noop!(WaylandClientStatePtr: ignore xdg_positioner::XdgPositioner);
delegate_noop!(WaylandClientStatePtr: ignore xdg_toplevel_tag_manager_v1::XdgToplevelTagManagerV1);
delegate_noop!(WaylandClientStatePtr: ignore zwlr_layer_shell_v1::ZwlrLayerShellV1);
delegate_noop!(WaylandClientStatePtr: ignore xdg_wm_dialog_v1::XdgWmDialogV1);
delegate_noop!(WaylandClientStatePtr: ignore ext_session_lock_manager_v1::ExtSessionLockManagerV1);

impl Dispatch<ext_session_lock_v1::ExtSessionLockV1, ()> for WaylandClientStatePtr {
    fn event(
        this: &mut Self,
        _: &ext_session_lock_v1::ExtSessionLockV1,
        event: ext_session_lock_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let event = match event {
            ext_session_lock_v1::Event::Locked => crate::SessionLockEvent::Locked,
            ext_session_lock_v1::Event::Finished => {
                this.get_client().borrow_mut().session_lock = None;
                crate::SessionLockEvent::Finished
            }
            _ => return,
        };
        session_lock_event(event);
    }
}

impl Dispatch<ext_session_lock_surface_v1::ExtSessionLockSurfaceV1, ObjectId>
    for WaylandClientStatePtr
{
    fn event(
        this: &mut Self,
        _: &ext_session_lock_surface_v1::ExtSessionLockSurfaceV1,
        event: ext_session_lock_surface_v1::Event,
        surface_id: &ObjectId,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let client = this.get_client();
        let mut state = client.borrow_mut();
        let Some(window) = get_window(&mut state, surface_id) else {
            return;
        };
        drop(state);
        window.handle_lock_surface_event(event);
    }
}

thread_local! {
    /// Olo: the client, for the session lock's free functions.
    static CLIENT: RefCell<std::rc::Weak<RefCell<WaylandClientState>>> = RefCell::new(std::rc::Weak::new());
    /// Olo: who hears of the session lock.
    static LOCK_HANDLER: RefCell<Option<Box<dyn FnMut(crate::SessionLockEvent)>>> = RefCell::new(None);
}

/// Tells the lock's handler (outside any borrow of the client).
fn session_lock_event(event: crate::SessionLockEvent) {
    let handler = LOCK_HANDLER.with(|h| h.borrow_mut().take());
    if let Some(mut handler) = handler {
        handler(event);
        LOCK_HANDLER.with(|h| {
            let mut h = h.borrow_mut();
            if h.is_none() {
                *h = Some(handler);
            }
        });
    }
}

/// Olo: locks the session (ext-session-lock): `handler` hears when it's
/// locked (then open a `WindowKind::SessionLock` window on each output),
/// when outputs change while it is (cover the new ones), and when the
/// compositor ends or refuses it.
pub fn lock_session(handler: impl FnMut(crate::SessionLockEvent) + 'static) -> anyhow::Result<()> {
    let client = CLIENT
        .with(|c| c.borrow().upgrade())
        .context("no Wayland connection")?;
    let mut state = client.borrow_mut();
    let manager = state
        .globals
        .session_lock_manager
        .clone()
        .context("the compositor can't lock the session")?;
    LOCK_HANDLER.with(|h| *h.borrow_mut() = Some(Box::new(handler)));
    if state.session_lock.is_none() {
        state.session_lock = Some(manager.lock(&state.globals.qh, ()));
    }
    Ok(())
}

/// Olo: ends the session lock (the lock screen was answered), sent to
/// the compositor at once.
pub fn unlock_session() {
    let Some(client) = CLIENT.with(|c| c.borrow().upgrade()) else {
        return;
    };
    let mut state = client.borrow_mut();
    if let Some(lock) = state.session_lock.take() {
        lock.unlock_and_destroy();
        let _ = state.connection.flush();
    }
}
delegate_noop!(WaylandClientStatePtr: ignore xdg_dialog_v1::XdgDialogV1);
delegate_noop!(WaylandClientStatePtr: ignore zxdg_importer_v2::ZxdgImporterV2);
delegate_noop!(WaylandClientStatePtr: ignore zxdg_exporter_v2::ZxdgExporterV2);

/// Olo: a window exported (`Window::export_handle`): its handle, to the
/// window and whoever waits for it.
impl Dispatch<zxdg_exported_v2::ZxdgExportedV2, ObjectId> for WaylandClientStatePtr {
    fn event(
        this: &mut Self,
        _: &zxdg_exported_v2::ZxdgExportedV2,
        event: zxdg_exported_v2::Event,
        surface_id: &ObjectId,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let zxdg_exported_v2::Event::Handle { handle } = event else {
            return;
        };
        let client = this.get_client();
        let mut state = client.borrow_mut();
        if let Some(window) = get_window(&mut state, surface_id) {
            window.exported_as(handle.clone());
        }
        for done in state.export_requests.remove(surface_id).unwrap_or_default() {
            let handle = handle.clone();
            // (Called once the state is free: it may open windows.)
            state
                .common
                .foreground_executor
                .spawn(async move { done(Some(handle)) })
                .detach();
        }
    }
}
delegate_noop!(WaylandClientStatePtr: ignore zxdg_imported_v2::ZxdgImportedV2);

impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, ObjectId> for WaylandClientStatePtr {
    fn event(
        this: &mut Self,
        _: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        surface_id: &ObjectId,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let client = this.get_client();
        let mut state = client.borrow_mut();
        let Some(window) = get_window(&mut state, surface_id) else {
            return;
        };
        drop(state);
        if window.handle_layer_surface_event(event) {
            window.close();
        }
    }
}

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for WaylandClientStatePtr {
    fn event(
        _: &mut Self,
        wm_base: &xdg_wm_base::XdgWmBase,
        event: <xdg_wm_base::XdgWmBase as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            wm_base.pong(serial);
        }
    }
}

impl Dispatch<xdg_activation_token_v1::XdgActivationTokenV1, ()> for WaylandClientStatePtr {
    fn event(
        this: &mut Self,
        token: &xdg_activation_token_v1::XdgActivationTokenV1,
        event: <xdg_activation_token_v1::XdgActivationTokenV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let client = this.get_client();
        let mut state = client.borrow_mut();

        if let xdg_activation_token_v1::Event::Done { token } = event {
            let executor = state.common.background_executor.clone();
            match state.pending_activation.take() {
                Some(PendingActivation::Uri(uri)) => open_uri_internal(executor, &uri, Some(token)),
                Some(PendingActivation::Path(path)) => {
                    reveal_path_internal(executor, path, Some(token))
                }
                Some(PendingActivation::Window(window)) => {
                    let Some(window) = get_window(&mut state, &window) else {
                        return;
                    };
                    let activation = state.globals.activation.as_ref().unwrap();
                    activation.activate(token, &window.surface());
                }
                None => log::error!("activation token received with no pending activation"),
            }
        }

        token.destroy();
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for WaylandClientStatePtr {
    fn event(
        state: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(capabilities),
        } = event
        {
            let client = state.get_client();
            let mut state = client.borrow_mut();
            if capabilities.contains(wl_seat::Capability::Keyboard) {
                let keyboard = seat.get_keyboard(qh, ());

                state.text_input = state
                    .globals
                    .text_input_manager
                    .as_ref()
                    .map(|text_input_manager| text_input_manager.get_text_input(seat, qh, ()));

                if let Some(wl_keyboard) = &state.wl_keyboard {
                    wl_keyboard.release();
                }

                state.wl_keyboard = Some(keyboard);
            }
            if capabilities.contains(wl_seat::Capability::Pointer) {
                let pointer = seat.get_pointer(qh, ());
                state.cursor_shape_device = state
                    .globals
                    .cursor_shape_manager
                    .as_ref()
                    .map(|cursor_shape_manager| cursor_shape_manager.get_pointer(&pointer, qh, ()));

                if let Some(pinch) = state.pinch.take() {
                    pinch.destroy();
                }
                state.pinch = state
                    .globals
                    .pointer_gestures
                    .as_ref()
                    .map(|g| g.get_pinch_gesture(&pointer, qh, ()));

                if let Some(wl_pointer) = &state.wl_pointer {
                    wl_pointer.release();
                }

                state.wl_pointer = Some(pointer);
            }
        }
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for WaylandClientStatePtr {
    fn event(
        this: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let mut client = this.get_client();
        let mut state = client.borrow_mut();
        match event {
            wl_keyboard::Event::RepeatInfo { rate, delay } => {
                state.repeat.characters_per_second = rate as u32;
                state.repeat.delay = Duration::from_millis(delay as u64);
            }
            wl_keyboard::Event::Keymap {
                format: WEnum::Value(format),
                fd,
                size,
                ..
            } => {
                if format != wl_keyboard::KeymapFormat::XkbV1 {
                    log::error!("Received keymap format {:?}, expected XkbV1", format);
                    return;
                }
                let xkb_context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
                let keymap = unsafe {
                    xkb::Keymap::new_from_fd(
                        &xkb_context,
                        fd,
                        size as usize,
                        XKB_KEYMAP_FORMAT_TEXT_V1,
                        KEYMAP_COMPILE_NO_FLAGS,
                    )
                    .log_err()
                    .flatten()
                    .expect("Failed to create keymap")
                };
                state.keymap_state = Some(xkb::State::new(&keymap));
                state.compose_state = get_xkb_compose_state(&xkb_context);
                drop(state);

                this.handle_keyboard_layout_change();
            }
            wl_keyboard::Event::Enter { surface, .. } => {
                state.keyboard_focused_window = get_window(&mut state, &surface.id());
                state.enter_token = Some(());

                if let Some(window) = state.keyboard_focused_window.clone() {
                    drop(state);
                    window.set_focused(true);
                }
            }
            wl_keyboard::Event::Leave { surface, .. } => {
                let keyboard_focused_window = get_window(&mut state, &surface.id());
                state.keyboard_focused_window = None;
                state.enter_token.take();
                crate::platform::RIGHT_CONTROL.store(false, std::sync::atomic::Ordering::Relaxed);
                // Prevent keyboard events from repeating after opening e.g. a file chooser and closing it quickly
                state.repeat.current_id += 1;

                if let Some(window) = keyboard_focused_window {
                    if let Some(ref mut compose) = state.compose_state {
                        compose.reset();
                    }
                    state.pre_edit_text.take();
                    drop(state);
                    window.handle_ime(ImeInput::DeleteText);
                    window.set_focused(false);
                }
            }
            wl_keyboard::Event::Modifiers {
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
                ..
            } => {
                let focused_window = state.keyboard_focused_window.clone();

                let keymap_state = state.keymap_state.as_mut().unwrap();
                let old_layout =
                    keymap_state.serialize_layout(xkbcommon::xkb::STATE_LAYOUT_EFFECTIVE);
                keymap_state.update_mask(mods_depressed, mods_latched, mods_locked, 0, 0, group);
                state.modifiers = Modifiers::from_xkb(keymap_state);
                let keymap_state = state.keymap_state.as_mut().unwrap();
                state.capslock = Capslock::from_xkb(keymap_state);

                let input = PlatformInput::ModifiersChanged(ModifiersChangedEvent {
                    modifiers: state.modifiers,
                    capslock: state.capslock,
                });
                drop(state);

                if let Some(focused_window) = focused_window {
                    focused_window.handle_input(input);
                }

                if group != old_layout {
                    this.handle_keyboard_layout_change();
                }
            }
            wl_keyboard::Event::Key {
                serial,
                key,
                state: WEnum::Value(key_state),
                ..
            } => {
                state.serial_tracker.update(SerialKind::KeyPress, serial);

                let focused_window = state.keyboard_focused_window.clone();
                let Some(focused_window) = focused_window else {
                    return;
                };

                let keymap_state = state.keymap_state.as_ref().unwrap();
                let keycode = Keycode::from(key + MIN_KEYCODE);
                let keysym = keymap_state.key_get_one_sym(keycode);
                // (KEY_RIGHTCTRL: app_server's ⌘.)
                if key == 97 {
                    crate::platform::RIGHT_CONTROL.store(
                        key_state == wl_keyboard::KeyState::Pressed,
                        std::sync::atomic::Ordering::Relaxed,
                    );
                }

                match key_state {
                    wl_keyboard::KeyState::Pressed if !keysym.is_modifier_key() => {
                        let mut keystroke =
                            Keystroke::from_xkb(keymap_state, state.modifiers, keycode);
                        if let Some(mut compose) = state.compose_state.take() {
                            compose.feed(keysym);
                            match compose.status() {
                                xkb::Status::Composing => {
                                    keystroke.key_char = None;
                                    state.pre_edit_text =
                                        compose.utf8().or(Keystroke::underlying_dead_key(keysym));
                                    let pre_edit =
                                        state.pre_edit_text.clone().unwrap_or(String::default());
                                    drop(state);
                                    focused_window.handle_ime(ImeInput::SetMarkedText(pre_edit));
                                    state = client.borrow_mut();
                                }

                                xkb::Status::Composed => {
                                    state.pre_edit_text.take();
                                    keystroke.key_char = compose.utf8();
                                    if let Some(keysym) = compose.keysym() {
                                        keystroke.key = xkb::keysym_get_name(keysym);
                                    }
                                }
                                xkb::Status::Cancelled => {
                                    let pre_edit = state.pre_edit_text.take();
                                    let new_pre_edit = Keystroke::underlying_dead_key(keysym);
                                    state.pre_edit_text = new_pre_edit.clone();
                                    drop(state);
                                    if let Some(pre_edit) = pre_edit {
                                        focused_window.handle_ime(ImeInput::InsertText(pre_edit));
                                    }
                                    if let Some(current_key) = new_pre_edit {
                                        focused_window
                                            .handle_ime(ImeInput::SetMarkedText(current_key));
                                    }
                                    compose.feed(keysym);
                                    state = client.borrow_mut();
                                }
                                _ => {}
                            }
                            state.compose_state = Some(compose);
                        }
                        let input = PlatformInput::KeyDown(KeyDownEvent {
                            keystroke: keystroke.clone(),
                            is_held: false,
                        });

                        state.repeat.current_id += 1;
                        state.repeat.current_keycode = Some(keycode);

                        let rate = state.repeat.characters_per_second;
                        let id = state.repeat.current_id;
                        state
                            .loop_handle
                            .insert_source(Timer::from_duration(state.repeat.delay), {
                                let input = PlatformInput::KeyDown(KeyDownEvent {
                                    keystroke,
                                    is_held: true,
                                });
                                move |_event, _metadata, this| {
                                    let mut client = this.get_client();
                                    let mut state = client.borrow_mut();
                                    let is_repeating = id == state.repeat.current_id
                                        && state.repeat.current_keycode.is_some()
                                        && state.keyboard_focused_window.is_some();

                                    if !is_repeating || rate == 0 {
                                        return TimeoutAction::Drop;
                                    }

                                    let focused_window =
                                        state.keyboard_focused_window.as_ref().unwrap().clone();

                                    drop(state);
                                    focused_window.handle_input(input.clone());

                                    TimeoutAction::ToDuration(Duration::from_secs(1) / rate)
                                }
                            })
                            .unwrap();

                        drop(state);
                        focused_window.handle_input(input);
                    }
                    wl_keyboard::KeyState::Released if !keysym.is_modifier_key() => {
                        let input = PlatformInput::KeyUp(KeyUpEvent {
                            keystroke: Keystroke::from_xkb(keymap_state, state.modifiers, keycode),
                        });

                        if state.repeat.current_keycode == Some(keycode) {
                            state.repeat.current_keycode = None;
                        }

                        drop(state);
                        focused_window.handle_input(input);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<zwp_text_input_v3::ZwpTextInputV3, ()> for WaylandClientStatePtr {
    fn event(
        this: &mut Self,
        text_input: &zwp_text_input_v3::ZwpTextInputV3,
        event: <zwp_text_input_v3::ZwpTextInputV3 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let client = this.get_client();
        let mut state = client.borrow_mut();
        match event {
            zwp_text_input_v3::Event::Enter { .. } => {
                drop(state);
                this.enable_ime();
            }
            zwp_text_input_v3::Event::Leave { .. } => {
                drop(state);
                this.disable_ime();
            }
            zwp_text_input_v3::Event::CommitString { text } => {
                state.composing = false;
                let Some(window) = state.keyboard_focused_window.clone() else {
                    return;
                };

                if let Some(commit_text) = text {
                    drop(state);
                    // IBus Intercepts keys like `a`, `b`, but those keys are needed for vim mode.
                    // We should only send ASCII characters to Zed, otherwise a user could remap a letter like `か` or `相`.
                    if commit_text.len() == 1 {
                        window.handle_input(PlatformInput::KeyDown(KeyDownEvent {
                            keystroke: Keystroke {
                                modifiers: Modifiers::default(),
                                key: commit_text.clone(),
                                key_char: Some(commit_text),
                            },
                            is_held: false,
                        }));
                    } else {
                        window.handle_ime(ImeInput::InsertText(commit_text));
                    }
                }
            }
            zwp_text_input_v3::Event::PreeditString { text, .. } => {
                state.composing = true;
                state.ime_pre_edit = text;
            }
            zwp_text_input_v3::Event::Done { serial } => {
                let last_serial = state.serial_tracker.get(SerialKind::InputMethod);
                state.serial_tracker.update(SerialKind::InputMethod, serial);
                let Some(window) = state.keyboard_focused_window.clone() else {
                    return;
                };

                if let Some(text) = state.ime_pre_edit.take() {
                    drop(state);
                    window.handle_ime(ImeInput::SetMarkedText(text));
                    if let Some(area) = window.get_ime_area() {
                        text_input.set_cursor_rectangle(
                            area.origin.x.0 as i32,
                            area.origin.y.0 as i32,
                            area.size.width.0 as i32,
                            area.size.height.0 as i32,
                        );
                        if last_serial == serial {
                            text_input.commit();
                        }
                    }
                } else {
                    state.composing = false;
                    drop(state);
                    window.handle_ime(ImeInput::DeleteText);
                }
            }
            _ => {}
        }
    }
}

fn linux_button_to_gpui(button: u32) -> Option<MouseButton> {
    // These values are coming from <linux/input-event-codes.h>.
    const BTN_LEFT: u32 = 0x110;
    const BTN_RIGHT: u32 = 0x111;
    const BTN_MIDDLE: u32 = 0x112;
    const BTN_SIDE: u32 = 0x113;
    const BTN_EXTRA: u32 = 0x114;
    const BTN_FORWARD: u32 = 0x115;
    const BTN_BACK: u32 = 0x116;

    Some(match button {
        BTN_LEFT => MouseButton::Left,
        BTN_RIGHT => MouseButton::Right,
        BTN_MIDDLE => MouseButton::Middle,
        BTN_BACK | BTN_SIDE => MouseButton::Navigate(NavigationDirection::Back),
        BTN_FORWARD | BTN_EXTRA => MouseButton::Navigate(NavigationDirection::Forward),
        _ => return None,
    })
}

impl Dispatch<wl_pointer::WlPointer, ()> for WaylandClientStatePtr {
    fn event(
        this: &mut Self,
        wl_pointer: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let mut client = this.get_client();
        let mut state = client.borrow_mut();

        match event {
            wl_pointer::Event::Enter {
                serial,
                surface,
                surface_x,
                surface_y,
                ..
            } => {
                state.serial_tracker.update(SerialKind::MouseEnter, serial);
                state.mouse_location = Some(point(px(surface_x as f32), px(surface_y as f32)));
                state.button_pressed = None;

                if let Some(window) = get_window(&mut state, &surface.id()) {
                    state.mouse_focused_window = Some(window.clone());

                    if state.enter_token.is_some() {
                        state.enter_token = None;
                    }
                    if let Some(style) = state.cursor_style {
                        if let CursorStyle::None = style {
                            let wl_pointer = state
                                .wl_pointer
                                .clone()
                                .expect("window is focused by pointer");
                            wl_pointer.set_cursor(serial, None, 0, 0);
                        } else if let Some(cursor_shape_device) = &state.cursor_shape_device {
                            cursor_shape_device.set_shape(serial, style.to_shape());
                        } else {
                            let scale = window.primary_output_scale();
                            state
                                .cursor
                                .set_icon(wl_pointer, serial, style.to_icon_names(), scale);
                        }
                    }
                    // Where the pointer came in is where it is: hover and
                    // the cursor follow from now, not from its first move
                    // (a window opened or brought forward under a still
                    // pointer: Olo's app_server enters it as it shows).
                    let moved = PlatformInput::MouseMove(MouseMoveEvent {
                        position: point(px(surface_x as f32), px(surface_y as f32)),
                        pressed_button: None,
                        modifiers: state.modifiers,
                    });
                    drop(state);
                    window.set_hovered(true);
                    window.handle_input(moved);
                }
            }
            wl_pointer::Event::Leave { .. } => {
                if let Some(focused_window) = state.mouse_focused_window.clone() {
                    let input = PlatformInput::MouseExited(MouseExitEvent {
                        position: state.mouse_location.unwrap(),
                        pressed_button: state.button_pressed,
                        modifiers: state.modifiers,
                    });
                    state.mouse_focused_window = None;
                    state.mouse_location = None;
                    state.button_pressed = None;

                    drop(state);
                    focused_window.handle_input(input);
                    focused_window.set_hovered(false);
                }
            }
            wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            } => {
                if state.mouse_focused_window.is_none() {
                    return;
                }
                state.mouse_location = Some(point(px(surface_x as f32), px(surface_y as f32)));

                if let Some(window) = state.mouse_focused_window.clone() {
                    if state
                        .keyboard_focused_window
                        .as_ref()
                        .is_some_and(|keyboard_window| window.ptr_eq(keyboard_window))
                    {
                        state.enter_token = None;
                    }
                    let input = PlatformInput::MouseMove(MouseMoveEvent {
                        position: state.mouse_location.unwrap(),
                        pressed_button: state.button_pressed,
                        modifiers: state.modifiers,
                    });
                    drop(state);
                    window.handle_input(input);
                }
            }
            wl_pointer::Event::Button {
                serial,
                button,
                state: WEnum::Value(button_state),
                ..
            } => {
                state.serial_tracker.update(SerialKind::MousePress, serial);
                let button = linux_button_to_gpui(button);
                let Some(button) = button else { return };
                if state.mouse_focused_window.is_none() {
                    return;
                }
                match button_state {
                    wl_pointer::ButtonState::Pressed => {
                        if let Some(window) = state.keyboard_focused_window.clone() {
                            if state.composing && state.text_input.is_some() {
                                drop(state);
                                // text_input_v3 don't have something like a reset function
                                this.disable_ime();
                                this.enable_ime();
                                window.handle_ime(ImeInput::UnmarkText);
                                state = client.borrow_mut();
                            } else if let (Some(text), Some(compose)) =
                                (state.pre_edit_text.take(), state.compose_state.as_mut())
                            {
                                compose.reset();
                                drop(state);
                                window.handle_ime(ImeInput::InsertText(text));
                                state = client.borrow_mut();
                            }
                        }
                        let click_elapsed = state.click.last_click.elapsed();

                        if click_elapsed < DOUBLE_CLICK_INTERVAL
                            && state
                                .click
                                .last_mouse_button
                                .is_some_and(|prev_button| prev_button == button)
                            && is_within_click_distance(
                                state.click.last_location,
                                state.mouse_location.unwrap(),
                            )
                        {
                            state.click.current_count += 1;
                        } else {
                            state.click.current_count = 1;
                        }

                        state.click.last_click = Instant::now();
                        state.click.last_mouse_button = Some(button);
                        state.click.last_location = state.mouse_location.unwrap();

                        state.button_pressed = Some(button);

                        if let Some(window) = state.mouse_focused_window.clone() {
                            let input = PlatformInput::MouseDown(MouseDownEvent {
                                button,
                                position: state.mouse_location.unwrap(),
                                modifiers: state.modifiers,
                                click_count: state.click.current_count,
                                first_mouse: state.enter_token.take().is_some(),
                            });
                            drop(state);
                            window.handle_input(input);
                        }
                    }
                    wl_pointer::ButtonState::Released => {
                        state.button_pressed = None;

                        if let Some(window) = state.mouse_focused_window.clone() {
                            let input = PlatformInput::MouseUp(MouseUpEvent {
                                button,
                                position: state.mouse_location.unwrap(),
                                modifiers: state.modifiers,
                                click_count: state.click.current_count,
                            });
                            drop(state);
                            window.handle_input(input);
                        }
                    }
                    _ => {}
                }
            }

            // Axis Events
            wl_pointer::Event::AxisSource {
                axis_source: WEnum::Value(axis_source),
            } => {
                state.axis_source = axis_source;
            }
            wl_pointer::Event::Axis {
                axis: WEnum::Value(axis),
                value,
                ..
            } => {
                if state.axis_source == AxisSource::Wheel {
                    return;
                }
                let axis = if state.modifiers.shift {
                    wl_pointer::Axis::HorizontalScroll
                } else {
                    axis
                };
                let axis_modifier = match axis {
                    wl_pointer::Axis::VerticalScroll => state.vertical_modifier,
                    wl_pointer::Axis::HorizontalScroll => state.horizontal_modifier,
                    _ => 1.0,
                };
                state.scroll_event_received = true;
                let scroll_delta = state
                    .continuous_scroll_delta
                    .get_or_insert(point(px(0.0), px(0.0)));
                // Olo: the fingers' own distance (surface pixels, as the
                // egui kit), not three times it.
                match axis {
                    wl_pointer::Axis::VerticalScroll => {
                        scroll_delta.y += px(value as f32 * axis_modifier);
                    }
                    wl_pointer::Axis::HorizontalScroll => {
                        scroll_delta.x += px(value as f32 * axis_modifier);
                    }
                    _ => unreachable!(),
                }
            }
            // Olo: the fingers lifted (or the glide after them ended).
            wl_pointer::Event::AxisStop { .. } => {
                state.scroll_stopped = true;
                state.scroll_event_received = true;
            }
            wl_pointer::Event::AxisDiscrete {
                axis: WEnum::Value(axis),
                discrete,
            } => {
                state.scroll_event_received = true;
                let axis = if state.modifiers.shift {
                    wl_pointer::Axis::HorizontalScroll
                } else {
                    axis
                };
                let axis_modifier = match axis {
                    wl_pointer::Axis::VerticalScroll => state.vertical_modifier,
                    wl_pointer::Axis::HorizontalScroll => state.horizontal_modifier,
                    _ => 1.0,
                };

                let scroll_delta = state.discrete_scroll_delta.get_or_insert(point(0.0, 0.0));
                match axis {
                    wl_pointer::Axis::VerticalScroll => {
                        scroll_delta.y += discrete as f32 * axis_modifier;
                    }
                    wl_pointer::Axis::HorizontalScroll => {
                        scroll_delta.x += discrete as f32 * axis_modifier;
                    }
                    _ => unreachable!(),
                }
            }
            wl_pointer::Event::AxisValue120 {
                axis: WEnum::Value(axis),
                value120,
            } => {
                state.scroll_event_received = true;
                let axis = if state.modifiers.shift {
                    wl_pointer::Axis::HorizontalScroll
                } else {
                    axis
                };
                let axis_modifier = match axis {
                    wl_pointer::Axis::VerticalScroll => state.vertical_modifier,
                    wl_pointer::Axis::HorizontalScroll => state.horizontal_modifier,
                    _ => unreachable!(),
                };

                let scroll_delta = state.discrete_scroll_delta.get_or_insert(point(0.0, 0.0));
                let wheel_percent = value120 as f32 / 120.0;
                match axis {
                    wl_pointer::Axis::VerticalScroll => {
                        scroll_delta.y += wheel_percent * axis_modifier;
                    }
                    wl_pointer::Axis::HorizontalScroll => {
                        scroll_delta.x += wheel_percent * axis_modifier;
                    }
                    _ => unreachable!(),
                }
            }
            wl_pointer::Event::Frame => {
                if state.scroll_event_received {
                    state.scroll_event_received = false;
                    let continuous = state.continuous_scroll_delta.take();
                    let discrete = state.discrete_scroll_delta.take();
                    let stopped = std::mem::take(&mut state.scroll_stopped);
                    let wheel =
                        matches!(state.axis_source, AxisSource::Wheel | AxisSource::WheelTilt);
                    let (Some(window), Some(position)) =
                        (state.mouse_focused_window.clone(), state.mouse_location)
                    else {
                        return;
                    };
                    let modifiers = state.modifiers;
                    if let Some(discrete) = discrete.filter(|_| wheel) {
                        // Olo: a wheel's notches eased in over the frames
                        // to come, as the egui kit's (48 points a notch).
                        drop(state);
                        window.add_wheel(
                            point(discrete.x * WHEEL_NOTCH, discrete.y * WHEEL_NOTCH),
                            position,
                            modifiers,
                        );
                        return;
                    }
                    // Fingers on the touchpad, then (app_server's glide,
                    // as `Continuous`) the glide after them: each from its
                    // first move to its stop.
                    let mut out = Vec::new();
                    let kind = match state.axis_source {
                        AxisSource::Finger => crate::ScrollKind::Fingers,
                        AxisSource::Continuous => crate::ScrollKind::Glide,
                        _ => crate::ScrollKind::Wheel,
                    };
                    if let Some(continuous) =
                        continuous.filter(|d| d.x != px(0.0) || d.y != px(0.0))
                    {
                        // Olo: the glide taking over from the fingers with
                        // no stop between: the fingers' scroll ends (they
                        // lifted), the glide's starts.
                        if state.scrolling && state.scroll_kind != kind {
                            out.push(ScrollWheelEvent {
                                position,
                                delta: ScrollDelta::Pixels(point(px(0.0), px(0.0))),
                                modifiers,
                                touch_phase: TouchPhase::Ended,
                                kind: state.scroll_kind,
                            });
                            state.scrolling = false;
                        }
                        state.scroll_kind = kind;
                        let phase = if state.scrolling {
                            TouchPhase::Moved
                        } else {
                            TouchPhase::Started
                        };
                        state.scrolling = true;
                        out.push(ScrollWheelEvent {
                            position,
                            delta: ScrollDelta::Pixels(continuous),
                            modifiers,
                            touch_phase: phase,
                            kind,
                        });
                    } else if let Some(discrete) = discrete {
                        let lines = point(discrete.x * SCROLL_LINES, discrete.y * SCROLL_LINES);
                        out.push(ScrollWheelEvent {
                            position,
                            delta: ScrollDelta::Lines(lines),
                            modifiers,
                            touch_phase: TouchPhase::Moved,
                            kind: crate::ScrollKind::Wheel,
                        });
                    }
                    if stopped && std::mem::take(&mut state.scrolling) {
                        out.push(ScrollWheelEvent {
                            position,
                            delta: ScrollDelta::Pixels(point(px(0.0), px(0.0))),
                            modifiers,
                            touch_phase: TouchPhase::Ended,
                            kind,
                        });
                    }
                    drop(state);
                    for e in out {
                        window.handle_input(PlatformInput::ScrollWheel(e));
                    }
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wp_fractional_scale_v1::WpFractionalScaleV1, ObjectId> for WaylandClientStatePtr {
    fn event(
        this: &mut Self,
        _: &wp_fractional_scale_v1::WpFractionalScaleV1,
        event: <wp_fractional_scale_v1::WpFractionalScaleV1 as Proxy>::Event,
        surface_id: &ObjectId,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let client = this.get_client();
        let mut state = client.borrow_mut();

        let Some(window) = get_window(&mut state, surface_id) else {
            return;
        };

        drop(state);
        window.handle_fractional_scale_event(event);
    }
}

impl Dispatch<zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1, ObjectId>
    for WaylandClientStatePtr
{
    fn event(
        this: &mut Self,
        _: &zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1,
        event: zxdg_toplevel_decoration_v1::Event,
        surface_id: &ObjectId,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let client = this.get_client();
        let mut state = client.borrow_mut();
        let Some(window) = get_window(&mut state, surface_id) else {
            return;
        };

        drop(state);
        window.handle_toplevel_decoration_event(event);
    }
}

impl Dispatch<wl_data_device::WlDataDevice, ()> for WaylandClientStatePtr {
    fn event(
        this: &mut Self,
        _: &wl_data_device::WlDataDevice,
        event: wl_data_device::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let client = this.get_client();
        let mut state = client.borrow_mut();

        match event {
            // Clipboard
            wl_data_device::Event::DataOffer { id: data_offer } => {
                state.data_offers.push(DataOffer::new(data_offer));
                if state.data_offers.len() > 2 {
                    // At most we store a clipboard offer and a drag and drop offer.
                    state.data_offers.remove(0).inner.destroy();
                }
            }
            wl_data_device::Event::Selection { id: data_offer } => {
                if let Some(offer) = data_offer {
                    let offer = state
                        .data_offers
                        .iter()
                        .find(|wrapper| wrapper.inner.id() == offer.id());
                    let offer = offer.cloned();
                    state.clipboard.set_offer(offer);
                } else {
                    state.clipboard.set_offer(None);
                }
                // Olo: what Paste would take changed (menus enable it).
                if let Some(mut callback) = state.common.callbacks.clipboard_changed.take() {
                    drop(state);
                    callback();
                    client.borrow_mut().common.callbacks.clipboard_changed = Some(callback);
                }
            }

            // Drag and drop
            wl_data_device::Event::Enter {
                serial,
                surface,
                x,
                y,
                id: data_offer,
            } => {
                state.serial_tracker.update(SerialKind::DataDevice, serial);
                let Some(data_offer) = data_offer else { return };
                let Some(drag_window) = get_window(&mut state, &surface.id()) else {
                    return;
                };
                let position = Point::new(x.into(), y.into());
                // Olo: what it is, for a window that takes drops itself
                // (`Window::accept_drop`): its types and what its source
                // allows; our own drag out's straight from what we gave.
                let from_us = state.drag_out.is_some();
                let types: Vec<String> = match &state.drag_out {
                    Some(out) => out.types.iter().map(|(t, _)| t.clone()).collect(),
                    None => state
                        .data_offers
                        .iter()
                        .find(|o| o.inner.id() == data_offer.id())
                        .map(|o| o.mime_types().to_vec())
                        .unwrap_or_default(),
                };
                let source_actions = match &state.drag_out {
                    Some(out) => out.allowed,
                    None => state
                        .offer_actions
                        .get(&data_offer.id())
                        .copied()
                        .unwrap_or(DndAction::Copy),
                };
                state.drag.data_offer = Some(data_offer.clone());
                state.drag.window = Some(drag_window.clone());
                state.drag.position = position;
                state.drag.types = types.clone();
                state.drag.source_actions = source_actions;
                state.drag.serial = serial;
                state.drag.from_us = from_us;
                state.drag.accept = None;
                let mut offer = crate::DropOffer {
                    types: types.clone(),
                    copy: source_actions.contains(DndAction::Copy),
                    move_: source_actions.contains(DndAction::Move),
                    ask: source_actions.contains(DndAction::Ask),
                    from_us,
                    paths: Vec::new(),
                };
                // Our own drag out isn't taken back the platform's way
                // (an app dragged out of Deskbar onto its own row); a
                // window that takes drops itself decides.
                if from_us {
                    data_offer.accept(serial, None);
                    if data_offer.version() >= 3 {
                        data_offer.set_actions(DndAction::empty(), DndAction::empty());
                    }
                    if let Some(out) = &state.drag_out {
                        offer.paths = out
                            .types
                            .iter()
                            .find(|(t, _)| t == FILE_LIST_MIME_TYPE)
                            .map(|(_, d)| uri_paths(d))
                            .unwrap_or_default();
                    }
                    drop(state);
                    drag_window
                        .handle_input(PlatformInput::FileDrop(FileDropEvent::Offered(offer)));
                    return;
                }
                if !types.iter().any(|t| t == FILE_LIST_MIME_TYPE) {
                    // Not files: refused the platform's way (text from
                    // other programs), offered to a window that decides.
                    data_offer.accept(serial, None);
                    drop(state);
                    drag_window
                        .handle_input(PlatformInput::FileDrop(FileDropEvent::Offered(offer)));
                    return;
                }

                const ACTIONS: DndAction = DndAction::Copy;
                data_offer.set_actions(ACTIONS, ACTIONS);

                let pipe = Pipe::new().unwrap();
                data_offer.receive(FILE_LIST_MIME_TYPE.to_string(), unsafe {
                    BorrowedFd::borrow_raw(pipe.write.as_raw_fd())
                });
                let fd = pipe.read;
                drop(pipe.write);

                let read_task = state.common.background_executor.spawn(async {
                    let buffer = unsafe { read_fd(fd)? };
                    let text = String::from_utf8(buffer)?;
                    anyhow::Ok(text)
                });

                let this = this.clone();
                state
                    .common
                    .foreground_executor
                    .spawn(async move {
                        let file_list = match read_task.await {
                            Ok(list) => list,
                            Err(err) => {
                                log::error!("error reading drag and drop pipe: {err:?}");
                                return;
                            }
                        };

                        let paths: SmallVec<[_; 2]> = file_list
                            .lines()
                            .filter_map(|path| Url::parse(path).log_err())
                            .filter_map(|url| url.to_file_path().log_err())
                            .collect();

                        // (Gone meanwhile, or another drag now.)
                        {
                            let client = this.get_client();
                            let state = client.borrow();
                            if state.drag.data_offer.as_ref().map(|o| o.id())
                                != Some(data_offer.id())
                            {
                                return;
                            }
                        }
                        offer.paths = paths.iter().cloned().collect();
                        drag_window
                            .handle_input(PlatformInput::FileDrop(FileDropEvent::Offered(offer)));
                        if paths.is_empty() {
                            return;
                        }
                        drag_window.handle_input(PlatformInput::FileDrop(FileDropEvent::Entered {
                            position,
                            paths: crate::ExternalPaths(paths),
                        }));
                    })
                    .detach();
            }
            wl_data_device::Event::Motion { x, y, .. } => {
                let Some(drag_window) = state.drag.window.clone() else {
                    return;
                };
                let position = Point::new(x.into(), y.into());
                state.drag.position = position;

                let input = PlatformInput::FileDrop(FileDropEvent::Pending { position });
                drop(state);
                drag_window.handle_input(input);
            }
            wl_data_device::Event::Leave => {
                let Some(drag_window) = state.drag.window.clone() else {
                    return;
                };
                if let Some(data_offer) = state.drag.data_offer.take() {
                    state.offer_actions.remove(&data_offer.id());
                    data_offer.destroy();
                }
                state.drag.window = None;
                state.drag.accept = None;

                let input = PlatformInput::FileDrop(FileDropEvent::Exited {});
                drop(state);
                drag_window.handle_input(input);
            }
            wl_data_device::Event::Drop => {
                let Some(drag_window) = state.drag.window.clone() else {
                    return;
                };
                let Some(data_offer) = state.drag.data_offer.take() else {
                    return;
                };
                state.offer_actions.remove(&data_offer.id());
                state.drag.window = None;
                // Olo: a window that takes drops itself: what it took,
                // read (our own drag's from what we gave), then handed it.
                match state.drag.accept.take() {
                    Some(Some(accept)) => {
                        let (types, from_us, position) = (
                            std::mem::take(&mut state.drag.types),
                            state.drag.from_us,
                            state.drag.position,
                        );
                        let mime = accept.mime.clone();
                        let dropped = move |data: Vec<u8>| crate::Dropped {
                            target: accept.target.clone(),
                            mime: accept.mime.clone(),
                            data,
                            action: accept.action,
                            position,
                            types: types.clone(),
                            from_us,
                        };
                        if from_us {
                            let data = state
                                .drag_out
                                .as_ref()
                                .and_then(|out| out.types.iter().find(|(t, _)| *t == mime))
                                .map(|(_, d)| d.to_vec())
                                .unwrap_or_default();
                            data_offer.destroy();
                            drop(state);
                            drag_window.handle_input(PlatformInput::FileDrop(
                                FileDropEvent::Dropped(dropped(data)),
                            ));
                            return;
                        }
                        let pipe = Pipe::new().unwrap();
                        data_offer.receive(mime, unsafe {
                            BorrowedFd::borrow_raw(pipe.write.as_raw_fd())
                        });
                        let fd = pipe.read;
                        drop(pipe.write);
                        let read = state
                            .common
                            .background_executor
                            .spawn(async move { unsafe { read_fd(fd) } });
                        state
                            .common
                            .foreground_executor
                            .spawn(async move {
                                let data = read.await.unwrap_or_default();
                                if data_offer.version() >= 3 {
                                    data_offer.finish();
                                }
                                data_offer.destroy();
                                drag_window.handle_input(PlatformInput::FileDrop(
                                    FileDropEvent::Dropped(dropped(data)),
                                ));
                            })
                            .detach();
                        return;
                    }
                    Some(None) => {
                        data_offer.destroy();
                        drop(state);
                        drag_window.handle_input(PlatformInput::FileDrop(FileDropEvent::Exited));
                        return;
                    }
                    None => {}
                }
                if state.drag.from_us {
                    data_offer.destroy();
                    drop(state);
                    drag_window.handle_input(PlatformInput::FileDrop(FileDropEvent::Exited));
                    return;
                }
                data_offer.finish();
                data_offer.destroy();

                let input = PlatformInput::FileDrop(FileDropEvent::Submit {
                    position: state.drag.position,
                });
                drop(state);
                drag_window.handle_input(input);
            }
            _ => {}
        }
    }

    event_created_child!(WaylandClientStatePtr, wl_data_device::WlDataDevice, [
        wl_data_device::EVT_DATA_OFFER_OPCODE => (wl_data_offer::WlDataOffer, ()),
    ]);
}

impl Dispatch<wl_data_offer::WlDataOffer, ()> for WaylandClientStatePtr {
    fn event(
        this: &mut Self,
        data_offer: &wl_data_offer::WlDataOffer,
        event: wl_data_offer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let client = this.get_client();
        let mut state = client.borrow_mut();

        // Olo: what a drag's source allows, for a window that takes it.
        if let wl_data_offer::Event::SourceActions { source_actions } = event {
            if let wayland_client::WEnum::Value(actions) = source_actions {
                state.offer_actions.insert(data_offer.id(), actions);
            }
            return;
        }
        if let wl_data_offer::Event::Offer { mime_type } = event {
            // Drag and drop
            if mime_type == FILE_LIST_MIME_TYPE {
                let serial = state.serial_tracker.get(SerialKind::DataDevice);
                let mime_type = mime_type.clone();
                data_offer.accept(serial, Some(mime_type));
            }

            // Clipboard
            if let Some(offer) = state
                .data_offers
                .iter_mut()
                .find(|wrapper| wrapper.inner.id() == data_offer.id())
            {
                offer.add_mime_type(mime_type);
            }
        }
    }
}

impl Dispatch<wl_data_source::WlDataSource, ()> for WaylandClientStatePtr {
    fn event(
        this: &mut Self,
        data_source: &wl_data_source::WlDataSource,
        event: wl_data_source::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let client = this.get_client();
        let mut state = client.borrow_mut();

        // Olo: our drag out.
        if state
            .drag_out
            .as_ref()
            .is_some_and(|d| &d.source == data_source)
        {
            match event {
                wl_data_source::Event::Send { mime_type, fd } => {
                    let bytes = state
                        .drag_out
                        .as_ref()
                        .and_then(|d| d.types.iter().find(|(t, _)| *t == mime_type))
                        .map(|(_, b)| b.to_vec());
                    if let Some(bytes) = bytes {
                        state.clipboard.send_bytes(fd, bytes);
                    }
                }
                wl_data_source::Event::Action { dnd_action } => {
                    let a = dnd_action.into_result().unwrap_or(DndAction::empty());
                    let action = if a.contains(DndAction::Move) {
                        Some(crate::DragAction::Move)
                    } else if a.contains(DndAction::Copy) {
                        Some(crate::DragAction::Copy)
                    } else if a.contains(DndAction::Ask) {
                        Some(crate::DragAction::Ask)
                    } else {
                        None
                    };
                    if let Some(d) = &mut state.drag_out {
                        d.action = action;
                    }
                }
                // Dropped and taken, or not: over.
                wl_data_source::Event::DndFinished | wl_data_source::Event::Cancelled => {
                    let finished = matches!(event, wl_data_source::Event::DndFinished);
                    if let Some(mut d) = state.drag_out.take() {
                        if let Some((surface, buffer, pool)) = d.icon.take() {
                            surface.destroy();
                            buffer.destroy();
                            pool.destroy();
                        }
                        d.source.destroy();
                        if let Some(done) = d.done.take() {
                            let _ = done.send(if finished { d.action } else { None });
                        }
                    }
                }
                _ => {}
            }
            return;
        }
        match event {
            wl_data_source::Event::Send { mime_type, fd } => {
                state.clipboard.send(mime_type, fd);
            }
            wl_data_source::Event::Cancelled => {
                data_source.destroy();
                // Olo: something else was copied; quit and waiting for
                // that, the app ends.
                if state.selection_source.as_ref() == Some(data_source) {
                    state.selection_source = None;
                    if state.lingering {
                        state.common.signal.stop();
                    }
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1, ()>
    for WaylandClientStatePtr
{
    fn event(
        this: &mut Self,
        _: &zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1,
        event: zwp_primary_selection_device_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let client = this.get_client();
        let mut state = client.borrow_mut();

        match event {
            zwp_primary_selection_device_v1::Event::DataOffer { offer } => {
                let old_offer = state.primary_data_offer.replace(DataOffer::new(offer));
                if let Some(old_offer) = old_offer {
                    old_offer.inner.destroy();
                }
            }
            zwp_primary_selection_device_v1::Event::Selection { id: data_offer } => {
                if data_offer.is_some() {
                    let offer = state.primary_data_offer.clone();
                    state.clipboard.set_primary_offer(offer);
                } else {
                    state.clipboard.set_primary_offer(None);
                }
            }
            _ => {}
        }
    }

    event_created_child!(WaylandClientStatePtr, zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1, [
        zwp_primary_selection_device_v1::EVT_DATA_OFFER_OPCODE => (zwp_primary_selection_offer_v1::ZwpPrimarySelectionOfferV1, ()),
    ]);
}

impl Dispatch<zwp_primary_selection_offer_v1::ZwpPrimarySelectionOfferV1, ()>
    for WaylandClientStatePtr
{
    fn event(
        this: &mut Self,
        _data_offer: &zwp_primary_selection_offer_v1::ZwpPrimarySelectionOfferV1,
        event: zwp_primary_selection_offer_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let client = this.get_client();
        let mut state = client.borrow_mut();

        if let zwp_primary_selection_offer_v1::Event::Offer { mime_type } = event
            && let Some(offer) = state.primary_data_offer.as_mut()
        {
            offer.add_mime_type(mime_type);
        }
    }
}

impl Dispatch<zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1, ()>
    for WaylandClientStatePtr
{
    fn event(
        this: &mut Self,
        selection_source: &zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1,
        event: zwp_primary_selection_source_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let client = this.get_client();
        let mut state = client.borrow_mut();

        match event {
            zwp_primary_selection_source_v1::Event::Send { mime_type, fd } => {
                state.clipboard.send_primary(mime_type, fd);
            }
            zwp_primary_selection_source_v1::Event::Cancelled => {
                selection_source.destroy();
            }
            _ => {}
        }
    }
}

impl WaylandClientStatePtr {
    /// Olo: asks for an activation token for `surface` with the last
    /// press's serial; `done` gets it (on the main thread, later).
    /// Olo: `done` gets the handle of the window on `surface` once it's
    /// exported (`Window::export_handle`); dropped if it closes first.
    pub fn wait_for_export(&self, surface: ObjectId, done: Box<dyn FnOnce(Option<String>)>) {
        self.get_client()
            .borrow_mut()
            .export_requests
            .entry(surface)
            .or_default()
            .push(done);
    }

    /// Olo: a window closed: nothing waits for its handle any more.
    pub fn forget_export(&self, surface: &ObjectId) {
        self.get_client()
            .borrow_mut()
            .export_requests
            .remove(surface);
    }

    pub fn request_activation_token(
        &self,
        surface: &wl_surface::WlSurface,
        app_id: Option<String>,
        done: Box<dyn FnOnce(Option<String>)>,
    ) {
        let client = self.get_client();
        let mut state = client.borrow_mut();
        let Some(activation) = state.globals.activation.clone() else {
            drop(state);
            return done(None);
        };
        let id = state.next_token_request;
        state.next_token_request += 1;
        state.token_requests.insert(id, done);
        let token = activation.get_activation_token(&state.globals.qh, id);
        let serial = state.serial_tracker.get(SerialKind::MousePress);
        token.set_serial(serial, &state.wl_seat);
        token.set_surface(surface);
        if let Some(app_id) = app_id {
            token.set_app_id(app_id);
        }
        token.commit();
    }
}

/// Olo: a token asked for with `request_activation_token`.
impl Dispatch<xdg_activation_token_v1::XdgActivationTokenV1, u64> for WaylandClientStatePtr {
    fn event(
        this: &mut Self,
        token: &xdg_activation_token_v1::XdgActivationTokenV1,
        event: xdg_activation_token_v1::Event,
        id: &u64,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_activation_token_v1::Event::Done { token: made } = event {
            let client = this.get_client();
            let mut state = client.borrow_mut();
            if let Some(done) = state.token_requests.remove(id) {
                // (Called once the state is free: it may open windows.)
                state
                    .common
                    .foreground_executor
                    .spawn(async move { done(Some(made)) })
                    .detach();
            }
        }
        token.destroy();
    }
}

/// (Olo) Whether the connection's socket is hung up or in error, asked
/// without waiting.
fn hung_up(connection: &Connection) -> bool {
    use std::os::fd::{AsFd as _, AsRawFd as _};
    let mut fd = libc::pollfd {
        fd: connection.as_fd().as_raw_fd(),
        events: 0,
        revents: 0,
    };
    // SAFETY: one pollfd, no wait.
    unsafe {
        libc::poll(&mut fd, 1, 0) == 1
            && fd.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0
    }
}

/// (Olo) The compositor's connection. In an Olo session `WAYLAND_DISPLAY`
/// is olo-wlproxy's socket, which keeps outside programs' connections
/// alive across app_server restarts by replaying their objects; Olo's
/// own programs connect to app_server's direct socket beside it,
/// `olo-<display>-direct`, and come back by themselves (`ON_COMPOSITOR_LOST`: re-executed with their state). Through
/// the proxy a GPUI app kept its old configures, outputs and frame state
/// over a replay and was left with dead windows (2026-11-01).
fn connect() -> Result<Connection, wayland_client::ConnectError> {
    if std::env::var_os("OLO_SESSION").is_some()
        && let (Some(dir), Ok(display)) = (
            std::env::var_os("XDG_RUNTIME_DIR"),
            std::env::var("WAYLAND_DISPLAY"),
        )
        && !display.starts_with('/')
    {
        let direct = std::path::Path::new(&dir).join(format!("olo-{display}-direct"));
        if let Ok(stream) = std::os::unix::net::UnixStream::connect(&direct) {
            return Connection::from_socket(stream);
        }
    }
    Connection::connect_to_env()
}

/// (Olo) The compositor's connection gone (hung up, or in protocol
/// error), or the GPU device lost (`gpu_device_lost`): told once to whoever asked (`Platform::on_compositor_lost`:
/// olo-ui re-executes the program with its state saved, and doesn't
/// come back), with no borrow held; then the loop stops. Nothing can
/// come of the connection again, and its socket, readable for ever,
/// would wake the loop at once each turn (a program outside a session,
/// where nobody re-executes, spun printing the error, 2026-11-01).
fn compositor_lost_check(state: &Rc<RefCell<WaylandClientState>>) {
    let (dead, gpu) = {
        let st = state.borrow();
        let gpu = crate::gpu_device_lost();
        (
            !st.lost
                && (gpu || st.connection.protocol_error().is_some() || hung_up(&st.connection)),
            gpu,
        )
    };
    if !dead {
        return;
    }
    state.borrow_mut().lost = true;
    if gpu {
        // (A driver reset: the windows can't be drawn again, so the
        // program starts anew as it does for the compositor gone.)
        log::error!("the GPU device is lost");
    } else {
        log::error!("the compositor's connection is gone");
    }
    let callback = state.borrow_mut().common.callbacks.compositor_lost.take();
    if let Some(mut f) = callback {
        f();
        state.borrow_mut().common.callbacks.compositor_lost = Some(f);
    }
    state.borrow().common.signal.stop();
}

/// Olo: the paths in a `text/uri-list`.
fn uri_paths(data: &[u8]) -> Vec<std::path::PathBuf> {
    String::from_utf8_lossy(data)
        .lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| crate::file_uri_path(l.trim()))
        .collect()
}
