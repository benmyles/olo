//! Desktop-level state: window identity, workspaces, outputs and the
//! settings Preferences edits (theme, night mode, scaling, ...).

use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use smithay::{
    desktop::layer_map_for_output,
    backend::renderer::Color32F,
    desktop::{WindowSurface, space::SpaceElement},
    output::{Output, Scale},
    reexports::wayland_server::Resource,
    utils::{IsAlive, Logical, Point, Rectangle},
    wayland::seat::WaylandFocus,
};
use tabula_protocol::{Event, NightLightMode, OutputConfig, OutputInfo, Settings, ThemePreference, WindowInfo};
use tabula_theme::ThemeMode;
use tracing::{info, warn};

use crate::{
    shell::{WindowElement, fixup_positions},
    state::{AnvilState, Backend},
    theme,
};

/// Per-window bookkeeping that isn't decoration.
pub struct WindowMeta {
    pub id: u64,
    /// `None` until first parked; then the workspace it lives on.
    pub workspace: Cell<Option<u32>>,
    pub minimized: Cell<bool>,
    /// Where it was when it left the space (minimize / workspace switch).
    pub saved_location: Cell<Point<i32, Logical>>,
    /// Set on the first commit, once placement memory has been consulted.
    pub placed: Cell<bool>,
    /// It was the focused window before an in-place restart, put back at
    /// this time.
    pub restored_focus: Cell<Option<std::time::Instant>>,
}

impl WindowElement {
    pub fn meta(&self) -> &WindowMeta {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        self.user_data().insert_if_missing(|| WindowMeta {
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            workspace: Cell::new(None),
            minimized: Cell::new(false),
            saved_location: Cell::new(Point::default()),
            placed: Cell::new(false),
            restored_focus: Cell::new(None),
        });
        self.user_data().get::<WindowMeta>().unwrap()
    }

    pub fn id(&self) -> u64 {
        self.meta().id
    }
}

/// Twitcher's state while ⌘ is held: the candidates as they stood when
/// switching began (front first), and which one is chosen.
#[derive(Debug)]
pub struct Twitch {
    pub apps: bool,
    pub ids: Vec<u64>,
    pub index: usize,
}

/// Windows not currently in the space: minimized, or on another workspace.
/// Kept in stacking order (bottom first).
#[derive(Debug, Default)]
pub struct Parked(pub RefCell<Vec<WindowElement>>);

pub fn config_path() -> PathBuf {
    // An agent's screen keeps settings of its own (see apps/screen).
    if let Some(path) = std::env::var_os("TABULA_APP_SERVER_SETTINGS") {
        return PathBuf::from(path);
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    base.join("tabula").join("app_server.json")
}

pub fn load_settings() -> Settings {
    match std::fs::read_to_string(config_path()) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
            warn!(?e, "ignoring malformed settings");
            Settings::default()
        }),
        Err(_) => Settings::default(),
    }
}

pub(crate) fn save_settings(settings: &Settings) {
    let path = config_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = path.with_extension("json.tmp");
    let ok = serde_json::to_vec_pretty(settings)
        .ok()
        .and_then(|data| std::fs::write(&tmp, data).ok())
        .and_then(|_| std::fs::rename(&tmp, &path).ok());
    if ok.is_none() {
        warn!(path = %path.display(), "could not save settings");
    }
}

/// `output` is set to mirror another.
fn entry_mirrors(display: &tabula_protocol::DisplaySettings, output: &Output) -> bool {
    display.config(&display_key(output), &output.name()).is_some_and(|c| c.mirror.as_ref().is_some_and(|m| !m.is_empty()))
}

/// Where `output`'s settings are kept: its monitor, else its connector.
pub fn display_key(output: &Output) -> String {
    let p = output.physical_properties();
    tabula_protocol::display_key(&output.name(), &p.make, &p.model, &p.serial_number)
}

/// Minutes since local midnight.
fn local_minutes() -> u32 {
    // SAFETY: plain libc time calls with valid pointers.
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&now, &mut tm);
        (tm.tm_hour * 60 + tm.tm_min) as u32
    }
}

impl<B: Backend + 'static> AnvilState<B> {
    // ---- windows -------------------------------------------------------

    /// All toplevels, mapped and parked.
    pub fn all_windows(&self) -> Vec<WindowElement> {
        let mut v: Vec<WindowElement> = self.space.elements().cloned().collect();
        v.extend(self.parked.0.borrow().iter().cloned());
        v
    }

    pub fn window_by_id(&self, id: u64) -> Option<WindowElement> {
        self.all_windows().into_iter().find(|w| w.id() == id)
    }

    pub fn window_infos(&self) -> Vec<WindowInfo> {
        let focused = self
            .seat
            .get_keyboard()
            .and_then(|k| k.current_focus())
            .and_then(|f| f.wl_surface().map(|s| s.into_owned()));
        let mut infos: Vec<WindowInfo> = self
            .all_windows()
            .into_iter()
            .filter(|w| {
                // Skip override-redirect X11 popups and the like.
                match w.0.underlying_surface() {
                    WindowSurface::Wayland(_) => true,
                    #[cfg(feature = "xwayland")]
                    WindowSurface::X11(x) => !x.is_override_redirect(),
                }
            })
            .map(|w| {
                let pid = w.wl_surface().and_then(|s| {
                    self.display_handle
                        .get_client(s.id())
                        .ok()
                        .and_then(|c| c.get_credentials(&self.display_handle).ok())
                        .map(|c| c.pid)
                });
                let meta = w.meta();
                WindowInfo {
                    id: meta.id,
                    app_id: w.app_id(),
                    title: w.title(),
                    pid,
                    workspace: meta.workspace.get().unwrap_or(self.workspace),
                    minimized: meta.minimized.get(),
                    focused: focused.is_some() && w.wl_surface().map(|s| s.into_owned()) == focused,
                    frame: None,
                    content: None,
                }
            })
            .collect();
        infos.sort_by_key(|i| i.id);
        infos
    }

    /// [`window_infos`](Self::window_infos) with where each window on
    /// screen is.
    pub fn window_infos_with_frames(&self) -> Vec<WindowInfo> {
        let mut infos = self.window_infos();
        let frame = |r: smithay::utils::Rectangle<i32, smithay::utils::Logical>| tabula_protocol::Frame { x: r.loc.x, y: r.loc.y, width: r.size.w, height: r.size.h };
        for info in &mut infos {
            let Some(w) = self.space.elements().find(|w| w.id() == info.id) else { continue };
            let Some(at) = self.space.element_location(w) else { continue };
            let geo = smithay::desktop::space::SpaceElement::geometry(w);
            info.frame = Some(frame(smithay::utils::Rectangle::new(at, geo.size)));
            // As screenshots find it: where the element draws from, then
            // past the tab and border.
            info.content = Some(if w.decorated() {
                frame(smithay::utils::Rectangle::new(at - geo.loc + crate::shell::deco::content_offset(), w.content_size()))
            } else {
                frame(smithay::utils::Rectangle::new(at, geo.size))
            });
        }
        infos
    }

    /// Called once per loop iteration: drop dead windows and tell
    /// subscribers (Deskbar) when the window list changed.
    pub fn tick(&mut self) {
        self.parked.0.borrow_mut().retain(|w| w.alive());
        let infos = self.window_infos();
        if infos != self.last_window_infos {
            crate::twitcher::prewarm(infos.iter().map(|i| i.app_id.as_str()));
            self.last_window_infos = infos.clone();
            self.ipc.broadcast(Event::WindowsChanged { windows: infos });
        }
    }

    // ---- workspaces ----------------------------------------------------

    fn park(&mut self, window: &WindowElement) {
        if let Some(loc) = self.space.element_location(window) {
            window.meta().saved_location.set(loc);
        }
        if window.meta().workspace.get().is_none() {
            window.meta().workspace.set(Some(self.workspace));
        }
        window.set_activate(false);
        self.space.unmap_elem(window);
        self.parked.0.borrow_mut().push(window.clone());
    }

    fn unpark(&mut self, window: &WindowElement) {
        self.parked.0.borrow_mut().retain(|w| w != window);
        window.meta().minimized.set(false);
        window.meta().workspace.set(Some(self.workspace));
        let loc = window.meta().saved_location.get();
        self.space.map_element(window.clone(), loc, false);
        // Its display may have gone while it was away.
        self.rehome_window(window);
    }

    pub fn minimize(&mut self, window: &WindowElement) {
        if self.space.element_location(window).is_none() {
            return;
        }
        self.park(window);
        window.meta().minimized.set(true);
        self.focus_topmost();
    }

    pub fn restore(&mut self, window: &WindowElement) {
        let ws = window.meta().workspace.get().unwrap_or(self.workspace);
        if ws != self.workspace {
            self.switch_workspace(ws);
        }
        if self.parked.0.borrow().contains(window) {
            self.unpark(window);
        }
        self.focus_window(window);
    }

    pub fn focus_topmost(&mut self) {
        let front = self.space.elements().next_back().cloned();
        match front {
            Some(front) => self.focus_window(&front),
            None => {
                let serial = smithay::utils::SERIAL_COUNTER.next_serial();
                if let Some(keyboard) = self.seat.get_keyboard() {
                    keyboard.set_focus(self, None, serial);
                }
            }
        }
    }

    /// Which display a new window of `client` goes on (the one under this
    /// point): its focused window's, if it has the focus (a window opened
    /// from the keyboard, e.g. Quick Look, belongs with the one it came
    /// from, not with a pointer that may be on another display), else the
    /// pointer's.
    pub fn new_window_near(&self, client: Option<smithay::reexports::wayland_server::backend::ClientId>) -> Point<f64, Logical> {
        let pointer = self.pointer.current_location();
        let Some(client) = client else { return pointer };
        let Some(w) = self.focused_window() else { return pointer };
        if w.wl_surface().and_then(|s| s.client()).map(|c| c.id()) != Some(client) {
            return pointer;
        }
        let Some(geo) = self.space.element_geometry(&w) else { return pointer };
        (geo.loc + geo.size.downscale(2).to_point()).to_f64()
    }

    /// The window with keyboard focus.
    pub fn focused_window(&self) -> Option<WindowElement> {
        let focus = self.seat.get_keyboard()?.current_focus()?;
        let surface = focus.wl_surface()?.into_owned();
        self.space.elements().find(|w| w.wl_surface().is_some_and(|s| *s == surface)).cloned()
    }

    /// Windows on this workspace, front first, minimized ones last.
    fn workspace_windows(&self) -> Vec<WindowElement> {
        let mut v: Vec<WindowElement> = self.space.elements().rev().cloned().collect();
        v.extend(
            self.parked
                .0
                .borrow()
                .iter()
                .rev()
                .filter(|w| w.meta().minimized.get() && w.meta().workspace.get().is_none_or(|ws| ws == self.workspace))
                .cloned(),
        );
        v
    }

    /// ⌘Tab (`apps`) or ⌘\`: bring the next app, or the focused app's next
    /// window, to the front. The order is fixed while ⌘ is held, so
    /// repeated presses walk it.
    pub fn twitch_step(&mut self, apps: bool, back: bool) {
        if self.twitch.as_ref().is_none_or(|t| t.apps != apps) {
            let windows = self.workspace_windows();
            let ids = if apps {
                let mut seen = Vec::new();
                let mut ids = Vec::new();
                for w in &windows {
                    let app = w.app_id();
                    if !seen.contains(&app) {
                        seen.push(app);
                        ids.push(w.id());
                    }
                }
                ids
            } else {
                let app = self.focused_window().map(|w| w.app_id());
                windows.iter().filter(|w| Some(w.app_id()) == app).map(|w| w.id()).collect()
            };
            // Start on the focused window's entry, so the first press moves
            // to the one behind it.
            let focused = self.focused_window().map(|w| (w.id(), w.app_id()));
            let index = ids
                .iter()
                .position(|id| {
                    focused.as_ref().is_some_and(|(fid, fapp)| {
                        *id == *fid || (apps && self.window_by_id(*id).is_some_and(|w| w.app_id() == *fapp))
                    })
                })
                .unwrap_or(0);
            self.twitch = Some(Twitch { apps, ids, index });
        }
        let Some(t) = self.twitch.as_mut() else { return };
        if t.ids.len() < 2 {
            return;
        }
        let n = t.ids.len();
        t.index = if back { (t.index + n - 1) % n } else { (t.index + 1) % n };
        let id = t.ids[t.index];
        if let Some(w) = self.window_by_id(id) {
            self.restore(&w);
        }
    }

    /// What Twitcher shows: its cells and the chosen one.
    pub fn twitcher_entries(&self) -> Option<(Vec<crate::twitcher::Entry>, usize)> {
        let t = self.twitch.as_ref()?;
        let entries: Vec<_> = t
            .ids
            .iter()
            .filter_map(|id| self.window_by_id(*id))
            .map(|w| crate::twitcher::Entry { app_id: w.app_id(), title: (!t.apps).then(|| w.title()) })
            .collect();
        (entries.len() == t.ids.len()).then_some((entries, t.index))
    }

    /// ⌘ released: the chosen window stays in front.
    pub fn twitch_done(&mut self) {
        self.twitch = None;
    }

    /// Start one of our programs in the session, without waiting for it.
    pub fn spawn_program(&self, program: &str, args: &[&str]) {
        // Our programs are installed beside app_server; prefer those (so a
        // dev build starts dev apps), then PATH.
        let Some(path) = tabula_xdg::find_program(program) else {
            warn!(program, "not installed");
            return;
        };
        let mut cmd = std::process::Command::new(path);
        cmd.args(args);
        if let Some(socket) = &self.socket_name {
            cmd.env("WAYLAND_DISPLAY", socket);
        }
        match cmd.spawn() {
            // Reap it when it exits.
            Ok(mut child) => {
                std::thread::spawn(move || child.wait());
            }
            Err(e) => warn!(program, %e, "could not start"),
        }
    }

    pub fn switch_workspace(&mut self, index: u32) {
        let count = self.settings.workspace_count.max(1);
        let index = index.min(count - 1);
        if index == self.workspace {
            return;
        }
        // Park everything visible, bottom-most first to keep stacking order.
        let visible: Vec<WindowElement> = self.space.elements().cloned().collect();
        for w in &visible {
            self.park(w);
        }
        self.workspace = index;
        let coming: Vec<WindowElement> = self
            .parked
            .0
            .borrow()
            .iter()
            .filter(|w| w.meta().workspace.get() == Some(index) && !w.meta().minimized.get())
            .cloned()
            .collect();
        for w in &coming {
            self.unpark(w);
        }
        self.focus_topmost();
        self.ipc.broadcast(Event::WorkspaceChanged { current: index, count });
        info!(workspace = index, "switched workspace");
    }

    pub fn move_to_workspace(&mut self, window: &WindowElement, index: u32) {
        let index = index.min(self.settings.workspace_count.max(1) - 1);
        if index == self.workspace {
            window.meta().workspace.set(Some(index));
            return;
        }
        if self.space.element_location(window).is_some() {
            self.park(window);
            self.focus_topmost();
        }
        window.meta().workspace.set(Some(index));
    }

    /// The current workspace's desktop colour.
    pub fn desktop_color(&self) -> Color32F {
        let [r, g, b] = self.settings.workspace_color(self.workspace);
        Color32F::new(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0)
    }

    // ---- outputs -------------------------------------------------------

    pub fn output_infos(&self) -> Vec<OutputInfo> {
        let main = self.main_output();
        self.space
            .outputs()
            .chain(self.mirrors.iter())
            .map(|o| {
                let props = o.physical_properties();
                let to_mode = |m: smithay::output::Mode| tabula_protocol::Mode {
                    width: m.size.w,
                    height: m.size.h,
                    refresh: m.refresh,
                };
                let source = crate::render::mirror_source(o);
                let placed = source.as_ref().unwrap_or(o);
                let loc = self.space.output_geometry(placed).map(|g| g.loc).unwrap_or_default();
                OutputInfo {
                    name: o.name(),
                    key: display_key(o),
                    make: props.make,
                    model: props.model,
                    physical_size_mm: (props.size.w, props.size.h),
                    modes: o.modes().into_iter().map(to_mode).collect(),
                    current_mode: o.current_mode().map(to_mode),
                    scale: o.current_scale().fractional_scale(),
                    position: (loc.x, loc.y),
                    rotation: self.settings.display.rotation_for(&display_key(o), &o.name()),
                    main: main.as_ref() == Some(o),
                    mirror_of: source.map(|s| s.name()),
                    brightness: if crate::power::is_internal(&o.name()) { self.power.brightness } else { self.power.display_brightness(&o.name()) },
                }
            })
            .collect()
    }

    /// Apply the configured scale and rotation to an output.
    pub fn apply_output_settings(&self, output: &Output) {
        let (key, name) = (display_key(output), output.name());
        let scale = self.settings.display.scale_for(&key, &name);
        let transform = rotation_transform(self.settings.display.rotation_for(&key, &name));
        output.change_current_state(None, Some(transform), Some(Scale::Fractional(scale)), None);
    }

    /// The main display: the chosen one while it is connected, else the first.
    pub fn main_output(&self) -> Option<Output> {
        let display = &self.settings.display;
        self.space
            .outputs()
            .find(|o| display.is_main(&display_key(o), &o.name()))
            .or_else(|| self.space.outputs().next())
            .cloned()
    }

    /// Take screens set to mirror another out of the desktop (they draw
    /// their source's picture), and put back ones that no longer do or
    /// whose source is gone. A screen can't mirror a mirror, so one
    /// always extends.
    pub fn apply_mirroring(&mut self) {
        let all: Vec<Output> = self.space.outputs().chain(self.mirrors.iter()).cloned().collect();
        let display = &self.settings.display;
        let wanted = |o: &Output| {
            display.config(&display_key(o), &o.name()).and_then(|c| c.mirror.clone()).filter(|k| !k.is_empty())
        };
        let mut sources: Vec<(Output, Option<Output>)> = Vec::new();
        for o in &all {
            let source = wanted(o).and_then(|key| all.iter().find(|s| *s != o && (display_key(s) == key || s.name() == key) && wanted(s).is_none()).cloned());
            sources.push((o.clone(), source));
        }
        let mut changed = Vec::new();
        for (o, source) in &sources {
            if crate::render::mirror_source(o) == *source {
                continue;
            }
            crate::render::set_mirror_source(o, source.clone());
            if source.is_some() {
                self.space.unmap_output(o);
            } else {
                self.space.map_output(o, (0, 0));
            }
            info!(output = o.name(), mirrors = ?source.as_ref().map(|s| s.name()), "mirroring");
            changed.push(o.clone());
        }
        self.mirrors = sources.into_iter().filter(|(_, s)| s.is_some()).map(|(o, _)| o).collect();
        if changed.is_empty() {
            return;
        }
        let pointer = self.pointer.current_location();
        fixup_positions(&mut self.space, &self.settings.display, pointer);
        let gone: Vec<Output> = changed.into_iter().filter(|o| self.mirrors.contains(o)).collect();
        self.move_auto_layers(&gone);
        self.follow_main_output();
        self.lock_outputs_changed();
        self.ipc.broadcast(Event::OutputsChanged { outputs: self.output_infos() });
    }

    /// Layer surfaces that didn't ask for an output (Deskbar, the desktop)
    /// live on the main display: move them there when it changes.
    /// After displays came, went or changed: Deskbar and the like to the
    /// main display, and windows onto displays that are there, fitted.
    pub fn follow_main_output(&mut self) {
        let Some(main) = self.main_output() else { return };
        let others: Vec<Output> = self.space.outputs().filter(|o| **o != main).cloned().collect();
        self.move_auto_layers(&others);
        self.rehome_windows();
    }

    /// Move the auto-placed layer surfaces on `from` to the main display;
    /// ones that named an output close when it is gone from the space.
    pub fn move_auto_layers(&mut self, from: &[Output]) {
        let Some(main) = self.main_output() else { return };
        for output in from.iter().filter(|o| **o != main) {
            if self.space.output_geometry(output).is_none() {
                for l in layer_map_for_output(output).layers() {
                    if l.user_data().get::<crate::shell::AutoOutput>().is_none() {
                        l.layer_surface().send_close();
                    }
                }
            }
            let moving: Vec<smithay::desktop::LayerSurface> = layer_map_for_output(output)
                .layers()
                .filter(|l| l.user_data().get::<crate::shell::AutoOutput>().is_some())
                .cloned()
                .collect();
            if moving.is_empty() {
                continue;
            }
            {
                let mut map = layer_map_for_output(output);
                for l in &moving {
                    map.unmap_layer(l);
                }
                map.arrange();
            }
            let mut map = layer_map_for_output(&main);
            for l in moving {
                if let Err(e) = map.map_layer(&l) {
                    warn!("can't move a layer surface to the main display: {e}");
                }
            }
            map.arrange();
            for l in map.layers() {
                l.layer_surface().send_pending_configure();
            }
        }
    }

    /// `name` changed size from `old` to `new`: screens placed against its
    /// right or bottom edge move with that edge, so the arrangement keeps
    /// touching (a gap would trap the pointer).
    fn keep_neighbours(&mut self, name: &str, old: Rectangle<i32, Logical>, new: Rectangle<i32, Logical>) {
        let (dw, dh) = (new.size.w - old.size.w, new.size.h - old.size.h);
        if (dw, dh) == (0, 0) {
            return;
        }
        let moves: Vec<(Output, (i32, i32))> = self
            .space
            .outputs()
            .filter(|o| o.name() != name)
            .filter_map(|o| {
                let g = self.space.output_geometry(o)?;
                let shift = (
                    if g.loc.x == old.loc.x + old.size.w { dw } else { 0 },
                    if g.loc.y == old.loc.y + old.size.h { dh } else { 0 },
                );
                (shift != (0, 0)).then(|| (o.clone(), (g.loc.x + shift.0, g.loc.y + shift.1)))
            })
            .collect();
        for (other, at) in moves {
            // Pin everything that is placed, so the row fallback can't reshuffle.
            self.settings.display.config_mut(&display_key(&other), &other.name()).position = Some(at);
        }
        let placed: Vec<(Output, (i32, i32))> = self
            .space
            .outputs()
            .filter_map(|o| Some((o.clone(), self.space.output_geometry(o)?.loc.into())))
            .collect();
        for (o, at) in placed {
            let entry = self.settings.display.config_mut(&display_key(&o), &o.name());
            entry.position.get_or_insert(at);
        }
    }

    pub fn configure_output(&mut self, name: &str, config: OutputConfig) -> Result<(), String> {
        let output = self
            .space
            .outputs()
            .chain(self.mirrors.iter())
            .find(|o| o.name() == name)
            .cloned()
            .ok_or_else(|| format!("no output named {name}"))?;
        let before = self.space.output_geometry(&output);
        if let Some(mode) = &config.mode {
            // Resolution goes through the backend (DRM, or headless for tests).
            let want = smithay::output::Mode { size: (mode.width, mode.height).into(), refresh: mode.refresh };
            if output.current_mode() != Some(want) {
                if !output.modes().contains(&want) {
                    return Err(format!("{name} doesn't offer {}×{}", mode.width, mode.height));
                }
                self.backend_data.set_mode(&output, want)?;
                self.backend_data.reset_buffers(&output);
            }
        }
        let entry = self.settings.display.config_mut(&display_key(&output), name);
        if let Some(mode) = config.mode {
            entry.mode = Some(mode);
        }
        if let Some(scale) = config.scale {
            entry.scale = Some(scale.clamp(0.5, 4.0));
        }
        if let Some(pos) = config.position {
            entry.position = Some(pos);
        }
        if let Some(r) = config.rotation.filter(|r| tabula_protocol::ROTATIONS.contains(r)) {
            entry.rotation = Some(r);
        }
        if let Some(m) = config.mirror {
            entry.mirror = (!m.is_empty()).then_some(m);
        }
        self.apply_output_settings(&output);
        if self.mirrors.contains(&output) || entry_mirrors(&self.settings.display, &output) {
            self.apply_mirroring();
            save_settings(&self.settings);
            self.ipc.broadcast(Event::OutputsChanged { outputs: self.output_infos() });
            return Ok(());
        }
        self.space.map_output(&output, before.map(|g| g.loc).unwrap_or_default());
        if let (Some(old), Some(new)) = (before, self.space.output_geometry(&output)) {
            self.keep_neighbours(name, old, new);
        }
        let pointer = self.pointer.current_location();
        fixup_positions(&mut self.space, &self.settings.display, pointer);
        save_settings(&self.settings);
        self.lock_outputs_changed();
        self.ipc.broadcast(Event::OutputsChanged { outputs: self.output_infos() });
        Ok(())
    }

    // ---- settings ------------------------------------------------------

    pub fn set_settings(&mut self, mut settings: Settings) {
        // Brightness is kept as it's set (SetOutputBrightness, the keys),
        // not from a client's possibly older copy.
        for c in settings.display.outputs.values_mut() {
            c.brightness = None;
        }
        for (key, c) in &self.settings.display.outputs {
            if c.brightness.is_some() {
                settings.display.outputs.entry(key.clone()).or_default().brightness = c.brightness;
            }
        }
        let scale_changed = settings.display != self.settings.display;
        let main_changed = settings.display.main != self.settings.display.main;
        // Fields app_server owns are recomputed, not taken from the client.
        settings.appearance.effective_theme = self.settings.appearance.effective_theme;
        settings.appearance.night_light_active = self.settings.appearance.night_light_active;
        self.settings = settings;
        self.keymap.set_bindings(tabula_protocol::shortcuts::bindings(&self.settings.shortcuts));
        if self.workspace >= self.settings.workspace_count.max(1) {
            self.switch_workspace(self.settings.workspace_count.max(1) - 1);
        }
        if scale_changed {
            for o in self.space.outputs().cloned().collect::<Vec<_>>() {
                self.apply_output_settings(&o);
            }
            let pointer = self.pointer.current_location();
            fixup_positions(&mut self.space, &self.settings.display, pointer);
            self.lock_outputs_changed();
            if main_changed {
                self.follow_main_output();
            }
            self.apply_mirroring();
            self.ipc.broadcast(Event::OutputsChanged { outputs: self.output_infos() });
        }
        self.apply_input_settings();
        self.update_appearance(true);
        save_settings(&self.settings);
    }

    /// Resolve Auto theme and the night-light schedule against the clock.
    /// Broadcasts when something changed (or when `force`).
    pub fn update_appearance(&mut self, force: bool) {
        let a = &self.settings.appearance;
        let in_night = a.night_light.in_window(local_minutes());
        let theme = match a.theme {
            ThemePreference::Light => ThemeMode::Light,
            ThemePreference::Dark => ThemeMode::Dark,
            ThemePreference::Auto => {
                if in_night {
                    ThemeMode::Dark
                } else {
                    ThemeMode::Light
                }
            }
        };
        let night = match a.night_light.mode {
            NightLightMode::Off => false,
            NightLightMode::On => true,
            NightLightMode::Scheduled => in_night,
        };
        let changed = theme != a.effective_theme || night != a.night_light_active;
        self.settings.appearance.effective_theme = theme;
        self.settings.appearance.night_light_active = night;
        if (theme::set_mode(theme) || force) && B::IS_SESSION {
            crate::toolkits::publish(theme);
        }
        let kelvin = night.then_some(self.settings.appearance.night_light.temperature);
        if kelvin != self.night_light || self.white_point.is_none() {
            self.night_light = kelvin;
            self.set_night_light(kelvin);
        }
        if changed || force {
            self.ipc.broadcast(Event::SettingsChanged { settings: self.settings.clone() });
        }
    }

    /// Push touchpad and keyboard settings into libinput / xkb.
    pub fn apply_input_settings(&mut self) {
        let k = self.settings.keyboard.clone();
        if let Some(keyboard) = self.seat.get_keyboard() {
            let xkb = smithay::input::keyboard::XkbConfig {
                layout: &k.layout,
                variant: &k.variant,
                options: if k.options.is_empty() { None } else { Some(k.options.clone()) },
                ..Default::default()
            };
            if let Err(e) = keyboard.set_xkb_config(self, xkb) {
                warn!(?e, "bad keyboard layout");
            }
            keyboard.change_repeat_info(self.settings.keyboard.repeat_rate_hz, self.settings.keyboard.repeat_delay_ms);
        }
        self.backend_data.apply_input_settings(&self.settings);
    }
}

/// Preferences' rotation (the picture turned clockwise, as macOS labels
/// it) as a wl_output transform (which turns counter-clockwise).
pub fn rotation_transform(degrees: u16) -> smithay::utils::Transform {
    use smithay::utils::Transform;
    match degrees {
        90 => Transform::_270,
        180 => Transform::_180,
        270 => Transform::_90,
        _ => Transform::Normal,
    }
}
