//! A headless backend: renders offscreen on a GPU render node with no
//! window or display, driven by a 60 Hz timer. Input comes only from the
//! IPC `Synthetic` request, and screenshots work as on the other backends.
//! Used for automated UI testing without touching the user's screen:
//!
//! ```text
//! olo-app-server --headless [WIDTHxHEIGHT[,WIDTHxHEIGHT…]]   (physical pixels)
//! ```
//!
//! Each size is a screen (HEADLESS-1, HEADLESS-2, …), offering that size
//! and a few smaller modes so Preferences ▸ Display can be tested.

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

/// The first headless screen's name.
pub const OUTPUT_NAME: &str = "HEADLESS-1";

/// One offscreen screen.
struct Screen {
    output: Output,
    target: GlesTexture,
    damage_tracker: OutputDamageTracker,
}

pub struct HeadlessData {
    renderer: GlesRenderer,
    screens: Vec<Screen>,
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

    fn set_displays_on(&mut self, on: bool) {
        tracing::info!(on, "displays (simulated)");
    }

    /// No scan-out to warm; logged so tests can follow a fade.
    fn set_white_point(&mut self, gains: [f64; 3]) {
        tracing::info!(r = gains[0], g = gains[1], b = gains[2], "white point");
    }

    fn set_mode(&mut self, output: &Output, mode: Mode) -> Result<(), String> {
        if !output.modes().contains(&mode) {
            return Err("the screen doesn't offer that mode".into());
        }
        let screen = self.screens.iter_mut().find(|s| s.output == *output).ok_or("no such screen")?;
        let size: Size<i32, Buffer> = (mode.size.w, mode.size.h).into();
        screen.target = self.renderer.create_buffer(Fourcc::Abgr8888, size).map_err(|e| e.to_string())?;
        output.change_current_state(Some(mode), None, None, None);
        screen.damage_tracker = OutputDamageTracker::from_output(output);
        info!(output = output.name(), ?mode, "mode changed");
        Ok(())
    }
}

fn render_node() -> Option<std::path::PathBuf> {
    if let Ok(p) = std::env::var("OLO_RENDER_NODE") {
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

/// "2560x1600,1920x1080" → one size per screen; nothing usable → one 2560×1600.
fn parse_sizes(arg: Option<&str>) -> Vec<(i32, i32)> {
    let sizes: Vec<(i32, i32)> = arg
        .unwrap_or_default()
        .split(',')
        .filter_map(|s| s.split_once('x'))
        .filter_map(|(w, h)| Some((w.trim().parse().ok()?, h.trim().parse().ok()?)))
        .filter(|&(w, h): &(i32, i32)| w > 0 && h > 0)
        .collect();
    if sizes.is_empty() { vec![(2560, 1600)] } else { sizes }
}

/// The modes a screen of `size` offers: its own at 60 and 120 Hz, then
/// common smaller sizes at 60 Hz.
fn modes_for((w, h): (i32, i32)) -> Vec<Mode> {
    let mut modes = vec![Mode { size: (w, h).into(), refresh: 60_000 }, Mode { size: (w, h).into(), refresh: 120_000 }];
    for (mw, mh) in [(1920, 1200), (1920, 1080), (1680, 1050), (1280, 800), (1280, 720)] {
        if mw < w && mh <= h {
            modes.push(Mode { size: (mw, mh).into(), refresh: 60_000 });
        }
    }
    modes
}

pub fn run_headless(size_arg: Option<&str>) {
    let sizes = parse_sizes(size_arg);

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

    let mut screens = Vec::new();
    for (i, &size) in sizes.iter().enumerate() {
        let modes = modes_for(size);
        let output = Output::new(
            format!("HEADLESS-{}", i + 1),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "Olo".into(),
                model: "Headless".into(),
                serial_number: i.to_string(),
            },
        );
        let _global = output.create_global::<AnvilState<HeadlessData>>(&display_handle);
        for &m in &modes {
            output.add_mode(m);
        }
        output.change_current_state(Some(modes[0]), Some(Transform::Normal), None, Some((0, 0).into()));
        output.set_preferred(modes[0]);
        let buffer_size: Size<i32, Buffer> = size.into();
        let target: GlesTexture = renderer.create_buffer(Fourcc::Abgr8888, buffer_size).expect("offscreen buffer");
        let damage_tracker = OutputDamageTracker::from_output(&output);
        screens.push(Screen { output, target, damage_tracker });
    }

    // Feedback naming the render node: Mesa's EGL finds the GPU through it
    // (without, GL clients fall back to software rendering).
    let formats = renderer.dmabuf_formats();
    let mut dmabuf_state = DmabufState::new();
    let feedback = smithay::backend::drm::DrmNode::from_path(&node)
        .ok()
        .and_then(|n| smithay::wayland::dmabuf::DmabufFeedbackBuilder::new(n.dev_id(), formats.clone()).build().ok());
    let dmabuf_global = match &feedback {
        Some(f) => dmabuf_state.create_global_with_default_feedback::<AnvilState<HeadlessData>>(&display_handle, f),
        None => dmabuf_state.create_global::<AnvilState<HeadlessData>>(&display_handle, formats),
    };

    let outputs: Vec<Output> = screens.iter().map(|s| s.output.clone()).collect();
    let data = HeadlessData { renderer, screens, dmabuf_state: (dmabuf_state, dmabuf_global) };
    let mut state = AnvilState::init(display, event_loop.handle(), data, true);
    let shm_formats = state.backend_data.renderer.shm_formats();
    state.shm_state.update_formats(shm_formats);
    for output in &outputs {
        // A resolution chosen in Preferences, as DRM outputs do.
        let saved = state.settings.display.config(&crate::desktop::display_key(output), &output.name()).and_then(|c| c.mode.clone());
        if let Some(m) = saved {
            let mode = Mode { size: (m.width, m.height).into(), refresh: m.refresh };
            if output.current_mode() != Some(mode)
                && let Err(e) = state.backend_data.set_mode(output, mode)
            {
                warn!(output = output.name(), "can't restore the saved mode: {e}");
            }
        }
        state.apply_output_settings(output);
        state.space.map_output(output, (0, 0));
    }
    crate::shell::fixup_positions(&mut state.space, &state.settings.display, state.pointer.current_location());
    state.apply_mirroring();


    let frame = Duration::from_micros(16_667);
    event_loop
        .handle()
        .insert_source(Timer::from_duration(frame), move |_, _, state| {
            for i in 0..state.backend_data.screens.len() {
                render_frame(state, i);
            }
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
        for output in &outputs {
            for shot in state.ipc.shots_for(output) {
                let color = state.desktop_color();
                let mut overlays = twitcher_overlay(&mut state, output);
                let locked = state.lock.locked();
                let pointer = state.pointer.current_location();
                // What is being dragged, under the pointer (on top).
                if let Some(icon) = state.dnd_icon.as_ref().filter(|i| smithay::utils::IsAlive::alive(&i.surface) && !locked) {
                    use smithay::backend::renderer::element::AsRenderElements;
                    let origin = state.space.output_geometry(output).map(|g| g.loc).unwrap_or_default();
                    let scale = output.current_scale().fractional_scale();
                    let at = (pointer - origin.to_f64() + icon.offset.to_f64()).to_physical(scale).to_i32_round();
                    let tree = smithay::desktop::space::SurfaceTree::from_surface(&icon.surface);
                    let dragged: Vec<crate::render::CustomRenderElements<GlesRenderer>> =
                        AsRenderElements::<GlesRenderer>::render_elements(&tree, &mut state.backend_data.renderer, at, scale.into(), 1.0);
                    overlays.splice(0..0, dragged);
                }
                crate::screenshot::take::<_, GlesTexture>(
                    &mut state.ipc,
                    shot,
                    &mut state.backend_data.renderer,
                    output,
                    &state.space,
                    overlays,
                    color,
                    locked,
                    (pointer, crate::cursor::pictured(&state.cursor_status)),
                );
            }
        }
        let _ = state.display_handle.flush_clients();
    }
    state.save_layout();
}

/// Keyboard hints and Twitcher, if they are up, as elements for `output`
/// (or, while locked, the lock screen).
fn twitcher_overlay(
    state: &mut AnvilState<HeadlessData>,
    output: &Output,
) -> Vec<crate::render::CustomRenderElements<GlesRenderer>> {
    if state.lock.locked() {
        return crate::lock::elements(&state.lock, &mut state.backend_data.renderer, output);
    }
    let geo = state.space.output_geometry(output).unwrap_or_default();
    let scale = output.current_scale().fractional_scale();
    let mut out: Vec<_> = state
        .hints
        .elements(&mut state.backend_data.renderer, geo, scale)
        .into_iter()
        .map(crate::render::CustomRenderElements::Overlay)
        .collect();
    let Some(shown) = state.twitcher_entries() else { return out };
    out.extend(
        state
            .twitcher
            .element(&mut state.backend_data.renderer, &shown, geo.size, scale)
            .map(crate::render::CustomRenderElements::Overlay),
    );
    out
}

fn render_frame(state: &mut AnvilState<HeadlessData>, index: usize) {
    let screen = state.backend_data.screens[index].output.clone();
    // A mirror draws its source's frame, fitted.
    let source = crate::render::mirror_source(&screen);
    let fit = source.as_ref().map(|s| crate::render::fit_outputs(&screen, s, &state.space));
    let output = source.as_ref().unwrap_or(&screen);
    let now = state.clock.now();
    let frame_target = now + Duration::from_micros(16_667);
    state.pre_repaint(output, frame_target);
    let desktop_color = state.desktop_color();
    let overlay = twitcher_overlay(state, output);
    let locked = state.lock.locked();
    let data = &mut state.backend_data;
    let screen = &mut data.screens[index];
    let result = {
        let Ok(mut fb) = data.renderer.bind(&mut screen.target) else {
            warn!("cannot bind offscreen target");
            return;
        };
        render_output(
            output,
            &state.space,
            overlay,
            &mut data.renderer,
            &mut fb,
            &mut screen.damage_tracker,
            // The single offscreen buffer always holds the last frame.
            1,
            false,
            desktop_color,
            locked,
            fit,
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
