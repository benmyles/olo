//! Date & Time: R5's Time preferences. A calendar and a clock face to set
//! the clock by hand, network time, the time zone, and how Deskbar shows
//! the time. The clock and zone belong to systemd-timedated
//! (`olo_system::time`): one worker thread reads them every couple of
//! seconds and makes the changes, which polkit may stop to ask for a
//! password, so the window never waits on it.

use std::{
    sync::{
        Arc, OnceLock,
        mpsc::{RecvTimeoutError, Sender, channel},
    },
    time::Duration,
};

use jiff::{
    Timestamp, Zoned,
    civil::{Date, Time},
    tz::TimeZone,
};
use olo_kit::{
    Bounds, Child, Cx, FontWeight, Hsla, InteractiveElement as _, MouseButton, MouseDownEvent, Painter, ParentElement as _, PathBuilder, Pixels, Point, SharedString, Styled as _, WindowCx, canvas,
    div, metrics as m, point, px, size,
    style::{self, c},
    theme,
    widgets::{Button, Column, ColumnListView, ListModel, StringView, TabView, TextControl, TextField, TextFieldEvent},
};
use olo_protocol::ClockSettings;
use olo_sync::Mutex;
use olo_system::time::{self as systime, TimeStatus};

use crate::{Msg, PanelView, form::Ui, panel};

enum Request {
    Time(i64),
    Zone(String),
    Ntp(bool),
    /// timedated said something changed: read it again.
    Refresh,
}

/// How often the clock is asked whether it's synchronised, while network
/// time is on and it isn't yet (timedated doesn't say when it is).
const SYNC_ASK_EVERY: Duration = Duration::from_secs(10);

#[derive(Default)]
struct Shared {
    /// None until first read; Err while timedated can't be reached.
    status: Option<Result<TimeStatus, String>>,
    zones: Arc<Vec<String>>,
    /// A change is on its way (polkit may be asking for a password).
    busy: bool,
    error: Option<String>,
    /// Windows not shown until the status is in (let go as it comes).
    holds: Vec<olo_kit::FirstShowHold>,
}

struct Link {
    shared: Arc<Mutex<Shared>>,
    tx: Sender<Request>,
}

/// The worker that talks to timedated (one for the app), telling the
/// windows through `proxy` when anything changes.
fn link(proxy: olo_kit::Proxy) -> &'static Link {
    static LINK: OnceLock<Link> = OnceLock::new();
    LINK.get_or_init(|| {
        let shared: Arc<Mutex<Shared>> = Default::default();
        // Tests (the layout lint): a zone with a long name, and no
        // timedated.
        if cfg!(test) {
            {
                let mut s = shared.lock().unwrap();
                s.status = Some(Ok(systime::TimeStatus { timezone: "America/Argentina/ComodRivadavia".into(), ntp: true, ntp_synchronized: true, can_ntp: true, local_rtc: false }));
                s.zones = Arc::new(["America/Argentina/ComodRivadavia", "America/Indiana/Petersburg", "Europe/London", "Pacific/Port_Moresby"].map(str::to_owned).to_vec());
            }
            return Link { shared, tx: channel().0 };
        }
        let (tx, rx) = channel::<Request>();
        // timedated's changes (another program's too) read again as
        // they're said; nothing asked otherwise.
        let told = tx.clone();
        systime::watch(move || {
            let _ = told.send(Request::Refresh);
        });
        let sh = shared.clone();
        std::thread::Builder::new()
            .name("timedate".into())
            .spawn(move || {
                let wake = || proxy.send(Msg::Redraw);
                // (What the panel shows first, then the zones to choose
                // from, a longer list.)
                let mut zones = None;
                loop {
                    let status = Some(systime::status().map_err(|e| e.to_string()));
                    let (changed, holds) = {
                        let mut s = sh.lock().unwrap();
                        (std::mem::replace(&mut s.status, status.clone()) != status, std::mem::take(&mut s.holds))
                    };
                    if changed {
                        wake();
                    }
                    drop(holds);
                    if zones.is_none() {
                        zones = Some(match systime::timezones() {
                            Ok(z) => Arc::new(z),
                            Err(e) => {
                                tracing::warn!("timedated: {e}");
                                Arc::default()
                            }
                        });
                        sh.lock().unwrap().zones = zones.clone().unwrap_or_default();
                        wake();
                    }
                    // Network time on and the clock not synchronised yet:
                    // asked again now and then until it is (that isn't
                    // signalled). Else nothing until something changes.
                    let syncing = matches!(&status, Some(Ok(st)) if st.ntp && !st.ntp_synchronized);
                    let req = if syncing {
                        match rx.recv_timeout(SYNC_ASK_EVERY) {
                            Ok(r) => r,
                            Err(RecvTimeoutError::Timeout) => continue,
                            Err(RecvTimeoutError::Disconnected) => break,
                        }
                    } else {
                        match rx.recv() {
                            Ok(r) => r,
                            Err(_) => break,
                        }
                    };
                    if matches!(req, Request::Refresh) {
                        continue;
                    }
                    sh.lock().unwrap().busy = true;
                    wake();
                    let (r, what) = match &req {
                        Request::Time(us) => (systime::set_time(*us), "date and time"),
                        Request::Zone(z) => (systime::set_timezone(z), "time zone"),
                        Request::Ntp(on) => (systime::set_ntp(*on), "network time setting"),
                        // (Read again above, before anything's busy.)
                        Request::Refresh => continue,
                    };
                    let mut s = sh.lock().unwrap();
                    s.busy = false;
                    s.error = r.err().map(|e| {
                        tracing::warn!("timedated: {e}");
                        if systime::needs_auth(&e) { format!("The {what} wasn't changed: an administrator's password is needed.") } else { format!("The {what} couldn't be changed.") }
                    });
                    drop(s);
                    wake();
                }
            })
            .expect("thread");
        Link { shared, tx }
    })
}

pub struct State {
    tab: usize,
    /// The first of the month the calendar shows; None follows the date
    /// being shown.
    month: Option<Date>,
    /// A date and time picked or typed but not yet set.
    edit: Option<(Date, String)>,
    /// The date shown when nothing is picked (today, where the clock is).
    today: Option<Date>,
    time: Child<TextField>,
    search: Child<TextField>,
    zones: ListModel,
    /// The zone list's rows (indices into the shared list) for a search.
    filtered: Option<(String, Arc<Vec<String>>, Arc<Vec<usize>>)>,
    /// The current zone was selected when the list first showed.
    zone_selected: bool,
    /// A frame is due at the next second (the clock ticks).
    ticking: bool,
    link: &'static Link,
}

impl State {
    pub fn new(cx: &mut Cx<PanelView>) -> State {
        let time = cx.new_view(|_| TextField::new());
        let search = olo_kit::widgets::text_field(cx, |f| f.hint("City or region"));
        // Typing a time picks it, on the day shown.
        cx.subscribe(&time, |v, e: &TextFieldEvent, cx| {
            if !matches!(e, TextFieldEvent::Changed) {
                return;
            }
            if let Some(st) = state_of(v) {
                let text = st.time.read(cx).text().to_owned();
                let date = st.edit.as_ref().map(|(d, _)| *d).or(st.today);
                if let Some(date) = date {
                    st.edit = Some((date, text));
                }
            }
            cx.notify();
        });
        cx.subscribe(&search, |_, e: &TextFieldEvent, cx| {
            if matches!(e, TextFieldEvent::Changed) {
                cx.notify();
            }
        });
        let zones = ListModel::new(false);
        State { tab: 0, month: None, edit: None, today: None, time, search, zones, filtered: None, zone_selected: false, ticking: false, link: link(olo_kit::proxy()) }
    }
}

fn state_of(v: &mut PanelView) -> Option<&mut State> {
    match &mut v.state {
        panel::State::DateTime(st) => Some(st),
        _ => None,
    }
}

pub fn ui(ui: &mut Ui, st: &mut State) {
    let (status, zones, busy, error) = {
        let mut sh = st.link.shared.lock().unwrap();
        // (Not shown until the status is in, briefly: it's most of what
        // the panel shows.)
        if sh.status.is_none()
            && let Some(hold) = olo_kit::hold_first_show(ui.cx)
        {
            sh.holds.push(hold);
        }
        (sh.status.clone(), sh.zones.clone(), sh.busy, sh.error.clone())
    };
    // Tick with the clock: drawn again at the next second.
    if !st.ticking {
        st.ticking = true;
        let ns = Timestamp::now().subsec_nanosecond();
        let wait = Duration::from_nanos((1_000_000_000 - ns.max(0)) as u64 + 5_000_000);
        ui.cx.after_do(wait, |v, cx| {
            if let Some(st) = state_of(v) {
                st.ticking = false;
            }
            cx.notify();
        });
    }

    let status = match status {
        Some(Ok(status)) => Some(status),
        Some(Err(_)) => None,
        None => {
            ui.note("Reading the clock…");
            return;
        }
    };
    let zone = status.as_ref().map(|s| s.timezone.clone()).unwrap_or_default();
    let tz = TimeZone::get(&zone).unwrap_or_else(|_| TimeZone::system());
    let now = Timestamp::now().to_zoned(tz.clone());
    st.today = Some(now.date());
    let clock_settings = ui.s.clock.clone();

    let items = ui.collect(|ui| {
        match st.tab {
            0 => date_time(ui, &clock_settings, st, status.as_ref(), &now, busy),
            1 => time_zone(ui, &clock_settings, st, &zone, &zones, busy),
            _ => clock(ui, &clock_settings, &now),
        }
        if status.is_none() {
            ui.note("The clock service (systemd-timedated) isn't answering.");
        } else if busy {
            ui.note("Changing…");
        } else if let Some(e) = &error {
            ui.label(e.clone());
        }
    });
    let pick = ui.on_view(|v, i: &usize, cx| {
        if let Some(st) = state_of(v) {
            st.tab = *i;
        }
        cx.notify();
    });
    let tabs: Vec<SharedString> = ["Date & Time", "Time Zone", "Clock"].into_iter().map(SharedString::from).collect();
    ui.add(
        div()
            .flex_1()
            .flex()
            .flex_col()
            .child(TabView::new("datetime-tabs", tabs, st.tab).grows().on_select(pick).child(div().flex_1().flex().flex_col().gap(px(m::SPACING)).p(px(m::SPACING)).children(items))),
    );
}

// ---- Date & Time ---------------------------------------------------------------

fn date_time(ui: &mut Ui, clock: &ClockSettings, st: &mut State, status: Option<&TimeStatus>, now: &Zoned, busy: bool) {
    let ntp = status.is_some_and(|s| s.ntp);
    let manual = status.is_some() && !ntp && !busy;
    if !manual {
        st.edit = None;
    }
    let date = st.edit.as_ref().map_or(now.date(), |(d, _)| *d);
    let shown_time = match &st.edit {
        Some((_, text)) => parse_time(text),
        None => Some(now.time()),
    };
    // The field shows the time going by until it's typed in.
    let focused = st.time.is_focused(ui.cx);
    if st.edit.is_none() && !focused {
        let text = time_text(now.time(), clock);
        st.time.update(ui.cx, |f, cx| {
            if f.text() != text {
                f.set_text(text, cx);
            }
        });
    }
    st.time.update(ui.cx, |f, cx| f.set_enabled(manual, cx));

    let month = st.month.unwrap_or_else(|| date.first_of_month());
    let clock_text = time_text(now.time(), clock);
    let pick_day: std::rc::Rc<dyn Fn(&Date, &mut WindowCx)> = std::rc::Rc::new(ui.on_view(move |v, day: &Date, cx| {
        if let Some(st) = state_of(v) {
            let text = st.edit.take().map_or_else(|| clock_text.clone(), |(_, t)| t);
            st.edit = Some((*day, text));
        }
        cx.notify();
    }));
    let step: std::rc::Rc<dyn Fn(&i8, &mut WindowCx)> = std::rc::Rc::new(ui.on_view(move |v, dir: &i8, cx| {
        if let Some(st) = state_of(v) {
            st.month = month.checked_add(jiff::Span::new().months(*dir)).ok();
        }
        cx.notify();
    }));
    let today = now.date();
    let found = CalendarPlaces::default();
    let places = found.clone();
    let cal = canvas(move |g, b| calendar(g, b, month, date, today, manual, &places)).w(px(CELL_W * 7.0 + 4.0)).h(px(CALENDAR_H)).flex_shrink_0().on_mouse_down(
        MouseButton::Left,
        move |e: &MouseDownEvent, cx| match calendar_press(&found, e.position) {
            Some(Ok(dir)) => step(&dir, cx),
            Some(Err(day)) => pick_day(&day, cx),
            None => {}
        },
    );
    let live = !manual;
    let face = canvas(move |g, b| paint_face(g, b, &theme::palette(), shown_time, live)).w(px(FACE)).h(px(FACE));
    let field = TextControl::new(ui.cx, &st.time).label("Time:").width(FACE);
    ui.add(div().flex().flex_row().gap(px(m::SPACING * 3.0)).child(cal).child(div().flex().flex_col().child(face).child(div().h(px(CALENDAR_H - FACE - m::CONTROL_HEIGHT))).child(field)));

    let can_ntp = status.is_some_and(|s| s.can_ntp);
    let tx = st.link.tx.clone();
    ui.enabled_if(can_ntp && !busy, |ui| {
        let tx = tx.clone();
        ui.add(olo_kit::widgets::CheckBox::new("Set date and time automatically", ntp).enabled(ui.enabled()).on_toggle(move |on, _| {
            let _ = tx.send(Request::Ntp(*on));
        }));
    });
    let note = match status {
        Some(s) if s.ntp && s.ntp_synchronized => "Kept in step with network time servers.",
        Some(s) if s.ntp => "Waiting for a network time server…",
        Some(s) if !s.can_ntp => "No network time service is installed.",
        _ => "Pick a day and type the time, then Set.",
    };
    ui.note(note);

    if let Some((day, text)) = st.edit.clone() {
        let when = parse_time(&text).and_then(|t| day.to_datetime(t).to_zoned(now.time_zone().clone()).ok());
        let at = when.as_ref().map(|w| w.timestamp().as_microsecond());
        let set = ui.on_click_view(move |v, cx| {
            if let (Some(st), Some(at)) = (state_of(v), at) {
                let _ = st.link.tx.send(Request::Time(at));
                st.edit = None;
                st.month = None;
            }
            cx.notify();
        });
        let cancel = ui.on_click_view(|v, cx| {
            if let Some(st) = state_of(v) {
                st.edit = None;
                st.month = None;
            }
            cx.notify();
        });
        let mut row =
            div().flex().flex_row().items_center().gap(px(m::SPACING)).child(Button::new("Set").default(true).enabled(when.is_some()).on_click(set)).child(Button::new("Cancel").on_click(cancel));
        if when.is_none() {
            row = row.child(StringView::new("Type a time like 13:05 or 1:05 PM.").dim(true));
        }
        ui.add(row);
    }
}

/// A calendar day.
const CELL_W: f32 = 28.0;
const CELL_H: f32 = 20.0;
const FACE: f32 = 136.0;
/// The month over the weekdays over six weeks.
const CAL_HEADER: f32 = 22.0;
const CALENDAR_H: f32 = CAL_HEADER + 18.0 + CELL_H * 6.0 + 6.0;

/// R5's month calendar: the month and year with arrows to step through
/// them over a grid of days; a click on a day picks it.
#[allow(clippy::too_many_arguments)]
fn calendar(g: &mut Painter, rect: Bounds<Pixels>, month: Date, date: Date, today: Date, enabled: bool, found: &CalendarPlaces) {
    let p = theme::palette();
    let ink = if enabled { c(p.panel_text) } else { style::disabled_text(&p) };
    let bold = style::ui_font(FontWeight::BOLD);
    let plain = style::ui_font(FontWeight::NORMAL);
    let head = Bounds { origin: rect.origin, size: size(rect.size.width, px(CAL_HEADER)) };
    let title = format!("{} {}", MONTHS[month.month() as usize - 1], month.year());
    style::label(g, head.center().x, head.center().y, style::Align::Center, title, style::PLAIN_SIZE, bold.clone(), ink);
    let arrows = [(point(head.left() + px(10.0), head.center().y), -1i8), (point(head.right() - px(10.0), head.center().y), 1)];
    for (centre, dir) in arrows {
        let d = dir as f32 * 3.5;
        let mut tri = PathBuilder::fill();
        tri.move_to(point(centre.x - px(d), centre.y - px(5.0)));
        tri.line_to(point(centre.x - px(d), centre.y + px(5.0)));
        tri.line_to(point(centre.x + px(d), centre.y));
        tri.close();
        if let Ok(path) = tri.build() {
            g.paint_path(path, ink);
        }
    }

    let well = Bounds::from_corners(point(rect.left(), head.bottom()), rect.bottom_right());
    style::fill_rect(g, style::inset(g, well, 2.0), if enabled { c(p.document_background) } else { c(p.panel_background) });
    style::sunken(g, well, &p, false);
    let text = if enabled { c(p.document_text) } else { style::disabled_text(&p) };
    let grid = style::inset(g, well, 2.0);
    for (i, d) in ["S", "M", "T", "W", "T", "F", "S"].iter().enumerate() {
        let x = grid.left() + px(CELL_W * (i as f32 + 0.5));
        style::label(g, x, grid.top() + px(9.0), style::Align::Center, *d, style::SMALL_SIZE, bold.clone(), text);
    }
    style::hline(g, grid.left() + px(4.0), grid.right() - px(4.0), grid.top() + px(18.0), style::shade(&p, p.document_background, 1.15));
    let lead = month.weekday().to_sunday_zero_offset() as i16;
    let mut cells = Vec::new();
    for day in 1..=month.days_in_month() {
        let i = lead + day as i16 - 1;
        let cell = Bounds { origin: point(grid.left() + px(CELL_W * (i % 7) as f32), grid.top() + px(18.0 + 2.0 + CELL_H * (i / 7) as f32)), size: size(px(CELL_W), px(CELL_H)) };
        let this = month.with().day(day).build().unwrap_or(month);
        let mut color = text;
        if this == date {
            g.paint_quad(olo_kit::fill(style::inset(g, cell, 1.0), c(p.list_selected_background)).corner_radii(px(2.0)));
            color = if enabled { c(p.list_selected_item_text) } else { text };
        }
        let font = if this == today { bold.clone() } else { plain.clone() };
        style::label(g, cell.center().x, cell.center().y, style::Align::Center, day.to_string(), style::PLAIN_SIZE, font, color);
        cells.push((cell, this));
    }
    olo_kit::agent::record(
        g,
        olo_kit::agent::Node {
            role: "Calendar",
            name: Some(format!("{} {}", MONTHS[month.month() as usize - 1], month.year()).into()),
            value: Some(date.to_string().into()),
            bounds: rect,
            enabled,
            clickable: enabled,
            ..Default::default()
        },
    );
    // (Where a press finds its arrows and days: `calendar_press`.)
    *found.borrow_mut() = if enabled { (arrows.to_vec(), cells) } else { Default::default() };
}

/// Where the calendar last drew its arrows (each with the way it steps)
/// and its days: what a press on it goes by.
type CalendarPlaces = std::rc::Rc<std::cell::RefCell<(Vec<(Point<Pixels>, i8)>, Vec<(Bounds<Pixels>, Date)>)>>;

/// A press on the calendar: an arrow steps the month, a day is picked.
fn calendar_press(found: &CalendarPlaces, at: Point<Pixels>) -> Option<Result<i8, Date>> {
    let found = found.borrow();
    let near = |c: Point<Pixels>| (f32::from(at.x - c.x)).abs() <= 9.0 && (f32::from(at.y - c.y)).abs() <= 9.0;
    if let Some((_, dir)) = found.0.iter().find(|(c, _)| near(*c)) { Some(Ok(*dir)) } else { found.1.iter().find(|(r, _)| r.contains(&at)).map(|(_, day)| Err(*day)) }
}

/// A straight line from `a` to `b`, `w` wide.
fn line(g: &mut Painter, a: Point<Pixels>, b: Point<Pixels>, w: Pixels, color: Hsla) {
    let mut path = PathBuilder::stroke(w);
    path.move_to(a);
    path.line_to(b);
    if let Ok(path) = path.build() {
        g.paint_path(path, color);
    }
}

/// The clock face: hour and minute hands, and a red second hand. `None`
/// (a time being typed that doesn't read yet) shows no hands.
fn paint_face(g: &mut Painter, b: Bounds<Pixels>, p: &olo_kit::palette::Palette, time: Option<Time>, live: bool) {
    let centre = b.center();
    let r = FACE / 2.0 - 2.0;
    let disc = |g: &mut Painter, at: Point<Pixels>, r: f32, fill: Hsla| g.paint_quad(olo_kit::fill(Bounds::centered_at(at, size(px(2.0 * r), px(2.0 * r))), fill).corner_radii(px(r)));
    let ring = |g: &mut Painter, r: f32, color: Hsla| {
        g.paint_quad(olo_kit::outline(Bounds::centered_at(centre, size(px(2.0 * r), px(2.0 * r))), color, olo_kit::BorderStyle::Solid).corner_radii(px(r)));
    };
    disc(g, point(centre.x + px(1.0), centre.y + px(1.5)), r, c(p.shadow).opacity(0.35));
    disc(g, centre, r, c(p.document_background));
    ring(g, r, style::shade(p, p.panel_background, 1.6));
    ring(g, r - 2.0, c(p.shine).opacity(0.5));
    let ink = c(p.document_text);
    let at = |dir: (f32, f32), d: f32| point(centre.x + px(dir.0 * d), centre.y + px(dir.1 * d));
    for i in 0..60 {
        let a = i as f32 / 60.0 * std::f32::consts::TAU;
        let dir = (a.sin(), -a.cos());
        let (inner, w) = if i % 5 == 0 { (r - 10.0, 2.0) } else { (r - 5.0, 1.0) };
        line(g, at(dir, inner), at(dir, r - 3.0), px(w), ink);
    }
    let Some(t) = time else { return };
    let (h, mi, s) = (t.hour() as f32, t.minute() as f32, t.second() as f32);
    let hand = |g: &mut Painter, turns: f32, len: f32, w: Pixels, color: Hsla| {
        let a = turns * std::f32::consts::TAU;
        let dir = (a.sin(), -a.cos());
        line(g, at(dir, -len * 0.15), at(dir, len), w, color);
    };
    hand(g, ((h % 12.0) + mi / 60.0) / 12.0, r * 0.5, px(3.5), ink);
    hand(g, (mi + s / 60.0) / 60.0, r * 0.75, px(2.5), ink);
    if live || s != 0.0 {
        // Whole device pixels, so it is crisp when it points straight up
        // or down.
        let hair = style::hairline(g);
        hand(g, s / 60.0, r * 0.82, hair, c(p.failure));
    }
    disc(g, centre, 3.0, ink);
}

const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

fn time_text(t: Time, clock: &ClockSettings) -> String {
    let c = ClockSettings { show_seconds: true, show_day: false, ..clock.clone() };
    c.format(t.hour() as u32, t.minute() as u32, t.second() as u32, 0)
}

/// "13:05", "13:05:09", "1:05 PM", "1:05:09pm", "12 am".
fn parse_time(s: &str) -> Option<Time> {
    let s = s.trim().to_ascii_lowercase();
    let (digits, half) = if let Some(d) = s.strip_suffix("am").or_else(|| s.strip_suffix("a.m.")) {
        (d.trim(), Some(false))
    } else if let Some(d) = s.strip_suffix("pm").or_else(|| s.strip_suffix("p.m.")) {
        (d.trim(), Some(true))
    } else {
        (s.as_str(), None)
    };
    let parts: Vec<&str> = digits.split([':', '.']).collect();
    if parts.is_empty() || parts.len() > 3 || parts.iter().any(|p| p.is_empty() || p.len() > 2) {
        return None;
    }
    let nums: Vec<i8> = parts.iter().map(|p| p.parse().ok()).collect::<Option<_>>()?;
    let mut hour = nums[0];
    match half {
        Some(pm) => {
            if !(1..=12).contains(&hour) {
                return None;
            }
            hour = hour % 12 + if pm { 12 } else { 0 };
        }
        None if parts.len() < 2 => return None,
        None => {}
    }
    Time::new(hour, *nums.get(1).unwrap_or(&0), *nums.get(2).unwrap_or(&0), 0).ok()
}

// ---- Time Zone -----------------------------------------------------------------

fn zone_label(z: &str) -> String {
    z.replace('_', " ").replace('/', " / ")
}

fn offset_text(tz: &TimeZone, at: Timestamp) -> String {
    let secs = tz.to_offset(at).seconds();
    let sign = if secs < 0 { '−' } else { '+' };
    let a = secs.unsigned_abs();
    format!("UTC{sign}{:02}:{:02}", a / 3600, a % 3600 / 60)
}

fn time_zone(ui: &mut Ui, clock: &ClockSettings, st: &mut State, current: &str, zones: &Arc<Vec<String>>, busy: bool) {
    ui.add(TextControl::new(ui.cx, &st.search).label("Search:"));
    let search = st.search.read(ui.cx).text().to_owned();
    let stale = st.filtered.as_ref().is_none_or(|(q, z, _)| *q != search || !Arc::ptr_eq(z, zones));
    if stale {
        let q = search.to_lowercase();
        let selected = st.filtered.as_ref().and_then(|(_, z, rows)| st.zones.state().single().and_then(|i| rows.get(i)).map(|&r| z[r].clone()));
        let rows: Vec<usize> = (0..zones.len()).filter(|&i| zone_label(&zones[i]).to_lowercase().contains(&q) || zones[i].to_lowercase().contains(&q)).collect();
        let keep = selected.or_else(|| (!st.zone_selected).then(|| current.to_owned()));
        let at = keep.and_then(|k| rows.iter().position(|&r| zones[r] == k));
        {
            let mut l = st.zones.state_mut();
            l.selected.clear();
            if let Some(i) = at {
                l.select(i);
            }
        }
        if let Some(i) = at {
            st.zones.reveal(ui.cx, i);
        }
        if at.is_some() {
            st.zone_selected = true;
        }
        st.filtered = Some((search.clone(), zones.clone(), Arc::new(rows)));
    }
    let Some((_, _, rows)) = &st.filtered else { return };
    let rows = rows.clone();
    // (All three in the list's width, no scrolling sideways: a long
    // zone's name is cut short.)
    let columns = vec![Column::new("Time zone", 186.0), Column::new("Offset", 84.0), Column::new("Time", 84.0).right()];
    let now = Timestamp::now();
    let (zones2, rows2, clock2) = (zones.clone(), rows.clone(), clock.clone());
    let list = ColumnListView::new(ui.cx, "zones", &st.zones, columns, rows.len(), move |_: &PanelView, cell, g| {
        let name = &zones2[rows2[cell.row]];
        let tz = TimeZone::get(name);
        match (cell.column, tz) {
            (0, _) => cell.text(g, zone_label(name)),
            (1, Ok(tz)) => cell.text(g, offset_text(&tz, now)),
            (2, Ok(tz)) => {
                let t = now.to_zoned(tz);
                let c = ClockSettings { show_seconds: false, ..clock2.clone() };
                cell.text(g, c.format(t.hour() as u32, t.minute() as u32, 0, t.weekday().to_sunday_zero_offset() as u32));
            }
            _ => {}
        }
    })
    .on_select(ui.cx.listener(|_: &mut PanelView, _: &(), cx| cx.notify()));
    ui.add(div().flex_1().min_h(px(100.0)).child(list));
    let chosen = st.zones.state().single().and_then(|i| rows.get(i)).map(|&r| zones[r].clone());
    let can = !busy && chosen.as_deref().is_some_and(|z| z != current);
    let tx = st.link.tx.clone();
    ui.add(
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(m::SPACING))
            .child(Button::new("Set Time Zone").enabled(can).on_click(move |_, _| {
                if let Some(z) = &chosen {
                    let _ = tx.send(Request::Zone(z.clone()));
                }
            }))
            .child(StringView::new(format!("Now: {}", zone_label(current)))),
    );
}

// ---- Clock ---------------------------------------------------------------------

fn clock(ui: &mut Ui, clock: &ClockSettings, now: &Zoned) {
    ui.bbox("Deskbar clock", |ui| {
        ui.check("24-hour clock", clock.twenty_four_hour, |s, v| s.clock.twenty_four_hour = v);
        ui.check("Show seconds", clock.show_seconds, |s, v| s.clock.show_seconds = v);
        ui.check("Show the day of the week", clock.show_day, |s, v| s.clock.show_day = v);
    });
    let sample = clock.format(now.hour() as u32, now.minute() as u32, now.second() as u32, now.weekday().to_sunday_zero_offset() as u32);
    ui.label(format!("Deskbar shows: {sample}"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_times() {
        let t = |h, m, s| Some(Time::new(h, m, s, 0).unwrap());
        assert_eq!(parse_time("13:05"), t(13, 5, 0));
        assert_eq!(parse_time(" 13:05:09 "), t(13, 5, 9));
        assert_eq!(parse_time("1:05 PM"), t(13, 5, 0));
        assert_eq!(parse_time("1:05:09pm"), t(13, 5, 9));
        assert_eq!(parse_time("12 am"), t(0, 0, 0));
        assert_eq!(parse_time("12:30 PM"), t(12, 30, 0));
        assert_eq!(parse_time("13"), None);
        assert_eq!(parse_time("13 pm"), None);
        assert_eq!(parse_time("25:00"), None);
        assert_eq!(parse_time("1:5:"), None);
    }

    #[test]
    fn labels() {
        assert_eq!(zone_label("America/Los_Angeles"), "America / Los Angeles");
        let tz = TimeZone::fixed(jiff::tz::offset(-7));
        assert_eq!(offset_text(&tz, Timestamp::UNIX_EPOCH), "UTC−07:00");
        assert_eq!(time_text(Time::new(13, 5, 9, 0).unwrap(), &ClockSettings::default()), "1:05:09 PM");
    }
}
