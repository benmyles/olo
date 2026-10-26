//! Desktop-level state: window identity, workspaces, outputs and the
//! settings Preferences edits (theme, night mode, scaling, ...).

use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use smithay::{
    backend::renderer::Color32F,
    desktop::{WindowSurface, space::SpaceElement},
    output::{Output, Scale},
    reexports::wayland_server::Resource,
    utils::{IsAlive, Logical, Point},
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
    /// It was the focused window before an in-place restart.
    pub restored_focus: Cell<bool>,
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
            restored_focus: Cell::new(false),
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

fn save_settings(settings: &Settings) {
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
                }
            })
            .collect();
        infos.sort_by_key(|i| i.id);
        infos
    }

    /// Called once per loop iteration: drop dead windows and tell
    /// subscribers (Deskbar) when the window list changed.
    pub fn tick(&mut self) {
        self.parked.0.borrow_mut().retain(|w| w.alive());
        let infos = self.window_infos();
        if infos != self.last_window_infos {
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

    /// ⌘Tab (`apps`) or ⌘`: bring the next app, or the focused app's next
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

    /// ⌘ released: the chosen window stays in front.
    pub fn twitch_done(&mut self) {
        self.twitch = None;
    }

    /// Start one of our programs in the session, without waiting for it.
    pub fn spawn_program(&self, program: &str, args: &[&str]) {
        // Our programs are installed beside app_server; prefer those (so a
        // dev build starts dev apps), then PATH.
        let beside = std::env::current_exe().ok().and_then(|e| Some(e.parent()?.join(program))).filter(|p| p.is_file());
        let mut cmd = std::process::Command::new(beside.as_deref().unwrap_or(program.as_ref()));
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
        self.space
            .outputs()
            .map(|o| {
                let props = o.physical_properties();
                let to_mode = |m: smithay::output::Mode| tabula_protocol::Mode {
                    width: m.size.w,
                    height: m.size.h,
                    refresh: m.refresh,
                };
                let loc = self.space.output_geometry(o).map(|g| g.loc).unwrap_or_default();
                OutputInfo {
                    name: o.name(),
                    make: props.make,
                    model: props.model,
                    physical_size_mm: (props.size.w, props.size.h),
                    modes: o.modes().into_iter().map(to_mode).collect(),
                    current_mode: o.current_mode().map(to_mode),
                    scale: o.current_scale().fractional_scale(),
                    position: (loc.x, loc.y),
                }
            })
            .collect()
    }

    /// Apply the configured scale to a newly created output.
    pub fn apply_output_settings(&self, output: &Output) {
        let scale = self.settings.display.scale_for(&output.name());
        output.change_current_state(None, None, Some(Scale::Fractional(scale)), None);
    }

    pub fn configure_output(&mut self, name: &str, config: OutputConfig) -> Result<(), String> {
        let output = self
            .space
            .outputs()
            .find(|o| o.name() == name)
            .cloned()
            .ok_or_else(|| format!("no output named {name}"))?;
        if config.mode.is_some() {
            // Mode switching goes through the DRM backend; scale and position
            // are handled here for every backend.
            warn!("mode changes are not supported yet");
        }
        let entry = self.settings.display.outputs.entry(name.to_owned()).or_default();
        if let Some(scale) = config.scale {
            entry.scale = Some(scale.clamp(0.5, 4.0));
        }
        if let Some(pos) = config.position {
            entry.position = Some(pos);
        }
        self.apply_output_settings(&output);
        if let Some((x, y)) = config.position {
            self.space.map_output(&output, (x, y));
        }
        let pointer = self.pointer.current_location();
        fixup_positions(&mut self.space, pointer);
        save_settings(&self.settings);
        self.ipc.broadcast(Event::OutputsChanged { outputs: self.output_infos() });
        Ok(())
    }

    // ---- settings ------------------------------------------------------

    pub fn set_settings(&mut self, mut settings: Settings) {
        let scale_changed = settings.display != self.settings.display;
        // Fields app_server owns are recomputed, not taken from the client.
        settings.appearance.effective_theme = self.settings.appearance.effective_theme;
        settings.appearance.night_light_active = self.settings.appearance.night_light_active;
        self.settings = settings;
        if self.workspace >= self.settings.workspace_count.max(1) {
            self.switch_workspace(self.settings.workspace_count.max(1) - 1);
        }
        if scale_changed {
            for o in self.space.outputs().cloned().collect::<Vec<_>>() {
                self.apply_output_settings(&o);
            }
            let pointer = self.pointer.current_location();
            fixup_positions(&mut self.space, pointer);
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
            set_toolkit_color_scheme(theme);
        }
        self.night_light = night.then_some(self.settings.appearance.night_light.temperature);
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

/// GTK, Qt (via the portal) and Firefox follow this.
fn set_toolkit_color_scheme(theme: ThemeMode) {
    let scheme = match theme {
        ThemeMode::Light => "default",
        ThemeMode::Dark => "prefer-dark",
    };
    let gtk_theme = match theme {
        ThemeMode::Light => "Adwaita",
        ThemeMode::Dark => "Adwaita-dark",
    };
    std::thread::spawn(move || {
        let run = |args: &[&str]| {
            let _ = std::process::Command::new("gsettings")
                .args(args)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        };
        run(&["set", "org.gnome.desktop.interface", "color-scheme", scheme]);
        run(&["set", "org.gnome.desktop.interface", "gtk-theme", gtk_theme]);
    });
}
