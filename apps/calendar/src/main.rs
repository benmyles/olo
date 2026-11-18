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
mod editor;
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
    /// The calendar the last new event went in (its address).
    pub calendar: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { mode: Mode::Month, week_start: None, sidebar: true, choices: Choices::default(), calendar: None }
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

/// Whether the person's region writes the month before the day (3/4 is
/// March 4th): the United States and a few others.
fn month_first() -> bool {
    let locale = ["LC_ALL", "LC_TIME", "LANG"].iter().find_map(|v| std::env::var(v).ok().filter(|s| !s.is_empty())).unwrap_or_default();
    let region = locale.split(['.', '@']).next().unwrap_or_default().rsplit('_').next().unwrap_or_default().to_ascii_uppercase();
    matches!(region.as_str(), "US" | "PH" | "FM" | "PW" | "MH")
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
    /// The list of accounts changed (Preferences ▸ Integrations).
    Accounts,
    /// The occurrence opened in the editor (a double-click, Return).
    Open(Key),
    /// A new event, starting then, or all that day (the editor opened
    /// for it).
    New(jiff::civil::DateTime, bool),
    /// The editor's Save, and its Delete.
    Save(Box<editor::Saving>),
    Delete(Box<editor::Target>),
    /// The answer to "only this one, or all?" (or "delete it?").
    Asked(Option<usize>),
    /// Something written to a calendar's server (its account, the
    /// calendar as it is now), and how it went.
    Wrote(String, Arc<Cal>, Result<(), dav::Error>),
}

/// A change waiting for the person's answer: which occurrences of a
/// repeating event it's for, or whether to delete at all.
enum Waiting {
    Save(editor::Saving),
    /// A repeating event's occurrence: only it, or all.
    DeleteWhich(editor::Target),
    /// One event: sure?
    Delete(editor::Target),
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
    editor: Option<olo_kit::WindowHandle<editor::Editor>>,
    /// The accounts' file followed (kept while the window is).
    _accounts: Option<olo_kit::watch::Watch>,
    waiting: Option<Waiting>,
    /// What's to be written, in order: the calendar's account and
    /// address, and the job. One at a time an account, and never while
    /// it's being asked what changed.
    writes: std::collections::VecDeque<(String, String, sync::Job)>,
    /// How many are being written now.
    pub writing: usize,
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
            editor: None,
            _accounts: None,
            waiting: None,
            writes: Default::default(),
            writing: 0,
            agenda_scroll: ScrollHandle::new(),
        };
        v.place_hours(cx);
        // An account added or removed in Preferences shows here at once.
        if !demo {
            let changed = cx.messenger();
            v._accounts = olo_kit::watch::watch([olo_calendar::account::accounts_path()], move |_| {
                changed.send(Msg::Accounts);
            });
        }
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

    /// The calendars a new event can go in: those shown that may be
    /// written to, by their place in the book.
    fn writable(&self) -> Vec<(usize, String)> {
        self.book.cals.iter().enumerate().filter(|(_, c)| c.kept.writable && !self.settings.choices.hidden.contains(&c.kept.href)).map(|(i, c)| (i, c.kept.name.clone())).collect()
    }

    /// The editor, for a new event starting at `start`.
    pub fn open_new(&mut self, start: jiff::civil::DateTime, all_day: bool, cx: &mut Cx<Self>) {
        let calendars = self.writable();
        if calendars.is_empty() {
            let why =
                if self.book.cals.is_empty() { "There's no calendar to put it in yet: connect an account in Preferences ▸ Integrations." } else { "None of the calendars shown can be written to." };
            olo_kit::widgets::open_alert(cx, "Calendar", "warning", why, &["OK"], |_, _| {});
            return;
        }
        // The calendar the last one went in, if it's still there.
        let calendar = self.settings.calendar.as_ref().and_then(|href| calendars.iter().position(|(i, _)| self.book.cals[*i].kept.href == *href)).unwrap_or(0);
        let opening =
            editor::Opening { target: editor::Target::New, draft: edit::Draft::new(start, all_day), calendars, calendar, today: self.today, h24: self.h24, month_first: month_first(), clock: None };
        self.open_editor(opening, cx);
    }

    /// When a new event starts by default: on the day shown, at the next
    /// whole hour if that's today, else at nine.
    pub fn new_start(&self) -> jiff::civil::DateTime {
        let now = now(&self.zone);
        if self.cursor == self.today && now.hour() < 23 { self.cursor.at(now.hour() + 1, 0, 0, 0) } else { self.cursor.at(9, 0, 0, 0) }
    }

    /// The editor, for the occurrence `key`.
    pub fn open_event(&mut self, key: &Key, cx: &mut Cx<Self>) {
        let Some(shown) = self.shown.iter().find(|s| s.key() == *key).cloned() else { return };
        let Some(stored) = self.book.stored(&shown) else { return };
        // The occurrence's own text, if it was changed by itself; else the event's.
        let event = shown.id.as_ref().and_then(|id| stored.overrides.iter().find(|o| o.recurrence_id.as_ref().is_some_and(|r| olo_calendar::event::same(r, id, &self.zone)))).unwrap_or(&stored.event);
        // Its times on its own clock (another zone's, said).
        let (zone, viewer) = (self.zone.clone(), self.zone.clone());
        let its = |t: jiff::civil::DateTime| match &event.start {
            olo_calendar::event::When::Zoned(z) => t.to_zoned(viewer.clone()).map(|v| v.with_time_zone(z.time_zone().clone()).datetime()).unwrap_or(t),
            _ => t,
        };
        let clock = match &event.start {
            olo_calendar::event::When::Zoned(z) if z.time_zone().iana_name() != zone.iana_name() => z.time_zone().iana_name().map(|n| n.rsplit('/').next().unwrap_or(n).replace('_', " ")),
            _ => None,
        };
        let (start, end) = (its(shown.start), its(shown.end));
        let name = self.book.cals.get(shown.cal).map(|c| c.kept.name.clone()).unwrap_or_default();
        let target = editor::Target::Existing { cal: shown.cal, href: shown.href.to_string(), id: shown.id.clone(), from: start, repeats: shown.repeats };
        // (How it repeats is the event's own, whichever occurrence is opened.)
        let series = edit::Draft::of(&stored.event, start, end);
        let draft = edit::Draft { repeat: series.repeat, until: series.until, ..edit::Draft::of(event, start, end) };
        let opening = editor::Opening { target, draft, calendars: vec![(shown.cal, name)], calendar: 0, today: self.today, h24: self.h24, month_first: month_first(), clock };
        self.open_editor(opening, cx);
    }

    fn open_editor(&mut self, opening: editor::Opening, cx: &mut Cx<Self>) {
        // (One at a time: another event opened takes its place.)
        if let Some(open) = self.editor.take() {
            open.close();
        }
        self.editor = Some(editor::Editor::open(opening, cx.messenger()));
    }

    /// A job for calendar `cal` put in line, and the line looked at.
    fn write(&mut self, cal: usize, job: sync::Job, cx: &mut Cx<Self>) {
        let Some(c) = self.book.cals.get(cal) else { return };
        self.writes.push_back((c.kept.account.clone(), c.kept.href.clone(), job));
        self.run_writes(cx);
    }

    /// The first job in line whose account is free is done, on a worker.
    fn run_writes(&mut self, cx: &mut Cx<Self>) {
        let Some(at) = self.writes.iter().position(|(account, ..)| !self.standing.get(account).is_some_and(|s| s.syncing)) else { return };
        let Some((account, href, job)) = self.writes.remove(at) else { return };
        let (Some(source), Some(kept)) = (self.sources.iter().find(|s| s.account.id() == account).cloned(), self.book.cals.iter().find(|c| c.kept.href == href).map(|c| c.kept.clone())) else {
            return;
        };
        self.standing.entry(account.clone()).or_default().syncing = true;
        self.writing += 1;
        cx.work(move || sync::write(&source, kept, job), move |(cal, result)| Msg::Wrote(account, Arc::new(cal), result));
        cx.invalidate();
    }

    /// A change to an event, done: to every occurrence, or only the one
    /// it was made on.
    fn save_event(&mut self, saving: editor::Saving, only_this: bool, cx: &mut Cx<Self>) {
        let zone = self.zone.clone();
        let draft = saving.draft;
        match saving.target {
            editor::Target::New => {
                // (Its times take their kind from the draft: a day's, or a time in the person's zone.)
                let day = olo_calendar::event::When::Date(draft.start.date());
                let mut event = olo_calendar::event::Event::new(&olo_calendar::event::new_uid(), "", day.clone(), day);
                draft.apply(&mut event, draft.start, draft.end, &zone);
                if let Some(c) = self.book.cals.get(saving.cal) {
                    self.settings.calendar = Some(c.kept.href.clone());
                    self.save();
                }
                self.write(saving.cal, sync::Job::Create(Box::new(event)), cx);
            }
            editor::Target::Existing { cal, href, id, from, .. } => {
                let which = id.clone();
                let job = match id.filter(|_| only_this) {
                    Some(id) => sync::Job::Change(href, Box::new(move |c| edit::change_one(c, &id, &draft, &zone))),
                    None => sync::Job::Change(href, Box::new(move |c| edit::change_all(c, &draft, from, which.as_ref(), &zone))),
                };
                self.write(cal, job, cx);
            }
        }
    }

    fn delete_event(&mut self, target: editor::Target, only_this: bool, cx: &mut Cx<Self>) {
        let editor::Target::Existing { cal, href, id, .. } = target else { return };
        let zone = self.zone.clone();
        let job = match id.filter(|_| only_this) {
            Some(id) => sync::Job::Change(href, Box::new(move |c| edit::remove_one(c, &id, &zone))),
            None => sync::Job::Remove(href),
        };
        self.selected = None;
        self.write(cal, job, cx);
    }

    /// The occurrence selected, as the editor's target.
    fn selected_target(&self) -> Option<(editor::Target, String)> {
        let key = self.selected.as_ref()?;
        let shown = self.shown.iter().find(|s| s.key() == *key)?;
        Some((editor::Target::Existing { cal: shown.cal, href: shown.href.to_string(), id: shown.id.clone(), from: shown.start, repeats: shown.repeats }, shown.title.to_string()))
    }

    /// Delete what `target` names, once the person has said which of it
    /// (a repeating event's occurrence: only it, or all) or that they're
    /// sure (nothing brings an event back).
    fn ask_delete(&mut self, target: editor::Target, title: &str, cx: &mut Cx<Self>) {
        let editor::Target::Existing { id, repeats, .. } = &target else { return };
        let asked = cx.msg_with(|choice: &Option<usize>| Msg::Asked(*choice));
        if *repeats && id.is_some() {
            self.waiting = Some(Waiting::DeleteWhich(target));
            olo_kit::widgets::open_alert(cx, "Calendar", "warning", &format!("“{title}” repeats. Delete only this one, or every one of them?"), &["Cancel", "All of Them", "Only This One"], asked);
        } else {
            self.waiting = Some(Waiting::Delete(target));
            olo_kit::widgets::open_alert(cx, "Calendar", "warning", &format!("Delete “{title}”? It’s deleted from its calendar everywhere, and can’t be brought back."), &["Cancel", "Delete"], asked);
        }
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
        if self.writing > 0 || !self.writes.is_empty() {
            return ("Saving…".into(), false);
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
            // (Said by the minute: a status that counts seconds draws a
            // frame a second for a minute after every asking.)
            Some(at) => (
                format!(
                    "Updated {}",
                    olo_kit::ago::ago(at, |age| match age.as_secs() {
                        s if s < 60 => "just now".into(),
                        s if s < 120 => "a minute ago".into(),
                        s if s < 3600 => format!("{} minutes ago", s / 60),
                        s if s < 7200 => "an hour ago".into(),
                        s => format!("{} hours ago", s / 3600),
                    })
                ),
                false,
            ),
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
            Menu::new("File")
                .app_items(&["integrations"])
                .item("New Event…", "new-event")
                .key("n")
                .enabled(!self.loading)
                .item("Refresh", "refresh")
                .key("r")
                .enabled(!self.sources.is_empty())
                .close_quit(),
            Menu::new("Edit").item("Edit Event…", "edit-event").key("e").enabled(self.selected.is_some()).item("Delete Event", "delete-event").key("backspace").enabled(self.selected.is_some()),
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
                // (What waited for the asking to end.)
                self.run_writes(cx);
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
            Msg::Accounts => {
                // Read again, as when the window opened (what's kept of
                // an account that went is let go there; a new one's
                // calendars come with the asking that follows).
                let today = self.today;
                cx.work(move || sync::load(today), |(sources, cals)| Msg::Loaded(sources, cals.into_iter().map(Arc::new).collect()));
                return;
            }
            Msg::Open(key) => {
                self.selected = Some(key.clone());
                self.open_event(&key, cx);
            }
            Msg::New(start, all_day) => self.open_new(start, all_day, cx),
            Msg::Save(saving) => match &saving.target {
                // A repeating event's occurrence: only it, or all of them?
                editor::Target::Existing { id: Some(_), repeats: true, .. } => {
                    let title = if saving.draft.title.trim().is_empty() { "This event".to_owned() } else { format!("“{}”", saving.draft.title.trim()) };
                    self.waiting = Some(Waiting::Save(*saving));
                    let asked = cx.msg_with(|choice: &Option<usize>| Msg::Asked(*choice));
                    olo_kit::widgets::open_alert(
                        cx,
                        "Calendar",
                        "warning",
                        &format!("{title} repeats. Change only this one, or every one of them?"),
                        &["Cancel", "All of Them", "Only This One"],
                        asked,
                    );
                }
                _ => self.save_event(*saving, false, cx),
            },
            Msg::Delete(target) => {
                let title = match &*target {
                    editor::Target::Existing { cal, href, .. } => self.book.cals.get(*cal).and_then(|c| c.events.iter().find(|s| s.href == *href)).map(|s| s.event.summary.clone()).unwrap_or_default(),
                    editor::Target::New => return,
                };
                let title = if title.trim().is_empty() { "this event".to_owned() } else { title };
                self.ask_delete(*target, &title, cx);
            }
            Msg::Asked(choice) => match (self.waiting.take(), choice) {
                // (Only This One is Return's: the smaller change.)
                (Some(Waiting::Save(saving)), Some(2)) => self.save_event(saving, true, cx),
                (Some(Waiting::Save(saving)), Some(1)) => self.save_event(saving, false, cx),
                (Some(Waiting::DeleteWhich(target)), Some(2)) => self.delete_event(target, true, cx),
                (Some(Waiting::DeleteWhich(target)), Some(1)) => self.delete_event(target, false, cx),
                (Some(Waiting::Delete(target)), Some(1)) => self.delete_event(target, false, cx),
                _ => return,
            },
            Msg::Wrote(account, cal, result) => {
                let order: Vec<String> = self.sources.iter().map(|s| s.account.id()).collect();
                self.standing.entry(account).or_default().syncing = false;
                self.writing = self.writing.saturating_sub(1);
                // The calendar as it is now: with the change, or, changed
                // somewhere else meanwhile, with that one.
                let mut cals: Vec<Arc<Cal>> = self.book.cals.iter().filter(|c| c.kept.href != cal.kept.href).cloned().collect();
                cals.push(cal);
                self.book = Arc::new(Book::new(cals, &order));
                self.changed(cx);
                match result {
                    Ok(()) => {}
                    Err(dav::Error::Changed) => {
                        olo_kit::widgets::open_alert(
                            cx,
                            "Calendar",
                            "warning",
                            "That event was changed somewhere else meanwhile, so nothing was written over it. It shows now as it is there: make your change again if it’s still wanted.",
                            &["OK"],
                            |_, _| {},
                        );
                    }
                    Err(e) => {
                        tracing::warn!("calendar: a change couldn't be made: {e}");
                        olo_kit::widgets::open_alert(cx, "Calendar", "stop", &format!("The change couldn’t be made: {e}."), &["OK"], |_, _| {});
                    }
                }
                self.run_writes(cx);
            }
        }
        cx.invalidate();
    }

    fn command(&mut self, command: &str, cx: &mut Cx<Self>) -> bool {
        match command {
            "refresh" => self.sync_all(cx),
            "new-event" => {
                let start = self.new_start();
                self.open_new(start, false, cx);
            }
            "edit-event" => {
                let Some(key) = self.selected.clone() else { return false };
                self.open_event(&key, cx);
            }
            "delete-event" => {
                let Some((target, title)) = self.selected_target() else { return false };
                self.ask_delete(target, &title, cx);
            }
            // (An agent's: a new event at a time, as 2026-11-20T14:00.)
            c if c.starts_with("new-event:") => match c[10..].parse::<jiff::civil::DateTime>() {
                Ok(start) => self.open_new(start, false, cx),
                Err(_) => return false,
            },
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
