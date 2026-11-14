//! Log Viewer: every Olo program's log in one place, from the user's
//! journal (read natively, journal.rs, on a worker thread, reader.rs),
//! live as it is written. One search (the kit's query language: words,
//! "phrases", -words, and `program:` several or `all`, `-program:`,
//! `level:`, `since:`, `pid:`) with the filters as menus beside it; a
//! line's full text below the list, and Show in Context for the lines
//! around one. On Olo's own kit (olo-kit): one view on its window's
//! looper, told each batch of lines by the reader's thread.
//!
//! For agents the same query prints from the command line:
//! `olo-logs --print [--query TEXT] [--since 10m|2h|today|boot|all]
//! [--level info|warning|error] [--program NAME | --all-programs]
//! [--find TEXT] [--json] [--follow]`.

mod highlight;
mod journal;
mod model;
mod reader;

use std::{
    collections::BTreeMap,
    ops::Range,
    path::PathBuf,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use model::{Level, Line, Now, Query, Since};
use olo_kit::{
    Child, ClickEvent, ClipboardItem, Cx, Element, Font, FontWeight, Hsla, IntoElement as _, Menu, MenuBar, ParentElement as _, Pixels, Point, SharedString, Styled as _, TextRun, View, WindowHandle,
    WindowOptions, agent, canvas, clipboard, div,
    file_panel::{self, FilePanel, PanelResult},
    instance::{self, Launch},
    menu, metrics as m, px,
    search::{self, Facet, SearchEvent, SearchField, search_field},
    style::{self, c},
    theme,
    widgets::{Button, CheckBox, Column, ColumnListView, ListModel, TextView, text_view},
};
use olo_kit_base::list::ListState;
use olo_theme::Palette;
use reader::Update;
use serde::{Deserialize, Serialize};

const APP_ID: &str = "olo.logs";
const TITLE: &str = "Log Viewer";

const LEVELS: [(Level, &str); 4] = [(Level::Debug, "Everything"), (Level::Info, "Info and Above"), (Level::Warning, "Warnings and Errors"), (Level::Error, "Errors Only")];
const LEVEL_CODES: [&str; 4] = ["debug", "info", "warning", "error"];
const TIMES: [(Since, &str); 5] = [(Since::Minutes(10), "Last 10 Minutes"), (Since::Minutes(60), "Last Hour"), (Since::Today, "Today"), (Since::Boot, "Since Startup"), (Since::All, "Everything")];

/// The height of the pane under the list with the selected line in full.
const DETAIL: f32 = 112.0;
/// The least the Message column is given (the last: it fills the rest).
const MESSAGE_LEAST: f32 = 200.0;

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    /// The search's text: its filters stay from one run to the next.
    search: String,
    live: bool,
    /// From before the search was one text: read once, then `search`.
    #[serde(skip_serializing)]
    query: Option<OldQuery>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { search: String::new(), live: true, query: None }
    }
}

/// The filters as they were kept before (a menu each).
#[derive(Deserialize, Default)]
#[serde(default)]
struct OldQuery {
    since: Option<Since>,
    level: Option<Level>,
    program: Option<String>,
    all_programs: bool,
}

impl OldQuery {
    fn text(&self) -> String {
        let mut t = String::new();
        let mut set = |key: &str, values: &[&str]| search::set_filter(&mut t, model::KEYS, key, values);
        if let Some(p) = &self.program {
            set("program", &[p]);
        } else if self.all_programs {
            set("program", &["all"]);
        }
        if let Some(l) = self.level.filter(|&l| l != Level::Info) {
            set("level", &[LEVEL_CODES[LEVELS.iter().position(|(x, _)| *x == l).unwrap_or(1)]]);
        }
        if let Some(s) = self.since.filter(|&s| s != Since::Boot) {
            set("since", &[&s.code()]);
        }
        t
    }
}

fn settings_path() -> std::path::PathBuf {
    olo_xdg::config_dir().join("logs.json")
}

struct Logs {
    settings: Settings,
    lines: Vec<Line>,
    /// The lines shown (indices into `lines`).
    view: Vec<usize>,
    warnings: usize,
    errors: usize,
    /// The query the search's text asks for.
    query: Query,
    /// The programs in the query's time, and how many lines each has
    /// there.
    programs: BTreeMap<Arc<str>, usize>,
    /// The time `programs` are of (words and the level don't change them).
    programs_since: Option<Since>,
    /// The search's filters (their values and counts), made again when
    /// the programs change.
    facets: Option<Rc<Vec<Facet>>>,
    /// Lines being chosen again, a slice at a time (`refilter`); the
    /// count tells a slice asked for by an earlier filter from this one's.
    filter: Option<Filter>,
    filters: u64,
    /// A sliding window of time ("Last 10 Minutes") moving on: the count
    /// of the tick that's waited for.
    ticks: u64,
    ticking: bool,
    now: Now,
    /// The search from before Show in Context, to go back to.
    before_context: Option<String>,
    list: ListModel,
    search: Child<SearchField>,
    detail: Child<TextView>,
    /// The selected line (an index into `lines`).
    selected: Option<usize>,
    /// Whether the list showed its last row (live lines keep it there).
    at_end: bool,
    loaded: bool,
    failed: Option<String>,
    /// What the lines are of: the journal, or a log file an app writes
    /// beside it (an Agent Screen's).
    source: Option<PathBuf>,
    /// Bumped when the source changes: the reader of the one before stops,
    /// and what it sent is dropped.
    generation: Arc<AtomicU64>,
    /// File ▸ Open Log File's panel, while it's open.
    panel: Option<WindowHandle<file_panel::PanelView>>,
    /// The Agent Screens' logs, as last looked for (File ▸ Agent Screen
    /// Logs), and when.
    screen_logs: (Option<Instant>, Vec<(String, PathBuf)>),
}

/// What the window is told.
enum Msg {
    /// An update from the reader of source `generation` ([`Logs::start`]).
    Read(u64, Update),
    /// The next slice of filter number this.
    FilterOn(u64),
    /// Tick number this: time moved on.
    Tick(u64),
    /// The search's text changed.
    Searched,
    /// The list's selection changed.
    Selected,
    Context(usize, Point<Pixels>),
    Command(String),
    /// The Open Log File panel's answer.
    Opened(Option<PathBuf>),
    /// Launched again: the search to show and the log to read.
    Launch(Option<String>, Option<PathBuf>),
}

/// The Agent Screens' logs (`$XDG_RUNTIME_DIR/olo-screens`: a screen's
/// own, `NAME.log`, and each program run there, `NAME-PROGRAM.log`), the
/// latest written first.
fn screen_logs() -> Vec<(String, PathBuf)> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map_or_else(|| PathBuf::from("/tmp"), PathBuf::from).join("olo-screens");
    let mut logs: Vec<(std::time::SystemTime, String, PathBuf)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "log"))
        .filter_map(|e| {
            let at = e.metadata().ok()?.modified().ok()?;
            Some((at, e.path().file_stem()?.to_string_lossy().into_owned(), e.path()))
        })
        .collect();
    logs.sort_by_key(|l| std::cmp::Reverse(l.0));
    logs.into_iter().take(24).map(|(_, name, path)| (name, path)).collect()
}

/// The lines being chosen again for a new query (`Logs::refilter`), a
/// slice of them at a time: a day's journal is hundreds of thousands of
/// lines, and a keystroke in the search must not hold a frame up. The
/// lines shown change once it has been through them all.
struct Filter {
    query: Query,
    now: Now,
    /// The next line to look at.
    next: usize,
    view: Vec<usize>,
    warnings: usize,
    errors: usize,
    /// The programs in the query's time, when they are found again.
    programs: Option<BTreeMap<Arc<str>, usize>>,
    /// The line to select and show when done (Show in Context), else
    /// the selected one stays selected if it's still there.
    select: Option<usize>,
}

/// How long a slice of filtering goes on for before frames get their
/// turn (`Filter`).
const FILTER_SLICE: Duration = Duration::from_millis(2);

/// `[u8; 4]` of a colour, for mixing.
fn rgba(h: Hsla) -> [u8; 4] {
    let r = olo_kit::Rgba::from(h);
    [(r.r * 255.0).round() as u8, (r.g * 255.0).round() as u8, (r.b * 255.0).round() as u8, (r.a * 255.0).round() as u8]
}

/// `text` highlighted in the palette's colours (highlight.rs), on `base`,
/// what `query`'s words find marked.
fn highlighted(text: &str, font: &Font, base: Hsla, p: &Palette, query: &search::Query) -> Vec<TextRun> {
    use highlight::Kind;
    use olo_theme::mix;
    let bg = p.document_background;
    let text_rgba = rgba(base);
    let color = |k: Kind| match k {
        Kind::Plain => base,
        Kind::Target => c(mix(text_rgba, bg, 0.45)),
        Kind::Punct => c(mix(text_rgba, bg, 0.3)),
        Kind::Key => c(p.keyboard_navigation),
        Kind::Str => c(p.success),
        Kind::Number => c(mix(p.failure, p.keyboard_navigation, 0.5)),
        Kind::Path => c(mix(p.success, p.keyboard_navigation, 0.5)),
        Kind::Type => c(mix(p.failure, p.tab_focused, 0.5)),
        Kind::Bad => c(p.failure),
    };
    let spans: Vec<(Range<usize>, Hsla)> = highlight::spans(text).into_iter().map(|(r, k)| (r, color(k))).collect();
    search::runs(text, font, base, &spans, &search::found(text, query), search::mark_color(p))
}

/// A line's message as its row shows it: its first line, and an
/// ellipsis if there is more.
/// A line's Message cell: its first line, "…" after it when there's
/// more. (Made shared at once: copied once, not into a `String` and then
/// again.)
fn message_text(l: &Line) -> SharedString {
    let first = l.first_line();
    if l.message.len() > first.len() { format!("{first} …").into() } else { SharedString::new(first) }
}

fn line_text(l: &Line, midnight: u64) -> String {
    let pid = if l.pid != 0 { format!("[{}]", l.pid) } else { String::new() };
    format!("{} {}{pid} {}: {}", model::format_time(l.time, midnight), l.program, l.level.name(), l.message)
}

impl Logs {
    fn new(settings: Settings, source: Option<PathBuf>, cx: &mut Cx<Self>) -> Logs {
        let query = Query::from_text(&settings.search);
        let search = search_field(cx, "Search: words, program:, level:, since:");
        let text = settings.search.clone();
        search.update(cx, |s, cx| s.set_text(text, cx));
        cx.subscribe(&search, |_: &mut Logs, e: &SearchEvent, cx| {
            if let SearchEvent::Changed = e {
                cx.send(Msg::Searched);
            }
        });
        let detail = text_view(cx, |v| v.mono(true).empty_text("Select a line to read it whole."));
        let mut logs = Logs {
            settings,
            lines: Vec::new(),
            view: Vec::new(),
            warnings: 0,
            errors: 0,
            query,
            programs: BTreeMap::new(),
            programs_since: None,
            facets: None,
            filter: None,
            filters: 0,
            ticks: 0,
            ticking: false,
            now: Now::get(),
            before_context: None,
            list: ListModel::new(true),
            search,
            detail,
            selected: None,
            at_end: true,
            loaded: false,
            failed: None,
            source,
            generation: Arc::new(AtomicU64::new(0)),
            panel: None,
            screen_logs: (None, Vec::new()),
        };
        logs.start(cx);
        logs.tick(cx);
        logs
    }

    fn store(&self) {
        olo_kit::store::save(settings_path(), &self.settings);
    }

    fn title(&self) -> String {
        title_of(self.source.as_deref())
    }

    /// Read the source from the start, on a worker (the one before's
    /// reader stopping).
    fn start(&mut self, cx: &mut Cx<Self>) {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.lines.clear();
        self.view.clear();
        (self.warnings, self.errors) = (0, 0);
        self.programs.clear();
        self.programs_since = None;
        self.facets = None;
        self.filter = None;
        self.before_context = None;
        *self.list.state_mut() = ListState { multiple: true, ..ListState::default() };
        self.selected = None;
        self.at_end = true;
        self.loaded = false;
        self.failed = None;
        self.show_detail(cx);
        let (to, current, source) = (cx.messenger(), self.generation.clone(), self.source.clone());
        std::thread::Builder::new()
            .name("journal".into())
            .spawn(move || {
                let mut on = |u: Update| {
                    if current.load(Ordering::SeqCst) != generation {
                        return false;
                    }
                    // Nothing new: nothing to send.
                    if !matches!(&u, Update::Lines { new, more } if new.is_empty() && more.is_empty()) {
                        return to.send(Msg::Read(generation, u));
                    }
                    true
                };
                match &source {
                    Some(path) => reader::run_file(path, true, &mut on),
                    None => reader::run(true, &mut on),
                }
            })
            .expect("thread");
        cx.set_window_title(self.title());
        cx.invalidate();
    }

    /// Read `source` (None: the journal) instead.
    fn read(&mut self, source: Option<PathBuf>, cx: &mut Cx<Self>) {
        if source != self.source {
            self.source = source;
            self.start(cx);
        }
    }

    fn count(&mut self, i: usize) {
        match self.lines[i].level {
            Level::Warning => self.warnings += 1,
            Level::Error => self.errors += 1,
            _ => {}
        }
    }

    /// Choose the lines shown again (the query changed, or time moved
    /// on), a slice at a time from now (`Filter`); then select line
    /// `select`.
    fn refilter(&mut self, select: Option<usize>, cx: &mut Cx<Self>) {
        self.now = Now::get();
        let since = self.query.since;
        let programs = (self.programs_since != Some(since) || matches!(since, Since::Minutes(_))).then(BTreeMap::new);
        self.programs_since = Some(since);
        self.filter = Some(Filter { query: self.query.clone(), now: self.now, next: 0, view: Vec::new(), warnings: 0, errors: 0, programs, select });
        // A slice, then frames' turn, until done.
        self.filters += 1;
        cx.send(Msg::FilterOn(self.filters));
        self.tick(cx);
        cx.invalidate();
    }

    /// A sliding window of time ("Last 10 Minutes") is chosen again every
    /// five seconds while it's asked for (nothing ticks otherwise).
    fn tick(&mut self, cx: &mut Cx<Self>) {
        self.ticks += 1;
        self.ticking = matches!(self.query.since, Since::Minutes(_));
        if self.ticking {
            cx.after(Duration::from_secs(5), Msg::Tick(self.ticks));
        }
    }

    /// Go through lines for the filter under way, for a slice; whether
    /// it's done.
    fn filter_on(&mut self, cx: &mut Cx<Self>) -> bool {
        let Some(f) = self.filter.as_mut() else { return true };
        let start = Instant::now();
        while f.next < self.lines.len() {
            let end = (f.next + 4096).min(self.lines.len());
            for (i, l) in self.lines[f.next..end].iter().enumerate() {
                if let Some(programs) = &mut f.programs
                    && f.query.in_time(l, &f.now)
                {
                    match programs.get_mut(&l.program) {
                        Some(n) => *n += 1,
                        None => {
                            programs.insert(l.program.clone(), 1);
                        }
                    }
                }
                if f.query.matches(l, &f.now) {
                    f.view.push(f.next + i);
                    match l.level {
                        Level::Warning => f.warnings += 1,
                        Level::Error => f.errors += 1,
                        _ => {}
                    }
                }
            }
            f.next = end;
            if start.elapsed() > FILTER_SLICE {
                return false;
            }
        }
        let f = self.filter.take().expect("a filter");
        (self.view, self.warnings, self.errors) = (f.view, f.warnings, f.errors);
        if let Some(programs) = f.programs {
            self.programs = programs;
            self.facets = None;
        }
        if let Some(line) = f.select {
            self.selected = Some(line);
        }
        if let Some(sel) = self.selected
            && self.view.binary_search(&sel).is_err()
        {
            self.selected = None;
        }
        // The rows were of the lines shown before.
        let row = self.selected.and_then(|s| self.view.binary_search(&s).ok());
        {
            let mut state = self.list.state_mut();
            state.selected = row.into_iter().collect();
            state.anchor = row;
            state.cursor = row;
        }
        // (The last row, where the list was following: once the rows it
        // has now are laid out.)
        match row {
            Some(r) => self.list.reveal(cx, r),
            None if self.at_end => self.list.scroll_to_end(cx),
            None => {}
        }
        self.show_detail(cx);
        cx.invalidate();
        true
    }

    fn query_changed(&mut self, cx: &mut Cx<Self>) {
        self.before_context = None;
        self.query = Query::from_text(&self.settings.search);
        self.refilter(None, cx);
        self.store();
    }

    /// The search's text set from elsewhere (a filter's menu, Show in
    /// Context, Back): the field shows it.
    fn set_search(&mut self, text: String, cx: &mut Cx<Self>) {
        self.settings.search = text.clone();
        self.search.update(cx, |s, cx| s.set_text(text, cx));
    }

    fn take(&mut self, new: Vec<Line>, more: Vec<(usize, String)>, cx: &mut Cx<Self>) {
        let following = self.list.at_end();
        let mut detail = false;
        for (index, text) in more {
            if let Some(l) = self.lines.get_mut(index) {
                l.message.push('\n');
                l.message.push_str(&text);
                detail |= self.selected == Some(index);
            }
        }
        for l in new {
            if self.query.in_time(&l, &self.now) {
                if !self.programs.contains_key(&l.program) {
                    self.facets = None;
                }
                *self.programs.entry(l.program.clone()).or_default() += 1;
            }
            let i = self.lines.len();
            self.lines.push(l);
            if self.query.matches(&self.lines[i], &self.now) {
                self.view.push(i);
                self.count(i);
            }
        }
        if detail {
            self.show_detail(cx);
        }
        self.at_end = following;
        if following && self.settings.live && !self.view.is_empty() {
            self.list.scroll_to_end(cx);
        }
        cx.invalidate();
    }

    /// The selected line, whole, in the pane under the list.
    fn show_detail(&mut self, cx: &mut Cx<Self>) {
        let text = self
            .selected
            .and_then(|i| self.lines.get(i))
            .map(|l| {
                let pid = if l.pid != 0 { format!(" (process {})", l.pid) } else { String::new() };
                let when = model::format_time(l.time, 0);
                format!("{when} · {}{pid} · {}\n\n{}", l.program, l.level.name(), l.message)
            })
            .unwrap_or_default();
        let p = theme::palette();
        let words = self.query.words.clone();
        self.detail.update(cx, |d, cx| {
            let runs = (!text.is_empty()).then(|| highlighted(&text, &d.font(), c(p.document_text), &p, &words));
            d.set_text(text, runs, cx);
        });
    }

    /// The list's selection changed.
    fn selection_changed(&mut self, cx: &mut Cx<Self>) {
        let line = self.list.state().single().and_then(|r| self.view.get(r).copied());
        self.selected = line;
        self.show_detail(cx);
        cx.invalidate();
    }

    /// The selected lines, as text.
    fn selected_text(&self) -> String {
        self.list.state().selected.iter().filter_map(|&r| self.view.get(r)).map(|&i| line_text(&self.lines[i], self.now.midnight)).collect::<Vec<_>>().join("\n")
    }

    /// Show every line from the selected one's time range, all programs
    /// and levels, around it.
    fn show_in_context(&mut self, cx: &mut Cx<Self>) {
        let Some(sel) = self.selected else { return };
        let saved = std::mem::take(&mut self.settings.search);
        self.set_search("program:all level:debug since:all".into(), cx);
        self.query = Query::from_text(&self.settings.search);
        self.before_context = Some(saved);
        self.refilter(Some(sel), cx);
    }

    fn back_from_context(&mut self, cx: &mut Cx<Self>) {
        if let Some(q) = self.before_context.take() {
            self.set_search(q, cx);
            self.query = Query::from_text(&self.settings.search);
            self.refilter(None, cx);
        }
    }

    fn do_command(&mut self, command: &str, cx: &mut Cx<Self>) {
        match command {
            "copy" => {
                let text = self.selected_text();
                if !text.is_empty() {
                    clipboard::write(ClipboardItem::new_string(text));
                }
            }
            "select-all" => {
                let n = self.view.len();
                self.list.state_mut().select_all(n);
                cx.invalidate();
            }
            "find" => self.search.update(cx, |s, cx| s.focus(cx)),
            "live" => {
                self.settings.live = !self.settings.live;
                if self.settings.live {
                    self.at_end = true;
                    if !self.view.is_empty() {
                        self.list.scroll_to_end(cx);
                    }
                }
                self.store();
                cx.invalidate();
            }
            "context" => self.show_in_context(cx),
            "back" => self.back_from_context(cx),
            "only-program" => {
                if let Some(sel) = self.selected {
                    let program = self.lines[sel].program.to_string();
                    let mut text = self.settings.search.clone();
                    search::set_filter(&mut text, model::KEYS, "program", &[&program]);
                    self.set_search(text, cx);
                    self.query_changed(cx);
                }
            }
            "end" => {
                self.at_end = true;
                if !self.view.is_empty() {
                    self.list.scroll_to_end(cx);
                }
                cx.invalidate();
            }
            "journal" => self.read(None, cx),
            "open-file" => {
                if olo_kit::raise(self.panel.as_ref()) {
                    return;
                }
                let dir = self.source.as_ref().and_then(|p| p.parent()).map_or_else(|| std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from), PathBuf::from);
                let panel = FilePanel::open(dir).filters(vec![file_panel::Filter::new("Log Files").glob("*.log").glob("*.txt").glob("*.out"), file_panel::Filter::new("All Files").glob("*")], 0);
                // (The panel answers on its own looper: it tells this
                // window.)
                let to = cx.messenger();
                self.panel = Some(file_panel::show("Open Log File", panel, move |a| {
                    to.send(Msg::Opened(match a.result {
                        PanelResult::Chosen(path) => Some(path),
                        _ => None,
                    }));
                }));
            }
            "close" => {
                self.store();
                cx.close_window();
            }
            "quit" => {
                self.store();
                olo_kit::quit();
            }
            c => {
                if let Some(path) = c.strip_prefix("file:") {
                    self.read(Some(PathBuf::from(path)), cx);
                }
            }
        }
    }

    /// The search's filters: Program (Olo's first, each with its lines in
    /// the query's time), Level and Time in the bar; `pid:` typed.
    fn facets(&mut self, cx: &mut Cx<Self>) -> Rc<Vec<Facet>> {
        if let Some(f) = &self.facets {
            return f.clone();
        }
        let mut program = Facet::new("program", "Program").any("Olo Programs").multiple().value("all", "All Programs", None);
        let mut names: Vec<(&Arc<str>, &usize)> = self.programs.iter().collect();
        names.sort_by_key(|(n, _)| (!model::is_olo(n), n.to_lowercase()));
        for (name, n) in names {
            program = program.value(name.to_string(), name.to_string(), Some(*n));
        }
        let mut level = Facet::new("level", "Level").any(LEVELS[1].1);
        for ((_, label), code) in LEVELS.iter().zip(LEVEL_CODES) {
            level = level.value(code, *label, None);
        }
        let mut time = Facet::new("since", "Time").any("Since Startup");
        for (since, label) in TIMES {
            time = time.value(since.code(), label, None);
        }
        let facets = Rc::new(vec![program, level, time, Facet::new("pid", "Process").typed_only()]);
        self.facets = Some(facets.clone());
        let shown = facets.clone();
        self.search.update(cx, |s, cx| s.set_facets(shown, cx));
        facets
    }

    fn menus(&mut self) -> Vec<Menu> {
        // (Looked for again at most every two seconds.)
        if self.screen_logs.0.is_none_or(|t| t.elapsed().as_secs() >= 2) {
            self.screen_logs = (Some(Instant::now()), screen_logs());
        }
        let mut screens = Menu::new("Agent Screen Logs");
        for (name, path) in &self.screen_logs.1 {
            screens = screens.item(name.as_str(), format!("file:{}", path.display())).checked(self.source.as_ref() == Some(path));
        }
        if self.screen_logs.1.is_empty() {
            screens = screens.item("No Agent Screens", "").enabled(false);
        }
        vec![
            Menu::new("File").app_items(&[]).item("Journal", "journal").checked(self.source.is_none()).item("Open Log File…", "open-file").key("o").submenu(screens).close_quit(),
            Menu::new("Edit").item("Copy", "copy").key("c").enabled(self.selected.is_some()).item("Select All", "select-all").key("a").separator().item("Find…", "find").key("f"),
            Menu::new("View")
                .item("Live", "live")
                .key("l")
                .checked(self.settings.live)
                .item("Go to End", "end")
                .key("e")
                .separator()
                .item("Show in Context", "context")
                .key("k")
                .enabled(self.selected.is_some())
                .item("Back", "back")
                .key("[")
                .enabled(self.before_context.is_some()),
        ]
    }

    /// What the bar under the list says: the counts, and how it's going.
    fn status(&self) -> String {
        let n = self.view.len();
        let mut status = format!("{n} {}", if n == 1 { "line" } else { "lines" });
        if self.warnings > 0 {
            status += &format!(", {} {}", self.warnings, if self.warnings == 1 { "warning" } else { "warnings" });
        }
        if self.errors > 0 {
            status += &format!(", {} {}", self.errors, if self.errors == 1 { "error" } else { "errors" });
        }
        if !self.loaded && self.failed.is_none() {
            status += if self.source.is_some() { " (reading…)" } else { " (reading the journal…)" };
        } else if self.filter.is_some() {
            status += " (finding…)";
        }
        if self.before_context.is_some() {
            status = format!("All lines around the selected one. {status}");
        }
        status
    }

    fn list_view(&mut self, cx: &mut Cx<Self>) -> Element {
        // The message as wide as the rest of the list allows: the last
        // column fills what the others leave, and the list scrolls
        // sideways only when the window is too narrow for 200 points of
        // it. (Not worked out from the window's width: a live resize then
        // lays the window out again without making it again.)
        let columns = vec![Column::new("Time", 150.0), Column::new("Program", 132.0), Column::new("Level", 70.0), Column::new("Message", MESSAGE_LEAST)];
        let empty = match (self.loaded, &self.source) {
            (true, _) => "No lines match.",
            (false, None) => "Reading the journal…",
            (false, Some(_)) => "Reading…",
        };
        ColumnListView::new(cx, "logs-lines", &self.list, columns, self.view.len(), |logs: &Logs, cell, g| {
            let Some(l) = logs.view.get(cell.row).and_then(|&i| logs.lines.get(i)) else { return };
            // (The program's name is shared already and the level's is
            // fixed: neither is copied for a cell.)
            let text: SharedString = match cell.column {
                0 => model::format_time(l.time, logs.now.midnight).into(),
                1 => SharedString::new(l.program.clone()),
                2 => SharedString::new_static(l.level.name()),
                _ => message_text(l),
            };
            let p = theme::palette();
            match cell.column {
                2 => {
                    let color = match l.level {
                        _ if cell.selected => cell.text_color,
                        Level::Error => c(p.failure),
                        Level::Warning => c(p.warning),
                        _ => cell.text_color,
                    };
                    cell.text_with(g, text, style::ui_font(FontWeight::NORMAL), color);
                }
                3 if !cell.selected => {
                    let runs = highlighted(&text, &style::ui_font(FontWeight::NORMAL), cell.text_color, &p, &logs.query.words);
                    cell.text_runs(g, text, &runs);
                }
                _ => cell.text(g, text),
            }
        })
        .empty_text(empty)
        // A line is as it was read: drawn again only when what the search
        // marks in it, or the day it's counted from, changes (typing in
        // the search redraws only the lines it marks).
        .row_key(|logs: &Logs, row| {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            if let Some(&i) = logs.view.get(row)
                && let Some(l) = logs.lines.get(i)
            {
                let marked = search::found(l.first_line(), &logs.query.words);
                (i, l.time, l.pid, l.message.as_ptr() as usize, l.message.len(), logs.now.midnight, marked).hash(&mut h);
            }
            h.finish()
        })
        .on_select(cx.msg_with(|_: &()| Msg::Selected))
        .on_context(cx.msg_with(|(row, at): &(usize, Point<Pixels>)| Msg::Context(*row, *at)))
        .into_element()
    }
}

/// The window's title for what it reads.
fn title_of(source: Option<&std::path::Path>) -> String {
    match source {
        Some(path) => format!("{} — {TITLE}", path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()),
        None => TITLE.into(),
    }
}

/// One line of words in the panel's colour, cut to its width, said to
/// agents as a label.
fn said(text: String, centred: bool) -> Element {
    let look = style::key((&text, centred));
    canvas(move |g, b| {
        let p = theme::palette();
        let (x, align) = if centred { (b.center().x, style::Align::Center) } else { (b.left(), style::Align::Left) };
        style::label_fit(g, x, b.center().y, align, text.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), c(p.panel_text), b.size.width);
        agent::record(g, agent::Node::label(text.clone(), b));
    })
    .paint_key(look)
}

impl View for Logs {
    type Message = Msg;

    fn message(&mut self, message: Msg, cx: &mut Cx<Self>) {
        match message {
            Msg::Read(generation, update) => {
                if generation != self.generation.load(Ordering::SeqCst) {
                    return;
                }
                match update {
                    Update::Lines { new, more } => self.take(new, more, cx),
                    Update::Loaded => {
                        self.loaded = true;
                        self.at_end = true;
                        if !self.view.is_empty() {
                            self.list.scroll_to_end(cx);
                        }
                        cx.invalidate();
                    }
                    Update::Failed(why) => {
                        self.failed = Some(why);
                        cx.invalidate();
                    }
                }
            }
            // A slice, then everything else's turn (the looper's queue),
            // until done.
            Msg::FilterOn(n) => {
                if n == self.filters && !self.filter_on(cx) {
                    cx.send(Msg::FilterOn(n));
                }
            }
            Msg::Tick(n) => {
                if n != self.ticks || !self.ticking {
                    return;
                }
                if self.filter.is_none() {
                    self.refilter(None, cx);
                } else {
                    self.tick(cx);
                }
            }
            Msg::Searched => {
                self.settings.search = self.search.read(cx).text(cx).to_owned();
                self.query_changed(cx);
            }
            Msg::Selected => self.selection_changed(cx),
            Msg::Context(row, at) => {
                if !self.list.state().selected.contains(&row) {
                    self.list.state_mut().select(row);
                }
                self.selection_changed(cx);
                let program = self.view.get(row).map(|&i| self.lines[i].program.to_string()).unwrap_or_default();
                let menu = Menu::new("").item("Copy", "copy").item("Show in Context", "context").item(format!("Show Only {program}"), "only-program");
                let chose = cx.msg_with(|command: &str| Msg::Command(command.to_owned()));
                menu::context_menu(cx, menu, at, chose);
            }
            Msg::Command(command) => self.do_command(&command, cx),
            Msg::Opened(path) => {
                self.panel = None;
                if let Some(path) = path {
                    self.read(Some(path), cx);
                }
            }
            Msg::Launch(query, file) => {
                // A log file to read (from Tracker, or `olo-logs FILE`).
                if let Some(path) = file {
                    self.read(Some(path), cx);
                }
                // A search to show (Olo Doctor's "Viewer crashed").
                if let Some(q) = query {
                    self.set_search(q, cx);
                    self.query_changed(cx);
                }
                cx.activate_window();
            }
        }
    }

    fn command(&mut self, command: &str, cx: &mut Cx<Self>) -> bool {
        self.do_command(command, cx);
        true
    }

    /// Its settings kept as it ends, by hand or for an upgrade.
    fn window_closing(&mut self, _: &mut Cx<Self>) {
        self.store();
    }

    fn save_state(&mut self, _: &mut Cx<Self>) -> Option<String> {
        self.store();
        Some(String::new())
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let menus = self.menus();
        let bar = MenuBar::new(cx, menus);

        // The search and the filters' menus: in one row when there's
        // room, else the menus under the search (the kit's search header).
        let facets = self.facets(cx);
        // (The width it was laid out at: the window's only until it was.)
        let width = search::header_width(cx, "logs").unwrap_or_else(|| f32::from(cx.viewport_size().width) - 2.0 * m::WINDOW_INSET);
        let controls = search::header(cx, &self.search, "logs", facets, width).min_field(260.0).indent(search::list_indent(cx));

        let list = match &self.failed {
            Some(why) => div().flex_1().min_h_0().child(said(why.clone(), true).size_full()),
            None => {
                let list = self.list_view(cx);
                div().flex_1().min_h_0().child(list)
            }
        };

        // The counts, and Live; Back after Show in Context.
        let mut foot = div().flex().flex_row().items_center().gap(px(m::SPACING)).h(px(m::CONTROL_HEIGHT));
        if self.before_context.is_some() {
            foot = foot.child(div().w(px(80.0)).child(Button::new("Back").on_click(cx.msg_with(|_: &ClickEvent| Msg::Command("back".into())))));
        }
        foot = foot.child(said(self.status(), false).flex_1().h_full());
        foot = foot.child(div().w(px(64.0)).flex().justify_center().child(CheckBox::new("Live", self.settings.live).on_toggle(cx.msg_with(|_: &bool| Msg::Command("live".into())))));

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(bar)
            .child(div().flex_1().min_h_0().p(px(m::WINDOW_INSET)).flex().flex_col().gap(px(m::SPACING)).child(controls).child(list).child(div().h(px(DETAIL)).child(&self.detail)).child(foot))
    }
}

/// `--print`: the query's lines on standard output, for agents and scripts.
/// The window's arguments: `--query TEXT` (the search to show) and a
/// log file to read.
fn window_args(args: &[String]) -> (Option<String>, Option<String>) {
    let (mut query, mut file) = (None, None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--query" => query = it.next().cloned(),
            a if !a.starts_with('-') && file.is_none() => file = Some(a.to_owned()),
            _ => {}
        }
    }
    (query, file)
}

fn print(args: &[String]) -> i32 {
    // The flags say it as filters, and --query as typed in the search.
    let mut text = String::new();
    let (mut json, mut follow) = (false, false);
    let mut file: Option<PathBuf> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().cloned().unwrap_or_default();
        match a.as_str() {
            "--print" => {}
            "--json" => json = true,
            "--follow" => follow = true,
            "--file" => file = Some(PathBuf::from(value())),
            "--all-programs" => search::set_filter(&mut text, model::KEYS, "program", &["all"]),
            "--since" => match Since::parse(&value()) {
                Some(s) => search::set_filter(&mut text, model::KEYS, "since", &[&s.code()]),
                None => return usage(),
            },
            "--level" => match Level::parse(&value()) {
                Some(l) => search::set_filter(&mut text, model::KEYS, "level", &[LEVEL_CODES[LEVELS.iter().position(|(x, _)| *x == l).unwrap_or(1)]]),
                None => return usage(),
            },
            "--program" => search::set_filter(&mut text, model::KEYS, "program", &[&value()]),
            "--find" => text = format!("{text} {}", search::quote(&value())),
            "--query" => text = format!("{text} {}", value()),
            _ => return usage(),
        }
    }
    let now = Now::get();
    let q = Query::from_text(&text);
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    let mut lines: Vec<Line> = Vec::new();
    let mut loaded = false;
    let mut printed = 0;
    let mut status = 0;
    // Continuations arrive after their line; print a line once the next
    // batch can no longer add to it (all at the end, or as followed).
    let mut flush = |lines: &mut Vec<Line>, upto: usize, out: &mut std::io::StdoutLock| -> bool {
        for l in &lines[printed.min(upto)..upto] {
            if !q.matches(l, &now) {
                continue;
            }
            let text = if json {
                serde_json::json!({
                    "time": iso_time(l.time),
                    "program": &*l.program,
                    "pid": l.pid,
                    "level": l.level,
                    "message": l.message,
                })
                .to_string()
            } else {
                line_text(l, now.midnight)
            };
            if writeln!(out, "{text}").is_err() {
                return false;
            }
        }
        printed = printed.max(upto);
        out.flush().is_ok()
    };
    let mut on = |u: Update| match u {
        Update::Lines { new, more } => {
            let idle = new.is_empty() && more.is_empty();
            for (i, text) in more {
                if let Some(l) = lines.get_mut(i) {
                    l.message.push('\n');
                    l.message.push_str(&text);
                }
            }
            lines.extend(new);
            if loaded {
                // Following: hold back the newest line until a poll finds
                // nothing more (its continuations come with it or not at all).
                let upto = if idle { lines.len() } else { lines.len().saturating_sub(1) };
                return flush(&mut lines, upto, &mut out);
            }
            true
        }
        Update::Loaded => {
            loaded = true;
            let upto = lines.len();
            flush(&mut lines, upto, &mut out)
        }
        Update::Failed(why) => {
            eprintln!("olo-logs: {why}");
            status = 1;
            false
        }
    };
    match &file {
        Some(path) => reader::run_file(path, follow, &mut on),
        None => reader::run(follow, &mut on),
    }
    status
}

fn usage() -> i32 {
    eprintln!(
        "usage: olo-logs --print [--query TEXT] [--since 10m|2h|1d|today|boot|all] [--level debug|info|warning|error]\n                    [--program NAME | --all-programs] [--find TEXT] [--json] [--follow] [--file LOG]\n       olo-logs [--query TEXT] [LOG]   (the journal, or a log file such as an Agent Screen's)\n  TEXT as in the search: words, \"phrases\", -word, program:a,b (or all), -program:x, level:, since:, pid:"
    );
    2
}

/// `micros` as UTC, ISO 8601 with milliseconds.
fn iso_time(micros: u64) -> String {
    let t = (micros / 1_000_000) as libc::time_t;
    // SAFETY: gmtime_r writes into the zeroed tm we own.
    let tm = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        libc::gmtime_r(&t, &mut tm);
        tm
    };
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec, (micros / 1000) % 1000)
}

fn main() {
    olo_crash::install();
    // A session or install from before the rename (Tabula) carries on.
    olo_xdg::legacy::adopt();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "--print") {
        std::process::exit(print(&args));
    }
    if instance::forward(APP_ID, &args) {
        return;
    }
    olo_kit::run(APP_ID, move |app| {
        instance::listen(APP_ID, olo_kit::proxy());
        let mut settings: Settings = olo_kit::store::load(&settings_path()).unwrap_or_default();
        // From before the search was one text.
        if let Some(old) = settings.query.take() {
            settings.search = old.text();
        }
        // A search's words are for now; its filters stay.
        settings.search = search::filters_only(&settings.search, model::KEYS);
        let (query, file) = window_args(&args);
        if let Some(q) = query {
            settings.search = q;
        }
        // A log file named on the command line, else the journal.
        let source = file.map(PathBuf::from).map(|p| std::fs::canonicalize(&p).unwrap_or(p));
        let title = title_of(source.as_deref());
        let window = olo_kit::open_window(WindowOptions::new(title, 820.0, 520.0).min_size(560.0, 340.0).keep_size("main"), move |cx| Logs::new(settings, source, cx));
        // Launched again: the window comes forward, with the search and
        // the log asked for.
        olo_kit::message_handler(app, move |message, _| {
            if let Ok(launch) = message.downcast::<Launch>() {
                let (query, file) = window_args(&launch.args);
                let file = file.map(|a| launch.cwd.as_ref().map_or_else(|| PathBuf::from(&a), |c| c.join(&a)));
                window.messenger().send(Msg::Launch(query, file));
            }
        });
    });
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore = "reads this machine's journal"]
    fn every_real_line_highlights() {
        let mut n = 0;
        super::reader::run(false, &mut |u| {
            if let super::Update::Lines { new, more } = u {
                for l in &new {
                    let s = super::highlight::spans(&l.message);
                    assert_eq!(s.last().map_or(0, |(r, _)| r.end), l.message.len());
                    n += 1;
                }
                for (_, t) in &more {
                    super::highlight::spans(t);
                }
            }
            true
        });
        assert!(n > 0);
    }

    #[test]
    fn iso_times() {
        assert_eq!(super::iso_time(1_793_085_657_797_247), "2026-10-27T07:20:57.797Z");
    }

    /// A log file is read whole, then followed as it grows.
    #[test]
    fn reads_and_follows_a_file() {
        let dir = std::path::Path::new("/var/tmp").join(format!("olo-logs-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("agent-olo-chrome.log");
        std::fs::write(&path, "2026-10-30T10:00:00Z  INFO olo: one\npanicked at src/main.rs:3\n    a backtrace line\n").unwrap();
        let mut lines: Vec<super::Line> = Vec::new();
        let mut appended = false;
        super::reader::run_file(&path, true, &mut |u| match u {
            super::Update::Lines { new, more } => {
                for (i, t) in more {
                    lines[i].message.push('\n');
                    lines[i].message.push_str(&t);
                }
                lines.extend(new);
                // Stop once the appended line has come.
                lines.len() < 3
            }
            super::Update::Loaded => {
                assert_eq!(lines.len(), 2);
                assert!(lines[1].message.ends_with("a backtrace line"));
                use std::io::Write;
                let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
                f.write_all(b"two\n").unwrap();
                appended = true;
                true
            }
            super::Update::Failed(why) => panic!("{why}"),
        });
        assert!(appended);
        assert_eq!(&*lines[0].program, "agent-olo-chrome");
        assert_eq!((lines[1].level, lines[2].message.as_str()), (super::Level::Error, "two"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
