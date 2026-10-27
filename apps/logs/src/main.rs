//! Log Viewer: every Tabula program's log in one place, from the user's
//! journal (read natively, journal.rs, on a worker thread, reader.rs),
//! live as it is written. Find text, show one program or all of them,
//! warnings and errors only, the last ten minutes or since startup; a
//! line's full text below the list, and Show in Context for the lines
//! around one.
//!
//! For agents the same query prints from the command line:
//! `tabula-logs --print [--since 10m|2h|today|boot|all] [--level
//! info|warning|error] [--program NAME | --all-programs] [--find TEXT]
//! [--json] [--follow]`.

mod highlight;
mod journal;
mod model;
mod reader;

use std::{collections::BTreeSet, sync::Arc, time::Instant};

use model::{Level, Line, Now, Query, Since};
use reader::Update;
use serde::{Deserialize, Serialize};
use tabula_kit::{
    App, Cx, Menu, WindowId, WindowOptions,
    egui::{self, Align, Id, Key, Rect, Ui, pos2, vec2},
    instance::{self, Launch},
    metrics as m,
    style::{self, c},
    widgets::{self, Button, Cell, CheckBox, Column, ColumnListView, ListState, MenuField, TextControl, TextView},
};

const APP_ID: &str = "tabula.logs";
const TITLE: &str = "Log Viewer";

const LEVELS: [(Level, &str); 4] =
    [(Level::Debug, "Everything"), (Level::Info, "Info and Above"), (Level::Warning, "Warnings and Errors"), (Level::Error, "Errors Only")];
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
    query: Query,
    live: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { query: Query::default(), live: true }
    }
}

fn settings_path() -> std::path::PathBuf {
    tabula_kit::config_dir().join("logs.json")
}

struct Logs {
    main: Option<WindowId>,
    settings: Settings,
    lines: Vec<Line>,
    /// The lines shown (indices into `lines`).
    view: Vec<usize>,
    warnings: usize,
    errors: usize,
    programs: BTreeSet<Arc<str>>,
    now: Now,
    filtered_at: Instant,
    /// The query from before Show in Context, to go back to.
    before_context: Option<Query>,
    list: ListState,
    /// The selected line (an index into `lines`).
    selected: Option<usize>,
    reveal: Option<usize>,
    /// Whether the list showed its last row (live lines keep it there).
    at_end: bool,
    loaded: bool,
    failed: Option<String>,
    detail: String,
}

/// `text` highlighted in the palette's colours (highlight.rs), on `base`.
fn highlighted(text: &str, font: egui::FontId, base: egui::Color32, p: &tabula_theme::Palette) -> egui::text::LayoutJob {
    use highlight::Kind;
    use tabula_theme::mix;
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
    job
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
        self.main = Some(cx.open_window(WindowOptions::new(TITLE, 820, 520).min_size(560, 340)));
    }

    fn count(&mut self, i: usize) {
        match self.lines[i].level {
            Level::Warning => self.warnings += 1,
            Level::Error => self.errors += 1,
            _ => {}
        }
    }

    /// Choose the lines shown again (the query changed, or time moved on).
    fn refilter(&mut self) {
        self.now = Now::get();
        self.filtered_at = Instant::now();
        let needle = self.settings.query.needle();
        self.view = (0..self.lines.len()).filter(|&i| self.settings.query.matches(&self.lines[i], &self.now, &needle)).collect();
        self.programs = self.lines.iter().filter(|l| self.settings.query.in_time(l, &self.now)).map(|l| l.program.clone()).collect();
        (self.warnings, self.errors) = (0, 0);
        for k in 0..self.view.len() {
            self.count(self.view[k]);
        }
        if let Some(sel) = self.selected
            && !self.view.contains(&sel)
        {
            self.selected = None;
            self.detail.clear();
        }
        self.reveal = self.selected.and_then(|s| self.view.binary_search(&s).ok()).or(if self.at_end { self.view.len().checked_sub(1) } else { None });
    }

    fn query_changed(&mut self) {
        self.before_context = None;
        self.refilter();
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
        let needle = self.settings.query.needle();
        for l in new {
            if !self.programs.contains(&l.program) && self.settings.query.in_time(&l, &self.now) {
                self.programs.insert(l.program.clone());
            }
            let i = self.lines.len();
            self.lines.push(l);
            if self.settings.query.matches(&self.lines[i], &self.now, &needle) {
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
        let saved = self.settings.query.clone();
        self.settings.query = Query { since: Since::All, level: Level::Debug, program: None, all_programs: true, text: String::new() };
        self.before_context = Some(saved);
        self.refilter();
        self.list.selected.clear();
        if let Ok(row) = self.view.binary_search(&sel) {
            self.list.select(row);
            self.reveal = Some(row);
        }
    }

    fn back_from_context(&mut self) {
        if let Some(q) = self.before_context.take() {
            self.settings.query = q;
            self.refilter();
            self.list.selected = self.selected.and_then(|s| self.view.binary_search(&s).ok()).into_iter().collect();
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
                    self.settings.query.program = Some(self.lines[sel].program.to_string());
                    self.query_changed();
                }
            }
            "end" => {
                self.at_end = true;
                self.reveal = self.view.len().checked_sub(1);
            }
            "quit" => cx.quit(),
            _ => {}
        }
    }

    fn controls(&mut self, ui: &mut Ui, top: Rect) {
        let q = &mut self.settings.query;
        let w_menu = 170.0;
        let find_r = Rect::from_min_size(top.min, vec2(top.width() - 3.0 * (w_menu + m::SPACING), m::CONTROL_HEIGHT));
        let before = q.text.clone();
        ui.put(find_r, TextControl::new(&mut q.text).label("Find:").hint("Text or program"));
        let mut changed = q.text != before;

        // Which programs: Tabula's, every one, or one of Tabula's seen in
        // this time (any other through Find, or a line's Show Only).
        let mut names: Vec<String> = self.programs.iter().filter(|p| model::is_tabula(p)).map(|p| p.to_string()).collect();
        if let Some(p) = &q.program
            && !names.contains(p)
        {
            names.push(p.clone());
        }
        names.sort_by_key(|n| (!model::is_tabula(n), n.to_lowercase()));
        let mut options: Vec<&str> = vec!["Tabula Programs", "All Programs"];
        options.extend(names.iter().map(String::as_str));
        let mut which = match &q.program {
            Some(p) => names.iter().position(|n| n == p).map_or(0, |i| i + 2),
            None => usize::from(q.all_programs),
        };
        let r = Rect::from_min_size(pos2(find_r.right() + m::SPACING, top.top()), vec2(w_menu, m::CONTROL_HEIGHT));
        if ui.put(r, MenuField::new("logs-program", &mut which, &options).width(w_menu)).changed() {
            q.all_programs = which == 1;
            q.program = which.checked_sub(2).and_then(|i| names.get(i).cloned());
            changed = true;
        }

        let labels: Vec<&str> = LEVELS.iter().map(|(_, l)| *l).collect();
        let mut level = LEVELS.iter().position(|(l, _)| *l == q.level).unwrap_or(1);
        let r = r.translate(vec2(w_menu + m::SPACING, 0.0));
        if ui.put(r, MenuField::new("logs-level", &mut level, &labels).width(w_menu)).changed() {
            q.level = LEVELS[level].0;
            changed = true;
        }

        let labels: Vec<&str> = TIMES.iter().map(|(_, l)| *l).collect();
        let mut time = TIMES.iter().position(|(t, _)| *t == q.since).unwrap_or(3);
        let r = r.translate(vec2(w_menu + m::SPACING, 0.0));
        if ui.put(r, MenuField::new("logs-time", &mut time, &labels).width(w_menu)).changed() {
            q.since = TIMES[time].0;
            changed = true;
        }
        if changed {
            self.query_changed();
        }
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
            c(match p.mode {
                tabula_theme::ThemeMode::Light => p.tab_focused_shadow,
                tabula_theme::ThemeMode::Dark => p.tab_focused_light,
            }),
            c(p.failure),
        );
        let reveal = self.reveal.take();
        let empty = if self.loaded { "No lines match." } else { "Reading the journal…" };
        let (lines, view, midnight) = (&self.lines, &self.view, self.now.midnight);
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
                    match cell.column {
                        0 => cell.text_with(&model::format_time(l.time, midnight), font, cell.text_color),
                        1 => cell.text(&l.program),
                        2 => cell.text_with(l.level.name(), font, color),
                        _ => {
                            let first = l.first_line();
                            let more = l.message.len() > first.len();
                            let text = if more { format!("{first} …") } else { first.to_owned() };
                            if cell.selected {
                                cell.text_with(&text, font, cell.text_color);
                            } else {
                                cell.text_job(highlighted(&text, font, cell.text_color, &p));
                            }
                        }
                    }
                })
            })
            .inner;
        self.at_end = self.view.is_empty() || out.visible.iter().any(|(row, _)| *row + 1 == self.view.len());
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
        if matches!(self.settings.query.since, Since::Minutes(_)) && self.filtered_at.elapsed().as_secs() >= 5 {
            self.refilter();
        }
        let area = ui.max_rect().shrink(m::WINDOW_INSET);
        let top = Rect::from_min_size(area.min, vec2(area.width(), m::CONTROL_HEIGHT));
        self.controls(ui, top);
        let bottom = Rect::from_min_max(pos2(area.left(), area.bottom() - m::CONTROL_HEIGHT), area.max);
        let detail_r = Rect::from_min_max(pos2(area.left(), bottom.top() - m::SPACING - DETAIL), pos2(area.right(), bottom.top() - m::SPACING));
        let list_r = Rect::from_min_max(pos2(area.left(), top.bottom() + m::SPACING), pos2(area.right(), detail_r.top() - m::SPACING));
        let p = style::palette(ui.ctx());
        if let Some(why) = &self.failed {
            style::label(ui.painter(), list_r.center().x, list_r.center().y, Align::Center, why, style::plain_font(), c(p.panel_text));
        } else {
            self.list_ui(cx, ui, list_r);
        }
        let highlight = |text: &str, font: egui::FontId, base: egui::Color32| highlighted(text, font, base, &p);
        ui.scope_builder(egui::UiBuilder::new().max_rect(detail_r), |ui| {
            TextView::new("logs-detail", &mut self.detail).mono(true).editable(false).highlight(&highlight).show(ui);
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
            status += " (reading the journal…)";
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
    }
}

impl App for Logs {
    fn ui(&mut self, cx: &mut Cx, window: WindowId, ui: &mut Ui) {
        if Some(window) == self.main {
            self.main_ui(cx, ui);
        }
    }

    fn menu_bar(&mut self, _window: WindowId) -> Option<Vec<Menu>> {
        let any = !self.list.selected.is_empty();
        Some(vec![
            Menu::new("File").item("Quit", "quit", Some(Key::Q)),
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

    fn close_requested(&mut self, cx: &mut Cx, _window: WindowId) {
        self.store();
        cx.quit();
    }

    fn message(&mut self, cx: &mut Cx, message: Box<dyn std::any::Any + Send>) {
        if message.is::<Launch>() {
            return self.open(cx);
        }
        let Ok(update) = message.downcast::<Update>() else { return };
        match *update {
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
    let mut q = Query { level: Level::Info, ..Query::default() };
    let (mut json, mut follow) = (false, false);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().cloned().unwrap_or_default();
        match a.as_str() {
            "--print" => {}
            "--json" => json = true,
            "--follow" => follow = true,
            "--all-programs" => q.all_programs = true,
            "--since" => match Since::parse(&value()) {
                Some(s) => q.since = s,
                None => return usage(),
            },
            "--level" => match Level::parse(&value()) {
                Some(l) => q.level = l,
                None => return usage(),
            },
            "--program" => q.program = Some(value()),
            "--find" => q.text = value(),
            _ => return usage(),
        }
    }
    let now = Now::get();
    let needle = q.needle();
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
            if !q.matches(l, &now, &needle) {
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
    reader::run(follow, &mut |u| match u {
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
            eprintln!("tabula-logs: {why}");
            status = 1;
            false
        }
    });
    status
}

fn usage() -> i32 {
    eprintln!(
        "usage: tabula-logs --print [--since 10m|2h|1d|today|boot|all] [--level debug|info|warning|error]\n                    [--program NAME | --all-programs] [--find TEXT] [--json] [--follow]"
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
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "--print") {
        std::process::exit(print(&args));
    }
    if instance::forward(APP_ID, &args) {
        return;
    }
    tabula_kit::run(APP_ID, |cx| {
        instance::listen(APP_ID, cx.proxy());
        let mut settings: Settings = std::fs::read_to_string(settings_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        // A search is for now; the filters stay.
        settings.query.text.clear();
        let proxy = cx.proxy();
        std::thread::Builder::new()
            .name("journal".into())
            .spawn(move || {
                reader::run(true, &mut |u| {
                    // Nothing new: nothing to send.
                    if !matches!(&u, Update::Lines { new, more } if new.is_empty() && more.is_empty()) {
                        proxy.send(u);
                    }
                    true
                })
            })
            .expect("thread");
        let mut app = Logs {
            main: None,
            settings,
            lines: Vec::new(),
            view: Vec::new(),
            warnings: 0,
            errors: 0,
            programs: BTreeSet::new(),
            now: Now::get(),
            filtered_at: Instant::now(),
            before_context: None,
            list: ListState { multiple: true, ..ListState::default() },
            selected: None,
            reveal: None,
            at_end: true,
            loaded: false,
            failed: None,
            detail: String::new(),
        };
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
}
