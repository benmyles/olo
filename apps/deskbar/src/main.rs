//! Deskbar: the Tabula menu, a status tray and the running applications,
//! docked in the top-right corner of the screen as on BeOS.

mod status;

use std::collections::HashMap;

use tabula_kit::{
    App, Cx, Layer, LayerOptions, Menu, WindowId, WindowKind, WindowOptions,
    desktop_entry::{self, DesktopEntry},
    egui::{self, Color32, Rect, Sense, pos2, vec2},
    icons,
    menu::{self, MenuAnchor, MenuRequest},
    style::{self, c, hline, shade, vline},
    theme::Palette,
    widgets,
};
use tabula_protocol::{Client, Event, Reply, Request, WindowInfo};

/// Logical width of the vertical Deskbar.
const WIDTH: f32 = 172.0;
const TILE_H: f32 = 30.0;
const ROW_H: f32 = 24.0;
const ICON: f32 = 16.0;
/// The tray: the date and time on a row of their own, then a recessed
/// well of status icons, each a 16 px glyph on a 22 px grid (so 6 px
/// between neighbours, and 3 px from the well's edges), centred in rows
/// of as many as fit.
const CLOCK_H: f32 = 20.0;
/// Inset of the tray's text and well from the Deskbar's edges.
const TRAY_INSET: f32 = 4.0;
const TRAY_PITCH: f32 = 22.0;
/// Below the well, before the applications.
const TRAY_FOOT: f32 = 3.0;
/// The tray's height with `rows` rows of icons.
fn tray_height(rows: usize) -> f32 {
    CLOCK_H + rows as f32 * TRAY_PITCH + TRAY_FOOT
}


use tabula_kit::apps::{OUR_APPS, PREFERENCE_PANELS, our_app};

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
    Power(tabula_protocol::PowerStatus),
    Clock(tabula_protocol::ClockSettings),
}

/// A team: the windows of one application.
struct Team {
    key: String,
    name: String,
    icon: Option<String>,
    windows: Vec<WindowInfo>,
}

struct Deskbar {
    win: WindowId,
    windows: Vec<WindowInfo>,
    /// Team keys in order of first appearance.
    order: Vec<String>,
    workspace: (u32, u32),
    status: status::Status,
    caffeinated: bool,
    /// A timed caffeinated stretch's end (seconds since the epoch).
    caffeinated_until: Option<u64>,
    caffeinated_lid: bool,
    clock: tabula_protocol::ClockSettings,
    entries: HashMap<String, Option<DesktopEntry>>,
    height: f32,
    scroll_acc: f32,
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
    fn handler(&mut self, key: &str) -> Option<tabula_kit::open::Handler> {
        let app_id = self.windows.iter().find(|w| team_key(w) == key)?.app_id.clone();
        let short = app_id.strip_prefix("tabula.").unwrap_or(&app_id);
        if let Some((bin, _, _)) = OUR_APPS.iter().find(|(bin, _, _)| bin.strip_prefix("tabula-") == Some(short)) {
            return Some(tabula_kit::open::Handler::Ours(bin));
        }
        if short == "preferences" {
            return Some(tabula_kit::open::Handler::Ours("tabula-preferences"));
        }
        self.entry(&app_id).filter(|e| e.exec.is_some()).map(|e| tabula_kit::open::Handler::Entry(Box::new(e)))
    }

    /// The program of the team `key`, to drag out as a link: ours, or an
    /// installed application whose command is a program alone.
    fn program(&mut self, key: &str) -> Option<std::path::PathBuf> {
        match self.handler(key)? {
            tabula_kit::open::Handler::Ours(bin) => tabula_kit::find_program(bin),
            tabula_kit::open::Handler::Entry(e) => {
                let command = e.command()?;
                let mut words = command.split_whitespace();
                let (program, None) = (words.next()?, words.next()) else { return None };
                let path = std::path::Path::new(program);
                if path.is_absolute() { path.is_file().then(|| path.to_owned()) } else { tabula_kit::find_program(program) }
            }
        }
    }

    /// Dragging the team `team` out: a link to its program, wherever it
    /// is dropped (BeOS's Deskbar made links the same way).
    fn app_drag(&mut self, ctx: &egui::Context, team: &Team) -> Option<tabula_kit::dnd::DragData> {
        let program = self.program(&team.key)?;
        let data = tabula_kit::dnd::DragData::files(&[program], tabula_kit::dnd::Actions::COPY).as_link();
        let icon = team.icon.as_deref().unwrap_or("application");
        Some(match icons::drag_icon(ctx, icon, 32.0, 1, vec2(16.0, 16.0), icons::DragLabel::Below(&team.name)) {
            Some(i) => data.icon(i),
            None => data,
        })
    }

    fn teams(&mut self) -> Vec<Team> {
        let mut teams: Vec<Team> = Vec::new();
        for key in self.order.clone() {
            let windows: Vec<WindowInfo> = self.windows.iter().filter(|w| team_key(w) == key).cloned().collect();
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
        self.windows = windows;
    }

    fn tabula_menu(&self) -> Menu {
        let mut apps = Menu::new("Applications");
        // Only the ones that are installed (the suite is still growing).
        for (bin, name, _) in OUR_APPS.iter().filter(|(bin, _, _)| tabula_kit::find_program(bin).is_some()) {
            apps = apps.item(*name, format!("run:{bin}"), None);
        }
        let have_prefs = tabula_kit::find_program("tabula-preferences").is_some();
        let mut prefs = Menu::new("Preferences");
        for (arg, name, _) in PREFERENCE_PANELS {
            prefs = prefs.item(*name, format!("pref:{arg}"), None).enabled(have_prefs);
        }
        // Everything else installed, by freedesktop category.
        let groups: &[(&str, &[&str])] = &[
            ("Accessories", &["Utility", "Accessories"]),
            ("Development", &["Development"]),
            ("Graphics", &["Graphics"]),
            ("Internet", &["Network", "WebBrowser", "Email"]),
            ("Multimedia", &["AudioVideo", "Audio", "Video"]),
            ("Office", &["Office"]),
            ("Games", &["Game"]),
            ("System", &["System", "Settings"]),
        ];
        let mut other = Menu::new("Other Applications");
        let all: Vec<DesktopEntry> = desktop_entry::all().into_iter().filter(|e| !e.no_display).collect();
        let mut used = vec![false; all.len()];
        for (title, cats) in groups {
            let mut m = Menu::new(*title);
            for (i, e) in all.iter().enumerate() {
                if !used[i] && e.categories.iter().any(|c| cats.contains(&c.as_str())) {
                    m = m.item(&e.name, format!("desktop:{}", e.id), None);
                    used[i] = true;
                }
            }
            if !m.items.is_empty() {
                other = other.submenu(m);
            }
        }
        let rest: Vec<_> = all.iter().enumerate().filter(|(i, _)| !used[*i]).map(|(_, e)| e).collect();
        if !rest.is_empty() {
            let mut m = Menu::new("Other");
            for e in rest {
                m = m.item(&e.name, format!("desktop:{}", e.id), None);
            }
            other = other.submenu(m);
        }

        Menu::new("Tabula")
            .item("About Tabula", "about", None)
            .item("Find…", "find", None)
            .separator()
            .submenu(self.caffeine_menu())
            .item("Lock Screen", "lock", None)
            .item("Log Out", "logout", None)
            .item("Restart", "restart", None)
            .item("Shut Down", "shutdown", None)
            .separator()
            .submenu(apps)
            .submenu(other)
            .submenu(prefs)
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
            m = m.item(label, format!("win:{}", w.id), None).checked(w.focused);
        }
        m.separator()
            .item("Hide All", "hide-all", None)
            .item("Show All", "show-all", None)
            .item("Close All", "close-all", None)
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
            let mut s = tabula_protocol::NotificationSettings::load();
            s.dnd = on;
            s.dnd_until = until;
            if let Err(e) = s.save() {
                tracing::warn!("notification settings: {e}");
            }
        });
    }

    fn run_command(&mut self, cx: &mut Cx, cmd: &str) {
        let spawn = |s: &str| {
            if let Err(e) = tabula_kit::spawn_detached(s) {
                tracing::warn!("launch {s}: {e}");
            }
        };
        match cmd.split_once(':') {
            Some(("run", bin)) => spawn(bin),
            Some(("pref", panel)) => spawn(&format!("tabula-preferences {panel}")),
            Some(("caff", arg)) => self.caffeine_command(arg),
            Some(("notif", "history")) => spawn("tabula-notifications --history"),
            Some(("notif", arg)) => self.do_not_disturb(arg),
            Some(("desktop", id)) => {
                if let Some(e) = desktop_entry::all().into_iter().find(|e| e.id == id) {
                    let _ = e.launch();
                }
            }
            _ => match cmd {
                "about" => spawn("tabula-preferences about"),
                "find" => spawn("tabula-tracker --find"),
                "lock" => request(Request::Lock),
                "logout" => request(Request::Quit),
                "restart" => self.confirm_power(cx, Power::Restart),
                "shutdown" => self.confirm_power(cx, Power::ShutDown),
                _ => {}
            },
        }
        cx.redraw(self.win);
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
                    Power::Restart => tabula_system::power::reboot(),
                    Power::ShutDown => tabula_system::power::power_off(),
                };
                if let Err(e) = r {
                    tracing::warn!("{}: {e}", action.label());
                }
            });
        }
    }

    /// Offer the newer build tabula-session found.
    fn ask_upgrade(&mut self, cx: &mut Cx) {
        if let Some((win, _)) = self.upgrade_ask.take() {
            cx.close_window(win);
        }
        // A new app_server means a reload, which programs not built on
        // Tabula's kit don't survive.
        let closing: Vec<String> = if self.status.upgrade.iter().any(|p| p == "tabula-app-server") {
            self.teams().into_iter().filter(|t| our_app(&t.windows[0].app_id).is_none()).map(|t| t.name).collect()
        } else {
            Vec::new()
        };
        let message = upgrade_message(&self.status.upgrade, &closing);
        let win = cx.open_window(WindowOptions::alert("Upgrade Tabula", &message));
        self.upgrade_ask = Some((win, message));
    }

    fn upgrade_ui(&mut self, cx: &mut Cx, win: WindowId, ui: &mut egui::Ui) {
        let Some((_, message)) = &self.upgrade_ask else { return };
        let Some(choice) = widgets::alert(ui, "system", message, &["Later", "Upgrade"]) else { return };
        self.upgrade_ask = None;
        cx.close_window(win);
        if choice == 1 {
            // tabula-session installs the programs and reloads in place.
            match std::env::var("TABULA_SESSION_PID").ok().and_then(|p| p.parse::<i32>().ok()) {
                // SAFETY: sending a signal.
                Some(pid) if unsafe { libc::kill(pid, libc::SIGUSR2) } == 0 => {}
                _ => tracing::warn!("no session to upgrade"),
            }
        }
    }

    /// Show an application's windows on this workspace (minimized ones
    /// too), the one it last had focused in front; with none here, go to
    /// its newest.
    fn bring_forward(&self, team_key: &str) {
        let team: Vec<&WindowInfo> = self.windows.iter().filter(|w| team_key == team_key_of(w)).collect();
        let mut here: Vec<&WindowInfo> = team.iter().copied().filter(|w| w.workspace == self.workspace.0).collect();
        if here.is_empty() {
            here.extend(team.last());
        }
        // Ids grow with age: the newest ends in front, unless one has focus.
        here.sort_by_key(|w| (w.focused, w.id));
        for w in here {
            request(Request::Activate { id: w.id });
        }
    }

    fn team_command(&mut self, team_key: &str, cmd: &str) {
        let ids: Vec<u64> = self.windows.iter().filter(|w| team_key == team_key_of(w)).map(|w| w.id).collect();
        if let Some(id) = cmd.strip_prefix("win:").and_then(|s| s.parse().ok()) {
            request(Request::Activate { id });
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

fn team_key_of(w: &WindowInfo) -> String {
    team_key(w)
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

/// "NetPositive, Deskbar and 3 others", for the upgrade alert.
fn upgrade_message(programs: &[String], closing: &[String]) -> String {
    let names: Vec<String> = programs
        .iter()
        .map(|bin| match OUR_APPS.iter().find(|(b, _, _)| b == bin) {
            Some((_, name, _)) => name.to_string(),
            None => match bin.as_str() {
                "tabula-deskbar" => "Deskbar".into(),
                "tabula-app-server" => "the window server".into(),
                "tabula-session" => "the session".into(),
                "tabula-preferences" => "Preferences".into(),
                other => other.to_string(),
            },
        })
        .collect();
    let list = listing(&names);
    if closing.is_empty() {
        return format!("A new build of Tabula is ready ({list}). Install it now? Your windows stay open.");
    }
    format!(
        "A new build of Tabula is ready ({list}). Installing it restarts the window server: Tabula's windows stay open, but {} will close.",
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
    hline(p, r.left(), r.right(), r.bottom() - 1.0, shade(pal, pal.panel_background, 1.3));
}

impl App for Deskbar {
    fn dropped(&mut self, _cx: &mut Cx, _window: WindowId, drop: tabula_kit::dnd::Drop) {
        let Some(key) = drop.target.strip_prefix("team:") else { return };
        let paths = drop.paths();
        if paths.is_empty() {
            return;
        }
        let Some(handler) = self.handler(key) else { return };
        std::thread::spawn(move || {
            if let Err(e) = tabula_kit::open::open_with(&handler, &paths) {
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
        if window != self.win {
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
        let pal = style::palette(ui.ctx());
        let full = ui.max_rect();
        let painter = ui.painter().clone();
        painter.rect_filled(full, 0.0, c(pal.panel_background));
        let teams = self.teams();

        // ---- the Tabula tile
        let tile = Rect::from_min_size(full.min, vec2(full.width(), TILE_H));
        let tile_id = egui::Id::unique("tabula-tile");
        let tile_resp = ui.interact(tile, tile_id, Sense::click());
        let tile_open = menu::is_open(ui.ctx(), tile_id);
        if tile_resp.clicked() && !tile_open {
            menu::request_popup(
                ui.ctx(),
                MenuRequest { menu: self.tabula_menu(), source: tile_id, anchor: MenuAnchor::Below(tile), selected: None },
            );
        }
        if let Some(cmd) = menu::take_chosen(ui.ctx(), tile_id) {
            self.run_command(cx, &cmd);
        }
        let hot = tile_resp.hovered() || tile_open;
        style::vgradient(
            &painter,
            tile,
            c(tabula_theme_tint(pal.panel_background, if hot { 0.75 } else { 0.85 })),
            shade(&pal, pal.panel_background, 1.04),
        );
        section_bevel(&painter, tile, &pal);
        let logo = Rect::from_center_size(pos2(tile.left() + 22.0, tile.center().y), vec2(22.0, 22.0));
        if !icons::paint(&painter, logo, "tabula") {
            icons::paint(&painter, logo, "application");
        }
        style::label(&painter, logo.right() + 8.0, tile.center().y, egui::Align::Min, "Tabula", egui::FontId::new(14.0, style::bold()), c(pal.panel_text));

        // ---- tray: the date and time, then the status icons
        let st = self.status.clone();
        let items = [!st.upgrade.is_empty(), st.agent_screens > 0, st.battery.is_some(), st.wifi.is_some(), st.volume.is_some(), st.notifications.is_some(), true]
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
            let tip = format!("A new build of Tabula is ready ({n} program{}). Click to upgrade.", if n == 1 { "" } else { "s" });
            if tray_item(ui, "upgrade", &|p, r| status::draw_upgrade(p, r, &pal), tip).clicked() {
                self.ask_upgrade(cx);
            }
        }
        // Agents at work on screens of their own: click to watch them.
        if st.agent_screens > 0 {
            let n = st.agent_screens;
            let tip = if n == 1 { "An agent screen is running. Click to watch it.".to_string() } else { format!("{n} agent screens are running. Click to watch them.") };
            if tray_item(ui, "agent-screens", &|p, r| status::draw_screens(p, r, &pal, n), tip).clicked() {
                self.run_command(cx, "run:tabula-screen watch");
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

        // ---- teams
        let mut y = tray.bottom();
        let pointer = ui.input(|i| i.pointer.hover_pos());
        let hover = tabula_kit::dnd::hover(ui.ctx()).filter(|h| h.has_files() && h.actions.copy && !h.has(tabula_kit::dnd::LINK_TYPE));
        for team in &teams {
            let row = Rect::from_min_size(pos2(full.left(), y), vec2(full.width(), ROW_H));
            // Files dragged onto an application open with it.
            let drop_here = hover.as_ref().is_some_and(|h| row.contains(h.pos)) && self.handler(&team.key).is_some();
            if drop_here {
                tabula_kit::dnd::accept(ui.ctx(), format!("team:{}", team.key), "text/uri-list", tabula_kit::dnd::Action::Copy);
            }
            let id = egui::Id::unique(("team", &team.key));
            let resp = ui.interact(row, id, Sense::click_and_drag());
            if resp.drag_started()
                && let Some(data) = self.app_drag(ui.ctx(), team)
            {
                cx.start_drag(self.win, data);
            }
            let open = menu::is_open(ui.ctx(), id);
            // A click brings an application with one window forward; with
            // more it opens the menu of its windows to choose from, as the
            // right button (or holding the left) always does.
            let several = team.windows.len() > 1;
            if resp.clicked() && !open && !several {
                self.bring_forward(&team.key);
            }
            if ((resp.clicked() && several) || resp.secondary_clicked() || resp.long_touched()) && !open {
                menu::request_popup(
                    ui.ctx(),
                    MenuRequest {
                        menu: self.team_menu(team),
                        source: id,
                        anchor: MenuAnchor::Below(Rect::from_min_max(pos2(row.left(), row.top()), pos2(row.right(), row.bottom()))),
                        selected: None,
                    },
                );
            }
            if let Some(cmd) = menu::take_chosen(ui.ctx(), id) {
                self.team_command(&team.key, &cmd);
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
                painter.rect_filled(row, 0.0, c(tabula_theme_tint(pal.panel_background, 0.8)));
            }
            hline(&painter, row.left(), row.right(), row.bottom() - 1.0, shade(&pal, pal.panel_background, 1.14));
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
            let size = style::PLAIN_SIZE;
            let pos = style::text_origin(ui.ctx(), &g, icon_r.right() + 8.0, row.center().y, size);
            painter.galley(pos, g, text_color);
            y += ROW_H;
        }

        // Outer frame: a dark line on the left and bottom, where the
        // Deskbar meets the desktop.
        let body = Rect::from_min_max(full.min, pos2(full.right(), y));
        vline(&painter, body.left(), body.top(), body.bottom(), shade(&pal, pal.panel_background, 1.6));
        hline(&painter, body.left(), body.right(), body.bottom() - 1.0, shade(&pal, pal.panel_background, 1.6));

        let want = (y - full.top()).ceil();
        if (want - self.height).abs() > 0.5 {
            self.height = want;
            cx.resize(self.win, (WIDTH as u32, want as u32));
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
                Msg::Clock(clock) => self.clock = clock,
            }
            cx.redraw(self.win);
        }
    }
}

fn tabula_theme_tint(rgba: tabula_kit::theme::Rgba, t: f32) -> tabula_kit::theme::Rgba {
    tabula_kit::theme::tint(rgba, t)
}

fn watch_server(proxy: tabula_kit::Proxy) {
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
                        proxy.send(Msg::Clock(settings.clock));
                    }
                    if let Ok(events) = c.into_events() {
                        for e in events.flatten() {
                            match e {
                                Event::WindowsChanged { windows } => proxy.send(Msg::Windows(windows)),
                                Event::WorkspaceChanged { current, count } => proxy.send(Msg::Workspace(current, count)),
                                Event::PowerChanged { status } => proxy.send(Msg::Power(status)),
                                Event::SettingsChanged { settings } => proxy.send(Msg::Clock(settings.clock)),
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
    tabula_kit::run("tabula.deskbar", |cx| {
        let win = cx.open_window(WindowOptions {
            title: "Deskbar".into(),
            size: (WIDTH as u32, (TILE_H + tray_height(1)) as u32),
            kind: WindowKind::Layer(LayerOptions {
                layer: Layer::Top,
                anchor: (true, false, false, true),
                exclusive_zone: 0,
                margin: (0, 0, 0, 0),
                keyboard: tabula_kit::Keyboard::None,
                namespace: "deskbar".into(),
                output: None,
            }),
            panel: true,
            ..Default::default()
        });
        watch_server(cx.proxy());
        let proxy = cx.proxy();
        status::spawn(move |s| proxy.send(Msg::Status(s)));
        Deskbar {
            win,
            windows: Vec::new(),
            order: Vec::new(),
            workspace: (0, 1),
            status: status::Status::default(),
            caffeinated: false,
            caffeinated_until: None,
            caffeinated_lid: false,
            clock: Default::default(),
            entries: HashMap::new(),
            height: TILE_H + tray_height(1),
            scroll_acc: 0.0,
            confirm: None,
            upgrade_ask: None,
        }
    });
}
