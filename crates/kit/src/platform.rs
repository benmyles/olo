//! The Wayland side of the kit: one connection, a calloop event loop, a
//! shared wgpu device, and one egui context per surface (windows, layer
//! surfaces and popup menus).

use std::{
    any::Any,
    io::{Read, Write},
    ptr::NonNull,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use egui::{Pos2, Rect, ViewportId, pos2, vec2};
use raw_window_handle::{RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, FrameCallbackData},
    data_device_manager::{
        DataDeviceManagerState, WritePipe,
        data_device::{DataDevice, DataDeviceHandler},
        data_offer::{DataOfferHandler, DragOffer},
        data_source::{CopyPasteSource, DataSourceHandler},
    },
    delegate_registry,
    dispatch2::Dispatch2,
    output::{OutputHandler, OutputState},
    reexports::{
        calloop::{
            EventLoop, LoopHandle,
            channel::{self, Channel},
        },
        calloop_wayland_source::WaylandSource,
        client::{
            Connection, Proxy as _, QueueHandle,
            globals::{GlobalList, registry_queue_init},
            protocol::{
                wl_data_device::WlDataDevice, wl_data_device_manager::DndAction, wl_data_source::WlDataSource,
                wl_keyboard::WlKeyboard, wl_output, wl_pointer::WlPointer, wl_seat, wl_surface::WlSurface,
            },
        },
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        Capability, SeatHandler, SeatState,
        keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers, RawModifiers},
        pointer::{PointerEvent, PointerEventKind, PointerHandler, ThemeSpec, ThemedPointer},
    },
    shell::{
        WaylandSurface,
        wlr_layer::{Anchor, KeyboardInteractivity, LayerShell, LayerShellHandler, LayerSurface, LayerSurfaceConfigure},
        xdg::{
            XdgPositioner, XdgShell, XdgSurface,
            popup::{Popup, PopupConfigure, PopupHandler},
            window::{Window, WindowConfigure, WindowDecorations, WindowHandler},
        },
    },
    shm::{Shm, ShmHandler},
};
use tabula_theme::Palette;
use tracing::{debug, info, warn};
use wayland_protocols::{
    wp::{
        fractional_scale::v1::client::{
            wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1,
            wp_fractional_scale_v1::{self, WpFractionalScaleV1},
        },
        idle_inhibit::zv1::client::{zwp_idle_inhibit_manager_v1::ZwpIdleInhibitManagerV1, zwp_idle_inhibitor_v1::ZwpIdleInhibitorV1},
        viewporter::client::{wp_viewport::WpViewport, wp_viewporter::WpViewporter},
    },
    xdg::shell::client::xdg_positioner,
};

use crate::{
    app::{App, Clip, Cx, EmbedId, Embedded, Layer, Proxy, Request, WindowId, WindowKind, WindowOptions},
    input, menu,
    menu::{BarState, Menu, MenuAnchor, MenuRequest, PopupResult, PopupState},
    style,
};

// ---- GPU ----------------------------------------------------------------

/// Popup path root meaning "the surface's ad-hoc menu", not a bar menu.
const CONTEXT: usize = usize::MAX;

fn menu_of(s: &Surface, path: &[usize]) -> Option<Menu> {
    let root = if path[0] == CONTEXT {
        s.context.as_ref().map(|r| &r.menu)
    } else {
        s.menus.as_ref().and_then(|m| m.get(path[0]))
    };
    root.and_then(|m| m.at_path(&path[1..])).cloned()
}

struct Gpu {
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
}

// ---- surfaces -----------------------------------------------------------

enum Role {
    Toplevel(Window),
    Layer(LayerSurface),
    /// Covers one output while the session is locked (held: dropping it
    /// destroys the lock surface).
    /// The output it covers (dropped with it if the output goes).
    Lock(#[allow(dead_code)] smithay_client_toolkit::session_lock::SessionLockSurface, wl_output::WlOutput),
    Popup {
        popup: Popup,
        /// The window whose menu bar (or context menu) this belongs to.
        owner: WindowId,
        /// Menu index in the owner's bar, then submenu indices.
        path: Vec<usize>,
        state: PopupState,
    },
}

/// A subsurface another renderer draws (see `Cx::embed`).
struct Embed {
    id: EmbedId,
    window: WindowId,
    wl: WlSurface,
    sub: wayland_client::protocol::wl_subsurface::WlSubsurface,
    viewport: Option<WpViewport>,
    rect: Option<egui::Rect>,
    /// The scale the app was last told.
    scale: f64,
}

struct Surface {
    id: WindowId,
    role: Role,
    wl: WlSurface,
    options: WindowOptions,
    gpu_surface: Option<wgpu::Surface<'static>>,
    format: wgpu::TextureFormat,
    renderer: Option<egui_wgpu::Renderer>,
    ctx: egui::Context,
    /// Which set of document fonts `ctx` has (see `style::add_document_font`).
    fonts: u64,
    /// Logical size.
    size: (u32, u32),
    /// Device pixels per logical pixel.
    scale: f64,
    fractional: Option<WpFractionalScaleV1>,
    viewport: Option<WpViewport>,
    /// Held while the window asks the screen to stay awake.
    inhibitor: Option<ZwpIdleInhibitorV1>,
    configured: bool,
    dirty: bool,
    frame_pending: bool,
    repaint_at: Option<Instant>,
    events: Vec<egui::Event>,
    pointer: Option<Pos2>,
    /// Buttons pressed on this surface and not yet released here.
    buttons: Vec<egui::PointerButton>,
    /// A press that closed this window's menus; it and its release are
    /// not passed on (clicking outside a menu only dismisses it, as on BeOS).
    swallow: bool,
    /// The menu bar, in window coordinates, from the last draw.
    bar_rect: Option<Rect>,
    cursor: egui::CursorIcon,
    // Menus (toplevels only).
    menus: Option<Vec<Menu>>,
    bar: BarState,
    /// Popup ids opened from this window, outermost first.
    popups: Vec<WindowId>,
    /// The ad-hoc popup menu (context menu, menu field) last requested.
    context: Option<MenuRequest>,
    start: Instant,
}

impl Surface {
    fn physical_size(&self) -> (u32, u32) {
        (
            ((self.size.0 as f64) * self.scale).round().max(1.0) as u32,
            ((self.size.1 as f64) * self.scale).round().max(1.0) as u32,
        )
    }
}

/// User data for our own protocol objects.
/// Where egui's repaint callbacks send (window, delay).
static REPAINT: std::sync::OnceLock<Mutex<channel::Sender<(WindowId, Duration)>>> = std::sync::OnceLock::new();
/// Clipboard text read on a helper thread, for the surface that pasted.
static PASTED: std::sync::OnceLock<Mutex<channel::Sender<(WlSurface, String)>>> = std::sync::OnceLock::new();

pub struct KitData;
pub struct ScaleData(WlSurface);

// ---- the state ----------------------------------------------------------

pub struct Kit {
    conn: Connection,
    qh: QueueHandle<Kit>,
    loop_handle: LoopHandle<'static, Kit>,
    registry_state: RegistryState,
    seat_state: SeatState,
    output_state: OutputState,
    compositor: CompositorState,
    xdg: XdgShell,
    layer_shell: Option<LayerShell>,
    shm: Shm,
    data_device_manager: Option<DataDeviceManagerState>,
    activation: Option<smithay_client_toolkit::activation::ActivationState>,
    session_lock_state: smithay_client_toolkit::session_lock::SessionLockState,
    session_lock: Option<smithay_client_toolkit::session_lock::SessionLock>,
    fractional_manager: Option<WpFractionalScaleManagerV1>,
    viewporter: Option<WpViewporter>,
    idle_inhibit: Option<ZwpIdleInhibitManagerV1>,
    subcompositor: Option<smithay_client_toolkit::subcompositor::SubcompositorState>,
    embeds: Vec<Embed>,
    /// Embeds whose handles the app hasn't been given yet (or anew).
    announce: Vec<EmbedId>,

    gpu: Option<Gpu>,
    surfaces: Vec<Surface>,
    app: Option<Box<dyn App>>,
    cx: Option<Cx>,
    palette: Arc<Palette>,

    seat: Option<wl_seat::WlSeat>,
    pointer: Option<ThemedPointer>,
    keyboard: Option<WlKeyboard>,
    data_device: Option<DataDevice>,
    pointer_focus: Option<WlSurface>,
    keyboard_focus: Option<WlSurface>,
    modifiers: egui::Modifiers,
    last_serial: u32,
    /// What we put on the clipboard: the source and its data by type.
    clipboard: Option<(CopyPasteSource, Clip)>,
    exit: bool,
    /// Windows all closed, but still serving the clipboard (see `finish`).
    linger: bool,
}

/// Run an app until it quits or closes its last window.
///
/// Inside a Tabula session (`$TABULA_SESSION` set by tabula-session) the
/// app survives app_server restarting in place: when the connection drops
/// it reconnects and recreates its windows, keeping all its state, or, if
/// its own binary was upgraded meanwhile, re-executes the new one with the
/// state from [`App::save_state`].
pub fn run<A: App>(app_id: &str, build: impl FnOnce(&mut Cx) -> A) {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .try_init();
    // A client handed its connection (WAYLAND_SOCKET, as app_server starts
    // the lock screen) has nothing to reconnect to: it ends with app_server.
    let in_session = std::env::var_os("TABULA_SESSION").is_some() && std::env::var_os("WAYLAND_SOCKET").is_none();
    let binary = Binary::current();

    let mut event_loop: EventLoop<'static, Kit> = EventLoop::try_new().expect("event loop");
    let loop_handle = event_loop.handle();

    // Messages from app threads.
    let (sender, channel): (_, Channel<Box<dyn Any + Send>>) = channel::channel();
    loop_handle
        .insert_source(channel, |event, _, kit: &mut Kit| {
            if let channel::Event::Msg(msg) = event {
                kit.with_app(|app, cx| app.message(cx, msg));
            }
        })
        .expect("channel");

    // Repaints asked for from other threads (`ctx.request_repaint()` on a
    // worker): egui only calls back, so wake the loop and schedule them.
    let (repaint_tx, repaint_rx) = channel::channel::<(WindowId, Duration)>();
    let _ = REPAINT.set(Mutex::new(repaint_tx));
    loop_handle
        .insert_source(repaint_rx, |event, _, kit: &mut Kit| {
            if let channel::Event::Msg((id, delay)) = event
                && let Some(s) = kit.surface_mut(id)
            {
                let at = Instant::now() + delay;
                s.repaint_at = Some(s.repaint_at.map_or(at, |t| t.min(at)));
            }
        })
        .expect("repaint channel");

    let (pasted_tx, pasted_rx) = channel::channel::<(WlSurface, String)>();
    let _ = PASTED.set(Mutex::new(pasted_tx));
    loop_handle
        .insert_source(pasted_rx, |event, _, kit: &mut Kit| {
            if let channel::Event::Msg((wl, text)) = event {
                kit.push_event(&wl, egui::Event::Paste(text));
            }
        })
        .expect("paste channel");

    // Live theme from app_server.
    let palette = Arc::new(crate::theme_watch::initial_palette());
    let (theme_tx, theme_rx) = channel::channel::<Palette>();
    crate::theme_watch::spawn(theme_tx);
    loop_handle
        .insert_source(theme_rx, |event, _, kit: &mut Kit| {
            if let channel::Event::Msg(p) = event {
                kit.set_palette(Arc::new(p));
            }
        })
        .expect("theme channel");

    let mut build = Some(build);
    let mut carried: Option<Carried> = None;
    let mut palette = palette;
    loop {
        let Some(conn) = connect(in_session) else {
            warn!("could not reconnect to app_server; exiting");
            return;
        };
        let (globals, mut event_queue) = registry_queue_init::<Kit>(&conn).expect("wayland registry");
        let qh = event_queue.handle();
        let mut kit = Kit::new(&globals, conn.clone(), qh, loop_handle.clone(), palette.clone());
        // Learn the outputs (wl_output, then xdg-output) before the app
        // starts, so its first windows can be sized for the screen.
        for _ in 0..2 {
            if event_queue.roundtrip(&mut kit).is_err() {
                break;
            }
        }
        let wayland = WaylandSource::new(conn.clone(), event_queue)
            .insert(loop_handle.clone())
            .expect("wayland source");

        match carried.take() {
            None => {
                let mut cx = Cx {
                    requests: Vec::new(),
                    next_id: 1,
                    palette: palette.clone(),
                    proxy: Proxy { sender: sender.clone() },
                    app_id: app_id.to_owned(),
                    sizes: Default::default(),
                    screen: kit.screen_size(),
                    outputs: kit.output_names(),
                    resident: false,
                };
                let app = (build.take().expect("built once"))(&mut cx);
                kit.app = Some(Box::new(app));
                kit.cx = Some(cx);
            }
            Some(c) => {
                info!(windows = c.windows.len(), "reconnected; restoring windows");
                kit.app = Some(c.app);
                kit.cx = Some(c.cx);
                for (id, options) in c.windows {
                    kit.open(id, options);
                }
                for (id, window, rect) in c.embeds {
                    kit.create_embed(window, id, rect);
                }
            }
        }
        kit.process_requests();

        let mut lost = false;
        while !kit.exit {
            let timeout = kit.next_timeout();
            if let Err(e) = event_loop.dispatch(timeout, &mut kit) {
                warn!(?e, "lost the connection to app_server");
                lost = true;
                break;
            }
            kit.redraw_due();
            kit.check_embed_scales();
            if kit.conn.flush().is_err() {
                lost = true;
                break;
            }
        }
        loop_handle.remove(wayland);
        palette = kit.palette.clone();
        let c = kit.into_carried();
        if !lost || !in_session {
            return;
        }
        // app_server is restarting (tabula-session reload). If our binary
        // was upgraded meanwhile, become the new version; otherwise just
        // reconnect with everything as it was.
        let Some(mut c) = c else { return };
        if binary.changed() {
            c.cx.sizes = c.windows.iter().map(|(id, o)| (*id, o.size)).collect();
            let state = c.app.save_state(&c.cx);
            binary.exec(state.as_deref());
            warn!("could not start the upgraded binary; carrying on");
        }
        carried = Some(c);
    }
}

/// What survives a reconnect: the app itself and its open windows.
struct Carried {
    app: Box<dyn App>,
    cx: Cx,
    windows: Vec<(WindowId, WindowOptions)>,
    /// Embedded surfaces, made again on the new connection.
    embeds: Vec<(EmbedId, WindowId, Option<egui::Rect>)>,
}

/// Connect to app_server; while it restarts, keep trying for a while.
fn connect(retry: bool) -> Option<Connection> {
    let deadline = Instant::now() + Duration::from_secs(if retry { 20 } else { 0 });
    loop {
        match Connection::connect_to_env() {
            Ok(c) => return Some(c),
            Err(e) if Instant::now() >= deadline => {
                if !retry {
                    panic!("not running under Wayland: {e}");
                }
                return None;
            }
            Err(_) => std::thread::sleep(Duration::from_millis(100)),
        }
    }
}

/// Our own executable, to notice when an upgrade replaces it.
struct Binary {
    path: std::path::PathBuf,
    id: Option<(u64, u64, std::time::SystemTime)>,
}

impl Binary {
    fn current() -> Binary {
        let path = std::env::current_exe().unwrap_or_default();
        let id = Self::identity(&path);
        Binary { path, id }
    }

    fn identity(path: &std::path::Path) -> Option<(u64, u64, std::time::SystemTime)> {
        use std::os::unix::fs::MetadataExt;
        let m = std::fs::metadata(path).ok()?;
        Some((m.dev(), m.ino(), m.modified().ok()?))
    }

    fn changed(&self) -> bool {
        let now = Self::identity(&self.path);
        now.is_some() && now != self.id
    }

    /// Replace this process with the binary now on disk, passing `state`.
    fn exec(&self, state: Option<&str>) {
        use std::os::unix::process::CommandExt;
        let mut cmd = std::process::Command::new(&self.path);
        cmd.args(std::env::args_os().skip(1));
        if let Some(state) = state {
            let dir = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);
            let file = dir.join(format!("tabula-restore-{}.json", std::process::id()));
            if std::fs::write(&file, state).is_ok() {
                cmd.env(crate::app::RESTORE_ENV, &file);
            }
        }
        info!(path = %self.path.display(), "upgraded: re-executing");
        let e = cmd.exec();
        warn!(?e, "exec failed");
    }
}

impl Kit {
    fn new(
        globals: &GlobalList,
        conn: Connection,
        qh: QueueHandle<Kit>,
        loop_handle: LoopHandle<'static, Kit>,
        palette: Arc<Palette>,
    ) -> Kit {
        let fractional_manager = globals.bind::<WpFractionalScaleManagerV1, _, _>(&qh, 1..=1, KitData).ok();
        let viewporter = globals.bind::<WpViewporter, _, _>(&qh, 1..=1, KitData).ok();
        let compositor = CompositorState::bind(globals, &qh).expect("wl_compositor");
        let subcompositor = smithay_client_toolkit::subcompositor::SubcompositorState::bind(compositor.wl_compositor().clone(), globals, &qh).ok();
        let idle_inhibit = globals.bind::<ZwpIdleInhibitManagerV1, _, _>(&qh, 1..=1, KitData).ok();
        Kit {
            registry_state: RegistryState::new(globals),
            seat_state: SeatState::new(globals, &qh),
            output_state: OutputState::new(globals, &qh),
            compositor,
            xdg: XdgShell::bind(globals, &qh).expect("xdg_wm_base"),
            layer_shell: LayerShell::bind(globals, &qh).ok(),
            shm: Shm::bind(globals, &qh).expect("wl_shm"),
            data_device_manager: DataDeviceManagerState::bind(globals, &qh).ok(),
            activation: smithay_client_toolkit::activation::ActivationState::bind(globals, &qh)
                .ok(),
            session_lock_state: smithay_client_toolkit::session_lock::SessionLockState::new(globals, &qh),
            session_lock: None,
            subcompositor,
            embeds: Vec::new(),
            announce: Vec::new(),
            fractional_manager,
            viewporter,
            idle_inhibit,
            conn,
            qh,
            loop_handle,
            gpu: None,
            surfaces: Vec::new(),
            app: None,
            cx: None,
            palette,
            seat: None,
            pointer: None,
            keyboard: None,
            data_device: None,
            pointer_focus: None,
            keyboard_focus: None,
            modifiers: egui::Modifiers::NONE,
            last_serial: 0,
            clipboard: None,
            exit: false,
            linger: false,
        }
    }

    /// Tear down every Wayland/GPU object (surfaces before the device and
    /// connection they belong to) and keep the app and its windows.
    fn into_carried(mut self) -> Option<Carried> {
        let embeds = self.embeds.iter().map(|e| (e.id, e.window, e.rect)).collect();
        self.lose_embeds(|_| true);
        let windows = self
            .surfaces
            .iter()
            .filter(|s| matches!(s.role, Role::Toplevel(_) | Role::Layer(_)))
            .map(|s| {
                let mut options = s.options.clone();
                if matches!(s.role, Role::Toplevel(_)) {
                    options.size = s.size; // come back the size it was
                }
                (s.id, options)
            })
            .collect();
        for s in self.surfaces.iter_mut() {
            s.gpu_surface = None;
            s.renderer = None;
        }
        self.surfaces.clear();
        self.gpu = None;
        let app = self.app.take()?;
        let mut cx = self.cx.take()?;
        cx.requests.clear();
        Some(Carried { app, cx, windows, embeds })
    }

    // ---- embedded surfaces ---------------------------------------------

    fn create_embed(&mut self, window: WindowId, id: EmbedId, rect: Option<egui::Rect>) {
        let Some(parent) = self.surfaces.iter().find(|s| s.id == window) else { return };
        let Some(sc) = &self.subcompositor else {
            warn!("no wl_subcompositor: can't embed");
            return;
        };
        let (sub, wl) = sc.create_subsurface(parent.wl.clone(), &self.qh);
        // Its renderer commits on its own; input goes to the window.
        sub.set_desync();
        if let Ok(region) = smithay_client_toolkit::compositor::Region::new(&self.compositor) {
            wl.set_input_region(Some(region.wl_region()));
        }
        let viewport = self.viewporter.as_ref().map(|vp| vp.get_viewport(&wl, &self.qh, KitData));
        let scale = parent.scale;
        self.embeds.push(Embed { id, window, wl, sub, viewport, rect: None, scale });
        if let Some(r) = rect {
            self.place_embed(id, r);
        }
        self.announce.push(id);
    }

    fn place_embed(&mut self, id: EmbedId, rect: egui::Rect) {
        let Some(e) = self.embeds.iter_mut().find(|e| e.id == id) else { return };
        if e.rect == Some(rect) {
            return;
        }
        e.rect = Some(rect);
        e.sub.set_position(rect.min.x.round() as i32, rect.min.y.round() as i32);
        let (w, h) = (rect.width().round().max(1.0) as i32, rect.height().round().max(1.0) as i32);
        match &e.viewport {
            Some(v) => v.set_destination(w, h),
            None => e.wl.set_buffer_scale(e.scale.round().max(1.0) as i32),
        }
        e.wl.commit();
        // The position takes effect with the window's next commit.
        let window = e.window;
        if let Some(s) = self.surface_mut(window) {
            s.dirty = true;
        }
    }

    /// Tell the app an embed's surface is going, then destroy it.
    fn lose_embeds(&mut self, which: impl Fn(&Embed) -> bool) {
        let ids: Vec<EmbedId> = self.embeds.iter().filter(|e| which(e)).map(|e| e.id).collect();
        if ids.is_empty() {
            return;
        }
        self.with_app(|app, cx| ids.iter().for_each(|&id| app.embed_lost(cx, id)));
        self.destroy_embeds(|e| ids.contains(&e.id));
    }

    fn destroy_embeds(&mut self, which: impl Fn(&Embed) -> bool) {
        let (gone, kept): (Vec<Embed>, Vec<Embed>) = std::mem::take(&mut self.embeds).into_iter().partition(|e| which(e));
        self.embeds = kept;
        for e in gone {
            if let Some(v) = e.viewport {
                v.destroy();
            }
            e.sub.destroy();
            e.wl.destroy();
            self.announce.retain(|&a| a != e.id);
        }
    }

    /// Hand the app the handles of new embeds, and of any whose window's
    /// scale changed.
    fn announce_embeds(&mut self) {
        let Some(ptr) = NonNull::new(self.conn.backend().display_ptr() as *mut _) else { return };
        let display = RawDisplayHandle::Wayland(WaylandDisplayHandle::new(ptr));
        let ready: Vec<Embedded> = std::mem::take(&mut self.announce)
            .into_iter()
            .filter_map(|id| {
                let e = self.embeds.iter().find(|e| e.id == id)?;
                let surface = RawWindowHandle::Wayland(WaylandWindowHandle::new(NonNull::new(e.wl.id().as_ptr() as *mut _)?));
                Some(Embedded { id, window: e.window, display, surface, scale: e.scale })
            })
            .collect();
        self.with_app(|app, cx| ready.iter().for_each(|e| app.embedded(cx, e)));
    }

    /// Embeds follow their window's scale.
    fn check_embed_scales(&mut self) {
        for i in 0..self.embeds.len() {
            let Some(scale) = self.surfaces.iter().find(|s| s.id == self.embeds[i].window).map(|s| s.scale) else { continue };
            let e = &mut self.embeds[i];
            if (e.scale - scale).abs() > f64::EPSILON {
                e.scale = scale;
                if e.viewport.is_none() {
                    e.wl.set_buffer_scale(scale.round().max(1.0) as i32);
                }
                if !self.announce.contains(&e.id) {
                    self.announce.push(e.id);
                }
            }
        }
        if !self.announce.is_empty() {
            self.announce_embeds();
        }
    }

    fn output_names(&self) -> Vec<String> {
        self.output_state.outputs().filter_map(|o| self.output_state.info(&o)?.name).collect()
    }

    /// The first output's size in logical pixels.
    fn screen_size(&self) -> Option<(u32, u32)> {
        self.output_state
            .outputs()
            .next()
            .and_then(|o| self.output_state.info(&o))
            .and_then(|i| {
                // Without xdg-output's logical size, derive it from the
                // current mode and the output scale.
                i.logical_size.or_else(|| {
                    let mode = i.modes.iter().find(|m| m.current)?;
                    let k = i.scale_factor.max(1);
                    Some((mode.dimensions.0 / k, mode.dimensions.1 / k))
                })
            })
            .map(|(w, h)| (w.max(1) as u32, h.max(1) as u32))
    }

    /// Call into the app with a Cx, then act on what it asked for.
    fn with_app<R>(&mut self, f: impl FnOnce(&mut dyn App, &mut Cx) -> R) -> Option<R> {
        let mut app = self.app.take()?;
        let mut cx = self.cx.take()?;
        cx.palette = self.palette.clone();
        cx.sizes.clear();
        cx.sizes.extend(self.surfaces.iter().filter(|s| matches!(s.role, Role::Toplevel(_))).map(|s| (s.id, s.size)));
        cx.screen = self.screen_size();
        cx.outputs = self.output_names();
        let r = f(app.as_mut(), &mut cx);
        self.app = Some(app);
        self.cx = Some(cx);
        self.process_requests();
        Some(r)
    }

    fn process_requests(&mut self) {
        loop {
            let Some(cx) = self.cx.as_mut() else { return };
            if cx.requests.is_empty() {
                if self.announce.is_empty() {
                    return;
                }
                self.announce_embeds();
                continue;
            }
            let requests = std::mem::take(&mut cx.requests);
            for r in requests {
                match r {
                    Request::Open(id, options) => {
                        // A window again (say, a forwarded launch): no
                        // longer only minding the clipboard.
                        self.linger = false;
                        self.open(id, options)
                    }
                    Request::Close(id) => {
                        self.lose_embeds(|e| e.window == id);
                        self.close(id)
                    }
                    Request::Embed(window, id) => self.create_embed(window, id, None),
                    Request::PlaceEmbed(id, rect) => self.place_embed(id, rect),
                    Request::RemoveEmbed(id) => self.destroy_embeds(|e| e.id == id),
                    Request::KeepAwake(id, on) => {
                        let qh = self.qh.clone();
                        let manager = self.idle_inhibit.clone();
                        if let Some(s) = self.surface_mut(id) {
                            match (on, s.inhibitor.take(), manager) {
                                (true, Some(i), _) => s.inhibitor = Some(i),
                                (true, None, Some(m)) => s.inhibitor = Some(m.create_inhibitor(&s.wl, &qh, KitData)),
                                (false, Some(i), _) => i.destroy(),
                                _ => {}
                            }
                        }
                    }
                    Request::SetTitle(id, title) => {
                        if let Some(s) = self.surface_mut(id) {
                            if let Role::Toplevel(w) = &s.role {
                                w.set_title(title.clone());
                            }
                            s.options.title = title;
                        }
                    }
                    Request::Resize(id, size) => {
                        let Some(i) = self.surfaces.iter().position(|s| s.id == id) else { continue };
                        let s = &mut self.surfaces[i];
                        if let Role::Layer(l) = &s.role {
                            // Takes effect when the compositor configures us.
                            let (t, b, lft, r) = match &s.options.kind {
                                WindowKind::Layer(lo) => lo.anchor,
                                _ => (false, false, false, false),
                            };
                            let w = if lft && r { 0 } else { size.0 };
                            let h = if t && b { 0 } else { size.1 };
                            l.set_size(w, h);
                            l.commit();
                        } else if s.size != size {
                            s.size = size;
                            s.dirty = true;
                            if s.gpu_surface.is_some() {
                                self.configure_gpu_surface(i);
                            }
                        }
                    }
                    Request::Redraw(id) => {
                        if let Some(s) = self.surface_mut(id) {
                            s.dirty = true;
                        }
                    }
                    Request::RedrawAll => self.surfaces.iter_mut().for_each(|s| s.dirty = true),
                    Request::LockSession => match self.session_lock_state.lock(&self.qh) {
                        Ok(lock) => self.session_lock = Some(lock),
                        Err(e) => {
                            warn!("cannot lock the session: {e}");
                            self.with_app(|app, cx| app.message(cx, Box::new(crate::SessionLock::Finished)));
                        }
                    },
                    Request::UnlockSession => {
                        if let Some(lock) = self.session_lock.take() {
                            lock.unlock();
                            let ids: Vec<WindowId> =
                                self.surfaces.iter().filter(|s| matches!(s.role, Role::Lock(..))).map(|s| s.id).collect();
                            for id in ids {
                                self.drop_surface(id);
                            }
                            let _ = self.conn.flush();
                        }
                    }
                    Request::Activate(id) => {
                        // Ask for a token for the surface, then use it
                        // (`new_token`): app_server raises and focuses it.
                        if let (Some(act), Some(s)) =
                            (&self.activation, self.surfaces.iter().find(|s| s.id == id))
                        {
                            let seat_and_serial =
                                self.seat.clone().map(|seat| (seat, self.last_serial));
                            act.request_token(
                                &self.qh,
                                smithay_client_toolkit::activation::RequestData {
                                    app_id: None,
                                    seat_and_serial,
                                    surface: Some(s.wl.clone()),
                                    udata: (),
                                },
                            );
                        }
                    }
                    Request::Copy(text) => self.set_clipboard(text),
                    Request::CopyData(clip) => self.set_clipboard_data(clip),
                    Request::Paste(id) => {
                        if let Some(wl) = self.surface_mut(id).map(|s| s.wl.clone()) {
                            self.paste(&wl);
                        }
                    }
                    Request::Quit => self.finish(),
                }
            }
        }
    }

    fn surface_mut(&mut self, id: WindowId) -> Option<&mut Surface> {
        self.surfaces.iter_mut().find(|s| s.id == id)
    }

    fn index_of_wl(&self, wl: &WlSurface) -> Option<usize> {
        self.surfaces.iter().position(|s| &s.wl == wl)
    }

    fn new_ctx(&self) -> egui::Context {
        let ctx = egui::Context::default();
        ctx.set_fonts(style::all_font_definitions());
        style::apply(&ctx, &self.palette);
        ctx.options_mut(|o| {
            o.zoom_with_keyboard = false;
        });
        ctx
    }

    fn attach_scale(&self, wl: &WlSurface) -> (Option<WpFractionalScaleV1>, Option<WpViewport>) {
        match (&self.fractional_manager, &self.viewporter) {
            (Some(fm), Some(vp)) => (
                Some(fm.get_fractional_scale(wl, &self.qh, ScaleData(wl.clone()))),
                Some(vp.get_viewport(wl, &self.qh, KitData)),
            ),
            _ => (None, None),
        }
    }

    fn make_surface(&self, id: WindowId, role: Role, wl: WlSurface, options: WindowOptions, size: (u32, u32)) -> Surface {
        let (fractional, viewport) = self.attach_scale(&wl);
        // Until the compositor tells us, assume the output scale.
        let scale = self
            .output_state
            .outputs()
            .next()
            .and_then(|o| self.output_state.info(&o))
            .map(|i| i.scale_factor as f64)
            .unwrap_or(2.0);
        Surface {
            id,
            role,
            wl,
            options,
            gpu_surface: None,
            format: wgpu::TextureFormat::Bgra8Unorm,
            renderer: None,
            ctx: {
                let ctx = self.new_ctx();
                ctx.set_request_repaint_callback(move |info| {
                    if let Some(tx) = REPAINT.get() {
                        let _ = tx.lock().unwrap().send((id, info.delay));
                    }
                });
                ctx
            },
            fonts: style::fonts_generation(),
            size,
            scale,
            fractional,
            viewport,
            inhibitor: None,
            configured: false,
            dirty: true,
            frame_pending: false,
            repaint_at: None,
            events: Vec::new(),
            pointer: None,
            buttons: Vec::new(),
            swallow: false,
            bar_rect: None,
            cursor: egui::CursorIcon::Default,
            menus: None,
            bar: BarState::default(),
            popups: Vec::new(),
            context: None,
            start: Instant::now(),
        }
    }

    fn open(&mut self, id: WindowId, options: WindowOptions) {
        let wl = self.compositor.create_surface(&self.qh);
        let size = options.size;
        let role = match &options.kind {
            WindowKind::Normal => {
                let window = self.xdg.create_window(wl.clone(), WindowDecorations::RequestServer, &self.qh);
                window.set_title(options.title.clone());
                window.set_app_id(self.cx.as_ref().map(|c| c.app_id.clone()).unwrap_or_default());
                window.set_min_size(options.min_size);
                window.set_max_size(options.max_size);
                let parent = match options.parent {
                    Some(p) => self.surfaces.iter().find(|s| s.id == p),
                    None if options.alert => self.surfaces.iter().find(|s| self.keyboard_focus.as_ref() == Some(&s.wl)),
                    None => None,
                };
                if let Some(Role::Toplevel(p)) = parent.map(|s| &s.role) {
                    window.set_parent(Some(p));
                }
                window.commit();
                Role::Toplevel(window)
            }
            WindowKind::Layer(lo) => {
                let Some(shell) = self.layer_shell.as_ref() else {
                    warn!("compositor has no layer shell");
                    return;
                };
                let layer = match lo.layer {
                    Layer::Background => smithay_client_toolkit::shell::wlr_layer::Layer::Background,
                    Layer::Bottom => smithay_client_toolkit::shell::wlr_layer::Layer::Bottom,
                    Layer::Top => smithay_client_toolkit::shell::wlr_layer::Layer::Top,
                    Layer::Overlay => smithay_client_toolkit::shell::wlr_layer::Layer::Overlay,
                };
                let output = lo.output.as_ref().and_then(|name| {
                    self.output_state
                        .outputs()
                        .find(|o| self.output_state.info(o).and_then(|i| i.name).as_ref() == Some(name))
                });
                let surface =
                    shell.create_layer_surface(&self.qh, wl.clone(), layer, Some(lo.namespace.clone()), output.as_ref());
                let mut anchor = Anchor::empty();
                let (t, b, l, r) = lo.anchor;
                if t {
                    anchor |= Anchor::TOP;
                }
                if b {
                    anchor |= Anchor::BOTTOM;
                }
                if l {
                    anchor |= Anchor::LEFT;
                }
                if r {
                    anchor |= Anchor::RIGHT;
                }
                surface.set_anchor(anchor);
                let w = if l && r { 0 } else { size.0 };
                let h = if t && b { 0 } else { size.1 };
                surface.set_size(w, h);
                surface.set_exclusive_zone(lo.exclusive_zone);
                let (mt, mr, mb, ml) = lo.margin;
                surface.set_margin(mt, mr, mb, ml);
                surface.set_keyboard_interactivity(match lo.keyboard {
                    crate::Keyboard::None => KeyboardInteractivity::None,
                    crate::Keyboard::OnDemand => KeyboardInteractivity::OnDemand,
                    crate::Keyboard::Exclusive => KeyboardInteractivity::Exclusive,
                });
                surface.commit();
                Role::Layer(surface)
            }
        };
        let s = self.make_surface(id, role, wl, options, size);
        self.surfaces.push(s);
    }

    fn close(&mut self, id: WindowId) {
        // Close popups owned by this window first.
        let popups: Vec<WindowId> = self.surface_mut(id).map(|s| s.popups.clone()).unwrap_or_default();
        for p in popups.into_iter().rev() {
            self.drop_surface(p);
        }
        self.drop_surface(id);
        let any_window = self
            .surfaces
            .iter()
            .any(|s| matches!(s.role, Role::Toplevel(_) | Role::Layer(_) | Role::Lock(..)));
        if !any_window && !self.cx.as_ref().is_some_and(|c| c.resident) {
            self.finish();
        }
    }

    /// Done: exit, unless we hold the clipboard. With no clipboard manager
    /// what we copied lives in us, so stay (windowless) until something
    /// else is copied.
    fn finish(&mut self) {
        if self.clipboard.is_none() {
            self.exit = true;
            return;
        }
        let ids: Vec<WindowId> = self.surfaces.iter().map(|s| s.id).collect();
        for id in ids.into_iter().rev() {
            self.drop_surface(id);
        }
        self.linger = true;
    }

    fn drop_surface(&mut self, id: WindowId) {
        if let Some(i) = self.surfaces.iter().position(|s| s.id == id) {
            let mut s = self.surfaces.remove(i);
            s.gpu_surface = None;
            if let Some(f) = s.fractional.take() {
                f.destroy();
            }
            if let Some(v) = s.viewport.take() {
                v.destroy();
            }
            if let Some(i) = s.inhibitor.take() {
                i.destroy();
            }
            // Dropping the role destroys the xdg/layer objects.
        }
    }

    // ---- palette ------------------------------------------------------

    fn set_palette(&mut self, p: Arc<Palette>) {
        if *self.palette == *p {
            return;
        }
        info!(mode = ?p.mode, "theme changed");
        self.palette = p.clone();
        for s in &mut self.surfaces {
            style::apply(&s.ctx, &p);
            s.dirty = true;
        }
        self.with_app(|app, cx| app.theme_changed(cx, &p));
    }

    // ---- clipboard ----------------------------------------------------

    fn set_clipboard(&mut self, text: String) {
        let text: Arc<[u8]> = text.into_bytes().into();
        let types = ["text/plain;charset=utf-8", "text/plain", "UTF8_STRING", "STRING", "TEXT"];
        self.set_clipboard_data(types.iter().map(|t| (t.to_string(), text.clone())).collect());
    }

    fn set_clipboard_data(&mut self, clip: Clip) {
        let (Some(mgr), Some(dev)) = (&self.data_device_manager, &self.data_device) else {
            return;
        };
        let source = mgr.create_copy_paste_source(&self.qh, clip.iter().map(|(t, _)| t.as_str()));
        source.set_selection(dev, self.last_serial);
        self.clipboard = Some((source, clip));
    }

    /// Deliver the clipboard's text to `wl` as a Paste event (empty if
    /// there is none). Another client's text arrives through a pipe: it is
    /// read on a helper thread and delivered when complete, however large
    /// or slow, without holding up the UI.
    fn paste(&mut self, wl: &WlSurface) {
        if let Some((_, clip)) = &self.clipboard {
            // Ours: text if it has any (an image pastes as nothing).
            let text = clip
                .iter()
                .find(|(t, _)| t.starts_with("text/plain"))
                .map(|(_, d)| String::from_utf8_lossy(d).into_owned())
                .unwrap_or_default();
            return self.push_event(wl, egui::Event::Paste(text));
        }
        let pipe = self.data_device.as_ref().and_then(|d| d.data().selection_offer()).and_then(|offer| {
            let mime = offer.with_mime_types(|types| {
                ["text/plain;charset=utf-8", "UTF8_STRING", "text/plain"]
                    .iter()
                    .find(|m| types.iter().any(|t| t == *m))
                    .map(|m| m.to_string())
            })?;
            offer.receive(mime).ok()
        });
        let Some(mut pipe) = pipe else {
            return self.push_event(wl, egui::Event::Paste(String::new()));
        };
        let _ = self.conn.flush();
        let wl = wl.clone();
        let _ = std::thread::Builder::new().name("paste".into()).spawn(move || {
            let mut out = Vec::new();
            let _ = pipe.read_to_end(&mut out);
            let text = String::from_utf8(out).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned());
            if let Some(tx) = PASTED.get() {
                let _ = tx.lock().unwrap_or_else(|e| e.into_inner()).send((wl, text));
            }
        });
    }

    // ---- rendering ----------------------------------------------------

    fn ensure_gpu(&mut self, wl: &WlSurface) -> Option<wgpu::Surface<'static>> {
        let display = RawDisplayHandle::Wayland(WaylandDisplayHandle::new(
            NonNull::new(self.conn.backend().display_ptr() as *mut _)?,
        ));
        let window = RawWindowHandle::Wayland(WaylandWindowHandle::new(NonNull::new(wl.id().as_ptr() as *mut _)?));
        if self.gpu.is_none() {
            let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
            desc.backends = wgpu::Backends::VULKAN | wgpu::Backends::GL;
            let instance = wgpu::Instance::new(desc);
            let surface = unsafe {
                instance
                    .create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                        raw_display_handle: Some(display),
                        raw_window_handle: window,
                    })
                    .ok()?
            };
            let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
                apply_limit_buckets: false,
            }))
            .ok()?;
            debug!(adapter = ?adapter.get_info(), "wgpu adapter");
            let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("tabula-kit"),
                // Small allocation blocks: a window needs a few textures,
                // not the large pools games want.
                memory_hints: wgpu::MemoryHints::MemoryUsage,
                ..Default::default()
            }))
            .ok()?;
            self.gpu = Some(Gpu { instance, adapter, device, queue });
            return Some(surface);
        }
        let gpu = self.gpu.as_ref()?;
        unsafe {
            gpu.instance
                .create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                    raw_display_handle: Some(display),
                    raw_window_handle: window,
                })
                .ok()
        }
    }

    fn configure_gpu_surface(&mut self, i: usize) {
        if self.surfaces[i].gpu_surface.is_none() {
            let wl = self.surfaces[i].wl.clone();
            let Some(gs) = self.ensure_gpu(&wl) else {
                warn!("could not create GPU surface");
                return;
            };
            self.surfaces[i].gpu_surface = Some(gs);
        }
        let gpu = self.gpu.as_ref().unwrap();
        let s = &mut self.surfaces[i];
        let gs = s.gpu_surface.as_ref().unwrap();
        let caps = gs.get_capabilities(&gpu.adapter);
        // egui blends in gamma space: use a non-sRGB target.
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| matches!(f, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Rgba8Unorm))
            .unwrap_or(caps.formats[0]);
        // Popups and surfaces without a panel background (the Tracker
        // desktop) are translucent.
        let alpha_mode = if (matches!(s.role, Role::Popup { .. }) || !s.options.panel)
            && caps.alpha_modes.contains(&wgpu::CompositeAlphaMode::PreMultiplied)
        {
            wgpu::CompositeAlphaMode::PreMultiplied
        } else if caps.alpha_modes.contains(&wgpu::CompositeAlphaMode::Opaque) {
            wgpu::CompositeAlphaMode::Opaque
        } else {
            caps.alpha_modes[0]
        };
        let present_mode = if caps.present_modes.contains(&wgpu::PresentMode::Mailbox) {
            wgpu::PresentMode::Mailbox
        } else {
            wgpu::PresentMode::Fifo
        };
        let (w, h) = s.physical_size();
        gs.configure(
            &gpu.device,
            &wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format,
                width: w,
                height: h,
                present_mode,
                desired_maximum_frame_latency: 1,
                alpha_mode,
                view_formats: vec![],
                color_space: wgpu::SurfaceColorSpace::Auto,
            },
        );
        if s.renderer.is_none() || s.format != format {
            s.renderer = Some(egui_wgpu::Renderer::new(&gpu.device, format, egui_wgpu::RendererOptions {
                msaa_samples: 1,
                depth_stencil_format: None,
                dithering: false,
                predictable_texture_filtering: false,
            }));
            s.format = format;
        }
        if let Some(vp) = &s.viewport {
            vp.set_destination(s.size.0 as i32, s.size.1 as i32);
        } else {
            s.wl.set_buffer_scale(s.scale.round().max(1.0) as i32);
        }
    }

    fn next_timeout(&self) -> Option<Duration> {
        let now = Instant::now();
        let mut t: Option<Duration> = None;
        for s in &self.surfaces {
            if s.dirty && !s.frame_pending && s.configured {
                return Some(Duration::ZERO);
            }
            if let Some(at) = s.repaint_at {
                let d = at.saturating_duration_since(now);
                t = Some(t.map_or(d, |t: Duration| t.min(d)));
            }
        }
        t
    }

    fn redraw_due(&mut self) {
        let now = Instant::now();
        for i in 0..self.surfaces.len() {
            if i >= self.surfaces.len() {
                break;
            }
            let s = &mut self.surfaces[i];
            if s.repaint_at.is_some_and(|at| at <= now) {
                s.repaint_at = None;
                s.dirty = true;
            }
            if s.dirty && s.configured && !s.frame_pending {
                let id = s.id;
                self.draw(id);
            }
        }
    }

    fn draw(&mut self, id: WindowId) {
        let Some(i) = self.surfaces.iter().position(|s| s.id == id) else { return };
        if self.surfaces[i].gpu_surface.is_none() {
            self.configure_gpu_surface(i);
        }
        if self.surfaces[i].gpu_surface.is_none() {
            return;
        }

        // Gather input.
        let max_texture_side = self.gpu.as_ref().map(|g| g.device.limits().max_texture_dimension_2d as usize);
        let s = &mut self.surfaces[i];
        s.dirty = false;
        let mut raw = egui::RawInput {
            // egui assumes 2048 unless told.
            max_texture_side,
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(s.size.0 as f32, s.size.1 as f32))),
            time: Some(s.start.elapsed().as_secs_f64()),
            focused: self.keyboard_focus.as_ref() == Some(&s.wl)
                || matches!(s.role, Role::Popup { .. }),
            events: std::mem::take(&mut s.events),
            ..Default::default()
        };
        raw.viewports
            .entry(ViewportId::ROOT)
            .or_default()
            .native_pixels_per_point = Some(s.scale as f32);
        let ctx = s.ctx.clone();
        // Document fonts added since this window last looked.
        let fonts = style::fonts_generation();
        if s.fonts != fonts {
            s.fonts = fonts;
            ctx.set_fonts(style::all_font_definitions());
        }
        let open_source = if s.popups.is_empty() { None } else { s.context.as_ref().map(|r| r.source) };
        menu::set_open_source(&ctx, open_source);
        let kind = match &s.role {
            Role::Toplevel(_) => 0,
            Role::Layer(_) | Role::Lock(..) => 1,
            Role::Popup { .. } => 2,
        };

        let mut menu_events: Vec<MenuEvent> = Vec::new();
        let output = if kind == 2 {
            // Popup menus are drawn entirely by the kit.
            let (owner, path, mut state) = match &s.role {
                Role::Popup { owner, path, state, .. } => (*owner, path.clone(), state.clone()),
                _ => unreachable!(),
            };
            let menu = self.surfaces.iter().find(|o| o.id == owner).and_then(|o| menu_of(o, &path));
            let Some(menu) = menu else {
                menu_events.push(MenuEvent::CloseAll(owner));
                self.handle_menu_events(menu_events);
                return;
            };
            let out = ctx.run_ui(raw, |ui| {
                let r = menu::popup_ui(ui, &menu, &mut state);
                if r != PopupResult::None {
                    menu_events.push(MenuEvent::Popup { popup: id, owner, path: path.clone(), result: r });
                }
            });
            if let Some(Role::Popup { state: st, .. }) = self.surface_mut(id).map(|s| &mut s.role) {
                *st = state;
            }
            out
        } else {
            let mut app = self.app.take();
            let mut cx = self.cx.take();
            let s = &mut self.surfaces[i];
            let mut bar = s.bar.clone();
            let mut drawn_bar = None;
            let menus = app.as_mut().and_then(|a| a.menu_bar(id));
            let panel = s.options.panel;
            let palette = self.palette.clone();
            let toplevel = matches!(s.role, Role::Toplevel(_));
            let raw_keys = s.options.raw_keys;
            let mut close_key = false;
            let out = ctx.run_ui(raw, |ui| {
                // ⌘ is Ctrl, so ⌘H/⌘K/⌘U/⌘W are command shortcuts here,
                // never egui's emacs deletions in a focused text field: keep
                // them from the app's ui and give them back for the menus.
                let held = if raw_keys { Vec::new() } else { hold_emacs_keys(ui) };
                if panel {
                    ui.painter().rect_filled(ui.max_rect(), 0.0, style::c(palette.panel_background));
                }
                if let Some(menus) = &menus {
                    let bar_rect = menu::menu_bar(ui, menus, &mut bar);
                    drawn_bar = Some(bar_rect);
                    let mut rect = ui.max_rect();
                    rect.min.y = bar_rect.max.y;
                    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
                    if let (Some(app), Some(cx)) = (app.as_mut(), cx.as_mut()) {
                        app.ui(cx, id, &mut child);
                    }
                    ui.input_mut(|inp| inp.events.extend(held.iter().cloned()));
                    // Command-key shortcuts.
                    // ⌘C/⌘X/⌘V arrive as clipboard events; when no text
                    // field takes them they go to the menu's Copy/Cut/Paste.
                    let text_focus = ui.ctx().egui_wants_keyboard_input();
                    let pressed: Vec<(egui::Key, bool)> = ui.input(|inp| {
                        inp.events
                            .iter()
                            .filter_map(|e| match e {
                                egui::Event::Key { key, pressed: true, modifiers, .. }
                                    if modifiers.ctrl && !modifiers.alt =>
                                {
                                    Some((*key, modifiers.shift))
                                }
                                egui::Event::Copy if !text_focus => Some((egui::Key::C, false)),
                                egui::Event::Cut if !text_focus => Some((egui::Key::X, false)),
                                egui::Event::Paste(_) if !text_focus => Some((egui::Key::V, false)),
                                _ => None,
                            })
                            .collect()
                    });
                    for (key, shift) in pressed {
                        if let Some(cmd) = menu::find_shortcut(menus, key, shift) {
                            menu_events.push(MenuEvent::Command(id, cmd));
                        }
                    }
                } else if let (Some(app), Some(cx)) = (app.as_mut(), cx.as_mut()) {
                    app.ui(cx, id, ui);
                    ui.input_mut(|inp| inp.events.extend(held));
                    // Windows without a menu bar (Get Info, Find, alerts)
                    // still close with ⌘W, as on BeOS.
                    close_key = toplevel
                        && ui.input(|inp| {
                            inp.events.iter().any(|e| {
                                matches!(e, egui::Event::Key { key: egui::Key::W, pressed: true, modifiers, .. }
                                    if modifiers.ctrl && !modifiers.alt && !modifiers.shift)
                            })
                        });
                }
            });
            self.app = app;
            self.cx = cx;
            if close_key {
                menu_events.push(MenuEvent::CloseWindow(id));
            }
            let s = &mut self.surfaces[i];
            s.bar_rect = drawn_bar;
            if bar != s.bar {
                menu_events.push(MenuEvent::BarChanged(id, s.bar.open.map(|o| o.0), bar.open));
                s.bar = bar;
            }
            s.menus = menus;
            if let Some(request) = menu::take_request(&ctx) {
                menu_events.push(MenuEvent::Context(id, request));
            }
            out
        };

        self.present(id, output);
        self.handle_menu_events(menu_events);
        self.process_requests();
    }

    fn present(&mut self, id: WindowId, output: egui::FullOutput) {
        let Some(i) = self.surfaces.iter().position(|s| s.id == id) else { return };
        // Platform output: clipboard, cursor, links.
        for cmd in &output.platform_output.commands {
            match cmd {
                egui::OutputCommand::CopyText(t) => self.set_clipboard(t.clone()),
                egui::OutputCommand::OpenUrl(u) => {
                    // Looking up the handler reads files: not on the UI thread.
                    let url = u.url.clone();
                    std::thread::spawn(move || {
                        if let Err(e) = tabula_xdg::open::open(&tabula_xdg::open::Target::parse(&url)) {
                            tracing::warn!("open {url}: {e}");
                        }
                    });
                }
                _ => {}
            }
        }
        let s = &mut self.surfaces[i];
        if output.platform_output.cursor_icon != s.cursor {
            s.cursor = output.platform_output.cursor_icon;
            if self.pointer_focus.as_ref() == Some(&s.wl)
                && let Some(p) = &self.pointer
            {
                show_cursor(p, &self.conn, s.cursor);
            }
        }
        if let Some(vo) = output.viewport_output.get(&ViewportId::ROOT) {
            if vo.repaint_delay.is_zero() {
                s.dirty = true;
            } else if vo.repaint_delay < Duration::from_secs(3600) {
                s.repaint_at = Some(Instant::now() + vo.repaint_delay);
            }
        }

        let gpu = self.gpu.as_ref().unwrap();
        let ppp = output.pixels_per_point;
        let prims = s.ctx.tessellate(output.shapes, ppp);
        let (w, h) = s.physical_size();
        // Textures first: they must be applied even if this frame is skipped.
        let renderer = s.renderer.as_mut().unwrap();
        let mut deltas = output.textures_delta;
        for (tid, ds) in &deltas.set {
            for delta in ds {
                renderer.update_texture(&gpu.device, &gpu.queue, *tid, delta);
            }
        }
        let freed = std::mem::take(&mut deltas.free);
        deltas.clear();
        let gs = s.gpu_surface.as_ref().unwrap();
        let frame = match gs.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            other => {
                debug!(?other, "surface lost; reconfiguring");
                for tid in &freed {
                    s.renderer.as_mut().unwrap().free_texture(tid);
                }
                self.configure_gpu_surface(i);
                self.surfaces[i].dirty = true;
                return;
            }
        };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let renderer = s.renderer.as_mut().unwrap();
        let screen = egui_wgpu::ScreenDescriptor { size_in_pixels: [w, h], pixels_per_point: ppp };
        let mut encoder = gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        let mut cmds = renderer.update_buffers(&gpu.device, &gpu.queue, &mut encoder, &prims, &screen);
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("egui"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let mut pass = pass.forget_lifetime();
            renderer.render(&mut pass, &prims, &screen);
        }
        cmds.push(encoder.finish());
        gpu.queue.submit(cmds);
        for tid in &freed {
            renderer.free_texture(tid);
        }
        // Throttle to the compositor's frame clock.
        s.wl.frame(&self.qh, FrameCallbackData(s.wl.clone()));
        s.frame_pending = true;
        gpu.queue.present(frame);
    }

    // ---- menus ----------------------------------------------------------

    fn handle_menu_events(&mut self, events: Vec<MenuEvent>) {
        for e in events {
            match e {
                MenuEvent::Command(win, cmd) => {
                    self.with_app(|app, cx| app.command(cx, win, &cmd));
                }
                MenuEvent::CloseWindow(win) => {
                    self.with_app(|app, cx| app.close_requested(cx, win));
                }
                MenuEvent::BarChanged(win, _old, new) => {
                    self.close_popups_of(win, 0);
                    if let Some((index, rect)) = new {
                        self.open_menu_popup(win, vec![index], rect, None);
                    }
                }
                MenuEvent::Context(win, request) => {
                    self.close_popups_of(win, 0);
                    let anchor = match request.anchor {
                        MenuAnchor::At(p) => Rect::from_min_size(p, vec2(1.0, 1.0)),
                        MenuAnchor::Below(r) | MenuAnchor::Over(r, _) => r,
                    };
                    let selected = request.selected;
                    if let Some(s) = self.surface_mut(win) {
                        s.bar.open = None;
                        s.context = Some(request);
                    }
                    self.open_menu_popup(win, vec![CONTEXT], anchor, None);
                    let popup = self.surface_mut(win).and_then(|s| s.popups.last().copied());
                    if let Some(Role::Popup { state, .. }) = popup.and_then(|p| self.surface_mut(p)).map(|s| &mut s.role) {
                        state.selected = selected;
                    }
                }
                MenuEvent::CloseAll(owner) => {
                    self.close_popups_of(owner, 0);
                    if let Some(s) = self.surface_mut(owner) {
                        s.bar.open = None;
                        s.dirty = true;
                    }
                }
                MenuEvent::Popup { popup, owner, path, result } => match result {
                    PopupResult::Chosen(cmd) => {
                        self.close_popups_of(owner, 0);
                        let mut source = None;
                        if let Some(s) = self.surface_mut(owner) {
                            s.bar.open = None;
                            s.dirty = true;
                            if path[0] == CONTEXT {
                                source = s.context.take().map(|r| r.source);
                            }
                        }
                        match source {
                            // Ad-hoc menus answer the widget that asked.
                            Some(source) => {
                                if let Some(s) = self.surface_mut(owner) {
                                    menu::deliver_chosen(&s.ctx, source, cmd);
                                }
                            }
                            None => {
                                self.with_app(|app, cx| app.command(cx, owner, &cmd));
                            }
                        }
                    }
                    PopupResult::OpenSubmenu(item, row) => {
                        let depth = path.len();
                        self.close_popups_of(owner, depth);
                        if let Some(Role::Popup { state, .. }) = self.surface_mut(popup).map(|s| &mut s.role) {
                            state.open_sub = Some((item, row));
                        }
                        let mut sub = path.clone();
                        sub.push(item);
                        self.open_menu_popup(owner, sub, row, Some(popup));
                    }
                    PopupResult::CloseSubmenu => {
                        self.close_popups_of(owner, path.len());
                        if let Some(Role::Popup { state, .. }) = self.surface_mut(popup).map(|s| &mut s.role) {
                            state.open_sub = None;
                        }
                        if let Some(s) = self.surface_mut(popup) {
                            s.dirty = true;
                        }
                    }
                    PopupResult::Back => {
                        let depth = path.len() - 1;
                        self.close_popups_of(owner, depth);
                        if depth == 0
                            && let Some(s) = self.surface_mut(owner) {
                                s.bar.open = None;
                                s.dirty = true;
                            }
                    }
                    PopupResult::None => {}
                },
            }
        }
    }

    /// Close the owner's popups from `depth` outward.
    fn close_popups_of(&mut self, owner: WindowId, depth: usize) {
        let Some(s) = self.surface_mut(owner) else { return };
        if s.popups.len() <= depth {
            return;
        }
        let gone: Vec<WindowId> = s.popups.split_off(depth);
        s.dirty = true;
        for id in gone.into_iter().rev() {
            self.drop_surface(id);
        }
    }

    fn open_menu_popup(&mut self, owner: WindowId, path: Vec<usize>, anchor: Rect, parent_popup: Option<WindowId>) {
        // The popup's grab takes the pointer, so the owner may never see
        // the release of the press that opened the menu; release it now or
        // egui would think the button is still down (and, say, keep a menu
        // bar title held open).
        if let Some(i) = self.surfaces.iter().position(|s| s.id == owner) {
            let at = self.surfaces[i].pointer.unwrap_or_default();
            let held = std::mem::take(&mut self.surfaces[i].buttons);
            for button in held {
                self.surfaces[i].events.push(egui::Event::PointerButton {
                    pos: at,
                    button,
                    pressed: false,
                    modifiers: self.modifiers,
                });
            }
        }
        let Some(owner_s) = self.surfaces.iter().find(|s| s.id == owner) else { return };
        let Some(menu) = menu_of(owner_s, &path) else {
            return;
        };
        let context_anchor = (path == [CONTEXT]).then(|| owner_s.context.as_ref().map(|r| r.anchor)).flatten();
        let mut size = menu::popup_size(&owner_s.ctx, &menu);
        if let Some(MenuAnchor::Below(r) | MenuAnchor::Over(r, _)) = context_anchor {
            size.x = size.x.max(r.width().round());
        }
        let size = (size.x.ceil() as u32, size.y.ceil() as u32);

        let parent_surface = match parent_popup {
            Some(pid) => self.surfaces.iter().find(|s| s.id == pid),
            None => Some(owner_s),
        };
        let Some(parent_surface) = parent_surface else { return };
        let Ok(positioner) = XdgPositioner::new(&self.xdg) else { return };
        positioner.set_size(size.0 as i32, size.1 as i32);
        let a = |v: f32| v.round() as i32;
        if let Some(MenuAnchor::At(_)) = context_anchor {
            positioner.set_anchor_rect(a(anchor.left()), a(anchor.top()), 1, 1);
            positioner.set_anchor(xdg_positioner::Anchor::TopLeft);
            positioner.set_gravity(xdg_positioner::Gravity::BottomRight);
        } else if let Some(MenuAnchor::Over(r, n)) = context_anchor {
            // The current item sits exactly over the field, as on BeOS.
            let dy = menu::item_offset(&menu, n) - (r.height() - menu::ITEM_HEIGHT) / 2.0;
            positioner.set_anchor_rect(a(r.left()), a(r.top()), a(r.width()), a(r.height()));
            positioner.set_anchor(xdg_positioner::Anchor::TopLeft);
            positioner.set_gravity(xdg_positioner::Gravity::BottomRight);
            positioner.set_offset(0, -a(dy));
        } else if parent_popup.is_some() {
            // Submenu: to the right of the item, top-aligned.
            positioner.set_anchor_rect(a(anchor.left()), a(anchor.top()) - 3, a(anchor.width()), a(anchor.height()));
            positioner.set_anchor(xdg_positioner::Anchor::TopRight);
            positioner.set_gravity(xdg_positioner::Gravity::BottomRight);
            positioner.set_offset(-2, 0);
        } else {
            positioner.set_anchor_rect(a(anchor.left()), a(anchor.top()), a(anchor.width()), a(anchor.height()));
            positioner.set_anchor(xdg_positioner::Anchor::BottomLeft);
            positioner.set_gravity(xdg_positioner::Gravity::BottomRight);
        }
        positioner.set_constraint_adjustment(
            xdg_positioner::ConstraintAdjustment::FlipX
                | xdg_positioner::ConstraintAdjustment::FlipY
                | xdg_positioner::ConstraintAdjustment::SlideX
                | xdg_positioner::ConstraintAdjustment::SlideY,
        );

        let wl = self.compositor.create_surface(&self.qh);
        let popup = match &parent_surface.role {
            Role::Toplevel(w) => {
                Popup::from_surface(Some(w.xdg_surface()), &positioner, &self.qh, wl.clone(), &self.xdg)
            }
            Role::Popup { popup, .. } => {
                Popup::from_surface(Some(popup.xdg_surface()), &positioner, &self.qh, wl.clone(), &self.xdg)
            }
            Role::Layer(l) => {
                let p = Popup::from_surface(None, &positioner, &self.qh, wl.clone(), &self.xdg);
                if let Ok(p) = &p {
                    l.get_popup(p.xdg_popup());
                }
                p
            }
            Role::Lock(..) => {
                // Nothing may sit over the lock screen.
                warn!("no popups on lock surfaces");
                return;
            }
        };
        let Ok(popup) = popup else {
            warn!("could not create popup");
            return;
        };
        if let Some(seat) = &self.seat {
            popup.xdg_popup().grab(seat, self.last_serial);
        }
        wl.commit();

        let id = {
            let cx = self.cx.as_mut().unwrap();
            let id = WindowId(cx.next_id);
            cx.next_id += 1;
            id
        };
        let options = WindowOptions { panel: false, ..Default::default() };
        let role = Role::Popup { popup, owner, path, state: PopupState::default() };
        let s = self.make_surface(id, role, wl, options, size);
        self.surfaces.push(s);
        if let Some(o) = self.surface_mut(owner) {
            o.popups.push(id);
        }
    }

    fn push_event(&mut self, wl: &WlSurface, event: egui::Event) {
        if let Some(i) = self.index_of_wl(wl) {
            self.surfaces[i].events.push(event);
            self.surfaces[i].dirty = true;
        }
    }

    fn key_event(&mut self, event: &KeyEvent, pressed: bool, repeat: bool) {
        let Some(wl) = self.keyboard_focus.clone() else { return };
        let key = input::key_from_keysym(event.keysym);
        let m = self.modifiers;
        let raw = self.index_of_wl(&wl).is_some_and(|i| self.surfaces[i].options.raw_keys);
        // Terminal-style windows: Ctrl alone is for the program inside;
        // the clipboard is on Ctrl+Shift.
        if pressed && m.ctrl && raw == m.shift {
            match key {
                Some(egui::Key::C) if !m.shift => return self.push_event(&wl, egui::Event::Copy),
                Some(egui::Key::X) if !m.shift => return self.push_event(&wl, egui::Event::Cut),
                Some(egui::Key::V) if !m.shift => {
                    // Always deliver, even with no text on the clipboard, so
                    // menu Paste (e.g. Tracker's file clipboard) still runs.
                    return self.paste(&wl);
                }
                _ => {}
            }
        }
        if let Some(key) = key {
            self.push_event(
                &wl,
                egui::Event::Key { key, physical_key: None, pressed, repeat, modifiers: m },
            );
        }
        if pressed
            && !m.ctrl
            && (!m.alt || raw)
            && let Some(text) = &event.utf8
            && !text.chars().any(|c| c.is_control())
            && !text.is_empty()
        {
            self.push_event(&wl, egui::Event::Text(text.clone()));
        }
    }
}

#[derive(Debug)]
enum MenuEvent {
    Command(WindowId, String),
    BarChanged(WindowId, Option<usize>, Option<(usize, Rect)>),
    CloseAll(WindowId),
    /// ⌘W in a window without a menu bar.
    CloseWindow(WindowId),
    Context(WindowId, MenuRequest),
    Popup { popup: WindowId, owner: WindowId, path: Vec<usize>, result: PopupResult },
}

// ---- protocol handlers ----------------------------------------------------

impl CompositorHandler for Kit {
    fn scale_factor_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, surface: &WlSurface, factor: i32) {
        // Integer scale, used only without fractional-scale support.
        if let Some(i) = self.index_of_wl(surface)
            && self.surfaces[i].fractional.is_none()
            && (self.surfaces[i].scale - factor as f64).abs() > f64::EPSILON
        {
            self.surfaces[i].scale = factor as f64;
            if self.surfaces[i].configured {
                self.configure_gpu_surface(i);
            }
            self.surfaces[i].dirty = true;
        }
    }
    fn transform_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlSurface, _: wl_output::Transform) {}
    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, surface: &WlSurface, _: u32) {
        if let Some(i) = self.index_of_wl(surface) {
            self.surfaces[i].frame_pending = false;
        }
    }
    fn surface_enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlSurface, _: &wl_output::WlOutput) {}
    fn surface_leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlSurface, _: &wl_output::WlOutput) {}
}

impl Dispatch2<WpFractionalScaleV1, Kit> for ScaleData {
    fn event(
        &self,
        kit: &mut Kit,
        _: &WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        _: &Connection,
        _: &QueueHandle<Kit>,
    ) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event
            && let Some(i) = kit.index_of_wl(&self.0)
        {
            let scale = scale as f64 / 120.0;
            if (kit.surfaces[i].scale - scale).abs() > f64::EPSILON {
                kit.surfaces[i].scale = scale;
                if kit.surfaces[i].configured {
                    kit.configure_gpu_surface(i);
                }
                kit.surfaces[i].dirty = true;
            }
        }
    }
}

impl Dispatch2<WpFractionalScaleManagerV1, Kit> for KitData {
    fn event(&self, _: &mut Kit, _: &WpFractionalScaleManagerV1, _: <WpFractionalScaleManagerV1 as wayland_client::Proxy>::Event, _: &Connection, _: &QueueHandle<Kit>) {}
}
impl Dispatch2<ZwpIdleInhibitManagerV1, Kit> for KitData {
    fn event(&self, _: &mut Kit, _: &ZwpIdleInhibitManagerV1, _: <ZwpIdleInhibitManagerV1 as wayland_client::Proxy>::Event, _: &Connection, _: &QueueHandle<Kit>) {}
}
impl Dispatch2<ZwpIdleInhibitorV1, Kit> for KitData {
    fn event(&self, _: &mut Kit, _: &ZwpIdleInhibitorV1, _: <ZwpIdleInhibitorV1 as wayland_client::Proxy>::Event, _: &Connection, _: &QueueHandle<Kit>) {}
}
impl Dispatch2<WpViewporter, Kit> for KitData {
    fn event(&self, _: &mut Kit, _: &WpViewporter, _: <WpViewporter as wayland_client::Proxy>::Event, _: &Connection, _: &QueueHandle<Kit>) {}
}
impl Dispatch2<WpViewport, Kit> for KitData {
    fn event(&self, _: &mut Kit, _: &WpViewport, _: <WpViewport as wayland_client::Proxy>::Event, _: &Connection, _: &QueueHandle<Kit>) {}
}

/// Set the pointer's shape; `CursorIcon::None` hides it (screen savers).
fn show_cursor(p: &ThemedPointer, conn: &Connection, icon: egui::CursorIcon) {
    let _ = if icon == egui::CursorIcon::None { p.hide_cursor() } else { p.set_cursor(conn, input::cursor_icon(icon)) };
}

impl OutputHandler for Kit {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {
        if self.session_lock.is_some() {
            self.cover_outputs();
        }
    }
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, output: wl_output::WlOutput) {
        let gone: Vec<WindowId> =
            self.surfaces.iter().filter(|s| matches!(&s.role, Role::Lock(_, o) if *o == output)).map(|s| s.id).collect();
        if !gone.is_empty() {
            for id in gone {
                self.drop_surface(id);
            }
            self.cover_outputs();
        }
    }
}

impl WindowHandler for Kit {
    fn request_close(&mut self, _: &Connection, _: &QueueHandle<Self>, window: &Window) {
        if let Some(id) = self
            .surfaces
            .iter()
            .find(|s| matches!(&s.role, Role::Toplevel(w) if w == window))
            .map(|s| s.id)
        {
            self.with_app(|app, cx| app.close_requested(cx, id));
        }
    }

    fn configure(&mut self, _: &Connection, _: &QueueHandle<Self>, window: &Window, configure: WindowConfigure, _: u32) {
        let Some(i) = self
            .surfaces
            .iter()
            .position(|s| matches!(&s.role, Role::Toplevel(w) if w == window))
        else {
            return;
        };
        let s = &mut self.surfaces[i];
        let mut size = s.size;
        if let (Some(w), Some(h)) = configure.new_size {
            size = (w.get(), h.get());
        }
        if let Some((mw, mh)) = s.options.min_size {
            size = (size.0.max(mw), size.1.max(mh));
        }
        let changed = size != s.size || !s.configured;
        s.size = size;
        s.configured = true;
        s.dirty = true;
        if changed {
            self.configure_gpu_surface(i);
        }
        // Draw right away; the first frame maps the window.
        self.surfaces[i].frame_pending = false;
        let id = self.surfaces[i].id;
        self.draw(id);
    }
}

impl Kit {
    /// A lock surface on every output (the session just locked, or outputs
    /// changed while locked); tells the app the current set.
    fn cover_outputs(&mut self) {
        let Some(lock) = self.session_lock.clone() else { return };
        let covered: Vec<wl_output::WlOutput> = self
            .surfaces
            .iter()
            .filter_map(|s| match &s.role {
                Role::Lock(_, o) => Some(o.clone()),
                _ => None,
            })
            .collect();
        let outputs: Vec<wl_output::WlOutput> = self.output_state.outputs().collect();
        for output in outputs.iter().filter(|o| !covered.contains(o)) {
            let wl = self.compositor.create_surface(&self.qh);
            let surface = lock.create_lock_surface(wl.clone(), output, &self.qh);
            let id = {
                let cx = self.cx.as_mut().unwrap();
                let id = WindowId(cx.next_id);
                cx.next_id += 1;
                id
            };
            let options = WindowOptions { title: String::new(), ..Default::default() };
            let s = self.make_surface(id, Role::Lock(surface, output.clone()), wl, options, (1, 1));
            self.surfaces.push(s);
        }
        let ids: Vec<WindowId> = self.surfaces.iter().filter(|s| matches!(s.role, Role::Lock(..))).map(|s| s.id).collect();
        self.with_app(|app, cx| app.message(cx, Box::new(crate::SessionLock::Locked(ids))));
    }
}

impl smithay_client_toolkit::session_lock::SessionLockHandler for Kit {
    fn locked(&mut self, _: &Connection, _: &QueueHandle<Self>, _: smithay_client_toolkit::session_lock::SessionLock) {
        self.cover_outputs();
    }

    fn finished(&mut self, _: &Connection, _: &QueueHandle<Self>, _: smithay_client_toolkit::session_lock::SessionLock) {
        self.session_lock = None;
        self.with_app(|app, cx| app.message(cx, Box::new(crate::SessionLock::Finished)));
    }

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        surface: smithay_client_toolkit::session_lock::SessionLockSurface,
        configure: smithay_client_toolkit::session_lock::SessionLockSurfaceConfigure,
        _: u32,
    ) {
        let Some(i) = self.surfaces.iter().position(|s| &s.wl == surface.wl_surface()) else { return };
        let s = &mut self.surfaces[i];
        let size = configure.new_size;
        let changed = size != s.size || !s.configured;
        s.size = size;
        s.configured = true;
        s.dirty = true;
        if changed {
            self.configure_gpu_surface(i);
        }
        self.surfaces[i].frame_pending = false;
        let id = self.surfaces[i].id;
        self.draw(id);
    }
}

impl LayerShellHandler for Kit {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface) {
        if let Some(id) = self
            .surfaces
            .iter()
            .find(|s| matches!(&s.role, Role::Layer(l) if l == layer))
            .map(|s| s.id)
        {
            self.close(id);
        }
    }

    fn configure(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface, configure: LayerSurfaceConfigure, _: u32) {
        let Some(i) = self
            .surfaces
            .iter()
            .position(|s| matches!(&s.role, Role::Layer(l) if l == layer))
        else {
            return;
        };
        let s = &mut self.surfaces[i];
        let (w, h) = configure.new_size;
        let size = (if w == 0 { s.size.0 } else { w }, if h == 0 { s.size.1 } else { h });
        let changed = size != s.size || !s.configured;
        s.size = size;
        s.configured = true;
        s.dirty = true;
        if changed {
            self.configure_gpu_surface(i);
        }
        self.surfaces[i].frame_pending = false;
        let id = self.surfaces[i].id;
        self.draw(id);
    }
}

impl PopupHandler for Kit {
    fn configure(&mut self, _: &Connection, _: &QueueHandle<Self>, popup: &Popup, config: PopupConfigure) {
        let Some(i) = self
            .surfaces
            .iter()
            .position(|s| matches!(&s.role, Role::Popup { popup: p, .. } if p == popup))
        else {
            return;
        };
        let s = &mut self.surfaces[i];
        let size = (config.width.max(1) as u32, config.height.max(1) as u32);
        let changed = size != s.size || !s.configured;
        s.size = size;
        s.configured = true;
        s.dirty = true;
        if changed {
            self.configure_gpu_surface(i);
        }
        self.surfaces[i].frame_pending = false;
        let id = self.surfaces[i].id;
        self.draw(id);
    }

    fn done(&mut self, _: &Connection, _: &QueueHandle<Self>, popup: &Popup) {
        let owner = self.surfaces.iter().find_map(|s| match &s.role {
            Role::Popup { popup: p, owner, .. } if p == popup => Some(*owner),
            _ => None,
        });
        if let Some(owner) = owner {
            self.handle_menu_events(vec![MenuEvent::CloseAll(owner)]);
        }
    }
}

impl SeatHandler for Kit {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }
    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
    fn new_capability(&mut self, _: &Connection, qh: &QueueHandle<Self>, seat: wl_seat::WlSeat, capability: Capability) {
        self.seat = Some(seat.clone());
        if capability == Capability::Keyboard && self.keyboard.is_none() {
            let kb = self.seat_state.get_keyboard_with_repeat(
                qh,
                &seat,
                None,
                self.loop_handle.clone(),
                Box::new(|kit: &mut Kit, _, event| {
                    kit.key_event(&event, true, true);
                    kit.redraw_due();
                }),
            );
            self.keyboard = kb.ok();
        }
        if capability == Capability::Pointer && self.pointer.is_none() {
            let cursor_surface = self.compositor.create_surface(qh);
            self.pointer = self
                .seat_state
                .get_pointer_with_theme::<Kit, ()>(qh, &seat, self.shm.wl_shm(), cursor_surface, ThemeSpec::default())
                .ok();
        }
        if self.data_device.is_none()
            && let Some(mgr) = &self.data_device_manager
        {
            self.data_device = Some(mgr.get_data_device(qh, &seat));
        }
    }
    fn remove_capability(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat, capability: Capability) {
        if capability == Capability::Keyboard
            && let Some(k) = self.keyboard.take()
        {
            k.release();
        }
        if capability == Capability::Pointer {
            self.pointer = None;
        }
    }
    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl KeyboardHandler for Kit {
    fn enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlKeyboard, surface: &WlSurface, serial: u32, _: &[u32], _: &[Keysym]) {
        self.keyboard_focus = Some(surface.clone());
        self.last_serial = serial;
        self.push_event(&surface.clone(), egui::Event::WindowFocused(true));
    }
    fn leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlKeyboard, surface: &WlSurface, _: u32) {
        if self.keyboard_focus.as_ref() == Some(surface) {
            self.keyboard_focus = None;
        }
        self.push_event(&surface.clone(), egui::Event::WindowFocused(false));
    }
    fn press_key(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlKeyboard, serial: u32, event: KeyEvent) {
        self.last_serial = serial;
        self.key_event(&event, true, false);
    }
    fn repeat_key(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlKeyboard, _: u32, event: KeyEvent) {
        self.key_event(&event, true, true);
    }
    fn release_key(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlKeyboard, _: u32, event: KeyEvent) {
        self.key_event(&event, false, false);
    }
    fn update_modifiers(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlKeyboard, _: u32, m: Modifiers, _: RawModifiers, _: u32) {
        self.modifiers = egui::Modifiers {
            alt: m.alt,
            ctrl: m.ctrl,
            shift: m.shift,
            mac_cmd: false,
            command: m.ctrl,
        };
        if let Some(wl) = self.keyboard_focus.clone() {
            self.push_event(&wl, egui::Event::ModifiersChanged(self.modifiers));
        }
    }
}

impl PointerHandler for Kit {
    fn pointer_frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlPointer, events: &[PointerEvent]) {
        for event in events {
            let pos = pos2(event.position.0 as f32, event.position.1 as f32);
            let wl = event.surface.clone();
            match &event.kind {
                PointerEventKind::Enter { serial } => {
                    self.pointer_focus = Some(wl.clone());
                    self.last_serial = *serial;
                    if let Some(i) = self.index_of_wl(&wl) {
                        self.surfaces[i].pointer = Some(pos);
                        if let Some(p) = &self.pointer {
                            show_cursor(p, &self.conn, self.surfaces[i].cursor);
                        }
                    }
                    self.push_event(&wl, egui::Event::PointerMoved(pos));
                }
                PointerEventKind::Leave { .. } => {
                    if self.pointer_focus.as_ref() == Some(&wl) {
                        self.pointer_focus = None;
                    }
                    // A popup grab takes the pointer between press and
                    // release; don't leave egui thinking a button is held.
                    let mut held = Vec::new();
                    if let Some(i) = self.index_of_wl(&wl) {
                        let last = self.surfaces[i].pointer.take().unwrap_or(pos);
                        held = std::mem::take(&mut self.surfaces[i].buttons)
                            .into_iter()
                            .map(|button| (button, last))
                            .collect();
                    }
                    for (button, at) in held {
                        self.push_event(
                            &wl,
                            egui::Event::PointerButton { pos: at, button, pressed: false, modifiers: self.modifiers },
                        );
                    }
                    self.push_event(&wl, egui::Event::PointerGone);
                }
                PointerEventKind::Motion { .. } => {
                    if let Some(i) = self.index_of_wl(&wl) {
                        self.surfaces[i].pointer = Some(pos);
                    }
                    self.push_event(&wl, egui::Event::PointerMoved(pos));
                }
                PointerEventKind::Press { button, serial, .. } | PointerEventKind::Release { button, serial, .. } => {
                    let pressed = matches!(event.kind, PointerEventKind::Press { .. });
                    if pressed {
                        self.last_serial = *serial;
                    }
                    if let Some(i) = self.index_of_wl(&wl) {
                        let s = &mut self.surfaces[i];
                        if pressed
                            && !matches!(s.role, Role::Popup { .. })
                            && !s.popups.is_empty()
                            && !s.bar_rect.is_some_and(|r| r.contains(pos))
                        {
                            s.swallow = true;
                            let owner = s.id;
                            self.handle_menu_events(vec![MenuEvent::CloseAll(owner)]);
                            continue;
                        }
                        if !pressed && std::mem::take(&mut self.surfaces[i].swallow) {
                            continue;
                        }
                    }
                    if let Some(button) = input::pointer_button(*button) {
                        if let Some(i) = self.index_of_wl(&wl) {
                            let b = &mut self.surfaces[i].buttons;
                            b.retain(|x| *x != button);
                            if pressed {
                                b.push(button);
                            }
                        }
                        self.push_event(
                            &wl,
                            egui::Event::PointerButton { pos, button, pressed, modifiers: self.modifiers },
                        );
                    }
                }
                PointerEventKind::Axis { horizontal, vertical, source, .. } => {
                    use smithay_client_toolkit::reexports::client::protocol::wl_pointer::AxisSource;
                    let wheel = matches!(source, Some(AxisSource::Wheel) | Some(AxisSource::WheelTilt));
                    let (dx, dy) = if wheel && (horizontal.discrete != 0 || vertical.discrete != 0) {
                        (-horizontal.discrete as f32 * 48.0, -vertical.discrete as f32 * 48.0)
                    } else {
                        (-horizontal.absolute as f32, -vertical.absolute as f32)
                    };
                    if dx != 0.0 || dy != 0.0 {
                        self.push_event(
                            &wl,
                            egui::Event::MouseWheel {
                                unit: egui::MouseWheelUnit::Point,
                                source: if wheel { egui::MouseWheelSource::Wheel } else { egui::MouseWheelSource::Trackpad },
                                delta: vec2(dx, dy),
                                phase: egui::TouchPhase::Move,
                                modifiers: self.modifiers,
                            },
                        );
                    }
                }
            }
        }
    }
}

impl ShmHandler for Kit {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl DataDeviceHandler for Kit {
    fn enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice, _: f64, _: f64, _: &WlSurface) {}
    fn leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice) {}
    fn motion(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice, _: f64, _: f64) {}
    fn selection(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice) {}
    fn drop_performed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice) {}
}

impl DataOfferHandler for Kit {
    fn source_actions(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &mut DragOffer, _: DndAction) {}
    fn selected_action(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &mut DragOffer, _: DndAction) {}
}

impl DataSourceHandler for Kit {
    fn accept_mime(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataSource, _: Option<String>) {}
    fn send_request(&mut self, _: &Connection, _: &QueueHandle<Self>, source: &WlDataSource, mime: String, mut fd: WritePipe) {
        if let Some((s, clip)) = &self.clipboard
            && s.inner() == source
            && let Some((_, data)) = clip.iter().find(|(t, _)| *t == mime).or(clip.first())
        {
            // On a thread: the reader may be slow, or be us.
            let data = data.clone();
            std::thread::spawn(move || fd.write_all(&data));
        }
    }
    fn cancelled(&mut self, _: &Connection, _: &QueueHandle<Self>, source: &WlDataSource) {
        if self.clipboard.as_ref().is_some_and(|(s, _)| s.inner() == source) {
            self.clipboard = None;
            if self.linger {
                self.exit = true;
            }
        }
    }
    fn dnd_dropped(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataSource) {}
    fn dnd_finished(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataSource) {}
    fn action(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataSource, _: DndAction) {}
}

impl smithay_client_toolkit::activation::ActivationHandler for Kit {
    type RequestUdata = ();
    fn new_token(
        &mut self,
        token: String,
        data: &smithay_client_toolkit::activation::RequestData<()>,
    ) {
        if let (Some(act), Some(surface)) = (&self.activation, &data.surface) {
            act.activate::<Kit>(surface, token);
        }
    }
}

delegate_registry!(Kit);

impl ProvidesRegistryState for Kit {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState, SeatState];
}

smithay_client_toolkit::delegate_dispatch2!(Kit);

/// Take ⌘H/⌘K/⌘U/⌘W out of this frame's input (see the caller).
fn hold_emacs_keys(ui: &egui::Ui) -> Vec<egui::Event> {
    ui.input_mut(|inp| {
        let (held, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut inp.events).into_iter().partition(|e| {
            matches!(e, egui::Event::Key { key: egui::Key::H | egui::Key::K | egui::Key::U | egui::Key::W, modifiers, .. }
                if modifiers.ctrl && !modifiers.alt)
        });
        inp.events = rest;
        held
    })
}
