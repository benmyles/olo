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
const TRAY_H: f32 = 22.0;
const ROW_H: f32 = 24.0;
const ICON: f32 = 16.0;
/// Inset of the tray's recessed well from the Deskbar's edges.
const TRAY_INSET: f32 = 4.0;


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
    show_date: bool,
    entries: HashMap<String, Option<DesktopEntry>>,
    height: f32,
    scroll_acc: f32,
    /// The Restart / Shut Down confirmation, while it is open.
    confirm: Option<(WindowId, Power)>,
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

    fn run_command(&mut self, cx: &mut Cx, cmd: &str) {
        let spawn = |s: &str| {
            if let Err(e) = tabula_kit::spawn_detached(s) {
                tracing::warn!("launch {s}: {e}");
            }
        };
        match cmd.split_once(':') {
            Some(("run", bin)) => spawn(bin),
            Some(("pref", panel)) => spawn(&format!("tabula-preferences {panel}")),
            Some(("desktop", id)) => {
                if let Some(e) = desktop_entry::all().into_iter().find(|e| e.id == id) {
                    let _ = e.launch();
                }
            }
            _ => match cmd {
                "about" => spawn("tabula-preferences about"),
                "find" => spawn("tabula-tracker --find"),
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
        let h = widgets::alert_height(message);
        let win = cx.open_window(WindowOptions::new(action.label(), widgets::ALERT_WIDTH, h).min_size(widgets::ALERT_WIDTH, h));
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

// ---- drawing ----------------------------------------------------------------

fn section_bevel(p: &egui::Painter, r: Rect, pal: &Palette) {
    hline(p, r.left(), r.right(), r.top(), c(pal.shine));
    vline(p, r.left(), r.top(), r.bottom(), c(pal.shine));
    hline(p, r.left(), r.right(), r.bottom() - 1.0, shade(pal, pal.panel_background, 1.3));
}

impl App for Deskbar {
    fn close_requested(&mut self, cx: &mut Cx, window: WindowId) {
        if self.confirm.is_some_and(|(w, _)| w == window) {
            self.confirm = None;
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

        // ---- tray
        let tray = Rect::from_min_size(pos2(full.left(), tile.bottom()), vec2(full.width(), TRAY_H));
        painter.rect_filled(tray, 0.0, shade(&pal, pal.panel_background, 1.03));
        style::bevel(&painter, tray.shrink2(vec2(TRAY_INSET, 2.0)), shade(&pal, pal.panel_background, 1.25), c(pal.shine));
        let clock = status::clock_text(self.show_date);
        let font = style::plain_font();
        let clock_rect = style::label(&painter, tray.right() - TRAY_INSET - 5.0, tray.center().y, egui::Align::Max, clock, font, c(pal.panel_text)).expand(3.0);
        if ui.interact(clock_rect, egui::Id::unique("clock"), Sense::click()).clicked() {
            self.show_date = !self.show_date;
        }
        let mut x = tray.left() + TRAY_INSET + 5.0;
        let cy = tray.center().y;
        let mut tray_item = |ui: &mut egui::Ui, id: &str, draw: &dyn Fn(&egui::Painter, Rect), tip: String| {
            let r = Rect::from_center_size(pos2(x + 8.0, cy), vec2(16.0, 16.0));
            draw(&painter, r);
            x += 21.0;
            ui.interact(r.expand(2.0), egui::Id::unique(("tray", id)), Sense::click()).on_hover_text(tip)
        };
        let st = self.status.clone();
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

        // Caffeinated: a cup, steaming while the machine is kept awake.
        let on = self.caffeinated;
        let tip = if on { "Caffeinated: staying awake. Click to allow sleep." } else { "Click to stay awake (caffeinated)" };
        if tray_item(ui, "caffeine", &|p, r| status::draw_cup(p, r, &pal, on), tip.into()).clicked() {
            self.caffeinated = !on;
            request(Request::SetCaffeinated { on: !on });
        }

        // ---- teams
        let mut y = tray.bottom();
        let pointer = ui.input(|i| i.pointer.hover_pos());
        for team in &teams {
            let row = Rect::from_min_size(pos2(full.left(), y), vec2(full.width(), ROW_H));
            let id = egui::Id::unique(("team", &team.key));
            let resp = ui.interact(row, id, Sense::click());
            let open = menu::is_open(ui.ctx(), id);
            if (resp.clicked() || resp.secondary_clicked()) && !open {
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
            if open {
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
            let text_color = if open {
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
        // Tick the clock on the minute.
        ui.ctx().request_repaint_after(status::until_next_minute());
    }

    fn message(&mut self, cx: &mut Cx, message: Box<dyn std::any::Any + Send>) {
        if let Ok(m) = message.downcast::<Msg>() {
            match *m {
                Msg::Windows(w) => self.set_windows(w),
                Msg::Workspace(cur, count) => self.workspace = (cur, count),
                Msg::Status(s) => self.status = s,
                Msg::Power(p) => self.caffeinated = p.caffeinated,
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
                    if let Ok(events) = c.into_events() {
                        for e in events.flatten() {
                            match e {
                                Event::WindowsChanged { windows } => proxy.send(Msg::Windows(windows)),
                                Event::WorkspaceChanged { current, count } => proxy.send(Msg::Workspace(current, count)),
                                Event::PowerChanged { status } => proxy.send(Msg::Power(status)),
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
            size: (WIDTH as u32, (TILE_H + TRAY_H) as u32),
            kind: WindowKind::Layer(LayerOptions {
                layer: Layer::Top,
                anchor: (true, false, false, true),
                exclusive_zone: 0,
                margin: (0, 0, 0, 0),
                keyboard: false,
                namespace: "deskbar".into(),
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
            show_date: false,
            entries: HashMap::new(),
            height: TILE_H + TRAY_H,
            scroll_acc: 0.0,
            confirm: None,
        }
    });
}
