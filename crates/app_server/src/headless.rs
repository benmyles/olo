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
            EventLoop, LoopHandle, RegistrationToken,
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
    /// When its next frame is due.
    due: std::time::Instant,
    /// When the GPU has finished its last frame. A display takes a frame
    /// only once it has shown the last; drawing on before the GPU is done
    /// piles frames up, and each new one then needs memory of its own
    /// (Mesa's for its commands: bound into the GPU's address space, which
    /// can hold the CPU for tens of milliseconds while the GPU is busy).
    drawn: Option<smithay::backend::renderer::sync::SyncPoint>,
    /// Looks in a row that found nothing new (`render::IDLE_FRAMES`).
    idle_frames: u32,
    /// Resting: its next look is `render::IDLE_POLL` away, unless woken.
    asleep: bool,
    /// When it last looked, so a wake comes no sooner than a frame after.
    looked: Option<std::time::Instant>,
}

/// How long a frame of `output` lasts, at its mode's rate (60 or 120 Hz).
fn frame_time(output: &Output) -> Duration {
    let mhz = output.current_mode().map_or(60_000, |m| m.refresh).max(1_000);
    Duration::from_nanos(1_000_000_000_000 / mhz as u64)
}

pub struct HeadlessData {
    renderer: GlesRenderer,
    screens: Vec<Screen>,
    dmabuf_state: (DmabufState, DmabufGlobal),
    dmabuf_checks: crate::dmabuf_check::Checks,
    handle: LoopHandle<'static, AnvilState<HeadlessData>>,
    /// The timer for the screen due next (`frames_due`), and when it fires.
    frame_timer: Option<(RegistrationToken, std::time::Instant)>,
    /// In `frames_due`: a wake there only moves screens' times up.
    in_frames: bool,
}

impl DmabufHandler for AnvilState<HeadlessData> {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.backend_data.dmabuf_state.0
    }

    fn dmabuf_imported(&mut self, _global: &DmabufGlobal, dmabuf: Dmabuf, notifier: ImportNotifier) {
        // Checked off the loop (`dmabuf_check`); here only if that can't be.
        if let Some((dmabuf, notifier)) = self.backend_data.dmabuf_checks.start(dmabuf, notifier) {
            self.dmabuf_checked(dmabuf, notifier, true, None);
        }
    }
}

impl AnvilState<HeadlessData> {
    /// Import a checked buffer, tell the client whether it is good, and let
    /// its commits waiting for that through.
    fn dmabuf_checked(&mut self, dmabuf: Dmabuf, notifier: ImportNotifier, ok: bool, gate: Option<crate::dmabuf_check::Gate>) {
        if ok && self.backend_data.renderer.import_dmabuf(&dmabuf, None).is_ok() {
            let _ = notifier.successful::<AnvilState<HeadlessData>>();
        } else {
            notifier.failed();
        }
        let dh = self.display_handle.clone();
        for client in gate.map(|g| g.open()).unwrap_or_default() {
            smithay::wayland::compositor::CompositorHandler::client_compositor_state(self, &client).blocker_cleared(self, &dh);
        }
    }
}

impl Backend for HeadlessData {
    fn dmabuf_gate(&self, dmabuf: &Dmabuf) -> Option<crate::dmabuf_check::Gate> {
        self.dmabuf_checks.gate(dmabuf)
    }
    fn seat_name(&self) -> String {
        "headless".into()
    }
    fn reset_buffers(&mut self, _output: &Output) {}
    fn wake(&mut self, outputs: Option<&[Output]>) {
        let now = std::time::Instant::now();
        let mut first = None::<std::time::Instant>;
        for s in &mut self.screens {
            if !s.asleep {
                continue;
            }
            // (A mirror shows its source's frame.)
            if let Some(outputs) = outputs {
                let source = crate::render::mirror_source(&s.output);
                if !outputs.iter().any(|o| *o == s.output || source.as_ref() == Some(o)) {
                    continue;
                }
            }
            s.asleep = false;
            // (A frame after its last look at least: a storm of wakes, the
            // pointer moving, stays a look a frame.)
            s.due = s.looked.map_or(now, |l| (l + frame_time(&s.output)).max(now));
            first = Some(first.map_or(s.due, |f| f.min(s.due)));
        }
        // The timer set for later: sooner.
        if let Some(at) = first
            && !self.in_frames
            && self.frame_timer.as_ref().is_none_or(|(_, when)| *when > at)
        {
            if let Some((token, _)) = self.frame_timer.take() {
                self.handle.remove(token);
            }
            schedule_frames(self, at);
        }
    }
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

    // The displays come this long after the start, one after another (a
    // test of what a real one sees: windows reconnecting before DRM has
    // found the displays).
    let late = std::env::var("OLO_HEADLESS_LATE_MS").ok().and_then(|v| v.parse::<u64>().ok()).filter(|&ms| ms > 0).map(Duration::from_millis);
    // The displays don't say who they are until this long after they're
    // found (a test of a monitor whose EDID comes late, as Apple's DCP
    // gives it after an upgrade: `display_identified`).
    let anonymous = std::env::var("OLO_HEADLESS_ANONYMOUS_MS").ok().and_then(|v| v.parse::<u64>().ok()).filter(|&ms| ms > 0).map(Duration::from_millis);
    let mut screens = Vec::new();
    for (i, &size) in sizes.iter().enumerate() {
        let modes = modes_for(size);
        let (make, model) = if anonymous.is_some() { ("Unknown", "Unknown") } else { ("Olo", "Headless") };
        let output = Output::new(
            format!("HEADLESS-{}", i + 1),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: make.into(),
                model: model.into(),
                serial_number: i.to_string(),
            },
        );
        // (Late, as DRM's come, for tests: `OLO_HEADLESS_LATE_MS`.)
        if late.is_none() {
            let _global = output.create_global::<AnvilState<HeadlessData>>(&display_handle);
        }
        for &m in &modes {
            output.add_mode(m);
        }
        output.change_current_state(Some(modes[0]), Some(Transform::Normal), None, Some((0, 0).into()));
        output.set_preferred(modes[0]);
        let buffer_size: Size<i32, Buffer> = size.into();
        let target: GlesTexture = renderer.create_buffer(Fourcc::Abgr8888, buffer_size).expect("offscreen buffer");
        let damage_tracker = OutputDamageTracker::from_output(&output);
        screens.push(Screen { output, target, damage_tracker, due: std::time::Instant::now(), drawn: None, idle_frames: 0, asleep: false, looked: None });
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

    let (checked, answers) = smithay::reexports::calloop::channel::channel();
    let dmabuf_checks = crate::dmabuf_check::Checks::new(renderer.egl_context().display().clone(), checked);
    event_loop
        .handle()
        .insert_source(answers, |event, _, state: &mut AnvilState<HeadlessData>| {
            let smithay::reexports::calloop::channel::Event::Msg(p) = event else { return };
            if let Some((dmabuf, notifier, gate)) = state.backend_data.dmabuf_checks.finish(&p) {
                state.dmabuf_checked(dmabuf, notifier, p.image.is_some(), Some(gate));
            }
        })
        .expect("dmabuf checks");

    // Screen captures draw into dmabufs of what this GPU renders.
    let capture_dma = smithay::backend::drm::DrmNode::from_path(&node)
        .ok()
        .and_then(|n| crate::capture::dma_constraints(n, renderer.egl_context().dmabuf_render_formats()));
    let outputs: Vec<Output> = screens.iter().map(|s| s.output.clone()).collect();
    let data = HeadlessData {
        renderer,
        screens,
        dmabuf_state: (dmabuf_state, dmabuf_global),
        dmabuf_checks,
        handle: event_loop.handle(),
        frame_timer: None,
        in_frames: false,
    };
    let mut state = AnvilState::init(display, event_loop.handle(), data, true);
    state.captures.dma = capture_dma;
    let shm_formats = state.backend_data.renderer.shm_formats();
    state.shm_state.update_formats(shm_formats);
    match late {
        None => {
            for output in &outputs {
                add_output(&mut state, output);
            }
            state.displays_changed();
        }
        Some(after) => {
            for (i, output) in outputs.iter().enumerate() {
                let output = output.clone();
                let _ = event_loop.handle().insert_source(Timer::from_duration(after * (i as u32 + 1)), move |_, _, state| {
                    let _global = output.create_global::<AnvilState<HeadlessData>>(&state.display_handle);
                    info!(output = output.name(), "headless display added late");
                    add_output(state, &output);
                    state.displays_changed();
                    TimeoutAction::Drop
                });
            }
        }
    }


    // Each screen draws at its mode's rate, on time (`frames_due`).
    schedule_frames(&mut state.backend_data, std::time::Instant::now());

    info!("Initialization completed, starting the main loop.");
    while state.running.load(Ordering::SeqCst) {
        // (As udev's: the timeout only keeps `tick` looking now and then.)
        if event_loop.dispatch(Some(crate::render::IDLE_POLL), &mut state).is_err() {
            state.running.store(false, Ordering::SeqCst);
            break;
        }
        let start = std::time::Instant::now();
        state.space.refresh();
        state.popups.cleanup();
        state.timings.phase("space refresh", start);
        let t = std::time::Instant::now();
        state.tick();
        state.timings.phase("tick", t);
        state.timings.handled(|| "tick".into(), start);
        for output in &outputs {
            for shot in state.ipc.shots_for(output) {
                let color = state.desktop_color(output);
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
                    state.overview.as_ref(),
                    color,
                    locked,
                    (pointer, crate::cursor::pictured(&crate::saver::shown_cursor(&state.saver, &state.cursor_status))),
                );
            }
            // Frames programs asked for (screen sharing).
            state.captures.finish();
            let here = crate::capture::windows_on(&state.space, output, state.lock.locked());
            let on_output = |id| here.contains(&id);
            if state.captures.wanted(output, &on_output) {
                let color = state.desktop_color(output);
                let mut overlays = [Some(twitcher_overlay(&mut state, output)), Some(twitcher_overlay(&mut state, output))];
                let locked = state.lock.locked();
                let pointer = (state.pointer.current_location(), crate::saver::shown_cursor(&state.saver, &state.cursor_status));
                let now = state.clock.now().into();
                let (space, overview) = (&state.space, state.overview.as_ref());
                state.captures.take(&mut state.backend_data.renderer, output, now, on_output, |renderer, what, cursor| {
                    crate::capture::picture(renderer, output, space, what, locked, || {
                        let overlays = overlays[cursor as usize].take().unwrap_or_default();
                        (overlays, overview, color, cursor.then_some((pointer.0, &pointer.1)))
                    })
                });
            }
        }
        // Recording (finding flicker): what this turn left on screen.
        if let Some(name) = state.ipc.recording.as_ref().map(|r| r.output.clone()) {
            let output = outputs.iter().find(|o| name.as_ref().is_none_or(|n| *n == o.name())).cloned();
            if let Some(output) = output {
                let color = state.desktop_color(&output);
                let overlays = twitcher_overlay(&mut state, &output);
                let locked = state.lock.locked();
                let shot = crate::render::capture::<_, GlesTexture>(&mut state.backend_data.renderer, &output, &state.space, overlays, state.overview.as_ref(), color, locked);
                let more = match shot {
                    Ok(img) => state.ipc.recording.as_mut().is_some_and(|r| r.add(img)),
                    Err(e) => {
                        warn!(%e, "recording failed");
                        false
                    }
                };
                if !more {
                    state.ipc.recording = None;
                }
            }
        }
        let start = std::time::Instant::now();
        let _ = state.display_handle.flush_clients();
        state.timings.handled(|| "flushing to clients".into(), start);
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
            .element(&mut state.backend_data.renderer, &shown, &output.name(), geo.size, scale)
            .map(crate::render::CustomRenderElements::Overlay),
    );
    out
}

/// Put `output` in the layout, as a DRM display is once found: its
/// resolution chosen in Preferences, its settings, its place.
fn add_output(state: &mut AnvilState<HeadlessData>, output: &Output) {
    if let Some(after) = std::env::var("OLO_HEADLESS_ANONYMOUS_MS").ok().and_then(|v| v.parse::<u64>().ok()).filter(|&ms| ms > 0) {
        let (output, serial) = (output.clone(), output.physical_properties().serial_number);
        let _ = state.handle.insert_source(Timer::from_duration(Duration::from_millis(after)), move |_, _, state| {
            state.display_identified(&output, "Olo".into(), "Headless".into(), serial.clone());
            TimeoutAction::Drop
        });
    }
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
    crate::shell::fixup_positions(&mut state.space, &state.settings.display, state.pointer.current_location());
    state.apply_mirroring();
}

/// Each screen draws at its mode's rate, on time; one that found nothing
/// new for `render::IDLE_FRAMES` rests, looking every `render::IDLE_POLL`
/// until woken (`Backend::wake`). The timer wakes for whichever is due
/// next.
fn schedule_frames(data: &mut HeadlessData, at: std::time::Instant) {
    let token = data.handle.insert_source(Timer::from_deadline(at), move |_, _, state| {
        state.backend_data.frame_timer = None;
        let next = frames_due(state);
        schedule_frames(&mut state.backend_data, next);
        TimeoutAction::Drop
    });
    data.frame_timer = token.ok().map(|t| (t, at));
}

/// Draw the screens due; when the next is due.
fn frames_due(state: &mut AnvilState<HeadlessData>) -> std::time::Instant {
    let now = std::time::Instant::now();
    state.backend_data.in_frames = true;
    for i in 0..state.backend_data.screens.len() {
        let screen = &mut state.backend_data.screens[i];
        if screen.due > now + Duration::from_micros(500) {
            continue;
        }
        let frame = frame_time(&screen.output);
        // Late by more than a frame (a stall): start afresh.
        screen.due = if screen.due + frame < now { now + frame } else { screen.due + frame };
        // The GPU still on the last: this one waits for the next
        // tick (its damage kept, its clients' frames not yet done).
        if screen.drawn.as_ref().is_some_and(|s| !s.is_reached()) {
            continue;
        }
        let was_asleep = std::mem::replace(&mut screen.asleep, false);
        screen.looked = Some(now);
        // (Something of ours moving: it doesn't rest.)
        let awake = state.keeps_awake();
        let t = std::time::Instant::now();
        let new = render_frame(state, i);
        state.timings.phase("look (whole)", t);
        for (name, took) in crate::render::take_parts() {
            state.timings.add(name, took);
        }
        let screen = &mut state.backend_data.screens[i];
        // (A resting look that found only the keep-warm pixel changed
        // rests on.)
        let output = screen.output.clone();
        let new = new && !(was_asleep && crate::render::warm_changed(&output)) || crate::wallpaper::coming(&output) || awake;
        screen.idle_frames = if new { 0 } else { screen.idle_frames.saturating_add(1) };
        if new && was_asleep {
            // Something changed without waking it (worth a wake).
            tracing::debug!(output = screen.output.name(), "a resting display found something new at its look");
        }
        if screen.idle_frames >= crate::render::IDLE_FRAMES && crate::render::rests(false) {
            screen.asleep = true;
            screen.due = now + crate::render::IDLE_POLL;
        }
    }
    state.backend_data.in_frames = false;
    state.backend_data.screens.iter().map(|s| s.due).min().unwrap_or(now + Duration::from_millis(16))
}

/// Draw screen `index`; whether anything on it was new.
fn render_frame(state: &mut AnvilState<HeadlessData>, index: usize) -> bool {
    let screen = state.backend_data.screens[index].output.clone();
    // (Not found yet: `OLO_HEADLESS_LATE_MS`.)
    if state.space.output_geometry(&screen).is_none() {
        return false;
    }
    // A mirror draws its source's frame, fitted.
    let source = crate::render::mirror_source(&screen);
    let fit = source.as_ref().map(|s| crate::render::fit_outputs(&screen, s, &state.space));
    let output = source.as_ref().unwrap_or(&screen);
    let (start, name) = (std::time::Instant::now(), output.name());
    let now = state.clock.now();
    let frame = frame_time(output);
    let frame_target = now + frame;
    let t = std::time::Instant::now();
    state.pre_repaint(output, frame_target);
    state.timings.phase("pre_repaint", t);
    let desktop_color = state.desktop_color(output);
    let overlay = twitcher_overlay(state, output);
    let locked = state.lock.locked();
    let data = &mut state.backend_data;
    let screen = &mut data.screens[index];
    let result = {
        let bind = std::time::Instant::now();
        let Ok(mut fb) = data.renderer.bind(&mut screen.target) else {
            warn!("cannot bind offscreen target");
            return false;
        };
        // (Binding should be nothing; when it isn't, the GPU was busy.)
        if bind.elapsed() > std::time::Duration::from_millis(4) {
            tracing::info!("slow frame on {name}: {:.1} ms binding the target", bind.elapsed().as_secs_f64() * 1e3);
        }
        render_output(
            output,
            &state.space,
            overlay,
            &mut data.renderer,
            &mut fb,
            &mut screen.damage_tracker,
            // The single offscreen buffer always holds the last frame.
            1,
            state.overview.as_ref(),
            desktop_color,
            locked,
            fit,
        )
    };
    let new = match result {
        Ok(r) => {
            let (damaged, sync, states) = (r.damage.is_some(), r.sync, r.states);
            state.backend_data.screens[index].drawn = Some(sync);
            // Without this every surface counts as off-screen and Smithay
            // throttles its frame callbacks to one a second.
            update_primary_scanout_output(&state.space, output, &state.dnd_icon, &state.cursor_status, &states);
            if damaged {
                let mut feedback = take_presentation_feedback(output, &state.space, &states);
                feedback.presented(
                    frame_target,
                    Refresh::fixed(frame),
                    0,
                    wp_presentation_feedback::Kind::empty(),
                );
            }
            let t = std::time::Instant::now();
            state.post_repaint(output, None, &states);
            state.timings.phase("post_repaint", t);
            damaged
        }
        Err(e) => {
            warn!("headless render: {e:?}");
            false
        }
    };
    state.timings.frame(&name, start.elapsed());
    new
}

#[cfg(test)]
mod gpu_timing {
    use smithay::{
        backend::{
            allocator::{Fourcc, gbm::GbmDevice},
            egl::{EGLContext, EGLDisplay},
            renderer::{Bind, Color32F, Frame, ImportMem, Offscreen, Renderer, gles::{GlesRenderer, GlesTexture}},
        },
        utils::{Buffer, Rectangle, Size, Transform},
    };
    use std::time::Instant;

    /// What putting small pictures (a tab's look) on the GPU costs: new
    /// textures against writing into one made before, with the GPU idle
    /// and busy drawing a full frame (`cargo test --release -p app_server
    /// gpu_upload_timing -- --ignored --nocapture`).
    #[test]
    #[ignore]
    fn gpu_upload_timing() {
        let node = super::render_node().expect("a render node");
        let file = std::fs::OpenOptions::new().read(true).write(true).open(&node).unwrap();
        let gbm = GbmDevice::new(file).unwrap();
        let egl = unsafe { EGLDisplay::new(gbm) }.unwrap();
        let context = EGLContext::new(&egl).unwrap();
        let mut r = unsafe { GlesRenderer::new(context) }.unwrap();
        let (w, h) = (147, 31);
        let data = vec![0x80u8; (w * h * 4) as usize];
        let size: Size<i32, Buffer> = (w, h).into();
        let screen: Size<i32, Buffer> = (3456, 2234).into();
        let mut target: GlesTexture = r.create_buffer(Fourcc::Abgr8888, screen).unwrap();
        let big = r.import_memory(&vec![0x40u8; (1600 * 1000 * 4) as usize], Fourcc::Abgr8888, (1600, 1000).into(), false).unwrap();
        let mut busy = |r: &mut GlesRenderer| {
            let mut fb = r.bind(&mut target).unwrap();
            let s = (3456, 2234).into();
            let mut f = r.render(&mut fb, s, Transform::Normal).unwrap();
            let all = Rectangle::from_size(s);
            f.clear(Color32F::new(0.2, 0.2, 0.2, 1.0), &[all]).unwrap();
            for _ in 0..20 {
                f.render_texture_from_to(&big, Rectangle::from_size((1600.0, 1000.0).into()), all, &[all], &[], Transform::Normal, 0.5, None, &[]).unwrap();
            }
            let _ = f.finish().unwrap();
        };
        let ms = |t: Instant| t.elapsed().as_secs_f64() * 1e3;
        let report = |what: &str, times: &mut Vec<f64>| {
            times.sort_by(f64::total_cmp);
            eprintln!("{what:40} median {:.3} ms, worst {:.3} ms", times[times.len() / 2], times[times.len() - 1]);
        };
        for gpu_busy in [false, true] {
            let mut fresh = Vec::new();
            let mut kept = Vec::new();
            let reused = r.import_memory(&data, Fourcc::Abgr8888, size, false).unwrap();
            let mut textures = Vec::new();
            for _ in 0..60 {
                if gpu_busy {
                    busy(&mut r);
                }
                let t = Instant::now();
                textures.push(r.import_memory(&data, Fourcc::Abgr8888, size, false).unwrap());
                fresh.push(ms(t));
                if textures.len() > 6 {
                    textures.remove(0);
                }
                if gpu_busy {
                    busy(&mut r);
                }
                let t = Instant::now();
                r.update_memory(&reused, &data, Rectangle::from_size(size)).unwrap();
                kept.push(ms(t));
            }
            let when = if gpu_busy { "GPU busy" } else { "GPU idle" };
            report(&format!("new texture, {when}"), &mut fresh);
            report(&format!("into one made before, {when}"), &mut kept);
        }
        let mut frames = Vec::new();
        for _ in 0..30 {
            let t = Instant::now();
            busy(&mut r);
            frames.push(ms(t));
        }
        report("a full frame's commands", &mut frames);
    }
}
