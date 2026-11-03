//! Notifications: Do Not Disturb (now, and every day at set times), how
//! long banners stay, how the history fades away, and what each app's
//! notifications may do. The
//! settings live in their own file (`notifications.json`), which
//! olo-notifications and Deskbar's bell follow; a watcher thread keeps
//! the panel current when they change it.

use std::{
    sync::{
        Arc,
        mpsc::{Receiver, channel},
    },
    time::{Duration, Instant, SystemTime},
};

use olo_protocol::{AppNotifications, ClockSettings, NotificationSettings};
use olo_ui::{
    gpui::{Bounds, Context, FontWeight, ParentElement as _, ScrollHandle, SharedString, Styled as _, Window, canvas, div, point, px, size},
    metrics as m,
    style::{self, c},
    theme,
    widgets::{CheckBox, MenuField, ScrollView, StringView},
};

use crate::{Msg, PanelView, form::Ui, panel};

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
    /// panel is open (`olo_ui::watch`).
    _watch: Option<olo_ui::watch::Watch>,
    scroll: ScrollHandle,
    /// A frame is due as Do Not Disturb's minutes count down.
    ticking: bool,
}

impl State {
    pub fn new(cx: &mut Context<PanelView>) -> State {
        let (tx, rx) = channel();
        // Tests (the layout lint): the defaults and apps with long names,
        // not this account's files.
        if cfg!(test) {
            drop(tx);
            let apps = ["Telegram", "Google Chrome", "Olo Passwords", "Microsoft Teams (work or school)", "Visual Studio Code - Insiders"].map(str::to_owned).to_vec();
            return State { settings: Some(NotificationSettings::default()), apps, rx, edited: None, _watch: None, scroll: ScrollHandle::new(), ticking: false };
        }
        let proxy = olo_ui::proxy(cx);
        let read = Arc::new(move || {
            // The settings, and the history (for apps new to it).
            let _ = tx.send((NotificationSettings::load(), known_apps()));
            proxy.send(Msg::Redraw);
        });
        let now = read.clone();
        std::thread::Builder::new().name("notifications-read".into()).spawn(move || now()).expect("thread");
        let history = olo_ui::state_dir().join("notifications").join("history.json");
        let watch = olo_ui::watch::watch([NotificationSettings::path(), history], move |_| read());
        State { settings: None, apps: Vec::new(), rx, edited: None, _watch: watch, scroll: ScrollHandle::new(), ticking: false }
    }
}

fn state_of(v: &mut PanelView) -> Option<&mut State> {
    match &mut v.state {
        panel::State::Notifications(st) => Some(st),
        _ => None,
    }
}

/// A change to the settings, saved (off the UI thread).
fn change(v: &mut PanelView, f: impl FnOnce(&mut NotificationSettings)) {
    let Some(st) = state_of(v) else { return };
    let Some(s) = st.settings.as_mut() else { return };
    let before = s.clone();
    f(s);
    if *s == before {
        return;
    }
    st.edited = Some(Instant::now());
    let s = s.clone();
    std::thread::spawn(move || {
        if let Err(e) = s.save() {
            tracing::warn!("notification settings: {e}");
        }
    });
}

/// A control's change to the settings.
fn set<T: 'static>(ui: &Ui, f: impl Fn(&mut NotificationSettings, T) + 'static) -> impl Fn(T, &mut Window, &mut olo_ui::gpui::App) + 'static {
    ui.on_view(move |v, value: T, _, cx| {
        change(v, |s| f(s, value));
        cx.notify();
    })
}

fn check(ui: &mut Ui, label: impl Into<SharedString>, on: bool, f: impl Fn(&mut NotificationSettings, bool) + 'static) {
    let c = CheckBox::new(label, on).enabled(ui.enabled()).on_toggle(set(ui, f));
    ui.add(c);
}

/// Apps that have sent notifications (from the history), by name.
fn known_apps() -> Vec<String> {
    let path = olo_ui::state_dir().join("notifications").join("history.json");
    let v: serde_json::Value = std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let mut apps: Vec<String> = v.as_array().into_iter().flatten().filter_map(|e| e["app"].as_str()).filter(|a| !a.is_empty()).map(str::to_owned).collect();
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
fn time_menu(ui: &mut Ui, id: &str, label: &str, clock: &ClockSettings, value: u16, f: fn(&mut (u16, u16), u16)) -> MenuField {
    let names: Vec<SharedString> = (0..48).map(|i| time_text(clock, i * 30).into()).collect();
    let divider = f32::from(style::text_width(ui.window, label.to_owned(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL))) + 8.0;
    let on = set(ui, move |s, i: usize| {
        if let Some(sched) = s.schedule.as_mut() {
            f(sched, i as u16 * 30);
        }
    });
    MenuField::new(id.to_owned(), names, (value / 30).min(47) as usize).label(label.to_owned()).divider(divider).width(divider + 130.0).on_change(on)
}

pub fn ui(ui: &mut Ui, st: &mut State) {
    while let Ok((settings, apps)) = st.rx.try_recv() {
        if st.edited.is_none_or(|t| t.elapsed() > Duration::from_millis(1500)) {
            st.settings = Some(settings);
        }
        st.apps = apps;
    }
    let Some(s) = st.settings.clone() else {
        ui.note("Reading…");
        return;
    };
    let clock = ui.s.clock.clone();
    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    ui.bbox("Do Not Disturb", |ui| {
        ui.note("No banners and no sounds (errors still show).");
        let on = s.dnd && s.dnd_until.is_none_or(|t| now < t);
        let label = match s.dnd_until {
            Some(t) if on => {
                // Counting down: shown again as the next minute goes.
                if !st.ticking {
                    st.ticking = true;
                    let wait = Duration::from_secs(t.saturating_sub(now) % 60 + 1);
                    ui.cx
                        .spawn(async move |view, cx| {
                            cx.background_executor().timer(wait).await;
                            let _ = view.update(cx, |v, cx| {
                                if let Some(st) = state_of(v) {
                                    st.ticking = false;
                                }
                                cx.notify();
                            });
                        })
                        .detach();
                }
                format!("Do Not Disturb (for {} more minutes)", t.saturating_sub(now).div_ceil(60))
            }
            _ => "Do Not Disturb".into(),
        };
        check(ui, label, on, |s, on| {
            s.dnd = on;
            s.dnd_until = None;
        });
        check(ui, "Every day at set times", s.schedule.is_some(), |s, daily| s.schedule = daily.then_some((22 * 60, 7 * 60)));
        if let Some((from, to)) = s.schedule {
            let from = time_menu(ui, "dnd-from", "From:", &clock, from, |s, v| s.0 = v);
            let to = time_menu(ui, "dnd-to", "to:", &clock, to, |s, v| s.1 = v);
            ui.add(div().flex().flex_row().gap(px(m::SPACING)).pl(px(20.0)).child(from).child(to));
        }
        check(ui, "While sharing the screen (errors wait too)", s.while_sharing, |s, v| s.while_sharing = v);
    });
    ui.bbox("Banners", |ui| {
        // 0: never (errors still show).
        const CHOICES: [u32; 6] = [3, 6, 10, 20, 30, 0];
        let mut names: Vec<SharedString> = CHOICES.iter().map(|&s| if s == 0 { "Never".into() } else { format!("{s} seconds").into() }).collect();
        let i = match CHOICES.iter().position(|&c| c == s.seconds) {
            Some(i) => i,
            None => {
                names.push(format!("{} seconds", s.seconds).into());
                names.len() - 1
            }
        };
        let on = set(ui, |s, i: usize| {
            if let Some(&c) = CHOICES.get(i) {
                s.seconds = c;
            }
        });
        ui.add(MenuField::new("banner-seconds", names, i).label("Show banners for:").divider(170.0).on_change(on));
        ui.note(if s.seconds == 0 { "Only errors show; the rest go to the history." } else { "Unless the app says; kept while the pointer is over it." });
    });
    ui.bbox("History", |ui| {
        const CHOICES: [u32; 8] = [15, 30, 60, 120, 240, 480, 1440, 10080];
        let mut names: Vec<SharedString> = CHOICES.iter().map(|&m| duration_text(m).into()).collect();
        let i = match CHOICES.iter().position(|&c| c == s.fade_minutes) {
            Some(i) => i,
            None => {
                names.push(duration_text(s.fade_minutes).into());
                names.len() - 1
            }
        };
        let on = set(ui, |s, i: usize| {
            if let Some(&c) = CHOICES.get(i) {
                s.fade_minutes = c;
            }
        });
        let fade = CheckBox::new("Fade notifications away over", s.fade).on_toggle(set(ui, |s, v| s.fade = v));
        ui.add(div().flex().flex_row().items_center().gap(px(m::SPACING)).child(fade).child(MenuField::new("fade-minutes", names, i).width(110.0).enabled(s.fade).on_change(on)));
        let strip = s.clone();
        ui.add(canvas(|_, _, _| {}, move |b, _, window, cx| fade_strip(window, cx, b, &strip)).w_full().h(px(10.0 + style::SMALL_SIZE + 4.0)));
        ui.note(if s.fade { "The newest show strongest; each goes once it has faded away." } else { "Kept until you clear them (the last 200)." });
        check(ui, "Only the latest of the same notification", s.one_of_each, |s, v| s.one_of_each = v);
        ui.note("One like it again from the same app (its title and text) takes the earlier one's place while that's still showing or unread.");
    });
    ui.bbox("Deskbar", |ui| {
        check(ui, "Show how many beside each app's dot", s.deskbar_count, |s, v| s.deskbar_count = v);
        ui.note(if s.fade { "An app's dot drains as its notifications fade." } else { "An app with notifications unread has a dot beside its name." });
    });
    apps_ui(ui, st, &s);
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
fn fade_strip(window: &mut Window, cx: &mut olo_ui::gpui::App, r: Bounds<Pixels>, s: &NotificationSettings) {
    let p = theme::palette(cx);
    let bar = style::snap(window, Bounds { origin: r.origin, size: size(r.size.width, px(10.0)) });
    window.with_element_opacity((!s.fade).then_some(0.4), |window| {
        style::fill_rect(window, bar, c(p.document_background));
        let span = s.fade_minutes.max(1) as u64 * 60;
        let steps = (f32::from(bar.size.width) / 2.0).ceil() as usize;
        for k in 0..steps {
            let x0 = bar.left() + px(k as f32 * 2.0);
            let strength = s.strength(span * k as u64 / steps as u64);
            let color = olo_ui::palette::mix(p.document_background, p.tab_focused, strength);
            style::fill_rect(window, Bounds::from_corners(point(x0, bar.top()), point((x0 + px(2.0)).min(bar.right()), bar.bottom())), c(color));
        }
        style::sunken(window, bar, &p, false);
    });
    let dim = style::disabled_text(&p);
    let y = bar.bottom() + px(3.0 + style::SMALL_SIZE / 2.0);
    let end = if s.fade { format!("{} later: gone", duration_text(s.fade_minutes)) } else { "Never fade".into() };
    let font = style::ui_font(FontWeight::NORMAL);
    style::label(window, cx, bar.left(), y, style::Align::Left, "Now", style::SMALL_SIZE, font.clone(), dim);
    style::label(window, cx, bar.right(), y, style::Align::Right, end, style::SMALL_SIZE, font, dim);
}

use olo_ui::gpui::Pixels;

/// Each app's column of choices, as it is now.
fn columns(a: &AppNotifications) -> [bool; 3] {
    [a.banners, a.sounds, a.history]
}

/// App `app`'s choices with column `k` set to `v` (none kept for an app
/// with the defaults).
fn set_column(s: &mut NotificationSettings, app: &str, k: usize, v: bool) {
    let mut a = s.app(app);
    *[&mut a.banners, &mut a.sounds, &mut a.history][k] = v;
    if a == AppNotifications::default() {
        s.apps.remove(app);
    } else {
        s.apps.insert(app.to_owned(), a);
    }
}

/// The apps, each with what its notifications may do.
fn apps_ui(ui: &mut Ui, st: &mut State, s: &NotificationSettings) {
    let p = theme::palette(ui.cx);
    let mut apps: Vec<String> = st.apps.iter().chain(s.apps.keys()).cloned().collect();
    apps.sort_by_key(|a| a.to_lowercase());
    apps.dedup();
    const NAMES: [&str; 3] = ["Banners", "Sounds", "History"];
    const COLUMN: f32 = 62.0;
    let dim = style::disabled_text(&p);
    // The heading, the columns' names smaller on its line.
    let mut head = div().flex().flex_row().items_end().h(px(m::LIST_ROW)).child(div().flex_1().child(StringView::new("Applications").bold(true)));
    for name in NAMES {
        head = head.child(div().w(px(COLUMN)).flex().justify_center().child(StringView::new(name).color(dim).small(true)));
    }
    ui.add(head.pr(px(2.0 + 14.0 - COLUMN / 2.0 + COLUMN / 2.0)));
    if apps.is_empty() {
        ui.add(
            div().flex_1().min_h(px(60.0)).child(
                canvas(
                    |_, _, _| {},
                    move |b, _, window, cx| {
                        let p = theme::palette(cx);
                        style::fill_rect(window, b, c(p.document_background));
                        style::sunken(window, b, &p, false);
                        style::label(
                            window,
                            cx,
                            b.center().x,
                            b.center().y,
                            style::Align::Center,
                            "Apps appear here once they have sent a notification.",
                            style::SMALL_SIZE,
                            style::ui_font(FontWeight::NORMAL),
                            style::disabled_text(&p),
                        );
                    },
                )
                .size_full(),
            ),
        );
        return;
    }
    // All of them at once: each column checked if every app's is, mixed
    // if some are; a click sets the column for every app.
    let all: Vec<[bool; 3]> = apps.iter().map(|a| columns(&s.app(a))).collect();
    let mut row = div().flex().flex_row().items_center().h(px(m::LIST_ROW)).pl(px(6.0)).child(div().flex_1().child(StringView::new("All Apps").bold(true)));
    for k in 0..NAMES.len() {
        let on = all.iter().filter(|a| a[k]).count();
        let names = apps.clone();
        let toggle = set(ui, move |s, v: bool| {
            for app in &names {
                set_column(s, app, k, v);
            }
        });
        row = row.child(div().w(px(COLUMN)).flex().justify_center().child(CheckBox::new("", on == all.len()).mixed(on > 0 && on < all.len()).on_toggle(toggle)));
    }
    // (Its marks over the apps' in the scroll view below: the view's
    // frame and its bar, as the heading.)
    ui.add(row.pr(px(2.0 + olo_ui::widgets::SCROLL_BAR)));
    let mut rows = Vec::new();
    for app in &apps {
        let a = s.app(app);
        let color = if a.blocked() { dim } else { c(p.document_text) };
        let mut r = div().flex().flex_row().items_center().h(px(m::LIST_ROW)).pl(px(6.0)).child(div().flex_1().overflow_hidden().child(StringView::new(app.clone()).color(color)));
        for (k, v) in columns(&a).into_iter().enumerate() {
            let name = app.clone();
            let toggle = set(ui, move |s, v: bool| set_column(s, &name, k, v));
            r = r.child(div().w(px(COLUMN)).flex().justify_center().child(CheckBox::new("", v).on_toggle(toggle)));
        }
        rows.push(r);
    }
    ui.add(div().flex_1().min_h_0().child(ScrollView::new("notification-apps", &st.scroll).background(p.document_background).child(div().flex().flex_col().children(rows))));
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
