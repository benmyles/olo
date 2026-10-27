//! Notifications: Do Not Disturb (now, and every day at set times), how
//! long banners stay, and what each app's notifications may do. The
//! settings live in their own file (`notifications.json`), which
//! tabula-notifications and Deskbar's bell follow; a watcher thread keeps
//! the panel current when they change it.

use std::{
    sync::mpsc::{Receiver, channel},
    time::{Duration, Instant, SystemTime},
};

use tabula_kit::{
    egui::{self, Rect, Sense, Ui, UiBuilder, pos2, vec2},
    metrics as m, style,
    style::c,
    widgets::{BBox, CheckBox, MenuField, ScrollView, StringView},
};
use tabula_protocol::{AppNotifications, ClockSettings, NotificationSettings};

/// What the watcher hands over: the settings and the apps known to have
/// sent notifications.
type Loaded = (NotificationSettings, Vec<String>);

pub struct State {
    settings: Option<NotificationSettings>,
    apps: Vec<String>,
    rx: Receiver<Loaded>,
    /// Our own last change: the watcher's echoes of older files are
    /// ignored for a moment after it.
    edited: Option<Instant>,
}

impl Default for State {
    fn default() -> Self {
        let (tx, rx) = channel();
        std::thread::Builder::new()
            .name("notifications-watch".into())
            .spawn(move || {
                // The settings, and the history (for apps new to it).
                let history = tabula_kit::state_dir().join("notifications").join("history.json");
                let mtime = |p: &std::path::Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
                let modified = || (mtime(&NotificationSettings::path()), mtime(&history));
                let mut seen: Option<(Option<SystemTime>, Option<SystemTime>)> = None;
                loop {
                    let m = modified();
                    if seen != Some(m) {
                        seen = Some(m);
                        if tx.send((NotificationSettings::load(), known_apps())).is_err() {
                            return;
                        }
                    }
                    std::thread::sleep(Duration::from_secs(1));
                }
            })
            .expect("thread");
        State { settings: None, apps: Vec::new(), rx, edited: None }
    }
}

/// Apps that have sent notifications (from the history), by name.
fn known_apps() -> Vec<String> {
    let path = tabula_kit::state_dir().join("notifications").join("history.json");
    let v: serde_json::Value = std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let mut apps: Vec<String> =
        v.as_array().into_iter().flatten().filter_map(|e| e["app"].as_str()).filter(|a| !a.is_empty()).map(str::to_owned).collect();
    apps.sort_by_key(|a| a.to_lowercase());
    apps.dedup();
    apps
}

/// "10:30 PM" or "22:30", as the clock shows times.
fn time_text(clock: &ClockSettings, minute: u16) -> String {
    let clock = ClockSettings { show_seconds: false, show_day: false, ..clock.clone() };
    clock.format(minute as u32 / 60, minute as u32 % 60, 0, 0)
}

/// A time of day in half hours.
fn time_menu(ui: &mut Ui, id: &str, label: &str, clock: &ClockSettings, value: &mut u16) {
    let names: Vec<String> = (0..48).map(|i| time_text(clock, i * 30)).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let mut i = (*value / 30).min(47) as usize;
    let before = i;
    let divider = ui.fonts_mut(|f| f.layout_no_wrap(label.to_owned(), style::plain_font(), egui::Color32::BLACK).size().x) + 8.0;
    ui.add(MenuField::new(id, &mut i, &names).label(label).divider(divider).width(divider + 130.0));
    if i != before {
        *value = i as u16 * 30;
    }
}

pub fn ui(ui: &mut Ui, clock: &ClockSettings, st: &mut State) {
    while let Ok((settings, apps)) = st.rx.try_recv() {
        if st.edited.is_none_or(|t| t.elapsed() > Duration::from_millis(1500)) {
            st.settings = Some(settings);
        }
        st.apps = apps;
    }
    ui.ctx().request_repaint_after(Duration::from_secs(1));
    let Some(mut s) = st.settings.clone() else {
        ui.add(StringView::new("Reading…").dim(true));
        return;
    };
    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    BBox::new("Do Not Disturb").show(ui, |ui| {
        ui.add(StringView::new("No banners and no sounds (errors still show).").dim(true));
        let mut on = s.dnd && s.dnd_until.is_none_or(|t| now < t);
        let label = match s.dnd_until {
            Some(t) if on => format!("Do Not Disturb (for {} more minutes)", t.saturating_sub(now).div_ceil(60)),
            _ => "Do Not Disturb".into(),
        };
        if ui.add(CheckBox::new(&mut on, &label)).changed() {
            s.dnd = on;
            s.dnd_until = None;
        }
        let mut daily = s.schedule.is_some();
        if ui.add(CheckBox::new(&mut daily, "Every day at set times")).changed() {
            s.schedule = daily.then_some((22 * 60, 7 * 60));
        }
        if let Some((mut from, mut to)) = s.schedule {
            ui.horizontal(|ui| {
                ui.add_space(20.0);
                time_menu(ui, "dnd-from", "From:", clock, &mut from);
                time_menu(ui, "dnd-to", "to:", clock, &mut to);
            });
            s.schedule = Some((from, to));
        }
    });
    BBox::new("Banners").show(ui, |ui| {
        let choices = [3, 6, 10, 20, 30];
        let names: Vec<String> = choices.iter().map(|s| format!("{s} seconds")).collect();
        let mut names: Vec<&str> = names.iter().map(String::as_str).collect();
        let custom = format!("{} seconds", s.seconds);
        let mut i = match choices.iter().position(|&c| c == s.seconds) {
            Some(i) => i,
            None => {
                names.push(&custom);
                names.len() - 1
            }
        };
        ui.add(MenuField::new("banner-seconds", &mut i, &names).label("Show banners for:").divider(170.0));
        if let Some(&c) = choices.get(i) {
            s.seconds = c;
        }
        ui.add(StringView::new("Unless the app says; kept while the pointer is over it.").dim(true));
    });
    apps_ui(ui, st, &mut s);
    if st.settings.as_ref() != Some(&s) {
        st.settings = Some(s.clone());
        st.edited = Some(Instant::now());
        std::thread::spawn(move || {
            if let Err(e) = s.save() {
                tracing::warn!("notification settings: {e}");
            }
        });
    }
}

/// The apps, each with what its notifications may do.
fn apps_ui(ui: &mut Ui, st: &State, s: &mut NotificationSettings) {
    let p = style::palette(ui.ctx());
    let mut apps: Vec<String> = st.apps.iter().chain(s.apps.keys()).cloned().collect();
    apps.sort_by_key(|a| a.to_lowercase());
    apps.dedup();
    let columns = ["Banners", "Sounds", "History"];
    const COLUMN: f32 = 62.0;
    let rect = ui.available_rect_before_wrap();
    let head = Rect::from_min_size(rect.min, vec2(rect.width(), m::LIST_ROW));
    let font = style::small_font();
    let dim = style::disabled_text(&p);
    style::label(ui.painter(), head.left(), head.center().y, egui::Align::Min, "Applications", style::bold_font(), c(p.panel_text));
    let col_x = |k: usize| head.right() - 2.0 - 14.0 - (columns.len() - k) as f32 * COLUMN + COLUMN / 2.0;
    for (k, name) in columns.iter().enumerate() {
        style::label(ui.painter(), col_x(k), head.center().y, egui::Align::Center, *name, font.clone(), dim);
    }
    let list = Rect::from_min_max(pos2(rect.left(), head.bottom() + 2.0), rect.max);
    ui.scope_builder(UiBuilder::new().max_rect(list), |ui| {
        if apps.is_empty() {
            ui.painter().rect_filled(list, 0.0, c(p.document_background));
            style::sunken(ui.painter(), list, &p, false);
            style::label(
                ui.painter(),
                list.center().x,
                list.center().y,
                egui::Align::Center,
                "Apps appear here once they have sent a notification.",
                font.clone(),
                dim,
            );
            return;
        }
        ui.spacing_mut().item_spacing.y = 0.0;
        ScrollView::new("notification-apps").background(p.document_background).show_rows(ui, m::LIST_ROW, apps.len(), |ui, range| {
            for i in range {
                let app = &apps[i];
                let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), m::LIST_ROW), Sense::hover());
                let mut a = s.app(app);
                let before = a;
                let color = if a.blocked() { dim } else { c(p.document_text) };
                let name_w = r.width() - columns.len() as f32 * COLUMN - 12.0;
                let name = ui.painter().layout(app.clone(), style::plain_font(), color, name_w);
                ui.painter().galley(pos2(r.left() + 6.0, r.center().y - name.size().y / 2.0), name, color);
                let fields: [&mut bool; 3] = [&mut a.banners, &mut a.sounds, &mut a.history];
                for (k, v) in fields.into_iter().enumerate() {
                    let x = r.right() - (columns.len() - k) as f32 * COLUMN + COLUMN / 2.0;
                    let cell = Rect::from_center_size(pos2(x, r.center().y), vec2(14.0, m::LIST_ROW));
                    ui.put(cell, CheckBox::new(v, ""));
                }
                if a != before {
                    if a == AppNotifications::default() {
                        s.apps.remove(app);
                    } else {
                        s.apps.insert(app.clone(), a);
                    }
                }
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times() {
        let mut clock = ClockSettings { twenty_four_hour: true, ..Default::default() };
        assert_eq!(time_text(&clock, 22 * 60 + 30), "22:30");
        clock.twenty_four_hour = false;
        assert_eq!(time_text(&clock, 7 * 60), "7:00 AM");
    }
}
