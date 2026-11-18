//! Calendar: every connected calendar in one (Ben, 2026-11-17: "build a
//! beautiful olo native calendar app … any number of google calendars (eg
//! from multiple google accounts) as well as any number of fastmail
//! calendars … aggregate all the connected calendar sources into one
//! unified calendar view, and also support read and write actions").
//!
//! - model.rs: the calendars kept, read, and their occurrences on the
//!   person's clock; layout.rs: where those go in a month and in a day.
//! - sync.rs: calendars in step with their servers, on workers; the demo.
//! - view.rs: the window (the small month and the calendars down the
//!   left; Day, Week, Month and Agenda); month.rs: the month.
//! - words.rs: days and times in words.
//!
//! The engine is crates/calendar (HACKING.md, "Calendar's engine").
//!
//! `olo-calendar --print [--from YYYY-MM-DD] [--days N]` prints what's on
//! (from what's kept; the demo's, with `OLO_CALENDAR_DEMO=1`) as JSON,
//! for agents and scripts, with no window.

mod agenda;
mod edit;
mod layout;
mod model;
mod month;
mod sync;
mod view;
mod week;
mod words;

use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use jiff::{Zoned, civil::Date, tz::TimeZone};
use olo_calendar::dav;
use olo_kit::{
    Cx, Element, ScrollHandle, View, WindowOptions,
    due::Due,
    instance::{self, Launch},
};
use olo_theme::Hue;
use serde::{Deserialize, Serialize};

use crate::{
    model::{Book, Cal, Choices, Shown},
    sync::Source,
};

pub const APP_ID: &str = "olo.calendar";
/// How often the servers are asked what changed, while the window's open.
const SYNC_EVERY: Duration = Duration::from_secs(15 * 60);
/// Coming back to the window after this long asks at once.
const SYNC_ON_RETURN: Duration = Duration::from_secs(2 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Day,
    Week,
    #[default]
    Month,
    Agenda,
}

impl Mode {
    pub const ALL: [Mode; 4] = [Mode::Day, Mode::Week, Mode::Month, Mode::Agenda];

    pub fn name(self) -> &'static str {
        match self {
            Mode::Day => "Day",
            Mode::Week => "Week",
            Mode::Month => "Month",
            Mode::Agenda => "Agenda",
        }
    }
}

/// What Calendar keeps (`$XDG_CONFIG_HOME/olo/calendar.json`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub mode: Mode,
    /// The day weeks start on (0 Monday … 6 Sunday); None: as the
    /// person's region has it.
    pub week_start: Option<u8>,
    pub sidebar: bool,
    pub choices: Choices,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { mode: Mode::Month, week_start: None, sidebar: true, choices: Choices::default() }
    }
}

fn settings_path() -> std::path::PathBuf {
    olo_kit::config_dir().join("calendar.json")
}

/// The day weeks start on where the person is, by their region (`LC_TIME`,
/// else `LANG`): Sunday in the Americas, Japan and a few others, Saturday
/// in much of the Middle East, Monday elsewhere.
fn regions_week_start() -> u8 {
    let locale = ["LC_ALL", "LC_TIME", "LANG"].iter().find_map(|v| std::env::var(v).ok().filter(|s| !s.is_empty())).unwrap_or_default();
    let region = locale.split(['.', '@']).next().unwrap_or_default().rsplit('_').next().unwrap_or_default().to_ascii_uppercase();
    match region.as_str() {
        "US" | "CA" | "MX" | "BR" | "AR" | "CO" | "PE" | "VE" | "JP" | "KR" | "TW" | "HK" | "PH" | "IN" | "IL" | "ZA" | "AU" | "SA" => 6,
        "AE" | "EG" | "IR" | "AF" | "DZ" | "BH" | "IQ" | "JO" | "KW" | "LY" | "OM" | "QA" | "SY" => 5,
        _ => 0,
    }
}

/// Now, on the person's clock (`OLO_CALENDAR_NOW=2026-11-18T10:30`: a
/// test's own now, so its pictures are the same every day).
pub fn now(zone: &TimeZone) -> Zoned {
    if let Ok(fixed) = std::env::var("OLO_CALENDAR_NOW")
        && let Ok(at) = fixed.parse::<jiff::civil::DateTime>()
        && let Ok(z) = at.to_zoned(zone.clone())
    {
        return z;
    }
    Zoned::now().with_time_zone(zone.clone())
}

/// How an account's calendars stand with its server.
#[derive(Debug, Clone, Default)]
pub struct Standing {
    pub syncing: bool,
    pub error: Option<dav::Error>,
    /// When it was last in step.
    pub synced: Option<std::time::SystemTime>,
}

/// What tells one occurrence shown from another.
pub type Key = (usize, Arc<str>, jiff::civil::DateTime);

#[derive(Clone)]
pub enum Msg {
    /// The accounts, and what's kept of their calendars, read.
    Loaded(Vec<Source>, Vec<Arc<Cal>>),
    /// An account (by its id) asked what changed: its calendars now, if
    /// anything did.
    Synced(String, Option<Vec<Arc<Cal>>>, Result<(), dav::Error>),
    /// The occurrences from a day to another, worked out (answer `n`).
    Shown(u64, Date, Date, Arc<Vec<Shown>>),
    /// The clock's 12- or 24-hour style.
    Clock(bool),
    Mode(Mode),
    /// Show this day.
    Go(Date),
    /// The small month turned to this year and month.
    Mini(i16, i8),
    /// A calendar (by its place) shown or not.
    Show(usize, bool),
    Select(Option<Key>),
}

pub struct Calendar {
    pub settings: Settings,
    demo: bool,
    pub zone: TimeZone,
    pub h24: bool,
    pub today: Date,
    /// The day the view is at.
    pub cursor: Date,
    /// The month the small month shows.
    pub mini: (i16, i8),
    pub sources: Vec<Source>,
    pub book: Arc<Book>,
    /// Goes up whenever what's shown could differ: the book, the choices.
    generation: u64,
    /// The occurrences worked out, the days they cover, and for which
    /// generation.
    pub shown: Arc<Vec<Shown>>,
    /// Goes up whenever `shown` is another list.
    pub shown_version: u64,
    covered: Option<(Date, Date, u64)>,
    asking: u64,
    pub selected: Option<Key>,
    pub standing: HashMap<String, Standing>,
    /// Nothing's been read from disk yet.
    pub loading: bool,
    last_sync: Option<Instant>,
    due: Due,
    pub calendars_scroll: ScrollHandle,
    pub hours_scroll: ScrollHandle,
    hours_placed: bool,
    pub agenda_scroll: ScrollHandle,
}

/// What's kept across an upgrade in place.
#[derive(Serialize, Deserialize)]
struct Saved {
    cursor: String,
    mode: Mode,
}

impl Calendar {
    fn new(cx: &mut Cx<Self>) -> Calendar {
        let demo = sync::demo();
        let mut settings: Settings = if demo { Settings::default() } else { olo_kit::store::load(&settings_path()).unwrap_or_default() };
        let zone = TimeZone::system();
        let today = now(&zone).date();
        let mut cursor = today;
        if let Some(saved) = olo_kit::restored_state().and_then(|s| serde_json::from_str::<Saved>(&s).ok()) {
            settings.mode = saved.mode;
            cursor = saved.cursor.parse().unwrap_or(today);
        }
        let clock = cx.messenger();
        olo_kit::settings::watch(move |s| {
            clock.send(Msg::Clock(s.clock.twenty_four_hour));
        });
        cx.work(move || sync::load(today), |(sources, cals)| Msg::Loaded(sources, cals.into_iter().map(Arc::new).collect()));
        let mut v = Calendar {
            settings,
            demo,
            zone,
            h24: true,
            today,
            cursor,
            mini: (cursor.year(), cursor.month()),
            sources: Vec::new(),
            book: Arc::new(Book::default()),
            generation: 0,
            shown: Arc::new(Vec::new()),
            shown_version: 0,
            covered: None,
            asking: 0,
            selected: None,
            standing: HashMap::new(),
            loading: true,
            last_sync: None,
            due: Due::default(),
            calendars_scroll: ScrollHandle::new(),
            hours_scroll: ScrollHandle::new(),
            hours_placed: false,
            agenda_scroll: ScrollHandle::new(),
        };
        v.place_hours(cx);
        v.wake_when_due(cx);
        v
    }

    /// The hours, the first time they show, start a little before the
    /// working day (once they're laid out: there's nothing to scroll
    /// before); after that they're where the person left them.
    fn place_hours(&mut self, cx: &mut Cx<Self>) {
        if self.hours_placed || !matches!(self.settings.mode, Mode::Day | Mode::Week) {
            return;
        }
        self.hours_placed = true;
        let hour = now(&self.zone).hour();
        self.hours_scroll.set_offset_once_laid_out(cx, olo_kit::point(olo_kit::px(0.0), olo_kit::px(-week::starting_offset(hour))));
    }

    fn save(&self) {
        if !self.demo {
            olo_kit::store::save(settings_path(), &self.settings);
        }
    }

    /// The day weeks start on: 0 Monday … 6 Sunday.
    pub fn week_start(&self) -> u8 {
        self.settings.week_start.unwrap_or_else(regions_week_start) % 7
    }

    /// The days the view shows: the first, and the day after the last.
    pub fn visible(&self) -> (Date, Date) {
        match self.settings.mode {
            Mode::Day => (self.cursor, layout::day_of(layout::day_number(self.cursor) + 1)),
            Mode::Week => {
                let start = layout::week_of(self.cursor, self.week_start());
                (start, layout::day_of(layout::day_number(start) + 7))
            }
            Mode::Month => {
                let (start, weeks) = layout::month_grid(self.cursor.year(), self.cursor.month(), self.week_start());
                (start, layout::day_of(layout::day_number(start) + 7 * weeks as i32))
            }
            Mode::Agenda => (self.cursor, layout::day_of(layout::day_number(self.cursor) + view::AGENDA_DAYS)),
        }
    }

    /// The days the small month shows.
    fn mini_days(&self) -> (Date, Date) {
        let start = layout::week_of(jiff::civil::date(self.mini.0, self.mini.1, 1), self.week_start());
        (start, layout::day_of(layout::day_number(start) + 42))
    }

    /// Have the occurrences for what shows: nothing to do if those worked
    /// out cover it; else worked out on a worker for what shows and as
    /// much again either side (stepping to the next week or month is
    /// there already).
    fn look(&mut self, cx: &mut Cx<Self>) {
        let ((a, b), (ma, mb)) = (self.visible(), self.mini_days());
        let (first, last) = (a.min(ma), b.max(mb));
        if self.covered.is_some_and(|(from, to, generation)| generation == self.generation && from <= first && last <= to) {
            return;
        }
        let pad = (layout::day_number(b) - layout::day_number(a)).max(7);
        let (from, to) = (layout::day_of(layout::day_number(first) - pad), layout::day_of(layout::day_number(last) + pad));
        self.asking += 1;
        let n = self.asking;
        // (Taken as covered now, so that nothing asks twice; the answer
        // says which asking it's for.)
        self.covered = Some((from, to, self.generation));
        let (book, zone, choices) = (self.book.clone(), self.zone.clone(), self.settings.choices.clone());
        let me: Vec<String> = self.sources.iter().map(|s| s.account.address.clone()).collect();
        cx.work(move || Arc::new(book.shown(from, to, &zone, &me, &choices)), move |shown| Msg::Shown(n, from, to, shown));
    }

    /// What's shown may differ now: worked out again.
    fn changed(&mut self, cx: &mut Cx<Self>) {
        self.generation += 1;
        self.look(cx);
    }

    /// Ask an account's server what changed, on a worker (one asking an
    /// account at a time).
    fn sync(&mut self, id: &str, cx: &mut Cx<Self>) {
        let Some(source) = self.sources.iter().find(|s| s.account.id() == id).cloned() else { return };
        let standing = self.standing.entry(id.to_owned()).or_default();
        if standing.syncing {
            return;
        }
        standing.syncing = true;
        let known: Vec<olo_calendar::store::Kept> = self.book.cals.iter().filter(|c| c.kept.account == id).map(|c| c.kept.clone()).collect();
        let id = id.to_owned();
        cx.work(move || sync::sync(&source, known), move |(cals, result)| Msg::Synced(id, cals.map(|c| c.into_iter().map(Arc::new).collect()), result));
        cx.invalidate();
    }

    pub fn sync_all(&mut self, cx: &mut Cx<Self>) {
        self.last_sync = Some(Instant::now());
        for id in self.sources.iter().map(|s| s.account.id()).collect::<Vec<_>>() {
            self.sync(&id, cx);
        }
        self.wake_when_due(cx);
    }

    /// One timer for what's next: the day turning, or the servers asked
    /// again.
    fn wake_when_due(&mut self, cx: &mut Cx<Self>) {
        let now = now(&self.zone);
        let midnight = now.date().tomorrow().ok().and_then(|d| d.at(0, 0, 1, 0).to_zoned(self.zone.clone()).ok());
        let to_midnight = midnight.and_then(|m| m.duration_since(&now).try_into().ok()).unwrap_or(Duration::from_secs(3600));
        let to_sync = self.last_sync.map_or(SYNC_EVERY, |at| SYNC_EVERY.saturating_sub(at.elapsed())).max(Duration::from_secs(1));
        self.due.at(Some(Instant::now() + to_midnight.min(to_sync)), cx, |v: &mut Calendar, cx| v.woken(cx));
    }

    /// The timer's, and the window's coming forward: the day may have
    /// turned, the servers may be due.
    fn woken(&mut self, cx: &mut Cx<Self>) {
        let today = now(&self.zone).date();
        if today != self.today {
            // (Still on today: today it stays.)
            if self.cursor == self.today {
                self.cursor = today;
                self.mini = (today.year(), today.month());
            }
            self.today = today;
            self.look(cx);
            cx.invalidate();
        }
        if !self.loading && self.last_sync.is_none_or(|at| at.elapsed() >= SYNC_EVERY) {
            self.sync_all(cx);
        }
        self.wake_when_due(cx);
    }

    pub fn go(&mut self, day: Date, cx: &mut Cx<Self>) {
        if day != self.cursor {
            self.cursor = day;
            self.mini = (day.year(), day.month());
            self.look(cx);
            cx.invalidate();
        }
    }

    /// A step on (or back, negative) by what the view shows: a day, a
    /// week, a month.
    pub fn step(&mut self, by: i32, cx: &mut Cx<Self>) {
        let to = match self.settings.mode {
            Mode::Day => layout::day_of(layout::day_number(self.cursor) + by),
            Mode::Week | Mode::Agenda => layout::day_of(layout::day_number(self.cursor) + 7 * by),
            Mode::Month => {
                let (y, m) = olo_kit::widgets::month::turned(self.cursor.year() as i32, self.cursor.month() as u8, by);
                let first = jiff::civil::date(y as i16, m as i8, 1);
                first.with().day(self.cursor.day().min(first.days_in_month())).build().unwrap_or(first)
            }
        };
        self.go(to, cx);
    }

    pub fn set_mode(&mut self, mode: Mode, cx: &mut Cx<Self>) {
        if mode != self.settings.mode {
            self.settings.mode = mode;
            self.save();
            self.place_hours(cx);
            self.look(cx);
            cx.invalidate();
        }
    }

    /// What's being done with the servers, or what went wrong, in words;
    /// and whether it's something wrong.
    pub fn status(&self) -> (String, bool) {
        if self.loading {
            return ("Opening…".into(), false);
        }
        if self.standing.values().any(|s| s.syncing) {
            return ("Updating…".into(), false);
        }
        let wrong: Vec<String> = self
            .sources
            .iter()
            .filter_map(|s| {
                let e = self.standing.get(&s.account.id())?.error.as_ref()?;
                Some(match e {
                    dav::Error::SignIn => format!("{} needs signing in again", s.account.address),
                    e => format!("{}: {e}", s.account.address),
                })
            })
            .collect();
        if !wrong.is_empty() {
            return (wrong.join(" · "), true);
        }
        match self.standing.values().filter_map(|s| s.synced).min() {
            Some(at) => (format!("Updated {}", olo_kit::ago::ago(at, olo_kit::ago::long)), false),
            None => (String::new(), false),
        }
    }

    pub fn menus(&self) -> Vec<olo_kit::Menu> {
        use olo_kit::Menu;
        let mode = self.settings.mode;
        let start = self.week_start();
        let mut week = Menu::new("Week Starts On");
        for (day, name) in [(0u8, "Monday"), (6, "Sunday"), (5, "Saturday")] {
            week = week
                .item(
                    name,
                    match day {
                        0 => "week-start:0",
                        6 => "week-start:6",
                        _ => "week-start:5",
                    },
                )
                .checked(start == day);
        }
        let (step, back, on) = match mode {
            Mode::Day => ("Day", "Previous Day", "Next Day"),
            Mode::Week | Mode::Agenda => ("Week", "Previous Week", "Next Week"),
            Mode::Month => ("Month", "Previous Month", "Next Month"),
        };
        let _ = step;
        vec![
            Menu::new("File").app_items(&["integrations"]).item("Refresh", "refresh").key("r").enabled(!self.sources.is_empty()).close_quit(),
            Menu::new("View")
                .item("Day", "mode:day")
                .key("1")
                .checked(mode == Mode::Day)
                .item("Week", "mode:week")
                .key("2")
                .checked(mode == Mode::Week)
                .item("Month", "mode:month")
                .key("3")
                .checked(mode == Mode::Month)
                .item("Agenda", "mode:agenda")
                .key("4")
                .checked(mode == Mode::Agenda)
                .separator()
                .item("Today", "today")
                .key("t")
                .item(back, "previous")
                .key("[")
                .item(on, "next")
                .key("]")
                .separator()
                .item("Show Calendars", "sidebar")
                .checked(self.settings.sidebar)
                .submenu(week),
        ]
    }
}

impl View for Calendar {
    type Message = Msg;

    fn message(&mut self, message: Msg, cx: &mut Cx<Self>) {
        match message {
            Msg::Loaded(sources, cals) => {
                self.sources = sources;
                self.loading = false;
                let order: Vec<String> = self.sources.iter().map(|s| s.account.id()).collect();
                self.book = Arc::new(Book::new(cals, &order));
                self.changed(cx);
                self.sync_all(cx);
            }
            Msg::Synced(id, cals, result) => {
                let order: Vec<String> = self.sources.iter().map(|s| s.account.id()).collect();
                let standing = self.standing.entry(id.clone()).or_default();
                standing.syncing = false;
                match &result {
                    Ok(()) => {
                        standing.error = None;
                        standing.synced = Some(std::time::SystemTime::now());
                    }
                    Err(e) => {
                        tracing::warn!("calendar: {id}: {e}");
                        standing.error = Some(e.clone());
                    }
                }
                if let Some(cals) = cals {
                    let others = self.book.cals.iter().filter(|c| c.kept.account != id).cloned();
                    self.book = Arc::new(Book::new(others.chain(cals).collect(), &order));
                    self.changed(cx);
                }
            }
            Msg::Shown(n, from, to, shown) => {
                if n != self.asking {
                    return;
                }
                self.covered = Some((from, to, self.generation));
                self.shown = shown;
                self.shown_version += 1;
                // (What was selected and is no longer there isn't selected.)
                if let Some(key) = &self.selected
                    && !self.shown.iter().any(|s| s.key() == *key)
                {
                    self.selected = None;
                }
            }
            Msg::Clock(h24) => {
                if self.h24 == h24 {
                    return;
                }
                self.h24 = h24;
            }
            Msg::Mode(mode) => return self.set_mode(mode, cx),
            Msg::Go(day) => return self.go(day, cx),
            Msg::Mini(year, month) => {
                self.mini = (year, month);
                self.look(cx);
            }
            Msg::Show(i, on) => {
                let Some(href) = self.book.cals.get(i).map(|c| c.kept.href.clone()) else { return };
                let changed = if on { self.settings.choices.hidden.remove(&href) } else { self.settings.choices.hidden.insert(href) };
                if !changed {
                    return;
                }
                self.save();
                self.changed(cx);
            }
            Msg::Select(key) => {
                if self.selected == key {
                    return;
                }
                self.selected = key;
            }
        }
        cx.invalidate();
    }

    fn command(&mut self, command: &str, cx: &mut Cx<Self>) -> bool {
        match command {
            "refresh" => self.sync_all(cx),
            "today" => {
                let today = self.today;
                self.go(today, cx);
            }
            "previous" => self.step(-1, cx),
            "next" => self.step(1, cx),
            "sidebar" => {
                self.settings.sidebar = !self.settings.sidebar;
                self.save();
                cx.invalidate();
            }
            "close" | "quit" => olo_kit::quit(),
            c if c.starts_with("mode:") => match Mode::ALL.into_iter().find(|m| m.name().eq_ignore_ascii_case(&c[5..])) {
                Some(mode) => self.set_mode(mode, cx),
                None => return false,
            },
            c if c.starts_with("week-start:") => {
                let Ok(day) = c[11..].parse::<u8>() else { return false };
                self.settings.week_start = Some(day % 7);
                self.save();
                self.covered = None;
                self.look(cx);
                cx.invalidate();
            }
            // (An agent's: a day, as 2026-11-20.)
            c if c.starts_with("go:") => match c[3..].parse::<Date>() {
                Ok(day) => self.go(day, cx),
                Err(_) => return false,
            },
            // (An agent's: a calendar by its name, shown or not.)
            c if c.starts_with("show:") || c.starts_with("hide:") => {
                let Some(i) = self.book.cals.iter().position(|cal| cal.kept.name.eq_ignore_ascii_case(&c[5..])) else { return false };
                cx.send(Msg::Show(i, c.starts_with("show:")));
            }
            _ => return false,
        }
        true
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        view::body(self, cx)
    }

    fn window_activated(&mut self, active: bool, cx: &mut Cx<Self>) {
        if !active {
            return;
        }
        if !self.loading && self.last_sync.is_some_and(|at| at.elapsed() >= SYNC_ON_RETURN) {
            self.sync_all(cx);
        }
        self.woken(cx);
    }

    fn save_state(&mut self, _: &mut Cx<Self>) -> Option<String> {
        serde_json::to_string(&Saved { cursor: self.cursor.to_string(), mode: self.settings.mode }).ok()
    }
}

/// A calendar's colour in the look in force, by its place in the book.
pub fn hue_of(v: &Calendar, i: usize) -> Hue {
    v.book.hue(i, &v.settings.choices)
}

/// `olo-calendar --print [--from YYYY-MM-DD] [--days N]`: what's on, as
/// JSON, from what's kept (the demo's own, in the demo), with no window.
fn print(args: &[String]) -> std::process::ExitCode {
    let value = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1));
    let zone = TimeZone::system();
    let from = value("--from").and_then(|d| d.parse::<Date>().ok()).unwrap_or_else(|| now(&zone).date());
    let days = value("--days").and_then(|n| n.parse::<i32>().ok()).unwrap_or(7).clamp(1, 3660);
    let (sources, mut cals) = sync::load(from);
    if sync::demo() {
        cals = sources.iter().flat_map(|s| sync::sync(s, Vec::new()).0.unwrap_or_default()).collect();
    }
    let order: Vec<String> = sources.iter().map(|s| s.account.id()).collect();
    let settings: Settings = if sync::demo() { Settings::default() } else { olo_kit::store::load(&settings_path()).unwrap_or_default() };
    let book = Book::new(cals.into_iter().map(Arc::new).collect(), &order);
    let me: Vec<String> = sources.iter().map(|s| s.account.address.clone()).collect();
    let shown = book.shown(from, layout::day_of(layout::day_number(from) + days), &zone, &me, &settings.choices);
    let out: Vec<serde_json::Value> = shown
        .iter()
        .map(|s| {
            serde_json::json!({
                "title": &*s.title,
                "calendar": book.cals.get(s.cal).map(|c| c.kept.name.clone()),
                "start": if s.all_day { s.start.date().to_string() } else { s.start.to_string() },
                "end": if s.all_day { s.end.date().to_string() } else { s.end.to_string() },
                "all_day": s.all_day,
                "location": &*s.location,
                "repeats": s.repeats,
            })
        })
        .collect();
    println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
    std::process::ExitCode::SUCCESS
}

fn main() -> std::process::ExitCode {
    olo_crash::install();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "--print") {
        return print(&args);
    }
    if instance::forward(APP_ID, &args) {
        return std::process::ExitCode::SUCCESS;
    }
    olo_kit::run(APP_ID, |app| {
        let window = olo_kit::open_window(WindowOptions::new("Calendar", 1080.0, 720.0).min_size(view::MIN_WIDTH, view::MIN_HEIGHT).keep_size("main"), Calendar::new);
        instance::listen(APP_ID, olo_kit::proxy());
        olo_kit::message_handler(app, move |message, _| {
            if message.is::<Launch>() {
                olo_kit::raise(Some(&window));
            }
        });
    });
    std::process::ExitCode::SUCCESS
}
