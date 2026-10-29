//! Date & Time: R5's Time preferences. A calendar and a clock face to set
//! the clock by hand, network time, the time zone, and how Deskbar shows
//! the time. The clock and zone belong to systemd-timedated
//! (`olo_system::time`): one worker thread reads them every couple of
//! seconds and makes the changes, which polkit may stop to ask for a
//! password, so the window never waits on it.

use std::{
    sync::{
        Arc, Mutex, OnceLock,
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
    egui::{Align, Color32, FontId, Context, Rect, Sense, Shape, Stroke, Ui, UiBuilder, pos2, vec2},
    metrics as m,
    style::{self, c},
    widgets::{BBox, Button, CheckBox, Column, ColumnListView, ListState, StringView, TabView, TextControl},
};
use olo_protocol::{ClockSettings, Settings};
use olo_system::time::{self as systime, TimeStatus};

enum Request {
    Time(i64),
    Zone(String),
    Ntp(bool),
}

#[derive(Default)]
struct Shared {
    /// None until first read; Err while timedated can't be reached.
    status: Option<Result<TimeStatus, String>>,
    zones: Arc<Vec<String>>,
    /// A change is on its way (polkit may be asking for a password).
    busy: bool,
    error: Option<String>,
}

struct Link {
    shared: Arc<Mutex<Shared>>,
    tx: Sender<Request>,
    /// Every context that shows the panel, to wake on a change.
    contexts: Arc<Mutex<Vec<Context>>>,
}

/// What tests show instead of timedated's (for the layout lint): the
/// longest zone name.
#[cfg(test)]
fn sample(sh: &mut Shared) {
    let zone = "America/Argentina/ComodRivadavia";
    sh.status = Some(Ok(TimeStatus { timezone: zone.into(), ntp: true, ntp_synchronized: true, can_ntp: true, local_rtc: false }));
    sh.zones = Arc::new(vec![zone.into(), "Europe/London".into(), "UTC".into()]);
}

fn link() -> &'static Link {
    static LINK: OnceLock<Link> = OnceLock::new();
    LINK.get_or_init(|| {
        let shared: Arc<Mutex<Shared>> = Default::default();
        #[cfg(test)]
        sample(&mut shared.lock().unwrap());
        let contexts: Arc<Mutex<Vec<Context>>> = Default::default();
        let (tx, rx) = channel::<Request>();
        let (sh, ctxs) = (shared.clone(), contexts.clone());
        std::thread::Builder::new()
            .name("timedate".into())
            .spawn(move || {
                if cfg!(test) {
                    return;
                }
                let wake = || ctxs.lock().unwrap().iter().for_each(|c| c.request_repaint());
                match systime::timezones() {
                    Ok(z) => sh.lock().unwrap().zones = Arc::new(z),
                    Err(e) => tracing::warn!("timedated: {e}"),
                }
                loop {
                    let status = Some(systime::status().map_err(|e| e.to_string()));
                    let changed = {
                        let mut s = sh.lock().unwrap();
                        std::mem::replace(&mut s.status, status.clone()) != status
                    };
                    if changed {
                        wake();
                    }
                    let req = match rx.recv_timeout(Duration::from_secs(2)) {
                        Ok(r) => r,
                        Err(RecvTimeoutError::Timeout) => continue,
                        Err(RecvTimeoutError::Disconnected) => break,
                    };
                    sh.lock().unwrap().busy = true;
                    wake();
                    let (r, what) = match &req {
                        Request::Time(us) => (systime::set_time(*us), "date and time"),
                        Request::Zone(z) => (systime::set_timezone(z), "time zone"),
                        Request::Ntp(on) => (systime::set_ntp(*on), "network time setting"),
                    };
                    let mut s = sh.lock().unwrap();
                    s.busy = false;
                    s.error = r.err().map(|e| {
                        tracing::warn!("timedated: {e}");
                        if systime::needs_auth(&e) {
                            format!("The {what} wasn't changed: an administrator's password is needed.")
                        } else {
                            format!("The {what} couldn't be changed.")
                        }
                    });
                    drop(s);
                    wake();
                }
            })
            .expect("thread");
        Link { shared, tx, contexts }
    })
}

#[derive(Default)]
pub struct State {
    tab: usize,
    /// The first of the month the calendar shows; None follows the date
    /// being shown.
    month: Option<Date>,
    /// A date and time picked or typed but not yet set.
    edit: Option<(Date, String)>,
    search: String,
    zones: ListState,
    /// The zone list's rows (indices into the shared list) for a search.
    filtered: Option<(String, Arc<Vec<String>>, Vec<usize>)>,
    /// The current zone was selected when the list first showed.
    zone_selected: bool,
    /// A row selected in code, to scroll into view.
    reveal: Option<usize>,
}

#[cfg(test)]
impl State {
    /// On `tab` (for the layout lint).
    pub fn showing(tab: usize) -> State {
        State { tab, ..Default::default() }
    }
}

pub fn ui(ui: &mut Ui, s: &mut Settings, st: &mut State) {
    let link = link();
    {
        let mut ctxs = link.contexts.lock().unwrap();
        if !ctxs.iter().any(|c| c == ui.ctx()) {
            ctxs.push(ui.ctx().clone());
        }
    }
    let (status, zones, busy, error) = {
        let sh = link.shared.lock().unwrap();
        (sh.status.clone(), sh.zones.clone(), sh.busy, sh.error.clone())
    };
    // Tick with the clock.
    let ns = Timestamp::now().subsec_nanosecond();
    ui.ctx().request_repaint_after(Duration::from_nanos((1_000_000_000 - ns.max(0)) as u64 + 5_000_000));

    let status = match status {
        Some(Ok(status)) => Some(status),
        Some(Err(_)) => None,
        None => {
            ui.add(StringView::new("Reading the clock…").dim(true));
            return;
        }
    };
    let zone = status.as_ref().map(|s| s.timezone.clone()).unwrap_or_default();
    let tz = TimeZone::get(&zone).unwrap_or_else(|_| TimeZone::system());
    let now = Timestamp::now().to_zoned(tz.clone());

    let mut tab = st.tab;
    TabView::new(&mut tab, &["Date & Time", "Time Zone", "Clock"]).show(ui, |ui, tab| {
        ui.spacing_mut().item_spacing.y = m::SPACING;
        match tab {
            0 => date_time(ui, &s.clock, st, status.as_ref(), &now, busy),
            1 => time_zone(ui, &s.clock, st, &zone, &zones, busy),
            _ => clock(ui, &mut s.clock, &now),
        }
        if status.is_none() {
            ui.add(StringView::new("The clock service (systemd-timedated) isn't answering.").dim(true));
        } else if busy {
            ui.add(StringView::new("Changing…").dim(true));
        } else if let Some(e) = &error {
            ui.add(StringView::new(e));
        }
    });
    st.tab = tab;
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

    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = m::SPACING * 3.0;
        ui.add_enabled_ui(manual, |ui| {
            if let Some(day) = calendar(ui, st, date, now.date()) {
                let text = st.edit.take().map_or_else(|| time_text(now.time(), clock), |(_, t)| t);
                st.edit = Some((day, text));
            }
        });
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = m::SPACING;
            face(ui, shown_time, !manual);
            // The field at the foot of the calendar, level with its sixth
            // week: the two columns end together.
            ui.add_space(CALENDAR_H - FACE - m::SPACING - m::CONTROL_HEIGHT);
            ui.add_enabled_ui(manual, |ui| {
                let mut text = match &st.edit {
                    Some((_, t)) => t.clone(),
                    None => time_text(now.time(), clock),
                };
                let r = ui.add(TextControl::new(&mut text).label("Time:").width(FACE));
                if r.changed() {
                    st.edit = Some((date, text));
                }
            });
        });
    });

    let mut on = ntp;
    let can_ntp = status.is_some_and(|s| s.can_ntp);
    ui.add_enabled_ui(can_ntp && !busy, |ui| {
        ui.add(CheckBox::new(&mut on, "Set date and time automatically"));
    });
    if on != ntp {
        let _ = link().tx.send(Request::Ntp(on));
    }
    let note = match status {
        Some(s) if s.ntp && s.ntp_synchronized => "Kept in step with network time servers.",
        Some(s) if s.ntp => "Waiting for a network time server…",
        Some(s) if !s.can_ntp => "No network time service is installed.",
        _ => "Pick a day and type the time, then Set.",
    };
    ui.add(StringView::new(note).dim(true));

    if let Some((day, text)) = st.edit.clone() {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = m::SPACING;
            let when = parse_time(&text).and_then(|t| day.to_datetime(t).to_zoned(now.time_zone().clone()).ok());
            let set = ui.add_enabled_ui(when.is_some(), |ui| ui.add(Button::new("Set").default(true))).inner;
            if set.clicked()
                && let Some(when) = &when
            {
                let _ = link().tx.send(Request::Time(when.timestamp().as_microsecond()));
                st.edit = None;
                st.month = None;
            }
            if ui.add(Button::new("Cancel")).clicked() {
                st.edit = None;
                st.month = None;
            }
            if when.is_none() {
                ui.add(StringView::new("Type a time like 13:05 or 1:05 PM.").dim(true));
            }
        });
    }
}

/// A calendar day.
const CELL_W: f32 = 28.0;
const CELL_H: f32 = 20.0;
const FACE: f32 = 120.0;
/// The month over the weekdays over six weeks.
const CAL_HEADER: f32 = 22.0;
const CALENDAR_H: f32 = CAL_HEADER + 18.0 + CELL_H * 6.0 + 6.0;

/// R5's month calendar: the month and year with arrows to step through
/// them over a grid of days. Returns the day clicked.
fn calendar(ui: &mut Ui, st: &mut State, date: Date, today: Date) -> Option<Date> {
    let p = style::palette(ui.ctx());
    let enabled = ui.is_enabled();
    let ink = if enabled { c(p.panel_text) } else { style::disabled_text(&p) };
    let month = st.month.unwrap_or_else(|| date.first_of_month());
    let width = CELL_W * 7.0 + 4.0;
    let header = CAL_HEADER;
    let (rect, _) = ui.allocate_exact_size(vec2(width, CALENDAR_H), Sense::hover());
    let painter = ui.painter();
    let head = Rect::from_min_size(rect.min, vec2(width, header));
    let title = format!("{} {}", MONTHS[month.month() as usize - 1], month.year());
    style::label(painter, head.center().x, head.center().y, Align::Center, title, style::bold_font(), ink);
    let mut step = |r: Rect, dir: i8, id: &str| {
        let resp = ui.interact(r, ui.scope_id().with(("cal", id)), Sense::click());
        let (cx, cy) = (r.center().x, r.center().y);
        let d = dir as f32 * 3.5;
        let tri = vec![pos2(cx - d, cy - 5.0), pos2(cx - d, cy + 5.0), pos2(cx + d, cy)];
        let fill = if resp.is_pointer_button_down_on() { c(p.control_mark) } else { ink };
        ui.painter().add(Shape::convex_polygon(tri, fill, Stroke::NONE));
        if resp.clicked() {
            st.month = month.checked_add(jiff::Span::new().months(dir)).ok();
        }
    };
    step(Rect::from_center_size(pos2(head.left() + 10.0, head.center().y), vec2(18.0, 18.0)), -1, "prev");
    step(Rect::from_center_size(pos2(head.right() - 10.0, head.center().y), vec2(18.0, 18.0)), 1, "next");

    let well = Rect::from_min_max(pos2(rect.left(), head.bottom()), rect.max);
    let painter = ui.painter();
    painter.rect_filled(style::inset(ui.ctx(), well, 2.0), 0.0, if enabled { c(p.document_background) } else { c(p.panel_background) });
    style::sunken(painter, well, &p, false);
    let text = if enabled { c(p.document_text) } else { style::disabled_text(&p) };
    let grid = well.shrink(2.0);
    for (i, d) in ["S", "M", "T", "W", "T", "F", "S"].iter().enumerate() {
        let x = grid.left() + CELL_W * (i as f32 + 0.5);
        style::label(painter, x, grid.top() + 9.0, Align::Center, *d, FontId::new(style::SMALL_SIZE, style::bold()), text);
    }
    style::hline(painter, grid.left() + 4.0, grid.right() - 4.0, grid.top() + 18.0, style::shade(&p, p.document_background, 1.15));
    let lead = month.weekday().to_sunday_zero_offset() as i16;
    let mut clicked = None;
    for day in 1..=month.days_in_month() {
        let i = lead + day as i16 - 1;
        let cell = Rect::from_min_size(
            pos2(grid.left() + CELL_W * (i % 7) as f32, grid.top() + 18.0 + 2.0 + CELL_H * (i / 7) as f32),
            vec2(CELL_W, CELL_H),
        );
        let this = month.with().day(day).build().unwrap_or(month);
        let selected = this == date;
        let resp = ui.interact(cell, ui.scope_id().with(("day", day)), Sense::click());
        let painter = ui.painter();
        let mut color = text;
        if selected {
            painter.rect_filled(style::inset(ui.ctx(), cell, 1.0), 2.0, c(p.list_selected_background));
            color = if enabled { c(p.list_selected_item_text) } else { text };
        }
        let font = if this == today { style::bold_font() } else { style::plain_font() };
        style::label(painter, cell.center().x, cell.center().y, Align::Center, day.to_string(), font, color);
        if resp.clicked() {
            clicked = Some(this);
        }
    }
    clicked
}

/// The clock face: hour and minute hands, and a red second hand. `None`
/// (a time being typed that doesn't read yet) shows no hands.
fn face(ui: &mut Ui, time: Option<Time>, live: bool) {
    let p = style::palette(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(vec2(FACE, FACE), Sense::hover());
    let painter = ui.painter();
    let centre = rect.center();
    let r = FACE / 2.0 - 2.0;
    painter.circle_filled(centre + vec2(1.0, 1.5), r, c(p.shadow).gamma_multiply(0.35));
    painter.circle_filled(centre, r, c(p.document_background));
    painter.circle_stroke(centre, r, Stroke::new(1.0, style::shade(&p, p.panel_background, 1.6)));
    painter.circle_stroke(centre, r - 2.0, Stroke::new(1.0, c(p.shine).gamma_multiply(0.5)));
    let ink = c(p.document_text);
    for i in 0..60 {
        let a = i as f32 / 60.0 * std::f32::consts::TAU;
        let dir = vec2(a.sin(), -a.cos());
        let (inner, w) = if i % 5 == 0 { (r - 10.0, 2.0) } else { (r - 5.0, 1.0) };
        painter.line_segment([centre + dir * inner, centre + dir * (r - 3.0)], Stroke::new(w, ink));
    }
    let Some(t) = time else { return };
    let (h, mi, s) = (t.hour() as f32, t.minute() as f32, t.second() as f32);
    let hand = |turns: f32, len: f32, w: f32, color: Color32| {
        let a = turns * std::f32::consts::TAU;
        let dir = vec2(a.sin(), -a.cos());
        painter.line_segment([centre - dir * (len * 0.15), centre + dir * len], Stroke::new(w, color));
    };
    hand(((h % 12.0) + mi / 60.0) / 12.0, r * 0.5, 3.5, ink);
    hand((mi + s / 60.0) / 60.0, r * 0.75, 2.5, ink);
    if live || s != 0.0 {
        // Whole device pixels, so it is crisp when it points straight up
        // or down.
        hand(s / 60.0, r * 0.82, style::hairline(ui.ctx()), c(p.failure));
    }
    painter.circle_filled(centre, 3.0, ink);
}

const MONTHS: [&str; 12] =
    ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

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
    ui.add(TextControl::new(&mut st.search).label("Search:").hint("City or region"));
    let stale = st.filtered.as_ref().is_none_or(|(q, z, _)| *q != st.search || !Arc::ptr_eq(z, zones));
    if stale {
        let q = st.search.to_lowercase();
        let selected = st.filtered.as_ref().and_then(|(_, z, rows)| st.zones.single().and_then(|i| rows.get(i)).map(|&r| z[r].clone()));
        let rows: Vec<usize> = (0..zones.len()).filter(|&i| zone_label(&zones[i]).to_lowercase().contains(&q) || zones[i].to_lowercase().contains(&q)).collect();
        st.zones.selected.clear();
        let keep = selected.or_else(|| (!st.zone_selected).then(|| current.to_owned()));
        if let Some(i) = keep.and_then(|k| rows.iter().position(|&r| zones[r] == k)) {
            st.zones.select(i);
            st.zone_selected = true;
            st.reveal = Some(i);
        }
        st.filtered = Some((st.search.clone(), zones.clone(), rows));
    }
    let Some((_, _, rows)) = &st.filtered else { return };
    let columns = [Column::new("Time zone", 200.0), Column::new("Offset", 96.0), Column::new("Time", 96.0).right()];
    let now = Timestamp::now();
    let list = Rect::from_min_size(ui.cursor().min, vec2(ui.available_width(), (ui.available_height() - 2.0 * (m::CONTROL_HEIGHT + m::SPACING)).max(100.0)));
    ui.scope_builder(UiBuilder::new().max_rect(list), |ui| {
        ColumnListView::new("zones", &columns, rows.len(), &mut st.zones).reveal(st.reveal.take()).show(ui, |cell| {
            let name = &zones[rows[cell.row]];
            let tz = TimeZone::get(name);
            match (cell.column, tz) {
                (0, _) => cell.text(&zone_label(name)),
                (1, Ok(tz)) => cell.text(&offset_text(&tz, now)),
                (2, Ok(tz)) => {
                    let t = now.to_zoned(tz);
                    let c = ClockSettings { show_seconds: false, ..clock.clone() };
                    cell.text(&c.format(t.hour() as u32, t.minute() as u32, 0, t.weekday().to_sunday_zero_offset() as u32));
                }
                _ => {}
            }
        });
    });
    ui.advance_cursor_after_rect(list);
    let chosen = st.zones.single().and_then(|i| rows.get(i)).map(|&r| zones[r].clone());
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = m::SPACING;
        let can = !busy && chosen.as_deref().is_some_and(|z| z != current);
        if ui.add_enabled_ui(can, |ui| ui.add(Button::new("Set Time Zone"))).inner.clicked()
            && let Some(z) = chosen
        {
            let _ = link().tx.send(Request::Zone(z));
        }
        ui.add(StringView::new(&format!("Now: {}", zone_label(current))));
    });
}

// ---- Clock ---------------------------------------------------------------------

fn clock(ui: &mut Ui, clock: &mut ClockSettings, now: &Zoned) {
    BBox::new("Deskbar clock").show(ui, |ui| {
        ui.add(CheckBox::new(&mut clock.twenty_four_hour, "24-hour clock"));
        ui.add(CheckBox::new(&mut clock.show_seconds, "Show seconds"));
        ui.add(CheckBox::new(&mut clock.show_day, "Show the day of the week"));
    });
    let sample = clock.format(now.hour() as u32, now.minute() as u32, now.second() as u32, now.weekday().to_sunday_zero_offset() as u32);
    ui.add(StringView::new(&format!("Deskbar shows: {sample}")));
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
