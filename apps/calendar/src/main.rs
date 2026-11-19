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
mod assistant;
mod edit;
mod editor;
mod glance;
mod layout;
mod model;
mod month;
mod preview;
mod service;
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
    /// The assistant shown beside the views.
    pub assistant: bool,
    /// The preview pane under the views (`preview.rs`).
    pub preview: bool,
    pub choices: Choices,
    /// The calendar the last new event went in (its address).
    pub calendar: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { mode: Mode::Month, week_start: None, sidebar: true, assistant: false, preview: true, choices: Choices::default(), calendar: None }
    }
}

pub(crate) fn settings_path() -> std::path::PathBuf {
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
    /// Show this occurrence (its event's address, its start on the
    /// person's clock): a reminder's Show.
    Reveal(String, jiff::civil::DateTime),
    /// The occurrence dragged among the hours, let go: it starts then
    /// (the person's clock), as long as it was.
    Move(Key, jiff::civil::DateTime),
    /// Its end dragged, let go: it ends then, starting when it did.
    Stretch(Key, jiff::civil::DateTime),
    /// An invitation answered: the event at this address of this
    /// calendar (its place in the book).
    Answer(usize, String, olo_calendar::event::Answer),
    /// What a search found (answer `n`).
    Found(u64, model::Found),
    /// The assistant's answer, as it comes.
    Assistant(assistant::Asked),
    /// One of what an answer proposes (which turn's, which of them):
    /// opened in the event's window, to save or not.
    Propose(usize, usize),
    /// One of what was found chosen (a click), or gone to (twice, Return).
    FoundChoose(usize),
    FoundOpen(usize),
}

/// How far a search looks, from today: back, and on (days).
const FIND_BACK: i32 = 366;
const FIND_ON: i32 = 2 * 366;
/// The most it lists.
pub const FOUND_MOST: usize = 500;

/// What takes a change back (Edit ▸ Undo and Redo: `olo_kit::undo`, as
/// every app's): the event's text as it was before, put back at its
/// address, or an event taken out again.
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    /// The calendar's address.
    calendar: String,
    back: Back,
}

#[derive(Debug, Clone, PartialEq)]
enum Back {
    /// The event at this address, as it was (its whole text).
    Put(String, String),
    /// The event at this address, gone again.
    Remove(String),
    /// The new event with this UID, gone again (its address isn't known
    /// until it's written).
    RemoveNew(String),
}

/// An event being dragged among the hours (the week's, the day's).
#[derive(Debug, Clone, PartialEq)]
pub struct Drag {
    pub key: Key,
    /// Where the press was, in the hours' drawing.
    pub from: olo_kit::Point<olo_kit::Pixels>,
    /// How far under the event's top the press was, and how long the
    /// event is (minutes).
    pub grab: f32,
    pub lasts: f32,
    /// Where it is, and where it would go: a day and a start (minutes of
    /// the day, by quarter hours). None until the pointer has gone a few
    /// points: a click selects, it doesn't move.
    pub was: (Date, i32),
    pub to: Option<(Date, i32)>,
    /// Its end is what's dragged (its bottom edge), on this day: `to` is
    /// then that day and when it would end (minutes of it, up to 1440).
    pub end: Option<Date>,
}

/// A question the assistant is answering.
pub struct Asking {
    n: u64,
    cancel: Arc<std::sync::atomic::AtomicBool>,
    pub question: String,
    /// The answer's words as far as they've come, and the model's name.
    pub so_far: String,
    pub model: String,
}

/// An event being dragged among the days (the month's, the week's
/// whole-day strip): it goes by whole days, at the time it has.
#[derive(Debug, Clone, PartialEq)]
pub struct DayDrag {
    pub key: Key,
    /// Where the press was, in the drawing.
    pub from: olo_kit::Point<olo_kit::Pixels>,
    /// How many days after its first the day pressed was.
    pub grab: i32,
    /// Its first day, how many days it's on, and the row it's drawn in
    /// (of its week's, of the strip's).
    pub was: Date,
    pub days: i32,
    pub lane: usize,
    /// The first day it would have. None until the pointer has gone a
    /// few points: a click selects, it doesn't move.
    pub to: Option<Date>,
}

/// `--show HREF START` among a launch's arguments: the occurrence to show.
fn to_reveal(args: &[String]) -> Option<(String, jiff::civil::DateTime)> {
    let at = args.iter().position(|a| a == "--show")?;
    Some((args.get(at + 1)?.clone(), args.get(at + 2)?.parse().ok()?))
}

/// A change waiting for the person's answer: which occurrences of a
/// repeating event it's for, or whether to delete at all.
enum Waiting {
    Save(editor::Saving),
    /// A repeating event's occurrence: only it, or all.
    DeleteWhich(editor::Target),
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
    /// The generation `shown` was worked out for.
    shown_for: u64,
    asking: u64,
    pub selected: Option<Key>,
    /// The preview pane, and what it was last told (`preview.rs`).
    pub preview: olo_kit::Child<preview::Pane>,
    pub preview_told: preview::Told,
    /// An occurrence to select once it's among those shown ([`Msg::Reveal`]).
    reveal: Option<(String, jiff::civil::DateTime)>,
    /// An event pressed among the hours, dragged to another time or day.
    pub drag: Option<Drag>,
    /// An event being dragged among the days.
    pub day_drag: Option<DayDrag>,
    /// The assistant: the field to ask in, the conversation, and the
    /// question being answered.
    pub question: olo_kit::Child<olo_kit::widgets::TextField>,
    pub turns: Vec<assistant::Turn>,
    pub answering: Option<Asking>,
    asks: u64,
    pub chat_scroll: ScrollHandle,
    /// The assistant's column and the calendars': beside the days while
    /// there's room, else giving way, in front alone when asked for (the
    /// kit's SidePane; View ▸ Show Assistant and Show Calendars).
    pub assistant_pane: olo_kit::widgets::SidePane,
    pub sidebar_pane: olo_kit::widgets::SidePane,
    /// The preview has room under the days, as laid out last (View ▸
    /// Show Preview is dimmed where it hasn't).
    pub preview_fits: bool,
    /// The month's weeks, scrolled in a window too short for them.
    pub month_scroll: ScrollHandle,
    /// What the event selected will be, once its move is written.
    moved_to: Option<Key>,
    /// The changes made here, to take back and make again.
    pub history: olo_kit::undo::History<Change>,
    /// Find: the search field, what's being looked for (empty: nothing),
    /// what was found for it, and which of that is chosen.
    pub search: olo_kit::Child<olo_kit::search::SearchField>,
    pub finding: String,
    pub found: Arc<Vec<Shown>>,
    /// How many there were in all (what's listed may be fewer), and
    /// which of the list is the first from today on.
    pub found_total: usize,
    found_upcoming: usize,
    pub found_version: u64,
    /// The search asked last (one overtaken gives up, and its answer is
    /// left), and what the answer showing was for.
    finding_n: Arc<std::sync::atomic::AtomicU64>,
    found_for: String,
    pub found_chosen: Option<usize>,
    pub found_scroll: ScrollHandle,
    /// What the change being made is called and is for, where it isn't
    /// what its job says (a drag's "Move"; an undo's or redo's own step).
    naming: Option<(String, olo_kit::undo::Purpose)>,
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
    /// What was being looked for (Find).
    #[serde(default)]
    finding: String,
    /// The assistant's conversation: each question, its answer, and
    /// whether that went wrong. (What an answer pointed to isn't kept:
    /// its words are.)
    #[serde(default)]
    turns: Vec<(String, String, bool)>,
}

impl Calendar {
    fn new(cx: &mut Cx<Self>) -> Calendar {
        let demo = sync::demo();
        let mut settings: Settings = if demo { Settings::default() } else { olo_kit::store::load(&settings_path()).unwrap_or_default() };
        let zone = TimeZone::system();
        let today = now(&zone).date();
        let mut cursor = today;
        let mut finding = String::new();
        let mut turns: Vec<assistant::Turn> = Vec::new();
        if let Some(saved) = olo_kit::restored_state().and_then(|s| serde_json::from_str::<Saved>(&s).ok()) {
            settings.mode = saved.mode;
            cursor = saved.cursor.parse().unwrap_or(today);
            finding = saved.finding;
            turns = saved.turns.into_iter().map(|(question, answer, failed)| assistant::Turn { question, answer, failed, ..Default::default() }).collect();
        }
        // The assistant's field: Return asks.
        let question = olo_kit::widgets::text_field(cx, |f| f.name("Question").hint("Ask about your calendars"));
        cx.subscribe(&question, |v: &mut Calendar, e: &olo_kit::widgets::TextFieldEvent, cx| {
            if matches!(e, olo_kit::widgets::TextFieldEvent::Entered) {
                v.ask_typed(cx);
            }
        });
        let clock = cx.messenger();
        olo_kit::settings::watch(move |s| {
            clock.send(Msg::Clock(s.clock.twenty_four_hour));
        });
        cx.work(move || sync::load(today), |(sources, cals)| Msg::Loaded(sources, cals.into_iter().map(Arc::new).collect()));
        // Find: what's typed is looked for as it's typed; Return goes to
        // the one chosen (the first, with none).
        let search = olo_kit::search::search_field(cx, "Search");
        cx.subscribe(&search, |v: &mut Calendar, e: &olo_kit::search::SearchEvent, cx| match e {
            olo_kit::search::SearchEvent::Changed => {
                let text = v.search.read(cx).text(cx).to_owned();
                v.find(text, cx);
            }
            olo_kit::search::SearchEvent::Entered => {
                // (With none chosen: the first to come, the list's top.)
                if !v.found.is_empty() {
                    cx.send(Msg::FoundOpen(v.found_chosen.unwrap_or(v.found_upcoming.min(v.found.len() - 1))));
                }
            }
        });
        // (A search under way before an upgrade: looked for again as the
        // calendars are read.)
        if !finding.is_empty() {
            let text = finding.clone();
            search.update(cx, |f, cx| f.set_text(text, cx));
        }
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
            shown_for: 0,
            asking: 0,
            selected: None,
            preview: preview::Pane::new(cx),
            preview_told: None,
            reveal: None,
            drag: None,
            day_drag: None,
            question,
            turns,
            answering: None,
            asks: 0,
            chat_scroll: ScrollHandle::new(),
            assistant_pane: olo_kit::widgets::SidePane::new(view::ASSISTANT, view::ASSISTANT_MIN).first(olo_kit::widgets::Front::Rest),
            sidebar_pane: olo_kit::widgets::SidePane::new(view::SIDEBAR, view::SIDEBAR).first(olo_kit::widgets::Front::Rest),
            preview_fits: true,
            month_scroll: ScrollHandle::new(),
            moved_to: None,
            history: Default::default(),
            search,
            finding,
            found: Arc::new(Vec::new()),
            found_total: 0,
            found_upcoming: 0,
            found_version: 0,
            finding_n: Default::default(),
            found_for: String::new(),
            found_chosen: None,
            found_scroll: ScrollHandle::new(),
            naming: None,
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
            v._accounts = olo_kit::watch::watch(olo_calendar::account::watched(), move |_| {
                changed.send(Msg::Accounts);
            });
        }
        v.wake_when_due(cx);
        // Started to show something (a reminder's Show, the window shut).
        if let Some((href, start)) = to_reveal(&std::env::args().skip(1).collect::<Vec<_>>()) {
            v.reveal = Some((href, start));
            v.go(start.date(), cx);
        }
        v
    }

    /// The occurrence asked for is selected, once it's among those
    /// shown; asked for and not there (deleted meanwhile, its calendar
    /// hidden), it's let go when its day has been worked out.
    fn try_reveal(&mut self) {
        let Some((href, start)) = &self.reveal else { return };
        if let Some(s) = self.shown.iter().find(|s| &*s.href == href.as_str() && s.start == *start) {
            self.selected = Some(s.key());
            self.reveal = None;
        } else if !self.loading && self.covered.is_some_and(|(from, to, generation)| generation == self.generation && from <= start.date() && start.date() <= to) && self.shown_for == self.generation {
            self.reveal = None;
        }
    }

    /// The hours, the first time they show, start a little before the
    /// working day (once they're laid out: there's nothing to scroll
    /// before); after that they're where the person left them.
    fn place_hours(&mut self, cx: &mut Cx<Self>) {
        if self.hours_placed || !matches!(self.settings.mode, Mode::Day | Mode::Week) {
            return;
        }
        self.hours_placed = true;
        let t = now(&self.zone);
        self.hours_scroll.set_offset_once_laid_out(cx, olo_kit::point(olo_kit::px(0.0), olo_kit::px(-week::starting_offset(t.hour()))));
        // The hour now too, with the half hour before it, where the hours
        // show too few for both (a window a third of a screen high opened
        // on 7 to 8 AM, now at 10:30 out of sight; Codex, 2026-11-19): it
        // moves only if that isn't in view already.
        let at = (t.hour() as f32 + t.minute() as f32 / 60.0) * week::HOUR;
        self.hours_scroll.reveal(cx, olo_kit::px(at - week::HOUR / 2.0), olo_kit::px(at + week::HOUR));
    }

    /// The calendars a new event can go in: those shown that may be
    /// written to, by their place in the book.
    fn writable(&self) -> Vec<(usize, String)> {
        self.book.cals.iter().enumerate().filter(|(_, c)| c.kept.writable && !self.settings.choices.hidden.contains(&c.kept.href)).map(|(i, c)| (i, c.kept.name.clone())).collect()
    }

    /// The editor, for a new event starting at `start`.
    pub fn open_new(&mut self, start: jiff::civil::DateTime, all_day: bool, cx: &mut Cx<Self>) {
        if let Some(opening) = self.opening_new(start, all_day, cx) {
            self.open_editor(opening, cx);
        }
    }

    /// What the editor is opened with for a new event starting at
    /// `start`; with no calendar to put one in, that's said, and None.
    fn opening_new(&mut self, start: jiff::civil::DateTime, all_day: bool, cx: &mut Cx<Self>) -> Option<editor::Opening> {
        let calendars = self.writable();
        if calendars.is_empty() {
            let why =
                if self.book.cals.is_empty() { "There's no calendar to put it in yet: connect an account in Preferences ▸ Integrations." } else { "None of the calendars shown can be written to." };
            olo_kit::widgets::open_alert(cx, "Calendar", "warning", why, &["OK"], |_, _| {});
            return None;
        }
        // The calendar the last one went in, if it's still there.
        let calendar = self.settings.calendar.as_ref().and_then(|href| calendars.iter().position(|(i, _)| self.book.cals[*i].kept.href == *href)).unwrap_or(0);
        let opening = editor::Opening {
            target: editor::Target::New,
            draft: edit::Draft::new(start, all_day),
            calendars,
            calendar,
            today: self.today,
            h24: self.h24,
            month_first: month_first(),
            clock: None,
            guests: Vec::new(),
            going: None,
            invite: Some(Vec::new()),
        };
        Some(opening)
    }

    /// When a new event starts by default: on the day shown, at the next
    /// whole hour if that's today, else at nine.
    pub fn new_start(&self) -> jiff::civil::DateTime {
        let now = now(&self.zone);
        if self.cursor == self.today && now.hour() < 23 { self.cursor.at(now.hour() + 1, 0, 0, 0) } else { self.cursor.at(9, 0, 0, 0) }
    }

    /// The editor, for the occurrence `key`.
    pub fn open_event(&mut self, key: &Key, cx: &mut Cx<Self>) {
        if let Some(opening) = self.opening_of(key) {
            self.open_editor(opening, cx);
        }
    }

    /// Whether the occurrence may be changed: its calendar can be
    /// written to.
    pub fn changeable(&self, key: &Key) -> bool {
        self.book.cals.get(key.0).is_some_and(|c| c.kept.writable)
    }

    /// The occurrence moved to start at `start` (the person's clock), as
    /// long as it was: a change as the editor's Save makes it (a
    /// repeating event's asks: only this one, or all).
    fn move_event(&mut self, key: &Key, start: jiff::civil::DateTime, cx: &mut Cx<Self>) {
        let Some(shown) = self.shown.iter().find(|s| s.key() == *key).cloned() else { return };
        let Some(opening) = self.opening_of(key) else { return };
        let Ok(by) = start.since(shown.start) else { return };
        if by.is_zero() {
            return;
        }
        let mut draft = opening.draft;
        let (Ok(new_start), Ok(new_end)) = (draft.start.checked_add(by), draft.end.checked_add(by)) else { return };
        (draft.start, draft.end) = (new_start, new_end);
        // (An event by itself is told from others by when it starts: the
        // one selected is this one, when it starts then.)
        self.moved_to = (self.selected.as_ref() == Some(key) && shown.id.is_none()).then(|| (key.0, key.1.clone(), start));
        self.naming = Some(("Move".to_owned(), olo_kit::undo::Purpose::Do));
        cx.send(Msg::Save(Box::new(editor::Saving { target: opening.target, draft, cal: shown.cal })));
    }

    /// Ask what's typed in the assistant's field.
    pub fn ask_typed(&mut self, cx: &mut Cx<Self>) {
        let q = self.question.read(cx).text().trim().to_owned();
        if !q.is_empty() {
            self.question.update(cx, |f, cx| f.set_text("", cx));
            self.ask(q, cx);
        }
    }

    /// Ask the assistant `question` about the calendars shown: it's
    /// given their events from a month back to four months on, worked
    /// out on its own worker.
    pub fn ask(&mut self, question: String, cx: &mut Cx<Self>) {
        self.stop_asking();
        if !self.settings.assistant {
            self.settings.assistant = true;
            self.save();
        }
        // (Asked in a window too narrow for it beside the days: in front.)
        if self.assistant_pane.alone() {
            self.assistant_pane.show_pane();
        }
        self.asks += 1;
        let n = self.asks;
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        // (Answers that came: not what went wrong, nor one stopped before
        // a word.)
        let history: Vec<olo_ai::Turn> = self.turns.iter().filter(|t| !t.failed && !t.answer.is_empty()).map(|t| olo_ai::Turn { question: t.question.clone(), answer: t.answer.clone() }).collect();
        let (book, zone, choices, h24) = (self.book.clone(), self.zone.clone(), self.settings.choices.clone(), self.h24);
        let me: Vec<String> = self.sources.iter().map(|s| s.account.address.clone()).collect();
        let now = now(&zone).datetime();
        let given = move || -> (String, assistant::Reader) {
            let today = now.date();
            let (from, to) = (layout::day_of(layout::day_number(today) - assistant::BACK_DAYS), layout::day_of(layout::day_number(today) + assistant::ON_DAYS));
            let shown = book.shown(from, to, &zone, &me, &choices);
            let name = |s: &Shown| book.cals.get(s.cal).map(|c| c.kept.name.clone()).unwrap_or_default();
            let (events, index) = assistant::listing(&shown, name);
            let calendars: Vec<String> = book.cals.iter().filter(|c| !choices.hidden.contains(&c.kept.href)).map(|c| c.kept.name.clone()).collect();
            let system = assistant::system(now, zone.iana_name().unwrap_or("the system's"), &calendars, &events, h24);
            (system, Box::new(move |answer| assistant::read(answer, &index, &shown, h24)))
        };
        let send = cx.messenger();
        assistant::ask(n, question.clone(), given, history, cancel.clone(), move |a| {
            send.send(Msg::Assistant(a));
        });
        self.answering = Some(Asking { n, cancel, question, so_far: String::new(), model: String::new() });
        self.chat_scroll.scroll_to_bottom(cx);
        cx.invalidate();
    }

    pub fn stop_asking(&mut self) {
        if let Some(a) = self.answering.take() {
            a.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            // (What was asked stays, with as much as came: none, if it
            // was stopped before anything did.)
            let answer = if a.so_far.is_empty() { String::new() } else { format!("{} …", a.so_far) };
            self.turns.push(assistant::Turn { question: a.question, answer, ..Default::default() });
        }
    }

    pub fn new_conversation(&mut self, cx: &mut Cx<Self>) {
        self.stop_asking();
        self.turns.clear();
        cx.invalidate();
    }

    /// One of what an answer proposed, opened in the event's window: a
    /// new event with its words and times, in the calendar new events
    /// go in; saved only if the person saves it.
    fn open_proposal(&mut self, turn: usize, i: usize, cx: &mut Cx<Self>) {
        let Some(p) = self.turns.get(turn).and_then(|t| t.proposals.get(i)).cloned() else { return };
        let Some(mut opening) = self.opening_new(p.start, p.all_day, cx) else { return };
        // (A whole day's ends, as people say it, on its last day.)
        let end = if p.all_day { p.end.date().yesterday().unwrap_or(p.end.date()).max(p.start.date()).at(0, 0, 0, 0) } else { p.end };
        let start = if p.all_day { p.start.date().at(0, 0, 0, 0) } else { p.start };
        opening.draft = edit::Draft { title: p.title, location: p.location, notes: p.notes, all_day: p.all_day, start, end, ..opening.draft };
        self.open_editor(opening, cx);
    }

    /// An invitation answered: the person's line among the event's guests
    /// says so, in the event and every occurrence of it (written to its
    /// server, which is what tells the organizer).
    fn answer_event(&mut self, cal: usize, href: String, answer: olo_calendar::event::Answer, cx: &mut Cx<Self>) {
        let me: Vec<String> = self.sources.iter().map(|s| s.account.address.clone()).collect();
        self.naming = Some((olo_kit::words::tr("Reply to").to_owned(), olo_kit::undo::Purpose::Do));
        self.write(cal, sync::Job::Change(href, Box::new(move |c| olo_calendar::event::answer(c, &me, answer))), cx);
    }

    /// The occurrence selected, if it's an invitation of the person's:
    /// its calendar, its address, and what they've answered.
    fn selected_invitation(&self) -> Option<(usize, String, olo_calendar::event::Answer)> {
        let key = self.selected.as_ref()?;
        let shown = self.shown.iter().find(|s| s.key() == *key)?;
        let going = shown.going.filter(|_| self.changeable(key))?;
        Some((shown.cal, shown.href.to_string(), going))
    }

    /// An event pressed among the days, on `pressed`, where it's drawn in
    /// row `lane`: it may be dragged to other days (one of a calendar
    /// that can be written to).
    pub fn day_drag_start(&mut self, key: &Key, from: olo_kit::Point<olo_kit::Pixels>, pressed: Date, lane: usize) {
        let Some(s) = self.shown.iter().find(|s| s.key() == *key) else { return };
        if !self.changeable(key) {
            return;
        }
        let (first, last) = s.days();
        let days = layout::day_number(last) - layout::day_number(first) + 1;
        let grab = (layout::day_number(pressed) - layout::day_number(first)).clamp(0, days - 1);
        self.day_drag = Some(DayDrag { key: key.clone(), from, grab, was: first, days, lane, to: None });
    }

    /// The pointer moved with it pressed, to `at` in the drawing, over
    /// `day`: where it would go, drawn again only when that changes.
    pub fn day_drag_over(&mut self, at: olo_kit::Point<olo_kit::Pixels>, day: Date, cx: &mut Cx<Self>) {
        let Some(d) = &mut self.day_drag else { return };
        let far = f32::from(at.x - d.from.x).abs().max(f32::from(at.y - d.from.y).abs()) >= week::DRAG_AFTER;
        if d.to.is_none() && !far {
            return;
        }
        let to = layout::day_of(layout::day_number(day) - d.grab);
        if d.to != Some(to) {
            d.to = Some(to);
            cx.invalidate();
        }
    }

    /// The button let go: dragged to other days, it goes there, at the
    /// time it has (a change as a move among the hours is).
    pub fn day_drag_drop(&mut self, cx: &mut Cx<Self>) {
        let Some(d) = self.day_drag.take() else { return };
        let Some(to) = d.to else { return };
        cx.invalidate();
        let by = layout::day_number(to) - layout::day_number(d.was);
        let start = self.shown.iter().find(|s| s.key() == d.key).and_then(|s| s.start.checked_add(jiff::Span::new().days(by as i64)).ok());
        if let (Some(start), true) = (start, by != 0) {
            cx.send(Msg::Move(d.key, start));
        }
    }

    /// The occurrence made to end at `end` (the person's clock), starting
    /// when it did: a change as the editor's Save makes it, as a move is.
    fn stretch_event(&mut self, key: &Key, end: jiff::civil::DateTime, cx: &mut Cx<Self>) {
        let Some(shown) = self.shown.iter().find(|s| s.key() == *key).cloned() else { return };
        let Some(opening) = self.opening_of(key) else { return };
        let Ok(by) = end.since(shown.end) else { return };
        if by.is_zero() || end <= shown.start {
            return;
        }
        let mut draft = opening.draft;
        let Ok(new_end) = draft.end.checked_add(by) else { return };
        draft.end = new_end;
        self.naming = Some(("Resize".to_owned(), olo_kit::undo::Purpose::Do));
        cx.send(Msg::Save(Box::new(editor::Saving { target: opening.target, draft, cal: shown.cal })));
    }

    /// What the editor is opened with for the occurrence `key`: what it
    /// is in its calendar, and its words and times as a draft.
    pub(crate) fn opening_of(&self, key: &Key) -> Option<editor::Opening> {
        let shown = self.shown.iter().find(|s| s.key() == *key).cloned()?;
        let stored = self.book.stored(&shown)?;
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
        // Who's asked, and what each has answered: who asks first, then
        // the person, then the others by name.
        let is_me = |address: &str| self.sources.iter().any(|s| s.account.address.eq_ignore_ascii_case(address));
        let asks = |address: &str| event.organizer.as_ref().is_some_and(|o| o.address.eq_ignore_ascii_case(address));
        let named = |p: &olo_calendar::event::Person| if p.name.trim().is_empty() { p.address.clone() } else { p.name.trim().to_owned() };
        let mut guests: Vec<editor::Guest> =
            event.attendees.iter().map(|p| editor::Guest { name: named(p), answer: Some(p.answer), organizer: asks(&p.address), optional: p.optional, me: is_me(&p.address) }).collect();
        if let Some(o) = event.organizer.as_ref().filter(|o| !event.attendees.is_empty() && !event.attendees.iter().any(|p| p.address.eq_ignore_ascii_case(&o.address))) {
            guests.push(editor::Guest { name: named(o), answer: None, organizer: true, optional: false, me: is_me(&o.address) });
        }
        guests.sort_by_key(|g| (!g.organizer, !g.me, g.name.to_lowercase()));
        let going = shown.going.filter(|_| self.changeable(key));
        // Who's invited is the person's to say where nobody else
        // organizes it (and it can be written): the guests there are,
        // by address, without themselves.
        let mine = event.organizer.as_ref().is_none_or(|o| is_me(&o.address));
        let invite = (mine && self.changeable(key)).then(|| event.attendees.iter().filter(|p| !is_me(&p.address)).map(|p| p.address.clone()).collect());
        Some(editor::Opening { target, draft, calendars: vec![(shown.cal, name)], calendar: 0, today: self.today, h24: self.h24, month_first: month_first(), clock, guests, going, invite })
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
        // What would take it back: the event as it is now, before it's
        // written, named for what's done to it ("Delete “Dentist”").
        let was = |href: &String| c.kept.items.get(href).map(|item| item.ics.clone());
        let title = |href: &String| c.events.iter().find(|s| s.href == *href).map(|s| s.event.summary.trim().to_owned()).unwrap_or_default();
        let (verb, about) = match &job {
            sync::Job::Create(event) => ("New Event", event.summary.trim().to_owned()),
            sync::Job::Remove(href) => ("Delete", title(href)),
            sync::Job::Change(href, _) | sync::Job::PutBack(href, _) => ("Change", title(href)),
        };
        let back = match &job {
            sync::Job::Create(event) => Some(Back::RemoveNew(event.uid.clone())),
            sync::Job::Change(href, _) | sync::Job::Remove(href) => was(href).map(|ics| Back::Put(href.clone(), ics)),
            sync::Job::PutBack(href, _) => Some(was(href).map_or_else(|| Back::Remove(href.clone()), |ics| Back::Put(href.clone(), ics))),
        };
        let (name, purpose) = match self.naming.take() {
            // (A step undone or redone keeps its name.)
            Some((name, purpose)) if purpose != olo_kit::undo::Purpose::Do => (name, purpose),
            named => {
                let verb = named.map_or(verb.to_owned(), |(verb, _)| verb);
                (if about.is_empty() { verb } else { format!("{verb} “{about}”") }, olo_kit::undo::Purpose::Do)
            }
        };
        if let Some(back) = back {
            self.history.record(purpose, name, Change { calendar: c.kept.href.clone(), back });
        }
        self.writes.push_back((c.kept.account.clone(), c.kept.href.clone(), job));
        self.run_writes(cx);
    }

    /// The last change taken back (`redo`: one taken back, made again):
    /// what the event was is put where it was, or one that was made is
    /// taken out. Itself a change, written as any is, and recorded as
    /// the step that goes the other way. Not while a change is still
    /// being written (what it would put back is from before that one).
    fn undo(&mut self, redo: bool, cx: &mut Cx<Self>) {
        if self.writing > 0 || !self.writes.is_empty() {
            return;
        }
        let Some(step) = (if redo { self.history.take_redo() } else { self.history.take_undo() }) else { return };
        let Some(cal) = self.book.cals.iter().position(|c| c.kept.href == step.change.calendar) else { return };
        let job = match step.change.back {
            Back::Put(href, ics) => sync::Job::PutBack(href, ics),
            Back::Remove(href) => sync::Job::Remove(href),
            Back::RemoveNew(uid) => match self.book.cals[cal].events.iter().find(|s| s.event.uid == uid) {
                Some(stored) => sync::Job::Remove(stored.href.clone()),
                None => return,
            },
        };
        self.selected = None;
        self.naming = Some((step.name, if redo { olo_kit::undo::Purpose::Redo } else { olo_kit::undo::Purpose::Undo }));
        self.write(cal, job, cx);
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
        let mut draft = saving.draft;
        // Who invites: the address its calendar's account is signed in with.
        let cal = match &saving.target {
            editor::Target::New => saving.cal,
            editor::Target::Existing { cal, .. } => *cal,
        };
        if let Some(invite) = draft.invite.as_mut() {
            let account = self.book.cals.get(cal).map(|c| c.kept.account.clone()).unwrap_or_default();
            invite.organizer = self.sources.iter().find(|s| s.account.id() == account).map(|s| s.account.address.clone()).unwrap_or_default();
            invite.guests.retain(|g| !g.eq_ignore_ascii_case(&invite.organizer));
        }
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

    /// Delete what `target` names: a repeating event's occurrence once
    /// the person has said which of it (only it, or all); anything else
    /// at once, with no asking (Edit ▸ Undo brings it back).
    fn ask_delete(&mut self, target: editor::Target, title: &str, cx: &mut Cx<Self>) {
        let editor::Target::Existing { id, repeats, .. } = &target else { return };
        let asked = cx.msg_with(|choice: &Option<usize>| Msg::Asked(*choice));
        if *repeats && id.is_some() {
            self.waiting = Some(Waiting::DeleteWhich(target));
            olo_kit::widgets::open_alert(
                cx,
                "Calendar",
                "warning",
                &olo_kit::tr!("“{}” repeats. Delete only this one, or every one of them?", title),
                &["Cancel", "All of Them", "Only This One"],
                asked,
            );
        } else {
            self.delete_event(target, false, cx);
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
        self.look_for(cx);
    }

    /// Find: `text` looked for from now on (nothing: the search is over).
    pub fn find(&mut self, text: String, cx: &mut Cx<Self>) {
        let text = text.trim().to_owned();
        if text == self.finding {
            return;
        }
        self.finding = text;
        self.found_chosen = None;
        if self.finding.is_empty() {
            // (An answer still on its way is for nothing now.)
            self.finding_n.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.found = Arc::new(Vec::new());
            (self.found_total, self.found_for) = (0, String::new());
            self.found_version += 1;
            cx.invalidate();
        } else {
            self.look_for(cx);
        }
    }

    /// What's being looked for, looked for (on a worker: every calendar
    /// shown, from a year back to two years on), again whenever what's
    /// kept or shown changes.
    fn look_for(&mut self, cx: &mut Cx<Self>) {
        if self.finding.is_empty() {
            return;
        }
        use std::sync::atomic::Ordering::Relaxed;
        let n = self.finding_n.fetch_add(1, Relaxed) + 1;
        let (latest, today) = (self.finding_n.clone(), self.today);
        let (book, zone, choices, text) = (self.book.clone(), self.zone.clone(), self.settings.choices.clone(), self.finding.clone());
        let me: Vec<String> = self.sources.iter().map(|s| s.account.address.clone()).collect();
        let (from, to) = (layout::day_of(layout::day_number(self.today) - FIND_BACK), layout::day_of(layout::day_number(self.today) + FIND_ON));
        cx.work(
            move || {
                let query = olo_kit::search::Query::parse(&text, &[]);
                book.find(&query, (from, to), today, &zone, &me, &choices, FOUND_MOST, || latest.load(Relaxed) != n)
            },
            move |found| Msg::Found(n, found),
        );
    }

    /// The next of what was found chosen (or the one before, `up`), from
    /// the keyboard; with none chosen, the first to come (the last there
    /// was, where nothing is to come); brought into view.
    pub fn step_found(&mut self, up: bool, cx: &mut Cx<Self>) {
        if self.found.is_empty() {
            return;
        }
        let last = self.found.len() - 1;
        let to = match self.found_chosen {
            None => self.found_upcoming.min(last),
            Some(i) if up => i.saturating_sub(1),
            Some(i) => (i + 1).min(last),
        };
        if self.found_chosen != Some(to) {
            self.found_chosen = Some(to);
            cx.invalidate();
        }
        if let Some((top, bottom)) = agenda::found_span(self, to) {
            self.found_scroll.reveal_span(olo_kit::px(top), olo_kit::px(bottom));
            cx.invalidate();
        }
    }

    /// One of what was found gone to: the search is over, and the view
    /// goes to its day with it chosen.
    fn open_found(&mut self, i: usize, cx: &mut Cx<Self>) {
        let Some(s) = self.found.get(i).cloned() else { return };
        self.search.update(cx, |f, cx| f.set_text("", cx));
        self.find(String::new(), cx);
        cx.send(Msg::Reveal(s.href.to_string(), s.start));
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
                    dav::Error::SignIn => olo_kit::tr!("{} needs signing in again", s.account.address),
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
        use olo_calendar::event::Answer;
        use olo_kit::Menu;
        let mode = self.settings.mode;
        let start = self.week_start();
        let going = self.selected_invitation().map(|(.., going)| going);
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
            self.history
                .edit_items(Menu::new("Edit"))
                .separator()
                // (Find… alone: ⌘F puts the caret in the search field,
                // and what's found is a list, with no next to go to.)
                .item("Find…", "find")
                .key("f")
                .separator()
                .item("Edit Event…", "edit-event")
                .key("e")
                .enabled(self.selected.is_some())
                .item("Delete Event", "delete-event")
                .key("backspace")
                .enabled(self.selected.is_some())
                .separator()
                // An invitation selected: what the person answers.
                .submenu(
                    Menu::new("Reply")
                        .item("Yes", "answer:yes")
                        .checked(going == Some(Answer::Yes))
                        .enabled(going.is_some())
                        .item("Maybe", "answer:maybe")
                        .checked(going == Some(Answer::Maybe))
                        .enabled(going.is_some())
                        .item("No", "answer:no")
                        .checked(going == Some(Answer::No))
                        .enabled(going.is_some()),
                ),
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
                .checked(self.sidebar_pane.showing())
                .item("Show Assistant", "assistant")
                .checked(self.assistant_pane.showing())
                .item("Show Preview", "preview")
                .checked(self.settings.preview)
                .enabled(self.preview_fits)
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
                self.shown_for = self.generation;
                // (What was selected and is no longer there isn't selected.)
                // (One moved while selected: it's selected where it went.)
                if let Some(key) = &self.selected
                    && !self.shown.iter().any(|s| s.key() == *key)
                {
                    self.selected = self.moved_to.take().filter(|to| self.shown.iter().any(|s| s.key() == *to));
                }
                self.try_reveal();
            }
            Msg::Move(key, start) => return self.move_event(&key, start, cx),
            Msg::Stretch(key, end) => return self.stretch_event(&key, end, cx),
            Msg::Answer(cal, href, answer) => return self.answer_event(cal, href, answer, cx),
            Msg::Assistant(a) => match a {
                assistant::Asked::Model(n, model) => match &mut self.answering {
                    Some(asking) if asking.n == n => asking.model = model,
                    _ => return,
                },
                assistant::Asked::SoFar(n, words) => match &mut self.answering {
                    Some(asking) if asking.n == n => {
                        asking.so_far = words;
                        self.chat_scroll.scroll_to_bottom(cx);
                    }
                    _ => return,
                },
                assistant::Asked::Done(n, result) => {
                    let Some(asking) = self.answering.take_if(|a| a.n == n) else { return };
                    let turn = match result {
                        Ok((answer, events, proposals)) => assistant::Turn { question: asking.question, answer, events, proposals, failed: false },
                        Err(e) => assistant::Turn { question: asking.question, answer: e, failed: true, ..Default::default() },
                    };
                    self.turns.push(turn);
                    self.chat_scroll.scroll_to_bottom(cx);
                }
            },
            Msg::Propose(turn, i) => return self.open_proposal(turn, i, cx),
            Msg::Found(n, found) => {
                if n != self.finding_n.load(std::sync::atomic::Ordering::Relaxed) {
                    return;
                }
                // (The one chosen, if it's still among them.)
                let chosen = self.found_chosen.and_then(|i| self.found.get(i)).map(|s| s.key());
                self.found_chosen = chosen.and_then(|key| found.list.iter().position(|s| s.key() == key));
                (self.found, self.found_total, self.found_upcoming) = (found.list, found.total, found.upcoming);
                self.found_version += 1;
                // What's found for other words starts at today: the first
                // to come at the top, what's been above it. (The same
                // words found again, as the calendars change, stay put.)
                if self.found_for != self.finding {
                    self.found_for = self.finding.clone();
                    let top = match agenda::found_span(self, found.upcoming) {
                        Some((top, _)) => top,
                        None => f32::MAX,
                    };
                    self.found_scroll.set_offset_once_laid_out(cx, olo_kit::point(olo_kit::px(0.0), olo_kit::px(-top)));
                }
            }
            Msg::FoundChoose(i) => {
                if self.found_chosen == Some(i) {
                    return;
                }
                self.found_chosen = Some(i);
            }
            Msg::FoundOpen(i) => return self.open_found(i, cx),
            Msg::Reveal(href, start) => {
                // (From the assistant in front of the days: the days.)
                self.assistant_pane.show_rest();
                self.reveal = Some((href, start));
                self.go(start.date(), cx);
                self.try_reveal();
            }
            Msg::Clock(h24) => {
                if self.h24 == h24 {
                    return;
                }
                self.h24 = h24;
            }
            Msg::Mode(mode) => return self.set_mode(mode, cx),
            Msg::Go(day) => {
                // (A day picked in the small month in front of the days:
                // the days.)
                self.sidebar_pane.show_rest();
                return self.go(day, cx);
            }
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
                    let title = if saving.draft.title.trim().is_empty() { olo_kit::words::tr("This event").to_owned() } else { format!("“{}”", saving.draft.title.trim()) };
                    self.waiting = Some(Waiting::Save(*saving));
                    let asked = cx.msg_with(|choice: &Option<usize>| Msg::Asked(*choice));
                    olo_kit::widgets::open_alert(
                        cx,
                        "Calendar",
                        "warning",
                        &olo_kit::tr!("{} repeats. Change only this one, or every one of them?", title),
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
                let title = if title.trim().is_empty() { olo_kit::words::tr("this event").to_owned() } else { title };
                self.ask_delete(*target, &title, cx);
            }
            Msg::Asked(choice) => match (self.waiting.take(), choice) {
                // (Only This One is Return's: the smaller change.)
                (Some(Waiting::Save(saving)), Some(2)) => self.save_event(saving, true, cx),
                (Some(Waiting::Save(saving)), Some(1)) => self.save_event(saving, false, cx),
                (Some(Waiting::DeleteWhich(target)), Some(2)) => self.delete_event(target, true, cx),
                (Some(Waiting::DeleteWhich(target)), Some(1)) => self.delete_event(target, false, cx),
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
                // (A change that didn't go through: what the steps would
                // put back is no longer what's there.)
                if result.is_err() {
                    self.history.clear();
                }
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
                        olo_kit::widgets::open_alert(cx, "Calendar", "stop", &olo_kit::tr!("The change couldn’t be made: {}.", e), &["OK"], |_, _| {});
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
            // (⌘F: the caret in the search field.)
            "find" => self.search.update(cx, |f, cx| f.focus(cx)),
            "undo" => self.undo(false, cx),
            "redo" => self.undo(true, cx),
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
                // (Too narrow for it beside the days: in front of them,
                // or back to them; shown so, it's on.)
                if self.sidebar_pane.alone() {
                    self.sidebar_pane.toggle();
                    // (One in front at a time: the assistant goes back.)
                    self.assistant_pane.show_rest();
                    if self.sidebar_pane.showing() && !self.settings.sidebar {
                        self.settings.sidebar = true;
                        self.save();
                    }
                } else {
                    self.settings.sidebar = !self.settings.sidebar;
                    self.save();
                }
                cx.invalidate();
            }
            "preview" => {
                self.settings.preview = !self.settings.preview;
                self.save();
                cx.invalidate();
            }
            "assistant" => {
                if self.assistant_pane.alone() {
                    self.assistant_pane.toggle();
                    self.sidebar_pane.show_rest();
                    if self.assistant_pane.showing() && !self.settings.assistant {
                        self.settings.assistant = true;
                        self.save();
                    }
                } else {
                    self.settings.assistant = !self.settings.assistant;
                    self.save();
                }
                if self.assistant_pane.showing() || (!self.assistant_pane.alone() && self.settings.assistant) {
                    self.question.focus(cx);
                }
                cx.invalidate();
            }
            // (An agent's way to ask: `ask:What's on today?`.)
            c if c.starts_with("ask:") => {
                let q = c[4..].trim().to_owned();
                if q.is_empty() {
                    return false;
                }
                self.ask(q, cx);
            }
            "close" | "quit" => olo_kit::quit(),
            c if c.starts_with("answer:") => {
                use olo_calendar::event::Answer;
                let answer = match &c[7..] {
                    "yes" => Answer::Yes,
                    "maybe" => Answer::Maybe,
                    "no" => Answer::No,
                    _ => return false,
                };
                let Some((cal, href, going)) = self.selected_invitation() else { return false };
                if going != answer {
                    self.answer_event(cal, href, answer, cx);
                }
            }
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
        let turns = self.turns.iter().map(|t| (t.question.clone(), t.answer.clone(), t.failed)).collect();
        serde_json::to_string(&Saved { cursor: self.cursor.to_string(), mode: self.settings.mode, finding: self.finding.clone(), turns }).ok()
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
                // (An invitation: what the person has answered.)
                "going": s.going.map(|a| match a {
                    olo_calendar::event::Answer::NotYet => "not answered",
                    olo_calendar::event::Answer::Yes => "yes",
                    olo_calendar::event::Answer::No => "no",
                    olo_calendar::event::Answer::Maybe => "maybe",
                }),
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
    // The reminders service (the session's; no window).
    // Deskbar's widgets (no window).
    if args.first().is_some_and(|a| a == "--glance") {
        return glance::serve();
    }
    if args.first().is_some_and(|a| a == "--service") {
        let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
        let _ = tracing_subscriber::fmt().with_env_filter(filter).with_writer(std::io::stderr).try_init();
        return service::run();
    }
    if instance::forward(APP_ID, &args) {
        return std::process::ExitCode::SUCCESS;
    }
    olo_kit::run(APP_ID, |app| {
        let window = olo_kit::open_window(WindowOptions::new("Calendar", 1080.0, 720.0).min_size(view::MIN_WIDTH, view::MIN_HEIGHT).keep_size("main"), Calendar::new);
        instance::listen(APP_ID, olo_kit::proxy());
        olo_kit::message_handler(app, move |message, _| {
            if let Ok(launch) = message.downcast::<Launch>() {
                olo_kit::raise(Some(&window));
                if let Some((href, start)) = to_reveal(&launch.args) {
                    window.messenger().send(Msg::Reveal(href, start));
                }
            }
        });
    });
    std::process::ExitCode::SUCCESS
}
