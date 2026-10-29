//! Deskbar: the Olo menu, a status tray and the running applications,
//! docked in the top-right corner of the screen as on BeOS. One, on the
//! main display, shows every application; with Preferences ▸ Displays ▸
//! Deskbar on every display, each display has its own, showing the
//! applications with windows there (and only those windows).
//!
//! With more than one workspace, the applications are under the heading
//! of the workspace showing, those with windows there; the rest are under
//! Other Workspaces, folded (as BOutlineListView folds), each under its
//! own workspace's heading when unfolded. Clicking a heading goes to that
//! workspace.

mod status;

use std::collections::HashMap;

use olo_kit::{
    App, Cx, Layer, LayerOptions, Menu, WindowId, WindowKind, WindowOptions,
    desktop_entry::{self, DesktopEntry},
    egui::{self, Color32, Rect, Sense, pos2, vec2},
    icons,
    menu::{self, MenuAnchor, MenuItem, MenuRequest},
    style::{self, c, hline, hline_above, shade, vline},
    theme::Palette,
    widgets,
};
use olo_protocol::{
    Client, Event, OutputInfo, Reply, Request, WindowInfo,
    shortcuts::{self, ShortcutSettings},
};

/// Logical width of the vertical Deskbar.
const WIDTH: f32 = olo_kit::metrics::DESKBAR_WIDTH;
const TILE_H: f32 = 30.0;
const ROW_H: f32 = 24.0;
/// A workspace's heading, and Other Workspaces' latch row.
const HEADING_H: f32 = 20.0;
const ICON: f32 = 16.0;
/// The tray: the date and time on a row of their own, then a recessed
/// well of status icons, each a 16 px glyph on a 22 px grid (so 6 px
/// between neighbours, and 3 px from the well's edges), centred in rows
/// of as many as fit.
const CLOCK_H: f32 = 20.0;
/// Inset of the tray's text and well from the Deskbar's edges.
const TRAY_INSET: f32 = 4.0;
const TRAY_PITCH: f32 = 22.0;
/// Below the well, before the applications (on the 2-point grid, so
/// the rows below land on pixels at 1.5×).
const TRAY_FOOT: f32 = 4.0;
/// The tray's height with `rows` rows of icons.
fn tray_height(rows: usize) -> f32 {
    CLOCK_H + rows as f32 * TRAY_PITCH + TRAY_FOOT
}


use olo_kit::apps::{OUR_APPS, PREFERENCE_PANELS, our_app};

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
    Workspace(u32, u32),
    Status(status::Status),
    Power(olo_protocol::PowerStatus),
    Settings(Settings),
    Outputs(Vec<OutputInfo>),
}

/// What Deskbar follows of app_server's settings.
struct Settings {
    clock: olo_protocol::ClockSettings,
    /// Whether the Olo menu shows icons.
    menu_icons: bool,
    /// Grouping, and where auto-tiling is on.
    tiling: olo_protocol::TilingSettings,
    /// The person's shortcut keys.
    keys: ShortcutSettings,
    /// A Deskbar on every display.
    every_display: bool,
}

/// A Deskbar on a display.
struct Bar {
    win: WindowId,
    /// The display it is on (a connector); None for the first, which
    /// app_server puts on the main display, until the displays are known.
    output: Option<String>,
    height: f32,
}

impl Bar {
    fn open(cx: &mut Cx, output: Option<String>) -> Bar {
        let height = TILE_H + tray_height(1);
        let win = cx.open_window(WindowOptions {
            title: "Deskbar".into(),
            size: (WIDTH as u32, height as u32),
            kind: WindowKind::Layer(LayerOptions {
                layer: Layer::Top,
                anchor: (true, false, false, true),
                exclusive_zone: 0,
                margin: (0, 0, 0, 0),
                keyboard: olo_kit::Keyboard::None,
                namespace: "deskbar".into(),
                output: output.clone(),
            }),
            panel: true,
            ..Default::default()
        });
        Bar { win, output, height }
    }
}

/// A team: the windows of one application.
struct Team {
    key: String,
    name: String,
    icon: Option<String>,
    windows: Vec<WindowInfo>,
}

struct Deskbar {
    /// One on the main display, or one on each display.
    bars: Vec<Bar>,
    outputs: Vec<OutputInfo>,
    every_display: bool,
    windows: Vec<WindowInfo>,
    /// Team keys in order of first appearance.
    order: Vec<String>,
    workspace: (u32, u32),
    status: status::Status,
    caffeinated: bool,
    /// A timed caffeinated stretch's end (seconds since the epoch).
    caffeinated_until: Option<u64>,
    caffeinated_lid: bool,
    clock: olo_protocol::ClockSettings,
    /// Appearance ▸ Show icons in the Deskbar menu.
    menu_icons: bool,
    entries: HashMap<String, Option<DesktopEntry>>,
    scroll_acc: f32,
    /// The window that last had the focus: still "this window" for the
    /// Olo menu's commands while the menu holds the keyboard.
    last_focused: Option<u64>,
    /// Apps whose windows aren't grouped (`TilingSettings::ungrouped_apps`),
    /// and where auto-tiling is on.
    tiling: olo_protocol::TilingSettings,
    /// Other Workspaces unfolded.
    others_open: bool,
    /// The display of the Deskbar whose Olo menu is open (its Auto-Tiling
    /// is that display's).
    menu_output: Option<String>,
    /// The person's changes to app_server's shortcuts, for the keys the
    /// Olo menu shows.
    keys: ShortcutSettings,
    /// The Restart / Shut Down confirmation, while it is open.
    confirm: Option<(WindowId, Power)>,
    /// The "upgrade and reload?" alert, while it is open, with its message.
    upgrade_ask: Option<(WindowId, String)>,
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
    std::thread::spawn(move || {
        if let Ok(mut c) = Client::connect() {
            let _ = c.request(&req);
        }
    });
}

impl Deskbar {
    fn entry(&mut self, app_id: &str) -> Option<DesktopEntry> {
        self.entries.entry(app_id.to_owned()).or_insert_with(|| desktop_entry::find(app_id)).clone()
    }

    /// Who opens files dropped on the team `key`: the app itself.
    fn handler(&mut self, key: &str) -> Option<olo_kit::open::Handler> {
        let app_id = self.windows.iter().find(|w| team_key(w) == key)?.app_id.clone();
        let short = app_id.strip_prefix("olo.").unwrap_or(&app_id);
        if let Some((bin, _, _)) = OUR_APPS.iter().find(|(bin, _, _)| bin.strip_prefix("olo-") == Some(short)) {
            return Some(olo_kit::open::Handler::Ours(bin));
        }
        if short == "preferences" {
            return Some(olo_kit::open::Handler::Ours("olo-preferences"));
        }
        self.entry(&app_id).filter(|e| e.exec.is_some()).map(|e| olo_kit::open::Handler::Entry(Box::new(e)))
    }

    /// The program of the team `key`, to drag out as a link: ours, or an
    /// installed application whose command is a program alone.
    fn program(&mut self, key: &str) -> Option<std::path::PathBuf> {
        match self.handler(key)? {
            olo_kit::open::Handler::Ours(bin) => olo_kit::find_program(bin),
            olo_kit::open::Handler::Entry(e) => {
                let command = e.command()?;
                let mut words = command.split_whitespace();
                let (program, None) = (words.next()?, words.next()) else { return None };
                let path = std::path::Path::new(program);
                if path.is_absolute() { path.is_file().then(|| path.to_owned()) } else { olo_kit::find_program(program) }
            }
        }
    }

    /// Dragging the team `team` out: a link to its program, wherever it
    /// is dropped (BeOS's Deskbar made links the same way).
    fn app_drag(&mut self, ctx: &egui::Context, team: &Team) -> Option<olo_kit::dnd::DragData> {
        let program = self.program(&team.key)?;
        let data = olo_kit::dnd::DragData::files(&[program], olo_kit::dnd::Actions::COPY).as_link();
        let icon = team.icon.as_deref().unwrap_or("application");
        Some(match icons::drag_icon(ctx, icon, 32.0, 1, vec2(16.0, 16.0), icons::DragLabel::Below(&team.name)) {
            Some(i) => data.icon(i),
            None => data,
        })
    }

    /// The display whose applications `bar` shows; None for all of them.
    fn shown_on(&self, bar: &Bar) -> Option<String> {
        if !self.every_display {
            return None;
        }
        bar.output.clone().or_else(|| self.main_output())
    }

    /// Whether auto-tiling is on for the workspace showing on `bar`'s
    /// display.
    fn auto_tiling(&self, bar: &Bar) -> bool {
        let output = bar.output.clone().or_else(|| self.main_output());
        let key = self.outputs.iter().find(|o| Some(&o.name) == output.as_ref()).map(|o| o.key.clone()).unwrap_or_default();
        self.tiling.auto_tiling(self.workspace.0, &key, self.every_display)
    }

    /// Auto-tiling on or off on `output`'s display (the active one for
    /// None), for the workspace showing; at once here too, so the tray
    /// shows it before app_server's answer.
    fn set_auto_tiling(&mut self, output: Option<String>, on: bool) {
        let key = self.outputs.iter().find(|o| Some(&o.name) == output.as_ref()).map(|o| o.key.clone()).unwrap_or_default();
        self.tiling.set_auto_tiling(self.workspace.0, &key, self.every_display, on);
        request(Request::SetAutoTiling { on: Some(on), workspace: None, output });
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

    /// A Deskbar on each display, or one on the main display, as asked.
    fn sync_bars(&mut self, cx: &mut Cx) {
        let main = self.main_output();
        let wanted: Vec<Option<String>> = if self.outputs.is_empty() {
            vec![None]
        } else if self.every_display {
            self.outputs.iter().filter(|o| o.mirror_of.is_none()).map(|o| Some(o.name.clone())).collect()
        } else {
            vec![main.clone()]
        };
        // The first, on the main display from the start, is that one's.
        if let Some(bar) = self.bars.iter_mut().find(|b| b.output.is_none())
            && main.is_some()
        {
            bar.output = main;
        }
        let (keep, gone): (Vec<Bar>, Vec<Bar>) = std::mem::take(&mut self.bars).into_iter().partition(|b| wanted.contains(&b.output));
        self.bars = keep;
        for output in wanted {
            if !self.bars.iter().any(|b| b.output == output) {
                self.bars.push(Bar::open(cx, output));
            }
        }
        for bar in gone {
            cx.close_window(bar.win);
        }
    }

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
            let (name, icon) = if let Some((n, i)) = our_app(&app_id) {
                (n.to_owned(), Some(i.to_owned()))
            } else if let Some(e) = self.entry(&app_id) {
                (e.name.clone(), e.icon.clone())
            } else if !app_id.is_empty() {
                let last = app_id.rsplit('.').next().unwrap_or(&app_id);
                let mut name: String = last.to_owned();
                if let Some(f) = name.get_mut(0..1) {
                    f.make_ascii_uppercase();
                }
                (name, Some(app_id.clone()))
            } else {
                (windows[0].title.clone(), None)
            };
            teams.push(Team { key, name, icon, windows });
        }
        teams
    }

    fn set_windows(&mut self, windows: Vec<WindowInfo>) {
        for w in &windows {
            let k = team_key(w);
            if !self.order.contains(&k) {
                self.order.push(k);
            }
        }
        self.order.retain(|k| windows.iter().any(|w| &team_key(w) == k));
        let focused = windows.iter().find(|w| w.focused).map(|w| w.id);
        self.last_focused = focused.or(self.last_focused.filter(|id| windows.iter().any(|w| w.id == *id && !w.minimized)));
        self.windows = windows;
    }

    fn olo_menu(&self, auto_tiling: bool) -> Menu {
        let mut menu = self.olo_menu_with_icons(auto_tiling);
        add_keys(&mut menu, &self.keys);
        if self.menu_icons { menu } else { without_icons(menu) }
    }

    fn olo_menu_with_icons(&self, auto_tiling: bool) -> Menu {
        let mut apps = Menu::new("Applications");
        // Only the ones that are installed (the suite is still growing).
        for (bin, name, icon) in OUR_APPS.iter().filter(|(bin, _, _)| olo_kit::find_program(bin).is_some()) {
            apps = apps.item(*name, format!("run:{bin}"), None).icon(*icon);
        }
        let have_prefs = olo_kit::find_program("olo-preferences").is_some();
        let mut prefs = Menu::new("Preferences");
        for (arg, name, icon) in PREFERENCE_PANELS {
            prefs = prefs.item(*name, format!("pref:{arg}"), None).enabled(have_prefs).icon(*icon);
        }
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
        let all: Vec<DesktopEntry> = desktop_entry::all().into_iter().filter(|e| !e.no_display).collect();
        let mut used = vec![false; all.len()];
        for (title, icon, cats) in groups {
            let mut m = Menu::new(*title);
            for (i, e) in all.iter().enumerate() {
                if !used[i] && e.categories.iter().any(|c| cats.contains(&c.as_str())) {
                    m = m.item(&e.name, format!("desktop:{}", e.id), None).icon(entry_icon(e));
                    used[i] = true;
                }
            }
            if !m.items.is_empty() {
                other = other.submenu(m).icon(*icon);
            }
        }
        let rest: Vec<_> = all.iter().enumerate().filter(|(i, _)| !used[*i]).map(|(_, e)| e).collect();
        if !rest.is_empty() {
            let mut m = Menu::new("Other");
            for e in rest {
                m = m.item(&e.name, format!("desktop:{}", e.id), None).icon(entry_icon(e));
            }
            other = other.submenu(m).icon("folder");
        }

        Menu::new("Olo")
            .item("About Olo Desktop", "about", None)
            .icon("olo")
            .item("Find…", "find", None)
            .icon("query")
            .separator()
            .submenu(self.arrange_menu(auto_tiling))
            .icon("display")
            .submenu(self.workspaces_menu())
            .icon("workspaces")
            .item("Show or Hide All Windows", "arrange:show-desktop", None)
            .icon("desktop")
            .separator()
            .submenu(self.caffeine_menu())
            .icon("stay-awake")
            .item("Lock Screen", "lock", None)
            .icon("lock-screen")
            .item("Log Out", "logout", None)
            .icon("log-out")
            .item("Restart", "restart", None)
            .icon("restart")
            .item("Shut Down", "shutdown", None)
            .icon("shut-down")
            .separator()
            .submenu(apps)
            .icon("application")
            .submenu(other)
            .icon("other-applications")
            .submenu(prefs)
            .icon("preferences")
    }

    /// The focused window (what "Others" leaves alone and a window is
    /// moved from), if any: the last one, as opening this menu takes the
    /// focus.
    fn focused_window(&self) -> Option<u64> {
        self.last_focused.filter(|id| self.windows.iter().any(|w| w.id == *id && w.workspace == self.workspace.0))
    }

    /// The arranging commands app_server's shortcuts do (`arrange:` and a
    /// shortcut id), for the active display.
    fn arrange_menu(&self, auto_tiling: bool) -> Menu {
        let focused = self.focused_window().is_some();
        Menu::new("Window Arrangement")
            .item("Auto-Tiling", "arrange:auto-tiling", None)
            .checked(auto_tiling)
            .separator()
            .item("Tile All", "arrange:tile-all", None)
            .item("Smart Tile", "arrange:smart-tile", None)
            .item("Zoom All", "arrange:zoom-all", None)
            .item("Cascade All", "arrange:cascade-all", None)
            .item("Fan Out", "arrange:fan-out", None)
            .separator()
            .item("Cascade Others", "arrange:cascade-others", None)
            .enabled(focused)
            .item("Fan Out Others", "arrange:fan-out-others", None)
            .enabled(focused)
            .separator()
            .item("Undo Window Arrangement", "arrange:undo-arrangement", None)
            .separator()
            .item("Windows Preferences…", "pref:windows", None)
    }

    /// Go to a workspace, or move the focused window to one.
    fn workspaces_menu(&self) -> Menu {
        let (current, count) = (self.workspace.0, self.workspace.1.clamp(1, 9));
        let mut m = Menu::new("Workspaces");
        for i in 0..count {
            m = m.item(format!("Workspace {}", i + 1), format!("arrange:workspace-{}", i + 1), None).checked(i == current);
        }
        if count > 1 && self.focused_window().is_some() {
            let mut to = Menu::new("Move Window To");
            for i in 0..count {
                to = to.item(format!("Workspace {}", i + 1), format!("arrange:window-to-workspace-{}", i + 1), None).enabled(i != current);
            }
            m = m.separator().submenu(to);
        }
        m.separator().item("Workspaces Preferences…", "pref:workspaces", None)
    }

    fn team_menu(&self, team: &Team) -> Menu {
        let mut m = Menu::new(&team.name);
        for w in &team.windows {
            let title = if w.title.is_empty() { team.name.clone() } else { w.title.clone() };
            let label = if w.workspace != self.workspace.0 {
                format!("{title}  ({})", w.workspace + 1)
            } else {
                title
            };
            m = m.item(label, format!("win:{}", w.id), None).checked(w.focused).minimized(w.minimized);
        }
        // An app of ours with several windows opens another, as BeOS's
        // Deskbar offered.
        if team.windows.first().is_some_and(|w| new_window_app(&w.app_id)) {
            m = m.separator().item("New Window", "new-window", None);
        }
        let app = team.windows.first().map(|w| w.app_id.clone()).unwrap_or_default();
        m.separator()
            .item("Hide All", "hide-all", None)
            .item("Show All", "show-all", None)
            .item("Close All", "close-all", None)
            .separator()
            .item("Group Windows", "group-windows", None)
            .checked(!self.tiling.ungrouped_apps.contains(&app))
            .enabled(!app.is_empty())
    }

    /// Caffeinated mode: indefinitely or for a while, with the lid too.
    fn caffeine_menu(&self) -> Menu {
        let on = self.caffeinated;
        let mut m = Menu::new("Stay Awake").item("Until Turned Off", "caff:on", None).checked(on && self.caffeinated_until.is_none());
        for (label, minutes) in [("For 1 Hour", 60), ("For 2 Hours", 120), ("For 4 Hours", 240), ("For 8 Hours", 480)] {
            m = m.item(label, format!("caff:{minutes}"), None);
        }
        let label = match self.caffeinated_until {
            Some(t) if on => format!("Allow Sleep (awake until {})", status::time_of_day(t, &self.clock)),
            _ => "Allow Sleep".into(),
        };
        m.separator()
            .item("Even With the Lid Closed", "caff:lid", None)
            .checked(self.caffeinated_lid)
            .separator()
            .item(label, "caff:off", None)
            .enabled(on)
    }

    /// A caffeinated-menu command (after `caff:`).
    fn caffeine_command(&mut self, arg: &str) {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
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

    /// A Do Not Disturb command (after `notif:`): `on`, `off`, `tomorrow`
    /// or minutes. Written to the notification settings, which the service
    /// follows.
    fn do_not_disturb(&mut self, arg: &str) {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
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

    fn run_command(&mut self, cx: &mut Cx, cmd: &str) {
        let spawn = |s: &str| {
            if let Err(e) = olo_kit::spawn_detached(s) {
                tracing::warn!("launch {s}: {e}");
            }
        };
        match cmd.split_once(':') {
            Some(("run", bin)) => spawn(bin),
            Some(("pref", panel)) => spawn(&format!("olo-preferences {panel}")),
            Some(("caff", arg)) => self.caffeine_command(arg),
            // For the display of the Deskbar it was chosen on.
            Some(("arrange", "auto-tiling")) => {
                let output = self.menu_output.clone();
                let on = self.bars.iter().find(|b| b.output == output || (output.is_none() && b.output.is_none())).map(|b| self.auto_tiling(b)).unwrap_or(false);
                self.set_auto_tiling(output, !on);
            }
            Some(("arrange", id)) => request(Request::RunShortcut { id: id.into(), window: self.focused_window() }),
            Some(("notif", "history")) => spawn("olo-notifications --history"),
            Some(("notif", arg)) => self.do_not_disturb(arg),
            Some(("desktop", id)) => {
                if let Some(e) = desktop_entry::all().into_iter().find(|e| e.id == id) {
                    let _ = e.launch();
                }
            }
            _ => match cmd {
                "about" => spawn("olo-preferences about"),
                "find" => spawn("olo-tracker --find"),
                "lock" => request(Request::Lock),
                "logout" => request(Request::Quit),
                "restart" => self.confirm_power(cx, Power::Restart),
                "shutdown" => self.confirm_power(cx, Power::ShutDown),
                _ => {}
            },
        }
        self.redraw(cx);
    }

    fn redraw(&self, cx: &mut Cx) {
        for bar in &self.bars {
            cx.redraw(bar.win);
        }
    }

    /// Ask before restarting or shutting down, as BeOS did.
    fn confirm_power(&mut self, cx: &mut Cx, action: Power) {
        if let Some((win, _)) = self.confirm.take() {
            cx.close_window(win);
        }
        let message = action.question();
        let win = cx.open_window(WindowOptions::alert(action.label(), message));
        self.confirm = Some((win, action));
    }

    fn confirm_ui(&mut self, cx: &mut Cx, win: WindowId, ui: &mut egui::Ui) {
        let Some((_, action)) = self.confirm else { return };
        let Some(choice) = widgets::alert(ui, "system", action.question(), &["Cancel", action.label()]) else { return };
        self.confirm = None;
        cx.close_window(win);
        if choice == 1 {
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
    }

    /// Offer the newer build olo-session found.
    fn ask_upgrade(&mut self, cx: &mut Cx) {
        if let Some((win, _)) = self.upgrade_ask.take() {
            cx.close_window(win);
        }
        // A new app_server means a reload, which programs it can't keep
        // (ones that came before olo-wlproxy, or without it) don't
        // survive.
        let closing: Vec<String> = if self.status.upgrade.iter().any(|p| p == "olo-app-server") {
            self.teams(None, None).into_iter().filter(|t| t.windows.iter().any(|w| !w.kept && our_app(&w.app_id).is_none())).map(|t| t.name).collect()
        } else {
            Vec::new()
        };
        let message = upgrade_message(&self.status.upgrade, &closing);
        let win = cx.open_window(WindowOptions::alert("Upgrade Olo", &message));
        self.upgrade_ask = Some((win, message));
    }

    fn upgrade_ui(&mut self, cx: &mut Cx, win: WindowId, ui: &mut egui::Ui) {
        let Some((_, message)) = &self.upgrade_ask else { return };
        let Some(choice) = widgets::alert(ui, "system", message, &["Later", "Upgrade"]) else { return };
        self.upgrade_ask = None;
        cx.close_window(win);
        if choice == 1 {
            // olo-session installs the programs and reloads in place.
            match std::env::var("OLO_SESSION_PID").ok().and_then(|p| p.parse::<i32>().ok()) {
                // SAFETY: sending a signal.
                Some(pid) if unsafe { libc::kill(pid, libc::SIGUSR2) } == 0 => {}
                _ => tracing::warn!("no session to upgrade"),
            }
        }
    }

    /// An application's row at `row`: its icon and name, recessed while
    /// it has the focus. A click brings it forward (with several windows,
    /// opens the menu of them); files dropped on it open with it; dragged
    /// out, it is a link to the program. Under workspace `ws`'s heading,
    /// it is its windows there.
    #[allow(clippy::too_many_arguments)]
    fn team_row(&mut self, cx: &mut Cx, w: WindowId, ui: &mut egui::Ui, row: Rect, team: &Team, display: Option<&str>, ws: Option<u32>) {
        let pal = style::palette(ui.ctx());
        let painter = ui.painter().clone();
        let pointer = ui.input(|i| i.pointer.hover_pos());
        let hover = olo_kit::dnd::hover(ui.ctx()).filter(|h| h.has_files() && h.actions.copy && !h.has(olo_kit::dnd::LINK_TYPE));
        // Files dragged onto an application open with it.
        let drop_here = hover.as_ref().is_some_and(|h| row.contains(h.pos)) && self.handler(&team.key).is_some();
        if drop_here {
            olo_kit::dnd::accept(ui.ctx(), format!("team:{}", team.key), "text/uri-list", olo_kit::dnd::Action::Copy);
        }
        let id = egui::Id::unique(("team", ws, &team.key));
        let resp = ui.interact(row, id, Sense::click_and_drag());
        if resp.drag_started()
            && let Some(data) = self.app_drag(ui.ctx(), team)
        {
            cx.start_drag(w, data);
        }
        let open = menu::is_open(ui.ctx(), id);
        // A click brings an application with one window forward; with
        // more it opens the menu of its windows to choose from, as the
        // right button (or holding the left) always does.
        let several = team.windows.len() > 1;
        if resp.clicked() && !open && !several {
            self.bring_forward(&team.key, display, ws);
        }
        if ((resp.clicked() && several) || resp.secondary_clicked() || resp.long_touched()) && !open {
            menu::request_popup(ui.ctx(), MenuRequest { menu: self.team_menu(team), source: id, anchor: MenuAnchor::Below(row), selected: None });
        }
        if let Some(cmd) = menu::take_chosen(ui.ctx(), id) {
            self.team_command(&team.key, &cmd, display, ws);
        }
        let focused = team.windows.iter().any(|w| w.focused);
        let hidden = team.windows.iter().all(|w| w.minimized);
        let hover = pointer.is_some_and(|p| row.contains(p));
        if open || drop_here {
            painter.rect_filled(row, 0.0, c(pal.menu_selected_background));
        } else if focused {
            // The active application sits slightly recessed.
            painter.rect_filled(row, 0.0, shade(&pal, pal.panel_background, 1.1));
            hline(&painter, row.left(), row.right(), row.top(), shade(&pal, pal.panel_background, 1.25));
        } else if hover {
            painter.rect_filled(row, 0.0, c(olo_theme_tint(pal.panel_background, 0.8)));
        }
        hline_above(&painter, row.left(), row.right(), row.bottom(), shade(&pal, pal.panel_background, 1.14));
        let icon_r = Rect::from_center_size(pos2(row.left() + 6.0 + ICON / 2.0, row.center().y), vec2(ICON, ICON));
        let tint = if hidden { Color32::from_white_alpha(120) } else { Color32::WHITE };
        let drawn = team.icon.as_deref().is_some_and(|i| icons::paint_tinted(&painter, icon_r, i, tint));
        if !drawn {
            icons::paint_tinted(&painter, icon_r, "application", tint);
        }
        let text_color = if open || drop_here {
            c(pal.menu_selected_item_text)
        } else if hidden {
            style::disabled_text(&pal)
        } else {
            c(pal.panel_text)
        };
        let font = if focused { egui::FontId::new(style::PLAIN_SIZE, style::bold()) } else { style::plain_font() };
        let max_w = row.right() - (icon_r.right() + 8.0) - 6.0;
        let mut job = egui::text::LayoutJob::simple_singleline(team.name.clone(), font, text_color);
        job.wrap = egui::text::TextWrapping::truncate_at_width(max_w);
        let g = painter.layout_job(job);
        let pos = style::text_origin(ui.ctx(), &g, icon_r.right() + 8.0, row.center().y, style::PLAIN_SIZE);
        painter.galley(pos, g, text_color);
    }

    /// Workspace `ws`'s heading at `r`, its applications under it: its
    /// name, set in the tray's tone as the date above is. A click goes
    /// there; on the workspace showing (and a right-click on any), the
    /// Workspaces menu.
    fn workspace_heading(&mut self, cx: &mut Cx, ui: &mut egui::Ui, r: Rect, ws: u32) {
        let pal = style::palette(ui.ctx());
        let painter = ui.painter().clone();
        let id = egui::Id::unique(("workspace", ws));
        let name = format!("Workspace {}", ws + 1);
        let here = ws == self.workspace.0;
        let resp = ui.interact(r, id, Sense::click());
        resp.widget_info(|| egui::WidgetInfo { selected: Some(here), ..egui::WidgetInfo::labeled(egui::Role::Button, true, name.clone()) });
        let open = menu::is_open(ui.ctx(), id);
        if !open && (resp.secondary_clicked() || (resp.clicked() && here)) {
            menu::request_popup(ui.ctx(), MenuRequest { menu: self.workspaces_menu(), source: id, anchor: MenuAnchor::Below(r), selected: None });
        } else if resp.clicked() && !open {
            request(Request::SwitchWorkspace { index: ws });
        }
        if let Some(cmd) = menu::take_chosen(ui.ctx(), id) {
            self.run_command(cx, &cmd);
        }
        let fill = if open {
            c(pal.menu_selected_background)
        } else if resp.hovered() && !here {
            c(olo_theme_tint(pal.panel_background, 0.8))
        } else {
            shade(&pal, pal.panel_background, 1.03)
        };
        painter.rect_filled(r, 0.0, fill);
        hline_above(&painter, r.left(), r.right(), r.bottom(), shade(&pal, pal.panel_background, 1.14));
        let ink = if open { c(pal.menu_selected_item_text) } else { c(pal.panel_text) };
        style::label(&painter, r.left() + TRAY_INSET + 5.0, r.center().y, egui::Align::Min, name, egui::FontId::new(style::SMALL_SIZE, style::bold()), ink);
    }

    /// Other Workspaces at `r`: a latch (BOutlineListView's) that unfolds
    /// the applications on the other workspaces, under their headings.
    fn others_latch(&mut self, ui: &mut egui::Ui, r: Rect) {
        let pal = style::palette(ui.ctx());
        let painter = ui.painter().clone();
        let open = self.others_open;
        let resp = ui.interact(r, egui::Id::unique("other-workspaces"), Sense::click());
        resp.widget_info(|| egui::WidgetInfo { selected: Some(open), ..egui::WidgetInfo::labeled(egui::Role::Button, true, "Other Workspaces") });
        if resp.clicked() {
            self.others_open = !open;
            ui.ctx().request_repaint();
        }
        if resp.hovered() {
            painter.rect_filled(r, 0.0, c(olo_theme_tint(pal.panel_background, 0.8)));
        }
        hline_above(&painter, r.left(), r.right(), r.bottom(), shade(&pal, pal.panel_background, 1.14));
        let ink = c(pal.panel_text);
        let dim = style::disabled_text(&pal);
        style::latch(&painter, Rect::from_center_size(pos2(r.left() + 6.0 + ICON / 2.0, r.center().y), vec2(10.0, 10.0)), open, if resp.hovered() { ink } else { dim });
        style::label(&painter, r.left() + 6.0 + ICON + 8.0, r.center().y, egui::Align::Min, "Other Workspaces", style::small_font(), ink);
    }

    /// Show an application's windows on this workspace (minimized ones
    /// too), the one it last had focused in front; with none here, go to
    /// its newest.
    fn bring_forward(&self, team_key: &str, on: Option<&str>, ws: Option<u32>) {
        let team: Vec<&WindowInfo> = self.windows_on(on).filter(|w| team_key == team_key_of(w) && ws.is_none_or(|ws| w.workspace == ws)).collect();
        // Under another workspace's heading: its windows there (and so
        // to that workspace).
        let at = ws.unwrap_or(self.workspace.0);
        let mut here: Vec<&WindowInfo> = team.iter().copied().filter(|w| w.workspace == at).collect();
        if here.is_empty() {
            here.extend(team.last());
        }
        // Ids grow with age: the newest ends in front, unless one has focus.
        here.sort_by_key(|w| (w.focused, w.id));
        for w in here {
            request(Request::Activate { id: w.id });
        }
    }

    fn team_command(&mut self, team_key: &str, cmd: &str, on: Option<&str>, ws: Option<u32>) {
        let ids: Vec<u64> = self.windows_on(on).filter(|w| team_key == team_key_of(w) && ws.is_none_or(|ws| w.workspace == ws)).map(|w| w.id).collect();
        if let Some(id) = cmd.strip_prefix("win:").and_then(|s| s.parse().ok()) {
            request(Request::Activate { id });
            return;
        }
        if cmd == "group-windows" {
            if let Some(app) = self.windows.iter().find(|w| team_key == team_key_of(w)).map(|w| w.app_id.clone()) {
                olo_kit::settings::set_groups_windows(&app, self.tiling.ungrouped_apps.contains(&app));
            }
            return;
        }
        if cmd == "new-window" {
            if let Some(app) = self.windows.iter().find(|w| team_key == team_key_of(w)).map(|w| w.app_id.clone()) {
                std::thread::spawn(move || olo_kit::instance::forward(&app, &["--new-window".to_owned()]));
            }
            return;
        }
        for id in ids {
            match cmd {
                "hide-all" => request(Request::Minimize { id }),
                "show-all" => request(Request::Activate { id }),
                "close-all" => request(Request::Close { id }),
                _ => {}
            }
        }
    }
}

/// Whether `app_id` is one of ours that opens a new window when asked.
fn new_window_app(app_id: &str) -> bool {
    app_id.strip_prefix("olo.").is_some_and(|rest| olo_kit::apps::NEW_WINDOW.contains(&format!("olo-{rest}").as_str()))
}

fn team_key_of(w: &WindowInfo) -> String {
    team_key(w)
}

/// `menu` and its submenus with no icons (Appearance ▸ Show icons in the
/// Deskbar menu, off).
/// Show beside each of the Olo menu's commands that app_server also has
/// as a shortcut (`find`, `lock`, `arrange:` and a shortcut id) the keys
/// the person has for it.
fn add_keys(menu: &mut Menu, changed: &ShortcutSettings) {
    for item in &mut menu.items {
        match item {
            MenuItem::Action(a) => {
                let id = a.command.strip_prefix("arrange:").unwrap_or(&a.command);
                if let Some(info) = shortcuts::by_id(id) {
                    a.keys = shortcuts::keys_for(info, changed).first().map(|c| c.symbols());
                }
            }
            MenuItem::Submenu(m) => add_keys(m, changed),
            MenuItem::Separator => {}
        }
    }
}

fn settings_msg(settings: olo_protocol::Settings) -> Msg {
    Msg::Settings(Settings {
        clock: settings.clock,
        menu_icons: settings.appearance.menu_icons,
        tiling: settings.tiling,
        keys: settings.shortcuts,
        every_display: settings.display.deskbar_on_every_display,
    })
}

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

/// The bell's menu.
fn notifications_menu(quiet: bool) -> Menu {
    Menu::new("Notifications")
        .item("Show History", "notif:history", None)
        .separator()
        .item("Do Not Disturb", if quiet { "notif:off" } else { "notif:on" }, None)
        .checked(quiet)
        .item("Quiet for 1 Hour", "notif:60", None)
        .item("Quiet Until Morning", "notif:tomorrow", None)
        .separator()
        .item("Notification Settings…", "pref:notifications", None)
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
    format!(
        "A new build of Olo is ready ({list}). Installing it restarts the window server: Olo's windows stay open, but {} will close.",
        listing(closing)
    )
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

// ---- drawing ----------------------------------------------------------------

fn section_bevel(p: &egui::Painter, r: Rect, pal: &Palette) {
    hline(p, r.left(), r.right(), r.top(), c(pal.shine));
    vline(p, r.left(), r.top(), r.bottom(), c(pal.shine));
    hline_above(p, r.left(), r.right(), r.bottom(), shade(pal, pal.panel_background, 1.3));
}

impl App for Deskbar {
    fn save_state(&mut self, _cx: &Cx) -> Option<String> {
        Some(if self.others_open { "others-open" } else { "" }.into())
    }

    fn dropped(&mut self, _cx: &mut Cx, _window: WindowId, drop: olo_kit::dnd::Drop) {
        let Some(key) = drop.target.strip_prefix("team:") else { return };
        let paths = drop.paths();
        if paths.is_empty() {
            return;
        }
        let Some(handler) = self.handler(key) else { return };
        std::thread::spawn(move || {
            if let Err(e) = olo_kit::open::open_with(&handler, &paths) {
                tracing::warn!("couldn't open the dropped files with {}: {e}", handler.name());
            }
        });
    }

    fn close_requested(&mut self, cx: &mut Cx, window: WindowId) {
        if self.confirm.is_some_and(|(w, _)| w == window) {
            self.confirm = None;
        }
        if self.upgrade_ask.as_ref().is_some_and(|(w, _)| *w == window) {
            self.upgrade_ask = None;
        }
        if !self.bars.iter().any(|b| b.win == window) {
            cx.close_window(window);
        }
    }

    fn ui(&mut self, cx: &mut Cx, w: WindowId, ui: &mut egui::Ui) {
        if self.confirm.is_some_and(|(win, _)| win == w) {
            self.confirm_ui(cx, w, ui);
            return;
        }
        if self.upgrade_ask.as_ref().is_some_and(|(win, _)| *win == w) {
            self.upgrade_ui(cx, w, ui);
            return;
        }
        let Some(bar) = self.bars.iter().position(|b| b.win == w) else { return };
        let display = self.shown_on(&self.bars[bar]);
        let pal = style::palette(ui.ctx());
        let full = ui.max_rect();
        let painter = ui.painter().clone();
        painter.rect_filled(full, 0.0, c(pal.panel_background));

        // ---- the Olo tile
        let tile = Rect::from_min_size(full.min, vec2(full.width(), TILE_H));
        let tile_id = egui::Id::unique("olo-tile");
        let tile_resp = ui.interact(tile, tile_id, Sense::click());
        let tile_open = menu::is_open(ui.ctx(), tile_id);
        let auto_tiling = self.auto_tiling(&self.bars[bar]);
        if tile_resp.clicked() && !tile_open {
            self.menu_output = self.bars[bar].output.clone();
            menu::request_popup(
                ui.ctx(),
                MenuRequest { menu: self.olo_menu(auto_tiling), source: tile_id, anchor: MenuAnchor::Below(tile), selected: None },
            );
        }
        if let Some(cmd) = menu::take_chosen(ui.ctx(), tile_id) {
            self.run_command(cx, &cmd);
        }
        let hot = tile_resp.hovered() || tile_open;
        style::vgradient(
            &painter,
            tile,
            c(olo_theme_tint(pal.panel_background, if hot { 0.75 } else { 0.85 })),
            shade(&pal, pal.panel_background, 1.04),
        );
        section_bevel(&painter, tile, &pal);
        // The wordmark alone, centred (beside the mark it read as "o olo").
        let dark = pal.mode == olo_kit::theme::ThemeMode::Dark;
        let word_h = 18.0;
        let word_w = olo_kit::brand::wordmark_width(word_h, dark);
        let word = Rect::from_min_size(pos2(tile.center().x - word_w / 2.0, tile.center().y - word_h / 2.0), vec2(word_w, word_h));
        if olo_kit::brand::wordmark(&painter, word, dark).is_none() {
            style::label(&painter, tile.center().x, tile.center().y, egui::Align::Center, "Olo", egui::FontId::new(14.0, style::bold()), c(pal.panel_text));
        }

        // ---- tray: the date and time, then the status icons
        let st = self.status.clone();
        let items = [!st.upgrade.is_empty(), st.agent_screens > 0, st.battery.is_some(), st.wifi.is_some(), st.volume.is_some(), st.notifications.is_some(), true, true]
            .iter()
            .filter(|on| **on)
            .count();
        let well_w = full.width() - 2.0 * TRAY_INSET;
        let per_row = ((well_w / TRAY_PITCH).floor() as usize).max(1);
        let rows = items.div_ceil(per_row);
        let tray = Rect::from_min_size(pos2(full.left(), tile.bottom()), vec2(full.width(), tray_height(rows)));
        painter.rect_filled(tray, 0.0, shade(&pal, pal.panel_background, 1.03));
        let clock_row = Rect::from_min_size(tray.min, vec2(tray.width(), CLOCK_H));
        let (date, time) = status::clock_text(&self.clock);
        let font = style::plain_font();
        let text_left = clock_row.left() + TRAY_INSET + 5.0;
        let text_right = clock_row.right() - TRAY_INSET - 5.0;
        style::label(&painter, text_left, clock_row.center().y, egui::Align::Min, date, font.clone(), c(pal.panel_text));
        style::label(&painter, text_right, clock_row.center().y, egui::Align::Max, time, font, c(pal.panel_text));
        if ui.interact(clock_row, egui::Id::unique("clock"), Sense::click()).on_hover_text("Date & Time preferences").clicked() {
            self.run_command(cx, "pref:datetime");
        }
        let well = Rect::from_min_size(pos2(tray.left() + TRAY_INSET, clock_row.bottom()), vec2(well_w, rows as f32 * TRAY_PITCH));
        style::bevel(&painter, well, shade(&pal, pal.panel_background, 1.25), c(pal.shine));
        // Each icon's place: rows filled in turn, each row centred, all on
        // whole pixels.
        let slots: Vec<Rect> = (0..items)
            .map(|i| {
                let (row, col) = (i / per_row, i % per_row);
                let in_row = (items - row * per_row).min(per_row);
                let left = well.left() + ((well.width() - in_row as f32 * TRAY_PITCH) / 2.0).floor();
                Rect::from_min_size(pos2(left + col as f32 * TRAY_PITCH + 3.0, well.top() + row as f32 * TRAY_PITCH + 3.0), vec2(ICON, ICON))
            })
            .collect();
        let mut slot = slots.iter();
        let mut tray_item = |ui: &mut egui::Ui, id: &str, draw: &dyn Fn(&egui::Painter, Rect), tip: String| {
            let r = *slot.next().expect("a slot for each tray item");
            draw(&painter, r);
            ui.interact(r.expand(3.0), egui::Id::unique(("tray", id)), Sense::click()).on_hover_text(tip)
        };
        if !st.upgrade.is_empty() {
            let n = st.upgrade.len();
            let tip = format!("A new build of Olo is ready ({n} program{}). Click to upgrade.", if n == 1 { "" } else { "s" });
            if tray_item(ui, "upgrade", &|p, r| status::draw_upgrade(p, r, &pal), tip).clicked() {
                self.ask_upgrade(cx);
            }
        }
        // Agents at work on screens of their own: click to watch them.
        if st.agent_screens > 0 {
            let n = st.agent_screens;
            let tip = if n == 1 { "An agent screen is running. Click to watch it.".to_string() } else { format!("{n} agent screens are running. Click to watch them.") };
            if tray_item(ui, "agent-screens", &|p, r| status::draw_screens(p, r, &pal, n), tip).clicked() {
                self.run_command(cx, "run:olo-screen watch");
            }
        }
        if let Some(b) = st.battery {
            let resp = tray_item(
                ui,
                "battery",
                &|p, r| status::draw_battery(p, r, &pal, b.percent, b.charging),
                format!("{}%{}", b.percent, if b.charging { " — charging" } else { "" }),
            );
            if resp.clicked() {
                self.run_command(cx, "pref:power");
            }
        }
        if let Some(w) = st.wifi.clone() {
            let tip = match &w.ssid {
                Some(s) => format!("{s} ({}%)", w.signal),
                None => "Not connected".into(),
            };
            let resp = tray_item(ui, "wifi", &|p, r| status::draw_wifi(p, r, &pal, w.ssid.as_ref().map(|_| w.signal)), tip);
            if resp.clicked() {
                self.run_command(cx, "pref:network");
            }
        }
        if let Some(v) = st.volume {
            let resp = tray_item(
                ui,
                "volume",
                &|p, r| status::draw_volume(p, r, &pal, v.level, v.muted),
                if v.muted { "Muted".into() } else { format!("Volume {}%", (v.level * 100.0).round()) },
            );
            if resp.clicked() {
                status::toggle_mute();
                self.status.volume = Some(status::Volume { muted: !v.muted, ..v });
            }
            if resp.hovered() {
                // One notch (or a trackpad swipe's worth) per 5% step.
                self.scroll_acc += ui.input(|i| {
                    i.events
                        .iter()
                        .map(|e| match e {
                            egui::Event::MouseWheel { delta, unit, .. } => match unit {
                                egui::MouseWheelUnit::Point => delta.y,
                                _ => delta.y * 24.0,
                            },
                            _ => 0.0,
                        })
                        .sum::<f32>()
                });
                let dy = if self.scroll_acc.abs() >= 24.0 {
                    let d = self.scroll_acc.signum();
                    self.scroll_acc = 0.0;
                    d
                } else {
                    0.0
                };
                if dy != 0.0 {
                    let level = (v.level + dy.signum() * 0.05).clamp(0.0, 1.0);
                    status::set_volume(level);
                    self.status.volume = Some(status::Volume { level, ..v });
                }
            }
        }

        // Notifications: a bell with the unread count. Click for the
        // history; right-click for Do Not Disturb.
        if let Some(n) = st.notifications.clone() {
            let tip = match (n.quiet, n.unread) {
                (true, 0) => "Do Not Disturb is on".to_string(),
                (true, u) => format!("Do Not Disturb is on — {u} unread"),
                (false, 0) => "No new notifications".to_string(),
                (false, 1) => "1 new notification".to_string(),
                (false, u) => format!("{u} new notifications"),
            };
            let bell = tray_item(ui, "notifications", &|p, r| status::draw_bell(p, r, &pal, n.unread, n.quiet), tip);
            let bell_id = egui::Id::unique("notifications-menu");
            if bell.clicked() {
                self.run_command(cx, "notif:history");
            }
            if bell.secondary_clicked() && !menu::is_open(ui.ctx(), bell_id) {
                menu::request_popup(
                    ui.ctx(),
                    MenuRequest { menu: notifications_menu(n.quiet), source: bell_id, anchor: MenuAnchor::Below(bell.rect), selected: None },
                );
            }
            if let Some(cmd) = menu::take_chosen(ui.ctx(), bell_id) {
                self.run_command(cx, &cmd);
            }
        }

        // Caffeinated: a cup, steaming while the machine is kept awake.
        // Right-click for a set time or to stay awake with the lid closed.
        let on = self.caffeinated;
        let tip = match (on, self.caffeinated_until) {
            (true, Some(t)) => format!("Caffeinated until {}. Click to allow sleep.", status::time_of_day(t, &self.clock)),
            (true, None) => "Caffeinated: staying awake. Click to allow sleep.".into(),
            _ => "Click to stay awake (caffeinated); right-click for more".into(),
        };
        let cup = tray_item(ui, "caffeine", &|p, r| status::draw_cup(p, r, &pal, on), tip);
        let cup_id = egui::Id::unique("caffeine-menu");
        if cup.clicked() {
            self.caffeinated = !on;
            request(Request::SetCaffeinated { on: !on, minutes: None, lid: false });
        }
        if cup.secondary_clicked() && !menu::is_open(ui.ctx(), cup_id) {
            menu::request_popup(
                ui.ctx(),
                MenuRequest { menu: self.caffeine_menu(), source: cup_id, anchor: MenuAnchor::Below(cup.rect), selected: None },
            );
        }
        if let Some(cmd) = menu::take_chosen(ui.ctx(), cup_id) {
            self.run_command(cx, &cmd);
        }

        // Auto-tiling, beside it: tiles, filled while every window on this
        // display (of this workspace) takes a tile. Right-click for the
        // Window Arrangement menu.
        let tip = if auto_tiling {
            "Auto-tiling: every window here takes a tile. Click to stop.".to_string()
        } else {
            "Click to tile every window here automatically (auto-tiling)".to_string()
        };
        let tiles = tray_item(ui, "auto-tiling", &|p, r| status::draw_tiles(p, r, &pal, auto_tiling), tip);
        let tiles_id = egui::Id::unique("auto-tiling-menu");
        if tiles.clicked() {
            let output = self.bars[bar].output.clone();
            self.set_auto_tiling(output, !auto_tiling);
        }
        if tiles.secondary_clicked() && !menu::is_open(ui.ctx(), tiles_id) {
            self.menu_output = self.bars[bar].output.clone();
            let mut m = self.arrange_menu(auto_tiling);
            add_keys(&mut m, &self.keys);
            menu::request_popup(ui.ctx(), MenuRequest { menu: m, source: tiles_id, anchor: MenuAnchor::Below(tiles.rect), selected: None });
        }
        if let Some(cmd) = menu::take_chosen(ui.ctx(), tiles_id) {
            self.run_command(cx, &cmd);
        }

        // ---- the applications: the workspace showing's, then the rest
        // under Other Workspaces (one workspace: just the applications)
        let mut y = tray.bottom();
        let (current, count) = self.workspace;
        if count <= 1 {
            for team in self.teams(display.as_deref(), None) {
                let row = Rect::from_min_size(pos2(full.left(), y), vec2(full.width(), ROW_H));
                self.team_row(cx, w, ui, row, &team, display.as_deref(), None);
                y += ROW_H;
            }
        } else {
            let mut elsewhere: Vec<u32> = self.windows_on(display.as_deref()).map(|w| w.workspace).filter(|ws| *ws != current).collect();
            elsewhere.sort();
            elsewhere.dedup();
            let heading = |y: f32| Rect::from_min_size(pos2(full.left(), y), vec2(full.width(), HEADING_H));
            self.workspace_heading(cx, ui, heading(y), current);
            y += HEADING_H;
            for team in self.teams(display.as_deref(), Some(current)) {
                let row = Rect::from_min_size(pos2(full.left(), y), vec2(full.width(), ROW_H));
                self.team_row(cx, w, ui, row, &team, display.as_deref(), Some(current));
                y += ROW_H;
            }
            if !elsewhere.is_empty() {
                self.others_latch(ui, heading(y));
                y += HEADING_H;
                if self.others_open {
                    for ws in elsewhere {
                        self.workspace_heading(cx, ui, heading(y), ws);
                        y += HEADING_H;
                        for team in self.teams(display.as_deref(), Some(ws)) {
                            let row = Rect::from_min_size(pos2(full.left(), y), vec2(full.width(), ROW_H));
                            self.team_row(cx, w, ui, row, &team, display.as_deref(), Some(ws));
                            y += ROW_H;
                        }
                    }
                }
            }
        }

        // Outer frame: a dark line on the left and bottom, where the
        // Deskbar meets the desktop.
        let body = Rect::from_min_max(full.min, pos2(full.right(), y));
        vline(&painter, body.left(), body.top(), body.bottom(), shade(&pal, pal.panel_background, 1.6));
        hline_above(&painter, body.left(), body.right(), body.bottom(), shade(&pal, pal.panel_background, 1.6));

        let want = (y - full.top()).ceil();
        let bar = &mut self.bars[bar];
        if (want - bar.height).abs() > 0.5 {
            bar.height = want;
            cx.resize(w, (WIDTH as u32, want as u32));
        }
        // Tick the clock on the minute (or second).
        ui.ctx().request_repaint_after(status::until_next_tick(self.clock.show_seconds));
    }

    fn message(&mut self, cx: &mut Cx, message: Box<dyn std::any::Any + Send>) {
        if let Ok(m) = message.downcast::<Msg>() {
            match *m {
                Msg::Windows(w) => self.set_windows(w),
                Msg::Workspace(cur, count) => self.workspace = (cur, count),
                Msg::Status(s) => self.status = s,
                Msg::Power(p) => {
                    self.caffeinated = p.caffeinated;
                    self.caffeinated_until = p.caffeinated_until;
                    self.caffeinated_lid = p.caffeinated_lid;
                }
                Msg::Settings(s) => {
                    self.clock = s.clock;
                    self.menu_icons = s.menu_icons;
                    self.tiling = s.tiling;
                    self.keys = s.keys;
                    self.every_display = s.every_display;
                    self.sync_bars(cx);
                }
                Msg::Outputs(outputs) => {
                    self.outputs = outputs;
                    self.sync_bars(cx);
                }
            }
            self.redraw(cx);
        }
    }
}

fn olo_theme_tint(rgba: olo_kit::theme::Rgba, t: f32) -> olo_kit::theme::Rgba {
    olo_kit::theme::tint(rgba, t)
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
                    if let Ok(Reply::Workspaces { current, count }) = c.request(&Request::Workspaces) {
                        proxy.send(Msg::Workspace(current, count));
                    }
                    if let Ok(Reply::Power { status }) = c.request(&Request::PowerStatus) {
                        proxy.send(Msg::Power(status));
                    }
                    if let Ok(Reply::Settings { settings }) = c.request(&Request::GetSettings) {
                        proxy.send(settings_msg(settings));
                    }
                    if let Ok(Reply::Outputs { outputs }) = c.request(&Request::Outputs) {
                        proxy.send(Msg::Outputs(outputs));
                    }
                    if let Ok(events) = c.into_events() {
                        for e in events.flatten() {
                            match e {
                                Event::WindowsChanged { windows } => proxy.send(Msg::Windows(windows)),
                                Event::WorkspaceChanged { current, count } => proxy.send(Msg::Workspace(current, count)),
                                Event::PowerChanged { status } => proxy.send(Msg::Power(status)),
                                Event::SettingsChanged { settings } => {
                                    proxy.send(settings_msg(settings))
                                }
                                Event::OutputsChanged { outputs } => proxy.send(Msg::Outputs(outputs)),
                                _ => {}
                            }
                        }
                    }
                }
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
        })
        .expect("thread");
}

fn main() {
    // A session or install from before the rename (Tabula) carries on.
    olo_kit::legacy::adopt();
    olo_kit::run("olo.deskbar", |cx| {
        // Other Workspaces stays unfolded through an upgrade.
        let others_open = cx.restored_state().is_some_and(|s| s.contains("others-open"));
        let bar = Bar::open(cx, None);
        watch_server(cx.proxy());
        let proxy = cx.proxy();
        status::spawn(move |s| proxy.send(Msg::Status(s)));
        Deskbar {
            bars: vec![bar],
            outputs: Vec::new(),
            every_display: false,
            windows: Vec::new(),
            order: Vec::new(),
            workspace: (0, 1),
            status: status::Status::default(),
            caffeinated: false,
            caffeinated_until: None,
            caffeinated_lid: false,
            clock: Default::default(),
            menu_icons: true,
            entries: HashMap::new(),
            scroll_acc: 0.0,
            last_focused: None,
            tiling: Default::default(),
            others_open,
            menu_output: None,
            keys: ShortcutSettings::new(),
            confirm: None,
            upgrade_ask: None,
        }
    });
}
