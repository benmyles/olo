//! Desktop-level state: window identity, workspaces, outputs and the
//! settings Preferences edits (theme, night mode, scaling, ...).

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use smithay::{
    desktop::layer_map_for_output,
    backend::renderer::Color32F,
    desktop::space::SpaceElement,
    output::{Output, Scale},
    reexports::wayland_server::Resource,
    utils::{IsAlive, Logical, Point, Rectangle},
    wayland::seat::WaylandFocus,
};
use olo_protocol::{Event, NightLightMode, OutputConfig, OutputInfo, Settings, ThemePreference, WindowInfo};
use olo_theme::ThemeMode;
use tracing::{info, warn};

use crate::{
    shell::{WindowElement, fixup_positions},
    state::{AnvilState, Backend},
    theme,
};

/// A window that joined a group by itself on arriving (`sync_groups`):
/// its own frame and arrangement then, and the group's frame and
/// arrangement it took.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Joined {
    pub own: Rectangle<i32, Logical>,
    pub own_arrangement: crate::shell::tiling::Arrangement,
    pub group: Rectangle<i32, Logical>,
    pub worn: crate::shell::tiling::Arrangement,
}

impl Joined {
    /// Untouched since: still at the group's frame (`frame`, where it is
    /// now) and arranged as the group was (`arrangement`, its own now: not
    /// zoomed or tiled since, into a frame that happens to be the same).
    pub fn untouched(&self, frame: Option<Rectangle<i32, Logical>>, arrangement: crate::shell::tiling::Arrangement) -> bool {
        frame == Some(self.group) && arrangement == self.worn
    }
}

/// Per-window bookkeeping that isn't decoration.
pub struct WindowMeta {
    pub id: u64,
    /// `None` until first parked; then the workspace it lives on (of its
    /// display's: while it shows, the one its display shows).
    pub workspace: Cell<Option<u32>>,
    /// The display it was on when it left the space (its connector): it
    /// belongs to that display's workspace, and comes back there.
    pub parked_on: RefCell<Option<String>>,
    pub minimized: Cell<bool>,
    /// Where it was when it left the space (minimize / workspace switch).
    pub saved_location: Cell<Point<i32, Logical>>,
    /// Set on the first commit, once placement memory has been consulted.
    pub placed: Cell<bool>,
    /// It was the focused window before an in-place restart, put back at
    /// this time.
    pub restored_focus: Cell<Option<std::time::Instant>>,
    /// What had the keyboard as it opened, and the window active then
    /// (new windows take both): one back on its own
    /// (`layout::restore_placement`) gives them back.
    pub focus_before: std::cell::RefCell<(Option<crate::focus::KeyboardFocusTarget>, Option<crate::shell::WindowElement>)>,
    /// Its place in the stacking before an in-place restart (bottom
    /// first), put back at this time.
    pub restored_depth: Cell<Option<(u32, std::time::Instant)>>,
    /// The process and connection that made it (Wayland windows).
    pub owner: std::cell::RefCell<Option<(i32, smithay::reexports::wayland_server::backend::ClientId)>>,
    /// The remembered place it is coming back to, claimed on its first
    /// commit (so its first configure has the old size) and taken when it
    /// first shows.
    pub returning: std::cell::RefCell<Option<crate::layout::Placement>>,
    /// Put back where it was: left there at its first size, though it
    /// hangs off the screen, as long as its tab is on it.
    pub put_back: Cell<bool>,
    /// The saved place it was put back from (`Placement::slot`): its tile
    /// in a tree kept across the restart (0: none).
    pub slot: Cell<u64>,
    /// The id of the window it came back as (`Placement::id`; 0: none),
    /// by which others back on their own are stacked beside it.
    pub was: Cell<u64>,
    /// A move it asked for before it showed (a tab torn off a strip,
    /// moved from its first frame): started once it is in its place.
    pub pending_move: Cell<Option<smithay::utils::Serial>>,
    /// Being moved over another of its app's windows (`window_drop`):
    /// drawn see-through, so the place it would go into shows under it.
    pub ghost: Cell<bool>,
    /// The part of its display's tiles it was put back into, as saved
    /// (for making a tree again from where windows were, when no tree
    /// was kept).
    pub restored_part: Cell<Option<(f32, f32, f32, f32)>>,
    /// The displays there were when it left the space (by name): if they
    /// are just the same when it comes back, so is it; if its own moved,
    /// it moves along.
    pub left_among: std::cell::RefCell<Vec<(String, Rectangle<i32, Logical>)>>,
    /// Back in the space this turn, at this frame: if it joins its app's
    /// group now (`sync_groups`), that frame is its own to go back to.
    pub arrived: Cell<Option<Rectangle<i32, Logical>>>,
    /// It joined a group by itself on arriving: its own frame and
    /// arrangement then, and the group's it took. Back somewhere it isn't
    /// grouped, untouched (`Joined::untouched`), it has its own again.
    pub joined: Cell<Option<Joined>>,
    /// How it groups (`shell::groups`): with its app's others, on its own
    /// (taken out by hand) or in a group it was put in.
    pub grouping: RefCell<crate::shell::groups::Grouping>,
    /// The window it was last put in a group with by hand: with its frame
    /// off every display (straddling two, its middle on neither), it is
    /// on that one's display, not wherever its own size tips it.
    pub put_with: Cell<Option<u64>>,
    /// Where its tab goes among its group's (lowest first).
    pub tab_order: Cell<u64>,
    /// It would be a tile on a display in Focus mode (`shell::modes`):
    /// that display's number this turn.
    pub focus_on: Cell<Option<u32>>,
    /// Behind the window Focus shows: not drawn, not under the pointer.
    pub covered: Cell<bool>,
    /// Filling a display (`FullscreenSurface`): drawn by that display
    /// alone, not also where it was on the others (its buffer the
    /// display's size, it would spill onto them).
    pub fullscreen: Cell<bool>,
    /// Zoomed by Focus, and unzoomed when it leaves it.
    pub focus_zoomed: Cell<bool>,
    /// Glowing as a new window (Preferences ▸ Appearance): since when,
    /// for how long (`shell::glow`).
    pub glow: Cell<Option<(std::time::Instant, std::time::Duration)>>,
    /// Its glow's pieces, as drawn for its size and look.
    pub glow_pieces: RefCell<crate::shell::glow::Glow>,
    /// Moved, sized or sent elsewhere on purpose (by hand, a shortcut, an
    /// agent) since it was put back after a restart: not put back again
    /// (`layout::check_restored`).
    pub moved_on_purpose: Cell<bool>,
    /// Olo asked it to close (its close box, ⌘W, the window menu, an
    /// agent): when it goes, it is gone, not kept for its app starting
    /// again (`remember_placement`).
    pub closed_by_hand: Cell<bool>,
}

impl WindowMeta {
    /// How strong its glow is now (0: none; one that has faded is let go).
    pub fn glow_strength(&self) -> f32 {
        let Some((since, total)) = self.glow.get() else { return 0.0 };
        let s = crate::shell::glow::strength(since.elapsed(), total);
        if s <= 0.0 {
            self.glow.set(None);
            *self.glow_pieces.borrow_mut() = Default::default();
        }
        s
    }
}

impl WindowElement {
    pub fn meta(&self) -> &WindowMeta {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        self.user_data().insert_if_missing(|| WindowMeta {
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            workspace: Cell::new(None),
            parked_on: RefCell::new(None),
            minimized: Cell::new(false),
            saved_location: Cell::new(Point::default()),
            placed: Cell::new(false),
            restored_focus: Cell::new(None),
            focus_before: Default::default(),
            restored_depth: Cell::new(None),
            owner: std::cell::RefCell::new(None),
            returning: std::cell::RefCell::new(None),
            put_back: Cell::new(false),
            slot: Cell::new(0),
            was: Cell::new(0),
            pending_move: Cell::new(None),
            ghost: Cell::new(false),
            restored_part: Cell::new(None),
            left_among: Default::default(),
            arrived: Cell::new(None),
            joined: Cell::new(None),
            grouping: Default::default(),
            put_with: Cell::new(None),
            tab_order: Cell::new(crate::shell::groups::next_tab_order()),
            focus_on: Cell::new(None),
            covered: Cell::new(false),
            fullscreen: Cell::new(false),
            focus_zoomed: Cell::new(false),
            glow: Cell::new(None),
            glow_pieces: Default::default(),
            moved_on_purpose: Cell::new(false),
            closed_by_hand: Cell::new(false),
        });
        self.user_data().get::<WindowMeta>().unwrap()
    }

    pub fn id(&self) -> u64 {
        self.meta().id
    }
}

/// Twitcher's state while ⌘ is held: the candidates as they stood when
/// switching began, a row per workspace (front first), and which one is
/// chosen.
#[derive(Debug)]
pub struct Twitch {
    pub apps: bool,
    /// Every workspace's (⌘Tab, ⌘\`), not just this one's (⌥Tab).
    all: bool,
    /// (workspace, window ids), in workspace order, those on each display
    /// together when grouped by display.
    pub rows: Vec<(u32, Vec<u64>)>,
    /// The displays it groups windows by, if any (`twitch_displays`), and
    /// each window's place among them.
    displays: Vec<crate::twitcher::Display>,
    on: HashMap<u64, usize>,
    /// Tab's order: every cell (row, index), the most recently used
    /// first, so ⌘Tab goes back and forth between the last two apps
    /// wherever they are, and ⌥Tab between the last two here.
    order: Vec<(usize, usize)>,
    pub row: usize,
    pub index: usize,
    /// Where switching began, for Esc: each display's workspace showing.
    home: Vec<(Output, u32)>,
    focus: Option<WindowElement>,
    /// The stacking when Twitcher first showed each display's workspace
    /// (back first): each window passed over goes back into it, so only
    /// the one chosen ends up raised.
    stacks: Vec<((String, u32), Vec<WindowElement>)>,
    /// The minimized window being shown now, hidden again when passed.
    unhidden: Option<WindowElement>,
}

/// Twitcher's candidates: its rows, the displays it groups them by and
/// each window's display (`Twitch`'s fields of the same names).
type TwitchRows = (Vec<(u32, Vec<u64>)>, Vec<crate::twitcher::Display>, HashMap<u64, usize>);

/// Windows not currently in the space: minimized, or on another workspace.
/// Kept in stacking order (bottom first).
#[derive(Debug, Default)]
pub struct Parked(pub RefCell<Vec<WindowElement>>);

pub fn config_path() -> PathBuf {
    // An agent's screen keeps settings of its own (see apps/screen).
    if let Some(path) = std::env::var_os("OLO_APP_SERVER_SETTINGS") {
        return PathBuf::from(path);
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    base.join("olo").join("app_server.json")
}

pub fn load_settings() -> Settings {
    let mut settings = match std::fs::read_to_string(config_path()) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
            warn!(?e, "ignoring malformed settings");
            Settings::default()
        }),
        Err(_) => Settings::default(),
    };
    adopt_old_app_ids(&mut settings);
    settings.tiling.upgrade();
    settings.migrate();
    settings
}

/// Settings saved before the rename name Tabula's apps (`tabula.terminal`):
/// as Olo's, or Terminal stops being a terminal to the keymap (and ⌘V
/// no longer pastes in it).
fn adopt_old_app_ids(settings: &mut Settings) {
    let apps = &mut settings.keyboard.terminal_apps;
    let mut seen = Vec::with_capacity(apps.len());
    for id in apps.drain(..).map(|id| olo_xdg::legacy::new_app_id(&id)) {
        if !seen.contains(&id) {
            seen.push(id);
        }
    }
    *apps = seen;
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
fn entry_mirrors(display: &olo_protocol::DisplaySettings, output: &Output) -> bool {
    display.config(&display_key(output), &output.name()).is_some_and(|c| c.mirror.as_ref().is_some_and(|m| !m.is_empty()))
}

/// Where `output`'s settings are kept: its monitor, else its connector.
pub fn display_key(output: &Output) -> String {
    if let Some(Identity(told)) = output.user_data().get::<Identity>()
        && let Some((make, model, serial)) = told.borrow().as_ref()
    {
        return olo_protocol::display_key(&output.name(), make, model, serial);
    }
    let p = output.physical_properties();
    olo_protocol::display_key(&output.name(), &p.make, &p.model, &p.serial_number)
}

/// Who a display's monitor is, told after it was set up (its EDID came
/// late): make, model, serial (`display_identified`).
pub struct Identity(RefCell<Option<(String, String, String)>>);

/// Who a display's monitor is: make, model, serial, as told late or from
/// its EDID when it was set up.
pub fn display_identity(output: &Output) -> (String, String, String) {
    if let Some(Identity(told)) = output.user_data().get::<Identity>()
        && let Some(who) = told.borrow().as_ref()
    {
        return who.clone();
    }
    let p = output.physical_properties();
    (p.make, p.model, p.serial_number)
}

/// The order of a display's colour stripes as the kernel had it from the
/// display when it was set up (what wl_output says may be a guess).
pub struct DrmSubpixel(pub smithay::output::Subpixel);

pub fn to_panel_subpixel(s: smithay::output::Subpixel) -> olo_protocol::panels::Subpixel {
    use olo_protocol::panels::Subpixel as P;
    use smithay::output::Subpixel as S;
    match s {
        S::Unknown => P::Unknown,
        S::None => P::None,
        S::HorizontalRgb => P::Rgb,
        S::HorizontalBgr => P::Bgr,
        S::VerticalRgb => P::VerticalRgb,
        S::VerticalBgr => P::VerticalBgr,
    }
}

pub fn from_panel_subpixel(s: olo_protocol::panels::Subpixel) -> smithay::output::Subpixel {
    use olo_protocol::panels::Subpixel as P;
    use smithay::output::Subpixel as S;
    match s {
        P::Unknown => S::Unknown,
        P::None => S::None,
        P::Rgb => S::HorizontalRgb,
        P::Bgr => S::HorizontalBgr,
        P::VerticalRgb => S::VerticalRgb,
        P::VerticalBgr => S::VerticalBgr,
    }
}

/// What a display's panel is, from what it says, Olo's guesses and the
/// person's settings (`config`), and how its text is drawn, with why.
pub fn panel(
    connector: &str,
    make: &str,
    model: &str,
    size_mm: (i32, i32),
    mode_width: Option<i32>,
    refresh_mhz: i32,
    drm_subpixel: smithay::output::Subpixel,
    scale: f64,
    rotation: u16,
    config: Option<&olo_protocol::OutputConfig>,
) -> olo_protocol::panels::Panel {
    use olo_protocol::panels::{self, Source, Subpixel};
    let internal = crate::power::is_internal(connector);
    let ppi = mode_width.and_then(|w| panels::ppi(w, size_mm.0));
    let (technology, technology_from) = match config.and_then(|c| c.technology) {
        Some(t) => (t, Source::Setting),
        None => (panels::guess_technology(make, model, internal, ppi), Source::Guess),
    };
    let told = to_panel_subpixel(drm_subpixel);
    let (subpixel, subpixel_from) = match config.and_then(|c| c.subpixel) {
        Some(s) => (s, Source::Setting),
        None if told != Subpixel::Unknown => (told, Source::Display),
        None => (panels::guess_subpixel(technology), Source::Guess),
    };
    let (text, text_from, text_why) = match config.and_then(|c| c.text) {
        Some(t) => (t, Source::Setting, "Chosen in Preferences.".to_string()),
        None => {
            let (t, why) = panels::text_mode(ppi, scale, subpixel, rotation);
            (t, Source::Guess, why)
        }
    };
    panels::Panel {
        ppi,
        internal,
        technology,
        technology_from,
        subpixel,
        subpixel_from,
        refresh_hz: refresh_mhz as f32 / 1000.0,
        text: Some(text),
        text_from,
        text_why,
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

    /// The display `w` is on, or was on while hidden or on another
    /// workspace (where it comes back).
    pub(crate) fn display_of(&self, w: &WindowElement) -> Option<Output> {
        if self.space.element_location(w).is_some() {
            return self.window_output(w);
        }
        let parked_on = w.meta().parked_on.borrow().clone();
        if let Some(o) = parked_on.and_then(|name| self.space.outputs().find(|o| o.name() == name).cloned()) {
            return Some(o);
        }
        let size = smithay::desktop::space::SpaceElement::geometry(w).size;
        let middle = (w.meta().saved_location.get() + size.downscale(2).to_point()).to_f64();
        self.space.output_under(middle).next().cloned().or_else(|| self.main_output())
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
            .map(|w| {
                let client = w.wl_surface().and_then(|s| self.display_handle.get_client(s.id()).ok());
                let pid = client.as_ref().and_then(|c| crate::state::client_pid(c, &self.display_handle));
                let kept = olo_xdg::apps::our_app(&w.app_id()).is_some()
                    || client.and_then(|c| c.get_data::<crate::state::ClientState>().map(|s| s.proxied)).unwrap_or(false);
                let meta = w.meta();
                let place = self.groups.place(&w);
                WindowInfo {
                    id: meta.id,
                    app_id: w.app_id(),
                    title: w.title(),
                    pid,
                    workspace: self.workspace_of(&w),
                    minimized: meta.minimized.get(),
                    focused: focused.is_some() && w.wl_surface().map(|s| s.into_owned()) == focused,
                    chosen: self.twitch.as_ref().is_some_and(|t| t.rows.get(t.row).and_then(|r| r.1.get(t.index)) == Some(&meta.id)),
                    rolled: w.rolled(),
                    covered: meta.covered.get(),
                    zoomed: self.is_zoomed(&w),
                    zoomed_along: Self::zoomed_axis(&w).map(|a| match a {
                        crate::shell::tiling::Axis::Vertical => olo_protocol::ZoomAxis::Vertical,
                        crate::shell::tiling::Axis::Horizontal => olo_protocol::ZoomAxis::Horizontal,
                    }),
                    kept,
                    group: place.map(|(g, _)| g),
                    groupable: self.groupable(&w),
                    tiled: Self::tile_state(&w).0.is_some(),
                    part: match Self::tile_state(&w) {
                        (Some(t), part) => {
                            let (x, y, pw, ph) = part.unwrap_or(t.fraction());
                            Some([x, y, pw, ph])
                        }
                        (None, _) => None,
                    },
                    floating: self.floats(&w),
                    float: Self::float_override(&w),
                    output: self.display_of(&w).map(|o| o.name()),
                    tab: place.map(|(_, t)| t),
                    behind: self.behind_in_group(&w),
                    frame: None,
                    content: None,
                    least: None,
                    tab_frame: None,
                    popups: Vec::new(),
                    settling: false,
                    glowing: w.meta().glow_strength() > 0.0,
                    surface: w.wl_surface().map(|s| s.id().protocol_id()),
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
        let frame = |r: smithay::utils::Rectangle<i32, smithay::utils::Logical>| olo_protocol::Frame { x: r.loc.x, y: r.loc.y, width: r.size.w, height: r.size.h };
        for info in &mut infos {
            let Some(w) = self.space.elements().find(|w| w.id() == info.id) else { continue };
            let Some(at) = self.space.element_location(w) else { continue };
            let geo = smithay::desktop::space::SpaceElement::geometry(w);
            info.frame = Some(frame(smithay::utils::Rectangle::new(at, geo.size)));
            let least = crate::shell::groups::least_frame(w);
            info.least = Some([least.w, least.h]);
            info.settling = match w.0.underlying_surface() {
                smithay::desktop::WindowSurface::Wayland(t) => crate::shell::settling(t),
            };
            // As screenshots find it: where the element draws from, then
            // past the tab and border.
            info.content = Some(if w.decorated() {
                frame(smithay::utils::Rectangle::new(at - geo.loc + crate::shell::deco::content_offset(), w.content_size()))
            } else {
                frame(smithay::utils::Rectangle::new(at, geo.size))
            });
            if w.decorated() {
                let (title, content) = (w.title(), w.content_size());
                let tab = w.decoration().tab_layout(&title, content).rect();
                info.tab_frame = Some(frame(smithay::utils::Rectangle::new(at - geo.loc + tab.loc, tab.size)));
            }
            // Popups are placed from the client's geometry.
            if let Some(surface) = w.wl_surface() {
                let origin = at + w.content_offset();
                info.popups = smithay::desktop::PopupManager::popups_for_surface(&surface)
                    .map(|(popup, offset)| {
                        // A popup that set no geometry is its surfaces' size.
                        let size = Some(popup.geometry().size).filter(|s| s.w > 0 && s.h > 0);
                        let size = size.unwrap_or_else(|| smithay::desktop::utils::bbox_from_surface_tree(popup.wl_surface(), (0, 0)).size);
                        frame(smithay::utils::Rectangle::new(origin + offset, size))
                    })
                    .collect();
            }
        }
        infos
    }

    /// Called once per loop iteration: drop dead windows and tell
    /// subscribers (Deskbar) when the window list changed.
    pub fn tick(&mut self) {
        let t = std::time::Instant::now();
        self.parked.0.borrow_mut().retain(|w| w.alive());
        self.sync_groups();
        crate::render::part("tick: groups", t);
        let t = std::time::Instant::now();
        self.auto_tick();
        crate::render::part("tick: auto-tiling", t);
        let t = std::time::Instant::now();
        self.sync_held_sides();
        self.keep_children_above();
        self.focus_tick();
        self.note_recent();
        crate::render::part("tick: sides, children, focus", t);
        let t = std::time::Instant::now();
        // Another display active (a window there focused, a press there):
        // "this workspace" is another, for apps opening windows.
        let active = self.active_output().map(|o| o.name());
        if active != self.told_active {
            self.told_active = active;
            self.tell_workspaces();
        }
        let infos = self.window_infos();
        crate::render::part("tick: window infos", t);
        for (name, took) in crate::render::take_parts() {
            self.timings.add(name, took);
        }
        if infos != self.last_window_infos {
            self.backend_data.wake(None);
            crate::twitcher::prewarm(infos.iter().map(|i| i.app_id.as_str()));
            self.sync_toplevel_handles(&infos);
            self.last_window_infos = infos.clone();
            self.ipc.broadcast(Event::WindowsChanged { windows: infos });
        }
    }

    // ---- workspaces ----------------------------------------------------

    /// The workspace `output` shows (each display has its own).
    pub fn showing(&self, output: &Output) -> u32 {
        let last = self.settings.workspace_count.max(1) - 1;
        self.showing_raw(output).min(last)
    }

    /// As kept (one past the last, just after fewer were asked for).
    fn showing_raw(&self, output: &Output) -> u32 {
        let every = || self.showing.get(crate::layout::EVERY).copied();
        self.showing.get(&display_key(output)).copied().or_else(every).unwrap_or(0)
    }

    /// The workspace the display with connector `name` shows.
    pub fn showing_named(&self, name: &str) -> u32 {
        self.space.outputs().find(|o| o.name() == name).map_or(0, |o| self.showing(o))
    }

    /// The workspace the active display shows: the one shortcuts switch
    /// from, and windows opened from the keyboard land on.
    pub fn workspace(&self) -> u32 {
        self.active_output().map_or(0, |o| self.showing(&o))
    }

    /// The workspace `w` is on: while it shows, the one its display
    /// shows; else the one it was left on.
    pub fn workspace_of(&self, w: &WindowElement) -> u32 {
        let display = || self.display_of(w).map_or(0, |o| self.showing(&o));
        if self.space.element_location(w).is_some() {
            return display();
        }
        w.meta().workspace.get().unwrap_or_else(display)
    }

    /// `w` is on the workspace its display shows (if hidden, there).
    pub fn shows(&self, w: &WindowElement) -> bool {
        self.space.element_location(w).is_some() || self.display_of(w).is_some_and(|o| w.meta().workspace.get().is_none_or(|ws| ws == self.showing(&o)))
    }

    /// Each display's workspace showing, and which is the active one, as
    /// the IPC tells it.
    pub fn display_workspaces(&self) -> Vec<olo_protocol::DisplayWorkspace> {
        let active = self.active_output();
        self.space
            .outputs()
            .map(|o| olo_protocol::DisplayWorkspace { output: o.name(), current: self.showing(o), active: active.as_ref() == Some(o) })
            .collect()
    }

    /// The number display `output` goes by ("D1"), with more than one.
    pub fn display_number(&self, output: &str) -> Option<u32> {
        olo_protocol::display_number(&self.output_infos(), output)
    }

    fn park(&mut self, window: &WindowElement) {
        if self.space.element_location(window).is_some() {
            // The workspace its display shows, which it is on.
            window.meta().workspace.set(Some(self.workspace_of(window)));
            *window.meta().parked_on.borrow_mut() = self.window_output(window).map(|o| o.name());
        }
        if let Some(loc) = self.space.element_location(window) {
            window.meta().saved_location.set(loc);
        }
        window.set_activate(false);
        // Back, it is whatever Focus makes it then.
        window.meta().focus_on.set(None);
        window.meta().covered.set(false);
        *window.meta().left_among.borrow_mut() = self.display_frames();
        self.space.unmap_elem(window);
        self.parked.0.borrow_mut().push(window.clone());
    }

    pub fn display_frames(&self) -> Vec<(String, Rectangle<i32, Logical>)> {
        self.space.outputs().filter_map(|o| Some((o.name(), self.space.output_geometry(o)?))).collect()
    }

    fn unpark(&mut self, window: &WindowElement) {
        let ws = self.workspace_of(window);
        self.parked.0.borrow_mut().retain(|w| w != window);
        window.meta().minimized.set(false);
        window.meta().workspace.set(Some(ws));
        let mut loc = window.meta().saved_location.get();
        let (left, now) = (window.meta().left_among.borrow().clone(), self.display_frames());
        let changed = left != now;
        if changed {
            // Its display moved while it was away (as the windows on it
            // did, `fixup_positions`): along with it.
            let delta = self.display_moved(Rectangle::new(loc, window.geometry().size), &left);
            loc += delta;
            crate::shell::tiling::shift_remembered(window, delta);
        }
        self.space.map_element(window.clone(), loc, false);
        self.welcome_back(window, changed);
        // Out of the group it joined when it last arrived, untouched: its
        // own frame again (unless it joins one here too).
        // (Others of its group coming back here too, a workspace being
        // switched to, count: they are put back one by one.)
        let (key, display) = (self.group_key(window), self.display_of(window));
        let mate = |o: &WindowElement| o != window && key.is_some() && self.group_key(o) == key && self.display_of(o) == display;
        let alone = !self.space.elements().any(mate)
            && !self.parked.0.borrow().iter().any(|o| mate(o) && self.shows(o) && !o.meta().minimized.get());
        if let Some(j) = window.meta().joined.get()
            && alone
            && j.untouched(self.space.element_geometry(window), Self::arrangement_of(window))
        {
            window.meta().joined.set(None);
            self.set_frame(window, j.own, false);
            Self::rearrange(window, j.own_arrangement);
        }
        window.meta().arrived.set(self.space.element_geometry(window));
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
        self.overview_interrupted();
        // On another of its display's workspaces: the display goes there.
        if !self.shows(window)
            && let Some(o) = self.display_of(window)
        {
            self.switch_workspace_on(&o, self.workspace_of(window));
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

    /// Windows on the workspaces showing (each display's), front first,
    /// minimized ones last.
    fn workspace_windows(&self) -> Vec<WindowElement> {
        let mut v: Vec<WindowElement> = self.space.elements().rev().cloned().collect();
        v.extend(self.parked.0.borrow().iter().rev().filter(|w| w.meta().minimized.get() && self.shows(w)).cloned());
        v
    }

    /// Keep `recent` up to date: the focused window first (but not the
    /// ones Twitcher passes over while switching).
    fn note_recent(&mut self) {
        if self.twitch.is_some() {
            return;
        }
        let Some(id) = self.focused_window().map(|w| w.id()) else { return };
        if self.recent.first() != Some(&id) {
            self.recent.retain(|r| *r != id);
            self.recent.insert(0, id);
            let alive: Vec<u64> = self.recent.iter().copied().filter(|r| self.window_by_id(*r).is_some()).collect();
            self.recent = alive;
        }
    }

    /// How recently window `id` had the focus (0: most), never-focused
    /// ones after all the others.
    pub(crate) fn recency(&self, id: u64) -> usize {
        self.recent.iter().position(|r| *r == id).unwrap_or(usize::MAX)
    }

    /// Windows on workspace `ws` of any display, those showing first
    /// (front first), then those away, minimized ones last.
    fn windows_on_workspace(&self, ws: u32) -> Vec<WindowElement> {
        let mut v: Vec<WindowElement> = self.space.elements().rev().filter(|w| self.workspace_of(w) == ws).cloned().collect();
        let parked = self.parked.0.borrow();
        let on = |w: &&WindowElement| self.workspace_of(w) == ws;
        v.extend(parked.iter().rev().filter(on).filter(|w| !w.meta().minimized.get()).cloned());
        v.extend(parked.iter().rev().filter(on).filter(|w| w.meta().minimized.get()).cloned());
        v
    }

    /// The displays Twitcher groups windows by, as they stand left to
    /// right (then top to bottom), each with its name and the number it
    /// goes by ("D1 Studio Display"): each display has its own
    /// workspaces, as each Deskbar lists only its own display's. None
    /// with only one.
    fn twitch_displays(&self) -> Vec<(Output, String)> {
        let mut outputs: Vec<Output> = self.space.outputs().cloned().collect();
        if outputs.len() < 2 {
            return Vec::new();
        }
        outputs.sort_by_key(|o| self.space.output_geometry(o).map(|g| (g.loc.x, g.loc.y)).unwrap_or_default());
        let infos = self.output_infos();
        outputs
            .into_iter()
            .map(|o| {
                let info = infos.iter().find(|i| i.name == o.name());
                let name = info.map_or_else(|| o.name(), |i| i.display_name());
                let name = match olo_protocol::display_number(&infos, &o.name()) {
                    Some(n) => format!("D{n} {name}"),
                    None => name,
                };
                (o, name)
            })
            .collect()
    }

    /// Twitcher's rows: for each workspace number with any (`all`, else
    /// just those showing), its apps (one window each, the one used
    /// last) or, for ⌘\`, the focused app's windows, the most recently
    /// used first.
    /// Grouped by display
    /// (`twitch_displays`), each display's come together, in display
    /// order, and an app is there once for each display it has windows on;
    /// then also which displays have any, and each window's among them.
    fn twitch_rows(&self, apps: bool, all: bool) -> TwitchRows {
        let app = self.focused_window().map(|w| w.app_id());
        let displays = self.twitch_displays();
        let column = |w: &WindowElement| {
            let o = self.display_of(w);
            displays.iter().position(|(d, _)| Some(d) == o.as_ref()).unwrap_or(0)
        };
        let count = self.settings.workspace_count.max(1);
        let showing: Vec<WindowElement> = if all { Vec::new() } else { self.workspace_windows() };
        let (mut rows, mut on) = (Vec::new(), HashMap::new());
        for ws in 0..count {
            let windows: Vec<WindowElement> = if all {
                self.windows_on_workspace(ws)
            } else {
                showing.iter().filter(|w| self.workspace_of(w) == ws).cloned().collect()
            };
            let mut windows: Vec<(usize, WindowElement)> = windows.into_iter().map(|w| (column(&w), w)).collect();
            windows.sort_by_key(|(c, w)| (*c, self.recency(w.id())));
            let mut seen = Vec::new();
            let mut ids = Vec::new();
            for (c, w) in windows {
                let a = w.app_id();
                let wanted = if apps { !seen.contains(&(c, a.clone())) } else { Some(&a) == app.as_ref() };
                if wanted {
                    seen.push((c, a));
                    on.insert(w.id(), c);
                    ids.push(w.id());
                }
            }
            if !ids.is_empty() {
                rows.push((ws, ids));
            }
        }
        // Only the displays with any, still in order.
        let used: Vec<usize> = (0..displays.len()).filter(|d| on.values().any(|c| c == d)).collect();
        for c in on.values_mut() {
            *c = used.iter().position(|u| u == c).unwrap_or(0);
        }
        let displays = used.iter().map(|&d| crate::twitcher::Display { output: displays[d].0.name(), name: displays[d].1.clone() }).collect();
        (rows, displays, on)
    }

    /// ⌘Tab (`apps` and `all`) or ⌥Tab (`apps`, this workspace's), or
    /// ⌘\` (`all`): bring the next app, or the focused app's next window,
    /// to the front, on whichever workspace it is, the most recently used
    /// first. The order is fixed while the key is held, so repeated
    /// presses walk it; the arrows move about the rows (`twitch_move`).
    pub fn twitch_step(&mut self, apps: bool, all: bool, back: bool) {
        if self.twitch.as_ref().is_none_or(|t| (t.apps, t.all) != (apps, all)) {
            let (rows, displays, on) = self.twitch_rows(apps, all);
            let mut order: Vec<(usize, usize)> =
                rows.iter().enumerate().flat_map(|(r, row)| (0..row.1.len()).map(move |i| (r, i))).collect();
            if order.len() < 2 {
                return;
            }
            order.sort_by_key(|&(r, i)| self.recency(rows[r].1[i]));
            // Start on the focused window's cell, so the first press moves
            // to the one used before it; with none, on the last, so it
            // moves to the first.
            let focus = self.focused_window();
            let focused = focus.as_ref().map(|w| (w.id(), w.app_id()));
            let start = order
                .iter()
                .position(|&(r, i)| {
                    let id = rows[r].1[i];
                    self.window_by_id(id).is_some_and(|w| {
                        self.shows(&w) && focused.as_ref().is_some_and(|(fid, fapp)| id == *fid || (apps && w.app_id() == *fapp))
                    })
                })
                .unwrap_or(order.len() - 1);
            let (row, index) = order[start];
            let stack: Vec<WindowElement> = self.space.elements().cloned().collect();
            let home: Vec<(Output, u32)> = self.space.outputs().map(|o| (o.clone(), self.showing(o))).collect();
            let stacks = home.iter().map(|(o, ws)| ((o.name(), *ws), stack.clone())).collect();
            self.twitch = Some(Twitch { apps, all, rows, displays, on, order, row, index, home, focus, stacks, unhidden: None });
        }
        let Some(t) = self.twitch.as_mut() else { return };
        let n = t.order.len();
        let at = t.order.iter().position(|&c| c == (t.row, t.index)).unwrap_or(0);
        (t.row, t.index) = t.order[if back { (at + n - 1) % n } else { (at + 1) % n }];
        self.twitch_show();
    }

    /// ←/→ (`dx`) along the chosen row, ↑/↓ (`dy`) to the row of the
    /// workspace above or below, starting at its front-most (on the same
    /// display, grouped by display, if it has any there).
    pub fn twitch_move(&mut self, dx: i32, dy: i32) {
        let Some(t) = self.twitch.as_mut() else { return };
        if dy != 0 {
            let row = (t.row as i32 + dy).clamp(0, t.rows.len() as i32 - 1) as usize;
            if row == t.row {
                return;
            }
            let on = |id: &u64| t.on.get(id).copied().unwrap_or(0);
            let here = on(&t.rows[t.row].1[t.index]);
            (t.row, t.index) = (row, t.rows[row].1.iter().position(|id| on(id) == here).unwrap_or(0));
        } else {
            let n = t.rows[t.row].1.len() as i32;
            t.index = (t.index as i32 + dx).rem_euclid(n) as usize;
        }
        self.twitch_show();
    }

    /// A letter typed with keyboard hints on: choose the cell it marks.
    pub fn twitch_pick(&mut self, c: char) {
        let Some((row, index)) = self.twitcher_entries().and_then(|s| s.find(c)) else { return };
        if let Some(t) = self.twitch.as_mut() {
            (t.row, t.index) = (row, index);
        }
        self.twitch_show();
        self.twitch_done();
    }

    /// Put the windows of the workspaces showing back as they were
    /// stacked when Twitcher first showed them, and hide again a
    /// minimized one shown.
    fn twitch_restack(&mut self) {
        let Some(t) = self.twitch.as_mut() else { return };
        let unhidden = t.unhidden.take();
        let showing: Vec<(String, u32)> = self.space.outputs().map(|o| (o.name(), self.showing(o))).collect();
        let Some(t) = self.twitch.as_ref() else { return };
        let stacks: Vec<Vec<WindowElement>> = t.stacks.iter().filter(|s| showing.contains(&s.0)).map(|s| s.1.clone()).collect();
        if let Some(u) = unhidden {
            self.minimize(&u);
        }
        for stack in &stacks {
            for s in stack {
                if self.space.element_location(s).is_some() {
                    self.space.raise_element(s, false);
                }
            }
        }
    }

    /// Bring the chosen window forward, on its workspace, with everything
    /// passed over back in its place.
    fn twitch_show(&mut self) {
        let Some(t) = self.twitch.as_ref() else { return };
        let (ws, id) = (t.rows[t.row].0, t.rows[t.row].1[t.index]);
        let Some(w) = self.window_by_id(id) else { return };
        self.twitch_restack();
        if !self.shows(&w)
            && let Some(o) = self.display_of(&w)
        {
            self.switch_workspace_on(&o, ws);
            let stack = self.space.elements().cloned().collect();
            let key = (o.name(), ws);
            if let Some(t) = self.twitch.as_mut().filter(|t| !t.stacks.iter().any(|s| s.0 == key)) {
                t.stacks.push((key, stack));
            }
            self.twitch_restack();
        }
        let was_hidden = w.meta().minimized.get();
        // An app comes forward with all its windows here, in their order,
        // as BeOS's roster activates an app; the chosen one in front.
        let app = (self.twitch.as_ref().is_some_and(|t| t.apps)).then(|| w.app_id());
        let mates: Vec<WindowElement> =
            self.space.elements().filter(|x| app.as_ref().is_some_and(|a| x.app_id() == *a)).cloned().collect();
        for m in &mates {
            self.space.raise_element(m, false);
        }
        self.restore(&w);
        crate::spotlight::show(self.spotlit(&w, app.is_some()));
        if let Some(t) = self.twitch.as_mut() {
            t.unhidden = was_hidden.then_some(w);
        }
    }

    /// What the spotlight keeps bright while `chosen` is chosen: its app's
    /// windows here (`app`) or it alone, and whatever they hold in front
    /// of them (dialogs, Quick Look).
    fn spotlit(&self, chosen: &WindowElement, app: bool) -> std::collections::HashSet<u64> {
        let windows: Vec<&WindowElement> = self.space.elements().collect();
        let mut bright: std::collections::HashSet<u64> =
            windows.iter().filter(|w| w.id() == chosen.id() || (app && w.app_id() == chosen.app_id())).map(|w| w.id()).collect();
        loop {
            let parents: Vec<_> = windows.iter().filter(|w| bright.contains(&w.id())).filter_map(|w| w.wl_surface()).collect();
            let children = windows.iter().filter(|w| {
                !bright.contains(&w.id())
                    && w.0.toplevel().and_then(|t| t.parent()).is_some_and(|p| parents.iter().any(|s| **s == p))
            });
            let more: Vec<u64> = children.map(|w| w.id()).collect();
            if more.is_empty() {
                return bright;
            }
            bright.extend(more);
        }
    }

    /// What Twitcher shows.
    pub fn twitcher_entries(&self) -> Option<crate::twitcher::Shown> {
        let t = self.twitch.as_ref()?;
        let mut rows = Vec::new();
        for (ws, ids) in &t.rows {
            let entries: Vec<_> = ids
                .iter()
                .filter_map(|id| self.window_by_id(*id))
                .map(|w| crate::twitcher::Entry {
                    app_id: w.app_id(),
                    title: (!t.apps).then(|| w.title()),
                    display: t.on.get(&w.id()).copied().unwrap_or(0),
                })
                .collect();
            if entries.len() != ids.len() {
                return None;
            }
            rows.push(crate::twitcher::Row { workspace: *ws, entries });
        }
        let (displays, letters) = (t.displays.clone(), self.settings.keyboard.hints);
        Some(crate::twitcher::Shown { rows, displays, row: t.row, index: t.index, letters })
    }

    /// The Twitcher cell under `pos` (global logical), in the copy of the
    /// panel on the display there.
    fn twitch_cell_at(&self, pos: Point<f64, Logical>) -> Option<(usize, usize)> {
        let shown = self.twitcher_entries()?;
        let output = self.space.output_under(pos).next()?;
        let geo = self.space.output_geometry(output)?;
        crate::twitcher::cell_at(&shown, (geo.size.w, geo.size.h), (pos.x - geo.loc.x as f64, pos.y - geo.loc.y as f64))
    }

    /// The pointer over a Twitcher cell while ⌘ (or ⌥) is held: that one
    /// is chosen, as ⇥ and the arrows choose.
    pub fn twitch_pointer(&mut self) {
        let Some(cell) = self.twitch_cell_at(self.pointer.current_location()) else { return };
        let Some(t) = self.twitch.as_mut() else { return };
        if (t.row, t.index) != cell {
            (t.row, t.index) = cell;
            self.twitch_show();
        }
    }

    /// A click while switching: on a cell, that one is chosen, as
    /// letting go of ⌘ chooses (and the click's release goes nowhere).
    /// True if it took the button.
    pub fn twitch_button(&mut self, button: u32, pressed: bool) -> bool {
        if !pressed {
            let Some(i) = self.twitch_buttons.iter().position(|b| *b == button) else { return false };
            self.twitch_buttons.swap_remove(i);
            return true;
        }
        let Some(cell) = self.twitch.as_ref().and_then(|_| self.twitch_cell_at(self.pointer.current_location())) else { return false };
        if let Some(t) = self.twitch.as_mut() {
            (t.row, t.index) = cell;
        }
        self.twitch_show();
        self.twitch_done();
        self.twitch_buttons.push(button);
        true
    }

    /// ⌘ released: the chosen window stays in front, the rest where they
    /// were (`twitch_step` put each back as it passed it).
    pub fn twitch_done(&mut self) {
        self.twitch = None;
        self.twitcher.forget();
        crate::spotlight::end(true);
    }

    /// Esc while switching: back to the workspace and window switching
    /// began on, everything as it was.
    pub fn twitch_cancel(&mut self) {
        let Some(home) = self.twitch.as_ref().map(|t| t.home.clone()) else { return };
        crate::spotlight::end(false);
        self.twitch_restack();
        let away: Vec<(Output, u32)> = home.into_iter().filter(|(o, ws)| self.space.output_geometry(o).is_some() && self.showing(o) != *ws).collect();
        for (o, ws) in &away {
            self.switch_workspace_on(o, *ws);
        }
        if !away.is_empty() {
            self.twitch_restack();
        }
        if let Some(f) = self.twitch.take().and_then(|t| t.focus).filter(|f| self.space.element_location(f).is_some()) {
            self.focus_window(&f);
        }
        self.twitcher.forget();
    }

    /// Start one of our programs in the session, without waiting for it.
    pub fn spawn_program(&self, program: &str, args: &[&str]) {
        // Our programs are installed beside app_server; prefer those (so a
        // dev build starts dev apps), then PATH.
        let Some(path) = olo_xdg::find_program(program) else {
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

    /// Show workspace `index` on the active display.
    pub fn switch_workspace(&mut self, index: u32) {
        if let Some(o) = self.active_output() {
            self.switch_workspace_on(&o, index);
        }
    }

    /// Show workspace `index` on `output`: its windows there go, that
    /// workspace's come; the other displays stay as they are.
    pub fn switch_workspace_on(&mut self, output: &Output, index: u32) {
        let count = self.settings.workspace_count.max(1);
        let index = index.min(count - 1);
        self.last_display = Some(output.name());
        // (As kept: after fewer workspaces were asked for, one past the
        // last, which `showing` reads as the last.)
        if self.showing_raw(output) == index {
            return;
        }
        self.overview_interrupted();
        // Park everything on it, bottom-most first to keep stacking order.
        let leaving: Vec<WindowElement> = self.space.elements().filter(|w| self.window_output(w).as_ref() == Some(output)).cloned().collect();
        let had_focus = self.focused_window().is_none_or(|f| leaving.contains(&f));
        for w in &leaving {
            self.park(w);
        }
        self.showing.insert(display_key(output), index);
        let coming: Vec<WindowElement> = self
            .parked
            .0
            .borrow()
            .iter()
            .filter(|w| w.meta().workspace.get() == Some(index) && !w.meta().minimized.get() && self.display_of(w).as_ref() == Some(output))
            .cloned()
            .collect();
        for w in &coming {
            self.unpark(w);
        }
        // The keyboard goes to the front window there (none if it has
        // none), not to another display's.
        if had_focus {
            self.focus_topmost_on(output);
        }
        self.tell_workspaces();
        info!(workspace = index, output = output.name(), "switched workspace");
    }

    /// Tell subscribers each display's workspace showing, and which is
    /// active.
    pub fn tell_workspaces(&mut self) {
        self.backend_data.wake(None);
        self.told_active = self.active_output().map(|o| o.name());
        let (current, count, displays) = (self.workspace(), self.settings.workspace_count.max(1), self.display_workspaces());
        self.ipc.broadcast(Event::WorkspaceChanged { current, count, displays });
    }

    /// The front window on `output` takes the keyboard; with none there,
    /// nothing has it.
    /// Go to display `o` (Deskbar's display menu): the pointer to the
    /// middle of where windows go on it, the keyboard to its front window
    /// (none: its desktop), so it is the active display.
    pub fn focus_display(&mut self, o: &Output) {
        let Some(area) = self.usable_area(o).or_else(|| self.space.output_geometry(o)) else { return };
        let middle = Point::<f64, Logical>::from((area.loc.x as f64 + area.size.w as f64 / 2.0, area.loc.y as f64 + area.size.h as f64 / 2.0));
        self.warp_pointer(middle);
        self.focus_topmost_on(o);
        info!(display = o.name(), "went to a display");
    }

    fn focus_topmost_on(&mut self, output: &Output) {
        let front = self.space.elements().rev().find(|w| self.window_output(w).as_ref() == Some(output)).cloned();
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

    /// Move `window` to workspace `index` of its display.
    pub fn move_to_workspace(&mut self, window: &WindowElement, index: u32) {
        let index = index.min(self.settings.workspace_count.max(1) - 1);
        let output = self.display_of(window);
        if output.as_ref().is_some_and(|o| index == self.showing(o)) {
            // One on another workspace comes here, to the front (a
            // minimized one stays so, here).
            let away = self.parked.0.borrow().contains(window) && !window.meta().minimized.get();
            window.meta().workspace.set(Some(index));
            if away {
                self.unpark(window);
                self.focus_window(window);
            }
            return;
        }
        if self.space.element_location(window).is_some() {
            let had_focus = self.focused_window().as_ref() == Some(window);
            self.park(window);
            if let (true, Some(o)) = (had_focus, &output) {
                self.focus_topmost_on(o);
            }
        }
        window.meta().workspace.set(Some(index));
    }

    /// Move every window on `from`'s workspace `index` to `to`'s workspace
    /// `to_index` (Deskbar's workspace headings): on another display, each
    /// to the same place on it relatively, tiled or zoomed as it was (as
    /// ⌃⌥⌘→ moves one). Hidden ones stay hidden; the rest show if `to`
    /// shows that workspace.
    pub fn move_workspace(&mut self, from: &Output, index: u32, to: &Output, to_index: u32) {
        let to_index = to_index.min(self.settings.workspace_count.max(1) - 1);
        if from == to && index == to_index {
            return;
        }
        let windows: Vec<WindowElement> =
            self.all_windows().into_iter().filter(|w| self.display_of(w).as_ref() == Some(from) && self.workspace_of(w) == index).collect();
        if windows.is_empty() {
            return;
        }
        self.move_windows(&windows, from, to, to_index);
        info!(workspace = index, from = from.name(), to = to.name(), to_index, windows = windows.len(), "moved a workspace's windows");
    }

    /// Move `window` to workspace `index` of display `to` (the window tab
    /// menu's, and Deskbar's app menus): on its own display as
    /// `move_to_workspace`; on another, to the same place there
    /// relatively, tiled or zoomed as it was, hidden if it was, in front
    /// if `to` shows that workspace.
    pub fn move_to_display_workspace(&mut self, window: &WindowElement, to: &Output, index: u32) {
        let Some(from) = self.display_of(window) else { return };
        if &from == to {
            self.move_to_workspace(window, index);
            return;
        }
        let index = index.min(self.settings.workspace_count.max(1) - 1);
        self.move_windows(std::slice::from_ref(window), &from, to, index);
        info!(window = window.id(), from = from.name(), to = to.name(), index, "moved to another display's workspace");
    }

    /// Move `windows`, all on display `from`, to `to`'s workspace
    /// `to_index`, bottom first so they stack there as they did.
    fn move_windows(&mut self, windows: &[WindowElement], from: &Output, to: &Output, to_index: u32) {
        self.overview_interrupted();
        let shown_there = self.showing(to) == to_index;
        let focused = self.focused_window();
        let mut front = None;
        // Bottom first (the space's, then the parked), so they stack there
        // as they did.
        for w in windows {
            let minimized = w.meta().minimized.get();
            if from != to {
                if self.parked.0.borrow().contains(w) {
                    // Into the space where it was, to go as a window
                    // showing goes (nothing is drawn before it is put
                    // away again).
                    self.parked.0.borrow_mut().retain(|p| p != w);
                    self.space.map_element(w.clone(), w.meta().saved_location.get(), false);
                }
                self.move_to_output(w, from, to);
            }
            let mapped = self.space.element_location(w).is_some();
            w.meta().workspace.set(Some(to_index));
            if shown_there && !minimized {
                if !mapped {
                    self.unpark(w);
                }
                front = Some(w.clone());
            } else {
                if mapped {
                    self.park(w);
                    w.meta().workspace.set(Some(to_index));
                }
                w.meta().minimized.set(minimized);
            }
        }
        match front {
            Some(w) if focused.as_ref().is_none_or(|f| windows.contains(f)) => self.focus_window(&w),
            None if focused.as_ref().is_some_and(|f| windows.contains(f)) => self.focus_topmost_on(from),
            _ => {}
        }
    }

    /// `output`'s desktop colour: that of the workspace it shows.
    pub fn desktop_color(&self, output: &Output) -> Color32F {
        let [r, g, b] = self.settings.workspace_color(self.showing(output));
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
                let (make, model, _) = display_identity(o);
                let key = display_key(o);
                let rotation = self.settings.display.rotation_for(&key, &o.name());
                let current = o.current_mode();
                let drm_subpixel = o.user_data().get::<DrmSubpixel>().map_or(props.subpixel, |d| d.0);
                let config = self.settings.display.config(&key, &o.name());
                let panel = panel(
                    &o.name(),
                    &make,
                    &model,
                    (props.size.w, props.size.h),
                    current.map(|m| m.size.w),
                    current.map_or(0, |m| m.refresh),
                    drm_subpixel,
                    o.current_scale().fractional_scale(),
                    rotation,
                    config.as_ref(),
                );
                let to_mode = |m: smithay::output::Mode| olo_protocol::Mode {
                    width: m.size.w,
                    height: m.size.h,
                    refresh: m.refresh,
                };
                let source = crate::render::mirror_source(o);
                let mirror = source.is_some();
                let placed = source.as_ref().unwrap_or(o);
                let loc = self.space.output_geometry(placed).map(|g| g.loc).unwrap_or_default();
                OutputInfo {
                    name: o.name(),
                    key,
                    make,
                    model,
                    physical_size_mm: (props.size.w, props.size.h),
                    modes: o.modes().into_iter().map(to_mode).collect(),
                    current_mode: o.current_mode().map(to_mode),
                    scale: o.current_scale().fractional_scale(),
                    position: (loc.x, loc.y),
                    rotation,
                    main: main.as_ref() == Some(o),
                    mirror_of: source.map(|s| s.name()),
                    brightness: if crate::power::is_internal(&o.name()) { self.power.brightness } else { self.power.display_brightness(&o.name()) },
                    number: (!mirror).then(|| self.settings.display.numbers.get(&display_key(o)).copied()).flatten(),
                    panel: Some(panel),
                }
            })
            .collect()
    }

    /// `output`'s monitor told who it is only after the display was set up
    /// by its connector (its EDID came late): from now on its settings,
    /// its place among the displays and its workspace are its monitor's,
    /// its windows going with it.
    pub fn display_identified(&mut self, output: &Output, make: String, model: String, serial: String) {
        let was = self.showing_raw(output);
        output.user_data().insert_if_missing(|| Identity(RefCell::new(None)));
        if let Some(Identity(told)) = output.user_data().get::<Identity>() {
            *told.borrow_mut() = Some((make, model, serial));
        }
        let key = display_key(output);
        info!(output = output.name(), key, "the display told who it is: its settings now");
        // Its number, if it had one only by its connector meanwhile.
        let numbers = &mut self.settings.display.numbers;
        if !numbers.contains_key(&key)
            && let Some(n) = numbers.remove(&output.name())
        {
            numbers.insert(key.clone(), n);
        }
        // Its saved workspace, switched to from the one it showed.
        let saved = self.showing.insert(key, was);
        self.apply_output_settings(output);
        let pointer = self.pointer.current_location();
        fixup_positions(&mut self.space, &self.settings.display, pointer);
        self.rehome_windows();
        if let Some(ws) = saved.filter(|ws| *ws != was) {
            self.switch_workspace_on(output, ws);
        }
        self.tell_workspaces();
        self.ipc.broadcast(Event::OutputsChanged { outputs: self.output_infos() });
    }

    /// Something of app_server's own is moving (a scroll being paced or
    /// gliding on, a window dragged or resized, the overview, Twitcher, the
    /// keyboard hints, the spotlight, the level indicator, a window's glow
    /// fading): the displays don't rest meanwhile, as a frame of it may
    /// change nothing to see and the next must still come at once.
    pub fn keeps_awake(&self) -> bool {
        self.pacer.busy()
            || self.pointer.is_grabbed()
            || self.overview.is_some()
            || self.twitch.is_some()
            || self.hints.active()
            || crate::spotlight::active()
            || self.space.outputs().any(crate::osd::showing)
            || self.space.elements().any(|w| (0.0..1.0).contains(&w.meta().glow_strength()) && w.meta().glow.get().is_some())
    }

    /// Give each display found a number of its own if it has none
    /// (`DisplaySettings::numbers`): the first ones left to right, later
    /// ones the smallest free. Kept, so a display's number doesn't change
    /// as others come, go or are moved about.
    pub fn number_displays(&mut self) {
        let mut shown: Vec<(Point<i32, Logical>, String)> =
            self.space.outputs().filter_map(|o| Some((self.space.output_geometry(o)?.loc, display_key(o)))).collect();
        shown.sort_by_key(|(at, _)| (at.x, at.y));
        let numbers = &mut self.settings.display.numbers;
        let mut changed = false;
        for (_, key) in shown {
            if !numbers.contains_key(&key) {
                let free = (0..).find(|n| !numbers.values().any(|v| v == n)).unwrap_or(0);
                numbers.insert(key, free);
                changed = true;
            }
        }
        if changed {
            save_settings(&self.settings);
        }
    }

    /// Apply the configured scale and rotation to an output.
    pub fn apply_output_settings(&self, output: &Output) {
        let (key, name) = (display_key(output), output.name());
        let scale = self.settings.display.scale_for(&key, &name);
        let transform = rotation_transform(self.settings.display.rotation_for(&key, &name));
        output.change_current_state(None, Some(transform), Some(Scale::Fractional(scale)), None);
        self.tell_scale(output);
    }

    /// Tells the windows on `output` its scale, now. Drawing a surface
    /// tells it, so a covered window (none of it drawn) went on drawing
    /// at the old scale, and its first frame uncovered was resampled.
    fn tell_scale(&self, output: &Output) {
        use smithay::desktop::utils::surface_primary_scanout_output;
        use smithay::wayland::fractional_scale::with_fractional_scale;
        let scale = output.current_scale().fractional_scale();
        let tell = |surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
                    states: &smithay::wayland::compositor::SurfaceData| {
            if surface_primary_scanout_output(surface, states).is_none_or(|o| o == *output) {
                with_fractional_scale(states, |f| f.set_preferred_scale(scale));
            }
        };
        for window in self.space.elements().filter(|w| self.space.outputs_for_element(w).first() == Some(output)) {
            window.with_surfaces(tell);
        }
        for layer in layer_map_for_output(output).layers() {
            layer.with_surfaces(tell);
        }
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

    /// `name` changed from `old` to `new`: if it changed size, screens
    /// placed against its right or bottom edge move with that edge, so the
    /// arrangement keeps touching (a gap would trap the pointer); the rest
    /// keep their places.
    fn keep_neighbours(&mut self, name: &str, old: Rectangle<i32, Logical>, new: Rectangle<i32, Logical>) {
        let (dw, dh) = (new.size.w - old.size.w, new.size.h - old.size.h);
        let moves: Vec<(Output, (i32, i32))> = self
            .space
            .outputs()
            .filter(|o| o.name() != name && (dw, dh) != (0, 0))
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
            self.settings.display.config_mut(&display_key(&other), &other.name()).position = Some(at);
        }
        // Pin everything that is placed, so the row fallback can't
        // reshuffle (a display moved, not resized, too: the others stay).
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
        if let Some(r) = config.rotation.filter(|r| olo_protocol::ROTATIONS.contains(r)) {
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
        // Tiled and zoomed windows fitted to its new size, the rest kept
        // in reach on it.
        self.rehome_windows();
        save_settings(&self.settings);
        self.lock_outputs_changed();
        self.ipc.broadcast(Event::OutputsChanged { outputs: self.output_infos() });
        Ok(())
    }

    // ---- settings ------------------------------------------------------

    pub fn set_settings(&mut self, mut settings: Settings) {
        // A backup restored from before keyboard profiles.
        settings.migrate();
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
        let space_changed = (settings.tiling.gap, settings.tiling.margin) != (self.settings.tiling.gap, self.settings.tiling.margin);
        // A keymap takes milliseconds to build: only when they changed.
        let input_changed = settings.keyboard != self.settings.keyboard || settings.touchpad != self.settings.touchpad;
        if settings.security.passwords.op_cli != self.settings.security.passwords.op_cli {
            olo_xdg::link_op_here(settings.security.passwords.op_cli);
        }
        // Fields app_server owns are recomputed, not taken from the client.
        settings.appearance.effective_theme = self.settings.appearance.effective_theme;
        settings.appearance.night_light_active = self.settings.appearance.night_light_active;
        // Tabs are laid out and drawn again (their cache keys on the cut title).
        crate::shell::deco::set_title_chars(settings.tab_title_chars);
        // Twitcher looks its icons up again, in the new theme.
        if olo_xdg::themes::set(&settings.appearance.icon_theme) {
            crate::twitcher::theme_changed();
        }
        // Windowing modes are kept as they are chosen, not from a client's
        // copy (which may be older).
        settings.tiling.modes = self.settings.tiling.modes.clone();
        settings.tiling.auto.clear();
        if settings.tiling.default_mode != self.settings.tiling.default_mode {
            self.auto.changed();
        }
        // Group Windows on again for an app: its windows taken out by hand
        // go back in too.
        let regrouped: Vec<String> =
            self.settings.tiling.ungrouped_apps.iter().filter(|a| !settings.tiling.ungrouped_apps.contains(a)).cloned().collect();
        self.settings = settings;
        if !regrouped.is_empty() {
            self.regroup_apps(&regrouped);
        }
        self.keymap.set_bindings(olo_protocol::shortcuts::bindings(&self.settings.keyboard.profile));
        // Fewer workspaces: the windows on those gone join the last, and
        // a display showing one goes to it (with its windows).
        let last = self.settings.workspace_count.max(1) - 1;
        for w in self.parked.0.borrow().iter().filter(|w| w.meta().workspace.get().is_some_and(|ws| ws > last)) {
            w.meta().workspace.set(Some(last));
        }
        let past: Vec<Output> = self.space.outputs().filter(|o| self.showing_raw(o) > last).cloned().collect();
        for o in &past {
            self.switch_workspace_on(o, last);
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
            } else {
                self.rehome_windows();
            }
            self.apply_mirroring();
            self.ipc.broadcast(Event::OutputsChanged { outputs: self.output_infos() });
        }
        if space_changed {
            self.tiling_space_changed();
        }
        if input_changed {
            self.apply_input_settings();
        }
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
        let a = &self.settings.appearance;
        if (theme::set(a.palette()) || force) && B::IS_SESSION {
            crate::toolkits::publish(a.icon_theme.clone(), a.colors.clone(), theme);
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
            // A new keymap starts with every lock off.
            let was = keyboard.modifier_state().num_lock;
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
            // Num Lock as set at start and when the setting changes;
            // otherwise as the Num Lock key last left it.
            let want = match self.num_lock_applied {
                Some(applied) if applied == k.num_lock => was,
                _ => k.num_lock,
            };
            self.num_lock_applied = Some(k.num_lock);
            let mut mods = keyboard.modifier_state();
            if mods.num_lock != want {
                mods.num_lock = want;
                keyboard.set_modifier_state(mods);
            }
            // The focused client keeps its own lock state until told,
            // even across the keymap just sent.
            keyboard.advertise_modifier_state(self);
            self.backend_data.update_led_state(keyboard.led_state());
        }
        let t = &self.settings.touchpad;
        self.pacer.momentum.length = if t.momentum { t.glide.clamp(0.25, 3.0) } else { 0.0 };
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

#[cfg(test)]
mod settings_tests {
    use super::*;

    #[test]
    fn old_terminal_app_ids_are_olos() {
        let mut s = Settings::default();
        s.keyboard.terminal_apps = ["tabula.terminal", "olo.terminal", "foot"].map(String::from).to_vec();
        adopt_old_app_ids(&mut s);
        assert_eq!(s.keyboard.terminal_apps, ["olo.terminal", "foot"]);
    }
}
