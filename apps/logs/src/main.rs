//! Log Viewer: every Olo program's log in one place, from the user's
//! journal (read natively, journal.rs, on a worker thread, reader.rs),
//! live as it is written. One search (the kit's query language: words,
//! "phrases", -words, and `program:` several or `all`, `-program:`,
//! `level:`, `since:`, `pid:`) with the filters as menus beside it; a
//! line's full text below the list, and Show in Context for the lines
//! around one.
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
    cell::RefCell,
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};

use model::{Level, Line, Now, Query, Since};
use reader::Update;
use serde::{Deserialize, Serialize};
use olo_kit::{
    App, Cx, Menu, WindowId, WindowOptions,
    egui::{self, Align, Id, Key, Rect, Ui, pos2, vec2},
    file_panel::{self, FilePanel, PanelResult},
    instance::{self, Launch},
    metrics as m,
    style::{self, c},
    search::{self, Facet, FilterBar, SearchField},
    widgets::{self, Button, Cell, CheckBox, Column, ColumnListView, ListState, TextView},
};

const APP_ID: &str = "olo.logs";
const TITLE: &str = "Log Viewer";

const LEVELS: [(Level, &str); 4] =
    [(Level::Debug, "Everything"), (Level::Info, "Info and Above"), (Level::Warning, "Warnings and Errors"), (Level::Error, "Errors Only")];
const LEVEL_CODES: [&str; 4] = ["debug", "info", "warning", "error"];
const TIMES: [(Since, &str); 5] = [
    (Since::Minutes(10), "Last 10 Minutes"),
    (Since::Minutes(60), "Last Hour"),
    (Since::Today, "Today"),
    (Since::Boot, "Since Startup"),
    (Since::All, "Everything"),
];

/// The height of the pane under the list with the selected line in full.
const DETAIL: f32 = 112.0;

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
    olo_kit::config_dir().join("logs.json")
}

struct Logs {
    main: Option<WindowId>,
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
    facets: RefCell<Option<Vec<Facet>>>,
    /// Lines being chosen again, a slice a frame (`refilter`).
    filter: Option<Filter>,
    now: Now,
    filtered_at: Instant,
    /// The search from before Show in Context, to go back to.
    before_context: Option<String>,
    list: ListState,
    /// The selected line (an index into `lines`).
    selected: Option<usize>,
    reveal: Option<usize>,
    /// Whether the list showed its last row (live lines keep it there).
    at_end: bool,
    /// The first row the list showed, and how many.
    shown: Option<(usize, usize)>,
    loaded: bool,
    failed: Option<String>,
    detail: String,
    /// What the lines are of: the journal, or a log file an app writes
    /// beside it (an Agent Screen's).
    source: Option<PathBuf>,
    /// Bumped when the source changes: the reader of the one before stops,
    /// and what it sent is dropped.
    generation: Arc<AtomicU64>,
    /// File ▸ Open Log File's panel.
    panel: Option<(WindowId, FilePanel)>,
    /// The Agent Screens' logs, as last looked for (File ▸ Agent Screen
    /// Logs), and when.
    screen_logs: (Option<Instant>, Vec<(String, PathBuf)>),
}

/// An update from the reader of source `generation` ([`Logs::start`]).
struct Tagged(u64, Update);

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
/// lines, and a keystroke in Find must not hold a frame up. The lines
/// shown change once it has been through them all.
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
    /// Once through them all: the rows (of `view`) that will show, and
    /// the next of them to shape. Shaping a screen of new lines took 10
    /// ms of the frame that showed them; shaped here, a slice at a time,
    /// that frame only breaks them into rows (epaint keeps shaped text a
    /// while, by its words and fonts).
    warm: Option<(std::ops::Range<usize>, usize)>,
}

/// How long a frame goes through lines for (`Filter`).
const FILTER_SLICE: std::time::Duration = std::time::Duration::from_millis(2);

/// `text` highlighted in the palette's colours (highlight.rs), on `base`,
/// what `query`'s words find marked.
fn highlighted(text: &str, font: egui::FontId, base: egui::Color32, p: &olo_theme::Palette, query: &search::Query) -> egui::text::LayoutJob {
    use highlight::Kind;
    use olo_theme::mix;
    let bg = p.document_background;
    let text_rgba = [base.r(), base.g(), base.b(), 255];
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
    let mut job = egui::text::LayoutJob::default();
    for (r, k) in highlight::spans(text) {
        job.append(&text[r], 0.0, egui::TextFormat { font_id: font.clone(), color: color(k), ..Default::default() });
    }
    search::mark(&mut job, &search::found(text, query), search::mark_color(p));
    job
}


/// A line's message as its row shows it: its first line, and an
/// ellipsis if there is more.
fn message_text(l: &Line) -> String {
    let first = l.first_line();
    if l.message.len() > first.len() { format!("{first} …") } else { first.to_owned() }
}

fn line_text(l: &Line, midnight: u64) -> String {
    let pid = if l.pid != 0 { format!("[{}]", l.pid) } else { String::new() };
    format!("{} {}{pid} {}: {}", model::format_time(l.time, midnight), l.program, l.level.name(), l.message)
}

impl Logs {
    fn store(&self) {
        let text = serde_json::to_string_pretty(&self.settings).unwrap_or_default();
        std::thread::spawn(move || {
            let path = settings_path();
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(path, text);
        });
    }

    fn open(&mut self, cx: &mut Cx) {
        if let Some(id) = self.main {
            return cx.activate(id);
        }
        self.main = Some(cx.open_window(WindowOptions::new(self.title(), 820, 520).min_size(560, 340)));
    }

    fn title(&self) -> String {
        match &self.source {
            Some(path) => format!("{} — {TITLE}", path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()),
            None => TITLE.into(),
        }
    }

    /// Read the source from the start, on a worker (the one before's
    /// reader stopping).
    fn start(&mut self, cx: &mut Cx) {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.lines.clear();
        self.view.clear();
        (self.warnings, self.errors) = (0, 0);
        self.programs.clear();
        self.programs_since = None;
        *self.facets.borrow_mut() = None;
        self.filter = None;
        self.before_context = None;
        self.list = ListState { multiple: true, ..ListState::default() };
        (self.selected, self.reveal, self.shown) = (None, None, None);
        self.at_end = true;
        self.loaded = false;
        self.failed = None;
        self.detail.clear();
        let (proxy, current, source) = (cx.proxy(), self.generation.clone(), self.source.clone());
        std::thread::Builder::new()
            .name("journal".into())
            .spawn(move || {
                let mut on = |u: Update| {
                    if current.load(Ordering::SeqCst) != generation {
                        return false;
                    }
                    // Nothing new: nothing to send.
                    if !matches!(&u, Update::Lines { new, more } if new.is_empty() && more.is_empty()) {
                        proxy.send(Tagged(generation, u));
                    }
                    true
                };
                match &source {
                    Some(path) => reader::run_file(path, true, &mut on),
                    None => reader::run(true, &mut on),
                }
            })
            .expect("thread");
        if let Some(id) = self.main {
            cx.set_title(id, self.title());
            cx.redraw(id);
        }
    }

    /// Read `source` (None: the journal) instead.
    fn read(&mut self, cx: &mut Cx, source: Option<PathBuf>) {
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
    /// on), from this frame on (`Filter`); then select line `select`.
    fn refilter(&mut self, select: Option<usize>) {
        self.now = Now::get();
        self.filtered_at = Instant::now();
        let since = self.query.since;
        let programs = (self.programs_since != Some(since) || matches!(since, Since::Minutes(_))).then(BTreeMap::new);
        self.programs_since = Some(since);
        self.filter = Some(Filter {
            query: self.query.clone(),
            now: self.now,
            next: 0,
            view: Vec::new(),
            warnings: 0,
            errors: 0,
            programs,
            select,
            warm: None,
        });
    }

    /// Go through lines for the filter under way, for a slice of a frame;
    /// whether it's done.
    fn filter_on(&mut self, ctx: &egui::Context) -> bool {
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
        // The rows that will show: where the list is, at its end if it
        // was there, or around the line to be shown.
        let (rows, (first, shown)) = (f.view.len(), self.shown.unwrap_or((0, 60)));
        let (range, next) = f.warm.get_or_insert_with(|| {
            let n = shown + 2;
            let at = f.select.and_then(|line| f.view.binary_search(&line).ok()).map(|row| row.saturating_sub(n / 2));
            let from = at.unwrap_or(if self.at_end { rows.saturating_sub(n) } else { first.min(rows.saturating_sub(n)) });
            let range = from..(from + n).min(rows);
            (range.clone(), range.start)
        });
        let (p, font) = (style::palette(ctx), style::plain_font());
        while *next < range.end {
            let l = &self.lines[f.view[*next]];
            *next += 1;
            // As the cells lay them out (`Cell::text_job`), but for the
            // width they're cut to and the colours, which shaping doesn't
            // depend on.
            let mut message = highlighted(&message_text(l), font.clone(), egui::Color32::WHITE, &p, &f.query.words);
            let mut time = egui::text::LayoutJob::simple_singleline(model::format_time(l.time, f.now.midnight), font.clone(), egui::Color32::WHITE);
            message.break_on_newline = false;
            time.break_on_newline = false;
            ctx.fonts_mut(|fonts| {
                fonts.layout_job(message);
                fonts.layout_job(time);
            });
            if start.elapsed() > FILTER_SLICE {
                return false;
            }
        }
        let f = self.filter.take().expect("a filter");
        (self.view, self.warnings, self.errors) = (f.view, f.warnings, f.errors);
        if let Some(programs) = f.programs {
            self.programs = programs;
            *self.facets.borrow_mut() = None;
        }
        if let Some(line) = f.select {
            self.select(Some(line));
        }
        if let Some(sel) = self.selected
            && self.view.binary_search(&sel).is_err()
        {
            self.selected = None;
            self.detail.clear();
        }
        // The rows were of the lines shown before.
        let row = self.selected.and_then(|s| self.view.binary_search(&s).ok());
        self.list.selected = row.into_iter().collect();
        self.list.anchor = row;
        self.reveal = row.or(if self.at_end { self.view.len().checked_sub(1) } else { None });
        true
    }

    fn query_changed(&mut self) {
        self.before_context = None;
        self.query = Query::from_text(&self.settings.search);
        self.refilter(None);
        self.store();
    }

    fn take(&mut self, new: Vec<Line>, more: Vec<(usize, String)>) {
        for (index, text) in more {
            if let Some(l) = self.lines.get_mut(index) {
                l.message.push('\n');
                l.message.push_str(&text);
                if self.selected == Some(index) {
                    self.select(Some(index));
                }
            }
        }
        for l in new {
            if self.query.in_time(&l, &self.now) {
                if !self.programs.contains_key(&l.program) {
                    *self.facets.borrow_mut() = None;
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
        if self.at_end && self.settings.live {
            self.reveal = self.view.len().checked_sub(1);
        }
    }

    fn select(&mut self, line: Option<usize>) {
        self.selected = line;
        self.detail = line
            .map(|i| {
                let l = &self.lines[i];
                let pid = if l.pid != 0 { format!(" (process {})", l.pid) } else { String::new() };
                let when = model::format_time(l.time, 0);
                format!("{when} · {}{pid} · {}\n\n{}", l.program, l.level.name(), l.message)
            })
            .unwrap_or_default();
    }

    /// The selected lines, as text.
    fn selected_text(&self) -> String {
        self.list.selected.iter().filter_map(|&r| self.view.get(r)).map(|&i| line_text(&self.lines[i], self.now.midnight)).collect::<Vec<_>>().join("\n")
    }

    /// Show every line from the selected one's time range, all programs
    /// and levels, around it.
    fn show_in_context(&mut self) {
        let Some(sel) = self.selected else { return };
        let saved = std::mem::replace(&mut self.settings.search, "program:all level:debug since:all".into());
        self.query = Query::from_text(&self.settings.search);
        self.before_context = Some(saved);
        self.refilter(Some(sel));
    }

    fn back_from_context(&mut self) {
        if let Some(q) = self.before_context.take() {
            self.settings.search = q;
            self.query = Query::from_text(&self.settings.search);
            self.refilter(None);
        }
    }

    fn command(&mut self, cx: &mut Cx, command: &str) {
        match command {
            "copy" => {
                let text = self.selected_text();
                if !text.is_empty() {
                    cx.copy(text);
                }
            }
            "select-all" => {
                self.list.selected = (0..self.view.len()).collect();
            }
            "live" => {
                self.settings.live = !self.settings.live;
                if self.settings.live {
                    self.at_end = true;
                    self.reveal = self.view.len().checked_sub(1);
                }
                self.store();
            }
            "context" => self.show_in_context(),
            "back" => self.back_from_context(),
            "only-program" => {
                if let Some(sel) = self.selected {
                    let program = self.lines[sel].program.to_string();
                    search::set_filter(&mut self.settings.search, model::KEYS, "program", &[&program]);
                    self.query_changed();
                }
            }
            "end" => {
                self.at_end = true;
                self.reveal = self.view.len().checked_sub(1);
            }
            "journal" => self.read(cx, None),
            "open-file" => {
                if let Some((id, _)) = &self.panel {
                    return cx.activate(*id);
                }
                let dir = self.source.as_ref().and_then(|p| p.parent()).map_or_else(|| std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from), PathBuf::from);
                let panel = FilePanel::open(dir).filters(vec![file_panel::Filter::new("Log Files").glob("*.log").glob("*.txt").glob("*.out"), file_panel::Filter::new("All Files").glob("*")], 0);
                let id = cx.open_window(WindowOptions::new("Open Log File", file_panel::PANEL_SIZE.0, file_panel::PANEL_SIZE.1).min_size(420, 300));
                self.panel = Some((id, panel));
            }
            "quit" => cx.quit(),
            c => {
                if let Some(path) = c.strip_prefix("file:") {
                    self.read(cx, Some(PathBuf::from(path)));
                }
            }
        }
    }

    /// The search's filters: Program (Olo's first, each with its lines in
    /// the query's time), Level and Time in the bar; `pid:` typed.
    fn facets(&self) -> std::cell::Ref<'_, Vec<Facet>> {
        if self.facets.borrow().is_none() {
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
            *self.facets.borrow_mut() = Some(vec![program, level, time, Facet::new("pid", "Process").typed_only()]);
        }
        std::cell::Ref::map(self.facets.borrow(), |f| f.as_ref().expect("made above"))
    }

    /// The search and the filters' menus: in one row when there's room,
    /// else the menus under the search. How tall that is.
    fn controls(&mut self, ui: &mut Ui, top: Rect) -> f32 {
        let facets = self.facets().clone();
        let bar_w = FilterBar::width(ui, &self.settings.search, &facets) + m::SPACING + 40.0;
        let one_row = top.width() - bar_w - m::SPACING >= 260.0;
        let field = if one_row { Rect::from_min_size(top.min, vec2(top.width() - bar_w - m::SPACING, m::CONTROL_HEIGHT)) } else { Rect::from_min_size(top.min, vec2(top.width(), m::CONTROL_HEIGHT)) };
        let bar = if one_row {
            Rect::from_min_max(pos2(field.right() + m::SPACING, top.top()), pos2(top.right(), top.top() + m::CONTROL_HEIGHT))
        } else {
            // Under the search, its first menu's text over the list's
            // first column's (inside the list's two lines of frame).
            let frame = style::inset(ui.ctx(), top, 2.0).left() - top.left();
            let indent = frame + m::CELL_INSET - m::TEXT_INSET;
            Rect::from_min_size(pos2(top.left() + indent, field.bottom() + m::SPACING), vec2(top.width() - indent, m::CONTROL_HEIGHT))
        };
        let hint = "Search: words, program:, level:, since:";
        let mut changed = ui.put(field, SearchField::new("logs", &mut self.settings.search).hint(hint).facets(&facets)).changed();
        changed |= ui.put(bar, FilterBar::new("logs", &mut self.settings.search, &facets)).changed();
        if changed {
            self.query_changed();
        }
        bar.bottom() - top.top()
    }

    fn list_ui(&mut self, cx: &mut Cx, ui: &mut Ui, area: Rect) {
        let p = style::palette(ui.ctx());
        let columns = [
            Column::new("Time", 150.0),
            Column::new("Program", 132.0),
            Column::new("Level", 70.0),
            Column::new("Message", (area.width() - 368.0).max(200.0)),
        ];
        let (warn, err) = (
            c(p.warning),
            c(p.failure),
        );
        let reveal = self.reveal.take();
        let empty = match (self.loaded, &self.source) {
            (true, _) => "No lines match.",
            (false, None) => "Reading the journal…",
            (false, Some(_)) => "Reading…",
        };
        let (lines, view, midnight, query) = (&self.lines, &self.view, self.now.midnight, &self.query);
        // (A line's cells are kept by its index, which starts again with
        // another source.)
        let source = self.generation.load(Ordering::SeqCst).rotate_left(48);
        // (Rows' messages kept by the words they mark, too.)
        let marks = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            query.words.terms.iter().flat_map(|t| &t.any).for_each(|n| n.as_str().hash(&mut h));
            h.finish()
        };
        let out = ui
            .scope_builder(egui::UiBuilder::new().max_rect(area), |ui| {
                ColumnListView::new("logs-lines", &columns, view.len(), &mut self.list).reveal(reveal).empty_text(empty).show(ui, |cell: &Cell| {
                    let l = &lines[view[cell.row]];
                    let color = match l.level {
                        _ if cell.selected => cell.text_color,
                        Level::Error => err,
                        Level::Warning => warn,
                        _ => cell.text_color,
                    };
                    let font = style::plain_font();
                    let plain = |text: String, color| egui::text::LayoutJob::simple_singleline(text, font.clone(), color);
                    // Each cell kept while its line is the same (a line
                    // grows when more of its message comes).
                    let line = view[cell.row] as u64 ^ source;
                    match cell.column {
                        0 => cell.text_cached(line ^ midnight.rotate_left(32), || plain(model::format_time(l.time, midnight), cell.text_color)),
                        1 => cell.text_cached(line, || plain(l.program.to_string(), cell.text_color)),
                        2 => cell.text_cached(line, || plain(l.level.name().to_owned(), color)),
                        _ => cell.text_cached(line ^ (l.message.len() as u64).rotate_left(40) ^ marks, || {
                            let text = message_text(l);
                            if cell.selected { plain(text, cell.text_color) } else { highlighted(&text, font.clone(), cell.text_color, &p, &query.words) }
                        }),
                    }
                })
            })
            .inner;
        self.at_end = self.view.is_empty() || out.visible.iter().any(|(row, _)| *row + 1 == self.view.len());
        self.shown = out.visible.first().map(|(row, _)| (*row, out.visible.len()));
        if out.selection_changed || out.context.is_some() {
            let line = self.list.single().and_then(|r| self.view.get(r).copied());
            self.select(line);
        }
        if let Some((row, pos)) = out.context {
            if !self.list.selected.contains(&row) {
                self.list.select(row);
            }
            self.select(self.view.get(row).copied());
            let menu = Menu::new("")
                .item("Copy", "copy", None)
                .item("Show in Context", "context", None)
                .item(format!("Show Only {}", self.lines[self.view[row]].program), "only-program", None);
            widgets::popup_menu_at(ui.ctx(), Id::unique("logs-row-menu"), pos, menu);
        }
        if let Some(cmd) = widgets::popup_result(ui.ctx(), Id::unique("logs-row-menu")) {
            self.command(cx, &cmd);
        }
    }

    fn main_ui(&mut self, cx: &mut Cx, ui: &mut Ui) {
        // A sliding window of time moves on by itself.
        if matches!(self.query.since, Since::Minutes(_)) && self.filtered_at.elapsed().as_secs() >= 5 && self.filter.is_none() {
            self.refilter(None);
        }
        let area = ui.max_rect().shrink(m::WINDOW_INSET);
        let height = self.controls(ui, Rect::from_min_size(area.min, vec2(area.width(), m::CONTROL_HEIGHT)));
        let top = Rect::from_min_size(area.min, vec2(area.width(), height));
        let bottom = Rect::from_min_max(pos2(area.left(), area.bottom() - m::CONTROL_HEIGHT), area.max);
        let detail_r = Rect::from_min_max(pos2(area.left(), bottom.top() - m::SPACING - DETAIL), pos2(area.right(), bottom.top() - m::SPACING));
        let list_r = Rect::from_min_max(pos2(area.left(), top.bottom() + m::SPACING), pos2(area.right(), detail_r.top() - m::SPACING));
        let p = style::palette(ui.ctx());
        if let Some(why) = &self.failed {
            style::label(ui.painter(), list_r.center().x, list_r.center().y, Align::Center, why, style::plain_font(), c(p.panel_text));
        } else {
            self.list_ui(cx, ui, list_r);
        }
        let words = self.query.words.clone();
        let highlight = |text: &str, font: egui::FontId, base: egui::Color32| highlighted(text, font, base, &p, &words);
        ui.scope_builder(egui::UiBuilder::new().max_rect(detail_r), |ui| {
            TextView::new("logs-detail", &mut self.detail).mono(true).editable(false).empty_text("Select a line to read it whole.").highlight(&highlight).show(ui);
        });

        // The counts, and Live; Back after Show in Context.
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
        let mut x = bottom.left();
        if self.before_context.is_some() {
            let r = Rect::from_min_size(bottom.min, vec2(80.0, m::CONTROL_HEIGHT));
            if ui.put(r, Button::new("Back")).clicked() {
                self.back_from_context();
            }
            x = r.right() + m::SPACING;
            status = format!("All lines around the selected one. {status}");
        }
        style::label(ui.painter(), x, bottom.center().y, Align::Min, status, style::plain_font(), c(p.panel_text));
        let live_r = Rect::from_min_size(pos2(bottom.right() - 64.0, bottom.top()), vec2(64.0, m::CONTROL_HEIGHT));
        let mut live = self.settings.live;
        if ui.put(live_r, CheckBox::new(&mut live, "Live")).changed() {
            self.command(cx, "live");
        }
        // Lines chosen again for a changed query, a slice of the frame
        // (last, so a keystroke's starts in its own frame); shown the
        // next.
        if self.filter.is_some() {
            self.filter_on(ui.ctx());
            ui.ctx().request_repaint();
        }
    }
}

impl App for Logs {
    fn ui(&mut self, cx: &mut Cx, window: WindowId, ui: &mut Ui) {
        if Some(window) == self.main {
            self.main_ui(cx, ui);
        } else if let Some((id, panel)) = &mut self.panel
            && *id == window
            && let Some(result) = panel.ui(ui)
        {
            self.panel = None;
            cx.close_window(window);
            if let PanelResult::Chosen(path) = result {
                self.read(cx, Some(path));
            }
        }
    }

    fn menu_bar(&mut self, window: WindowId) -> Option<Vec<Menu>> {
        if self.panel.as_ref().is_some_and(|(id, _)| *id == window) {
            return None;
        }
        let any = !self.list.selected.is_empty();
        // (Looked for again at most every two seconds: the bar is built
        // every frame.)
        if self.screen_logs.0.is_none_or(|t| t.elapsed().as_secs() >= 2) {
            self.screen_logs = (Some(Instant::now()), screen_logs());
        }
        let mut screens = Menu::new("Agent Screen Logs");
        for (name, path) in &self.screen_logs.1 {
            screens = screens.item(name.as_str(), format!("file:{}", path.display()), None).checked(self.source.as_ref() == Some(path));
        }
        if self.screen_logs.1.is_empty() {
            screens = screens.item("No Agent Screens", "", None).enabled(false);
        }
        Some(vec![
            Menu::new("File")
                .item("Journal", "journal", None)
                .checked(self.source.is_none())
                .item("Open Log File…", "open-file", Some(Key::O))
                .submenu(screens)
                .separator()
                .item("Quit", "quit", Some(Key::Q)),
            Menu::new("Edit").item("Copy", "copy", Some(Key::C)).enabled(any).item("Select All", "select-all", Some(Key::A)),
            Menu::new("View")
                .item("Live", "live", Some(Key::L))
                .checked(self.settings.live)
                .item("Go to End", "end", Some(Key::E))
                .separator()
                .item("Show in Context", "context", Some(Key::K))
                .enabled(self.selected.is_some())
                .item("Back", "back", Some(Key::OpenBracket))
                .enabled(self.before_context.is_some()),
        ])
    }

    fn command(&mut self, cx: &mut Cx, _window: WindowId, command: &str) {
        Logs::command(self, cx, command);
    }

    fn close_requested(&mut self, cx: &mut Cx, window: WindowId) {
        if self.panel.as_ref().is_some_and(|(id, _)| *id == window) {
            self.panel = None;
            return cx.close_window(window);
        }
        self.store();
        cx.quit();
    }

    fn message(&mut self, cx: &mut Cx, message: Box<dyn std::any::Any + Send>) {
        if let Some(launch) = message.downcast_ref::<Launch>() {
            // A log file to read (from Tracker, or `olo-logs FILE`).
            if let Some(a) = launch.args.iter().find(|a| !a.starts_with('-')) {
                let path = launch.cwd.as_ref().map_or_else(|| PathBuf::from(a), |c| c.join(a));
                self.read(cx, Some(path));
            }
            return self.open(cx);
        }
        let Ok(tagged) = message.downcast::<Tagged>() else { return };
        let Tagged(generation, update) = *tagged;
        if generation != self.generation.load(Ordering::SeqCst) {
            return;
        }
        match update {
            Update::Lines { new, more } => self.take(new, more),
            Update::Loaded => {
                self.loaded = true;
                self.at_end = true;
                self.reveal = self.view.len().checked_sub(1);
            }
            Update::Failed(why) => self.failed = Some(why),
        }
        if let Some(id) = self.main {
            cx.redraw(id);
        }
    }

    fn save_state(&mut self, _cx: &Cx) -> Option<String> {
        self.store();
        Some(String::new())
    }
}

/// `--print`: the query's lines on standard output, for agents and scripts.
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
        "usage: olo-logs --print [--query TEXT] [--since 10m|2h|1d|today|boot|all] [--level debug|info|warning|error]\n                    [--program NAME | --all-programs] [--find TEXT] [--json] [--follow] [--file LOG]\n       olo-logs [LOG]   (the journal, or a log file such as an Agent Screen's)\n  TEXT as in the search: words, \"phrases\", -word, program:a,b (or all), -program:x, level:, since:, pid:"
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
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec,
        (micros / 1000) % 1000
    )
}

fn main() {
    // A session or install from before the rename (Tabula) carries on.
    olo_kit::legacy::adopt();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "--print") {
        std::process::exit(print(&args));
    }
    if instance::forward(APP_ID, &args) {
        return;
    }
    olo_kit::run(APP_ID, |cx| {
        instance::listen(APP_ID, cx.proxy());
        let mut settings: Settings = std::fs::read_to_string(settings_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        // From before the search was one text.
        if let Some(old) = settings.query.take() {
            settings.search = old.text();
        }
        // A search's words are for now; its filters stay.
        settings.search = search::filters_only(&settings.search, model::KEYS);
        let query = Query::from_text(&settings.search);
        let mut app = Logs {
            main: None,
            settings,
            lines: Vec::new(),
            view: Vec::new(),
            warnings: 0,
            errors: 0,
            query,
            programs: BTreeMap::new(),
            programs_since: None,
            facets: RefCell::new(None),
            filter: None,
            now: Now::get(),
            filtered_at: Instant::now(),
            before_context: None,
            list: ListState { multiple: true, ..ListState::default() },
            selected: None,
            reveal: None,
            at_end: true,
            shown: None,
            loaded: false,
            failed: None,
            detail: String::new(),
            // A log file named on the command line, else the journal.
            source: args.iter().find(|a| !a.starts_with('-')).map(PathBuf::from).map(|p| std::fs::canonicalize(&p).unwrap_or(p)),
            generation: Arc::new(AtomicU64::new(0)),
            panel: None,
            screen_logs: (None, Vec::new()),
        };
        app.start(cx);
        app.open(cx);
        app
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
