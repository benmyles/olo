//! Deskbar: the Olo menu, a status tray and the running applications,
//! docked in the top-right corner of the screen as on BeOS. Every display
//! has its own, showing the applications with windows there (and only
//! those windows).
//!
//! Each display has its own workspaces. The applications are grouped by
//! workspace, in order, each under its workspace's heading (as a
//! BOutlineListView groups them): every workspace of this display with
//! windows, and the one showing, whose heading is in the focused tab's
//! colour. With more than one display, a heading says whose ("D1
//! Workspace 2"). Clicking a heading goes to that workspace; its right end
//! is the workspace's windowing mode, to choose another; its menu (the
//! right button) shows, hides, closes or moves all its windows, to any
//! workspace of any display.
//!
//! Clicking an application brings it forward, every window of it there;
//! the right button opens the menu of its windows, to choose one.
//!
//! On olo-kit: one model (`Deskbar`) behind a lock, which the app's
//! looper (`Hub`) keeps as app_server, the status watchers and the tray
//! say, and every display's bar (`BarView`, a layer surface on a looper
//! of its own) draws, each keeping only what the pointer is doing over
//! it. A bar is drawn again when the hub says the model changed or the
//! pointer moves onto something else in it, and the clock ticks on the
//! minute (or second): nothing runs while nothing changes. The live
//! widgets are a view of their own over the bar (`widgets::Layer`): a
//! reading draws only them.

mod approve;
mod sni;
mod status;
mod widgets;

/// AppImages (`~/AppImages` in the Olo menu; running ones as themselves).
use olo_appimage as appimages;

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard, mpsc},
    time::{Duration, Instant},
};

use olo_kit::{
    Bounds, Child, ClickEvent, ContentMask, Corner, Cx, Element, ExternalPaths, FontWeight, InteractiveElement as _, LayerKeyboard, LayerShellLayer, LocalWindow, Menu, MenuItem, MouseButton,
    MouseDownEvent, MouseMoveEvent, Painter, ParentElement as _, Pixels, Point, RenderImage, ScrollWheelEvent, SharedString, Styled as _, View, ViewRef, WindowCx, WindowHandle, WindowOptions, agent,
    canvas, div, dnd, icons, menu,
    palette::{Palette, Rgba},
    point, px, size,
    style::{self, Align, c, shade},
    theme, tip,
    widgets::Alert,
};
use olo_protocol::{Client, DisplayWorkspace, Event, KeyboardProfile, NoteApp, OutputInfo, Reply, Request, WindowInfo, WindowingMode, shortcuts};
use olo_xdg::{
    apps::{OUR_APPS, PREFERENCE_PANELS, our_app},
    desktop_entry::{self, DesktopEntry},
    open::Handler,
};

/// Logical width of the vertical Deskbar.
const WIDTH: f32 = olo_kit::metrics::DESKBAR_WIDTH;
const TILE_H: f32 = 30.0;
/// How long a gone application keeps its place in the order.
const GONE_KEEP: Duration = Duration::from_secs(20);
const ROW_H: f32 = 24.0;
/// A workspace's heading.
const HEADING_H: f32 = 20.0;
const ICON: f32 = 16.0;
/// An application with notifications unread: a dot this wide at its
/// row's right end, as far in as its name's text, and the gap it keeps
/// from the name.
const NOTE_DOT: f32 = 7.0;
const NOTE_DOT_INSET: f32 = 6.0;
const NOTE_DOT_GAP: f32 = 5.0;
/// The dot drains from its right as the app's notifications fade: never
/// below this much filled, so it still reads as a dot; the rest is a
/// faint well of its colour, this strong.
const NOTE_DOT_FLOOR: f32 = 0.3;
const NOTE_DOT_WELL: f32 = 0.28;
/// With the count shown (Preferences ▸ Notifications), its gap from the
/// dot.
const NOTE_COUNT_GAP: f32 = 6.0;
/// One arriving shakes the dot side to side: this long, this far, this
/// many times, settling as it goes.
const SHAKE: Duration = Duration::from_millis(600);
const SHAKE_PX: f32 = 2.0;
const SHAKE_TIMES: f32 = 3.0;
/// Where an application row's icon starts, and its workspace's name
/// above it: in line with the tray's date and time (`TRAY_TEXT`).
const ICON_INSET: f32 = TRAY_TEXT;
/// The tray: the date and time on a row of their own, then a recessed
/// well of status icons, each a 16 px glyph on a 22 px grid (so 6 px
/// between neighbours, and 3 px from the well's edges), centred in as
/// many rows as it takes, shared out evenly among them. (Ben liked them
/// this close, 2026-11-03: a grid of its own to the layout review.)
const CLOCK_H: f32 = 32.0;
/// The date and time's own row in it: `TIGHT` from the tile above and
/// from the well below.
const CLOCK_ROW: f32 = 20.0;
/// Inset of the tray's well from the Deskbar's edges.
const TRAY_INSET: f32 = 4.0;
/// Inset of the tray's date and time.
const TRAY_TEXT: f32 = TRAY_INSET + 5.0;
const TRAY_PITCH: f32 = 22.0;
/// How far an icon's plate (lit under the pointer, and where it takes
/// clicks) reaches beyond its glyph.
const PLATE: f32 = 3.0;
/// Below the well, before the applications (on the 2-point grid, so
/// the rows below land on pixels at 1.5×).
const TRAY_FOOT: f32 = 4.0;
/// How far a press on an application moves before it's a drag (a link
/// to the program, dragged out).
const DRAG_START: f32 = 6.0;
/// The tray's height with `rows` rows of icons.
fn tray_height(rows: usize) -> f32 {
    CLOCK_H + rows as f32 * TRAY_PITCH + TRAY_FOOT
}

#[derive(Debug, Clone, Copy)]
enum Power {
    Restart,
    ShutDown,
}

impl Power {
    fn label(self) -> &'static str {
        match self {
            Power::Restart => "Restart",
            Power::ShutDown => "Shut Down",
        }
    }

    fn question(self) -> &'static str {
        match self {
            Power::Restart => "Do you really want to restart the system?",
            Power::ShutDown => "Do you really want to shut down the system?",
        }
    }
}

enum Msg {
    Windows(Vec<WindowInfo>),
    /// The active display's workspace showing, how many each display
    /// has, and each display's showing.
    Workspace(u32, u32, Vec<DisplayWorkspace>),
    Status(status::Status),
    Power(olo_protocol::PowerStatus),
    /// The power button held for an "Asking First" (app_server's
    /// power_button.rs): its alert, as the menu's Restart and Shut Down.
    PowerButton(Power),
    /// Undo and Redo Window Arrangement's names (None: nothing to).
    Arrangement(Option<String>, Option<String>),
    Settings(Settings),
    Outputs(Vec<OutputInfo>),
    Installed(Installed),
    /// What's in `~/AppImages` (None: no such folder).
    AppImages(Option<Vec<appimages::AppImage>>),
    /// Other programs' tray icons and launcher badges (`sni`).
    Tray(sni::Tray),
    /// The live widgets' readings, by widget (`widgets`).
    Meters(Vec<(u32, f64, olo_system::meters::Reading)>),
    /// The processes using most of what a widget measures (its tip's).
    Top(u32, Option<Vec<(String, f64)>>),
    /// What there is to measure (the widgets' GPUs, disks and sensors by
    /// name).
    Sources(olo_system::meters::Sources),

    // A bar's (or the model's own), to the app.
    /// The model changed under a bar's hand: every bar draws again.
    Changed,
    /// The clock's next minute (or second) came.
    Tick,
    /// A display came, went or is known to this connection now; a bar
    /// was closed under it.
    Displays,
    /// Something to ask the person, in a window of the main bar's.
    Effect(Effect),
    /// Approve Without Asking, for so many minutes: its window.
    Approve(u32),
}

impl Msg {
    /// Which it is, for the log.
    fn name(&self) -> &'static str {
        match self {
            Msg::Windows(_) => "windows",
            Msg::Workspace(..) => "workspace",
            Msg::Status(_) => "status",
            Msg::Power(_) => "power",
            Msg::PowerButton(_) => "power button",
            Msg::Arrangement(..) => "arrangement",
            Msg::Settings(_) => "settings",
            Msg::Outputs(_) => "outputs",
            Msg::Installed(_) => "installed",
            Msg::AppImages(_) => "appimages",
            Msg::Tray(_) => "tray",
            Msg::Meters(_) => "meters",
            Msg::Top(..) => "top",
            Msg::Sources(_) => "sources",
            Msg::Changed => "changed",
            Msg::Tick => "tick",
            Msg::Displays => "displays",
            Msg::Effect(_) => "effect",
            Msg::Approve(_) => "approve",
        }
    }
}

/// What a command wants asked of the person first (the main display's
/// bar opens it, so there's only ever one of each).
#[derive(Clone)]
enum Effect {
    /// Restart or Shut Down: its alert.
    ConfirmPower(Power),
    /// The newer build olo-session found: its alert (`waiting`: already
    /// asked for, while a build runs, which the alert says).
    Upgrade { message: String, waiting: bool },
}

/// Tells the app (its looper, from any thread).
fn tell(msg: Msg) {
    olo_kit::proxy().send(msg);
}

/// The model every bar draws, kept by the app's looper.
type Model = Arc<Mutex<Deskbar>>;

trait ModelExt {
    /// The model, to read (or to draw from: its caches fill).
    fn read(&self) -> MutexGuard<'_, Deskbar>;
    /// Changes the model, and has every bar drawn again.
    fn update<R>(&self, f: impl FnOnce(&mut Deskbar) -> R) -> R;
}

impl ModelExt for Model {
    fn read(&self) -> MutexGuard<'_, Deskbar> {
        self.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn update<R>(&self, f: impl FnOnce(&mut Deskbar) -> R) -> R {
        let r = f(&mut self.read());
        tell(Msg::Changed);
        r
    }
}

/// Runs a command from a bar (a menu's choice, a click).
fn run(model: &Model, cmd: &str, _: &mut WindowCx) {
    model.update(|d| d.run_command(cmd));
}

/// What is installed, found on a worker (`scan_installed`): reading every
/// application's .desktop file and searching PATH for each of Olo's apps
/// took 16 ms, and a frame has 8.3.
#[derive(Default)]
struct Installed {
    entries: Vec<DesktopEntry>,
    /// Which of `OUR_APPS` are installed.
    ours: Vec<&'static str>,
    preferences: bool,
}

/// Look for what is installed on a worker, and tell the app.
fn scan_installed(proxy: olo_kit::Proxy) {
    std::thread::spawn(move || {
        let ours = OUR_APPS.iter().map(|(bin, _, _)| *bin).filter(|bin| olo_xdg::find_program(bin).is_some()).collect();
        let preferences = olo_xdg::find_program("olo-preferences").is_some();
        proxy.send(Msg::Installed(Installed { entries: desktop_entry::all(), ours, preferences }));
    });
}

/// Look in `~/AppImages` on a worker (an AppImage new or changed is read
/// for its name and icon), and tell the app. One look at a time; a change
/// meanwhile (an AppImage still being downloaded) has one more after it.
fn scan_appimages(proxy: olo_kit::Proxy) {
    use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
    static RUNNING: AtomicBool = AtomicBool::new(false);
    static AGAIN: AtomicBool = AtomicBool::new(false);
    if RUNNING.swap(true, SeqCst) {
        AGAIN.store(true, SeqCst);
        return;
    }
    std::thread::spawn(move || {
        loop {
            AGAIN.store(false, SeqCst);
            proxy.send(Msg::AppImages(appimages::scan()));
            if !AGAIN.load(SeqCst) {
                RUNNING.store(false, SeqCst);
                // (One that came as this one ended.)
                if !AGAIN.swap(false, SeqCst) || RUNNING.swap(true, SeqCst) {
                    return;
                }
            }
        }
    });
}

/// What Deskbar follows of app_server's settings.
struct Settings {
    clock: olo_protocol::ClockSettings,
    /// Whether the Olo menu shows icons.
    menu_icons: bool,
    /// Grouping, and each workspace's windowing mode.
    tiling: olo_protocol::TilingSettings,
    /// The person's shortcut keys.
    keys: KeyboardProfile,
    /// The live widgets (Preferences ▸ Deskbar).
    widgets: Vec<olo_protocol::deskbar::Widget>,
}

/// A team: the windows of one application.
#[derive(Clone)]
struct Team {
    key: String,
    name: String,
    icon: Option<String>,
    windows: Vec<WindowInfo>,
}

struct Deskbar {
    /// The display of the bar being drawn (`draw_bar`): only its
    /// workspace showing is lit.
    drawing_for: Option<String>,
    outputs: Vec<OutputInfo>,
    windows: Vec<WindowInfo>,
    /// Team keys in order of first appearance. One whose windows have
    /// gone keeps its place for a while (`GONE_KEEP`): an app that starts
    /// itself anew (it lost its GPU, or was upgraded) comes back to it.
    order: Vec<String>,
    gone: HashMap<String, Instant>,
    /// The active display's workspace showing, and how many each display
    /// has.
    workspace: (u32, u32),
    /// Each display's workspace showing.
    showing: Vec<DisplayWorkspace>,
    status: status::Status,
    caffeinated: bool,
    /// A timed caffeinated stretch's end (seconds since the epoch).
    caffeinated_until: Option<u64>,
    caffeinated_lid: bool,
    clock: olo_protocol::ClockSettings,
    /// Appearance ▸ Show icons in the Deskbar menu.
    menu_icons: bool,
    installed: Installed,
    /// The AppImages in `~/AppImages` (None: no such folder).
    appimages: Option<Vec<appimages::AppImage>>,
    /// A look for what is installed under way (`scan_installed`), and
    /// when the last one started.
    scanning: bool,
    scanned_at: Option<Instant>,
    /// What is installed has been looked for once.
    installed_known: bool,
    entries: HashMap<String, Option<DesktopEntry>>,
    /// App ids of windows looked at for an entry, and with none, at their
    /// process (`appimages::of_process`: an AppImage's, its entry not
    /// saying it).
    asked_process: std::collections::HashSet<String>,
    /// The window that last had the focus: still "this window" for the
    /// Olo menu's commands while the menu holds the keyboard.
    last_focused: Option<u64>,
    /// Apps whose windows aren't grouped (`TilingSettings::ungrouped_apps`),
    /// and each workspace's windowing mode.
    tiling: olo_protocol::TilingSettings,
    /// The display of the Deskbar whose Olo menu (or windowing mode menu)
    /// is open: its windowing mode is that display's, and its workspaces
    /// are switched.
    menu_output: Option<String>,
    /// The workspace of that display whose windowing mode menu is open
    /// (a heading's); None for the one showing.
    menu_workspace: Option<u32>,
    /// The person's changes to app_server's shortcuts, for the keys the
    /// Olo menu shows.
    keys: KeyboardProfile,
    /// Approving requests without asking until then (seconds since the
    /// epoch; app_server's approve.rs).
    approve_until: Option<u64>,
    /// What Undo and Redo Window Arrangement would do, by name (app_server's).
    arrangement: (Option<String>, Option<String>),
    /// The newest unread notification seen (None until the first word
    /// from the service: what is unread then was there before Deskbar).
    notes_seen: Option<(u64, u32)>,
    /// The apps whose dot is shaking (a notification just arrived), by
    /// `note_key`: since when.
    shaking: HashMap<String, Instant>,
    /// Other programs' tray icons, and who to tell what's done to them.
    tray_apps: Vec<sni::Item>,
    tray_host: sni::Host,
    /// Programs' unread counts, progress and wishes for attention, by
    /// desktop entry.
    badges: Vec<sni::Badge>,
    /// Their pictures, by item, picture and size in device pixels.
    tray_pictures: HashMap<(String, u64, u32), Option<Arc<RenderImage>>>,
    /// The live widgets, what they've read, and their worker.
    live: widgets::Live,
    /// What there is to measure, for the widgets' tips (read once, off
    /// the UI thread).
    sources: olo_system::meters::Sources,
}

/// An application's dot: how far its shake has gone if one just came
/// (0–1), how fresh its notifications are together (1: none has begun to
/// fade; the dot drains from its right as they do), and how many it has.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Dot {
    shake: Option<f32>,
    fresh: f32,
    count: u32,
}

/// The count beside a dot, as shown.
fn count_text(n: u32) -> String {
    if n > 99 { "99+".to_owned() } else { n.to_string() }
}

/// Which app a notification's dot belongs to.
fn note_key(n: &NoteApp) -> String {
    format!("{}\u{1}{}", n.desktop_entry.as_deref().unwrap_or(""), n.name)
}

/// Whether notifications from `n` are the application of windows
/// `app_id` (its desktop entry `entry`, named `name`): by the desktop
/// entry they name (an Olo app's `olo-terminal` is window `olo.terminal`'s;
/// another's, its entry's file or window class), else by name.
fn same_app(n: &NoteApp, app_id: &str, entry: Option<&DesktopEntry>, name: &str) -> bool {
    let norm = |s: &str| s.trim().trim_end_matches(".desktop").to_ascii_lowercase();
    let id = norm(app_id);
    if let Some(de) = n.desktop_entry.as_deref().map(norm).filter(|d| !d.is_empty())
        && (de == id
            || id.strip_prefix("olo.").is_some_and(|rest| de == format!("olo-{rest}"))
            || entry.is_some_and(|e| norm(&e.id) == de || e.startup_wm_class.as_deref().is_some_and(|c| norm(c) == de)))
    {
        return true;
    }
    !n.name.is_empty() && n.name.eq_ignore_ascii_case(name)
}

/// Where a launcher badge's dot shake is kept (beside notifications').
fn badge_key(app: &str) -> String {
    format!("badge:{app}")
}

fn team_key(w: &WindowInfo) -> String {
    if !w.app_id.is_empty() {
        w.app_id.clone()
    } else if let Some(pid) = w.pid {
        format!("pid:{pid}")
    } else {
        format!("win:{}", w.id)
    }
}

/// Send `req` to app_server off the UI thread.
fn request(req: Request) {
    requests(vec![req]);
}

/// Ask app_server for each of `reqs`, in turn.
fn requests(reqs: Vec<Request>) {
    std::thread::spawn(move || {
        if let Ok(mut c) = Client::connect() {
            for req in &reqs {
                let _ = c.request(req);
            }
        }
    });
}

fn spawn(command: &str) {
    if let Err(e) = olo_xdg::spawn_detached(command) {
        tracing::warn!("launch {command}: {e}");
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl Deskbar {
    /// The installed application `app_id` is, from what was found
    /// installed; one not found starts a new look (an app just installed).
    fn entry(&mut self, app_id: &str) -> Option<DesktopEntry> {
        let found = self.entries.entry(app_id.to_owned()).or_insert_with(|| desktop_entry::find_among(&self.installed.entries, app_id).cloned()).clone();
        if found.is_none() && !app_id.is_empty() && self.scanned_at.is_none_or(|t| t.elapsed().as_secs() >= 10) {
            self.rescan();
        }
        found
    }

    /// Look for what is installed again, on a worker.
    fn rescan(&mut self) {
        if !std::mem::replace(&mut self.scanning, true) {
            self.scanned_at = Some(Instant::now());
            scan_installed(olo_kit::proxy());
        }
    }

    /// Who opens files dropped on the team `key`: the app itself.
    fn handler(&mut self, key: &str) -> Option<Handler> {
        let app_id = self.windows.iter().find(|w| team_key(w) == key)?.app_id.clone();
        let short = app_id.strip_prefix("olo.").unwrap_or(&app_id);
        if let Some((bin, _, _)) = OUR_APPS.iter().find(|(bin, _, _)| bin.strip_prefix("olo-") == Some(short)) {
            return Some(Handler::Ours(bin));
        }
        if short == "preferences" {
            return Some(Handler::Ours("olo-preferences"));
        }
        self.entry(&app_id).filter(|e| e.exec.is_some()).map(|e| Handler::Entry(Box::new(e)))
    }

    /// The program of the team `key`, to drag out as a link: ours, or an
    /// installed application whose command is a program alone.
    fn program(&mut self, key: &str) -> Option<PathBuf> {
        match self.handler(key)? {
            Handler::Ours(bin) => olo_xdg::find_program(bin),
            Handler::Entry(e) => {
                let command = e.command()?;
                let mut words = command.split_whitespace();
                let (program, None) = (words.next()?, words.next()) else { return None };
                let path = std::path::Path::new(program);
                if path.is_absolute() { path.is_file().then(|| path.to_owned()) } else { olo_xdg::find_program(program) }
            }
        }
    }

    /// The display whose applications a bar on `output` shows.
    fn shown_on(&self, output: Option<&str>) -> Option<String> {
        output.map(str::to_owned).or_else(|| self.main_output())
    }

    /// The workspace display `display` shows (the active one's, if it
    /// isn't known).
    fn showing(&self, display: Option<&str>) -> u32 {
        self.showing.iter().find(|d| Some(d.output.as_str()) == display).map_or(self.workspace.0, |d| d.current)
    }

    /// What `display`'s workspace `ws` is called: "Workspace 2", or with
    /// several displays, "D1 Workspace 2".
    fn workspace_name(&self, display: Option<&str>, ws: u32) -> String {
        olo_protocol::workspace_name(display.and_then(|d| olo_protocol::display_number(&self.outputs, d)), ws)
    }

    /// The windowing mode of `display`'s workspace `ws`.
    fn mode(&self, display: Option<&str>, ws: u32) -> WindowingMode {
        let key = self.outputs.iter().find(|o| Some(o.name.as_str()) == display).map(|o| o.key.clone()).unwrap_or_default();
        self.tiling.mode(ws, &key)
    }

    /// Choose the windowing mode of `output`'s display (the active one
    /// for None), on its workspace `ws` (the one showing for None); at
    /// once here too, so its heading shows it before app_server's answer.
    fn set_mode(&mut self, output: Option<String>, ws: Option<u32>, mode: WindowingMode) {
        let key = self.outputs.iter().find(|o| Some(&o.name) == output.as_ref()).map(|o| o.key.clone()).unwrap_or_default();
        let at = ws.unwrap_or_else(|| self.showing(output.as_deref()));
        self.tiling.set_mode(at, &key, mode);
        request(Request::SetWindowingMode { mode, workspace: ws, output });
    }

    /// The display icon's menu, on display `own`'s Deskbar: every display
    /// (its number and name, left to right), its own checked; choosing
    /// one goes there.
    fn display_menu(&self, own: &str) -> Menu {
        let mut displays: Vec<&OutputInfo> = self.outputs.iter().filter(|o| o.mirror_of.is_none()).collect();
        displays.sort_by_key(|o| o.position);
        let mut m = Menu::new("Displays");
        for o in displays {
            let n = olo_protocol::display_number(&self.outputs, &o.name).map(|n| format!("{}  ", olo_protocol::display_label(n))).unwrap_or_default();
            let here = o.name == own;
            m = m.item(format!("{n}{}", o.display_name()), format!("display:{}", o.name)).checked(here).enabled(!here);
        }
        m.separator().item("Displays Preferences…", "pref:display")
    }

    /// The displays whose workspaces a Deskbar on `own` lists: its own
    /// first, then the others left to right (not those mirroring
    /// another); just `own` while the displays aren't known.
    fn displays_from(&self, own: Option<&str>) -> Vec<Option<String>> {
        let mut out = vec![own.map(str::to_owned)];
        if own.is_some() {
            let mut others: Vec<&OutputInfo> = self.outputs.iter().filter(|o| o.mirror_of.is_none() && Some(o.name.as_str()) != own).collect();
            others.sort_by_key(|o| o.position);
            out.extend(others.into_iter().map(|o| Some(o.name.clone())));
        }
        out
    }

    fn main_output(&self) -> Option<String> {
        self.outputs.iter().find(|o| o.main).map(|o| o.name.clone())
    }

    /// The windows on display `on` (a window whose display isn't known is
    /// the main one's); all of them for None.
    fn windows_on(&self, on: Option<&str>) -> impl Iterator<Item = &WindowInfo> {
        let main = self.main_output();
        self.windows.iter().filter(move |w| on.is_none_or(|on| w.output.as_deref().or(main.as_deref()) == Some(on)))
    }

    /// A Deskbar on each display (but those mirroring another), each
    /// asking for its display by name: one that didn't (opened before the
    /// displays were known) would follow the main display about, and its
    /// own would be left without.
    /// The applications with windows on display `on` (all for None), and
    /// on workspace `ws` (any for None), each with those windows.
    fn teams(&mut self, on: Option<&str>, ws: Option<u32>) -> Vec<Team> {
        let mut teams: Vec<Team> = Vec::new();
        for key in self.order.clone() {
            let windows: Vec<WindowInfo> = self.windows_on(on).filter(|w| team_key(w) == key && ws.is_none_or(|ws| w.workspace == ws)).cloned().collect();
            if windows.is_empty() {
                continue;
            }
            let app_id = windows[0].app_id.clone();
            let (name, icon) = self.app_name(&app_id);
            let name = if name.is_empty() { windows[0].title.clone() } else { name };
            teams.push(Team { key, name, icon, windows });
        }
        teams
    }

    /// What to call the app `app_id` (or a program's name), and its icon:
    /// ours, its desktop entry's, else the id's last part capitalised.
    fn app_name(&mut self, app_id: &str) -> (String, Option<String>) {
        // (Ours need no entry: none looked for.)
        let entry = if our_app(app_id).is_some() { None } else { self.entry(app_id) };
        olo_xdg::apps::app_name(app_id, entry.as_ref())
    }

    fn set_windows(&mut self, windows: Vec<WindowInfo>) {
        for w in &windows {
            let k = team_key(w);
            if !self.order.contains(&k) {
                self.order.push(k);
            }
        }
        let now = Instant::now();
        for k in &self.order {
            if windows.iter().any(|w| &team_key(w) == k) {
                self.gone.remove(k);
            } else {
                self.gone.entry(k.clone()).or_insert(now);
            }
        }
        let gone = &self.gone;
        self.order.retain(|k| gone.get(k).is_none_or(|t| now.duration_since(*t) < GONE_KEEP));
        let order = &self.order;
        self.gone.retain(|k, _| order.contains(k));
        let focused = windows.iter().find(|w| w.focused).map(|w| w.id);
        self.last_focused = focused.or(self.last_focused.filter(|id| windows.iter().any(|w| w.id == *id && !w.minimized)));
        self.windows = windows;
        self.ask_processes();
    }

    /// Windows of an app no entry gives (once what is installed is
    /// known): an AppImage's whose own entry doesn't say its app id is
    /// found by its process, on a worker, and the id kept for it; its
    /// entry then says it, and the folder's change brings it here (and
    /// to every other app) with its name and icon. Each id looked at once.
    fn ask_processes(&mut self) {
        if !self.installed_known {
            return;
        }
        let mut ask = Vec::new();
        for w in &self.windows {
            let Some(pid) = w.pid else { continue };
            // (Each id once: a window event costs a lookup for each g.)
            if w.app_id.is_empty() || self.asked_process.contains(&w.app_id) {
                continue;
            }
            self.asked_process.insert(w.app_id.clone());
            if our_app(&w.app_id).is_none() && desktop_entry::find_among(&self.installed.entries, &w.app_id).is_none() {
                ask.push((w.app_id.clone(), pid));
            }
        }
        if !ask.is_empty() {
            std::thread::spawn(move || {
                for (app_id, pid) in ask {
                    if let Some(path) = appimages::of_process(pid) {
                        appimages::learn(&path, &app_id);
                    }
                }
            });
        }
    }

    fn olo_menu(&self, mode: WindowingMode) -> Menu {
        let mut menu = self.olo_menu_with_icons(mode);
        add_keys(&mut menu, &self.keys);
        if self.menu_icons { menu } else { without_icons(menu) }
    }

    fn olo_menu_with_icons(&self, mode: WindowingMode) -> Menu {
        let mut apps = Menu::new("Applications");
        // Only the ones that are installed (the suite is still growing).
        for (bin, name, icon) in OUR_APPS.iter().filter(|(bin, _, _)| self.installed.ours.contains(bin)) {
            apps = apps.item(*name, format!("run:{bin}")).icon(*icon);
        }
        let apps = apps.sorted();
        let have_prefs = self.installed.preferences;
        let mut prefs = Menu::new("Preferences");
        for (arg, name, icon) in PREFERENCE_PANELS {
            prefs = prefs.item(*name, format!("pref:{arg}")).enabled(have_prefs).icon(*icon);
        }
        let prefs = prefs.sorted();
        // Everything else installed, by freedesktop category.
        let groups: &[(&str, &str, &[&str])] = &[
            ("Accessories", "accessories", &["Utility", "Accessories"]),
            ("Development", "development", &["Development"]),
            ("Graphics", "image", &["Graphics"]),
            ("Internet", "internet", &["Network", "WebBrowser", "Email"]),
            ("Multimedia", "multimedia", &["AudioVideo", "Audio", "Video"]),
            ("Office", "office", &["Office"]),
            ("Games", "games", &["Game"]),
            ("System", "system", &["System", "Settings"]),
        ];
        let entry_icon = |e: &DesktopEntry| e.icon.clone().filter(|i| !i.is_empty()).unwrap_or_else(|| "application".into());
        let mut other = Menu::new("Other Applications");
        let all: Vec<&DesktopEntry> = self.installed.entries.iter().filter(|e| !e.no_display).collect();
        let mut used = vec![false; all.len()];
        for (title, icon, cats) in groups {
            let mut m = Menu::new(*title);
            for (i, e) in all.iter().enumerate() {
                if !used[i] && e.categories.iter().any(|c| cats.contains(&c.as_str())) {
                    m = m.item(&e.name, format!("desktop:{}", e.id)).icon(entry_icon(e));
                    used[i] = true;
                }
            }
            if !m.items.is_empty() {
                other = other.submenu(m).icon(*icon);
            }
        }
        // Each kind, and what's in it, in alphabetical order; Other last.
        other = other.sorted();
        let rest: Vec<_> = all.iter().enumerate().filter(|(i, _)| !used[*i]).map(|(_, e)| e).collect();
        if !rest.is_empty() {
            let mut m = Menu::new("Other");
            for e in rest {
                m = m.item(&e.name, format!("desktop:{}", e.id)).icon(entry_icon(e));
            }
            other = other.submenu(m.sorted()).icon("folder");
        }

        let mut menu = Menu::new("Olo")
            .item("About Olo Desktop", "about")
            .icon("olo")
            .item("Find…", "find")
            .icon("query")
            .separator()
            .submenu(self.arrange_menu(mode))
            .icon("display")
            .submenu(self.workspaces_menu(self.menu_output.as_deref()))
            .icon("workspaces")
            .item("Show or Hide All Windows", "arrange:show-desktop")
            .icon("desktop")
            .separator()
            .submenu(self.caffeine_menu())
            .icon("stay-awake")
            .submenu(approve::menu(self.approve_until, &self.clock))
            .icon("key")
            .item("Lock Screen", "lock")
            .icon("lock-screen")
            .item("Log Out", "logout")
            .icon("log-out")
            .item("Restart", "restart")
            .icon("restart")
            .item("Shut Down", "shutdown")
            .icon("shut-down")
            .separator()
            .submenu(apps)
            .icon("application");
        // Under Applications: what's in ~/AppImages, if there's the folder.
        if let Some(m) = self.appimages_menu() {
            menu = menu.submenu(m).icon("archive");
        }
        menu.submenu(other).icon("other-applications").submenu(prefs).icon("preferences")
    }

    /// `~/AppImages`' apps, by name, each with its own icon (taken out of
    /// it) or the generic one; None without the folder.
    fn appimages_menu(&self) -> Option<Menu> {
        let found = self.appimages.as_ref()?;
        let mut m = Menu::new("AppImages");
        if found.is_empty() {
            return Some(m.item("Put AppImages in ~/AppImages", "none").enabled(false));
        }
        for a in found {
            let icon = a.icon.as_ref().map_or_else(|| "application".to_owned(), |p| p.display().to_string());
            m = m.item(&a.name, format!("appimage:{}", a.path.display())).icon(icon);
        }
        Some(m)
    }

    /// The focused window (what "Others" leaves alone and a window is
    /// moved from), if any: the last one, as opening this menu takes the
    /// focus.
    fn focused_window(&self) -> Option<u64> {
        self.last_focused.filter(|id| self.windows.iter().any(|w| w.id == *id && w.workspace == self.showing(w.output.as_deref())))
    }

    /// The windowing modes (`mode:` and its id), `mode` checked, and the
    /// arranging commands app_server's shortcuts do (`arrange:` and a
    /// shortcut id), for the active display.
    fn arrange_menu(&self, mode: WindowingMode) -> Menu {
        let focused = self.focused_window().is_some();
        let mut menu = self.modes_menu(mode);
        menu.title = "Window Arrangement".into();
        menu.separator()
            .item("Tile All", "arrange:tile-all")
            .item("Smart Tile", "arrange:smart-tile")
            .item("Zoom All", "arrange:zoom-all")
            .item("Cascade All", "arrange:cascade-all")
            .item("Fan Out", "arrange:fan-out")
            .separator()
            .item("Cascade Others", "arrange:cascade-others")
            .enabled(focused)
            .item("Fan Out Others", "arrange:fan-out-others")
            .enabled(focused)
            .separator()
            // (Named as an app's Edit ▸ Undo is: Undo Cascade, Redo Move
            // to Workspace 2.)
            .item(self.arrangement.0.as_ref().map_or("Undo Window Arrangement".into(), |n| format!("Undo {n}")), "arrange:undo-arrangement")
            .enabled(self.arrangement.0.is_some())
            .item(self.arrangement.1.as_ref().map_or("Redo Window Arrangement".into(), |n| format!("Redo {n}")), "arrange:redo-arrangement")
            .enabled(self.arrangement.1.is_some())
            .separator()
            .item("Windows Preferences…", "pref:windows")
    }

    /// The windowing modes (`mode:` and its id), `mode` checked, and
    /// auto-tiling's layouts (`layout:` and its id; every display's).
    fn modes_menu(&self, mode: WindowingMode) -> Menu {
        let mut menu = Menu::new("Windowing Mode");
        for m in WindowingMode::ALL {
            menu = menu.item(m.title(), format!("mode:{}", m.id())).checked(m == mode);
        }
        let mut layouts = Menu::new("Auto-Tiling Layout");
        for l in olo_protocol::TilingLayout::ALL {
            layouts = layouts.item(l.title(), format!("layout:{}", l.id())).checked(l == self.tiling.layout);
        }
        menu.submenu(layouts)
    }

    /// Go to one of `display`'s workspaces, or move the focused window to
    /// one of its display's.
    fn workspaces_menu(&self, display: Option<&str>) -> Menu {
        let (current, count) = (self.showing(display), self.workspace.1.clamp(1, 9));
        let mut m = Menu::new("Workspaces");
        for i in 0..count {
            m = m.item(self.workspace_name(display, i), format!("workspace:{i}")).checked(i == current);
            if let Some(keys) = shortcuts::by_id(&format!("workspace-{}", i + 1)).and_then(|info| shortcuts::keys_for(info, &self.keys).first().map(|c| c.label(self.keys.platform))) {
                m = m.keys(keys);
            }
        }
        if let Some(w) = self.focused_window().and_then(|id| self.windows.iter().find(|w| w.id == id))
            && count > 1
        {
            let mut to = Menu::new("Move Window To");
            for i in 0..count {
                let name = self.workspace_name(w.output.as_deref(), i);
                to = to.item(name, format!("arrange:window-to-workspace-{}", i + 1)).enabled(i != w.workspace);
            }
            m = m.separator().submenu(to);
        }
        m.separator().item("Workspaces Preferences…", "pref:workspaces")
    }

    /// A workspace heading's menu, for `display`'s workspace `ws`: all its
    /// windows shown, hidden, closed, or moved to the same workspace of
    /// another display, or to any workspace of any display.
    fn heading_menu(&self, display: Option<&str>, ws: u32) -> Menu {
        let any = self.windows_on(display).any(|w| w.workspace == ws);
        let mut m = Menu::new(self.workspace_name(display, ws))
            .item("Show All", format!("ws-show:{ws}"))
            .enabled(any)
            .item("Hide All", format!("ws-hide:{ws}"))
            .enabled(any)
            .item("Close All", format!("ws-close:{ws}"))
            .enabled(any)
            .separator();
        let mut displays: Vec<&OutputInfo> = self.outputs.iter().filter(|o| o.mirror_of.is_none()).collect();
        displays.sort_by_key(|o| o.position);
        for d in displays.iter().filter(|d| Some(d.name.as_str()) != display) {
            m = m.item(format!("Move to {}", self.workspace_name(Some(&d.name), ws)), format!("ws-move:{ws}:{ws}:{}", d.name)).enabled(any);
        }
        let mut to = Menu::new("Move All Windows To");
        for (k, d) in displays.iter().enumerate() {
            if k > 0 {
                to = to.separator();
            }
            for i in 0..self.workspace.1.clamp(1, 9) {
                let here = Some(d.name.as_str()) == display && i == ws;
                to = to.item(self.workspace_name(Some(&d.name), i), format!("ws-move:{ws}:{i}:{}", d.name)).enabled(any && !here);
            }
        }
        m = m.submenu(to);
        // Or onto an agent screen, programs running on.
        if olo_kit::moving::menus_offer() && self.windows_on(display).any(|w| w.workspace == ws && w.kept) {
            m = m.submenu(Menu::agent_screens("Move All Windows to Agent Screen", &self.status.agent_screens, &format!("ws-screen:{ws}:")));
        }
        m.separator().item("Workspaces Preferences…", "pref:workspaces")
    }

    /// A command from `display`'s workspace heading's menu
    /// (`heading_menu`); false if it isn't one.
    fn heading_command(&mut self, display: Option<&str>, cmd: &str) -> bool {
        let Some((what, rest)) = cmd.split_once(':') else { return false };
        let mut parts = rest.splitn(3, ':');
        let Some(ws) = parts.next().and_then(|n| n.parse::<u32>().ok()) else { return false };
        let ids: Vec<u64> = self.windows_on(display).filter(|w| w.workspace == ws).map(|w| w.id).collect();
        match what {
            "ws-show" if !ids.is_empty() => request(Request::ActivateAll { ids }),
            "ws-hide" => ids.into_iter().for_each(|id| request(Request::Minimize { id })),
            "ws-close" => ids.into_iter().for_each(|id| request(Request::Close { id })),
            "ws-screen" => {
                let rest = parts.next().unwrap_or_default();
                let ids: Vec<u64> = self.windows_on(display).filter(|w| w.workspace == ws && w.kept).map(|w| w.id).collect();
                if let Some(screen) = olo_kit::moving::screen_command(rest)
                    && !ids.is_empty()
                {
                    olo_kit::moving::take_to_screen(screen, ids);
                }
            }
            "ws-move" => {
                let (Some(to_index), Some(to), Some(output)) = (parts.next().and_then(|n| n.parse().ok()), parts.next(), display) else { return true };
                request(Request::MoveWorkspace { output: output.to_owned(), index: ws, to: to.to_owned(), to_index: Some(to_index) });
            }
            _ => return what == "ws-show",
        }
        true
    }

    fn team_menu(&self, team: &Team) -> Menu {
        let mut m = Menu::new(&team.name);
        for w in &team.windows {
            let title = if w.title.is_empty() { team.name.clone() } else { w.title.clone() };
            let label = if w.workspace != self.showing(w.output.as_deref()) { format!("{title}  ({})", w.workspace + 1) } else { title };
            m = m.item(label, format!("win:{}", w.id)).checked(w.focused).minimized(w.minimized);
        }
        // An app of ours with several windows opens another, as BeOS's
        // Deskbar offered.
        if team.windows.first().is_some_and(|w| new_window_app(&w.app_id)) {
            m = m.separator().item("New Window", "new-window");
        }
        let app = team.windows.first().map(|w| w.app_id.clone()).unwrap_or_default();
        m = m.separator().item("Hide All", "hide-all").item("Show All", "show-all").item("Close All", "close-all").separator();
        // Its windows to any workspace of any display, as their tabs'
        // menus move one.
        let several = team.windows.len() > 1;
        let mut displays: Vec<&OutputInfo> = self.outputs.iter().filter(|o| o.mirror_of.is_none()).collect();
        displays.sort_by_key(|o| o.position);
        let mut to = Menu::new(if several { "Move All to Workspace" } else { "Move to Workspace" });
        for (k, d) in displays.iter().enumerate() {
            if k > 0 {
                to = to.separator();
            }
            for i in 0..self.workspace.1.clamp(1, 9) {
                let there = team.windows.iter().all(|w| w.output.as_deref() == Some(d.name.as_str()) && w.workspace == i);
                to = to.item(self.workspace_name(Some(&d.name), i), format!("move:{i}:{}", d.name)).checked(there).enabled(!there);
            }
        }
        m = m.submenu(to);
        if displays.len() > 1 {
            m = m
                .item(if several { "All to the Next Display" } else { "To the Next Display" }, "shortcut:next-display")
                .item(if several { "All to the Previous Display" } else { "To the Previous Display" }, "shortcut:previous-display");
        }
        // Or onto an agent screen, the program running on.
        if olo_kit::moving::menus_offer() && team.windows.iter().any(|w| w.kept) {
            m = m.submenu(Menu::agent_screens(if several { "Move All to Agent Screen" } else { "Move to Agent Screen" }, &self.status.agent_screens, ""));
        }
        // Live on a television (Ben, 2026-11-12: "integrated airplay … by
        // right clicking the … deskbar row"): its window, or one of them.
        let paired = &self.status.airplay_paired;
        m = match &team.windows[..] {
            [_] => m.submenu(Menu::airplay("Show on AirPlay", paired, "")),
            windows => {
                let mut each = Menu::new("Show on AirPlay");
                for w in windows {
                    let title = if w.title.is_empty() { team.name.clone() } else { w.title.chars().take(60).collect() };
                    each = each.submenu(Menu::airplay(&title, paired, &format!("airplay-win:{}:", w.id)));
                }
                m.submenu(each)
            }
        };
        // Said as it is (Ben, 2026-11-08: "no checks/ticks just the option
        // changes based on the state"): grouped, Ungroup All; else Group All.
        let grouped = self.all_grouped(&team.windows, !self.tiling.ungrouped_apps.contains(&app));
        let (label, cmd) = if grouped { ("Ungroup All Windows", "ungroup-all") } else { ("Group All Windows", "group-all") };
        m.separator().item(label, cmd).enabled(!app.is_empty())
    }

    /// Whether all of a team's windows are grouped, as they show now:
    /// wherever two or more show together (a display's workspace, not
    /// minimized, ones that can be grouped), all in one group. With fewer
    /// showing, as the app's grouping is set (`grouping`), so a window
    /// opening later joins or doesn't.
    fn all_grouped(&self, windows: &[WindowInfo], grouping: bool) -> bool {
        let shown: Vec<&WindowInfo> = windows.iter().filter(|w| w.groupable && !w.minimized && w.workspace == self.showing(w.output.as_deref())).collect();
        let mut together = false;
        for w in &shown {
            let here: Vec<&&WindowInfo> = shown.iter().filter(|o| o.output == w.output && o.workspace == w.workspace).collect();
            if here.len() < 2 {
                continue;
            }
            together = true;
            if w.group.is_none() || here.iter().any(|o| o.group != w.group) {
                return false;
            }
        }
        together || grouping
    }

    /// The battery's menu (right click): the power mode, and Power's
    /// preferences.
    fn battery_menu(&self) -> Menu {
        use olo_system::power_profile::Mode;
        let mut m = Menu::new("Power");
        if let Some(modes) = &self.status.power_modes {
            for mode in Mode::ALL.into_iter().filter(|x| modes.available.contains(x)) {
                m = m.item(mode.label(), format!("power-mode:{}", mode.id())).checked(modes.active == mode);
            }
            m = m.separator();
        }
        m.item("Power Preferences…", "pref:power")
    }

    /// Caffeinated mode: indefinitely or for a while, with the lid too.
    fn caffeine_menu(&self) -> Menu {
        let on = self.caffeinated;
        let mut m = Menu::new("Stay Awake").item("Until Turned Off", "caff:on").checked(on && self.caffeinated_until.is_none());
        for (label, minutes) in [("For 1 Hour", 60), ("For 2 Hours", 120), ("For 4 Hours", 240), ("For 8 Hours", 480)] {
            m = m.item(label, format!("caff:{minutes}"));
        }
        let label = match self.caffeinated_until {
            Some(t) if on => format!("Allow Sleep (awake until {})", status::time_of_day(t, &self.clock)),
            _ => "Allow Sleep".into(),
        };
        m.separator().item("Even With the Lid Closed", "caff:lid").checked(self.caffeinated_lid).separator().item(label, "caff:off").enabled(on).separator().item("Power Preferences…", "pref:power")
    }

    /// A caffeinated-menu command (after `caff:`).
    fn caffeine_command(&mut self, arg: &str) {
        let now = now_secs();
        // What's left of a timed stretch, to keep it when only the lid changes.
        let left = self.caffeinated_until.filter(|_| self.caffeinated).map(|t| t.saturating_sub(now).div_ceil(60).max(1) as u32);
        let (on, minutes, lid) = match arg {
            "on" => (true, None, self.caffeinated_lid),
            "off" => (false, None, false),
            "lid" => (true, left, !self.caffeinated_lid),
            m => match m.parse::<u32>() {
                Ok(m) => (true, Some(m), self.caffeinated_lid),
                Err(_) => return,
            },
        };
        self.caffeinated = on;
        self.caffeinated_lid = lid;
        self.caffeinated_until = minutes.map(|m| now + m as u64 * 60);
        request(Request::SetCaffeinated { on, minutes, lid });
    }

    /// An approve-without-asking command (after `approve:`): `off`, or
    /// minutes. Turning it on takes the password (the window); on
    /// already, only the length changes.
    fn approve_command(&mut self, arg: &str) {
        if arg == "off" {
            self.approve_until = None;
            request(Request::SetAutoApprove { minutes: None, password: None });
            return;
        }
        let Ok(minutes) = arg.parse::<u32>() else { return };
        if self.approve_until.is_some() {
            request(Request::SetAutoApprove { minutes: Some(minutes), password: None });
        } else {
            // (Its window, asking for the password: the app's.)
            tell(Msg::Approve(minutes));
        }
    }

    /// A Do Not Disturb command (after `notif:`): `on`, `off`, `tomorrow`
    /// or minutes. Written to the notification settings, which the service
    /// follows.
    fn do_not_disturb(&mut self, arg: &str) {
        let now = now_secs();
        let (on, until) = match arg {
            "on" => (true, None),
            "off" => (false, None),
            "tomorrow" => (true, Some(now + status::seconds_until(8 * 60) as u64)),
            m => match m.parse::<u64>() {
                Ok(m) => (true, Some(now + m * 60)),
                Err(_) => return,
            },
        };
        if let Some(n) = &mut self.status.notifications {
            n.quiet = on;
        }
        std::thread::spawn(move || {
            let mut s = olo_protocol::NotificationSettings::load();
            s.dnd = on;
            s.dnd_until = until;
            if let Err(e) = s.save() {
                tracing::warn!("notification settings: {e}");
            }
        });
    }

    fn run_command(&mut self, cmd: &str) {
        match cmd.split_once(':') {
            // One of Olo's apps (with ⌘ held, a copy of its own beside one
            // running).
            Some(("run", bin)) if CHOSEN_WITH_COMMAND.with(|c| c.get()) => {
                let app = bin.split_whitespace().next().and_then(|b| b.strip_prefix("olo-")).map(|b| format!("olo.{b}")).unwrap_or_default();
                if let Err(e) = olo_xdg::spawn_detached_app_with(bin, &app, &[(olo_xdg::NEW_INSTANCE, "1")]) {
                    tracing::warn!("launch {bin} again: {e}");
                }
            }
            Some(("run", bin)) => spawn(bin),
            Some(("pref", panel)) => spawn(&format!("olo-preferences {panel}")),
            // An agent screen shown on an AirPlay receiver (its tray item's
            // menu).
            Some(("screen-airplay", rest)) => {
                if let Some((name, chosen)) = rest.split_once(':') {
                    olo_kit::airplay::show_from_menu(chosen, &olo_protocol::airplay::Shown::Screen { name: name.to_owned() });
                }
            }
            Some(("appimage", path)) => {
                if let Err(e) = appimages::launch(std::path::Path::new(path)) {
                    tracing::warn!("appimages: {path}: {e}");
                }
            }
            Some(("caff", arg)) => self.caffeine_command(arg),
            // A power mode: set by the daemon (off the UI thread); its
            // signal says so back.
            Some(("power-mode", id)) => {
                if let Some(mode) = olo_system::power_profile::Mode::from_id(id) {
                    if let Some(m) = &mut self.status.power_modes {
                        m.active = mode;
                    }
                    std::thread::spawn(move || {
                        if let Err(e) = olo_system::power_profile::set(mode) {
                            tracing::warn!(?mode, "power mode: {e}");
                        }
                    });
                }
            }
            // Stop Sharing: olo-portal ends the session (and tells the app).
            // Stop Recording: the recorder finishes the file and says so.
            Some(("record", "stop")) => spawn("olo-screenshot --stop"),
            Some(("record", "pause")) => spawn("olo-screenshot --pause"),
            Some(("record", "resume")) => spawn("olo-screenshot --resume"),
            Some(("record", "mark")) => spawn("olo-screenshot --mark"),
            Some(("record", "restart")) => spawn("olo-screenshot --restart"),
            Some(("record", "discard")) => spawn("olo-screenshot --discard"),
            Some(("share", "all")) => {
                for s in &self.status.sharing {
                    spawn(&format!("olo-portal --stop {}", s.session));
                }
            }
            Some(("share", session)) => spawn(&format!("olo-portal --stop {session}")),
            // A film on an AirPlay receiver: its play is told (a process
            // of olo-airplay's, by its number), or every one.
            Some(("airplay", told)) => match told.split_once(':') {
                Some((verb @ ("stop" | "pause" | "resume"), "all")) => spawn(&format!("olo-airplay {verb}")),
                Some((verb @ ("stop" | "pause" | "resume"), pid)) if pid.parse::<u32>().is_ok() => spawn(&format!("olo-airplay {verb} --pid {pid}")),
                _ => {}
            },
            Some(("sound", "mute")) => {
                status::toggle_mute();
                if let Some(v) = &mut self.status.volume {
                    v.muted = !v.muted;
                }
            }
            Some(("sound", what)) => {
                use olo_system::audio::Kind;
                match what.split_once(':') {
                    Some(("out", name)) => status::set_default_device(Kind::Output, name.to_owned()),
                    Some(("in", name)) => status::set_default_device(Kind::Input, name.to_owned()),
                    _ => {}
                }
            }
            // AirPods: the service does it (`olo-airpods --mode anc`, …).
            // Voice processing: the service's own command line, as for an
            // agent (`olo-voice --set noise=strong`).
            Some(("voice", what)) => spawn(&format!("olo-voice --set {what}")),
            // DiskBackup: likewise (`olo-diskbackup --now`, `--set enabled=off`).
            Some(("backup", "now")) => spawn("olo-diskbackup --now"),
            Some(("backup", "stop")) => spawn("olo-diskbackup --stop"),
            Some(("backup", what)) if what.starts_with("enabled=") => spawn(&format!("olo-diskbackup --set {what}")),
            Some(("airpods", what)) => {
                let address = self.status.airpods.as_ref().map(|a| a.address.clone()).unwrap_or_default();
                spawn(&format!("olo-airpods --address {address} --{what}"));
            }
            // The supervisor on or off: its settings, which it follows
            // (shown at once; written off the UI thread).
            Some(("perf", "toggle")) => {
                if let Some(sup) = &mut self.status.supervisor {
                    sup.on = !sup.on;
                    let on = sup.on;
                    std::thread::spawn(move || {
                        let mut s = olo_protocol::performance::PerformanceSettings::load();
                        s.enabled = on;
                        if let Err(e) = s.save() {
                            tracing::warn!("supervisor settings: {e}");
                        }
                    });
                }
            }
            Some(("approve", arg)) => self.approve_command(arg),
            Some(("display", output)) => request(Request::FocusDisplay { output: output.to_owned() }),
            // For the display of the Deskbar it was chosen on, and the
            // workspace whose heading it was chosen from.
            Some(("mode", id)) => {
                if let Some(mode) = WindowingMode::from_id(id) {
                    self.set_mode(self.menu_output.clone(), self.menu_workspace, mode);
                }
            }
            // (At once here too, so the menu shows it before the settings
            // come back.)
            Some(("layout", id)) => {
                if let Some(layout) = olo_protocol::TilingLayout::from_id(id) {
                    self.tiling.layout = layout;
                    request(Request::SetTilingLayout { layout });
                }
            }
            Some(("workspace", n)) => {
                if let Ok(index) = n.parse() {
                    request(Request::SwitchWorkspace { index, output: self.menu_output.clone() });
                }
            }
            Some(("arrange", id)) => request(Request::RunShortcut { id: id.into(), window: self.focused_window() }),
            Some(("notif", "history")) => spawn("olo-notifications --history"),
            Some(("notif", arg)) => self.do_not_disturb(arg),
            // With ⌘ held, a copy of its own beside one running.
            Some(("desktop", id)) => {
                if let Some(e) = self.installed.entries.iter().find(|e| e.id == id) {
                    let _ = if CHOSEN_WITH_COMMAND.with(|c| c.get()) { e.launch_new_instance() } else { e.launch() };
                }
            }
            _ => match cmd {
                "about" => spawn("olo-preferences about"),
                "find" => spawn("olo-tracker --find"),
                "lock" => request(Request::Lock),
                "logout" => request(Request::Quit),
                "restart" => tell(Msg::Effect(Effect::ConfirmPower(Power::Restart))),
                "shutdown" => tell(Msg::Effect(Effect::ConfirmPower(Power::ShutDown))),
                _ => {}
            },
        }
    }

    /// Offer the newer build olo-session found.
    fn ask_upgrade(&mut self) {
        // Already asked for, while a build runs: it says so.
        if self.status.upgrade_waiting {
            let message = "A build of Olo is still running. Olo upgrades as soon as it has finished: your windows stay open.".to_owned();
            return tell(Msg::Effect(Effect::Upgrade { message, waiting: true }));
        }
        // A new app_server means a reload, which programs it can't keep
        // (ones that came before olo-wlproxy, or without it) don't
        // survive.
        let closing: Vec<String> = if self.status.upgrade.iter().any(|p| p == "olo-app-server") {
            self.teams(None, None).into_iter().filter(|t| t.windows.iter().any(|w| !w.kept && our_app(&w.app_id).is_none())).map(|t| t.name).collect()
        } else {
            Vec::new()
        };
        tell(Msg::Effect(Effect::Upgrade { message: upgrade_message(&self.status.upgrade, &closing), waiting: false }));
    }

    /// New word from the notification service: the apps a notification
    /// arrived for since the last shake their dots (not while Do Not
    /// Disturb keeps things quiet).
    fn note_arrivals(&mut self, s: &status::Status) {
        let now = Instant::now();
        self.shaking.retain(|_, at| now.duration_since(*at) < SHAKE);
        let Some(n) = &s.notifications else { return };
        if let Some(seen) = self.notes_seen
            && !n.quiet
        {
            for a in n.apps.iter().filter(|a| a.newest > seen) {
                self.shaking.insert(note_key(a), now);
            }
        }
        let newest = n.apps.iter().map(|a| a.newest).max().unwrap_or_default();
        self.notes_seen = Some(self.notes_seen.map_or(newest, |s| s.max(newest)));
    }

    /// Whether `team`'s application has notifications unread (or a
    /// launcher badge's count, or wants attention): its dot.
    fn notes_of(&mut self, team: &Team) -> Option<Dot> {
        let app_id = team.windows.first().map(|w| w.app_id.clone()).unwrap_or_default();
        let entry = self.entry(&app_id);
        let badge = self.badge_of(team).filter(|b| b.count.is_some() || b.urgent);
        let (apps, fade) = self.status.notifications.as_ref().map_or((&[][..], None), |n| (n.apps.as_slice(), n.fade_minutes));
        let mine: Vec<&NoteApp> = apps.iter().filter(|a| same_app(a, &app_id, entry.as_ref(), &team.name)).collect();
        let mut keys: Vec<String> = mine.iter().map(|a| note_key(a)).collect();
        keys.extend(badge.as_ref().map(|b| badge_key(&b.app)));
        if keys.is_empty() {
            return None;
        }
        let now = Instant::now();
        let shake = keys.iter().filter_map(|k| self.shaking.get(k)).map(|at| now.duration_since(*at).as_secs_f32() / SHAKE.as_secs_f32()).filter(|t| *t < 1.0).reduce(f32::min);
        // Each notification its share, however many apps' they are (a
        // browser's sites); a badge alone is full.
        let secs = now_secs();
        let weight = |a: &NoteApp| a.count.max(1) as f32;
        let total: f32 = mine.iter().map(|a| weight(a)).sum();
        let fresh = if total > 0.0 { mine.iter().map(|a| a.freshness(fade, secs) * weight(a)).sum::<f32>() / total } else { 1.0 };
        let notes: u32 = mine.iter().map(|a| a.count).sum();
        let count = if notes > 0 { notes } else { badge.and_then(|b| b.count).unwrap_or(0) as u32 };
        Some(Dot { shake, fresh, count })
    }

    /// `team`'s application's launcher badge, if it sent one: matched by
    /// its desktop entry, as notifications are.
    fn badge_of(&mut self, team: &Team) -> Option<sni::Badge> {
        let app_id = team.windows.first().map(|w| w.app_id.clone()).unwrap_or_default();
        let entry = self.entry(&app_id);
        self.badges.iter().find(|b| same_app(&NoteApp { desktop_entry: Some(b.app.clone()), ..Default::default() }, &app_id, entry.as_ref(), &team.name)).cloned()
    }

    /// Whether the applications under `display`'s workspace `ws`'s
    /// heading are the workspace showing on the display of the bar being
    /// drawn, which is lit (another display's, listed after its own, is
    /// not: one place lit, where you are).
    fn lit(&self, display: Option<&str>, ws: Option<u32>) -> bool {
        display == self.drawing_for.as_deref() && ws == Some(self.showing(display))
    }

    /// The ground of the rows under workspace `ws`'s heading: the panel,
    /// or the workspace showing's lit ground (`style::lit_ground`).
    fn ground(&self, pal: &Palette, display: Option<&str>, ws: Option<u32>) -> Rgba {
        if self.lit(display, ws) { style::lit_ground(pal, pal.panel_background) } else { pal.panel_background }
    }

    /// Show an application's windows on this workspace together
    /// (minimized ones too), the one it used last in front; with none
    /// here, go to its newest.
    fn bring_forward(&self, team_key_: &str, on: Option<&str>, ws: Option<u32>) {
        let team: Vec<&WindowInfo> = self.windows_on(on).filter(|w| team_key_ == team_key(w) && ws.is_none_or(|ws| w.workspace == ws)).collect();
        // Under another workspace's heading: its windows there (and so
        // to that workspace).
        let at = ws.unwrap_or_else(|| self.showing(on));
        let mut ids: Vec<u64> = team.iter().filter(|w| w.workspace == at).map(|w| w.id).collect();
        if ids.is_empty() {
            ids.extend(team.last().map(|w| w.id));
        }
        if !ids.is_empty() {
            request(Request::ActivateAll { ids });
        }
    }

    /// The application `team_key_` started again as a copy of its own
    /// (⌘-click): from its installed entry.
    fn launch_again(&mut self, team_key_: &str) {
        let Some(app) = self.windows.iter().find(|w| team_key_ == team_key(w)).map(|w| w.app_id.clone()) else { return };
        let started = match self.entry(&app) {
            Some(e) => e.launch_new_instance(),
            // One of Olo's own with no entry installed (a test's home):
            // its program, as Olo names them (olo.terminal, olo-terminal).
            None if olo_xdg::apps::our_app(&app).is_some() => {
                let program = format!("olo-{}", app.strip_prefix("olo.").unwrap_or(&app));
                olo_xdg::spawn_detached_app_with(&program, &app, &[(olo_xdg::NEW_INSTANCE, "1")])
            }
            None => Err(std::io::Error::other("no entry to start it from")),
        };
        if let Err(err) = started {
            tracing::warn!("starting {app} again: {err}");
        }
    }

    fn team_command(&mut self, team_key_: &str, cmd: &str, on: Option<&str>, ws: Option<u32>) {
        let ids: Vec<u64> = self.windows_on(on).filter(|w| team_key_ == team_key(w) && ws.is_none_or(|ws| w.workspace == ws)).map(|w| w.id).collect();
        if let Some(id) = cmd.strip_prefix("win:").and_then(|s| s.parse().ok()) {
            request(Request::Activate { id });
            return;
        }
        if cmd == "group-all" || cmd == "ungroup-all" {
            if let Some(app_id) = self.windows.iter().find(|w| team_key_ == team_key(w)).map(|w| w.app_id.clone()) {
                request(Request::GroupApp { app_id, grouped: cmd == "group-all" });
            }
            return;
        }
        if cmd == "new-window" {
            if let Some(app) = self.windows.iter().find(|w| team_key_ == team_key(w)).map(|w| w.app_id.clone()) {
                std::thread::spawn(move || olo_kit::instance::forward(&app, &["--new-window".to_owned()]));
            }
            return;
        }
        if cmd == "show-all" {
            if !ids.is_empty() {
                request(Request::ActivateAll { ids });
            }
            return;
        }
        if let Some(screen) = olo_kit::moving::screen_command(cmd) {
            if !ids.is_empty() {
                olo_kit::moving::take_to_screen(screen, ids);
            }
            return;
        }
        // Show on AirPlay: the team's one window, or the one chosen.
        let (window, airplay) = match cmd.strip_prefix("airplay-win:").and_then(|r| r.split_once(':')) {
            Some((id, rest)) => (id.parse::<u64>().ok(), olo_protocol::airplay::menu_command(rest)),
            None => (ids.first().copied().filter(|_| ids.len() == 1), olo_protocol::airplay::menu_command(cmd)),
        };
        match airplay {
            Some(Some(receiver)) => {
                if let Some(w) = window.and_then(|id| self.windows.iter().find(|w| w.id == id)) {
                    let what = olo_protocol::airplay::Shown::Window { id: w.id, title: w.title.clone() };
                    if let Err(e) = olo_protocol::airplay::show_on(receiver, &what) {
                        tracing::warn!("couldn't show the window on AirPlay: {e}");
                    }
                }
                return;
            }
            Some(None) => {
                let _ = olo_protocol::airplay::open_settings();
                return;
            }
            None => {}
        }
        if let Some((index, output)) = cmd.strip_prefix("move:").and_then(|r| r.split_once(':')) {
            let Ok(index) = index.parse::<u32>() else { return };
            requests(ids.into_iter().map(|id| Request::MoveToWorkspace { id, index, output: Some(output.to_owned()) }).collect());
            return;
        }
        if let Some(shortcut) = cmd.strip_prefix("shortcut:") {
            requests(ids.into_iter().map(|id| Request::RunShortcut { id: shortcut.to_owned(), window: Some(id) }).collect());
            return;
        }
        for id in ids {
            match cmd {
                "hide-all" => request(Request::Minimize { id }),
                "close-all" => request(Request::Close { id }),
                _ => {}
            }
        }
    }

    /// Files dropped on the team `key` open with its application.
    fn dropped(&mut self, key: &str, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        let Some(handler) = self.handler(key) else { return };
        tracing::debug!(key, ?paths, "dropped");
        std::thread::spawn(move || {
            if let Err(e) = olo_xdg::open::open_with(&handler, &paths) {
                tracing::warn!("couldn't open the dropped files with {}: {e}", handler.name());
            }
        });
    }

    /// Ticks the clock: every bar drawn again on the next minute (or
    /// second, when it shows them), and nothing between.
    /// Takes word from app_server, the watchers and the tray: what the
    /// app is to do about it (every bar drawn again, unless it says
    /// less).
    fn take(&mut self, m: Msg) -> Taken {
        let proxy = olo_kit::proxy();
        match m {
            Msg::Windows(w) => self.set_windows(w),
            Msg::Workspace(cur, count, showing) => {
                self.workspace = (cur, count);
                self.showing = showing;
            }
            Msg::Status(s) => {
                self.note_arrivals(&s);
                self.status = s;
            }
            Msg::Arrangement(undo, redo) => self.arrangement = (undo, redo),
            Msg::Power(p) => {
                // (Nothing read for widgets nobody can see.)
                self.live.pause(p.display_asleep || p.locked, &proxy);
                self.caffeinated = p.caffeinated;
                self.caffeinated_until = p.caffeinated_until;
                self.caffeinated_lid = p.caffeinated_lid;
                self.approve_until = p.auto_approve_until;
            }
            Msg::Settings(s) => {
                let seconds = s.clock.show_seconds != self.clock.show_seconds;
                self.clock = s.clock;
                self.menu_icons = s.menu_icons;
                self.tiling = s.tiling;
                self.keys = s.keys;
                self.live.set(s.widgets, &proxy);
                if seconds {
                    return Taken::Seconds(self.clock.show_seconds);
                }
            }
            // Each bar draws its display's number and name: drawn again
            // now (a display told who it is, the main one changed).
            Msg::Outputs(outputs) => {
                self.outputs = outputs;
                return Taken::Displays;
            }
            Msg::AppImages(found) => self.appimages = found,
            Msg::Installed(installed) => {
                self.installed = installed;
                self.scanning = false;
                self.installed_known = true;
                self.entries.clear();
                self.ask_processes();
            }
            // Only the widgets drawn again (and the tip of one the
            // pointer is on): not the bar.
            Msg::Meters(batch) => {
                self.live.add(batch);
                return Taken::Meters(self.retip());
            }
            Msg::Top(id, top) => {
                self.live.set_top(id, top);
                return Taken::Retip(self.retip());
            }
            Msg::Sources(sources) => self.sources = sources,
            Msg::Tray(tray) => {
                // Pictures no item has any more go.
                self.tray_pictures.retain(|(key, serial, _), _| tray.items.iter().any(|i| i.key == *key && i.serial == *serial));
                self.tray_apps = tray.items;
                // More unread (or a wish for attention) shakes the dot.
                let now = Instant::now();
                for b in &tray.badges {
                    let before = self.badges.iter().find(|o| o.app == b.app);
                    if b.count.unwrap_or(0) > before.and_then(|o| o.count).unwrap_or(0) || (b.urgent && !before.is_some_and(|o| o.urgent)) {
                        self.shaking.insert(badge_key(&b.app), now);
                    }
                }
                self.badges = tray.badges;
            }
            // (The app's own: not the model's.)
            Msg::PowerButton(_) | Msg::Changed | Msg::Tick | Msg::Displays | Msg::Effect(_) | Msg::Approve(_) => {}
        }
        Taken::Changed
    }

    /// What the tip of the widget the pointer is on says now (its
    /// reading and the processes using most): the widget, and its tip.
    fn retip(&self) -> Option<(u32, String)> {
        let id = self.live.hovered()?;
        let w = self.live.widgets.iter().find(|w| w.id == id)?;
        Some((id, self.live.tip(w, &self.sources)))
    }
}

/// What the app does about word the model took.
enum Taken {
    /// Every bar drawn again.
    Changed,
    /// The clock shows seconds now, or doesn't: its tick follows.
    Seconds(bool),
    /// The displays changed: a bar on each, each drawn again.
    Displays,
    /// A reading came: the bars' widgets drawn again, and the tip of the
    /// one the pointer is on.
    Meters(Option<(u32, String)>),
    /// That tip alone.
    Retip(Option<(u32, String)>),
}

/// Whether `app_id` is one of ours that opens a new window when asked.
fn new_window_app(app_id: &str) -> bool {
    app_id.strip_prefix("olo.").is_some_and(|rest| olo_xdg::apps::NEW_WINDOW.contains(&format!("olo-{rest}").as_str()))
}

/// Show beside each of the Olo menu's commands that app_server also has
/// as a shortcut (`find`, `lock`, `arrange:` and a shortcut id) the keys
/// the person has for it.
fn add_keys(menu: &mut Menu, profile: &KeyboardProfile) {
    for item in &mut menu.items {
        match item {
            MenuItem::Action(a) => {
                // A mode's item shows the keys that turn it on or off.
                let id = match a.command.as_str() {
                    "mode:auto_tiling" => "auto-tiling",
                    "mode:focus" => "focus-mode",
                    c => c.strip_prefix("arrange:").unwrap_or(c),
                };
                if let Some(info) = shortcuts::by_id(id) {
                    a.keys = shortcuts::keys_for(info, profile).first().map(|c| c.label(profile.platform));
                }
            }
            MenuItem::Submenu(m) => add_keys(m, profile),
            MenuItem::Separator => {}
        }
    }
}

fn settings_msg(settings: olo_protocol::Settings) -> Msg {
    Msg::Settings(Settings { clock: settings.clock, menu_icons: settings.appearance.menu_icons, tiling: settings.tiling, keys: settings.keyboard.profile, widgets: settings.deskbar.widgets })
}

/// `menu` and its submenus with no icons (Appearance ▸ Show icons in the
/// Deskbar menu, off).
fn without_icons(mut menu: Menu) -> Menu {
    menu.icon = None;
    menu.items = menu
        .items
        .into_iter()
        .map(|item| match item {
            MenuItem::Action(mut a) => {
                a.icon = None;
                MenuItem::Action(a)
            }
            MenuItem::Submenu(m) => MenuItem::Submenu(without_icons(m)),
            sep => sep,
        })
        .collect();
    menu
}

/// Each share, said, with Stop Sharing under it.
/// DiskBackup's menu, as it is now (its state read as it opens: what it's
/// doing and did, then Back Up Now or Stop, Pause or Resume, Restore
/// Files…, the app).
fn backup_menu(clock: &olo_protocol::ClockSettings) -> Menu {
    use olo_protocol::diskbackup::{Doing, Settings, State};
    let (s, st) = (Settings::load(), State::load());
    let now = now_secs() as i64;
    // "at 14:05", "yesterday at 14:05", "tomorrow at 9:00", "3 days ago".
    let when = |t: i64| {
        let at = status::time_of_day(t.max(0) as u64, clock);
        match status::local_day(now) - status::local_day(t) {
            0 => format!("at {at}"),
            1 => format!("yesterday at {at}"),
            -1 => format!("tomorrow at {at}"),
            d if d > 0 => format!("{d} days ago"),
            d => format!("in {} days", -d),
        }
    };
    let count = |n: u64| {
        let digits = n.to_string();
        let mut grouped = String::new();
        for (i, ch) in digits.chars().enumerate() {
            if i > 0 && (digits.len() - i).is_multiple_of(3) {
                grouped.push(',');
            }
            grouped.push(ch);
        }
        if n == 1 { "1 file".to_owned() } else { format!("{grouped} files") }
    };
    let mut said: Vec<String> = Vec::new();
    match &st.doing {
        // What, how far through how much, how long, files, what's new
        // and sent: DiskBackup's own words (its window says the same).
        Doing::BackingUp => said.extend(st.progress.clone().unwrap_or_default().lines(&s, now)),
        Doing::Pruning => said.push("Tidying: letting go of old snapshots".into()),
        Doing::Restoring => said.push("Restoring".into()),
        Doing::Erasing => said.push("Erasing every backup, to start fresh".into()),
        Doing::Validating => said.push("Validating backups: reading them back".into()),
        Doing::Deleting { what } => {
            let p = st.progress.clone().unwrap_or_default();
            said.push(if p.total_files > 0 { format!("Deleting {what}: {} of {}", count(p.files), count(p.total_files)) } else { format!("Deleting {what}") });
        }
        Doing::Waiting { why } => said.push(format!("Waiting: {why}")),
        Doing::Idle if st.pid == 0 => said.push("The DiskBackup service isn’t running".into()),
        Doing::Idle => {}
    }
    match (&st.last, st.last_ok) {
        // Stopped by the person: calmly, and what it got through.
        (Some(l), ok) if l.stopped() && st.doing == Doing::Idle => {
            said.push(format!("You stopped the backup {}", when(l.time)));
            if let Some(u) = &st.unfinished {
                said.push(if u.files > 0 {
                    format!("{} ({}) done so far: Back Up Now goes on", olo_protocol::diskbackup::words::plural(u.files, "file"), olo_protocol::diskbackup::words::size(u.bytes))
                } else {
                    "Back Up Now goes on from where it got to".into()
                });
            }
            if let Some(t) = ok {
                said.push(format!("Last backed up whole {}", when(t)));
            }
        }
        (Some(l), ok) if l.error.is_some() && st.doing == Doing::Idle => {
            let mut why = l.error.clone().unwrap_or_default();
            if why.chars().count() > 60 {
                why = format!("{}…", why.chars().take(59).collect::<String>());
            }
            said.push(format!("The backup {} didn’t finish: {why}", when(l.time)));
            if let Some(t) = ok {
                said.push(format!("Last backed up whole {}", when(t)));
            }
        }
        (_, Some(t)) if st.doing == Doing::BackingUp => said.push(format!("Last backed up {}", when(t))),
        (_, Some(t)) => said.push(format!("Backed up {}", when(t))),
        _ => said.push("Not backed up yet".into()),
    }
    if let Some(n) = st.next.filter(|_| st.doing == Doing::Idle) {
        said.push(format!("Next {}", when(n)));
    }
    let mut m = Menu::new("DiskBackup");
    for line in said {
        m = m.item(line, "").enabled(false);
    }
    let busy = matches!(st.doing, Doing::BackingUp | Doing::Pruning | Doing::Validating | Doing::Deleting { .. });
    m = m.separator();
    let stop = match st.doing {
        Doing::Pruning => "Stop Tidying",
        Doing::Validating => "Stop Validating",
        Doing::Deleting { .. } => "Stop Deleting",
        _ => "Stop Backing Up",
    };
    m = if busy { m.item(stop, "backup:stop") } else { m.item("Back Up Now", "backup:now").enabled(st.pid != 0 && s.enabled) };
    m = if s.enabled { m.item("Pause Backups", "backup:enabled=off") } else { m.item("Resume Backups", "backup:enabled=on") };
    m.separator().item("Restore Files…", "run:olo-diskbackup --restore-window").item("DiskBackup…", "run:olo-diskbackup")
}

/// What a play is, in a line: "A film.mp4 on Front Room".
fn play_said(p: &olo_protocol::airplay::Playing) -> String {
    let doing = match p.state.as_str() {
        "paused" => " (paused)",
        "loading" => " (loading)",
        _ => "",
    };
    format!("{} on {}{doing}", p.title, p.receiver)
}

/// Whether a play is going on (a film playing, a screen showing), and
/// can be paused.
fn play_going(p: &olo_protocol::airplay::Playing) -> bool {
    p.state == "playing" || p.state == "showing"
}

/// The films playing on AirPlay receivers: each with Pause (or Resume)
/// and Stop.
fn airplay_menu(plays: &[olo_protocol::airplay::Playing]) -> Menu {
    let mut m = Menu::new("AirPlay");
    for (i, p) in plays.iter().enumerate() {
        if i > 0 {
            m = m.separator();
        }
        m = m.item(play_said(p), "").enabled(false);
        m = if p.paused() { m.item("Resume", format!("airplay:resume:{}", p.pid)) } else { m.item("Pause", format!("airplay:pause:{}", p.pid)).enabled(play_going(p)) };
        m = m.item("Stop", format!("airplay:stop:{}", p.pid));
    }
    if plays.len() > 1 {
        m = m.separator().item("Stop All", "airplay:stop:all");
    }
    m.separator().item("AirPlay Preferences…", "pref:airplay")
}

/// The agent screens' menu (right click): watch them, or show one on an
/// AirPlay receiver.
fn screens_menu(screens: &[olo_kit::moving::RunningScreen], paired: &[olo_protocol::airplay::Paired]) -> Menu {
    let m = Menu::new("Agent Screens").item(if screens.len() == 1 { "Watch Agent Screen" } else { "Watch Agent Screens" }, "run:olo-screen watch").separator();
    match screens {
        [one] => m.submenu(Menu::airplay("Show on AirPlay", paired, &format!("screen-airplay:{}:", one.name))),
        many => {
            let mut each = Menu::new("Show on AirPlay");
            for s in many {
                each = each.submenu(Menu::airplay(&s.label, paired, &format!("screen-airplay:{}:", s.name)));
            }
            m.submenu(each)
        }
    }
}

fn sharing_menu(shares: &[olo_protocol::sharing::Share]) -> Menu {
    let mut m = Menu::new("Sharing");
    for (i, s) in shares.iter().enumerate() {
        if i > 0 {
            m = m.separator();
        }
        // (A program controlling the computer: the RemoteDesktop portal.)
        let stop = if s.control.is_empty() { "Stop Sharing" } else { "Stop Remote Control" };
        m = m.item(olo_protocol::SharingState::said(s), "").enabled(false).item(stop, format!("share:{}", s.session));
    }
    if shares.len() > 1 {
        m = m.separator().item("Stop All", "share:all");
    }
    m
}

/// The speaker's menu (right click): where sound goes and comes from,
/// mute, and while AirPods play, their levels, noise control, listening
/// features and quality, and the way to the settings.
fn sound_menu(devices: Option<&olo_system::audio::Devices>, pods: Option<&olo_protocol::airpods::AirPods>, muted: bool, voice: &olo_protocol::voice::Settings) -> Menu {
    let mut m = Menu::new("Sound");
    if let Some(d) = devices {
        m = m.item("Output", "").enabled(false);
        // None: sound goes nowhere (never forced onto the only device).
        m = m.item("None", format!("sound:out:{}", olo_system::audio::NO_OUTPUT)).checked(d.no_output);
        for o in &d.outputs {
            m = m.item(o.description.clone(), format!("sound:out:{}", o.name)).checked(o.is_default && !d.no_output);
        }
        m = m.separator().item("Input", "").enabled(false);
        for i in &d.inputs {
            m = m.item(i.description.clone(), format!("sound:in:{}", i.name)).checked(i.is_default);
        }
        m = m.separator();
    }
    m = m.item("Mute", "sound:mute").checked(muted);
    m = voice_items(m.separator(), voice);
    let playing = |p: &&olo_protocol::airpods::AirPods| devices.and_then(|d| d.default_device(olo_system::audio::Kind::Output)).is_some_and(|o| o.name == p.sound.output);
    if let Some(p) = pods.filter(playing) {
        m = airpods_items(m.separator(), p);
        if p.abilities.adaptive
            && p.linked
            && let Some(on) = p.settings.personalized_volume
        {
            m = m.item("Personalized Volume", format!("airpods:set personalized-volume {}", if on { "off" } else { "on" })).checked(on);
        }
        if !p.sound.card.is_empty() {
            m = m.item("Music Quality (AAC)", "airpods:quality music").checked(p.sound.high_quality).item("Call Quality, with Microphone", "airpods:quality calls").checked(!p.sound.high_quality);
        }
        m = m.separator().item("AirPods Settings…", "pref:airpods");
    } else {
        m = m.separator();
    }
    m.item("Sound Settings…", "pref:sound")
}

/// Voice processing (olo-voice): on or off, and in a submenu what for,
/// how much noise, echo and level.
fn voice_items(m: Menu, v: &olo_protocol::voice::Settings) -> Menu {
    use olo_protocol::voice::{Mode, Model, Noise};
    let flip = |key: &str, on: bool| format!("voice:{key}={}", if on { "off" } else { "on" });
    let m = m.item("Clean Up Microphone Sound", flip("enabled", v.enabled)).checked(v.enabled);
    if !v.enabled {
        return m;
    }
    let mut sub = Menu::new("Voice Processing");
    for mode in Mode::ALL {
        sub = sub.item(format!("For {}", mode.label()), format!("voice:mode={}", mode.label().to_lowercase())).checked(v.mode == mode);
    }
    sub = sub.separator().item("Background Noise", "").enabled(false);
    for n in Noise::ALL {
        let word = match n {
            Noise::Full => "full".to_owned(),
            _ => n.label().to_lowercase(),
        };
        sub = sub.item(n.label(), format!("voice:noise={word}")).checked(v.noise == n);
    }
    sub = sub.separator().item("Noise Model", "").enabled(false);
    for model in Model::ALL {
        sub = sub.item(model.label(), format!("voice:model={}", model.word())).checked(v.model == model).enabled(v.noise != Noise::Off);
    }
    sub = sub
        .separator()
        .item("Cancel Echo from the Speakers", flip("echo", v.echo_cancellation))
        .checked(v.echo_cancellation)
        .item("Even Out the Voice's Level", flip("normalize", v.normalize))
        .checked(v.normalize);
    m.submenu(sub)
}

/// AirPods' tray menu: their levels, noise control, Conversation
/// Awareness, and the way to their settings and the sound's.
fn airpods_menu(pods: &olo_protocol::airpods::AirPods) -> Menu {
    airpods_items(Menu::new("AirPods"), pods).separator().item("Disconnect", "airpods:disconnect").item("AirPods Settings…", "pref:airpods").item("Sound Settings…", "pref:sound")
}

/// AirPods' name and levels, noise control and Conversation Awareness,
/// added to `m` (their own menu, and the speaker's).
fn airpods_items(mut m: Menu, pods: &olo_protocol::airpods::AirPods) -> Menu {
    use olo_protocol::airpods::ListeningMode;
    m = m.item(pods.name.clone(), "").enabled(false);
    let levels = pods.levels();
    if !levels.is_empty() {
        m = m.item(levels, "").enabled(false);
    }
    if pods.abilities.noise_control && pods.linked {
        m = m.separator();
        for mode in ListeningMode::ALL.into_iter().filter(|x| *x != ListeningMode::Adaptive || pods.abilities.adaptive) {
            m = m.item(mode.label(), format!("airpods:mode {}", mode.word())).checked(pods.settings.listening_mode == Some(mode));
        }
    }
    if let Some(on) = pods.settings.conversation_awareness.filter(|_| pods.linked) {
        m = m.separator().item("Conversation Awareness", format!("airpods:set conversation-awareness {}", if on { "off" } else { "on" })).checked(on);
    }
    m
}

/// The supervisor's tray menu: on or off, what it found (each to its
/// process in Activity Monitor), and the way to all of it.
fn supervisor_menu(sup: &status::Supervisor) -> Menu {
    const SHOWN: usize = 6;
    let mut m = Menu::new("Supervisor").item("Keep the Desktop Responsive", "perf:toggle").checked(sup.on).separator();
    let st = &sup.state;
    if !sup.on {
        m = m.item("Off", "").enabled(false);
    } else if st.findings.is_empty() {
        m = m.item("Nothing Found", "").enabled(false);
    }
    if sup.on {
        for f in st.findings.iter().take(SHOWN) {
            m = m.item(format!("{}: {} ({}){}", f.kind.title(), f.name, f.pid, if f.urgent { ", Urgent" } else { "" }), format!("run:olo-activity --inspect {}", f.pid));
        }
        if st.findings.len() > SHOWN {
            m = m.item(format!("And {} More", st.findings.len() - SHOWN), "run:olo-activity --section supervisor");
        }
        if !st.lowered.is_empty() {
            let n = st.lowered.len();
            m = m.item(format!("{n} Background Process{} Lowered", if n == 1 { "" } else { "es" }), "run:olo-activity --section supervisor");
        }
    }
    m.separator().item("Show in Activity Monitor", "run:olo-activity --section supervisor").item("Performance Settings…", "pref:performance")
}

/// A link's tray icon: a click (either button) opens the link's menu,
/// read as it opens, which ends with Network's preferences on its tab.
fn link_hit(h: Element, r: Bounds<Pixels>, kind: olo_system::network::Kind, model: &Model, view: &ViewRef<BarView>) -> Element {
    let (m, v) = (model.clone(), view.clone());
    h.on_any_button(move |e, cx| match button(e) {
        MouseButton::Left | MouseButton::Right => {
            let mm = m.clone();
            let key = if kind == olo_system::network::Kind::Wifi { "tray:wifi" } else { "tray:ethernet" };
            open_menu(cx, &v, key, r, link_menu(kind, &status::links()), move |cmd, cx| run(&mm, cmd, cx));
        }
        _ => {}
    })
}

/// A link's menu: what it's joined to, whether it carries the traffic
/// (the default route) or stands by, its addresses, router, DNS servers,
/// rate and what has gone through it (the interface's counters, read as
/// it opens), as a Mac's Option-click on Wi-Fi shows; and Network's
/// preferences.
fn link_menu(kind: olo_system::network::Kind, l: &olo_system::network::Links) -> Menu {
    use olo_system::network::{self as net, Kind};
    let row = |m: Menu, text: String| m.item(text, "").enabled(false);
    let (name, other, dev) = match kind {
        Kind::Wifi => ("Wi-Fi", "Ethernet", l.wifi_device.as_ref()),
        _ => ("Ethernet", "Wi-Fi", l.wired.as_ref()),
    };
    let mut m = Menu::new(name);
    let joined = match kind {
        Kind::Wifi => l.wifi.as_ref().and_then(|w| w.ssid.clone()),
        _ => dev.and_then(|d| d.connection.clone()),
    };
    m = row(
        m,
        match &joined {
            Some(j) => format!("{name}: {j}"),
            None => format!("{name}: Not Connected"),
        },
    );
    if let Some(d) = dev.filter(|d| d.connected()) {
        match l.primary {
            Some(p) if p == kind => m = row(m, "In Use for Internet Traffic".into()),
            Some(_) => m = row(m, format!("Standing By: {other} Is in Use")),
            None => {}
        }
        m = m.separator();
        if kind == Kind::Wifi
            && let Some(w) = &l.wifi
        {
            m = row(m, format!("Signal: {}%", w.strength));
        }
        if let Some(s) = d.speed {
            m = row(m, format!("{}: {}", if kind == Kind::Wifi { "Rate" } else { "Speed" }, net::speed_text(s)));
        }
        let bare: Vec<&str> = d.addresses.iter().map(|a| a.split('/').next().unwrap_or(a)).collect();
        if !bare.is_empty() {
            m = row(m, format!("IP Address: {}", bare.join(", ")));
        }
        if let Some(g) = &d.gateway {
            m = row(m, format!("Router: {g}"));
        }
        if !d.dns.is_empty() {
            m = row(m, format!("DNS: {}", d.dns.join(", ")));
        }
        m = row(m, format!("Interface: {}", d.interface));
        if !d.hw_address.is_empty() {
            m = row(m, format!("Hardware Address: {}", d.hw_address));
        }
        if let Some((received, sent)) = net::traffic(&d.interface) {
            use olo_kit::files::format_size;
            m = row(m, format!("Received: {}, Sent: {}", format_size(received), format_size(sent)));
        }
    }
    // (On the link's own tab.)
    m.separator().item("Network Preferences…", if kind == Kind::Wifi { "pref:network --tab Wi-Fi" } else { "pref:network --tab Ethernet" })
}

/// The bell's menu.
fn notifications_menu(quiet: bool) -> Menu {
    Menu::new("Notifications")
        .item("Show History", "notif:history")
        .separator()
        .item("Do Not Disturb", if quiet { "notif:off" } else { "notif:on" })
        .checked(quiet)
        .item("Quiet for 1 Hour", "notif:60")
        .item("Quiet Until Morning", "notif:tomorrow")
        .separator()
        .item("Notification Settings…", "pref:notifications")
}

/// "Terminal, Deskbar and 3 others", for the upgrade alert.
fn upgrade_message(programs: &[String], closing: &[String]) -> String {
    let names: Vec<String> = programs
        .iter()
        .map(|bin| match OUR_APPS.iter().find(|(b, _, _)| b == bin) {
            Some((_, name, _)) => name.to_string(),
            None => match bin.as_str() {
                "olo-deskbar" => "Deskbar".into(),
                "olo-app-server" => "the window server".into(),
                "olo-session" => "the session".into(),
                "olo-preferences" => "Preferences".into(),
                other => other.to_string(),
            },
        })
        .collect();
    let list = listing(&names);
    if closing.is_empty() {
        return format!("A new build of Olo is ready ({list}). Install it now? Your windows stay open.");
    }
    format!("A new build of Olo is ready ({list}). Installing it restarts the window server: Olo's windows stay open, but {} will close.", listing(closing))
}

/// "A", "A and B", "A, B and C", "A, B, C and 2 others".
fn listing(names: &[String]) -> String {
    match names.len() {
        0 => "nothing".to_string(),
        1 => names[0].clone(),
        2..=4 => format!("{} and {}", names[..names.len() - 1].join(", "), names[names.len() - 1]),
        n => format!("{} and {} others", names[..3].join(", "), n - 3),
    }
}

// ---- the bars -----------------------------------------------------------------

/// Opens a Deskbar on `output` (None: where app_server puts it, until the
/// displays are known).
/// Opens a Deskbar on display `output`.
fn open_bar(model: Model, output: Option<String>) -> WindowHandle<BarView> {
    let height = TILE_H + tray_height(1);
    let on = output.clone();
    let options = WindowOptions::new("Deskbar", WIDTH, height).layer("deskbar", LayerShellLayer::Top, (true, false, false, true)).layer_options(move |l| {
        l.keyboard = LayerKeyboard::None;
        l.output = on;
    });
    olo_kit::open_window(options, move |cx| {
        let shelf = widgets::Shelf::default();
        let layer = cx.new_view(|_| widgets::Layer::new(shelf.clone(), model.clone()));
        BarView { model, output, hot: None, open: None, press: None, height, scroll: 0.0, fading: false, shelf, layer, confirm: None, upgrade_ask: None }
    })
}

/// A press on an application that may become a drag out of it.
#[derive(Clone)]
struct Press {
    at: Point<Pixels>,
    team: String,
    icon: String,
    name: String,
}

/// A Deskbar on a display: what the pointer is doing over it.
struct BarView {
    model: Model,
    /// The display it is on (a connector); None for one opened before the
    /// displays were known (app_server keeps it on the main display).
    output: Option<String>,
    /// What the pointer is over, by its key.
    hot: Option<SharedString>,
    /// What a menu is open from.
    open: Option<SharedString>,
    press: Option<Press>,
    /// Its height, as last asked of the compositor.
    height: f32,
    /// Wheel movement over the volume, towards the next 5% step.
    scroll: f32,
    /// Drawn again once a dot has had time to drain a device pixel (one
    /// wait at a time).
    fading: bool,
    /// Where its widgets are, for the layer over it.
    shelf: widgets::Shelf,
    /// Its live widgets, drawn over it: a view of their own, so a
    /// reading (or their moving) draws only them.
    layer: Child<widgets::Layer>,
    /// The Restart / Shut Down confirmation, while it is open (the main
    /// display's bar opens them).
    confirm: Option<LocalWindow<Alert>>,
    /// The "upgrade and reload?" alert, while it is open.
    upgrade_ask: Option<LocalWindow<Alert>>,
}

/// What a bar is told.
enum BarMsg {
    /// The model changed (or a dot is shaking): drawn again.
    Changed,
    /// A reading came: the widgets drawn again, and this tip said again
    /// if the pointer is on its widget.
    Meters(Option<(u32, String)>),
    Retip(Option<(u32, String)>),
    /// Something to ask the person.
    Effect(Effect),
    /// A dot has had time to drain.
    Faded,
}

impl BarView {
    fn set_hot(&mut self, key: &SharedString, on: bool, cx: &mut Cx<Self>) {
        let hot = if on {
            Some(key.clone())
        } else if self.hot.as_ref() == Some(key) {
            None
        } else {
            return;
        };
        if hot != self.hot {
            // The widget the pointer is on, whose tip lists processes.
            let (was, now) = (self.hot.as_ref().and_then(|k| widget_of(k)), hot.as_ref().and_then(|k| widget_of(k)));
            if was != now {
                self.model.read().live.hover(now);
            }
            self.hot = hot;
            cx.invalidate();
        }
    }

    /// The tip of the widget the pointer is on, if it's this one, says
    /// what it does now (drawn again too, so its next tip starts from
    /// it).
    fn retip(&mut self, tip_now: Option<(u32, String)>, cx: &mut Cx<Self>) {
        let Some((id, text)) = tip_now else { return };
        let Some(key) = self.hot.clone().filter(|k| widget_of(k) == Some(id)) else { return };
        cx.invalidate();
        tip::retext(cx, key, text);
    }

    /// Asks the person before something is done.
    fn ask(&mut self, effect: Effect, cx: &mut Cx<Self>) {
        match effect {
            // Ask before restarting or shutting down, as BeOS did.
            Effect::ConfirmPower(action) => {
                if let Some(w) = self.confirm.take() {
                    w.close(cx);
                }
                self.confirm = olo_kit::widgets::open_alert(cx, action.label(), "system", action.question(), &["Cancel", action.label()], move |choice, _| {
                    if *choice == Some(1) {
                        // logind may take a moment (and polkit may ask): not on the UI thread.
                        std::thread::spawn(move || {
                            let r = match action {
                                Power::Restart => olo_system::power::reboot(),
                                Power::ShutDown => olo_system::power::power_off(),
                            };
                            if let Err(e) = r {
                                tracing::warn!("{}: {e}", action.label());
                            }
                        });
                    }
                });
            }
            Effect::Upgrade { message, waiting } => {
                if let Some(w) = self.upgrade_ask.take() {
                    w.close(cx);
                }
                let buttons: &[&str] = if waiting { &["OK"] } else { &["Later", "Upgrade"] };
                self.upgrade_ask = olo_kit::widgets::open_alert(cx, "Upgrade Olo", "system", &message, buttons, move |choice, _| {
                    if *choice == Some(1) && !waiting {
                        // olo-session installs the programs and reloads in place.
                        match std::env::var("OLO_SESSION_PID").ok().and_then(|p| p.parse::<i32>().ok()) {
                            // SAFETY: sending a signal.
                            Some(pid) if unsafe { libc::kill(pid, libc::SIGUSR2) } == 0 => {}
                            _ => tracing::warn!("no session to upgrade"),
                        }
                    }
                });
            }
        }
    }
}

/// The live widget a bar's element is ("widget:3", "tray:widget:3").
fn widget_of(key: &str) -> Option<u32> {
    key.strip_prefix("tray:").unwrap_or(key).strip_prefix("widget:")?.parse().ok()
}

/// What a bar's drawing needs to know of the pointer.
#[derive(Clone)]
struct Pointer {
    hot: Option<SharedString>,
    open: Option<SharedString>,
}

impl Pointer {
    fn hot(&self, key: &str) -> bool {
        self.hot.as_ref().is_some_and(|h| h.as_ref() == key)
    }

    fn open(&self, key: &str) -> bool {
        self.open.as_ref().is_some_and(|o| o.as_ref() == key)
    }
}

/// Drawing a bar: what to paint, in order, and where the pointer can do
/// things.
struct Drawing {
    paint: Vec<Box<dyn Fn(&mut Painter)>>,
    hits: Vec<Element>,
    /// A dot is shaking: drawn again next frame.
    animating: bool,
    /// A dot has begun to drain.
    fading: bool,
    /// The live widgets, drawn over the bar (`widgets::Layer`).
    placed: Vec<widgets::Placed>,
}

impl Drawing {
    fn paint(&mut self, f: impl Fn(&mut Painter) + 'static) {
        self.paint.push(Box::new(f));
    }
}

fn rect(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
    Bounds { origin: point(px(x), px(y)), size: size(px(w), px(h)) }
}

fn expand(r: Bounds<Pixels>, by: f32) -> Bounds<Pixels> {
    Bounds { origin: point(r.left() - px(by), r.top() - px(by)), size: size(r.size.width + px(2.0 * by), r.size.height + px(2.0 * by)) }
}

/// The mouse button used on an area of the bar.
fn button(e: &MouseButton) -> MouseButton {
    *e
}

/// The buttons on an area of the bar: a left click (pressed and let go
/// on it), and the right and middle buttons as they're pressed (menus
/// open on the press, as BeOS's do).
trait Buttons {
    fn on_any_button(self, f: impl Fn(&MouseButton, &mut WindowCx) + 'static) -> Self;
}

impl Buttons for Element {
    fn on_any_button(self, f: impl Fn(&MouseButton, &mut WindowCx) + 'static) -> Self {
        let f = std::rc::Rc::new(f);
        let (left, right, middle) = (f.clone(), f.clone(), f);
        self.on_click(move |e: &ClickEvent, cx| {
            if e.standard_click() {
                left(&MouseButton::Left, cx)
            }
        })
        .on_mouse_down(MouseButton::Right, move |_, cx| right(&MouseButton::Right, cx))
        .on_mouse_down(MouseButton::Middle, move |_, cx| middle(&MouseButton::Middle, cx))
    }
}

/// An area of the bar the pointer can use, `key` its name: hovering it
/// lights it (and shows `tip` a moment later, in a popup of its own:
/// the bar is narrower than what it says), a press puts the tip away.
fn hit(view: &ViewRef<BarView>, key: &str, r: Bounds<Pixels>, tip_text: Option<String>) -> Element {
    let key = SharedString::from(key.to_owned());
    let (v, k, t) = (*view, key.clone(), tip_text);
    let (pk, rk) = (key.clone(), key.clone());
    div()
        .id(key)
        .absolute()
        .left(r.left())
        .top(r.top())
        .w(r.size.width)
        .h(r.size.height)
        .on_hover(move |on: &bool, cx| {
            let (key, on) = (k.clone(), *on);
            v.update(cx, move |v, cx| v.set_hot(&key, on, cx));
            if let Some(t) = &t {
                tip::hover(cx, k.clone(), t.clone(), r, on);
            }
        })
        .on_mouse_down(MouseButton::Left, move |_, cx| tip::pressed(cx, pk.clone()))
        .on_mouse_down(MouseButton::Right, move |_, cx| tip::pressed(cx, rk.clone()))
}

/// Whether Super (⌘ on a Mac's keys) or Control was held as a menu's item
/// was chosen or a row clicked: an app started then is a copy of its own
/// (`DesktopEntry::launch_new_instance`). Deskbar never has the keyboard,
/// so Wayland doesn't say: app_server is asked (`HeldModifiers`), at
/// the click only.
fn command_held() -> bool {
    let asked = olo_protocol::Client::connect().ok().and_then(|mut c| c.request(&Request::HeldModifiers).ok());
    matches!(asked, Some(olo_protocol::Reply::HeldModifiers { ctrl, logo, .. }) if ctrl || logo)
}

thread_local! {
    /// The command key was held as the menu item being run was chosen.
    static CHOSEN_WITH_COMMAND: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Opens `menu` under `r`, from the bar's element `key` (shown open
/// until it goes); commands go to `then`.
fn open_menu(cx: &mut WindowCx, view: &ViewRef<BarView>, key: &str, r: Bounds<Pixels>, menu: Menu, then: impl Fn(&str, &mut WindowCx) + 'static) {
    let key = SharedString::from(key.to_owned());
    let (v, k) = (*view, key.clone());
    let handle = menu::popup_then(
        cx,
        menu,
        r,
        Corner::BottomLeft,
        move |cmd, cx| {
            CHOSEN_WITH_COMMAND.with(|c| c.set(command_held()));
            then(cmd, cx);
            CHOSEN_WITH_COMMAND.with(|c| c.set(false));
        },
        move |cx| {
            let k = k.clone();
            v.update(cx, move |v, cx| {
                if v.open.as_ref() == Some(&k) {
                    v.open = None;
                    cx.invalidate();
                }
            })
        },
    );
    if handle.is_some() {
        view.update(cx, move |v, cx| {
            v.open = Some(key);
            cx.invalidate();
        });
    }
}

impl View for BarView {
    type Message = BarMsg;

    fn message(&mut self, msg: BarMsg, cx: &mut Cx<Self>) {
        match msg {
            BarMsg::Changed => cx.invalidate(),
            BarMsg::Meters(tip_now) => {
                self.layer.update(cx, |_, cx| cx.invalidate());
                self.retip(tip_now, cx);
            }
            BarMsg::Retip(tip_now) => self.retip(tip_now, cx),
            BarMsg::Effect(effect) => self.ask(effect, cx),
            BarMsg::Faded => {
                self.fading = false;
                cx.invalidate();
            }
        }
    }

    /// Closed under it (the compositor took its display's surface): the
    /// app opens another as soon as its display is listed again.
    fn window_closing(&mut self, _: &mut Cx<Self>) {
        tell(Msg::Displays);
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let view = cx.view_ref();
        let pointer = Pointer { hot: self.hot.clone(), open: self.open.clone() };
        let (output, model) = (self.output.clone(), self.model.clone());
        let (mut drawing, height) = model.read().draw_bar(output.as_deref(), &pointer, &view, &model, cx);
        self.shelf.set(std::mem::take(&mut drawing.placed));
        // Its height follows what it shows (the compositor sizes a layer
        // surface).
        if (height - self.height).abs() > 0.5 {
            self.height = height;
            cx.resize_window(size(px(WIDTH), px(height)));
        }
        // (A dot shaking: drawn again next frame.)
        if drawing.animating {
            cx.after(Duration::from_millis(16), BarMsg::Changed);
        }
        if drawing.fading && !self.fading {
            self.fading = true;
            cx.after(Duration::from_secs(20), BarMsg::Faded);
        }
        let paint = drawing.paint;
        div()
            .relative()
            .w(px(WIDTH))
            .h(px(height))
            .child(
                canvas(move |g, _| {
                    for f in &paint {
                        f(g);
                    }
                })
                .absolute()
                .inset_0(),
            )
            .children(drawing.hits)
            .child(div().absolute().inset_0().child(&self.layer))
    }
}

impl Deskbar {
    /// Draws the bar on `output`: the Olo tile, the tray, and the
    /// applications by display and workspace. Its height.
    fn draw_bar(&mut self, output: Option<&str>, pointer: &Pointer, view: &ViewRef<BarView>, model: &Model, cx: &Cx<BarView>) -> (Drawing, f32) {
        let model = model.clone();
        let p = theme::palette();
        let display = self.shown_on(output);
        self.drawing_for = display.clone();
        let mut d = Drawing { paint: Vec::new(), hits: Vec::new(), animating: false, fading: false, placed: Vec::new() };

        // ---- the Olo tile
        let tile = rect(0.0, 0.0, WIDTH, TILE_H);
        let mode = self.mode(display.as_deref(), self.showing(display.as_deref()));
        let hot = pointer.hot("tile") || pointer.open("tile");
        d.paint(move |g| {
            let p = theme::palette();
            style::vgradient(g, tile, style::tint(p.panel_background, if hot { 0.75 } else { 0.85 }), shade(&p, p.panel_background, 1.04));
            section_bevel(g, tile, &p);
            // The wordmark alone, centred (beside the mark it read as "o olo").
            let word = rect(0.0, f32::from(tile.center().y) - 9.0, WIDTH, 18.0);
            if let olo_kit::brand::Drawn::Missing = olo_kit::brand::wordmark_centred(g, word, status::dark(&p)) {
                style::label(g, tile.center().x, tile.center().y, Align::Center, "Olo", 14.0, style::ui_font(FontWeight::BOLD), c(p.panel_text));
            }
            agent::record(g, agent::Node { role: "Button", name: Some("Olo".into()), bounds: tile, enabled: true, clickable: true, ..Default::default() });
        });
        {
            let (m, v, dsp) = (model.clone(), view.clone(), display.clone());
            d.hits.push(hit(view, "tile", tile, None).on_any_button(move |e, cx| {
                if button(e) != MouseButton::Left {
                    return;
                }
                let menu = m.update(|d| {
                    d.menu_output = dsp.clone();
                    d.menu_workspace = None;
                    d.olo_menu(mode)
                });
                let m2 = m.clone();
                open_menu(cx, &v, "tile", tile, menu, move |cmd, cx| run(&m2, cmd, cx));
            }));
        }

        // ---- tray: the date and time, then the status icons
        let st = self.status.clone();
        let clock = self.clock.clone();
        let mut apps: Vec<sni::Item> = self.tray_apps.iter().filter(|i| i.status != sni::Status::Passive).cloned().collect();
        // Untitled (Electron's): named by their program, as its row is.
        for it in apps.iter_mut().filter(|i| i.title.is_empty() && !i.program.is_empty()) {
            it.title = self.app_name(&it.program.clone()).0;
        }
        let px_size = (ICON * cx.scale_factor()).round() as u32;
        let pictures: Vec<Option<Arc<RenderImage>>> =
            apps.iter().map(|it| self.tray_pictures.entry((it.key.clone(), it.serial, px_size)).or_insert_with(|| sni::picture(it, px_size)).clone()).collect();
        // This Deskbar's display, with several: its number, to go to another.
        let display_number = display.as_deref().and_then(|dn| olo_protocol::display_number(&self.outputs, dn));
        let tray_widgets: Vec<olo_protocol::deskbar::Widget> = self.live.widgets.iter().filter(|w| w.place == olo_protocol::deskbar::Place::Tray).cloned().collect();
        let items = apps.len()
            + tray_widgets.len()
            + [
                display_number.is_some(),
                !st.upgrade.is_empty() || st.upgrade_waiting,
                st.recording.is_some(),
                st.backup.is_some(),
                !st.sharing.is_empty(),
                !st.airplay.is_empty(),
                st.airpods.is_some(),
                !st.agent_screens.is_empty(),
                st.supervisor.is_some(),
                // (Olo Doctor's bag, while it has found something.)
                !st.doctor.issues.is_empty(),
                st.battery.is_some(),
                st.wired.is_some(),
                st.wifi.is_some(),
                st.volume.is_some(),
                st.notifications.is_some(),
                true,
                true,
            ]
            .iter()
            .filter(|on| **on)
            .count();
        let well_w = WIDTH - 2.0 * TRAY_INSET;
        // As many rows as it takes, the icons shared out evenly among them
        // (eight are two rows of four, not seven and one left over).
        let fit = ((well_w / TRAY_PITCH).floor() as usize).max(1);
        let rows = items.div_ceil(fit);
        let per_row = items.div_ceil(rows.max(1)).max(1);
        let tray = rect(0.0, TILE_H, WIDTH, tray_height(rows));
        let clock_row = rect(0.0, TILE_H + (CLOCK_H - CLOCK_ROW) / 2.0, WIDTH, CLOCK_ROW);
        let (date, time) = status::clock_text(&self.clock);
        let well = rect(TRAY_INSET, TILE_H + CLOCK_H, well_w, rows as f32 * TRAY_PITCH);
        // (The icons on a grid of their own, their plates near touching.)
        d.paint(move |g| olo_kit::review::own_grid(g, well));
        {
            let (date, time) = (date.clone(), time.clone());
            d.paint(move |g| {
                let p = theme::palette();
                style::fill_rect(g, tray, shade(&p, p.panel_background, 1.03));
                let font = style::ui_font(FontWeight::NORMAL);
                let cy = clock_row.center().y;
                style::label(g, px(TRAY_TEXT), cy, Align::Left, date.clone(), style::PLAIN_SIZE, font.clone(), c(p.panel_text));
                style::label(g, px(WIDTH - TRAY_TEXT), cy, Align::Right, time.clone(), style::PLAIN_SIZE, font, c(p.panel_text));
                style::bevel(g, well, shade(&p, p.panel_background, 1.25), c(p.shine));
            });
        }
        {
            let m = model.clone();
            let said = format!("{date} {time}");
            d.paint(move |g| {
                agent::record(
                    g,
                    agent::Node {
                        role: "Button",
                        name: Some("Date & Time preferences".into()),
                        value: Some(said.clone().into()),
                        bounds: clock_row,
                        enabled: true,
                        clickable: true,
                        ..Default::default()
                    },
                )
            });
            d.hits.push(hit(view, "clock", clock_row, Some("Date & Time preferences".into())).on_any_button(move |e, cx| {
                if button(e) == MouseButton::Left {
                    run(&m, "pref:datetime", cx);
                }
            }));
        }
        // Each icon's place: rows filled in turn, each row centred, all on
        // whole pixels.
        let slots: Vec<Bounds<Pixels>> = (0..items)
            .map(|i| {
                let (row, col) = (i / per_row, i % per_row);
                let in_row = (items - row * per_row).min(per_row);
                let left = TRAY_INSET + ((well_w - in_row as f32 * TRAY_PITCH) / 2.0).floor();
                let inset = (TRAY_PITCH - ICON) / 2.0;
                rect(left + col as f32 * TRAY_PITCH + inset, TILE_H + CLOCK_H + row as f32 * TRAY_PITCH + inset, ICON, ICON)
            })
            .collect();
        let mut slot = slots.into_iter();
        // A tray item: its glyph drawn (lit while hot or its menu is
        // open), its tip, and what the pointer does on it.
        let mut tray_item = |d: &mut Drawing, id: &str, tip_text: String, draw: Box<dyn Fn(&mut Painter, Bounds<Pixels>)>| -> (Bounds<Pixels>, Element) {
            // (Each item is counted above: one not counted is a bug,
            // said, and drawn nowhere; never the shell going down.)
            let r = slot.next().unwrap_or_else(|| {
                debug_assert!(false, "tray item {id} has no slot");
                tracing::error!(id, "tray item not counted: drawn nowhere");
                Bounds::default()
            });
            let key = format!("tray:{id}");
            let lit = pointer.open(&key);
            let said = tip_text.clone();
            d.paint(move |g| {
                if lit {
                    let p = theme::palette();
                    style::fill_rect(g, expand(r, PLATE), c(p.menu_selected_background));
                }
                draw(g, r);
                agent::record(g, agent::Node { role: "Button", name: Some(said.clone().into()), bounds: expand(r, PLATE), enabled: true, clickable: true, ..Default::default() });
            });
            (r, hit(view, &key, expand(r, PLATE), Some(tip_text)))
        };
        // The live widgets first, as BeOS's ProcessController sat at the
        // tray's left.
        for w in tray_widgets {
            let key = format!("widget:{}", w.id);
            let tip_text = self.live.tip(&w, &self.sources);
            // (Drawn on the layer over the bar.)
            let (r, h) = tray_item(&mut d, &key, tip_text, Box::new(|_g, _| {}));
            d.placed.push(widgets::Placed { widget: w, spot: widgets::Spot::Tray(r) });
            d.hits.push(self.widget_hit(h, &format!("tray:{key}"), r, view, &model));
        }
        for (it, picture) in apps.iter().zip(pictures) {
            let tip_text = match (it.title.as_str(), it.tooltip.as_str()) {
                (t, "") => t.to_owned(),
                ("", tip) => tip.to_owned(),
                (t, tip) if tip.starts_with(t) => tip.to_owned(),
                (t, tip) => format!("{t}: {tip}"),
            };
            let icon = it.icon.clone();
            let (r, h) = tray_item(
                &mut d,
                &it.key,
                tip_text,
                Box::new(move |g, r| {
                    // Its theme icon if it names one we have, else its picture.
                    if !icon.is_empty() && icons::paint(g, r, &icon) {
                        return;
                    }
                    match picture.clone() {
                        Some(img) => {
                            let s = img.size(0);
                            let scale = g.scale_factor();
                            let (w, h) = (s.width.0 as f32 / scale, s.height.0 as f32 / scale);
                            let at = style::snap_centred(g, r.center(), w, h);
                            let _ = g.paint_image(at, Default::default(), img, 0, false);
                        }
                        None => {
                            icons::paint(g, r, "application");
                        }
                    }
                }),
            );
            let key = format!("tray:{}", it.key);
            let (item_key, title, entries) = (it.key.clone(), it.title.clone(), it.menu.clone().filter(|m| !m.is_empty()));
            let (m, v, m2) = (model.clone(), view.clone(), model.clone());
            let wheel_key = it.key.clone();
            d.hits.push(
                h.on_any_button(move |e, cx| {
                    let b = button(e);
                    // Either button: its menu, if it has one (ending with
                    // what a click did, opening the program), else what a
                    // click does.
                    let show = matches!(b, MouseButton::Left | MouseButton::Right) && entries.is_some();
                    if show && let Some(entries) = &entries {
                        let ik = item_key.clone();
                        let mm = m.clone();
                        let open = if title.is_empty() { "Open".to_owned() } else { format!("Open {title}") };
                        let menu = sni::menu(&title, entries).separator().item(open, "sni-open");
                        open_menu(cx, &v, &key, r, menu, move |cmd, _cx| {
                            if cmd == "sni-open" {
                                mm.read().tray_host.send(&ik, sni::Do::Activate);
                            } else if let Some(id) = cmd.strip_prefix("sni:").and_then(|n| n.parse::<i32>().ok()) {
                                mm.read().tray_host.send(&ik, sni::Do::Chosen(id));
                            }
                        });
                        m.read().tray_host.send(&item_key, sni::Do::MenuShown);
                        return;
                    }
                    let what = match b {
                        MouseButton::Left => sni::Do::Activate,
                        MouseButton::Right => sni::Do::ContextMenu,
                        MouseButton::Middle => sni::Do::SecondaryActivate,
                        _ => return,
                    };
                    m.read().tray_host.send(&item_key, what);
                })
                .on_scroll_wheel(move |e: &ScrollWheelEvent, _cx| {
                    let dy = f32::from(e.delta.pixel_delta(px(24.0)).y);
                    if dy.abs() >= 1.0 {
                        m2.read().tray_host.send(&wheel_key, sni::Do::Scroll(-dy.round() as i32));
                    }
                }),
            );
        }
        if let (Some(n), Some(own)) = (display_number, display.clone()) {
            let name = self.outputs.iter().find(|o| o.name == own).map(|o| o.display_name()).unwrap_or_default();
            let (r, h) = tray_item(
                &mut d,
                "display",
                format!("Display {}: {name}. Click to go to another display.", olo_protocol::display_number_text(n)),
                Box::new(move |g, r| status::draw_display(g, r, &theme::palette(), n)),
            );
            let (m, v) = (model.clone(), view.clone());
            d.hits.push(h.on_any_button(move |e, cx| {
                if matches!(button(e), MouseButton::Left | MouseButton::Right) {
                    let menu = m.read().display_menu(&own);
                    let mm = m.clone();
                    open_menu(cx, &v, "tray:display", r, menu, move |cmd, cx| run(&mm, cmd, cx));
                }
            }));
        }
        if !st.upgrade.is_empty() || st.upgrade_waiting {
            let n = st.upgrade.len();
            let tip_text = if st.upgrade_waiting {
                "A build of Olo is running: Olo upgrades as soon as it has finished.".to_owned()
            } else {
                format!("A new build of Olo is ready ({n} program{}). Click to upgrade.", if n == 1 { "" } else { "s" })
            };
            let (_, h) = tray_item(&mut d, "upgrade", tip_text, Box::new(|g, r| status::draw_upgrade(g, r, &theme::palette())));
            let m = model.clone();
            d.hits.push(h.on_any_button(move |e, _cx| {
                if matches!(button(e), MouseButton::Left | MouseButton::Right) {
                    m.update(|d| d.ask_upgrade());
                }
            }));
        }
        // A recording of this display going: since when, Pause (or
        // Resume) and Stop.
        if let Some(rec) = st.recording.clone() {
            let since = status::time_of_day(rec.started, &clock);
            let what = match rec.what.as_str() {
                "display" => "the screen".to_owned(),
                "region" => "part of the screen".to_owned(),
                w => format!("a window ({})", w.trim_start_matches("window ")),
            };
            let paused = rec.paused;
            let (tip_text, said) = if paused {
                (format!("Recording {what} since {since}, paused. Click to resume or stop."), format!("Recording {what} since {since} (paused)"))
            } else {
                (format!("Recording {what} since {since}. Click to pause or stop."), format!("Recording {what} since {since}"))
            };
            let (r, h) = tray_item(&mut d, "recording", tip_text, Box::new(move |g, r| status::draw_recording(g, r, &theme::palette(), paused)));
            let (m, v) = (model.clone(), view.clone());
            let menu = Menu::new("").item(said, "").enabled(false);
            let menu = if paused { menu.item("Resume Recording", "record:resume") } else { menu.item("Pause Recording", "record:pause") };
            // (What the controls on the screen have.)
            let menu = menu.item("Mark This Moment", "record:mark").item("Restart Recording", "record:restart").item("Discard Recording", "record:discard");
            let menu = menu.item("Stop Recording", "record:stop").separator().item("Recordings…", "run:olo-screenshot --library").item("Open Screenshot", "run:olo-screenshot");
            d.hits.push(h.on_any_button(move |e, cx| {
                if matches!(button(e), MouseButton::Left | MouseButton::Right) {
                    let mm = m.clone();
                    open_menu(cx, &v, "tray:recording", r, menu.clone(), move |cmd, cx| run(&mm, cmd, cx));
                }
            }));
        }
        // DiskBackup, once set up: what it's doing (and anything to see),
        // a click its menu.
        if let Some(b) = st.backup.clone() {
            let tip_text = match (&b.busy, &b.off, &b.trouble) {
                (Some(busy), _, _) => format!("DiskBackup: {}. Click to see how far it is.", busy.to_lowercase()),
                (_, Some(why), _) => format!("DiskBackup: not backing up ({why})."),
                (_, _, Some(what)) => format!("DiskBackup: {what}. Click to see more."),
                _ => match b.last_ok {
                    Some(t) => format!("DiskBackup: backed up at {}.", status::time_of_day(t.max(0) as u64, &clock)),
                    None => "DiskBackup: not backed up yet.".to_owned(),
                },
            };
            let (busy, off) = (b.busy.is_some(), b.off.is_some());
            let failed = b.trouble.as_deref().is_some_and(|t| t.contains("didn’t finish"));
            let trouble = b.trouble.is_some();
            let (r, h) = tray_item(
                &mut d,
                "backup",
                tip_text,
                Box::new(move |g, r| {
                    let p = theme::palette();
                    let dot = trouble.then(|| style::c(if failed { p.failure } else { p.warning }));
                    status::draw_backup(g, r, &p, busy, off, dot)
                }),
            );
            let (m, v, clock) = (model.clone(), view.clone(), clock.clone());
            d.hits.push(h.on_any_button(move |e, cx| {
                if matches!(button(e), MouseButton::Left | MouseButton::Right) {
                    let mm = m.clone();
                    open_menu(cx, &v, "tray:backup", r, backup_menu(&clock), move |cmd, cx| run(&mm, cmd, cx));
                }
            }));
        }
        // A screen being shared: what, with whom, and Stop.
        if !st.sharing.is_empty() {
            let tip_text = match st.sharing.as_slice() {
                [one] => format!("{}. Click to stop.", olo_protocol::SharingState::said(one)),
                many if many.iter().any(|s| !s.control.is_empty()) => format!("{} shares, control among them. Click to stop them.", many.len()),
                many => format!("{} screen shares. Click to stop them.", many.len()),
            };
            let (r, h) = tray_item(&mut d, "sharing", tip_text, Box::new(|g, r| status::draw_sharing(g, r, &theme::palette())));
            let (m, v, shares) = (model.clone(), view.clone(), st.sharing.clone());
            d.hits.push(h.on_any_button(move |e, cx| {
                if matches!(button(e), MouseButton::Left | MouseButton::Right) {
                    let mm = m.clone();
                    open_menu(cx, &v, "tray:sharing", r, sharing_menu(&shares), move |cmd, cx| run(&mm, cmd, cx));
                }
            }));
        }
        // A film playing on an AirPlay receiver, or a screen shown on
        // one: what, where, and Pause and Stop.
        if !st.airplay.is_empty() {
            let tip_text = match st.airplay.as_slice() {
                [one] if one.paused() => format!("{}. Click to resume or stop.", play_said(one)),
                [one] => format!("{}. Click to pause or stop.", play_said(one)),
                many if many.iter().all(|p| p.live) => format!("{} screens shown on AirPlay receivers. Click to pause or stop them.", many.len()),
                many if many.iter().any(|p| p.live) => format!("{} playing and shown on AirPlay receivers. Click to pause or stop them.", many.len()),
                many => format!("{} films playing on AirPlay receivers. Click to pause or stop them.", many.len()),
            };
            let (paused, count) = (st.airplay.iter().all(|p| p.paused()), st.airplay.len());
            let (r, h) = tray_item(&mut d, "airplay", tip_text, Box::new(move |g, r| status::draw_airplay(g, r, &theme::palette(), paused, count)));
            let (m, v, plays) = (model.clone(), view.clone(), st.airplay.clone());
            d.hits.push(h.on_any_button(move |e, cx| {
                if matches!(button(e), MouseButton::Left | MouseButton::Right) {
                    let mm = m.clone();
                    open_menu(cx, &v, "tray:airplay", r, airplay_menu(&plays), move |cmd, cx| run(&mm, cmd, cx));
                }
            }));
        }
        // AirPods connected: their levels, noise control and settings.
        if let Some(pods) = st.airpods.clone() {
            let levels = pods.levels();
            let mut tip_text = if levels.is_empty() { pods.name.clone() } else { format!("{}: {levels}", pods.name) };
            if let Some(m) = pods.settings.listening_mode {
                tip_text.push_str(&format!(". {}", m.label()));
            }
            let lowest = pods.lowest();
            let (r, h) = tray_item(&mut d, "airpods", tip_text, Box::new(move |g, r| status::draw_airpods(g, r, &theme::palette(), lowest)));
            let (m, v) = (model.clone(), view.clone());
            d.hits.push(h.on_any_button(move |e, cx| {
                if matches!(button(e), MouseButton::Left | MouseButton::Right) {
                    let mm = m.clone();
                    open_menu(cx, &v, "tray:airpods", r, airpods_menu(&pods), move |cmd, cx| run(&mm, cmd, cx));
                }
            }));
        }
        // Agents at work on screens of their own: click to watch them;
        // the right button, a menu with each shown on AirPlay too.
        if !st.agent_screens.is_empty() {
            let n = st.agent_screens.len();
            let tip_text = if n == 1 { "An agent screen is running. Click to watch it.".to_string() } else { format!("{n} agent screens are running. Click to watch them.") };
            let (r, h) = tray_item(&mut d, "agent-screens", tip_text, Box::new(move |g, r| status::draw_screens(g, r, &theme::palette(), n)));
            let (m, v, menu) = (model.clone(), view.clone(), screens_menu(&st.agent_screens, &st.airplay_paired));
            d.hits.push(h.on_any_button(move |e, cx| match button(e) {
                MouseButton::Left => run(&m, "run:olo-screen watch", cx),
                MouseButton::Right => {
                    let mm = m.clone();
                    open_menu(cx, &v, "tray:agent-screens", r, menu.clone(), move |cmd, cx| run(&mm, cmd, cx));
                }
                _ => {}
            }));
        }
        // The supervisor: a click opens its menu (on or off, what it
        // found, Activity Monitor).
        if let Some(sup) = st.supervisor.clone() {
            let (n, urgent) = (sup.state.findings.len(), sup.state.urgent());
            let tip_text = match (sup.on, n, urgent) {
                (false, ..) => "Performance supervisor: off".to_owned(),
                (true, 0, _) => "Performance supervisor: on, nothing found".to_owned(),
                (true, n, 0) => format!("Performance supervisor: {n} finding{}", if n == 1 { "" } else { "s" }),
                (true, _, u) => format!("Performance supervisor: {u} urgent finding{}", if u == 1 { "" } else { "s" }),
            };
            let on = sup.on;
            let (r, h) = tray_item(&mut d, "supervisor", tip_text, Box::new(move |g, r| status::draw_gauge(g, r, &theme::palette(), on, n, urgent > 0)));
            let (m, v) = (model.clone(), view.clone());
            d.hits.push(h.on_any_button(move |e, cx| match button(e) {
                MouseButton::Left | MouseButton::Right => {
                    let mm = m.clone();
                    open_menu(cx, &v, "tray:supervisor", r, supervisor_menu(&sup), move |cmd, cx| run(&mm, cmd, cx));
                }
                _ => {}
            }));
        }
        // Olo Doctor's bag, while it has found something to fix (it looks
        // at login and every 15 minutes): a click opens it.
        if !st.doctor.issues.is_empty() {
            let n = st.doctor.issues.len();
            let first = &st.doctor.issues[0];
            let tip_text = if n == 1 { format!("Olo Doctor: {first}. Click to see it.") } else { format!("Olo Doctor found {n} things to fix: {first}, and more. Click to see them.") };
            let (_, h) = tray_item(&mut d, "doctor", tip_text, Box::new(move |g, r| _ = icons::paint(g, r, "doctor")));
            let m = model.clone();
            d.hits.push(h.on_any_button(move |e, cx| {
                if matches!(button(e), MouseButton::Left | MouseButton::Right) {
                    run(&m, "run:olo-doctor", cx);
                }
            }));
        }
        // The battery: a click opens its menu (the power mode, Power's
        // preferences).
        if let Some(b) = st.battery {
            let mode = st.power_modes.as_ref().map(|m| format!(", {}", m.active.label())).unwrap_or_default();
            let power = match (b.plugged, b.charging) {
                (true, true) => " — charging",
                (true, false) => " — plugged in",
                _ => "",
            };
            let tip_text = format!("{}%{power}{mode}", b.percent);
            let (r, h) = tray_item(&mut d, "battery", tip_text, Box::new(move |g, r| status::draw_battery(g, r, &theme::palette(), b.percent, b.plugged)));
            let (m, v) = (model.clone(), view.clone());
            d.hits.push(h.on_any_button(move |e, cx| match button(e) {
                MouseButton::Left | MouseButton::Right => {
                    let menu = m.read().battery_menu();
                    let mm = m.clone();
                    open_menu(cx, &v, "tray:battery", r, menu, move |cmd, cx| run(&mm, cmd, cx));
                }
                _ => {}
            }));
        }
        // The links: Ethernet while a cable's connected, Wi-Fi while
        // there's an adapter. With both, the one not carrying the traffic
        // (the default route) is drawn faint. A click opens the link's
        // menu (its addresses and rates, Network's preferences).
        use olo_system::network::Kind;
        let standing_by = |kind: Kind| st.wired.is_some() && st.wifi.as_ref().is_some_and(|w| w.ssid.is_some()) && st.primary.is_some_and(|p| p != kind);
        let faint = |on: bool| if on { Some(0.45) } else { None };
        if let Some(w) = st.wired.clone() {
            let by = standing_by(Kind::Ethernet);
            let speed = w.speed.map(|s| format!(", {}", olo_system::network::speed_text(s))).unwrap_or_default();
            let tip_text = format!("Ethernet{speed}{}", if by { ", standing by: Wi-Fi is in use" } else { "" });
            let (r, h) = tray_item(&mut d, "ethernet", tip_text, Box::new(move |g, r| g.with_element_opacity(faint(by), |g| status::draw_ethernet(g, r, &theme::palette()))));
            d.hits.push(link_hit(h, r, Kind::Ethernet, &model, view));
        }
        if let Some(w) = st.wifi.clone() {
            let by = standing_by(Kind::Wifi);
            let tip_text = match &w.ssid {
                Some(s) => format!("{s} ({}%){}", w.signal, if by { ", standing by: Ethernet is in use" } else { "" }),
                None => "Not connected".into(),
            };
            let signal = w.ssid.as_ref().map(|_| w.signal);
            let (r, h) = tray_item(&mut d, "wifi", tip_text, Box::new(move |g, r| g.with_element_opacity(faint(by), |g| status::draw_wifi(g, r, &theme::palette(), signal))));
            d.hits.push(link_hit(h, r, Kind::Wifi, &model, view));
        }
        if let Some(vol) = st.volume {
            let tip_text = if vol.muted { "Muted".into() } else { format!("Volume {}%", (vol.level * 100.0).round()) };
            let (r, h) = tray_item(&mut d, "volume", tip_text, Box::new(move |g, r| status::draw_volume(g, r, &theme::palette(), vol.level, vol.muted)));
            let (m2, v) = (model.clone(), view.clone());
            let (m3, v3, menu_of) = (model.clone(), view.clone(), (st.sound.clone(), st.airpods.clone(), vol.muted, st.voice));
            d.hits.push(
                h.on_any_button(move |e, cx| {
                    // Either button: every output and input, Mute, and
                    // AirPods' controls; the wheel sets the volume.
                    if matches!(button(e), MouseButton::Left | MouseButton::Right) {
                        let mm = m3.clone();
                        let menu = sound_menu(menu_of.0.as_ref(), menu_of.1.as_ref(), menu_of.2, &menu_of.3);
                        open_menu(cx, &v3, "tray:volume", r, menu, move |cmd, cx| run(&mm, cmd, cx));
                    }
                })
                // One notch (or a trackpad swipe's worth) per 5% step.
                .on_scroll_wheel(move |e: &ScrollWheelEvent, cx| {
                    let dy = f32::from(e.delta.pixel_delta(px(24.0)).y);
                    let m2 = m2.clone();
                    v.update(cx, move |v, _| {
                        v.scroll += dy;
                        if v.scroll.abs() < 24.0 {
                            return;
                        }
                        let step = v.scroll.signum();
                        v.scroll = 0.0;
                        m2.update(|d| {
                            if let Some(v) = &mut d.status.volume {
                                v.level = (v.level + step * 0.05).clamp(0.0, 1.0);
                                status::set_volume(v.level);
                            }
                        });
                    });
                }),
            );
        }
        // Notifications: a bell with the unread count. A click opens its
        // menu (the history, Do Not Disturb, the settings).
        if let Some(n) = st.notifications.clone() {
            let tip_text = match (n.quiet, n.unread) {
                (true, 0) => "Do Not Disturb is on".to_string(),
                (true, u) => format!("Do Not Disturb is on — {u} unread"),
                (false, 0) => "No new notifications".to_string(),
                (false, 1) => "1 new notification".to_string(),
                (false, u) => format!("{u} new notifications"),
            };
            let (unread, quiet) = (n.unread, n.quiet);
            let (r, h) = tray_item(&mut d, "notifications", tip_text, Box::new(move |g, r| status::draw_bell(g, r, &theme::palette(), unread, quiet)));
            let (m, v) = (model.clone(), view.clone());
            d.hits.push(h.on_any_button(move |e, cx| match button(e) {
                MouseButton::Left | MouseButton::Right => {
                    let mm = m.clone();
                    open_menu(cx, &v, "tray:notifications", r, notifications_menu(quiet), move |cmd, cx| run(&mm, cmd, cx));
                }
                _ => {}
            }));
        }
        // Caffeinated: a cup, steaming while the machine is kept awake. A
        // click opens its menu (until turned off, for a while, with the lid
        // closed, Allow Sleep, Power's preferences).
        {
            let on = self.caffeinated;
            let tip_text = match (on, self.caffeinated_until) {
                (true, Some(t)) => format!("Caffeinated until {}", status::time_of_day(t, &self.clock)),
                (true, None) => "Caffeinated: staying awake".into(),
                _ => "Stay awake (caffeinated)".into(),
            };
            let (r, h) = tray_item(&mut d, "caffeine", tip_text, Box::new(move |g, r| status::draw_cup(g, r, &theme::palette(), on)));
            let (m, v) = (model.clone(), view.clone());
            d.hits.push(h.on_any_button(move |e, cx| match button(e) {
                MouseButton::Left | MouseButton::Right => {
                    let menu = m.read().caffeine_menu();
                    let mm = m.clone();
                    open_menu(cx, &v, "tray:caffeine", r, menu, move |cmd, cx| run(&mm, cmd, cx));
                }
                _ => {}
            }));
        }
        // Approving without asking: a key, gold while requests go ahead
        // without asking. A click opens its menu (for how long, which
        // takes the password; Ask Again; Security's preferences).
        {
            let tip_text = match self.approve_until {
                Some(t) => format!("Approving without asking: {} left (until {})", approve::left(t), status::time_of_day(t, &self.clock)),
                None => "Approve requests without asking, for a while".into(),
            };
            let on = self.approve_until.is_some();
            let (r, h) = tray_item(&mut d, "approve", tip_text, Box::new(move |g, r| status::draw_key(g, r, &theme::palette(), on)));
            let (m, v) = (model.clone(), view.clone());
            d.hits.push(h.on_any_button(move |e, cx| match button(e) {
                MouseButton::Left | MouseButton::Right => {
                    let menu = {
                        let d = m.read();
                        approve::menu(d.approve_until, &d.clock)
                    };
                    let mm = m.clone();
                    open_menu(cx, &v, "tray:approve", r, menu, move |cmd, cx| run(&mm, cmd, cx));
                }
                _ => {}
            }));
        }

        // ---- the applications: by display (its own first, then the
        // others left to right), by workspace, in order, each under its
        // heading (each display's workspaces with windows, and the one it
        // shows)
        let mut y = self.widget_rows(&mut d, olo_protocol::deskbar::Place::Top, TILE_H + tray_height(rows), view, &model);
        for on in self.displays_from(display.as_deref()) {
            let on = on.as_deref();
            let current = self.showing(on);
            let mut workspaces: Vec<u32> = self.windows_on(on).map(|w| w.workspace).chain([current]).collect();
            workspaces.sort();
            workspaces.dedup();
            for ws in workspaces {
                let teams = self.teams(on, Some(ws));
                // The workspace showing is lit, its heading and
                // applications together, as one selection: on its own
                // ground, edged above and below.
                let group = rect(0.0, y, WIDTH, HEADING_H + teams.len() as f32 * ROW_H);
                let lit = self.lit(on, Some(ws));
                if lit {
                    let ground = self.ground(&p, on, Some(ws));
                    d.paint(move |g| style::fill_rect(g, group, c(ground)));
                }
                self.workspace_heading(&mut d, &p, rect(0.0, y, WIDTH, HEADING_H), on, ws, pointer, view, &model);
                y += HEADING_H;
                for team in teams {
                    self.team_row(&mut d, &p, rect(0.0, y, WIDTH, ROW_H), &team, on, Some(ws), pointer, view, &model, cx);
                    y += ROW_H;
                }
                if lit {
                    d.paint(move |g| style::lit_edges(g, group, &theme::palette()));
                }
            }
        }

        // The widgets at its foot, under the applications.
        y = self.widget_rows(&mut d, olo_protocol::deskbar::Place::Foot, y, view, &model);

        // Outer frame: a dark line on the left and bottom, where the
        // Deskbar meets the desktop.
        let bottom = y;
        d.paint(move |g| {
            let p = theme::palette();
            let edge = shade(&p, p.panel_background, 1.6);
            style::vline(g, px(0.0), px(0.0), px(bottom), edge);
            style::hline_above(g, px(0.0), px(WIDTH), px(bottom), edge);
        });
        (d, y.ceil())
    }

    /// Workspace `ws`'s heading at `r`, on `display`'s Deskbar, its
    /// applications under it: its name as a quiet caption over their
    /// icons, the one showing's in the selection's text on its lit ground
    /// (`draw_bar` lights the whole workspace, `style::lit_ground`), and
    /// at its right end the workspace's windowing mode (click: the modes,
    /// and on the one showing, the arranging commands). A click goes
    /// there; on the workspace showing, the Workspaces menu; the right
    /// button, the heading's menu (`heading_menu`).
    #[allow(clippy::too_many_arguments)]
    fn workspace_heading(&self, d: &mut Drawing, p: &Palette, r: Bounds<Pixels>, display: Option<&str>, ws: u32, pointer: &Pointer, view: &ViewRef<BarView>, model: &Model) {
        let name = self.workspace_name(display, ws);
        let here = self.lit(display, Some(ws));
        let mode = self.mode(display, ws);
        // The windowing mode, square at the right end, as far in as the
        // caption is from the left.
        let mode_r = rect(f32::from(r.right()) - ICON_INSET - ICON, f32::from(r.center().y) - ICON / 2.0, ICON, ICON);
        let key = format!("ws:{}:{ws}", display.unwrap_or(""));
        let mode_key = format!("mode:{}:{ws}", display.unwrap_or(""));
        let (open, mode_open) = (pointer.open(&key), pointer.open(&mode_key));
        let (hovered, mode_hovered) = (pointer.hot(&key), pointer.hot(&mode_key));
        let tip_text = match mode {
            WindowingMode::Standard => format!("{name}: windows stay where you put them. Click to choose a windowing mode."),
            WindowingMode::AutoTiling => format!("{name}: Auto-Tiling, every window takes a tile. Click to choose a windowing mode."),
            WindowingMode::Focus => format!("{name}: Focus, only the active window shows, zoomed. Click to choose a windowing mode."),
        };
        let ground = self.ground(p, display, Some(ws));
        {
            let (name, tip_text) = (name.clone(), tip_text.clone());
            d.paint(move |g| {
                let p = theme::palette();
                if open || mode_open {
                    style::fill_rect(g, r, c(p.menu_selected_background));
                } else if hovered {
                    style::fill_rect(g, r, style::tint(ground, 0.8));
                }
                let ink = if open || mode_open {
                    c(p.menu_selected_item_text)
                } else if here {
                    c(p.list_selected_item_text)
                } else if hovered {
                    c(p.panel_text)
                } else {
                    c(olo_kit::palette::mix(p.panel_text, p.panel_background, 0.45))
                };
                let room = f32::from(mode_r.left()) - 4.0 - ICON_INSET;
                style::label_fit(g, px(ICON_INSET), r.center().y, Align::Left, name.clone(), style::SMALL_SIZE, style::ui_font(FontWeight::BOLD), ink, px(room));
                if mode_hovered && !mode_open {
                    let at = expand(mode_r, 2.0);
                    g.paint_quad(olo_kit::quad(style::snap(g, at), px(2.0), style::tint(ground, 0.7), px(0.0), olo_kit::transparent_black(), olo_kit::BorderStyle::Solid));
                }
                status::draw_mode(g, mode_r, &p, mode);
                agent::record(g, agent::Node { role: "Button", name: Some(name.clone().into()), bounds: r, enabled: true, clickable: true, selected: Some(here), ..Default::default() });
                agent::record(g, agent::Node { role: "Button", name: Some(tip_text.clone().into()), bounds: expand(mode_r, 2.0), enabled: true, clickable: true, ..Default::default() });
            });
        }
        let dsp = display.map(str::to_owned);
        {
            let (m, v, dsp, k) = (model.clone(), view.clone(), dsp.clone(), key.clone());
            d.hits.push(hit(view, &key, r, None).on_any_button(move |e, cx| match button(e) {
                MouseButton::Right => {
                    let menu = m.read().heading_menu(dsp.as_deref(), ws);
                    let (mm, dd) = (m.clone(), dsp.clone());
                    open_menu(cx, &v, &k, r, menu, move |cmd, _cx| {
                        mm.update(|d| {
                            if !d.heading_command(dd.as_deref(), cmd) {
                                d.run_command(cmd);
                            }
                        })
                    });
                }
                MouseButton::Left if here => {
                    let menu = m.update(|d| {
                        d.menu_output = dsp.clone();
                        d.workspaces_menu(dsp.as_deref())
                    });
                    let mm = m.clone();
                    open_menu(cx, &v, &k, r, menu, move |cmd, cx| run(&mm, cmd, cx));
                }
                MouseButton::Left => request(Request::SwitchWorkspace { index: ws, output: dsp.clone() }),
                _ => {}
            }));
        }
        {
            let (m, v, k) = (model.clone(), view.clone(), mode_key.clone());
            d.hits.push(hit(view, &mode_key, expand(mode_r, 2.0), Some(tip_text)).occlude().on_any_button(move |e, cx| {
                if !matches!(button(e), MouseButton::Left | MouseButton::Right) {
                    return;
                }
                let menu = m.update(|d| {
                    d.menu_output = dsp.clone();
                    d.menu_workspace = Some(ws);
                    let mut menu = if here { d.arrange_menu(mode) } else { d.modes_menu(mode) };
                    add_keys(&mut menu, &d.keys);
                    menu
                });
                let mm = m.clone();
                open_menu(cx, &v, &k, r, menu, move |cmd, cx| run(&mm, cmd, cx));
            }));
        }
    }

    /// An application's row at `r`: its icon and name, recessed while
    /// it has the focus, highlighted while Twitcher is on it. A click
    /// brings it forward, all its windows; the right button opens the
    /// menu of them; files dropped on it open with it; dragged out, it is
    /// a link to the program. Under workspace `ws`'s heading, it is its
    /// windows there.
    #[allow(clippy::too_many_arguments)]
    fn team_row(
        &mut self,
        d: &mut Drawing,
        p: &Palette,
        r: Bounds<Pixels>,
        team: &Team,
        display: Option<&str>,
        ws: Option<u32>,
        pointer: &Pointer,
        view: &ViewRef<BarView>,
        model: &Model,
        cx: &Cx<BarView>,
    ) {
        let notes = self.notes_of(team);
        let badge = self.badge_of(team);
        let takes_drops = self.handler(&team.key).is_some();
        let key = format!("team:{}:{}:{}", display.unwrap_or(""), ws.map_or(String::new(), |w| w.to_string()), team.key);
        let open = pointer.open(&key);
        let hovered = pointer.hot(&key);
        let focused = team.windows.iter().any(|w| w.focused);
        let hidden = team.windows.iter().all(|w| w.minimized);
        // Twitcher's choice, while ⌘Tab (or ⌥Tab, ⌘`) is held.
        let chosen = team.windows.iter().any(|w| w.chosen);
        // What agents read of it: its unread count, notifications, progress.
        let said: Vec<String> = [
            badge.as_ref().and_then(|b| b.count).map(|n| format!("{n} unread")),
            notes.map(|d| match (d.count, d.fresh < 1.0) {
                (0, _) => "has notifications".to_owned(),
                (n, false) => format!("has notifications ({n})"),
                (n, true) => format!("has notifications ({n}, the dot {}% full)", (d.fresh * 100.0).round()),
            }),
            badge.as_ref().and_then(|b| b.progress).map(|p| format!("{}% done", (p * 100.0).round())),
        ]
        .into_iter()
        .flatten()
        .collect();
        // In the workspace showing, the row is on its lit ground.
        let lit = self.lit(display, ws);
        let ground = self.ground(p, display, ws);
        let show_count = self.status.notifications.as_ref().is_some_and(|n| n.show_count);
        if notes.is_some_and(|n| n.shake.is_some()) {
            d.animating = true;
        }
        if notes.is_some_and(|n| n.fresh < 1.0) {
            d.fading = true;
        }
        let (name, icon) = (team.name.clone(), team.icon.clone());
        let progress = badge.as_ref().and_then(|b| b.progress);
        // Files dragged onto an application open with it: lit while
        // they're over it.
        let drop_here = takes_drops && cx.files_dragged_over() && pointer.hot(&key);
        d.paint(move |g| {
            let p = theme::palette();
            let selected = open || drop_here || (chosen && lit);
            if selected {
                // (Twitcher's choice in the workspace showing takes the
                // menu's selection, as the list's would be lost on its lit
                // ground.)
                style::fill_rect(g, r, c(p.menu_selected_background));
            } else if chosen {
                // As Twitcher marks it, in the selection's colours.
                style::selection_wash(g, style::snap(g, r), &p);
            } else if focused {
                // The active application sits slightly recessed.
                style::fill_rect(g, r, shade(&p, ground, 1.1));
                style::hline(g, r.left(), r.right(), r.top(), shade(&p, ground, 1.25));
            } else if hovered {
                style::fill_rect(g, r, style::tint(ground, 0.8));
            }
            // (The wash's own edge is the chosen row's.)
            if !chosen {
                style::hline_above(g, r.left(), r.right(), r.bottom(), shade(&p, ground, 1.14));
            }
            let icon_r = rect(ICON_INSET, f32::from(r.center().y) - ICON / 2.0, ICON, ICON);
            let opacity = if hidden { 120.0 / 255.0 } else { 1.0 };
            let drawn = icon.as_deref().is_some_and(|i| icons::paint_faded(g, icon_r, i, opacity));
            if !drawn {
                icons::paint_faded(g, icon_r, "application", opacity);
            }
            let text_color = if selected {
                c(p.menu_selected_item_text)
            } else if chosen {
                c(p.list_selected_item_text)
            } else if hidden {
                style::disabled_text(&p)
            } else {
                c(p.panel_text)
            };
            let font = style::ui_font(if focused { FontWeight::BOLD } else { FontWeight::NORMAL });
            let count_font = style::ui_font(FontWeight::NORMAL);
            // The count beside the dot, when asked for.
            let count = notes.filter(|d| show_count && d.count > 0).map(|d| count_text(d.count));
            let count_w = count.as_ref().map_or(0.0, |t| f32::from(style::text_width(t.clone(), style::DETAIL_SIZE, count_font.clone())) + NOTE_COUNT_GAP);
            let dot_room = if notes.is_some() { NOTE_DOT + NOTE_DOT_GAP + count_w } else { 0.0 };
            let text_x = f32::from(icon_r.right()) + 8.0;
            let max_w = f32::from(r.right()) - text_x - 6.0 - dot_room;
            style::label_fit(g, px(text_x), r.center().y, Align::Left, name.clone(), style::PLAIN_SIZE, font, text_color, px(max_w));
            // Notifications unread: a dot at the right end, in the
            // history's mark for what is unread (on a selected row, in its
            // text's colour); one just in shakes it a moment. As they fade
            // it drains from its right, to a floor (the rest a faint well
            // of its colour); the count, when shown, beside it.
            if let Some(dot) = notes {
                let at = style::snap_centred(g, point(r.right() - px(NOTE_DOT_INSET + NOTE_DOT / 2.0), r.center().y), NOTE_DOT, NOTE_DOT);
                let scale = g.scale_factor();
                // (Whole device pixels: the shake too, so the edge stays sharp.)
                let dx = dot.shake.map_or(0.0, |t| (SHAKE_PX * (t * SHAKE_TIMES * std::f32::consts::TAU).sin() * (1.0 - t).powi(2) * scale).round() / scale);
                let color = if open || drop_here || chosen { text_color } else { c(p.keyboard_navigation) };
                let centre = point(at.center().x + px(dx), at.center().y);
                let radius = f32::from(at.size.width) / 2.0;
                let fill = NOTE_DOT_FLOOR + (1.0 - NOTE_DOT_FLOOR) * dot.fresh.clamp(0.0, 1.0);
                let cx_ = f32::from(centre.x);
                let edge = ((cx_ - radius + fill * 2.0 * radius) * scale).round() / scale;
                let tau = std::f32::consts::TAU;
                if edge >= cx_ + radius - 0.5 / scale {
                    g.paint_sector(centre, px(0.0), px(radius), 0.0, tau, color);
                } else {
                    g.paint_sector(centre, px(0.0), px(radius), 0.0, tau, color.opacity(NOTE_DOT_WELL));
                    let left = Bounds::from_corners(point(px(cx_ - radius - 1.0), centre.y - px(radius + 1.0)), point(px(edge), centre.y + px(radius + 1.0)));
                    g.with_content_mask(Some(ContentMask { bounds: left }), |g| g.paint_sector(centre, px(0.0), px(radius), 0.0, tau, color));
                }
                if let Some(t) = count {
                    style::label(g, at.left() + px(dx - NOTE_COUNT_GAP), r.center().y, Align::Right, t, style::DETAIL_SIZE, count_font, color);
                }
            }
            // Something under way (a download): a thin bar along the row's
            // foot, under the name, in the selection's colour on a faint
            // track.
            if let Some(done) = progress {
                let scale = g.scale_factor();
                let snap = |v: f32| (v * scale).round() / scale;
                let (left, right) = (snap(f32::from(icon_r.right()) + 8.0), snap(f32::from(r.right()) - NOTE_DOT_INSET));
                // (Clear of the names' descenders, a device pixel above the
                // row's line.)
                let bottom = snap(f32::from(r.bottom())) - 2.0 / scale;
                let track = Bounds::from_corners(point(px(left), px(bottom - 2.0)), point(px(right), px(bottom)));
                style::fill_rect(g, track, shade(&p, p.panel_background, 1.2));
                let fill_to = snap(left + (right - left) * done as f32);
                style::fill_rect(g, Bounds::from_corners(track.origin, point(px(fill_to), px(bottom))), c(p.keyboard_navigation));
            }
            agent::record(
                g,
                agent::Node {
                    role: "Button",
                    name: Some(name.clone().into()),
                    value: (!said.is_empty()).then(|| said.join(", ").into()),
                    bounds: r,
                    enabled: true,
                    clickable: true,
                    selected: Some(chosen),
                    ..Default::default()
                },
            );
        });

        let team_key_ = team.key.clone();
        let dsp = display.map(str::to_owned);
        let (m, v, k) = (model.clone(), view.clone(), key.clone());
        let menu_team = team.clone();
        let (m_down, v_down, m_move, v_move, v_up) = (model.clone(), view.clone(), model.clone(), view.clone(), view.clone());
        let press = Press { at: point(px(0.0), px(0.0)), team: team.key.clone(), icon: team.icon.clone().unwrap_or_else(|| "application".into()), name: team.name.clone() };
        let mut row = hit(view, &key, r, None)
            .on_any_button(move |e, cx| match button(e) {
                // A click brings the application forward, every window of
                // it (with ⌘ held, starts a copy of it of its own); the
                // right button opens the menu of them, to choose one.
                MouseButton::Left if command_held() => m.update(|d| d.launch_again(&team_key_)),
                MouseButton::Left => m.read().bring_forward(&team_key_, dsp.as_deref(), ws),
                MouseButton::Right => {
                    let menu = m.read().team_menu(&menu_team);
                    let (mm, tk, dd) = (m.clone(), team_key_.clone(), dsp.clone());
                    open_menu(cx, &v, &k, r, menu, move |cmd, _cx| mm.update(|d| d.team_command(&tk, cmd, dd.as_deref(), ws)));
                }
                _ => {}
            })
            // Dragged out: a link to its program, wherever it's dropped
            // (BeOS's Deskbar made links the same way).
            .on_mouse_down(MouseButton::Left, move |e: &MouseDownEvent, cx| {
                let side = (32.0 * cx.scale_factor().ceil()).round() as u32;
                let _ = icons::bitmap(&press.icon, side);
                let _ = &m_down;
                let press = Press { at: e.position, ..press.clone() };
                v_down.update(cx, move |v, _| v.press = Some(press));
            })
            .on_mouse_move(move |e: &MouseMoveEvent, cx| {
                if e.pressed_button != Some(MouseButton::Left) {
                    return;
                }
                let (at, m) = (e.position, m_move.clone());
                v_move.update(cx, move |v, cx| {
                    let Some(p) = v.press.clone() else { return };
                    let moved = at - p.at;
                    if f32::from(moved.x).hypot(f32::from(moved.y)) < DRAG_START {
                        return;
                    }
                    v.press = None;
                    let Some(program) = m.read().program(&p.team) else { return };
                    let mut drag = dnd::as_link(dnd::files(&[program], true, false));
                    drag.icon = dnd::picture(cx, &p.icon, 32.0, Some(&p.name), point(px(16.0), px(16.0)));
                    tip::hide(cx);
                    dnd::start(cx, drag, |_| BarMsg::Changed);
                });
            })
            .on_mouse_up(MouseButton::Left, move |_, cx| v_up.update(cx, move |v, _| v.press = None));
        if takes_drops {
            let (m, tk) = (model.clone(), team.key.clone());
            row = row.on_drop(move |paths: &ExternalPaths, _cx| {
                let paths = paths.paths().to_vec();
                m.update(|d| d.dropped(&tk, paths));
            });
        }
        d.hits.push(row);
    }
}

impl Deskbar {
    /// The live widgets placed at `place`, in rows from `y` down (each on
    /// its own, or beside the one before; the tray's ground under the
    /// tray, the panel's at the foot, an etched line above them): where
    /// the next thing starts. Drawn on the layer over the bar; the
    /// ground, tips and clicks here.
    fn widget_rows(&self, d: &mut Drawing, place: olo_protocol::deskbar::Place, y: f32, view: &ViewRef<BarView>, model: &Model) -> f32 {
        let placed: Vec<olo_protocol::deskbar::Widget> = self.live.widgets.iter().filter(|w| w.place == place).cloned().collect();
        let rows = widgets::rows(&placed);
        if rows.is_empty() {
            return y;
        }
        let top = y;
        let height: f32 = rows.iter().map(|row| widgets::row_height(row)).sum();
        let under_tray = place == olo_protocol::deskbar::Place::Top;
        let section = rect(0.0, top, WIDTH, height + TRAY_FOOT);
        d.paint(move |g| {
            let p = theme::palette();
            let ground = if under_tray { shade(&p, p.panel_background, 1.03) } else { c(p.panel_background) };
            style::fill_rect(g, section, ground);
            // (An etched line between it and what's above.)
            style::hline(g, px(0.0), px(WIDTH), section.top(), shade(&p, p.panel_background, 1.25));
            style::hline(g, px(0.0), px(WIDTH), section.top() + style::hairline(g), c(p.shine));
        });
        let mut y = top + TRAY_FOOT / 2.0;
        for row in rows {
            let h = widgets::row_height(&row);
            let r = rect(0.0, y, WIDTH, h);
            y += h;
            let spots: Vec<widgets::Spot> = if row.len() == 1 { vec![widgets::Spot::Row(r)] } else { widgets::cells(r, row.len(), TRAY_TEXT).into_iter().map(widgets::Spot::Cell).collect() };
            for (w, spot) in row.into_iter().zip(spots) {
                let key = format!("widget:{}", w.id);
                let tip_text = self.live.tip(&w, &self.sources);
                let at = match spot {
                    widgets::Spot::Row(r) | widgets::Spot::Tray(r) => r,
                    // (A cell's hit takes in half the gap at each side;
                    // not above or below, where the rows beside it are.)
                    widgets::Spot::Cell(c) => Bounds { origin: point(c.left() - px(olo_kit::metrics::TIGHT / 2.0), c.top()), size: size(c.size.width + px(olo_kit::metrics::TIGHT), c.size.height) },
                };
                let h = hit(view, &key, at, Some(tip_text));
                d.hits.push(self.widget_hit(h, &key, at, view, model));
                d.placed.push(widgets::Placed { widget: w, spot });
            }
        }
        top + height + TRAY_FOOT
    }

    /// What a widget does under the pointer: a click opens Activity
    /// Monitor; the other button, a menu with it and Deskbar's
    /// preferences.
    fn widget_hit(&self, h: Element, key: &str, r: Bounds<Pixels>, view: &ViewRef<BarView>, model: &Model) -> Element {
        let (m, v, key) = (model.clone(), view.clone(), key.to_owned());
        h.on_any_button(move |e, cx| match button(e) {
            MouseButton::Left => run(&m, "run:olo-activity", cx),
            MouseButton::Right => {
                let menu = Menu::new("Widget").item("Activity Monitor", "run:olo-activity").separator().item("Deskbar Preferences…", "pref:deskbar");
                let mm = m.clone();
                open_menu(cx, &v, &key, r, menu, move |cmd, cx| run(&mm, cmd, cx));
            }
            _ => {}
        })
    }
}

/// A section's bevel: light along the top and left, dark along the
/// bottom.
fn section_bevel(g: &mut Painter, r: Bounds<Pixels>, p: &Palette) {
    style::hline(g, r.left(), r.right(), r.top(), c(p.shine));
    style::vline(g, r.left(), r.top(), r.bottom(), c(p.shine));
    style::hline_above(g, r.left(), r.right(), r.bottom(), shade(p, p.panel_background, 1.3));
}

fn watch_server(proxy: olo_kit::Proxy) {
    std::thread::Builder::new()
        .name("deskbar-events".into())
        .spawn(move || {
            loop {
                if let Ok(mut c) = Client::connect() {
                    if let Ok(Reply::Windows { windows }) = c.request(&Request::ListWindows) {
                        proxy.send(Msg::Windows(windows));
                    }
                    if let Ok(Reply::Workspaces { current, count, displays }) = c.request(&Request::Workspaces) {
                        proxy.send(Msg::Workspace(current, count, displays));
                    }
                    if let Ok(Reply::Power { status }) = c.request(&Request::PowerStatus) {
                        proxy.send(Msg::Power(status));
                    }
                    if let Ok(Reply::ArrangementUndo { undo, redo }) = c.request(&Request::ArrangementUndo) {
                        proxy.send(Msg::Arrangement(undo, redo));
                    }
                    if let Ok(Reply::Settings { settings }) = c.request(&Request::GetSettings) {
                        proxy.send(settings_msg(settings));
                    }
                    if let Ok(Reply::Outputs { outputs }) = c.request(&Request::Outputs) {
                        proxy.send(Msg::Outputs(outputs));
                    }
                    use olo_protocol::EventKind as K;
                    if let Ok(events) = c.into_events_of(&[K::Windows, K::Workspace, K::Power, K::Settings, K::Outputs, K::ArrangementUndo, K::PowerButton]) {
                        // Displays that came while this was subscribing
                        // (after an upgrade they come one by one): asked
                        // again, now that what changes from here comes as
                        // an event.
                        if let Ok(mut again) = Client::connect()
                            && let Ok(Reply::Outputs { outputs }) = again.request(&Request::Outputs)
                        {
                            proxy.send(Msg::Outputs(outputs));
                        }
                        for e in events.flatten() {
                            match e {
                                Event::WindowsChanged { windows } => proxy.send(Msg::Windows(windows)),
                                Event::WorkspaceChanged { current, count, displays } => proxy.send(Msg::Workspace(current, count, displays)),
                                Event::PowerChanged { status } => proxy.send(Msg::Power(status)),
                                Event::ArrangementUndoChanged { undo, redo } => proxy.send(Msg::Arrangement(undo, redo)),
                                Event::SettingsChanged { settings } => proxy.send(settings_msg(settings)),
                                Event::OutputsChanged { outputs } => proxy.send(Msg::Outputs(outputs)),
                                Event::PowerButton { action: olo_protocol::power_button::PowerAction::RestartAsk, done: true } => proxy.send(Msg::PowerButton(Power::Restart)),
                                Event::PowerButton { action: olo_protocol::power_button::PowerAction::ShutDownAsk, done: true } => proxy.send(Msg::PowerButton(Power::ShutDown)),
                                _ => {}
                            }
                        }
                    }
                }
                std::thread::sleep(Duration::from_secs(1));
            }
        })
        .expect("thread");
}

/// The app's looper: the model kept as word comes, a bar on each
/// display, each told when to draw again; and the windows that aren't
/// a bar's.
struct Hub {
    model: Model,
    /// One on each display: its display (None: opened before the
    /// displays were known) and its window.
    bars: Vec<(Option<String>, WindowHandle<BarView>)>,
    /// Approve Without Asking's window, while it is open.
    approve: Option<WindowHandle<approve::Ask>>,
    /// Whether the clock shows seconds, to whoever ticks.
    seconds: mpsc::Sender<bool>,
}

impl Hub {
    fn tell_bars(&self, msg: impl Fn() -> BarMsg) {
        for (_, bar) in &self.bars {
            bar.messenger().send(msg());
        }
    }

    /// A Deskbar on each display that isn't a mirror, and none where
    /// there's no display any more. With no displays known yet
    /// (reconnecting to an app_server still finding them), as it is:
    /// they come as app_server lists them and this connection learns
    /// them.
    fn sync_bars(&mut self) {
        let wanted: Vec<Option<String>> = self.model.read().outputs.iter().filter(|o| o.mirror_of.is_none()).map(|o| Some(o.name.clone())).collect();
        if wanted.is_empty() {
            return;
        }
        tracing::debug!(?wanted, have = ?self.bars.iter().map(|(o, _)| o.clone()).collect::<Vec<_>>(), "displays");
        // A bar whose window the compositor closed (its display went,
        // or came back as another) is no bar: its display gets a new
        // one below. (Kept, it hid the Dell's missing Deskbar, 2026-11-01.)
        self.bars.retain(|(_, bar)| bar.is_open());
        // (The new ones first, so the app always has one.)
        for output in &wanted {
            if !self.bars.iter().any(|(o, _)| o == output) {
                self.bars.push((output.clone(), open_bar(self.model.clone(), output.clone())));
            }
        }
        let (keep, gone): (Vec<_>, Vec<_>) = std::mem::take(&mut self.bars).into_iter().partition(|(o, _)| wanted.contains(o));
        self.bars = keep;
        for (_, bar) in gone {
            bar.close();
        }
    }

    /// The main display's bar (or any): what asks the person is its.
    fn main_bar(&self) -> Option<&WindowHandle<BarView>> {
        let main = self.model.read().main_output();
        self.bars.iter().find(|(o, _)| main.is_some() && *o == main).or(self.bars.first()).map(|(_, bar)| bar)
    }

    fn ask(&self, effect: Effect) {
        if let Some(bar) = self.main_bar() {
            bar.messenger().send(BarMsg::Effect(effect));
        }
    }

    fn message(&mut self, message: Box<dyn std::any::Any + Send>) {
        let Ok(m) = message.downcast::<Msg>() else { return };
        tracing::debug!(what = m.name(), "told");
        match *m {
            Msg::Changed | Msg::Tick => self.tell_bars(|| BarMsg::Changed),
            Msg::Displays => {
                self.sync_bars();
                self.tell_bars(|| BarMsg::Changed);
            }
            // The power button held for an "Asking First": its alert, as
            // the menu's Restart and Shut Down.
            Msg::PowerButton(action) => self.ask(Effect::ConfirmPower(action)),
            Msg::Effect(effect) => self.ask(effect),
            Msg::Approve(minutes) => match self.approve.as_ref().filter(|a| a.is_open()) {
                Some(open) => {
                    open.messenger().send(approve::Msg::Choose(minutes));
                }
                None => self.approve = Some(approve::Ask::open(minutes)),
            },
            m => {
                let taken = self.model.read().take(m);
                match taken {
                    Taken::Changed => self.tell_bars(|| BarMsg::Changed),
                    Taken::Seconds(on) => {
                        let _ = self.seconds.send(on);
                        self.tell_bars(|| BarMsg::Changed);
                    }
                    Taken::Displays => {
                        self.sync_bars();
                        self.tell_bars(|| BarMsg::Changed);
                    }
                    Taken::Meters(tip_now) => self.tell_bars(|| BarMsg::Meters(tip_now.clone())),
                    Taken::Retip(tip_now) => self.tell_bars(|| BarMsg::Retip(tip_now.clone())),
                }
            }
        }
    }
}

/// Tells the app as the clock's next minute comes (or second, while it
/// shows them: `seconds` says when that changes), so the bars draw the
/// time again. Asleep between.
fn tick(seconds: mpsc::Receiver<bool>) {
    std::thread::Builder::new()
        .name("deskbar-clock".into())
        .spawn(move || {
            let mut show_seconds = false;
            loop {
                match seconds.recv_timeout(status::until_next_tick(show_seconds)) {
                    Ok(on) => show_seconds = on,
                    Err(mpsc::RecvTimeoutError::Timeout) => tell(Msg::Tick),
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
            }
        })
        .expect("thread");
}

fn main() {
    olo_crash::install();
    // A session or install from before the rename (Tabula) carries on.
    olo_xdg::legacy::adopt();
    // The shell: above apps and background work (olo_protocol::performance::tier).
    if let Err(e) = olo_system::priority::raise_self(olo_protocol::performance::SHELL_NICE) {
        tracing::debug!("Deskbar stays at its nice: {e}");
    }
    olo_kit::run("olo.deskbar", |app| {
        let proxy = olo_kit::proxy();
        watch_server(proxy.clone());
        scan_installed(proxy.clone());
        std::thread::spawn(|| tell(Msg::Sources(olo_system::meters::sources())));
        // Applications installed or removed: listed again at once.
        let rescan = proxy.clone();
        if let Some(w) = olo_kit::watch::watch(desktop_entry::app_dirs(), move |_| scan_installed(rescan.clone())) {
            std::mem::forget(w);
        }
        // AppImages put in ~/AppImages or taken out (the folder there or
        // not, made later): listed again at once.
        scan_appimages(proxy.clone());
        let rescan = proxy.clone();
        if let Some(w) = olo_kit::watch::watch([appimages::dir()], move |_| scan_appimages(rescan.clone())) {
            std::mem::forget(w);
        }
        status::spawn(|s| tell(Msg::Status(s)));
        let tray_host = sni::start(None, |items| tell(Msg::Tray(items)));
        let order = olo_kit::restored_state().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        let model: Model = Arc::new(Mutex::new(Deskbar {
            drawing_for: None,
            outputs: Vec::new(),
            windows: Vec::new(),
            order,
            gone: HashMap::new(),
            workspace: (0, 1),
            showing: Vec::new(),
            status: status::Status::default(),
            caffeinated: false,
            caffeinated_until: None,
            caffeinated_lid: false,
            clock: Default::default(),
            menu_icons: true,
            installed: Installed::default(),
            scanning: true,
            appimages: None,
            scanned_at: Some(Instant::now()),
            entries: HashMap::new(),
            installed_known: false,
            asked_process: Default::default(),
            last_focused: None,
            tiling: Default::default(),
            menu_output: None,
            menu_workspace: None,
            keys: KeyboardProfile::default(),
            approve_until: None,
            arrangement: (None, None),
            notes_seen: None,
            shaking: HashMap::new(),
            tray_apps: Vec::new(),
            tray_host,
            badges: Vec::new(),
            tray_pictures: HashMap::new(),
            live: widgets::Live::new(),
            sources: Default::default(),
        }));
        let (seconds, ticking) = mpsc::channel();
        tick(ticking);
        // One on each display, by name (mirrors are known later, and put
        // away then). With none known yet (reconnecting to an app_server
        // still finding its displays), none: they come as app_server
        // lists them and this connection learns them (`sync_bars`).
        let bars = olo_kit::display_names().into_iter().flatten().map(|name| (Some(name.clone()), open_bar(model.clone(), Some(name)))).collect();
        let mut hub = Hub { model: model.clone(), bars, approve: None, seconds };
        // A display app_server listed before this app's connection knew it
        // (after a restart they come one by one, the external ones last):
        // its bar is opened once it is known here, not on the main display
        // in its stead (the Dell's bar hid under the Studio Display's,
        // 2026-11-01).
        olo_kit::on_displays_changed(|| tell(Msg::Displays));
        olo_kit::message_handler(app, move |message, _| hub.message(message));
        // Deskbar stays with no bar open (between a display going and
        // another coming).
        olo_kit::stay_resident();
        // Its applications' order, for the Deskbar it re-executes as (an
        // upgrade, a lost GPU): they come back in the order they were.
        olo_kit::on_upgrade(move |_| serde_json::to_string(&model.read().order).ok());
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(desktop_entry: Option<&str>, name: &str) -> NoteApp {
        NoteApp { desktop_entry: desktop_entry.map(str::to_owned), name: name.into(), newest: (1, 1), ..Default::default() }
    }

    #[test]
    fn notes_find_their_application() {
        // An Olo app's desktop entry is its window's app id with a dash.
        assert!(same_app(&note(Some("olo-terminal"), "Terminal"), "olo.terminal", None, "Terminal"));
        assert!(!same_app(&note(Some("olo-terminal"), "Terminal"), "olo.tracker", None, "Tracker"));
        // Another's: its window's app id, its entry's file or window class.
        let slack = DesktopEntry { id: "com.slack.Slack".into(), name: "Slack".into(), startup_wm_class: Some("Slack".into()), ..Default::default() };
        assert!(same_app(&note(Some("com.slack.Slack.desktop"), "Slack"), "Slack", Some(&slack), "Slack"));
        assert!(same_app(&note(Some("slack"), "Slack Desktop"), "Slack", None, "Slack"));
        // Else by name, a website's by its browser's.
        assert!(same_app(&note(None, "Google Chrome"), "google-chrome", None, "Google Chrome"));
        assert!(!same_app(&note(Some("org.gnome.Calendar"), "Calendar"), "olo.deskcalc", None, "DeskCalc"));
    }

    #[test]
    fn listings_read_well() {
        let n = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(listing(&n(&["A"])), "A");
        assert_eq!(listing(&n(&["A", "B", "C"])), "A, B and C");
        assert_eq!(listing(&n(&["A", "B", "C", "D", "E", "F"])), "A, B, C and 3 others");
    }
}
