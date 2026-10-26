//! A headless backend: renders offscreen on a GPU render node with no
//! window or display, driven by a 60 Hz timer. Input comes only from the
//! IPC `Synthetic` request, and screenshots work as on the other backends.
//! Used for automated UI testing without touching the user's screen:
//!
//! ```text
//! tabula-app-server --headless [WIDTHxHEIGHT]    (physical pixels)
//! ```

use std::{
    fs::OpenOptions,
    sync::atomic::Ordering,
    time::Duration,
};

use smithay::{
    backend::{
        allocator::{Fourcc, dmabuf::Dmabuf, gbm::GbmDevice},
        egl::{EGLContext, EGLDisplay},
        renderer::{
            Bind, ImportDma, ImportMemWl, Offscreen,
            damage::OutputDamageTracker,
            gles::{GlesRenderer, GlesTexture},
        },
    },
    input::keyboard::LedState,
    output::{Mode, Output, PhysicalProperties, Subpixel},
    reexports::{
        calloop::{
            EventLoop,
            timer::{TimeoutAction, Timer},
        },
        wayland_protocols::wp::presentation_time::server::wp_presentation_feedback,
        wayland_server::{Display, protocol::wl_surface},
    },
    utils::{Buffer, Size, Transform},
    wayland::{
        dmabuf::{DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier},
        presentation::Refresh,
    },
};
use tracing::{error, info, warn};

use crate::render::render_output;
use crate::state::{AnvilState, Backend, take_presentation_feedback, update_primary_scanout_output};

pub const OUTPUT_NAME: &str = "HEADLESS-1";

pub struct HeadlessData {
    renderer: GlesRenderer,
    target: GlesTexture,
    damage_tracker: OutputDamageTracker,
    dmabuf_state: (DmabufState, DmabufGlobal),
}

impl DmabufHandler for AnvilState<HeadlessData> {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.backend_data.dmabuf_state.0
    }

    fn dmabuf_imported(&mut self, _global: &DmabufGlobal, dmabuf: Dmabuf, notifier: ImportNotifier) {
        if self.backend_data.renderer.import_dmabuf(&dmabuf, None).is_ok() {
            let _ = notifier.successful::<AnvilState<HeadlessData>>();
        } else {
            notifier.failed();
        }
    }
}

impl Backend for HeadlessData {
    fn seat_name(&self) -> String {
        "headless".into()
    }
    fn reset_buffers(&mut self, _output: &Output) {}
    fn early_import(&mut self, _surface: &wl_surface::WlSurface) {}
    fn update_led_state(&mut self, _led_state: LedState) {}
}

fn render_node() -> Option<std::path::PathBuf> {
    if let Ok(p) = std::env::var("TABULA_RENDER_NODE") {
        return Some(p.into());
    }
    let mut nodes: Vec<_> = std::fs::read_dir("/dev/dri")
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("renderD")))
        .collect();
    nodes.sort();
    nodes.into_iter().next()
}

pub fn run_headless(size_arg: Option<&str>) {
    let (w, h) = size_arg
        .and_then(|s| s.split_once('x'))
        .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
        .unwrap_or((2560, 1600));

    let mut event_loop = EventLoop::<AnvilState<HeadlessData>>::try_new().unwrap();
    let display = Display::new().unwrap();
    let display_handle = display.handle();

    let Some(node) = render_node() else {
        error!("no GPU render node in /dev/dri");
        return;
    };
    let file = match OpenOptions::new().read(true).write(true).open(&node) {
        Ok(f) => f,
        Err(e) => {
            error!(?node, "cannot open render node: {e}");
            return;
        }
    };
    let gbm = GbmDevice::new(file).expect("gbm device");
    // SAFETY: the gbm device outlives the display (moved into it).
    let egl = unsafe { EGLDisplay::new(gbm) }.expect("EGL display");
    let context = EGLContext::new(&egl).expect("EGL context");
    // SAFETY: the context is current only on this thread.
    let mut renderer = unsafe { GlesRenderer::new(context) }.expect("GLES renderer");
    info!(?node, "headless renderer ready");

    let mode = Mode { size: (w, h).into(), refresh: 60_000 };
    let output = Output::new(
        OUTPUT_NAME.to_string(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "Tabula".into(),
            model: "Headless".into(),
            serial_number: "0".into(),
        },
    );
    let _global = output.create_global::<AnvilState<HeadlessData>>(&display_handle);
    output.change_current_state(Some(mode), Some(Transform::Normal), None, Some((0, 0).into()));
    output.set_preferred(mode);

    let formats = renderer.dmabuf_formats();
    let mut dmabuf_state = DmabufState::new();
    let dmabuf_global = dmabuf_state.create_global::<AnvilState<HeadlessData>>(&display_handle, formats);

    let buffer_size: Size<i32, Buffer> = (w, h).into();
    let target: GlesTexture = renderer.create_buffer(Fourcc::Abgr8888, buffer_size).expect("offscreen buffer");

    let data = HeadlessData {
        renderer,
        target,
        damage_tracker: OutputDamageTracker::from_output(&output),
        dmabuf_state: (dmabuf_state, dmabuf_global),
    };
    let mut state = AnvilState::init(display, event_loop.handle(), data, true);
    let shm_formats = state.backend_data.renderer.shm_formats();
    state.shm_state.update_formats(shm_formats);
    state.apply_output_settings(&output);
    state.space.map_output(&output, (0, 0));

    #[cfg(feature = "xwayland")]
    state.start_xwayland();

    let frame = Duration::from_micros(16_667);
    let out = output.clone();
    event_loop
        .handle()
        .insert_source(Timer::from_duration(frame), move |_, _, state| {
            render_frame(state, &out);
            TimeoutAction::ToDuration(frame)
        })
        .expect("frame timer");

    info!("Initialization completed, starting the main loop.");
    while state.running.load(Ordering::SeqCst) {
        if event_loop.dispatch(Some(Duration::from_millis(16)), &mut state).is_err() {
            state.running.store(false, Ordering::SeqCst);
            break;
        }
        state.space.refresh();
        state.popups.cleanup();
        state.tick();
        for (path, conn) in std::mem::take(&mut state.ipc.pending_screenshots) {
            let color = state.desktop_color();
            let res = crate::render::screenshot::<_, GlesTexture>(
                &mut state.backend_data.renderer,
                &output,
                &state.space,
                color,
                &path,
            );
            conn.send(&match res {
                Ok(()) => tabula_protocol::Reply::Ok,
                Err(message) => tabula_protocol::Reply::Error { message },
            });
        }
        let _ = state.display_handle.flush_clients();
    }
    state.save_layout();
}

fn render_frame(state: &mut AnvilState<HeadlessData>, output: &Output) {
    let now = state.clock.now();
    let frame_target = now + Duration::from_micros(16_667);
    state.pre_repaint(output, frame_target);
    let desktop_color = state.desktop_color();
    let data = &mut state.backend_data;
    let result = {
        let Ok(mut fb) = data.renderer.bind(&mut data.target) else {
            warn!("cannot bind offscreen target");
            return;
        };
        render_output(
            output,
            &state.space,
            Vec::new(),
            &mut data.renderer,
            &mut fb,
            &mut data.damage_tracker,
            // The single offscreen buffer always holds the last frame.
            1,
            false,
            desktop_color,
        )
    };
    match result {
        Ok(r) => {
            let states = r.states;
            // Without this every surface counts as off-screen and Smithay
            // throttles its frame callbacks to one a second.
            update_primary_scanout_output(&state.space, output, &state.dnd_icon, &state.cursor_status, &states);
            if r.damage.is_some() {
                let mut feedback = take_presentation_feedback(output, &state.space, &states);
                feedback.presented(
                    frame_target,
                    Refresh::fixed(Duration::from_micros(16_667)),
                    0,
                    wp_presentation_feedback::Kind::empty(),
                );
            }
            state.post_repaint(output, frame_target, None, &states);
        }
        Err(e) => warn!("headless render: {e:?}"),
    }
}
