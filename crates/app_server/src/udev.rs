// Allow in this module because of existing usage
#![allow(clippy::uninlined_format_args)]
use std::{
    collections::hash_map::HashMap,
    io,
    ops::Not,
    path::Path,
    sync::{Once, atomic::Ordering},
    time::{Duration, Instant},
};

use crate::{
    drawing::*,
    render::*,
    shell::WindowElement,
    state::{AnvilState, Backend, take_presentation_feedback, update_primary_scanout_output},
};
use crate::{
    shell::WindowRenderElement,
    state::{DndIcon, SurfaceDmabufFeedback},
};
#[cfg(feature = "renderer_sync")]
use smithay::backend::drm::compositor::PrimaryPlaneElement;
#[cfg(feature = "egl")]
use smithay::backend::renderer::ImportEgl;
#[cfg(feature = "debug")]
use smithay::backend::renderer::{ImportMem, multigpu::MultiTexture};
use smithay::{
    backend::{
        SwapBuffersError,
        allocator::{
            Fourcc, Modifier,
            dmabuf::Dmabuf,
            format::FormatSet,
            gbm::{GbmAllocator, GbmBufferFlags, GbmDevice},
        },
        drm::{
            CreateDrmNodeError, DrmAccessError, DrmDevice, DrmDeviceFd, DrmError, DrmEvent, DrmEventMetadata,
            DrmEventTime, DrmNode, DrmSurface, GbmBufferedSurface, NodeType,
            compositor::{DrmCompositor, FrameFlags},
            exporter::gbm::GbmFramebufferExporter,
            output::{DrmOutput, DrmOutputManager, DrmOutputRenderElements},
        },
        egl::{self, EGLContext, EGLDevice, EGLDisplay, context::ContextPriority},
        input::InputEvent,
        libinput::{LibinputInputBackend, LibinputSessionInterface},
        renderer::{
            DebugFlags, ImportDma, ImportMemWl,
            damage::Error as OutputDamageTrackerError,
            element::{AsRenderElements, RenderElementStates, memory::MemoryRenderBuffer},
            gles::{Capability, GlesRenderer},
            multigpu::{GpuManager, MultiRenderer, gbm::GbmGlesBackend},
        },
        session::{
            Event as SessionEvent, Session,
            libseat::{self, LibSeatSession},
        },
        udev::{UdevBackend, UdevEvent, all_gpus, primary_gpu},
    },
    desktop::{
        space::{Space, SurfaceTree},
        utils::OutputPresentationFeedback,
    },
    input::{
        keyboard::LedState,
        pointer::CursorImageStatus,
    },
    output::{Mode as WlMode, Output, PhysicalProperties},
    reexports::{
        calloop::{
            EventLoop, RegistrationToken,
            timer::{TimeoutAction, Timer},
        },
        drm::{
            Device as _,
            control::{AtomicCommitFlags, Device, ModeTypeFlags, connector, crtc},
        },
        input::{DeviceCapability, Libinput},
        rustix::fs::OFlags,
        wayland_protocols::wp::{
            linux_dmabuf::zv1::server::zwp_linux_dmabuf_feedback_v1,
            presentation_time::server::wp_presentation_feedback,
        },
        wayland_server::{Display, DisplayHandle, backend::GlobalId, protocol::wl_surface},
    },
    utils::{DeviceFd, IsAlive, Logical, Monotonic, Point, Scale, Time},
    wayland::{
        dmabuf::{DmabufFeedbackBuilder, DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier},
        drm_lease::{
            DrmLease, DrmLeaseBuilder, DrmLeaseHandler, DrmLeaseRequest, DrmLeaseState, LeaseRejected,
        },
        drm_syncobj::{DrmSyncobjHandler, DrmSyncobjState, supports_syncobj_eventfd},
        presentation::Refresh,
    },
};
use smithay_drm_extras::{
    display_info,
    drm_scanner::{DrmScanEvent, DrmScanner},
};
use tracing::{debug, error, info, trace, warn};

// we cannot simply pick the first supported format of the intersection of *all* formats, because:
// - we do not want something like Abgr4444, which looses color information, if something better is available
// - some formats might perform terribly
// - we might need some work-arounds, if one supports modifiers, but the other does not
//
// So lets just pick `ARGB2101010` (10-bit) or `ARGB8888` (8-bit) for now, they are widely supported.
const SUPPORTED_FORMATS: &[Fourcc] = &[
    Fourcc::Abgr2101010,
    Fourcc::Argb2101010,
    Fourcc::Abgr8888,
    Fourcc::Argb8888,
];
const SUPPORTED_FORMATS_8BIT_ONLY: &[Fourcc] = &[Fourcc::Abgr8888, Fourcc::Argb8888];

type UdevRenderer<'a> = MultiRenderer<
    'a,
    'a,
    GbmGlesBackend<GlesRenderer, DrmDeviceFd>,
    GbmGlesBackend<GlesRenderer, DrmDeviceFd>,
>;

#[derive(Debug, PartialEq)]
struct UdevOutputId {
    device_id: DrmNode,
    crtc: crtc::Handle,
}

pub struct UdevData {
    pub session: LibSeatSession,
    dh: DisplayHandle,
    dmabuf_state: Option<(DmabufState, DmabufGlobal)>,
    syncobj_state: Option<DrmSyncobjState>,
    primary_gpu: DrmNode,
    gpus: GpuManager<GbmGlesBackend<GlesRenderer, DrmDeviceFd>>,
    backends: HashMap<DrmNode, BackendData>,
    pointer_element: PointerElement,
    #[cfg(feature = "debug")]
    fps_texture: Option<MultiTexture>,
    debug_flags: DebugFlags,
    keyboards: Vec<smithay::reexports::input::Device>,
    /// Touchpads and mice, configured from Preferences ▸ Touchpad.
    pointers: Vec<smithay::reexports::input::Device>,
    /// The settings last applied, for devices plugged in later.
    input_settings: Option<tabula_protocol::Settings>,
    /// Night-light gamma gains, for screens connected later.
    white_point: [f64; 3],
    /// The screens are off for idleness: nothing is rendered (a frame
    /// would turn them back on).
    displays_off: bool,
    loop_handle: smithay::reexports::calloop::LoopHandle<'static, AnvilState<UdevData>>,
}

impl UdevData {
    pub fn set_debug_flags(&mut self, flags: DebugFlags) {
        if self.debug_flags != flags {
            self.debug_flags = flags;

            for backend in self.backends.values_mut() {
                for surface in backend.surfaces.values_mut() {
                    surface.drm_output.set_debug_flags(flags);
                }
            }
        }
    }

    pub fn debug_flags(&self) -> DebugFlags {
        self.debug_flags
    }
}

impl DmabufHandler for AnvilState<UdevData> {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.backend_data.dmabuf_state.as_mut().unwrap().0
    }

    fn dmabuf_imported(&mut self, _global: &DmabufGlobal, dmabuf: Dmabuf, notifier: ImportNotifier) {
        if self
            .backend_data
            .gpus
            .single_renderer(&self.backend_data.primary_gpu)
            .and_then(|mut renderer| renderer.import_dmabuf(&dmabuf, None))
            .is_ok()
        {
            if dmabuf.node().is_none() {
                dmabuf.set_node(self.backend_data.primary_gpu);
            }
            let _ = notifier.successful::<AnvilState<UdevData>>();
        } else {
            notifier.failed();
        }
    }
}

impl Backend for UdevData {
    const HAS_RELATIVE_MOTION: bool = true;
    const HAS_GESTURES: bool = true;
    const IS_SESSION: bool = true;

    fn seat_name(&self) -> String {
        self.session.seat()
    }

    fn reset_buffers(&mut self, output: &Output) {
        if let Some(id) = output.user_data().get::<UdevOutputId>()
            && let Some(gpu) = self.backends.get_mut(&id.device_id)
                && let Some(surface) = gpu.surfaces.get_mut(&id.crtc) {
                    surface.drm_output.reset_buffers();
                }
    }

    fn early_import(&mut self, surface: &wl_surface::WlSurface) {
        if let Err(err) = self.gpus.early_import(self.primary_gpu, surface) {
            warn!("Early buffer import failed: {}", err);
        }
    }

    fn update_led_state(&mut self, led_state: LedState) {
        for keyboard in self.keyboards.iter_mut() {
            keyboard.led_update(led_state.into());
        }
    }

    fn set_displays_on(&mut self, on: bool) {
        if self.displays_off != on {
            return;
        }
        self.displays_off = !on;
        if on {
            // The next frame on each turns it back on.
            for &node in self.backends.keys() {
                self.loop_handle.insert_idle(move |state| state.render(node, None, state.clock.now()));
            }
            info!("displays on");
            return;
        }
        for backend in self.backends.values_mut() {
            for surface in backend.surfaces.values_mut() {
                if let Err(e) = surface.drm_output.with_compositor(|c| c.clear()) {
                    warn!(?e, "couldn't turn a display off");
                }
            }
        }
        info!("displays off");
    }

    fn set_white_point(&mut self, gains: [f64; 3]) {
        self.white_point = gains;
        for backend in self.backends.values() {
            for crtc in backend.surfaces.keys() {
                set_gamma(backend, *crtc, gains);
            }
        }
    }

    fn apply_input_settings(&mut self, settings: &tabula_protocol::Settings) {
        for device in self.pointers.iter_mut() {
            configure_pointer(device, settings);
        }
        self.input_settings = Some(settings.clone());
    }

    fn set_mode(&mut self, output: &Output, mode: WlMode) -> Result<(), String> {
        let id = output.user_data().get::<UdevOutputId>().ok_or("not a DRM output")?;
        let primary = self.primary_gpu;
        let device = self.backends.get_mut(&id.device_id).ok_or("its GPU is gone")?;
        let render_node = device.render_node.unwrap_or(primary);
        let surface = device.surfaces.get_mut(&id.crtc).ok_or("its CRTC is gone")?;
        let drm_mode = *surface
            .drm_modes
            .iter()
            .find(|m| WlMode::from(**m) == mode)
            .ok_or("the display doesn't offer that mode")?;
        let mut renderer = self.gpus.single_renderer(&render_node).map_err(|e| e.to_string())?;
        surface
            .drm_output
            .use_mode::<_, OutputRenderElements<UdevRenderer<'_>, WindowRenderElement<UdevRenderer<'_>>>>(
                drm_mode,
                &mut renderer,
                &DrmOutputRenderElements::default(),
            )
            .map_err(|e| e.to_string())?;
        output.change_current_state(Some(mode), None, None, None);
        info!(output = output.name(), ?mode, "mode changed");
        Ok(())
    }
}

/// Load a CRTC's gamma ramps scaled by `gains` (night light).
fn set_gamma(backend: &BackendData, crtc: crtc::Handle, gains: [f64; 3]) {
    let fd = backend.drm_output_manager.device().device_fd();
    let len = match fd.get_crtc(crtc) {
        Ok(info) => info.gamma_length() as usize,
        Err(e) => return warn!(?e, "no CRTC for night light"),
    };
    if len < 2 {
        return set_ctm(backend, crtc, gains, AtomicCommitFlags::NONBLOCK);
    }
    let [r, g, b] = crate::night::ramps(len, gains);
    if let Err(e) = fd.set_gamma(crtc, &r, &g, &b) {
        warn!(?e, "this display can't take a gamma ramp; night light is off");
    }
}

/// Night light on displays with no gamma ramp but a colour matrix (Apple's
/// display controller): a diagonal CTM scales the channels. It goes in an
/// atomic commit of its own, which mustn't block the compositor: when a
/// page flip is still in flight it is refused, and tried again at that
/// screen's next vblank (`retry_ctm`).
fn set_ctm(backend: &BackendData, crtc: crtc::Handle, gains: [f64; 3], flags: AtomicCommitFlags) {
    use smithay::reexports::drm::control::{atomic::AtomicModeReq, property};
    let device = backend.drm_output_manager.device();
    if !device.is_atomic() {
        return;
    }
    let fd = device.device_fd();
    let mut ctm = backend.ctm.borrow_mut();
    let state = ctm.entry(crtc).or_default();
    let prop = *state.prop.get_or_insert_with(|| {
        let props = fd.get_properties(crtc).ok()?;
        props.iter().map(|(p, _)| *p).find(|p| fd.get_property(*p).is_ok_and(|i| i.name().to_bytes() == b"CTM"))
    });
    let Some(prop) = prop else {
        state.pending = None;
        return;
    };
    let identity = gains.iter().all(|g| (g - 1.0).abs() < 1e-4);
    if identity && state.blob == 0 {
        state.pending = None;
        return;
    }
    let blob = if identity {
        0
    } else {
        match fd.create_property_blob(&crate::night::ctm(gains)) {
            Ok(property::Value::Blob(b)) => b,
            Ok(_) => return,
            Err(e) => return warn!(?e, "couldn't make a colour matrix for night light"),
        }
    };
    let mut req = AtomicModeReq::new();
    req.add_property(crtc, prop, property::Value::Blob(blob));
    match fd.atomic_commit(flags, req) {
        Ok(()) => {
            let old = std::mem::replace(&mut state.blob, blob);
            if old != 0 {
                let _ = fd.destroy_property_blob(old);
            }
            state.pending = None;
        }
        Err(e) => {
            if blob != 0 {
                let _ = fd.destroy_property_blob(blob);
            }
            if e.kind() == std::io::ErrorKind::ResourceBusy {
                state.pending = Some(gains);
            } else {
                state.pending = None;
                warn!(?e, "this display can't take a colour matrix; night light is off");
            }
        }
    }
}

/// A colour matrix refused while a flip was in flight, again (at vblank).
fn retry_ctm(backend: &BackendData, crtc: crtc::Handle) {
    let pending = backend.ctm.borrow().get(&crtc).and_then(|c| c.pending);
    if let Some(gains) = pending {
        set_ctm(backend, crtc, gains, AtomicCommitFlags::NONBLOCK);
    }
}

/// Night light's colour matrix on one screen.
#[derive(Default)]
struct Ctm {
    /// The CRTC's CTM property (`Some(None)`: it has none).
    prop: Option<Option<smithay::reexports::drm::control::property::Handle>>,
    /// The blob in use (0: none, the identity).
    blob: u64,
    /// Gains still to be set.
    pending: Option<[f64; 3]>,
}

/// Preferences ▸ Touchpad into libinput: tapping, natural scrolling,
/// typing palm rejection, click method and speed for touchpads; speed for
/// mice; the rest as a Mac trackpad behaves. Options a device lacks are
/// skipped. (Momentum scrolling is app_server's own: `momentum.rs`.)
fn configure_pointer(device: &mut smithay::reexports::input::Device, settings: &tabula_protocol::Settings) {
    use smithay::reexports::input::{ClickMethod, DragLockState, ScrollMethod, TapButtonMap};
    let t = &settings.touchpad;
    let touchpad = device.config_tap_finger_count() > 0;
    if touchpad {
        let _ = device.config_tap_set_enabled(t.tap_to_click);
        let _ = device.config_tap_set_drag_enabled(t.tap_and_drag);
        // As on a Mac: a two-finger tap is the secondary click, a drag
        // ends when the finger lifts, and nothing becomes a middle click
        // (which would paste the selection somewhere by surprise).
        let _ = device.config_tap_set_button_map(TapButtonMap::LeftRightMiddle);
        let _ = device.config_tap_set_drag_lock_enabled(DragLockState::Disabled);
        if device.config_middle_emulation_is_available() {
            let _ = device.config_middle_emulation_set_enabled(false);
        }
        if device.config_dwt_is_available() {
            let _ = device.config_dwt_set_enabled(t.disable_while_typing);
        }
        let methods = device.config_click_methods();
        let method = if t.clickfinger { ClickMethod::Clickfinger } else { ClickMethod::ButtonAreas };
        if methods.contains(&method) {
            let _ = device.config_click_set_method(method);
        }
        if device.config_scroll_methods().contains(&ScrollMethod::TwoFinger) {
            let _ = device.config_scroll_set_method(ScrollMethod::TwoFinger);
        }
    }
    if device.config_scroll_has_natural_scroll() {
        let _ = device.config_scroll_set_natural_scroll_enabled(t.natural_scroll);
    }
    if device.config_accel_is_available() {
        let speed = if touchpad { t.speed } else { settings.mouse_speed };
        let _ = device.config_accel_set_speed(speed.clamp(-1.0, 1.0));
    }
    info!(device = %device.name(), touchpad, "applied pointer settings");
}

/// `--probe-gpus`: what each DRM device offers, as the session would
/// see it at start-up (diagnostics for new hardware). Opens the devices
/// unprivileged and leaves connectors alone, so it is safe beside a
/// running desktop.
pub fn probe_gpus() {
    let seat = std::env::var("XDG_SEAT").unwrap_or_else(|_| "seat0".into());
    println!("seat {seat}; boot GPU: {:?}", primary_gpu(&seat).ok().flatten());
    println!("chosen primary: {:?}", choose_primary_gpu(&seat));
    for path in all_gpus(&seat).unwrap_or_default() {
        println!("{}", path.display());
        let Ok(node) = DrmNode::from_path(&path) else {
            println!("  not a DRM node");
            continue;
        };
        println!("  render node: {:?}", node.node_with_type(NodeType::Render).and_then(|n| n.ok()));
        let file = match std::fs::OpenOptions::new().read(true).write(true).open(&path) {
            Ok(f) => f,
            Err(e) => {
                println!("  can't open: {e}");
                continue;
            }
        };
        let fd = DrmDeviceFd::new(DeviceFd::from(std::os::fd::OwnedFd::from(file)));
        match DrmDevice::new(fd.clone(), false) {
            Ok((drm, _)) => match drm.resource_handles() {
                Ok(r) => println!("  KMS: {} connectors, {} CRTCs", r.connectors().len(), r.crtcs().len()),
                Err(e) => println!("  KMS: no resources ({e})"),
            },
            Err(e) => println!("  KMS: {e}"),
        }
        match GbmDevice::new(fd) {
            Ok(gbm) => match unsafe { EGLDisplay::new(gbm) } {
                Ok(display) => match EGLDevice::device_for_display(&display) {
                    Ok(d) => println!("  EGL: software {}, render node {:?}", d.is_software(), d.try_get_render_node()),
                    Err(e) => println!("  EGL device: {e}"),
                },
                Err(e) => println!("  EGL: {e}"),
            },
            Err(e) => println!("  GBM: {e}"),
        }
    }
}

/// The GPU to render with: the boot GPU's render node, or else the first
/// device that has a render node. (On Apple silicon there is no boot GPU,
/// and the display controller, apple-drm, is a DRM device that can't
/// render: asahi's node does that.)
fn choose_primary_gpu(seat: &str) -> Option<DrmNode> {
    if let Ok(var) = std::env::var("ANVIL_DRM_DEVICE") {
        return DrmNode::from_path(var).ok();
    }
    let render = |path: &Path| DrmNode::from_path(path).ok()?.node_with_type(NodeType::Render)?.ok();
    if let Some(node) = primary_gpu(seat).ok().flatten().and_then(|p| render(&p)) {
        return Some(node);
    }
    let all = all_gpus(seat).unwrap_or_default();
    all.iter().find_map(|p| render(p)).or_else(|| all.iter().find_map(|p| DrmNode::from_path(p).ok()))
}

pub fn run_udev() {
    let mut event_loop = EventLoop::try_new().unwrap();
    let display = Display::new().unwrap();
    let mut display_handle = display.handle();

    /*
     * Initialize session
     */
    let (session, notifier) = match LibSeatSession::new() {
        Ok(ret) => ret,
        Err(err) => {
            error!("Could not initialize a session: {}", err);
            return;
        }
    };

    /*
     * Initialize the compositor
     */
    let Some(primary_gpu) = choose_primary_gpu(&session.seat()) else {
        error!("No GPU!");
        return;
    };
    info!("Using {} as primary gpu.", primary_gpu);

    let gpus = GpuManager::new(GbmGlesBackend::with_factory(|display| {
        let context = EGLContext::new_with_priority(display, ContextPriority::High)?;
        let mut capabilities = unsafe { GlesRenderer::supported_capabilities(&context)? };
        if std::env::var("ANVIL_GLES_DISABLE_INSTANCING").is_ok() {
            capabilities.retain(|capability| *capability != Capability::Instancing);
        }
        Ok(unsafe { GlesRenderer::with_capabilities(context, capabilities)? })
    }))
    .unwrap();

    let data = UdevData {
        dh: display_handle.clone(),
        dmabuf_state: None,
        syncobj_state: None,
        session,
        primary_gpu,
        gpus,
        backends: HashMap::new(),
        pointer_element: PointerElement::default(),
        #[cfg(feature = "debug")]
        fps_texture: None,
        debug_flags: DebugFlags::empty(),
        keyboards: Vec::new(),
        pointers: Vec::new(),
        input_settings: None,
        white_point: [1.0; 3],
        displays_off: false,
        loop_handle: event_loop.handle(),
    };
    let mut state = AnvilState::init(display, event_loop.handle(), data, true);

    /*
     * Initialize the udev backend
     */
    let udev_backend = match UdevBackend::new(&state.seat_name) {
        Ok(ret) => ret,
        Err(err) => {
            error!(error = ?err, "Failed to initialize udev backend");
            return;
        }
    };

    /*
     * Initialize libinput backend
     */
    let mut libinput_context = Libinput::new_with_udev::<LibinputSessionInterface<LibSeatSession>>(
        state.backend_data.session.clone().into(),
    );
    libinput_context.udev_assign_seat(&state.seat_name).unwrap();
    let libinput_backend = LibinputInputBackend::new(libinput_context.clone());

    /*
     * Bind all our objects that get driven by the event loop
     */
    event_loop
        .handle()
        .insert_source(libinput_backend, move |mut event, _, data| {
            let dh = data.backend_data.dh.clone();
            if let InputEvent::DeviceAdded { device } = &mut event {
                if device.has_capability(DeviceCapability::Keyboard) {
                    if let Some(led_state) = data.seat.get_keyboard().map(|keyboard| keyboard.led_state()) {
                        device.led_update(led_state.into());
                    }
                    data.backend_data.keyboards.push(device.clone());
                }
                if device.has_capability(DeviceCapability::Pointer) {
                    if let Some(settings) = &data.backend_data.input_settings {
                        configure_pointer(device, settings);
                    }
                    data.backend_data.pointers.push(device.clone());
                }
            } else if let InputEvent::DeviceRemoved { ref device } = event {
                data.backend_data.keyboards.retain(|item| item != device);
                data.backend_data.pointers.retain(|item| item != device);
            }

            data.process_input_event(&dh, event)
        })
        .unwrap();

    event_loop
        .handle()
        .insert_source(notifier, move |event, &mut (), data| match event {
            SessionEvent::PauseSession => {
                libinput_context.suspend();
                info!("pausing session");

                for backend in data.backend_data.backends.values_mut() {
                    backend.drm_output_manager.pause();
                    backend.active_leases.clear();
                    if let Some(lease_global) = backend.leasing_global.as_mut() {
                        lease_global.suspend();
                    }
                }
            }
            SessionEvent::ActivateSession => {
                info!("resuming session");

                if let Err(err) = libinput_context.resume() {
                    error!("Failed to resume libinput context: {:?}", err);
                }
                for (node, backend) in data
                    .backend_data
                    .backends
                    .iter_mut()
                    .map(|(handle, backend)| (*handle, backend))
                {
                    // if we do not care about flicking (caused by modesetting) we could just
                    // pass true for disable connectors here. this would make sure our drm
                    // device is in a known state (all connectors and planes disabled).
                    // but for demonstration we choose a more optimistic path by leaving the
                    // state as is and assume it will just work. If this assumption fails
                    // we will try to reset the state when trying to queue a frame.
                    backend
                        .drm_output_manager
                        .lock()
                        .activate(false)
                        .expect("failed to activate drm backend");
                    if let Some(lease_global) = backend.leasing_global.as_mut() {
                        lease_global.resume::<AnvilState<UdevData>>();
                    }
                    data.handle
                        .insert_idle(move |data| data.render(node, None, data.clock.now()));
                }
                // Whoever had the screens may have changed their ramps.
                let gains = data.backend_data.white_point;
                data.backend_data.set_white_point(gains);
            }
        })
        .unwrap();

    // We try to initialize the primary node before others to make sure
    // any display only node can fall back to the primary node for rendering
    let primary_node = primary_gpu
        .node_with_type(NodeType::Primary)
        .and_then(|node| node.ok());
    let primary_device = udev_backend.device_list().find(|(device_id, _)| {
        primary_node
            .map(|primary_node| *device_id == primary_node.dev_id())
            .unwrap_or(false)
            || *device_id == primary_gpu.dev_id()
    });

    if let Some((device_id, path)) = primary_device {
        // A GPU without displays (Apple silicon's asahi) has no modesetting:
        // nothing to add, and the display devices render on it through
        // their own EGL (Mesa's kmsro), whose render node is this one.
        match DrmNode::from_dev_id(device_id).map_err(DeviceAddError::DrmNode).and_then(|node| state.device_added(node, path)) {
            Ok(()) => {}
            Err(err) => warn!("Primary GPU {device_id} has no displays of its own: {err}"),
        }
    }

    let primary_device_id = primary_device.map(|(device_id, _)| device_id);
    for (device_id, path) in udev_backend.device_list() {
        if Some(device_id) == primary_device_id {
            continue;
        }

        if let Err(err) = DrmNode::from_dev_id(device_id)
            .map_err(DeviceAddError::DrmNode)
            .and_then(|node| state.device_added(node, path))
        {
            error!("Skipping device {device_id}: {err}");
        }
    }
    let shm_formats = match state.backend_data.gpus.single_renderer(&primary_gpu) {
        Ok(r) => r.shm_formats(),
        Err(err) => {
            error!("Nothing can render on {primary_gpu}: {err}");
            return;
        }
    };
    state.shm_state.update_formats(shm_formats);

    #[cfg_attr(not(feature = "egl"), allow(unused_mut))]
    let mut renderer = state.backend_data.gpus.single_renderer(&primary_gpu).unwrap();

    #[cfg(feature = "debug")]
    {
        #[allow(deprecated)]
        let fps_image =
            image::io::Reader::with_format(std::io::Cursor::new(FPS_NUMBERS_PNG), image::ImageFormat::Png)
                .decode()
                .unwrap();
        let fps_texture = renderer
            .import_memory(
                &fps_image.to_rgba8(),
                Fourcc::Abgr8888,
                (fps_image.width() as i32, fps_image.height() as i32).into(),
                false,
            )
            .expect("Unable to upload FPS texture");

        for backend in state.backend_data.backends.values_mut() {
            for surface in backend.surfaces.values_mut() {
                surface.fps_element = Some(FpsElement::new(fps_texture.clone()));
            }
        }
        state.backend_data.fps_texture = Some(fps_texture);
    }

    #[cfg(feature = "egl")]
    {
        info!(?primary_gpu, "Trying to initialize EGL Hardware Acceleration",);
        match renderer.bind_wl_display(&display_handle) {
            Ok(_) => info!("EGL hardware-acceleration enabled"),
            Err(err) => info!(?err, "Failed to initialize EGL hardware-acceleration"),
        }
    }

    // init dmabuf support with format list from our primary gpu
    let dmabuf_formats = renderer.dmabuf_formats();
    let default_feedback = DmabufFeedbackBuilder::new(primary_gpu.dev_id(), dmabuf_formats)
        .build()
        .unwrap();
    let mut dmabuf_state = DmabufState::new();
    let global = dmabuf_state
        .create_global_with_default_feedback::<AnvilState<UdevData>>(&display_handle, &default_feedback);
    state.backend_data.dmabuf_state = Some((dmabuf_state, global));

    let gpus = &mut state.backend_data.gpus;
    state
        .backend_data
        .backends
        .iter_mut()
        .for_each(|(node, backend_data)| {
            // Update the per drm surface dmabuf feedback
            backend_data.surfaces.values_mut().for_each(|surface_data| {
                surface_data.dmabuf_feedback = surface_data.dmabuf_feedback.take().or_else(|| {
                    surface_data.drm_output.with_compositor(|compositor| {
                        get_surface_dmabuf_feedback(
                            primary_gpu,
                            surface_data.render_node,
                            *node,
                            gpus,
                            compositor.surface(),
                        )
                    })
                });
            });
        });

    // Expose syncobj protocol if supported by primary GPU
    if let Some(primary_node) = state
        .backend_data
        .primary_gpu
        .node_with_type(NodeType::Primary)
        .and_then(|x| x.ok())
        && let Some(backend) = state.backend_data.backends.get(&primary_node) {
            let import_device = backend.drm_output_manager.device().device_fd().clone();
            if supports_syncobj_eventfd(&import_device) {
                let syncobj_state =
                    DrmSyncobjState::new::<AnvilState<UdevData>>(&display_handle, import_device);
                state.backend_data.syncobj_state = Some(syncobj_state);
            }
        }

    event_loop
        .handle()
        .insert_source(udev_backend, move |event, _, data| match event {
            UdevEvent::Added { device_id, path } => {
                if let Err(err) = DrmNode::from_dev_id(device_id)
                    .map_err(DeviceAddError::DrmNode)
                    .and_then(|node| data.device_added(node, &path))
                {
                    error!("Skipping device {device_id}: {err}");
                }
            }
            UdevEvent::Changed { device_id } => {
                if let Ok(node) = DrmNode::from_dev_id(device_id) {
                    data.device_changed(node)
                }
            }
            UdevEvent::Removed { device_id } => {
                if let Ok(node) = DrmNode::from_dev_id(device_id) {
                    data.device_removed(node)
                }
            }
        })
        .unwrap();

    #[cfg(feature = "libei")]
    crate::libei::listen_eis(&event_loop.handle());

    /*
     * And run our loop
     */

    while state.running.load(Ordering::SeqCst) {
        let result = event_loop.dispatch(Some(Duration::from_millis(16)), &mut state);
        if result.is_err() {
            state.running.store(false, Ordering::SeqCst);
        } else {
            state.space.refresh();
            state.popups.cleanup();
            state.tick();
            display_handle.flush_clients().unwrap();
        }
    }
    state.save_layout();
    // A colour matrix outlives us: leave the screens as they were.
    for backend in state.backend_data.backends.values() {
        let crtcs: Vec<crtc::Handle> = backend.ctm.borrow().keys().copied().collect();
        for crtc in crtcs {
            set_ctm(backend, crtc, [1.0; 3], AtomicCommitFlags::empty());
        }
    }
    // Leave the displays showing what they show: the next app_server (an
    // upgrade, a restart) or the greeter takes them from there. Smithay
    // would otherwise put back the state from before we started, which
    // flickers and, with DRM master already handed back, only fails
    // ("Failed to restore previous state … Permission denied").
    for backend in state.backend_data.backends.values_mut() {
        backend.drm_output_manager.pause();
    }
}

impl DrmLeaseHandler for AnvilState<UdevData> {
    fn drm_lease_state(&mut self, node: DrmNode) -> &mut DrmLeaseState {
        self.backend_data
            .backends
            .get_mut(&node)
            .unwrap()
            .leasing_global
            .as_mut()
            .unwrap()
    }

    fn lease_request(
        &mut self,
        node: DrmNode,
        request: DrmLeaseRequest,
    ) -> Result<DrmLeaseBuilder, LeaseRejected> {
        let backend = self
            .backend_data
            .backends
            .get(&node)
            .ok_or(LeaseRejected::default())?;

        let drm_device = backend.drm_output_manager.device();
        let mut builder = DrmLeaseBuilder::new(drm_device);
        for conn in request.connectors {
            if let Some((_, crtc)) = backend
                .non_desktop_connectors
                .iter()
                .find(|(handle, _)| *handle == conn)
            {
                builder.add_connector(conn);
                builder.add_crtc(*crtc);
                let planes = drm_device.planes(crtc).map_err(LeaseRejected::with_cause)?;
                let (primary_plane, primary_plane_claim) = planes
                    .primary
                    .iter()
                    .find_map(|plane| {
                        drm_device
                            .claim_plane(plane.handle, *crtc)
                            .map(|claim| (plane, claim))
                    })
                    .ok_or_else(LeaseRejected::default)?;
                builder.add_plane(primary_plane.handle, primary_plane_claim);
                if let Some((cursor, claim)) = planes.cursor.iter().find_map(|plane| {
                    drm_device
                        .claim_plane(plane.handle, *crtc)
                        .map(|claim| (plane, claim))
                }) {
                    builder.add_plane(cursor.handle, claim);
                }
            } else {
                tracing::warn!(?conn, "Lease requested for desktop connector, denying request");
                return Err(LeaseRejected::default());
            }
        }

        Ok(builder)
    }

    fn new_active_lease(&mut self, node: DrmNode, lease: DrmLease) {
        let backend = self.backend_data.backends.get_mut(&node).unwrap();
        backend.active_leases.push(lease);
    }

    fn lease_destroyed(&mut self, node: DrmNode, lease: u32) {
        let backend = self.backend_data.backends.get_mut(&node).unwrap();
        backend.active_leases.retain(|l| l.id() != lease);
    }
}

impl DrmSyncobjHandler for AnvilState<UdevData> {
    fn drm_syncobj_state(&mut self) -> Option<&mut DrmSyncobjState> {
        self.backend_data.syncobj_state.as_mut()
    }
}

pub type RenderSurface = GbmBufferedSurface<GbmAllocator<DrmDeviceFd>, Option<OutputPresentationFeedback>>;

pub type GbmDrmCompositor = DrmCompositor<
    GbmAllocator<DrmDeviceFd>,
    GbmDevice<DrmDeviceFd>,
    Option<OutputPresentationFeedback>,
    DrmDeviceFd,
>;

struct SurfaceData {
    dh: DisplayHandle,
    device_id: DrmNode,
    render_node: Option<DrmNode>,
    output: Output,
    /// The connector's modes, for `Backend::set_mode`.
    drm_modes: Vec<smithay::reexports::drm::control::Mode>,
    global: Option<GlobalId>,
    drm_output: DrmOutput<
        GbmAllocator<DrmDeviceFd>,
        GbmFramebufferExporter<DrmDeviceFd>,
        Option<OutputPresentationFeedback>,
        DrmDeviceFd,
    >,
    disable_direct_scanout: bool,
    #[cfg(feature = "debug")]
    fps: fps_ticker::Fps,
    #[cfg(feature = "debug")]
    fps_element: Option<FpsElement<MultiTexture>>,
    dmabuf_feedback: Option<SurfaceDmabufFeedback>,
    last_presentation_time: Option<Time<Monotonic>>,
    vblank_throttle_timer: Option<RegistrationToken>,
}

impl Drop for SurfaceData {
    fn drop(&mut self) {
        self.output.leave_all();
        if let Some(global) = self.global.take() {
            self.dh.remove_global::<AnvilState<UdevData>>(global);
        }
    }
}

struct BackendData {
    surfaces: HashMap<crtc::Handle, SurfaceData>,
    non_desktop_connectors: Vec<(connector::Handle, crtc::Handle)>,
    leasing_global: Option<DrmLeaseState>,
    active_leases: Vec<DrmLease>,
    drm_output_manager: DrmOutputManager<
        GbmAllocator<DrmDeviceFd>,
        GbmFramebufferExporter<DrmDeviceFd>,
        Option<OutputPresentationFeedback>,
        DrmDeviceFd,
    >,
    drm_scanner: DrmScanner,
    render_node: Option<DrmNode>,
    registration_token: RegistrationToken,
    /// Night light by colour matrix, per screen.
    ctm: std::cell::RefCell<HashMap<crtc::Handle, Ctm>>,
}

#[derive(Debug, thiserror::Error)]
enum DeviceAddError {
    #[error("Failed to open device using libseat: {0}")]
    DeviceOpen(libseat::Error),
    #[error("Failed to initialize drm device: {0}")]
    DrmDevice(DrmError),
    #[error("Failed to initialize gbm device: {0}")]
    GbmDevice(std::io::Error),
    #[error("Failed to access drm node: {0}")]
    DrmNode(CreateDrmNodeError),
    #[error("Failed to add device to GpuManager: {0}")]
    AddNode(egl::Error),
    #[error("The device has no render node")]
    NoRenderNode,
    #[error("Primary GPU is missing")]
    PrimaryGpuMissing,
}

fn get_surface_dmabuf_feedback(
    primary_gpu: DrmNode,
    render_node: Option<DrmNode>,
    scanout_node: DrmNode,
    gpus: &mut GpuManager<GbmGlesBackend<GlesRenderer, DrmDeviceFd>>,
    surface: &DrmSurface,
) -> Option<SurfaceDmabufFeedback> {
    let primary_formats = gpus.single_renderer(&primary_gpu).ok()?.dmabuf_formats();
    let render_formats = if let Some(render_node) = render_node {
        gpus.single_renderer(&render_node).ok()?.dmabuf_formats()
    } else {
        FormatSet::default()
    };

    let all_render_formats = primary_formats
        .iter()
        .chain(render_formats.iter())
        .copied()
        .collect::<FormatSet>();

    let planes = surface.planes().clone();

    // We limit the scan-out tranche to formats we can also render from
    // so that there is always a fallback render path available in case
    // the supplied buffer can not be scanned out directly
    let planes_formats = surface
        .plane_info()
        .formats
        .iter()
        .copied()
        .chain(planes.overlay.into_iter().flat_map(|p| p.formats))
        .collect::<FormatSet>()
        .intersection(&all_render_formats)
        .copied()
        .collect::<FormatSet>();

    let builder = DmabufFeedbackBuilder::new(primary_gpu.dev_id(), primary_formats);
    let render_feedback = if let Some(render_node) = render_node {
        builder
            .clone()
            .add_preference_tranche(
                render_node.dev_id(),
                zwp_linux_dmabuf_feedback_v1::TrancheFlags::Sampling,
                render_formats.clone(),
                3u32..=6,
            )
            .build()
            .unwrap()
    } else {
        builder.clone().build().unwrap()
    };

    let scanout_feedback = builder
        .add_preference_tranche(
            surface.device_fd().dev_id().unwrap(),
            zwp_linux_dmabuf_feedback_v1::TrancheFlags::Scanout,
            planes_formats,
            4u32..=6,
        )
        .add_preference_tranche(
            scanout_node.dev_id(),
            zwp_linux_dmabuf_feedback_v1::TrancheFlags::Sampling,
            render_formats,
            4u32..=6,
        )
        .build()
        .unwrap();

    Some(SurfaceDmabufFeedback {
        render_feedback,
        scanout_feedback,
    })
}

impl AnvilState<UdevData> {
    fn device_added(&mut self, node: DrmNode, path: &Path) -> Result<(), DeviceAddError> {
        // Try to open the device
        let fd = self
            .backend_data
            .session
            .open(
                path,
                OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY | OFlags::NONBLOCK,
            )
            .map_err(DeviceAddError::DeviceOpen)?;

        let fd = DrmDeviceFd::new(DeviceFd::from(fd));

        let (drm, notifier) = DrmDevice::new(fd.clone(), true).map_err(DeviceAddError::DrmDevice)?;
        let gbm = GbmDevice::new(fd).map_err(DeviceAddError::GbmDevice)?;

        let registration_token = self
            .handle
            .insert_source(
                notifier,
                move |event, metadata, data: &mut AnvilState<_>| match event {
                    DrmEvent::VBlank(crtc) => {
                        profiling::scope!("vblank", &format!("{crtc:?}"));
                        if let Some(backend) = data.backend_data.backends.get(&node) {
                            retry_ctm(backend, crtc);
                        }
                        data.frame_finish(node, crtc, metadata);
                    }
                    DrmEvent::Error(error) => {
                        error!("{:?}", error);
                    }
                },
            )
            .unwrap();

        let mut try_initialize_gpu = || {
            let display = unsafe { EGLDisplay::new(gbm.clone()).map_err(DeviceAddError::AddNode)? };
            let egl_device = EGLDevice::device_for_display(&display).map_err(DeviceAddError::AddNode)?;

            if egl_device.is_software() {
                return Err(DeviceAddError::NoRenderNode);
            }

            let render_node = egl_device.try_get_render_node().ok().flatten().unwrap_or(node);
            self.backend_data
                .gpus
                .as_mut()
                .add_node(render_node, gbm.clone())
                .map_err(DeviceAddError::AddNode)?;

            std::result::Result::<DrmNode, DeviceAddError>::Ok(render_node)
        };

        let render_node = try_initialize_gpu()
            .inspect_err(|err| {
                warn!(?err, "failed to initialize gpu");
            })
            .ok();

        let allocator = render_node
            .is_some()
            .then(|| GbmAllocator::new(gbm.clone(), GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT))
            .or_else(|| {
                self.backend_data
                    .backends
                    .get(&self.backend_data.primary_gpu)
                    .or_else(|| {
                        self.backend_data
                            .backends
                            .values()
                            .find(|backend| backend.render_node == Some(self.backend_data.primary_gpu))
                    })
                    .map(|backend| backend.drm_output_manager.allocator().clone())
            })
            .ok_or(DeviceAddError::PrimaryGpuMissing)?;

        let framebuffer_exporter = GbmFramebufferExporter::new(gbm.clone(), render_node.into());

        let color_formats = if std::env::var("ANVIL_DISABLE_10BIT").is_ok() {
            SUPPORTED_FORMATS_8BIT_ONLY
        } else {
            SUPPORTED_FORMATS
        };
        let mut renderer = self
            .backend_data
            .gpus
            .single_renderer(&render_node.unwrap_or(self.backend_data.primary_gpu))
            .unwrap();
        let render_formats = renderer
            .as_mut()
            .egl_context()
            .dmabuf_render_formats()
            .iter()
            .filter(|format| render_node.is_some() || format.modifier == Modifier::Linear)
            .copied()
            .collect::<FormatSet>();

        let drm_output_manager = DrmOutputManager::new(
            drm,
            allocator,
            framebuffer_exporter,
            Some(gbm),
            color_formats.iter().copied(),
            render_formats,
        );

        self.backend_data.backends.insert(
            node,
            BackendData {
                registration_token,
                drm_output_manager,
                drm_scanner: DrmScanner::new(),
                non_desktop_connectors: Vec::new(),
                render_node,
                surfaces: HashMap::new(),
                leasing_global: DrmLeaseState::new::<AnvilState<UdevData>>(&self.display_handle, &node)
                    .inspect_err(|err| {
                        warn!(?err, "Failed to initialize drm lease global for: {}", node);
                    })
                    .ok(),
                active_leases: Vec::new(),
                ctm: Default::default(),
            },
        );

        self.device_changed(node);

        Ok(())
    }

    fn connector_connected(&mut self, node: DrmNode, connector: connector::Info, crtc: crtc::Handle) {
        let device = if let Some(device) = self.backend_data.backends.get_mut(&node) {
            device
        } else {
            return;
        };

        let render_node = device.render_node.unwrap_or(self.backend_data.primary_gpu);
        let mut renderer = self.backend_data.gpus.single_renderer(&render_node).unwrap();

        let output_name = format!("{}-{}", connector.interface().as_str(), connector.interface_id());
        info!(?crtc, "Trying to setup connector {}", output_name,);

        let drm_device = device.drm_output_manager.device();

        let non_desktop = drm_device
            .get_properties(connector.handle())
            .ok()
            .and_then(|props| {
                let (info, value) = props
                    .into_iter()
                    .filter_map(|(handle, value)| {
                        let info = drm_device.get_property(handle).ok()?;

                        Some((info, value))
                    })
                    .find(|(info, _)| info.name().to_str() == Ok("non-desktop"))?;

                info.value_type().convert_value(value).as_boolean()
            })
            .unwrap_or(false);

        let display_info = display_info::for_connector(drm_device, connector.handle());
        // The kernel's copy of the EDID when the DRM route gives nothing
        // (Apple Silicon's DCP).
        let edid = tabula_system::edid::for_connector(&output_name);

        let make = display_info
            .as_ref()
            .and_then(|info| info.make())
            .or_else(|| edid.as_ref().map(|e| e.maker.clone()))
            .unwrap_or_else(|| "Unknown".into());

        let model = display_info
            .as_ref()
            .and_then(|info| info.model())
            .or_else(|| edid.as_ref().and_then(|e| e.model.clone()))
            .unwrap_or_else(|| "Unknown".into());

        let serial_number = display_info
            .as_ref()
            .and_then(|info| info.serial())
            .or_else(|| edid.as_ref().and_then(|e| e.serial.clone()))
            .unwrap_or_else(|| "Unknown".into());

        if non_desktop {
            info!("Connector {} is non-desktop, setting up for leasing", output_name);
            device.non_desktop_connectors.push((connector.handle(), crtc));
            if let Some(lease_state) = device.leasing_global.as_mut() {
                lease_state.add_connector::<AnvilState<UdevData>>(
                    connector.handle(),
                    output_name,
                    format!("{make} {model}"),
                );
            }
        } else {
            let preferred = connector
                .modes()
                .iter()
                .position(|mode| mode.mode_type().contains(ModeTypeFlags::PREFERRED))
                .unwrap_or(0);
            // The mode chosen in Preferences ▸ Display, while the display offers it.
            let key = tabula_protocol::display_key(&output_name, &make, &model, &serial_number);
            let saved = self.settings.display.config(&key, &output_name).and_then(|c| c.mode.clone());
            let mode_id = saved
                .and_then(|m| {
                    connector.modes().iter().position(|d| {
                        let w = WlMode::from(*d);
                        (w.size.w, w.size.h, w.refresh) == (m.width, m.height, m.refresh)
                    })
                })
                .unwrap_or(preferred);

            let drm_mode = connector.modes()[mode_id];
            let wl_mode = WlMode::from(drm_mode);

            let (phys_w, phys_h) = connector.size().unwrap_or((0, 0));
            let output = Output::new(
                output_name,
                PhysicalProperties {
                    size: (phys_w as i32, phys_h as i32).into(),
                    subpixel: connector.subpixel().into(),
                    make,
                    model,
                    serial_number,
                },
            );
            let global = output.create_global::<AnvilState<UdevData>>(&self.display_handle);

            // Right of the others until the saved arrangement is applied.
            let x = self.space.outputs().filter_map(|o| self.space.output_geometry(o)).map(|g| g.loc.x + g.size.w).max().unwrap_or(0);
            let position = (x, 0).into();

            for m in connector.modes() {
                output.add_mode(WlMode::from(*m));
            }
            output.set_preferred(WlMode::from(connector.modes()[preferred]));
            output.change_current_state(Some(wl_mode), None, None, Some(position));
            let key = crate::desktop::display_key(&output);
            let scale = self.settings.display.scale_for(&key, &output.name());
            let transform = crate::desktop::rotation_transform(self.settings.display.rotation_for(&key, &output.name()));
            output.change_current_state(None, Some(transform), Some(smithay::output::Scale::Fractional(scale)), None);
            self.space.map_output(&output, position);

            output.user_data().insert_if_missing(|| UdevOutputId {
                crtc,
                device_id: node,
            });

            #[cfg(feature = "debug")]
            let fps_element = self.backend_data.fps_texture.clone().map(FpsElement::new);

            let driver = match drm_device.get_driver() {
                Ok(driver) => driver,
                Err(err) => {
                    warn!("Failed to query drm driver: {}", err);
                    return;
                }
            };

            let mut planes = match drm_device.planes(&crtc) {
                Ok(planes) => planes,
                Err(err) => {
                    warn!("Failed to query crtc planes: {}", err);
                    return;
                }
            };

            // Using an overlay plane on a nvidia card breaks
            if driver.name().to_string_lossy().to_lowercase().contains("nvidia")
                || driver
                    .description()
                    .to_string_lossy()
                    .to_lowercase()
                    .contains("nvidia")
            {
                planes.overlay = vec![];
            }

            let drm_output = match device
                .drm_output_manager
                .lock()
                .initialize_output::<_, OutputRenderElements<UdevRenderer<'_>, WindowRenderElement<UdevRenderer<'_>>>>(
                    crtc,
                    drm_mode,
                    &[connector.handle()],
                    &output,
                    Some(planes),
                    &mut renderer,
                    &DrmOutputRenderElements::default(),
                ) {
                Ok(drm_output) => drm_output,
                Err(err) => {
                    warn!("Failed to initialize drm output: {}", err);
                    return;
                }
            };

            let disable_direct_scanout = std::env::var("ANVIL_DISABLE_DIRECT_SCANOUT").is_ok();

            let dmabuf_feedback = drm_output.with_compositor(|compositor| {
                compositor.set_debug_flags(self.backend_data.debug_flags);

                get_surface_dmabuf_feedback(
                    self.backend_data.primary_gpu,
                    device.render_node,
                    node,
                    &mut self.backend_data.gpus,
                    compositor.surface(),
                )
            });

            let surface = SurfaceData {
                dh: self.display_handle.clone(),
                device_id: node,
                render_node: device.render_node,
                output,
                drm_modes: connector.modes().to_vec(),
                global: Some(global),
                drm_output,
                disable_direct_scanout,
                #[cfg(feature = "debug")]
                fps: fps_ticker::Fps::default(),
                #[cfg(feature = "debug")]
                fps_element,
                dmabuf_feedback,
                last_presentation_time: None,
                vblank_throttle_timer: None,
            };

            device.surfaces.insert(crtc, surface);

            // kick-off rendering, then warm it if night light is on
            self.handle.insert_idle(move |state| {
                state.render_surface(node, crtc, state.clock.now());
                if let Some(backend) = state.backend_data.backends.get(&node) {
                    set_gamma(backend, crtc, state.backend_data.white_point);
                }
            });
        }
    }

    fn connector_disconnected(&mut self, node: DrmNode, connector: connector::Info, crtc: crtc::Handle) {
        let mut gone = None;
        let device = if let Some(device) = self.backend_data.backends.get_mut(&node) {
            device
        } else {
            return;
        };

        if let Some(pos) = device
            .non_desktop_connectors
            .iter()
            .position(|(handle, _)| *handle == connector.handle())
        {
            let _ = device.non_desktop_connectors.remove(pos);
            if let Some(leasing_state) = device.leasing_global.as_mut() {
                leasing_state.withdraw_connector(connector.handle());
            }
        } else if let Some(surface) = device.surfaces.remove(&crtc) {
            gone = Some(surface.output.clone());
            self.space.unmap_output(&surface.output);
            self.mirrors.retain(|m| *m != surface.output);
            crate::render::set_mirror_source(&surface.output, None);
            self.space.refresh();
        }

        let render_node = device.render_node.unwrap_or(self.backend_data.primary_gpu);
        let mut renderer = self.backend_data.gpus.single_renderer(&render_node).unwrap();
        let _ = device.drm_output_manager.lock().try_to_restore_modifiers::<_, OutputRenderElements<
            UdevRenderer<'_>,
            WindowRenderElement<UdevRenderer<'_>>,
        >>(
            &mut renderer,
            // FIXME: For a flicker free operation we should return the actual elements for this output..
            // Instead we just use black to "simulate" a modeset :)
            &DrmOutputRenderElements::default(),
        );
        // Deskbar and the desktop move to what is now the main display.
        if let Some(gone) = gone {
            self.move_auto_layers(&[gone]);
        }
    }

    fn device_changed(&mut self, node: DrmNode) {
        let device = if let Some(device) = self.backend_data.backends.get_mut(&node) {
            device
        } else {
            return;
        };

        let scan_result = match device
            .drm_scanner
            .scan_connectors(device.drm_output_manager.device())
        {
            Ok(scan_result) => scan_result,
            Err(err) => {
                tracing::warn!(?err, "Failed to scan connectors");
                return;
            }
        };

        for event in scan_result {
            match event {
                DrmScanEvent::Connected {
                    connector,
                    crtc: Some(crtc),
                } => {
                    self.connector_connected(node, connector, crtc);
                }
                DrmScanEvent::Disconnected {
                    connector,
                    crtc: Some(crtc),
                } => {
                    self.connector_disconnected(node, connector, crtc);
                }
                // The connector's mode list changed while it stayed connected (e.g. EDID
                // arrived after the initial probe returned empty/fallback modes). Compositors
                // should re-evaluate the output's mode selection and recreate the surface here.
                DrmScanEvent::Changed { .. } => {}
                _ => {}
            }
        }

        // fixup window coordinates
        crate::shell::fixup_positions(&mut self.space, &self.settings.display, self.pointer.current_location());
        // Screens set to mirror, or whose source just left.
        self.apply_mirroring();
        self.follow_main_output();
    }

    fn device_removed(&mut self, node: DrmNode) {
        let device = if let Some(device) = self.backend_data.backends.get_mut(&node) {
            device
        } else {
            return;
        };

        let crtcs: Vec<_> = device
            .drm_scanner
            .crtcs()
            .map(|(info, crtc)| (info.clone(), crtc))
            .collect();

        for (connector, crtc) in crtcs {
            self.connector_disconnected(node, connector, crtc);
        }

        debug!("Surfaces dropped");

        // drop the backends on this side
        if let Some(mut backend_data) = self.backend_data.backends.remove(&node) {
            if let Some(mut leasing_global) = backend_data.leasing_global.take() {
                leasing_global.disable_global::<AnvilState<UdevData>>();
            }

            if let Some(render_node) = backend_data.render_node {
                self.backend_data.gpus.as_mut().remove_node(&render_node);
            }

            self.handle.remove(backend_data.registration_token);

            debug!("Dropping device");
        }

        crate::shell::fixup_positions(&mut self.space, &self.settings.display, self.pointer.current_location());
        self.follow_main_output();
    }

    fn frame_finish(&mut self, dev_id: DrmNode, crtc: crtc::Handle, metadata: &mut Option<DrmEventMetadata>) {
        profiling::scope!("frame_finish", &format!("{crtc:?}"));

        let device_backend = match self.backend_data.backends.get_mut(&dev_id) {
            Some(backend) => backend,
            None => {
                error!("Trying to finish frame on non-existent backend {}", dev_id);
                return;
            }
        };

        let surface = match device_backend.surfaces.get_mut(&crtc) {
            Some(surface) => surface,
            None => {
                error!("Trying to finish frame on non-existent crtc {:?}", crtc);
                return;
            }
        };

        if let Some(timer_token) = surface.vblank_throttle_timer.take() {
            self.handle.remove(timer_token);
        }

        let output = if let Some(output) = self.space.outputs().find(|o| {
            o.user_data().get::<UdevOutputId>()
                == Some(&UdevOutputId {
                    device_id: surface.device_id,
                    crtc,
                })
        }) {
            output.clone()
        } else {
            // somehow we got called with an invalid output
            return;
        };

        let Some(frame_duration) = output
            .current_mode()
            .map(|mode| Duration::from_secs_f64(1_000f64 / mode.refresh as f64))
        else {
            return;
        };

        let tp = metadata.as_ref().and_then(|metadata| match metadata.time {
            smithay::backend::drm::DrmEventTime::Monotonic(tp) => tp.is_zero().not().then_some(tp),
            smithay::backend::drm::DrmEventTime::Realtime(_) => None,
        });

        let seq = metadata.as_ref().map(|metadata| metadata.sequence).unwrap_or(0);

        let (clock, flags) = if let Some(tp) = tp {
            (
                tp.into(),
                wp_presentation_feedback::Kind::Vsync
                    | wp_presentation_feedback::Kind::HwClock
                    | wp_presentation_feedback::Kind::HwCompletion,
            )
        } else {
            (self.clock.now(), wp_presentation_feedback::Kind::Vsync)
        };

        let vblank_remaining_time = surface.last_presentation_time.map(|last_presentation_time| {
            frame_duration.saturating_sub(Time::elapsed(&last_presentation_time, clock))
        });

        if let Some(vblank_remaining_time) = vblank_remaining_time
            && vblank_remaining_time > frame_duration / 2 {
                static WARN_ONCE: Once = Once::new();
                WARN_ONCE.call_once(|| {
                    warn!("display running faster than expected, throttling vblanks and disabling HwClock")
                });
                let throttled_time = tp
                    .map(|tp| tp.saturating_add(vblank_remaining_time))
                    .unwrap_or(Duration::ZERO);
                let throttled_metadata = DrmEventMetadata {
                    sequence: seq,
                    time: DrmEventTime::Monotonic(throttled_time),
                };
                let timer_token = self
                    .handle
                    .insert_source(Timer::from_duration(vblank_remaining_time), move |_, _, data| {
                        data.frame_finish(dev_id, crtc, &mut Some(throttled_metadata));
                        TimeoutAction::Drop
                    })
                    .expect("failed to register vblank throttle timer");
                surface.vblank_throttle_timer = Some(timer_token);
                return;
            }
        surface.last_presentation_time = Some(clock);

        let submit_result = surface
            .drm_output
            .frame_submitted()
            .map_err(Into::<SwapBuffersError>::into);

        let schedule_render = match submit_result {
            Ok(user_data) => {
                if let Some(mut feedback) = user_data.flatten() {
                    feedback.presented(clock, Refresh::fixed(frame_duration), seq as u64, flags);
                }

                true
            }
            Err(err) => {
                warn!("Error during rendering: {:?}", err);
                match err {
                    SwapBuffersError::AlreadySwapped => true,
                    // If the device has been deactivated do not reschedule, this will be done
                    // by session resume
                    SwapBuffersError::TemporaryFailure(err)
                        if matches!(err.downcast_ref::<DrmError>(), Some(&DrmError::DeviceInactive)) =>
                    {
                        false
                    }
                    SwapBuffersError::TemporaryFailure(err) => matches!(
                        err.downcast_ref::<DrmError>(),
                        Some(DrmError::Access(DrmAccessError {
                            source,
                            ..
                        })) if source.kind() == io::ErrorKind::PermissionDenied
                    ),
                    SwapBuffersError::ContextLost(err) => panic!("Rendering loop lost: {err}"),
                }
            }
        };

        if schedule_render {
            let next_frame_target = clock + frame_duration;

            // What are we trying to solve by introducing a delay here:
            //
            // Basically it is all about latency of client provided buffers.
            // A client driven by frame callbacks will wait for a frame callback
            // to repaint and submit a new buffer. As we send frame callbacks
            // as part of the repaint in the compositor the latency would always
            // be approx. 2 frames. By introducing a delay before we repaint in
            // the compositor we can reduce the latency to approx. 1 frame + the
            // remaining duration from the repaint to the next VBlank.
            //
            // With the delay it is also possible to further reduce latency if
            // the client is driven by presentation feedback. As the presentation
            // feedback is directly sent after a VBlank the client can submit a
            // new buffer during the repaint delay that can hit the very next
            // VBlank, thus reducing the potential latency to below one frame.
            //
            // Choosing a good delay is a topic on its own so we just implement
            // a simple strategy here. We just split the duration between two
            // VBlanks into two steps, one for the client repaint and one for the
            // compositor repaint. Theoretically the repaint in the compositor should
            // be faster so we give the client a bit more time to repaint. On a typical
            // modern system the repaint in the compositor should not take more than 2ms
            // so this should be safe for refresh rates up to at least 120 Hz. For 120 Hz
            // this results in approx. 3.33ms time for repainting in the compositor.
            // A too big delay could result in missing the next VBlank in the compositor.
            //
            // A more complete solution could work on a sliding window analyzing past repaints
            // and do some prediction for the next repaint.
            let repaint_delay = Duration::from_secs_f64(frame_duration.as_secs_f64() * 0.6f64);

            let timer = if surface
                .render_node
                .map(|render_node| render_node != self.backend_data.primary_gpu)
                .unwrap_or(true)
            {
                // However, if we need to do a copy, that might not be enough.
                // (And without actual comparison to previous frames we cannot really know.)
                // So lets ignore that in those cases to avoid thrashing performance.
                trace!("scheduling repaint timer immediately on {:?}", crtc);
                Timer::immediate()
            } else {
                trace!(
                    "scheduling repaint timer with delay {:?} on {:?}",
                    repaint_delay, crtc
                );
                Timer::from_duration(repaint_delay)
            };

            self.handle
                .insert_source(timer, move |_, _, data| {
                    data.render(dev_id, Some(crtc), next_frame_target);
                    TimeoutAction::Drop
                })
                .expect("failed to schedule frame timer");
        }
    }

    // If crtc is `Some()`, render it, else render all crtcs
    fn render(&mut self, node: DrmNode, crtc: Option<crtc::Handle>, frame_target: Time<Monotonic>) {
        let device_backend = match self.backend_data.backends.get_mut(&node) {
            Some(backend) => backend,
            None => {
                error!("Trying to render on non-existent backend {}", node);
                return;
            }
        };

        if let Some(crtc) = crtc {
            self.render_surface(node, crtc, frame_target);
        } else {
            let crtcs: Vec<_> = device_backend.surfaces.keys().copied().collect();
            for crtc in crtcs {
                self.render_surface(node, crtc, frame_target);
            }
        };
    }

    fn render_surface(&mut self, node: DrmNode, crtc: crtc::Handle, frame_target: Time<Monotonic>) {
        profiling::scope!("render_surface", &format!("{crtc:?}"));
        if self.backend_data.displays_off {
            return;
        }

        let screen = if let Some(output) = self.space.outputs().chain(self.mirrors.iter()).find(|o| {
            o.user_data().get::<UdevOutputId>()
                == Some(&UdevOutputId {
                    device_id: node,
                    crtc,
                })
        }) {
            output.clone()
        } else {
            // somehow we got called with an invalid output
            return;
        };
        // A mirror draws its source's frame, fitted to its own mode.
        let source = crate::render::mirror_source(&screen);
        let fit = source.as_ref().map(|s| crate::render::fit_outputs(&screen, s, &self.space));
        let output = source.unwrap_or(screen);

        self.pre_repaint(&output, frame_target);
        let desktop_color = self.desktop_color();
        let twitcher = self.twitcher_entries();
        let output_size = self.space.output_geometry(&output).map(|g| g.size).unwrap_or_default();

        let device = if let Some(device) = self.backend_data.backends.get_mut(&node) {
            device
        } else {
            return;
        };

        let surface = if let Some(surface) = device.surfaces.get_mut(&crtc) {
            surface
        } else {
            return;
        };

        let start = Instant::now();

        let primary_gpu = self.backend_data.primary_gpu;
        let render_node = surface.render_node.unwrap_or(primary_gpu);
        let mut renderer = if primary_gpu == render_node {
            self.backend_data.gpus.single_renderer(&render_node)
        } else {
            let format = surface.drm_output.format();
            self.backend_data
                .gpus
                .renderer(&primary_gpu, &render_node, format)
        }
        .unwrap();

        let locked = self.lock.locked();
        for shot in self.ipc.shots_for(&output) {
            let overlays = if locked { crate::lock::elements(&self.lock, &mut renderer, &output) } else { Vec::new() };
            crate::screenshot::take::<_, smithay::backend::renderer::gles::GlesTexture>(
                &mut self.ipc,
                shot,
                &mut renderer,
                &output,
                &self.space,
                overlays,
                desktop_color,
                locked,
                (self.pointer.current_location(), crate::cursor::pictured(&self.cursor_status)),
            );
        }
        let overlays: Vec<CustomRenderElements<_>> = if locked {
            crate::lock::elements(&self.lock, &mut renderer, &output)
        } else {
            twitcher
                .and_then(|(entries, selected)| {
                    let scale = output.current_scale().fractional_scale();
                    self.twitcher.element(&mut renderer, &entries, selected, output_size, scale)
                })
                .map(CustomRenderElements::Overlay)
                .into_iter()
                .collect()
        };

        // The built-in cursor this pointer shows, at this screen's scale.
        let pointer_scale = output.current_scale().fractional_scale();
        let shape = crate::cursor::named(&self.cursor_status).unwrap_or(crate::cursor::Shape::SystemDefault);
        let pointer_image = (crate::cursor::buffer(shape, pointer_scale), crate::cursor::image(shape, pointer_scale).size as i32);

        let result = render_surface(
            surface,
            &mut renderer,
            &self.space,
            &output,
            self.pointer.current_location(),
            &pointer_image,
            &mut self.backend_data.pointer_element,
            &self.dnd_icon,
            &mut self.cursor_status,
            self.show_window_preview,
            desktop_color,
            overlays,
            locked,
            fit,
        );
        let reschedule = match result {
            Ok((has_rendered, states)) => {
                let dmabuf_feedback = surface.dmabuf_feedback.clone();
                self.post_repaint(&output, frame_target, dmabuf_feedback, &states);
                !has_rendered
            }
            Err(err) => {
                warn!("Error during rendering: {:#?}", err);
                match err {
                    SwapBuffersError::AlreadySwapped => false,
                    // Nothing was queued, so no VBlank will come to drive the
                    // next frame: try again in a frame, from scratch. A flip
                    // refused with EBUSY (the previous master's last flip, or
                    // our own modeset, still in flight) would otherwise leave
                    // the screen black for good.
                    SwapBuffersError::TemporaryFailure(_) => {
                        surface.drm_output.reset_buffers();
                        true
                    }
                    SwapBuffersError::ContextLost(err) => match err.downcast_ref::<DrmError>() {
                        Some(DrmError::TestFailed(_)) => {
                            // reset the complete state, disabling all connectors and planes in case we hit a test failed
                            // most likely we hit this after a tty switch when a foreign master changed CRTC <-> connector bindings
                            // and we run in a mismatch
                            device
                                .drm_output_manager
                                .device_mut()
                                .reset_state()
                                .expect("failed to reset drm device");
                            true
                        }
                        _ => panic!("Rendering loop lost: {err}"),
                    },
                }
            }
        };

        if reschedule {
            let output_refresh = match output.current_mode() {
                Some(mode) => mode.refresh,
                None => return,
            };

            // If reschedule is true we either hit a temporary failure or more likely rendering
            // did not cause any damage on the output. In this case we just re-schedule a repaint
            // after approx. one frame to re-test for damage.
            let next_frame_target = frame_target + Duration::from_millis(1_000_000 / output_refresh as u64);
            let reschedule_timeout =
                Duration::from(next_frame_target).saturating_sub(self.clock.now().into());
            trace!(
                "reschedule repaint timer with delay {:?} on {:?}",
                reschedule_timeout, crtc,
            );
            let timer = Timer::from_duration(reschedule_timeout);
            self.handle
                .insert_source(timer, move |_, _, data| {
                    data.render(node, Some(crtc), next_frame_target);
                    TimeoutAction::Drop
                })
                .expect("failed to schedule frame timer");
        } else {
            let elapsed = start.elapsed();
            tracing::trace!(?elapsed, "rendered surface");
        }

        profiling::finish_frame!();
    }
}

#[allow(clippy::too_many_arguments)]
#[profiling::function]
fn render_surface<'a>(
    surface: &'a mut SurfaceData,
    renderer: &mut UdevRenderer<'a>,
    space: &Space<WindowElement>,
    output: &Output,
    pointer_location: Point<f64, Logical>,
    pointer_image: &(MemoryRenderBuffer, i32),
    pointer_element: &mut PointerElement,
    dnd_icon: &Option<DndIcon>,
    cursor_status: &mut CursorImageStatus,
    show_window_preview: bool,
    desktop_color: smithay::backend::renderer::Color32F,
    overlays: Vec<CustomRenderElements<UdevRenderer<'a>>>,
    locked: bool,
    fit: Option<crate::render::Fit>,
) -> Result<(bool, RenderElementStates), SwapBuffersError> {
    let output_geometry = space.output_geometry(output).unwrap();
    let scale = Scale::from(output.current_scale().fractional_scale());

    let mut custom_elements: Vec<CustomRenderElements<_>> = Vec::new();

    if output_geometry.to_f64().contains(pointer_location) {
        let cursor_hotspot = crate::cursor::hotspot(cursor_status);
        let cursor_pos = pointer_location - output_geometry.loc.to_f64();

        // set cursor
        pointer_element.set_buffer(pointer_image.0.clone(), pointer_image.1);

        // draw the cursor as relevant
        {
            // reset the cursor if the surface is no longer alive
            let mut reset = false;
            if let CursorImageStatus::Surface(ref surface) = *cursor_status {
                reset = !surface.alive();
            }
            if reset {
                *cursor_status = CursorImageStatus::default_named();
            }

            pointer_element.set_status(cursor_status.clone());
        }

        custom_elements.extend(
            pointer_element.render_elements(
                renderer,
                (cursor_pos - cursor_hotspot.to_f64())
                    .to_physical(scale)
                    .to_i32_round(),
                scale,
                1.0,
            ),
        );

        // draw the dnd icon if applicable
        {
            if let Some(icon) = dnd_icon.as_ref() {
                let dnd_icon_pos = (cursor_pos + icon.offset.to_f64())
                    .to_physical(scale)
                    .to_i32_round();
                if icon.surface.alive() {
                    custom_elements.extend(AsRenderElements::<UdevRenderer<'a>>::render_elements(
                        &SurfaceTree::from_surface(&icon.surface),
                        renderer,
                        dnd_icon_pos,
                        scale,
                        1.0,
                    ));
                }
            }
        }
    }

    // Twitcher (or the lock screen) sits under the pointer, over everything else.
    custom_elements.extend(overlays);

    #[cfg(feature = "debug")]
    if let Some(element) = surface.fps_element.as_mut() {
        element.update_fps(surface.fps.avg().round() as u32);
        surface.fps.tick();
        custom_elements.push(CustomRenderElements::Fps(element.clone()));
    }

    let (elements, clear_color) =
        output_elements(output, space, custom_elements, renderer, show_window_preview, desktop_color, locked);
    let elements = crate::render::frame(elements, fit);

    let frame_mode = if surface.disable_direct_scanout {
        FrameFlags::empty()
    } else {
        FrameFlags::DEFAULT
    };
    let (rendered, states) = surface
        .drm_output
        .render_frame(renderer, &elements, clear_color, frame_mode)
        .map(|render_frame_result| {
            #[cfg(feature = "renderer_sync")]
            if let PrimaryPlaneElement::Swapchain(element) = render_frame_result.primary_element {
                element.sync.wait();
            }
            (!render_frame_result.is_empty, render_frame_result.states)
        })
        .map_err(|err| match err {
            smithay::backend::drm::compositor::RenderFrameError::PrepareFrame(err) => {
                SwapBuffersError::from(err)
            }
            smithay::backend::drm::compositor::RenderFrameError::RenderFrame(
                OutputDamageTrackerError::Rendering(err),
            ) => SwapBuffersError::from(err),
            _ => unreachable!(),
        })?;

    update_primary_scanout_output(space, output, dnd_icon, cursor_status, &states);

    if rendered {
        let output_presentation_feedback = take_presentation_feedback(output, space, &states);
        surface
            .drm_output
            .queue_frame(Some(output_presentation_feedback))
            .map_err(Into::<SwapBuffersError>::into)?;
    }

    Ok((rendered, states))
}
