//! To-Do: lists of things to do (Ben, 2026-11-18: "a beautiful olo native
//! todo app that supports reminders / due dates / simple kanban style
//! board / attaching links, images, etc. / beautiful styled deskbar
//! widgets").
//!
//! - view.rs: the window (the places and lists down the left; the place
//!   chosen as a list or a board; the to-do chosen beside it).
//! - list.rs: a place's to-dos down a list, under their headings.
//! - board.rs: a list's sections side by side, its to-dos cards to drag
//!   between them.
//! - details.rs: the to-do chosen: its title, due, reminder, notes, list
//!   and section, attachments.
//! - service.rs: reminders, as notifications, from the session's
//!   service (no window); glance.rs: Deskbar's widgets.
//! - words.rs: days and times in words.
//!
//! The engine is crates/todo (HACKING.md, "To-Do"): a list a folder, a
//! to-do an iCalendar file.
//!
//! For agents and scripts, with no window: `olo-todo --print [PLACE]`
//! (today, upcoming, all, done, or a list's name) prints to-dos as JSON;
//! `olo-todo --add "Call Ann tomorrow at 3pm" [--list NAME]` adds one;
//! `olo-todo --done UID` checks one off. `olo-todo --show UID` opens the
//! window on one.

mod board;
mod details;
mod glance;
mod list;
mod marks;
mod service;
mod view;
mod words;

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use jiff::{Zoned, tz::TimeZone};
use olo_kit::{
    Child, Cx, ScrollHandle, View, WindowHandle, WindowOptions,
    due::Due,
    instance::{self, Launch},
    undo::{History, Purpose},
    widgets::{SidePane, TextField, TextFieldEvent, text_field},
};
use olo_theme::Hue;
use olo_todo::{
    Book, Item, List, ToDo,
    quick::{self, Typed},
    store,
    view::{self as places, Place, Row},
};
use serde::{Deserialize, Serialize};

pub const APP_ID: &str = "olo.todo";

/// How long a to-do just checked off stays where it was (shown done, to
/// be unchecked at once if that was a slip) before it goes.
const LINGER: Duration = Duration::from_millis(1500);
/// What's typed in the chosen to-do's fields is kept this long after the
/// last key (and at once on Return, or leaving it).
const KEEP_TYPED: Duration = Duration::from_millis(700);

/// How a list is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Layout {
    #[default]
    List,
    Board,
}

/// Where in To-Do, as kept.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", tag = "is", content = "id")]
pub enum Kept {
    #[default]
    Today,
    Upcoming,
    All,
    Done,
    List(String),
}

impl Kept {
    pub fn place(&self) -> Place {
        match self {
            Kept::Today => Place::Today,
            Kept::Upcoming => Place::Upcoming,
            Kept::All => Place::All,
            Kept::Done => Place::Done,
            Kept::List(id) => Place::List(id.clone()),
        }
    }

    pub fn of(place: &Place) -> Kept {
        match place {
            Place::Today => Kept::Today,
            Place::Upcoming => Kept::Upcoming,
            Place::All => Kept::All,
            Place::Done => Kept::Done,
            Place::List(id) => Kept::List(id.clone()),
        }
    }
}

/// What To-Do keeps (`$XDG_CONFIG_HOME/olo/todo.json`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub place: Kept,
    /// Each list's layout, by its id (a list not here: a list).
    pub layouts: HashMap<String, Layout>,
    /// The lists in the person's order (by id; any not here after them).
    pub order: Vec<String>,
    /// A list's done to-dos shown in its own place too: its board's last
    /// lane, the last group down its list.
    pub show_done: bool,
    pub sidebar: bool,
    pub sidebar_width: Option<f32>,
    pub details: bool,
    pub details_width: Option<f32>,
    /// The list the last new to-do went in, from a place of no list's own.
    pub last_list: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { place: Kept::Today, layouts: HashMap::new(), order: Vec::new(), show_done: true, sidebar: true, sidebar_width: None, details: true, details_width: None, last_list: None }
    }
}

pub(crate) fn settings_path() -> PathBuf {
    olo_kit::config_dir().join("todo.json")
}

/// Whether the person's region writes the month before the day (3/4 is
/// March 4th).
pub fn month_first() -> bool {
    let locale = ["LC_ALL", "LC_TIME", "LANG"].iter().find_map(|v| std::env::var(v).ok().filter(|s| !s.is_empty())).unwrap_or_default();
    let region = locale.split(['.', '@']).next().unwrap_or_default().rsplit('_').next().unwrap_or_default().to_ascii_uppercase();
    matches!(region.as_str(), "US" | "PH" | "FM" | "PW" | "MH")
}

/// Now, on the person's clock (`OLO_TODO_NOW=2026-11-19T10:30`: a
/// test's own now, so its pictures are the same every day).
pub fn now(zone: &TimeZone) -> Zoned {
    if let Ok(fixed) = std::env::var("OLO_TODO_NOW")
        && let Ok(at) = fixed.parse::<jiff::civil::DateTime>()
        && let Ok(z) = at.to_zoned(zone.clone())
    {
        return z;
    }
    Zoned::now().with_time_zone(zone.clone())
}

/// How many to-dos each place has to do, and how many are done.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Counts {
    pub today: usize,
    pub upcoming: usize,
    pub all: usize,
    pub done: usize,
    pub lists: HashMap<String, usize>,
}

impl Counts {
    /// Counted in one pass of the book.
    pub fn of(book: &Book, now: &Zoned) -> Counts {
        let (zone, today) = (now.time_zone(), now.date());
        let mut c = Counts::default();
        for item in &book.items {
            if !item.todo.open() {
                c.done += 1;
                continue;
            }
            c.all += 1;
            *c.lists.entry(item.list.clone()).or_default() += 1;
            match item.todo.due.as_ref().map(|d| places::due_day(d, zone)) {
                Some(day) if day <= today => c.today += 1,
                Some(_) => c.upcoming += 1,
                None => {}
            }
        }
        c
    }

    pub fn at(&self, place: &Place) -> usize {
        match place {
            Place::Today => self.today,
            Place::Upcoming => self.upcoming,
            Place::All => self.all,
            Place::Done => 0,
            Place::List(id) => self.lists.get(id).copied().unwrap_or(0),
        }
    }
}

/// Where the last `n` words of `text` are (bytes): from the start of
/// the first of them to the end of the last.
fn last_words(text: &str, n: usize) -> Option<std::ops::Range<usize>> {
    let mut starts = Vec::new();
    let mut in_word = false;
    for (i, c) in text.char_indices() {
        if c.is_whitespace() {
            in_word = false;
        } else if !in_word {
            in_word = true;
            starts.push(i);
        }
    }
    let first = *starts.get(starts.len().checked_sub(n.max(1))?)?;
    Some(first..text.trim_end().len())
}

/// What a change wrote, kept to take it back (Edit ▸ Undo).
#[derive(Debug, Clone, PartialEq)]
pub enum Back {
    /// A file put back as it was (its path, its text).
    Put(PathBuf, String),
    /// A file made, taken away again.
    Remove(PathBuf),
    /// A to-do moved to another list: moved back (from, to the list).
    Move(PathBuf, String),
    /// A list's name, colour and sections as they were.
    List(List),
    /// A new list, taken away again.
    RemoveList(String),
    /// A list put in the Trash, brought back (its id, where it went).
    Untrash(String, PathBuf),
}

/// A change to the files, made on a worker.
#[derive(Debug, Clone)]
pub enum Job {
    Create(String, Box<ToDo>),
    Change(Box<Item>, Box<ToDo>),
    Remove(Box<Item>),
    Move(Box<Item>, String, Box<ToDo>),
    /// Files attached to a to-do (copied into its list's keeping), or
    /// bytes as a file called so (a picture pasted).
    Attach(Box<Item>, Vec<PathBuf>),
    AttachBytes(Box<Item>, String, Vec<u8>, Option<String>),
    NewList(String, Hue),
    ChangeList(Option<Box<List>>, Box<List>),
    TrashList(String),
    /// Undo's: a step taken back.
    Back(Back),
}

/// What a job did: the paths it changed (read again at once), and the
/// steps that take it back.
#[derive(Debug, Clone, Default)]
pub struct Did {
    pub paths: Vec<PathBuf>,
    pub back: Vec<Back>,
    /// A list made: chosen.
    pub new_list: Option<String>,
}

fn run(root: &std::path::Path, job: Job) -> Result<Did, String> {
    let e = |e: std::io::Error| e.to_string();
    let list_dir = |id: &str| root.join(id);
    Ok(match job {
        Job::Create(list, todo) => {
            let path = store::create(root, &list, &todo).map_err(e)?;
            Did { paths: vec![path.clone()], back: vec![Back::Remove(path)], new_list: None }
        }
        Job::Change(item, todo) => {
            let was = store::change(&item, &todo).map_err(e)?;
            Did { paths: vec![item.file.clone()], back: vec![Back::Put(item.file.clone(), was)], new_list: None }
        }
        Job::Remove(item) => {
            let was = store::remove(&item).map_err(e)?;
            Did { paths: vec![item.file.clone()], back: vec![Back::Put(item.file.clone(), was)], new_list: None }
        }
        Job::Move(item, list, todo) => {
            let to = store::move_to(root, &item, &list).map_err(e)?;
            let mut back = vec![Back::Move(to.clone(), item.list.clone())];
            // (Its section, its place, as it goes.)
            if let Some(moved) = std::fs::read_to_string(&to).ok().and_then(|t| olo_todo::item::read_file(&t)).map(|(t, _)| t)
                && (moved.section != todo.section || moved.order != todo.order)
            {
                let at = Item { list: list.clone(), file: to.clone(), changed: None, todo: moved };
                let was = store::change(&at, &todo).map_err(e)?;
                back.insert(0, Back::Put(to.clone(), was));
            }
            Did { paths: vec![item.file.clone(), to, root.to_path_buf()], back, new_list: None }
        }
        Job::Attach(item, files) => {
            let mut todo = item.todo.clone();
            for f in &files {
                let mime = olo_xdg::open::Target::Path(f.clone()).mime();
                todo.attachments.push(store::attach(root, &item.list, &item.todo.uid, f, Some(&mime)).map_err(e)?);
            }
            let was = store::change(&item, &todo).map_err(e)?;
            Did { paths: vec![item.file.clone()], back: vec![Back::Put(item.file.clone(), was)], new_list: None }
        }
        Job::AttachBytes(item, name, bytes, mime) => {
            let mut todo = item.todo.clone();
            todo.attachments.push(store::attach_bytes(root, &item.list, &item.todo.uid, &name, &bytes, mime.as_deref()).map_err(e)?);
            let was = store::change(&item, &todo).map_err(e)?;
            Did { paths: vec![item.file.clone()], back: vec![Back::Put(item.file.clone(), was)], new_list: None }
        }
        Job::NewList(name, hue) => {
            let list = store::create_list(root, &name, hue).map_err(e)?;
            Did { paths: vec![root.to_path_buf()], back: vec![Back::RemoveList(list.id.clone())], new_list: Some(list.id) }
        }
        Job::ChangeList(was, list) => {
            store::change_list(root, was.as_deref(), &list).map_err(e)?;
            let back = was.map(|w| vec![Back::List(*w)]).unwrap_or_default();
            Did { paths: vec![list_dir(&list.id)], back, new_list: None }
        }
        Job::TrashList(id) => {
            let went = store::trash_list(root, &id).map_err(e)?;
            Did { paths: vec![root.to_path_buf()], back: vec![Back::Untrash(id, went)], new_list: None }
        }
        Job::Back(step) => match step {
            Back::Put(path, text) => {
                let now = std::fs::read_to_string(&path).ok();
                store::put_back(&path, &text).map_err(e)?;
                let back = match now {
                    Some(t) => Back::Put(path.clone(), t),
                    None => Back::Remove(path.clone()),
                };
                Did { paths: vec![path], back: vec![back], new_list: None }
            }
            Back::Remove(path) => {
                let was = std::fs::read_to_string(&path).map_err(e)?;
                std::fs::remove_file(&path).map_err(e)?;
                Did { paths: vec![path.clone()], back: vec![Back::Put(path, was)], new_list: None }
            }
            Back::Move(from, list) => {
                let book = Book::load(root);
                let item = book.items.iter().find(|i| i.file == from).cloned().ok_or("it's gone")?;
                let to = store::move_to(root, &item, &list).map_err(e)?;
                Did { paths: vec![from, to.clone(), root.to_path_buf()], back: vec![Back::Move(to, item.list)], new_list: None }
            }
            Back::List(list) => {
                let was = Book::load(root).list(&list.id).cloned();
                store::change_list(root, was.as_ref(), &list).map_err(e)?;
                Did { paths: vec![list_dir(&list.id)], back: was.map(|w| vec![Back::List(w)]).unwrap_or_default(), new_list: None }
            }
            Back::RemoveList(id) => {
                let went = store::trash_list(root, &id).map_err(e)?;
                Did { paths: vec![root.to_path_buf()], back: vec![Back::Untrash(id, went)], new_list: None }
            }
            Back::Untrash(id, from) => {
                store::untrash_list(root, &id, &from).map_err(e)?;
                Did { paths: vec![root.to_path_buf()], back: vec![Back::RemoveList(id)], new_list: None }
            }
        },
    })
}

#[derive(Clone)]
pub enum Msg {
    /// Every list and to-do, read.
    Loaded(Arc<Book>),
    /// Files changed (the watch's, or a job's own).
    Changed(Vec<PathBuf>),
    /// What changed, read again (answer `n`).
    Refreshed(u64, Arc<Book>),
    /// A job done: what it did, or why it couldn't (and its undo's name).
    Did(Result<Did, String>, Option<(String, Purpose)>),
    Clock(bool),
    Choose(Place),
    Select(Option<String>),
    /// A to-do checked off, or back on.
    Check(String),
    /// What's typed in the new to-do's field changed, or Return in it.
    EntryChanged,
    EntryEntered,
    /// The new to-do's due, as read from its words, let go of: its words
    /// stay in its title.
    KeepWords,
    /// The chosen to-do's fields: what's typed kept now.
    KeepTyped,
    /// A to-do dropped: where it goes (its list, its section, its place
    /// among that section's).
    Drop(String, String, Option<String>, usize),
    /// A to-do dropped on a board's Done lane: it's done.
    DropDone(String),
    /// A list dragged to another place among them (from, to).
    Reorder(usize, usize),
    /// A list of no sections given three to start a board with.
    Starter(String),
    /// A list made, renamed, coloured.
    NewList(String, Hue),
    Rename(String, String),
    Recolour(String, Hue),
    /// A section added, renamed (old, new) or removed, of a list.
    AddSection(String, String),
    RenameSection(String, String, String),
    RemoveSection(String, String),
    /// The chosen to-do's: due (a day, a time), reminder (its place among
    /// those offered), list, section.
    Due(Option<olo_calendar::event::When>),
    Remind(usize),
    MoveTo(usize),
    Section(usize),
    /// Show the to-do with this UID (a reminder's Show, the palette).
    Reveal(String),
    /// Files attached to a to-do (chosen in the panel, dropped on it).
    Attach(String, Vec<PathBuf>),
    /// A link attached to a to-do: its address as typed.
    AddLink(String, String),
    /// What the clipboard held, for a to-do: a picture, files, or an
    /// address (its type, its bytes).
    Pasted(String, Option<(String, Vec<u8>)>),
    /// The chosen to-do's attachment (by its place) taken off, or opened.
    Detach(usize),
    OpenAttachment(usize),
    /// (An attachment's program started: nothing to show.)
    Opened,
}

/// A lane of a list's board: one of its sections' (None: those in no
/// section), or the last, what's done (a card dropped there is checked
/// off; one dragged out of it is to do again).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Lane {
    Section(Option<String>),
    Done,
}

/// A to-do being dragged (a card on the board, a row down the list).
#[derive(Debug, Clone, PartialEq)]
pub struct Drag {
    pub uid: String,
    /// Where the press was.
    pub from: olo_kit::Point<olo_kit::Pixels>,
    /// Where the pointer is, once it's a drag (past a few points).
    pub at: Option<olo_kit::Point<olo_kit::Pixels>>,
    /// Where it would go: a lane, and a place in it.
    pub to: Option<(Lane, usize)>,
}

/// What a window keeps across an upgrade.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct Saved {
    selected: Option<String>,
    entry: String,
}

pub struct ToDoApp {
    pub root: PathBuf,
    pub book: Arc<Book>,
    pub loading: bool,
    pub settings: Settings,
    pub place: Place,
    /// What the place shows, worked out when anything it goes by changed.
    pub rows: Arc<Vec<Row>>,
    /// Where each of them starts down the list (`list::tops`), for a
    /// display of scale `tops_scale`.
    pub row_tops: std::rc::Rc<[f32]>,
    pub tops_scale: f32,
    /// The rows' columns at their right end, as wide as the place's
    /// to-dos need: their dues, and a slot for each mark any of them has
    /// (a reminder, attachments, notes), so each lines up down the list.
    pub due_col: f32,
    pub list_col: f32,
    /// A board's lanes' width, as last laid out (a drag works out from it
    /// where its card would drop).
    pub lane: std::cell::Cell<f32>,
    /// Where the board is in the window, as last laid out.
    pub board_area: std::cell::Cell<olo_kit::Bounds<olo_kit::Pixels>>,
    /// A list's board: a lane for each of its sections, with its to-dos
    /// in order, and last the lane of what's done.
    pub columns: Arc<Vec<(Lane, Vec<usize>)>>,
    /// Which working-out of them this is (each `rework` another), and
    /// each lane's cards as laid out for it (`board::lane_layout`).
    pub board_version: u64,
    pub lane_layouts: std::cell::RefCell<Vec<Option<board::LaneLayout>>>,
    /// How many each place has to do (and Done, how many are), as the
    /// sidebar and the header say: counted when the book or the day
    /// changes, not each time the window says what it shows.
    pub counts: Counts,
    /// Each lane's scrolling.
    pub column_scrolls: std::cell::RefCell<HashMap<Lane, ScrollHandle>>,
    pub selected: Option<String>,
    pub zone: TimeZone,
    pub now: Zoned,
    pub h24: bool,
    pub month_first: bool,
    /// To-dos just checked off, still shown where they were until then.
    pub lingering: HashMap<String, Instant>,
    pub history: History<Back>,
    /// Jobs in line, one at a time on a worker (each sees the last's
    /// files), with the name of the change each is.
    jobs: std::collections::VecDeque<(Job, Option<(String, Purpose)>)>,
    working: bool,
    /// Paths changed while a job was under way: read again once it's done.
    changed_meanwhile: Vec<PathBuf>,
    refreshing: u64,
    pub problem: Option<String>,
    /// The new to-do's field, and what it reads as.
    /// The keyboard, while it's the to-dos' own (a row or a card
    /// pressed) and no field's.
    pub focus: olo_kit::FocusHandle,
    pub entry: Child<TextField>,
    pub typed: Typed,
    /// Its due let go of (its words are the title's), for what's typed now.
    pub literal: bool,
    /// The section the next new to-do goes in, where a board's column
    /// was asked for one (None: the place's first).
    pub entry_section: Option<Option<String>>,
    /// The chosen to-do's fields, which to-do they hold, and whether what's
    /// typed in them is still to be kept.
    pub title: Child<TextField>,
    pub notes: Child<TextField>,
    pub notes_scroll: ScrollHandle,
    pub due_day: Child<TextField>,
    pub due_time: Child<TextField>,
    pub editing: Option<String>,
    pub typed_pending: bool,
    /// The fields are being filled from the to-do (not typed in).
    filling: bool,
    pub due_wrong: Option<String>,
    /// The month the due's small month shows (None: the due's, or this),
    /// and whether the small month is open.
    pub month: Option<(i32, u8)>,
    pub month_open: bool,
    pub details_scroll: ScrollHandle,
    keep_due: Due,
    pub sidebar_pane: SidePane,
    pub details_pane: SidePane,
    pub list_scroll: ScrollHandle,
    pub board_scroll: ScrollHandle,
    pub drag: Option<Drag>,
    /// The day turning, a lingering one going.
    due: Due,
    _watch: Option<olo_kit::watch::Watch>,
    watched: Vec<PathBuf>,
    pub dialog: Option<WindowHandle<view::Naming>>,
}

impl ToDoApp {
    fn new(cx: &mut Cx<Self>) -> ToDoApp {
        let zone = TimeZone::system();
        let settings: Settings = olo_kit::store::load(&settings_path()).unwrap_or_default();
        let saved = olo_kit::restored_state().and_then(|s| serde_json::from_str::<Saved>(&s).ok()).unwrap_or_default();
        let clock = cx.messenger();
        olo_kit::settings::watch(move |s| {
            clock.send(Msg::Clock(s.clock.twenty_four_hour));
        });
        // (Its hint is the row's own: said as what the row is for, not in
        // the faint colour of a field's hint.)
        let entry = text_field(cx, |f| f.name("New To-Do").with_text(saved.entry.clone()));
        cx.subscribe(&entry, |_: &mut ToDoApp, e: &TextFieldEvent, cx| match e {
            TextFieldEvent::Changed => cx.send(Msg::EntryChanged),
            TextFieldEvent::Entered => cx.send(Msg::EntryEntered),
            _ => {}
        });
        let field = |cx: &mut Cx<ToDoApp>, name: &'static str, hint: &'static str| text_field(cx, |f| f.name(name).hint(hint));
        // (Its title: a heading, typed in where it is, on as many lines
        // as it needs.)
        let details_scroll = ScrollHandle::new();
        let title = text_field(cx, |f| f.name("Title").multiline(true).scrolled_by(&details_scroll).font_size(olo_kit::style::LEAD_SIZE).font(olo_kit::style::ui_font(olo_kit::FontWeight::BOLD)));
        let due_day = field(cx, "Due Day", "No due date");
        let due_time = field(cx, "Due Time", "Any time");
        let notes_scroll = ScrollHandle::new();
        let notes = text_field(cx, |f| f.name("Notes").multiline(false).scrolled_by(&notes_scroll));
        for f in [&title, &notes, &due_day, &due_time] {
            cx.subscribe(f, |v: &mut ToDoApp, e: &TextFieldEvent, cx| match e {
                TextFieldEvent::Changed => {
                    if v.editing.is_some() && !v.filling {
                        v.typed_pending = true;
                        v.keep_due.at(Some(Instant::now() + KEEP_TYPED), cx, |v: &mut ToDoApp, cx| v.keep_typed(cx));
                    }
                }
                TextFieldEvent::Entered => cx.send(Msg::KeepTyped),
                _ => {}
            });
        }
        let root = store::root();
        let at = root.clone();
        let demo_now = now(&zone);
        cx.work(
            move || {
                // (Made-up lists, asked for: into a folder with none.)
                if std::env::var_os("OLO_TODO_DEMO").is_some() && std::env::var_os("OLO_TODO_DIR").is_some() {
                    let _ = std::fs::create_dir_all(&at);
                    if let Err(e) = olo_todo::demo::fill(&at, &demo_now) {
                        tracing::warn!("to-do: the made-up lists: {e}");
                    }
                }
                Arc::new(Book::load(&at))
            },
            Msg::Loaded,
        );
        let now = now(&zone);
        let place = settings.place.place();
        // (Each one width: what's in them never moves as the window's
        // other parts change.)
        let sidebar_pane = SidePane::new(view::SIDEBAR, view::SIDEBAR).first(olo_kit::widgets::side_pane::Front::Rest).restored(None, !settings.sidebar);
        let details_pane = SidePane::new(view::DETAILS, view::DETAILS).first(olo_kit::widgets::side_pane::Front::Rest).restored(None, !settings.details);
        let mut app = ToDoApp {
            root,
            book: Arc::new(Book::default()),
            loading: true,
            settings,
            place,
            rows: Arc::new(Vec::new()),
            row_tops: list::tops(&[], 1.0),
            tops_scale: 1.0,
            due_col: 0.0,
            list_col: 0.0,
            lane: std::cell::Cell::new(board::LANE_LEAST),
            board_area: Default::default(),
            columns: Arc::new(Vec::new()),
            board_version: 0,
            lane_layouts: Default::default(),
            counts: Counts::default(),
            column_scrolls: Default::default(),
            selected: saved.selected,
            zone,
            now,
            h24: true,
            month_first: month_first(),
            lingering: HashMap::new(),
            history: History::default(),
            jobs: Default::default(),
            working: false,
            changed_meanwhile: Vec::new(),
            refreshing: 0,
            problem: None,
            focus: olo_kit::FocusHandle::new(),
            entry,
            typed: Typed { title: String::new(), due: None, said: None },
            literal: false,
            entry_section: None,
            title,
            notes,
            notes_scroll,
            due_day,
            due_time,
            editing: None,
            typed_pending: false,
            due_wrong: None,
            month: None,
            month_open: false,
            details_scroll,
            keep_due: Due::default(),
            sidebar_pane,
            details_pane,
            list_scroll: ScrollHandle::new(),
            board_scroll: ScrollHandle::new(),
            drag: None,
            due: Due::default(),
            _watch: None,
            watched: Vec::new(),
            dialog: None,
            filling: false,
        };
        app.read_entry(cx);
        app.wake_when_due(cx);
        app
    }

    pub fn save(&self) {
        olo_kit::store::save(settings_path(), &self.settings);
    }

    /// The lists in the person's order.
    pub fn lists(&self) -> Vec<&List> {
        let mut lists: Vec<&List> = self.book.lists.iter().collect();
        let rank = |id: &str| self.settings.order.iter().position(|o| o == id).unwrap_or(usize::MAX);
        lists.sort_by(|a, b| rank(&a.id).cmp(&rank(&b.id)).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
        lists
    }

    pub fn list_of(&self, item: &Item) -> Option<&List> {
        self.book.list(&item.list)
    }

    pub fn hue_of(&self, list: &str) -> Hue {
        self.book.list(list).map_or(Hue::Blue, |l| l.hue)
    }

    pub fn layout(&self) -> Layout {
        match &self.place {
            Place::List(id) => self.settings.layouts.get(id).copied().unwrap_or_default(),
            _ => Layout::List,
        }
    }

    pub fn selected_item(&self) -> Option<&Item> {
        self.selected.as_deref().and_then(|uid| self.book.item(uid))
    }

    /// The list a new to-do goes in: the place's, else the one the last
    /// went in, else the first.
    pub fn list_for_new(&self) -> Option<String> {
        if let Place::List(id) = &self.place {
            return Some(id.clone());
        }
        let last = self.settings.last_list.as_ref().filter(|id| self.book.list(id).is_some());
        last.cloned().or_else(|| self.lists().first().map(|l| l.id.clone()))
    }

    /// The place's rows worked out again (the book, the place, the day or
    /// what's lingering changed).
    pub fn rework(&mut self, cx: &mut Cx<Self>) {
        let done = self.settings.show_done;
        // (Those just checked off still where they were, a moment: to do
        // yet, to the places. Said so of each, not by a copy of the book
        // with them changed: that was five thousand to-dos copied at each
        // check.)
        let (shown, lingering) = (&*self.book, &self.lingering);
        let is_open = |item: &Item| item.todo.open() || lingering.contains_key(&item.todo.uid);
        self.counts = Counts::of(&self.book, &self.now);
        self.board_version += 1;
        self.rows = Arc::new(places::rows_where(shown, &self.place, &self.now, done, &is_open));
        self.tops_scale = cx.scale_factor();
        self.row_tops = list::tops(&self.rows, self.tops_scale);
        // (A board's lanes hold what's to do; what's done has a lane of
        // its own, the newest first.)
        self.columns = Arc::new(match &self.place {
            Place::List(id) => {
                let mut lanes: Vec<(Lane, Vec<usize>)> = places::columns_where(shown, id, &is_open).into_iter().map(|(s, items)| (Lane::Section(s), items)).collect();
                if done {
                    lanes.push((Lane::Done, places::done_of_where(shown, id, &is_open)));
                }
                lanes
            }
            _ => Vec::new(),
        });
        (self.due_col, self.list_col) = list::columns(self);
        // (What was chosen and is gone isn't chosen.)
        if let Some(uid) = &self.selected
            && self.book.item(uid).is_none()
            && !self.loading
        {
            self.selected = None;
        }
        self.fill_details(cx);
        cx.invalidate();
    }

    /// The chosen to-do's fields hold what it says (unless they hold it
    /// already, with what's typed in them still to be kept).
    pub fn fill_details(&mut self, cx: &mut Cx<Self>) {
        let uid = self.selected.clone();
        if self.editing == uid && (self.typed_pending || self.any_field_focused(cx)) {
            return;
        }
        if self.editing != uid && self.typed_pending {
            self.keep_typed(cx);
        }
        self.editing = uid;
        let (title, notes, day, time) = match self.selected_item() {
            Some(item) => {
                let t = &item.todo;
                let (day, time) = match &t.due {
                    Some(olo_calendar::event::When::Date(d)) => (words::day_field(*d), String::new()),
                    Some(other) => {
                        let at = other.moment(&self.zone);
                        (words::day_field(at.date()), words::time(at.time(), self.h24))
                    }
                    None => (String::new(), String::new()),
                };
                (t.title.clone(), t.notes.clone(), day, time)
            }
            None => Default::default(),
        };
        self.filling = true;
        for (field, text) in [(&self.title, title), (&self.notes, notes), (&self.due_day, day), (&self.due_time, time)] {
            if field.read(cx).text() != text {
                field.update(cx, |f, cx| f.set_text(text, cx));
            }
        }
        self.filling = false;
        self.due_wrong = None;
    }

    fn any_field_focused(&self, cx: &Cx<Self>) -> bool {
        [&self.title, &self.notes, &self.due_day, &self.due_time].iter().any(|f| f.read(cx).is_focused())
    }

    /// What's typed in the chosen to-do's fields, kept: its title, notes
    /// and due as they read.
    pub fn keep_typed(&mut self, cx: &mut Cx<Self>) {
        self.keep_due.at(None, cx, |_, _| {});
        if !self.typed_pending {
            return;
        }
        self.typed_pending = false;
        let Some(item) = self.editing.as_deref().and_then(|uid| self.book.item(uid)).cloned() else { return };
        let mut todo = item.todo.clone();
        todo.title = self.title.read(cx).text().trim().to_owned();
        if todo.title.is_empty() {
            todo.title = item.todo.title.clone();
        }
        todo.notes = self.notes.read(cx).text().trim_end().to_owned();
        match details::read_due(self, cx) {
            Ok(due) => {
                self.due_wrong = None;
                if due != todo.due {
                    details::set_due(&mut todo, due);
                }
            }
            Err(wrong) => self.due_wrong = Some(wrong),
        }
        if todo != item.todo {
            self.change(&item, todo, "Typing", cx);
        }
        cx.invalidate();
    }

    /// A job put in line (and started, if none is under way); `name`: the
    /// change it is, for Undo.
    pub fn job(&mut self, job: Job, name: Option<(String, Purpose)>, cx: &mut Cx<Self>) {
        self.jobs.push_back((job, name));
        self.next_job(cx);
    }

    fn next_job(&mut self, cx: &mut Cx<Self>) {
        if self.working {
            return;
        }
        let Some((job, name)) = self.jobs.pop_front() else {
            // (What changed meanwhile, read now.)
            let paths = std::mem::take(&mut self.changed_meanwhile);
            if !paths.is_empty() {
                self.refresh(paths, cx);
            }
            return;
        };
        self.working = true;
        let root = self.root.clone();
        cx.work(move || run(&root, job), move |did| Msg::Did(did, name.clone()));
    }

    fn refresh(&mut self, paths: Vec<PathBuf>, cx: &mut Cx<Self>) {
        if self.working || self.loading {
            self.changed_meanwhile.extend(paths);
            return;
        }
        self.refreshing += 1;
        let (n, root, book) = (self.refreshing, self.root.clone(), self.book.clone());
        cx.work(
            move || {
                let mut book = (*book).clone();
                book.refresh(&root, &paths);
                Arc::new(book)
            },
            move |book| Msg::Refreshed(n, book),
        );
    }

    /// The store's folders watched: the folder of lists, and each list's.
    fn watch(&mut self, cx: &mut Cx<Self>) {
        let mut paths = vec![self.root.clone()];
        paths.extend(self.book.lists.iter().map(|l| self.root.join(&l.id)));
        if paths == self.watched {
            return;
        }
        let _ = std::fs::create_dir_all(&self.root);
        self.watched = paths.clone();
        let changed = cx.messenger();
        self._watch = olo_kit::watch::watch(paths, move |p| {
            changed.send(Msg::Changed(p.to_vec()));
        });
    }

    /// `todo` (the to-do in `item`, changed) written, and shown so at once.
    pub fn change(&mut self, item: &Item, todo: ToDo, name: &str, cx: &mut Cx<Self>) {
        let book = Arc::make_mut(&mut self.book);
        if let Some(i) = book.items.iter_mut().find(|i| i.todo.uid == item.todo.uid) {
            i.todo = todo.clone();
        }
        self.job(Job::Change(Box::new(item.clone()), Box::new(todo)), Some((name.to_owned(), Purpose::Do)), cx);
        self.rework(cx);
    }

    /// A to-do checked off, or back on.
    pub fn check(&mut self, uid: &str, cx: &mut Cx<Self>) {
        let Some(item) = self.book.item(uid).cloned() else { return };
        let mut todo = item.todo.clone();
        if todo.open() {
            todo.status = olo_todo::Status::Done;
            // (By the clock the window goes by, `now`: with a made-up
            // one, a to-do checked in a picture of the 19th was done
            // "tomorrow", the day the picture was taken.)
            todo.done_at = Some(now(&self.zone).timestamp());
            // (Still shown where it was, a moment, struck through: what
            // happened is seen before it goes to where done ones are.)
            if self.place != Place::Done {
                self.lingering.insert(uid.to_owned(), Instant::now() + LINGER);
            }
            self.change(&item, todo, "Done", cx);
        } else {
            todo.status = olo_todo::Status::Open;
            todo.done_at = None;
            self.lingering.remove(uid);
            self.change(&item, todo, "Not Done", cx);
        }
        self.wake_when_due(cx);
    }

    /// What's typed in the new to-do's field, read.
    fn read_entry(&mut self, cx: &mut Cx<Self>) {
        let text = self.entry.read(cx).text().to_owned();
        self.typed = if self.literal { Typed { title: text.trim().to_owned(), due: None, said: None } } else { quick::read(&text, &self.now, self.month_first) };
        // (The words read as its due, in the blue the due is said in
        // beside them: which of what's typed won't be in its title is
        // seen before Return takes them out of it. Opus's review,
        // 2026-11-20.)
        let read = self.typed.said.as_ref().filter(|_| self.typed.due.is_some()).and_then(|said| last_words(&text, said.split_whitespace().count()));
        let on_card = self.layout() == Layout::Board && matches!(self.place, Place::List(_));
        let colors = read.map(|range| {
            std::rc::Rc::new(move |text: &str, p: &olo_theme::Palette| if range.end <= text.len() { vec![(range.clone(), olo_kit::style::c(board::due_ink(p, on_card)))] } else { Vec::new() })
                as olo_kit::widgets::text_field::Colors
        });
        self.entry.update(cx, |f, cx| f.set_colors(colors, cx));
    }

    /// A new to-do, as typed: at the top of the place's list (its section,
    /// on a board's column), due as its words say, or today in Today.
    pub fn add_typed(&mut self, section: Option<String>, cx: &mut Cx<Self>) {
        self.read_entry(cx);
        if self.typed.title.is_empty() {
            return;
        }
        let Some(list) = self.list_for_new() else { return };
        let mut todo = self.typed.to_do(&olo_calendar::event::new_uid());
        // (Typed in Today with no day of its own: it's today's.)
        todo.due = todo.due.or_else(|| (self.place == Place::Today).then(|| olo_calendar::event::When::Date(self.now.date())));
        // (On a board: the column asked for, else its first.)
        todo.section = match (section, &self.entry_section) {
            (Some(s), _) => Some(s),
            (None, Some(asked)) => asked.clone(),
            (None, None) if self.layout() == Layout::Board => match self.columns.first() {
                Some((Lane::Section(s), _)) => s.clone(),
                _ => None,
            },
            (None, None) => None,
        };
        let others: Vec<usize> = places::columns(&self.book, &list).into_iter().find(|(s, _)| *s == todo.section).map(|(_, items)| items).unwrap_or_default();
        todo.order = places::top(&self.book, &others);
        todo.created = Some(now(&self.zone).timestamp());
        let file = self.root.join(&list).join(store::file_name(&todo.uid));
        Arc::make_mut(&mut self.book).items.push(Item { list: list.clone(), file, changed: None, todo: todo.clone() });
        if !matches!(self.place, Place::List(_)) {
            self.settings.last_list = Some(list.clone());
            self.save();
        }
        self.job(Job::Create(list, Box::new(todo)), Some(("New To-Do".into(), Purpose::Do)), cx);
        self.entry.update(cx, |f, cx| f.set_text("", cx));
        self.literal = false;
        self.read_entry(cx);
        self.rework(cx);
    }

    /// The chosen to-do removed (Edit ▸ Delete; Undo brings it back).
    pub fn delete_selected(&mut self, cx: &mut Cx<Self>) {
        let Some(item) = self.selected_item().cloned() else { return };
        let next = self.neighbour(&item.todo.uid);
        Arc::make_mut(&mut self.book).items.retain(|i| i.todo.uid != item.todo.uid);
        self.job(Job::Remove(Box::new(item)), Some(("Delete".into(), Purpose::Do)), cx);
        self.selected = next;
        self.rework(cx);
    }

    /// The to-do after `uid` among the rows (or before it, at the end).
    fn neighbour(&self, uid: &str) -> Option<String> {
        let todos: Vec<&str> = self.rows.iter().filter_map(|r| if let Row::ToDo(i) = r { self.book.items.get(*i).map(|i| i.todo.uid.as_str()) } else { None }).collect();
        let at = todos.iter().position(|u| *u == uid)?;
        todos.get(at + 1).or_else(|| at.checked_sub(1).and_then(|p| todos.get(p))).map(|s| (*s).to_owned())
    }

    /// On a board, the chosen one stepped up or down its lane (`across`
    /// false) or to the lane beside it, at the same place down it (true;
    /// past an empty lane to the next with to-dos): a board's lanes are
    /// read as columns, not in the list's order. With none chosen, the
    /// first lane's first.
    pub fn step_board(&mut self, by: isize, across: bool, cx: &mut Cx<Self>) {
        let columns = self.columns.clone();
        let uid_of = |i: usize| self.book.items.get(i).map(|item| item.todo.uid.clone());
        let at = self.selected.as_deref().and_then(|s| columns.iter().enumerate().find_map(|(k, (_, items))| items.iter().position(|&i| uid_of(i).as_deref() == Some(s)).map(|n| (k, n))));
        let to = match at {
            None => columns.iter().find_map(|(_, items)| items.first().copied()),
            Some((k, n)) if !across => {
                let items = &columns[k].1;
                items.get((n as isize + by).clamp(0, items.len() as isize - 1) as usize).copied()
            }
            Some((k, n)) => {
                let mut lane = k as isize + by;
                let mut found = None;
                while lane >= 0 && (lane as usize) < columns.len() {
                    let items = &columns[lane as usize].1;
                    if !items.is_empty() {
                        found = items.get(n.min(items.len() - 1)).copied();
                        break;
                    }
                    lane += by;
                }
                found
            }
        };
        if let Some(uid) = to.and_then(uid_of) {
            self.select(Some(uid), cx);
        }
    }

    /// The chosen one stepped up or down the rows.
    pub fn step(&mut self, by: isize, cx: &mut Cx<Self>) {
        // (Its to-dos by their places among the book's: none copied.)
        let uid_of = |row: &Row| if let Row::ToDo(i) = row { self.book.items.get(*i).map(|i| i.todo.uid.as_str()) } else { None };
        let count = self.rows.iter().filter(|r| matches!(r, Row::ToDo(_))).count();
        if count == 0 {
            return;
        }
        let at = self.selected.as_deref().and_then(|s| self.rows.iter().filter_map(uid_of).position(|u| u == s));
        let to = match at {
            Some(a) => (a as isize + by).clamp(0, count as isize - 1) as usize,
            None if by < 0 => count - 1,
            None => 0,
        };
        let Some(uid) = self.rows.iter().filter_map(uid_of).nth(to).map(str::to_owned) else { return };
        self.select(Some(uid), cx);
        list::reveal(self, to);
    }

    pub fn select(&mut self, uid: Option<String>, cx: &mut Cx<Self>) {
        if self.selected == uid {
            return;
        }
        if self.typed_pending {
            self.keep_typed(cx);
        }
        self.selected = uid;
        self.month = None;
        self.fill_details(cx);
        cx.invalidate();
    }

    pub fn choose(&mut self, place: Place, cx: &mut Cx<Self>) {
        if self.typed_pending {
            self.keep_typed(cx);
        }
        self.place = place.clone();
        self.settings.place = Kept::of(&place);
        self.save();
        self.drag = None;
        self.entry_section = None;
        self.list_scroll.set_offset(cx, olo_kit::point(olo_kit::px(0.0), olo_kit::px(0.0)));
        // (The one chosen stays chosen where it's shown here.)
        self.rework(cx);
        if let Some(uid) = &self.selected
            && !self.rows.iter().any(|r| matches!(r, Row::ToDo(i) if self.book.items.get(*i).is_some_and(|i| &i.todo.uid == uid)))
        {
            self.selected = None;
            self.fill_details(cx);
        }
        // (In front of it in a narrow window: the place is what shows now.)
        self.sidebar_pane.show_rest();
    }

    /// One timer for what's next: the day turning, a lingering to-do going.
    pub fn wake_when_due(&mut self, cx: &mut Cx<Self>) {
        let midnight = self.now.date().tomorrow().ok().and_then(|d| d.at(0, 0, 1, 0).to_zoned(self.zone.clone()).ok());
        let to_midnight = midnight.and_then(|m| m.duration_since(&self.now).try_into().ok()).unwrap_or(Duration::from_secs(3600));
        let linger = self.lingering.values().min().copied();
        let at = Instant::now() + to_midnight;
        let at = linger.map_or(at, |l| l.min(at));
        self.due.at(Some(at), cx, |v: &mut ToDoApp, cx| v.woken(cx));
    }

    fn woken(&mut self, cx: &mut Cx<Self>) {
        self.now = now(&self.zone);
        let gone = Instant::now();
        self.lingering.retain(|_, until| *until > gone);
        self.read_entry(cx);
        self.rework(cx);
        self.wake_when_due(cx);
    }

    /// A list made, called `name`.
    pub fn new_list(&mut self, name: String, hue: Hue, cx: &mut Cx<Self>) {
        let name = name.trim().to_owned();
        if name.is_empty() {
            return;
        }
        self.job(Job::NewList(name, hue), Some(("New List".into(), Purpose::Do)), cx);
    }

    pub fn change_list(&mut self, list: List, name: &str, cx: &mut Cx<Self>) {
        let was = self.book.list(&list.id).cloned();
        if was.as_ref() == Some(&list) {
            return;
        }
        if let Some(l) = Arc::make_mut(&mut self.book).lists.iter_mut().find(|l| l.id == list.id) {
            *l = list.clone();
        }
        self.job(Job::ChangeList(was.map(Box::new), Box::new(list)), Some((name.to_owned(), Purpose::Do)), cx);
        self.rework(cx);
    }

    /// A list put in the Trash, its to-dos with it (Undo brings it back).
    pub fn trash_list(&mut self, id: &str, cx: &mut Cx<Self>) {
        if self.place == Place::List(id.to_owned()) {
            self.choose(Place::Today, cx);
        }
        let book = Arc::make_mut(&mut self.book);
        book.lists.retain(|l| l.id != id);
        book.items.retain(|i| i.list != id);
        self.job(Job::TrashList(id.to_owned()), Some(("Delete List".into(), Purpose::Do)), cx);
        self.rework(cx);
    }

    /// The to-do `uid` dropped into list `list`'s section `section`, at
    /// `index` among that section's to-dos.
    pub fn drop_into(&mut self, uid: &str, list: &str, section: Option<String>, index: usize, cx: &mut Cx<Self>) {
        let Some(item) = self.book.item(uid).cloned() else { return };
        // (Among those still to do: what a board's lanes hold.)
        let others: Vec<usize> = places::columns(&self.book, list)
            .into_iter()
            .find(|(s, _)| *s == section)
            .map(|(_, items)| items.into_iter().filter(|&i| self.book.items[i].todo.uid != uid).collect())
            .unwrap_or_default();
        let orders = places::place(&self.book, &others, index, uid);
        let mut todo = item.todo.clone();
        todo.section = section;
        for (u, order) in &orders {
            if u == uid {
                todo.order = *order;
            }
        }
        // (Its neighbours numbered again where there was no room.)
        for (u, order) in orders.iter().filter(|(u, _)| u != uid) {
            if let Some(other) = self.book.item(u).cloned() {
                let mut t = other.todo.clone();
                t.order = *order;
                self.change(&other, t, "Move", cx);
            }
        }
        if item.list != list {
            let book = Arc::make_mut(&mut self.book);
            if let Some(i) = book.items.iter_mut().find(|i| i.todo.uid == uid) {
                i.list = list.to_owned();
                i.todo = todo.clone();
            }
            self.job(Job::Move(Box::new(item), list.to_owned(), Box::new(todo)), Some(("Move".into(), Purpose::Do)), cx);
            self.rework(cx);
        } else if todo != item.todo {
            self.change(&item, todo, "Move", cx);
        }
    }

    /// The last change taken back (`redo`: made again).
    fn undo(&mut self, redo: bool, cx: &mut Cx<Self>) {
        if self.typed_pending {
            self.keep_typed(cx);
        }
        let Some(step) = (if redo { self.history.take_redo() } else { self.history.take_undo() }) else { return };
        let purpose = if redo { Purpose::Redo } else { Purpose::Undo };
        self.job(Job::Back(step.change), Some((step.name, purpose)), cx);
    }

    pub fn menus(&self) -> Vec<olo_kit::Menu> {
        use olo_kit::Menu;
        let chosen = self.selected_item();
        let has = chosen.is_some();
        let done = chosen.is_some_and(|i| !i.todo.open());
        let in_list = matches!(self.place, Place::List(_));
        let board = self.layout() == Layout::Board;
        let mut lists = Menu::new("Move To");
        for (i, l) in self.lists().iter().enumerate() {
            lists = lists.item(l.name.clone(), format!("move-to:{i}")).checked(chosen.is_some_and(|c| c.list == l.id)).enabled(has);
        }
        vec![
            Menu::new("To-Do")
                .app_items(&["notifications"])
                .item("New To-Do", "new")
                .key("n")
                .item("New List…", "new-list")
                .key("N")
                .item("New Section…", "new-section")
                .enabled(in_list)
                .separator()
                .item("Rename List…", "rename-list")
                .enabled(in_list)
                .item("Delete List", "delete-list")
                .enabled(in_list)
                .close_quit(),
            self.history.edit_items(Menu::new("Edit")).separator().item("Delete", "delete").key("backspace").enabled(has),
            Menu::new("Item")
                .item(if done { "Mark as Not Done" } else { "Mark as Done" }, "check")
                .key("k")
                .enabled(has)
                .separator()
                .item("Due Today", "due:today")
                .key("t")
                .enabled(has)
                .item("Due Tomorrow", "due:tomorrow")
                .enabled(has)
                .item("Due Next Week", "due:next-week")
                .enabled(has)
                .item("No Due Date", "due:none")
                .enabled(chosen.is_some_and(|i| i.todo.due.is_some()))
                .separator()
                .item("Attach File…", "attach-file")
                .enabled(has)
                .item("Attach Link…", "attach-link")
                .enabled(has)
                .item("Paste as Attachment", "paste-attachment")
                .key("V")
                .enabled(has)
                .separator()
                .submenu(lists),
            Menu::new("View")
                .item("Today", "place:today")
                .key("1")
                .checked(self.place == Place::Today)
                .item("Upcoming", "place:upcoming")
                .key("2")
                .checked(self.place == Place::Upcoming)
                .item("All", "place:all")
                .key("3")
                .checked(self.place == Place::All)
                .item("Done", "place:done")
                .key("4")
                .checked(self.place == Place::Done)
                .separator()
                .item("As List", "layout:list")
                .key("l")
                .checked(in_list && !board)
                .enabled(in_list)
                .item("As Board", "layout:board")
                .key("b")
                .checked(board)
                .enabled(in_list)
                .item("Show Done", "show-done")
                .checked(self.settings.show_done)
                .separator()
                .item("Show Sidebar", "sidebar")
                .key("s")
                .checked(self.sidebar_pane.showing())
                .item("Show Details", "details")
                .key("i")
                .checked(self.details_pane.showing()),
        ]
    }

    /// The day a command like `due:tomorrow` says, for the chosen to-do.
    fn due_command(&mut self, what: &str, cx: &mut Cx<Self>) -> bool {
        let today = self.now.date();
        let day = match what {
            "today" => Some(today),
            "tomorrow" => today.tomorrow().ok(),
            "next-week" => today.checked_add(jiff::Span::new().days(7 - i64::from(today.weekday().to_monday_zero_offset()))).ok(),
            "none" => None,
            other => match other.parse::<jiff::civil::Date>() {
                Ok(d) => Some(d),
                Err(_) => return false,
            },
        };
        cx.send(Msg::Due(day.map(olo_calendar::event::When::Date)));
        true
    }
}

impl View for ToDoApp {
    type Message = Msg;

    fn message(&mut self, message: Msg, cx: &mut Cx<Self>) {
        match message {
            Msg::Loaded(book) => {
                self.book = book;
                self.loading = false;
                // A first list, the first time (nothing to put a to-do in).
                if self.book.lists.is_empty() && !self.working && self.jobs.is_empty() {
                    self.job(Job::NewList(olo_kit::words::tr("To-Do").into(), Hue::Green), None, cx);
                }
                if let Place::List(id) = &self.place
                    && self.book.list(id).is_none()
                {
                    self.place = Place::Today;
                }
                self.watch(cx);
                self.rework(cx);
                let paths = std::mem::take(&mut self.changed_meanwhile);
                if !paths.is_empty() {
                    self.refresh(paths, cx);
                }
            }
            Msg::Changed(paths) => self.refresh(paths, cx),
            Msg::Refreshed(n, book) => {
                if n != self.refreshing || self.working {
                    return;
                }
                self.book = book;
                self.watch(cx);
                if let Place::List(id) = &self.place
                    && self.book.list(id).is_none()
                {
                    self.place = Place::Today;
                }
                self.rework(cx);
            }
            Msg::Did(did, name) => {
                self.working = false;
                match did {
                    Ok(did) => {
                        if let Some((name, purpose)) = name {
                            for back in did.back.into_iter().rev() {
                                self.history.record(purpose, name.clone(), back);
                            }
                        }
                        if let Some(id) = did.new_list {
                            self.place = Place::List(id.clone());
                            self.settings.place = Kept::List(id);
                            self.save();
                        }
                        self.problem = None;
                        self.changed_meanwhile.extend(did.paths);
                    }
                    Err(e) => {
                        tracing::warn!("to-do: {e}");
                        self.problem = Some(e);
                        // (What's shown as it was meant to be is read again.)
                        self.changed_meanwhile.push(self.root.clone());
                        for l in &self.book.lists {
                            self.changed_meanwhile.push(self.root.join(&l.id));
                        }
                    }
                }
                self.next_job(cx);
                cx.invalidate();
            }
            Msg::Clock(h24) => {
                if self.h24 != h24 {
                    self.h24 = h24;
                    if !self.typed_pending {
                        self.editing = None;
                        self.fill_details(cx);
                    }
                    cx.invalidate();
                }
            }
            Msg::Choose(place) => self.choose(place, cx),
            Msg::Select(uid) => self.select(uid, cx),
            Msg::Check(uid) => self.check(&uid, cx),
            Msg::EntryChanged => {
                self.literal = false;
                self.read_entry(cx);
                cx.invalidate();
            }
            Msg::EntryEntered => self.add_typed(None, cx),
            Msg::KeepWords => {
                self.literal = true;
                self.read_entry(cx);
                cx.invalidate();
            }
            Msg::KeepTyped => {
                self.typed_pending = true;
                self.keep_typed(cx);
            }
            Msg::Drop(uid, list, section, index) => {
                // (Out of the Done lane: to do again, there.)
                if self.book.item(&uid).is_some_and(|i| !i.todo.open()) {
                    self.check(&uid, cx);
                }
                self.drop_into(&uid, &list, section, index, cx);
            }
            Msg::DropDone(uid) => {
                if self.book.item(&uid).is_some_and(|i| i.todo.open()) {
                    self.check(&uid, cx);
                    // (It's in its lane at once: nowhere to linger.)
                    self.lingering.remove(&uid);
                    self.rework(cx);
                }
            }
            Msg::Reorder(from, to) => {
                let mut ids: Vec<String> = self.lists().iter().map(|l| l.id.clone()).collect();
                if from < ids.len() && to < ids.len() {
                    let id = ids.remove(from);
                    ids.insert(to, id);
                    self.settings.order = ids;
                    self.save();
                    cx.invalidate();
                }
            }
            Msg::Starter(id) => {
                let Some(mut list) = self.book.list(&id).cloned() else { return };
                if !list.sections.is_empty() {
                    return;
                }
                // (Two: a board's last lane is what's done, always.)
                list.sections = ["To Do", "Doing"].iter().map(|s| olo_kit::words::tr(s).to_owned()).collect();
                // (What it has to do goes in the first of them.)
                let first = list.sections[0].clone();
                let loose: Vec<Item> = self.book.items.iter().filter(|i| i.list == id && i.todo.section.is_none() && i.todo.open()).cloned().collect();
                self.change_list(list, "New Sections", cx);
                for item in loose {
                    let mut t = item.todo.clone();
                    t.section = Some(first.clone());
                    self.change(&item, t, "New Sections", cx);
                }
            }
            Msg::NewList(name, hue) => self.new_list(name, hue, cx),
            Msg::Rename(id, name) => {
                let Some(mut list) = self.book.list(&id).cloned() else { return };
                let name = name.trim();
                if name.is_empty() {
                    return;
                }
                list.name = name.to_owned();
                self.change_list(list, "Rename List", cx);
            }
            Msg::Recolour(id, hue) => {
                let Some(mut list) = self.book.list(&id).cloned() else { return };
                list.hue = hue;
                self.change_list(list, "Colour", cx);
            }
            Msg::AddSection(id, name) => {
                let Some(mut list) = self.book.list(&id).cloned() else { return };
                let name = name.trim().to_owned();
                if name.is_empty() || list.sections.contains(&name) {
                    return;
                }
                // (Those of no section the list had are shown first, as a
                // section of none.)
                list.sections = places::sections(&self.book, &id);
                list.sections.push(name);
                self.change_list(list, "New Section", cx);
            }
            Msg::RenameSection(id, old, new) => {
                let Some(mut list) = self.book.list(&id).cloned() else { return };
                let new = new.trim().to_owned();
                if new.is_empty() || new == old || list.sections.contains(&new) {
                    return;
                }
                list.sections = places::sections(&self.book, &id).into_iter().map(|s| if s == old { new.clone() } else { s }).collect();
                let moving: Vec<Item> = self.book.items.iter().filter(|i| i.list == id && i.todo.section.as_deref() == Some(old.as_str())).cloned().collect();
                self.change_list(list, "Rename Section", cx);
                for item in moving {
                    let mut t = item.todo.clone();
                    t.section = Some(new.clone());
                    self.change(&item, t, "Rename Section", cx);
                }
            }
            Msg::RemoveSection(id, name) => {
                let Some(mut list) = self.book.list(&id).cloned() else { return };
                list.sections = places::sections(&self.book, &id).into_iter().filter(|s| *s != name).collect();
                // (Its to-dos stay, in no section.)
                let moving: Vec<Item> = self.book.items.iter().filter(|i| i.list == id && i.todo.section.as_deref() == Some(name.as_str())).cloned().collect();
                self.change_list(list, "Delete Section", cx);
                for item in moving {
                    let mut t = item.todo.clone();
                    t.section = None;
                    self.change(&item, t, "Delete Section", cx);
                }
            }
            Msg::Due(due) => {
                let Some(item) = self.selected_item().cloned() else { return };
                let mut todo = item.todo.clone();
                details::set_due(&mut todo, due);
                self.editing = None;
                self.typed_pending = false;
                self.change(&item, todo, "Due Date", cx);
                self.fill_details(cx);
            }
            Msg::Remind(i) => {
                let Some(item) = self.selected_item().cloned() else { return };
                let mut todo = item.todo.clone();
                todo.remind = details::reminders(&todo).get(i).and_then(|r| r.0.clone());
                if todo != item.todo {
                    self.change(&item, todo, "Reminder", cx);
                }
            }
            Msg::MoveTo(i) => {
                let Some(item) = self.selected_item().cloned() else { return };
                let Some(list) = self.lists().get(i).map(|l| l.id.clone()) else { return };
                if list != item.list {
                    let uid = item.todo.uid.clone();
                    self.drop_into(&uid, &list, None, 0, cx);
                }
            }
            Msg::Section(i) => {
                let Some(item) = self.selected_item().cloned() else { return };
                let sections = places::sections(&self.book, &item.list);
                // (The first offered is none.)
                let section = i.checked_sub(1).and_then(|i| sections.get(i).cloned());
                if section != item.todo.section {
                    let uid = item.todo.uid.clone();
                    self.drop_into(&uid, &item.list.clone(), section, 0, cx);
                }
            }
            Msg::Attach(uid, files) => {
                let Some(item) = self.book.item(&uid).cloned() else { return };
                let files: Vec<PathBuf> = files.into_iter().filter(|f| f.is_file()).collect();
                if !files.is_empty() {
                    self.job(Job::Attach(Box::new(item), files), Some(("Attach".into(), Purpose::Do)), cx);
                }
            }
            Msg::AddLink(uid, address) => {
                let Some(item) = self.book.item(&uid).cloned() else { return };
                let Some(uri) = details::link(&address) else { return };
                let mut todo = item.todo.clone();
                if todo.attachments.iter().any(|a| a.uri == uri) {
                    return;
                }
                todo.attachments.push(olo_todo::Attachment { name: details::link_name(&uri), uri, mime: None });
                self.change(&item, todo, "Attach", cx);
            }
            Msg::Pasted(uid, held) => {
                let Some(item) = self.book.item(&uid).cloned() else { return };
                match held {
                    Some((mime, bytes)) if mime.starts_with("image/") => {
                        let ext = mime.trim_start_matches("image/").split(['+', ';']).next().unwrap_or("png").replace("jpeg", "jpg");
                        let name = format!("{}.{ext}", olo_kit::words::tr("Pasted Picture"));
                        self.job(Job::AttachBytes(Box::new(item), name, bytes, Some(mime)), Some(("Attach".into(), Purpose::Do)), cx);
                    }
                    Some((mime, bytes)) if mime == "text/uri-list" => {
                        let files: Vec<PathBuf> =
                            String::from_utf8_lossy(&bytes).lines().filter_map(|l| l.trim().strip_prefix("file://")).map(|p| PathBuf::from(olo_todo::item::percent_decode(p))).collect();
                        cx.send(Msg::Attach(uid, files));
                    }
                    Some((_, bytes)) => {
                        let text = String::from_utf8_lossy(&bytes).trim().to_owned();
                        if details::link(&text).is_some() && !text.contains(char::is_whitespace) {
                            cx.send(Msg::AddLink(uid, text));
                        }
                    }
                    None => {}
                }
            }
            Msg::Detach(i) => {
                let Some(item) = self.selected_item().cloned() else { return };
                let mut todo = item.todo.clone();
                if i < todo.attachments.len() {
                    todo.attachments.remove(i);
                    self.change(&item, todo, "Remove Attachment", cx);
                }
            }
            Msg::OpenAttachment(i) => {
                let Some(uri) = self.selected_item().and_then(|item| item.todo.attachments.get(i)).map(|a| a.uri.clone()) else { return };
                cx.work(
                    move || {
                        if let Err(e) = olo_xdg::open::open(&olo_xdg::open::Target::parse(&uri)) {
                            tracing::warn!("to-do: opening {uri}: {e}");
                        }
                    },
                    |()| Msg::Opened,
                );
                return;
            }
            Msg::Opened => return,
            Msg::Reveal(uid) => {
                if self.loading {
                    self.selected = Some(uid);
                    return;
                }
                let Some(item) = self.book.item(&uid).cloned() else { return };
                let place = if item.todo.open() { Place::List(item.list.clone()) } else { Place::Done };
                self.choose(place, cx);
                self.select(Some(uid.clone()), cx);
                if let Some(at) = self.rows.iter().filter(|r| matches!(r, Row::ToDo(_))).position(|r| matches!(r, Row::ToDo(i) if self.book.items[*i].todo.uid == uid)) {
                    list::reveal(self, at);
                }
                self.details_pane.show_pane();
            }
        }
        cx.invalidate();
    }

    fn command(&mut self, command: &str, cx: &mut Cx<Self>) -> bool {
        match command {
            "new" => {
                if self.place == Place::Done {
                    self.choose(Place::Today, cx);
                }
                // (On a board: a card to type it in, in its first lane.)
                self.entry_section = match (self.layout(), &self.place, self.columns.first()) {
                    (Layout::Board, Place::List(_), Some((Lane::Section(first), _))) => Some(first.clone()),
                    _ => None,
                };
                self.sidebar_pane.show_rest();
                self.details_pane.show_rest();
                self.entry.focus(cx);
            }
            "new-list" => view::Naming::ask(self, view::Asking::NewList, cx),
            "new-section" => {
                let Place::List(id) = &self.place else { return false };
                view::Naming::ask(self, view::Asking::NewSection(id.clone()), cx);
            }
            "rename-list" => {
                let Place::List(id) = &self.place else { return false };
                view::Naming::ask(self, view::Asking::RenameList(id.clone()), cx);
            }
            "delete-list" => {
                let Place::List(id) = self.place.clone() else { return false };
                self.trash_list(&id, cx);
            }
            "delete" => self.delete_selected(cx),
            "attach-file" => {
                let Some(uid) = self.selected.clone() else { return false };
                let home = std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from);
                let to = cx.messenger();
                olo_kit::file_panel::show("Attach", olo_kit::file_panel::FilePanel::open(home).multiple(), move |a| match a.result {
                    olo_kit::file_panel::PanelResult::Chosen(f) => {
                        to.send(Msg::Attach(uid, vec![f]));
                    }
                    olo_kit::file_panel::PanelResult::Several(files) => {
                        to.send(Msg::Attach(uid, files));
                    }
                    olo_kit::file_panel::PanelResult::Cancelled => {}
                });
            }
            "attach-link" => {
                let Some(uid) = self.selected.clone() else { return false };
                view::Naming::ask(self, view::Asking::Link(uid), cx);
            }
            "paste-attachment" => {
                let Some(uid) = self.selected.clone() else { return false };
                let to = cx.messenger();
                let wanted = ["image/png", "image/jpeg", "image/webp", "image/gif", "text/uri-list", "text/plain;charset=utf-8", "text/plain"].map(str::to_owned).to_vec();
                olo_kit::clipboard::paste_as(wanted, move |held| {
                    to.send(Msg::Pasted(uid, held));
                });
            }
            "undo" => self.undo(false, cx),
            "redo" => self.undo(true, cx),
            "check" => {
                let Some(uid) = self.selected.clone() else { return false };
                self.check(&uid, cx);
            }
            "show-done" => {
                self.settings.show_done = !self.settings.show_done;
                self.save();
                self.rework(cx);
            }
            "sidebar" => {
                if self.sidebar_pane.alone() {
                    self.sidebar_pane.toggle();
                    self.details_pane.show_rest();
                } else {
                    self.settings.sidebar = !self.settings.sidebar;
                    self.save();
                }
                cx.invalidate();
            }
            "details" => {
                if self.details_pane.alone() {
                    self.details_pane.toggle();
                    self.sidebar_pane.show_rest();
                } else {
                    self.settings.details = !self.settings.details;
                    self.save();
                }
                cx.invalidate();
            }
            "close" | "quit" => {
                if self.typed_pending {
                    self.keep_typed(cx);
                }
                olo_kit::quit();
            }
            c if c.starts_with("place:") => {
                let place = match &c[6..] {
                    "today" => Place::Today,
                    "upcoming" => Place::Upcoming,
                    "all" => Place::All,
                    "done" => Place::Done,
                    // (An agent's: a list by its name.)
                    name => match self.book.lists.iter().find(|l| l.name.eq_ignore_ascii_case(name) || l.id == name) {
                        Some(l) => Place::List(l.id.clone()),
                        None => return false,
                    },
                };
                self.choose(place, cx);
            }
            c if c.starts_with("layout:") => {
                let Place::List(id) = &self.place else { return false };
                let layout = match &c[7..] {
                    "list" => Layout::List,
                    "board" => Layout::Board,
                    _ => return false,
                };
                self.settings.layouts.insert(id.clone(), layout);
                self.save();
                cx.invalidate();
            }
            // (The small month under the due's fields, opened or shut: the
            // calendar button's Pick from a Month, for an agent too.)
            "due:month" => {
                self.month_open = !self.month_open;
                self.month = None;
                cx.invalidate();
            }
            c if c.starts_with("due:") => return self.due_command(&c[4..].to_owned(), cx),
            c if c.starts_with("move-to:") => {
                let Ok(i) = c[8..].parse::<usize>() else { return false };
                cx.send(Msg::MoveTo(i));
            }
            // (An agent's: a to-do by its UID, chosen; or one added, as typed.)
            // ("select:" alone: none chosen.)
            c if c.starts_with("select:") => self.select(Some(c[7..].to_owned()).filter(|uid| !uid.is_empty()), cx),
            c if c.starts_with("add:") => {
                let typed = c[4..].to_owned();
                self.entry.update(cx, |f, cx| f.set_text(typed, cx));
                self.add_typed(None, cx);
            }
            _ => return false,
        }
        true
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> olo_kit::Element {
        view::body(self, cx)
    }

    fn window_activated(&mut self, active: bool, cx: &mut Cx<Self>) {
        if !active {
            if self.typed_pending {
                self.keep_typed(cx);
            }
            return;
        }
        let now = now(&self.zone);
        if now.date() != self.now.date() {
            self.woken(cx);
        }
    }

    fn save_state(&mut self, cx: &mut Cx<Self>) -> Option<String> {
        if self.typed_pending {
            self.keep_typed(cx);
        }
        serde_json::to_string(&Saved { selected: self.selected.clone(), entry: self.entry.read(cx).text().to_owned() }).ok()
    }
}

/// `olo-todo --print [PLACE]`: what's shown in a place (today, upcoming,
/// all, done, or a list by its name), as JSON, with no window.
fn print(args: &[String]) -> std::process::ExitCode {
    let zone = TimeZone::system();
    let now = now(&zone);
    let book = Book::load(&store::root());
    let place = match args.get(1).map(|s| s.to_ascii_lowercase()).as_deref() {
        None | Some("today") => Place::Today,
        Some("upcoming") => Place::Upcoming,
        Some("all") => Place::All,
        Some("done") => Place::Done,
        Some(name) => match book.lists.iter().find(|l| l.name.eq_ignore_ascii_case(name)) {
            Some(l) => Place::List(l.id.clone()),
            None => {
                eprintln!("olo-todo: no list called “{name}”");
                return std::process::ExitCode::FAILURE;
            }
        },
    };
    let out: Vec<serde_json::Value> = places::rows(&book, &place, &now, place == Place::Done)
        .iter()
        .filter_map(|r| match r {
            Row::ToDo(i) => Some(&book.items[*i]),
            Row::Heading(_) | Row::Nothing => None,
        })
        .map(|item| {
            let t = &item.todo;
            serde_json::json!({
                "uid": t.uid,
                "title": t.title,
                "list": book.list(&item.list).map(|l| l.name.clone()),
                "section": t.section,
                "done": !t.open(),
                "due": t.due.as_ref().map(|d| if d.is_date() { d.local().date().to_string() } else { d.moment(&zone).datetime().to_string() }),
                "reminder": olo_todo::item::remind_at(t, &zone).map(|r| r.to_zoned(zone.clone()).datetime().to_string()),
                "notes": t.notes,
                "attachments": t.attachments.iter().map(|a| a.uri.clone()).collect::<Vec<_>>(),
                "file": item.file,
            })
        })
        .collect();
    println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
    std::process::ExitCode::SUCCESS
}

/// `olo-todo --add "TEXT" [--list NAME]`: a to-do added as typed, with no
/// window (its file's path printed).
fn add(args: &[String]) -> std::process::ExitCode {
    let value = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1));
    let Some(text) = args.get(1).filter(|t| !t.starts_with("--")) else {
        eprintln!("olo-todo: --add \"what to do [when]\" [--list NAME]");
        return std::process::ExitCode::FAILURE;
    };
    let zone = TimeZone::system();
    let root = store::root();
    let mut book = Book::load(&root);
    if book.lists.is_empty() {
        match store::create_list(&root, olo_kit::words::tr("To-Do"), Hue::Green) {
            Ok(l) => book.lists.push(l),
            Err(e) => {
                eprintln!("olo-todo: {e}");
                return std::process::ExitCode::FAILURE;
            }
        }
    }
    let list = match value("--list") {
        Some(name) => match book.lists.iter().find(|l| l.name.eq_ignore_ascii_case(name)) {
            Some(l) => l.id.clone(),
            None => {
                eprintln!("olo-todo: no list called “{name}”");
                return std::process::ExitCode::FAILURE;
            }
        },
        None => {
            let settings: Settings = olo_kit::store::load(&settings_path()).unwrap_or_default();
            settings.last_list.filter(|id| book.list(id).is_some()).unwrap_or_else(|| book.lists[0].id.clone())
        }
    };
    let typed = quick::read(text, &now(&zone), month_first());
    let mut todo = typed.to_do(&olo_calendar::event::new_uid());
    let others: Vec<usize> = places::columns(&book, &list).into_iter().find(|(s, _)| s.is_none()).map(|(_, i)| i).unwrap_or_default();
    todo.order = places::top(&book, &others);
    match store::create(&root, &list, &todo) {
        Ok(path) => {
            println!("{}", serde_json::json!({ "uid": todo.uid, "title": todo.title, "file": path }));
            std::process::ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("olo-todo: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

/// `olo-todo --done UID`: a to-do checked off, with no window (a
/// reminder's Done, Deskbar's check).
pub fn done(uid: &str) -> std::process::ExitCode {
    let book = Book::load(&store::root());
    let Some(item) = book.item(uid) else {
        eprintln!("olo-todo: no to-do {uid}");
        return std::process::ExitCode::FAILURE;
    };
    let mut todo = item.todo.clone();
    todo.status = olo_todo::Status::Done;
    todo.done_at = Some(now(&TimeZone::system()).timestamp());
    match store::change(item, &todo) {
        Ok(_) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("olo-todo: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

/// The to-do a launch asks to be shown (`--show UID`).
fn to_reveal(args: &[String]) -> Option<String> {
    let at = args.iter().position(|a| a == "--show")?;
    args.get(at + 1).cloned()
}

fn main() -> std::process::ExitCode {
    olo_crash::install();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--print") => return print(&args),
        Some("--add") => return add(&args),
        Some("--done") => return args.get(1).map_or(std::process::ExitCode::FAILURE, |uid| done(uid)),
        // Deskbar's widgets (no window).
        Some("--glance") => return glance::serve(),
        // The reminders service (the session's; no window).
        Some("--service") => {
            let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
            let _ = tracing_subscriber::fmt().with_env_filter(filter).with_writer(std::io::stderr).try_init();
            return service::run();
        }
        _ => {}
    }
    if instance::forward(APP_ID, &args) {
        return std::process::ExitCode::SUCCESS;
    }
    olo_kit::run(APP_ID, |app| {
        let window = olo_kit::open_window(WindowOptions::new("To-Do", 1100.0, 700.0).min_size(view::MIN_WIDTH, view::MIN_HEIGHT).keep_size("main"), ToDoApp::new);
        if let Some(uid) = to_reveal(&std::env::args().skip(1).collect::<Vec<_>>()) {
            window.messenger().send(Msg::Reveal(uid));
        }
        instance::listen(APP_ID, olo_kit::proxy());
        olo_kit::message_handler(app, move |message, _| {
            if let Ok(launch) = message.downcast::<Launch>() {
                olo_kit::raise(Some(&window));
                if let Some(uid) = to_reveal(&launch.args) {
                    window.messenger().send(Msg::Reveal(uid));
                }
            }
        });
    });
    std::process::ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The words read as a new to-do's due are the last of what's typed:
    /// where they are in it, however it's spaced, to colour them.
    #[test]
    fn the_last_words_of_what_is_typed() {
        assert_eq!(last_words("Buy stamps friday 3pm", 2), Some(11..21));
        assert_eq!(last_words("Buy  stamps   tomorrow  ", 1), Some(14..22));
        assert_eq!(last_words("tomorrow", 1), Some(0..8));
        assert_eq!(last_words("Appeler Zoé demain", 1), Some(13..19));
        assert_eq!(last_words("one two", 3), None);
        assert_eq!(last_words("", 1), None);
    }
}
