//! The Wayland side of the kit: one connection, a calloop event loop, a
//! shared wgpu device, and one egui context per surface (windows, layer
//! surfaces and popup menus).

use std::{
    any::Any,
    io::{Read, Write},
    ptr::NonNull,
    sync::Arc,
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
        viewporter::client::{wp_viewport::WpViewport, wp_viewporter::WpViewporter},
    },
    xdg::shell::client::xdg_positioner,
};

use crate::{
    app::{App, Cx, Layer, Proxy, Request, WindowId, WindowKind, WindowOptions},
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
    Popup {
        popup: Popup,
        /// The window whose menu bar (or context menu) this belongs to.
        owner: WindowId,
        /// Menu index in the owner's bar, then submenu indices.
        path: Vec<usize>,
        state: PopupState,
    },
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
    /// Logical size.
    size: (u32, u32),
    /// Device pixels per logical pixel.
    scale: f64,
    fractional: Option<WpFractionalScaleV1>,
    viewport: Option<WpViewport>,
    configured: bool,
    dirty: bool,
    frame_pending: bool,
    repaint_at: Option<Instant>,
    events: Vec<egui::Event>,
    pointer: Option<Pos2>,
    /// Buttons pressed on this surface and not yet released here.
    buttons: Vec<egui::PointerButton>,
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
    fractional_manager: Option<WpFractionalScaleManagerV1>,
    viewporter: Option<WpViewporter>,

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
    clipboard: Option<(CopyPasteSource, String)>,
    exit: bool,
}

/// Run an app until it quits or closes its last window.
pub fn run<A: App>(app_id: &str, build: impl FnOnce(&mut Cx) -> A) {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .try_init();

    let conn = Connection::connect_to_env().expect("not running under Wayland");
    let (globals, event_queue) = registry_queue_init::<Kit>(&conn).expect("wayland registry");
    let qh = event_queue.handle();
    let mut event_loop: EventLoop<'static, Kit> = EventLoop::try_new().expect("event loop");
    let loop_handle = event_loop.handle();
    WaylandSource::new(conn.clone(), event_queue)
        .insert(loop_handle.clone())
        .expect("wayland source");

    // Messages from app threads.
    let (sender, channel): (_, Channel<Box<dyn Any + Send>>) = channel::channel();
    loop_handle
        .insert_source(channel, |event, _, kit: &mut Kit| {
            if let channel::Event::Msg(msg) = event {
                kit.with_app(|app, cx| app.message(cx, msg));
            }
        })
        .expect("channel");

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

    let mut kit = Kit::new(&globals, conn, qh, loop_handle, palette.clone());
    let mut cx = Cx {
        requests: Vec::new(),
        next_id: 1,
        palette,
        proxy: Proxy { sender },
        app_id: app_id.to_owned(),
    };
    let app = build(&mut cx);
    kit.app = Some(Box::new(app));
    kit.cx = Some(cx);
    kit.process_requests();

    while !kit.exit {
        let timeout = kit.next_timeout();
        if let Err(e) = event_loop.dispatch(timeout, &mut kit) {
            warn!(?e, "event loop error");
            break;
        }
        kit.redraw_due();
        let _ = kit.conn.flush();
    }
    // Surfaces must go before the wayland objects they render to.
    for s in kit.surfaces.iter_mut() {
        s.gpu_surface = None;
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
        Kit {
            registry_state: RegistryState::new(globals),
            seat_state: SeatState::new(globals, &qh),
            output_state: OutputState::new(globals, &qh),
            compositor: CompositorState::bind(globals, &qh).expect("wl_compositor"),
            xdg: XdgShell::bind(globals, &qh).expect("xdg_wm_base"),
            layer_shell: LayerShell::bind(globals, &qh).ok(),
            shm: Shm::bind(globals, &qh).expect("wl_shm"),
            data_device_manager: DataDeviceManagerState::bind(globals, &qh).ok(),
            fractional_manager,
            viewporter,
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
        }
    }

    /// Call into the app with a Cx, then act on what it asked for.
    fn with_app<R>(&mut self, f: impl FnOnce(&mut dyn App, &mut Cx) -> R) -> Option<R> {
        let mut app = self.app.take()?;
        let mut cx = self.cx.take()?;
        cx.palette = self.palette.clone();
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
                return;
            }
            let requests = std::mem::take(&mut cx.requests);
            for r in requests {
                match r {
                    Request::Open(id, options) => self.open(id, options),
                    Request::Close(id) => self.close(id),
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
                    Request::Activate(_) => {}
                    Request::Copy(text) => self.set_clipboard(text),
                    Request::Quit => self.exit = true,
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
        ctx.set_fonts(style::font_definitions());
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
            ctx: self.new_ctx(),
            size,
            scale,
            fractional,
            viewport,
            configured: false,
            dirty: true,
            frame_pending: false,
            repaint_at: None,
            events: Vec::new(),
            pointer: None,
            buttons: Vec::new(),
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
                let surface = shell.create_layer_surface(&self.qh, wl.clone(), layer, Some(lo.namespace.clone()), None);
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
                surface.set_keyboard_interactivity(if lo.keyboard {
                    KeyboardInteractivity::OnDemand
                } else {
                    KeyboardInteractivity::None
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
            .any(|s| matches!(s.role, Role::Toplevel(_) | Role::Layer(_)));
        if !any_window {
            self.exit = true;
        }
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
        let (Some(mgr), Some(dev)) = (&self.data_device_manager, &self.data_device) else {
            return;
        };
        let source = mgr.create_copy_paste_source(
            &self.qh,
            ["text/plain;charset=utf-8", "text/plain", "UTF8_STRING", "STRING", "TEXT"],
        );
        source.set_selection(dev, self.last_serial);
        self.clipboard = Some((source, text));
    }

    fn read_clipboard(&self) -> Option<String> {
        if let Some((_, text)) = &self.clipboard {
            return Some(text.clone());
        }
        let offer = self.data_device.as_ref()?.data().selection_offer()?;
        let mime = offer.with_mime_types(|types| {
            ["text/plain;charset=utf-8", "UTF8_STRING", "text/plain"]
                .iter()
                .find(|m| types.iter().any(|t| t == *m))
                .map(|m| m.to_string())
        })?;
        let mut pipe = offer.receive(mime).ok()?;
        let _ = self.conn.flush();
        // Read on a helper thread so a stuck source can't freeze the UI.
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut out = Vec::new();
            let _ = pipe.read_to_end(&mut out);
            let _ = tx.send(out);
        });
        let out = rx.recv_timeout(Duration::from_millis(500)).ok()?;
        String::from_utf8(out).ok()
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
        let alpha_mode = if matches!(s.role, Role::Popup { .. })
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
        let s = &mut self.surfaces[i];
        s.dirty = false;
        let mut raw = egui::RawInput {
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
        let open_source = if s.popups.is_empty() { None } else { s.context.as_ref().map(|r| r.source) };
        menu::set_open_source(&ctx, open_source);
        let kind = match &s.role {
            Role::Toplevel(_) => 0,
            Role::Layer(_) => 1,
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
            let menus = app.as_mut().and_then(|a| a.menu_bar(id));
            let panel = s.options.panel;
            let palette = self.palette.clone();
            let out = ctx.run_ui(raw, |ui| {
                if panel {
                    ui.painter().rect_filled(ui.max_rect(), 0.0, style::c(palette.panel_background));
                }
                if let Some(menus) = &menus {
                    let bar_rect = menu::menu_bar(ui, menus, &mut bar);
                    let mut rect = ui.max_rect();
                    rect.min.y = bar_rect.max.y;
                    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
                    if let (Some(app), Some(cx)) = (app.as_mut(), cx.as_mut()) {
                        app.ui(cx, id, &mut child);
                    }
                    // Command-key shortcuts.
                    let pressed: Vec<(egui::Key, bool)> = ui.input(|inp| {
                        inp.events
                            .iter()
                            .filter_map(|e| match e {
                                egui::Event::Key { key, pressed: true, modifiers, .. }
                                    if modifiers.ctrl && !modifiers.alt =>
                                {
                                    Some((*key, modifiers.shift))
                                }
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
                }
            });
            self.app = app;
            self.cx = cx;
            let s = &mut self.surfaces[i];
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
                    let _ = std::process::Command::new("xdg-open").arg(&u.url).spawn();
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
                let _ = p.set_cursor(&self.conn, input::cursor_icon(s.cursor));
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
                        if depth == 0 {
                            if let Some(s) = self.surface_mut(owner) {
                                s.bar.open = None;
                                s.dirty = true;
                            }
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
        if pressed && m.ctrl {
            match key {
                Some(egui::Key::C) if !m.shift => return self.push_event(&wl, egui::Event::Copy),
                Some(egui::Key::X) if !m.shift => return self.push_event(&wl, egui::Event::Cut),
                Some(egui::Key::V) if !m.shift => {
                    if let Some(text) = self.read_clipboard() {
                        self.push_event(&wl, egui::Event::Paste(text));
                    }
                    return;
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
            && !m.alt
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
impl Dispatch2<WpViewporter, Kit> for KitData {
    fn event(&self, _: &mut Kit, _: &WpViewporter, _: <WpViewporter as wayland_client::Proxy>::Event, _: &Connection, _: &QueueHandle<Kit>) {}
}
impl Dispatch2<WpViewport, Kit> for KitData {
    fn event(&self, _: &mut Kit, _: &WpViewport, _: <WpViewport as wayland_client::Proxy>::Event, _: &Connection, _: &QueueHandle<Kit>) {}
}

impl OutputHandler for Kit {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
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
                        let icon = input::cursor_icon(self.surfaces[i].cursor);
                        if let Some(p) = &self.pointer {
                            let _ = p.set_cursor(&self.conn, icon);
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
    fn send_request(&mut self, _: &Connection, _: &QueueHandle<Self>, source: &WlDataSource, _mime: String, mut fd: WritePipe) {
        if let Some((s, text)) = &self.clipboard
            && s.inner() == source
        {
            let _ = fd.write_all(text.as_bytes());
        }
    }
    fn cancelled(&mut self, _: &Connection, _: &QueueHandle<Self>, source: &WlDataSource) {
        if self.clipboard.as_ref().is_some_and(|(s, _)| s.inner() == source) {
            self.clipboard = None;
        }
    }
    fn dnd_dropped(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataSource) {}
    fn dnd_finished(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataSource) {}
    fn action(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataSource, _: DndAction) {}
}

delegate_registry!(Kit);

impl ProvidesRegistryState for Kit {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState, SeatState];
}

smithay_client_toolkit::delegate_dispatch2!(Kit);
