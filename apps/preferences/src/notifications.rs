//! Notifications: Do Not Disturb (now, and every day at set times), how
//! long banners stay, how the history fades away, and what each app's
//! notifications may do. The
//! settings live in their own file (`notifications.json`), which
//! olo-notifications and Deskbar's bell follow; a watcher thread keeps
//! the panel current when they change it.

use std::{
    sync::{
        Arc, Mutex,
        mpsc::{Receiver, channel},
    },
    time::{Duration, Instant, SystemTime},
};

use olo_kit::{
    egui::{self, Rect, Sense, Ui, UiBuilder, pos2, vec2},
    metrics as m, style,
    style::c,
    widgets::{BBox, CheckBox, MenuField, ScrollView, StringView},
};
use olo_protocol::{AppNotifications, ClockSettings, NotificationSettings};

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
    /// The settings and the history, followed as they change while the
    /// panel is open (`olo_kit::watch`); the context to wake with them.
    _watch: Option<olo_kit::watch::Watch>,
    ctx: Arc<Mutex<Option<egui::Context>>>,
}

impl Default for State {
    fn default() -> Self {
        let (tx, rx) = channel();
        let ctx: Arc<Mutex<Option<egui::Context>>> = Arc::default();
        let read = {
            let ctx = ctx.clone();
            Arc::new(move || {
                // The settings, and the history (for apps new to it).
                let _ = tx.send((NotificationSettings::load(), known_apps()));
                if let Some(c) = ctx.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
                    c.request_repaint();
                }
            })
        };
        let now = read.clone();
        std::thread::Builder::new().name("notifications-read".into()).spawn(move || now()).expect("thread");
        let history = olo_kit::state_dir().join("notifications").join("history.json");
        let watch = olo_kit::watch::watch([NotificationSettings::path(), history], move |_| read());
        State { settings: None, apps: Vec::new(), rx, edited: None, _watch: watch, ctx }
    }
}

#[cfg(test)]
impl State {
    /// The default settings and apps with long names, already read, with
    /// no watcher (for the layout lint).
    pub fn sample() -> State {
        let apps = ["Google Chrome", "Terminal", "olo.some-very-long-application-identifier"].map(str::to_owned).to_vec();
        State { settings: Some(NotificationSettings::default()), apps, rx: channel().1, edited: None, _watch: None, ctx: Arc::default() }
    }
}

/// Apps that have sent notifications (from the history), by name.
fn known_apps() -> Vec<String> {
    let path = olo_kit::state_dir().join("notifications").join("history.json");
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
    st.ctx.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(|| ui.ctx().clone());
    while let Ok((settings, apps)) = st.rx.try_recv() {
        if st.edited.is_none_or(|t| t.elapsed() > Duration::from_millis(1500)) {
            st.settings = Some(settings);
        }
        st.apps = apps;
    }
    let Some(mut s) = st.settings.clone() else {
        ui.add(StringView::new("Reading…").dim(true));
        return;
    };
    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    BBox::new("Do Not Disturb").show(ui, |ui| {
        ui.add(StringView::new("No banners and no sounds (errors still show).").dim(true));
        let mut on = s.dnd && s.dnd_until.is_none_or(|t| now < t);
        let label = match s.dnd_until {
            Some(t) if on => {
                // Counting down: shown again as the next minute goes.
                ui.ctx().request_repaint_after(Duration::from_secs(t.saturating_sub(now) % 60 + 1));
                format!("Do Not Disturb (for {} more minutes)", t.saturating_sub(now).div_ceil(60))
            }
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
        ui.add(CheckBox::new(&mut s.while_sharing, "While sharing the screen (errors wait too)"));
    });
    BBox::new("Banners").show(ui, |ui| {
        // 0: never (errors still show).
        let choices = [3, 6, 10, 20, 30, 0];
        let names: Vec<String> = choices.iter().map(|&s| if s == 0 { "Never".to_owned() } else { format!("{s} seconds") }).collect();
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
        let note = if s.seconds == 0 { "Only errors show; the rest go to the history." } else { "Unless the app says; kept while the pointer is over it." };
        ui.add(StringView::new(note).dim(true));
    });
    BBox::new("History").show(ui, |ui| {
        let choices = [15, 30, 60, 120, 240, 480, 1440, 10080];
        let names: Vec<String> = choices.iter().map(|&m| duration_text(m)).collect();
        let mut names: Vec<&str> = names.iter().map(String::as_str).collect();
        let custom = duration_text(s.fade_minutes);
        let mut i = match choices.iter().position(|&c| c == s.fade_minutes) {
            Some(i) => i,
            None => {
                names.push(&custom);
                names.len() - 1
            }
        };
        ui.horizontal(|ui| {
            ui.add(CheckBox::new(&mut s.fade, "Fade notifications away over"));
            ui.add_enabled_ui(s.fade, |ui| ui.add(MenuField::new("fade-minutes", &mut i, &names).width(110.0)));
        });
        if let Some(&c) = choices.get(i) {
            s.fade_minutes = c;
        }
        fade_strip(ui, &s);
        let note = if s.fade {
            "The newest show strongest; each goes once it has faded away."
        } else {
            "Kept until you clear them (the last 200)."
        };
        ui.add(StringView::new(note).dim(true).wrap(true));
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

/// "15 minutes", "1 hour", "8 hours", "1 day", "1 week".
fn duration_text(minutes: u32) -> String {
    let unit = |n: u32, one: &str| if n == 1 { format!("1 {one}") } else { format!("{n} {one}s") };
    match minutes {
        m if m > 0 && m % 10080 == 0 => unit(m / 10080, "week"),
        m if m > 0 && m % 1440 == 0 => unit(m / 1440, "day"),
        m if m > 0 && m % 60 == 0 => unit(m / 60, "hour"),
        m => unit(m, "minute"),
    }
}

/// How the history fades: a sunken strip lit in the yellow of a tab in
/// front (as unread notifications are marked) where a notification is
/// new, fading as it ages, with now at its left and the end at its right
/// (with fading off, lit evenly and dimmed, as a disabled control is).
fn fade_strip(ui: &mut Ui, s: &NotificationSettings) {
    let p = style::palette(ui.ctx());
    let font = style::small_font();
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 10.0 + font.size + 4.0), Sense::hover());
    let bar = style::snap(ui.ctx(), Rect::from_min_size(r.min, vec2(r.width(), 10.0)));
    let mut painter = ui.painter().clone();
    if !s.fade {
        painter.multiply_opacity(0.4);
    }
    painter.rect_filled(bar, 0.0, c(p.document_background));
    let span = s.fade_minutes.max(1) as u64 * 60;
    let steps = (bar.width() / 2.0).ceil() as usize;
    for k in 0..steps {
        let x0 = bar.left() + k as f32 * 2.0;
        let strength = s.strength(span * k as u64 / steps as u64);
        let color = olo_kit::theme::mix(p.document_background, p.tab_focused, strength);
        painter.rect_filled(Rect::from_min_max(pos2(x0, bar.top()), pos2((x0 + 2.0).min(bar.right()), bar.bottom())), 0.0, c(color));
    }
    style::sunken(&painter, bar, &p, false);
    let dim = style::disabled_text(&p);
    let y = bar.bottom() + 3.0 + font.size / 2.0;
    let end = if s.fade { format!("{} later: gone", duration_text(s.fade_minutes)) } else { "Never fade".into() };
    style::label(&painter, bar.left(), y, egui::Align::Min, "Now", font.clone(), dim);
    style::label(&painter, bar.right(), y, egui::Align::Max, end, font, dim);
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
    // A table of its own: its column headings line up with its checkboxes,
    // not with the History box's labels above.
    olo_kit::layout_review::own_grid(ui, rect);
    let head = Rect::from_min_size(rect.min, vec2(rect.width(), m::LIST_ROW));
    let font = style::small_font();
    let dim = style::disabled_text(&p);
    let (_, baseline) = style::label_with_baseline(ui.painter(), head.left(), head.center().y, egui::Align::Min, "Applications", style::bold_font(), c(p.panel_text));
    let col_x = |k: usize| head.right() - 2.0 - 14.0 - (columns.len() - k) as f32 * COLUMN + COLUMN / 2.0;
    // Smaller, on the heading's line.
    for (k, name) in columns.iter().enumerate() {
        style::label_on_baseline(ui.painter(), col_x(k), baseline, egui::Align::Center, *name, font.clone(), dim);
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
    fn durations() {
        assert_eq!(duration_text(15), "15 minutes");
        assert_eq!(duration_text(60), "1 hour");
        assert_eq!(duration_text(480), "8 hours");
        assert_eq!(duration_text(1440), "1 day");
        assert_eq!(duration_text(10080), "1 week");
        assert_eq!(duration_text(90), "90 minutes");
    }

    #[test]
    fn times() {
        let mut clock = ClockSettings { twenty_four_hour: true, ..Default::default() };
        assert_eq!(time_text(&clock, 22 * 60 + 30), "22:30");
        clock.twenty_four_hour = false;
        assert_eq!(time_text(&clock, 7 * 60), "7:00 AM");
    }
}
