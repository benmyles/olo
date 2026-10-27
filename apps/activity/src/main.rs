//! Activity Monitor: Haiku's ActivityMonitor (graph views of chosen data
//! sources, each with its legend; right-click a graph to choose what it
//! shows, add or remove graphs) beside a process list in the spirit of
//! BeOS's ProcessController (quit or force-quit a process). Everything is
//! measured on a worker thread (sample.rs).

mod sample;

use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use sample::{Group, Proc, Sample, Source};
use serde::{Deserialize, Serialize};
use olo_kit::{
    App, Cx, Menu, WindowId, WindowOptions,
    egui::{self, Align, Color32, Id, Key, Rect, Sense, Shape, Stroke, Ui, pos2, vec2},
    files::format_size,
    instance::{self, Launch},
    metrics as m,
    style::{self, c},
    widgets::{self, Button, Cell, Column, ColumnListView, ListState, MenuField, TabView, TextControl},
};
use olo_theme::mix;

const APP_ID: &str = "olo.activity";

/// How many samples each source remembers.
const HISTORY: usize = 1200;
/// Pixels between samples on a graph.
const STEP: f32 = 2.0;
const LEGEND_ROW: f32 = 16.0;
const INTERVALS: [(u64, &str); 5] = [(250, "¼ second"), (500, "½ second"), (1000, "1 second"), (2000, "2 seconds"), (5000, "5 seconds")];

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    /// Each graph's sources, by key.
    views: Vec<Vec<String>>,
    interval_ms: u64,
    tab: usize,
    sort: Option<(usize, bool)>,
    mine_only: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            views: vec![
                vec!["cpu".into(), "memory".into(), "cached".into()],
                vec!["net-rx".into(), "net-tx".into(), "disk-read".into(), "disk-write".into()],
            ],
            interval_ms: 1000,
            tab: 0,
            sort: Some((3, false)),
            mine_only: false,
        }
    }
}

fn settings_path() -> std::path::PathBuf {
    olo_kit::config_dir().join("activity.json")
}

enum Msg {
    Sources(Vec<Source>),
    Sample(Sample),
}

enum Win {
    Main,
    Confirm { pid: i32, name: String, message: String },
    Error { message: String },
}

struct Activity {
    windows: HashMap<WindowId, Win>,
    main: Option<WindowId>,
    settings: Settings,
    control: sample::Control,
    sources: Vec<Source>,
    history: Vec<VecDeque<f64>>,
    processes: Vec<Proc>,
    list: ListState,
    selected: Option<i32>,
    filter: String,
    uid: u32,
}

/// What a process is doing, from /proc's state letter.
fn state_name(state: char) -> &'static str {
    match state {
        'R' => "Running",
        'S' => "Sleeping",
        'D' => "Waiting",
        'T' | 't' => "Stopped",
        'Z' => "Zombie",
        'I' => "Idle",
        _ => "Other",
    }
}

/// The next "nice" number at or above `v` (1, 2, 5 × 10ⁿ).
fn nice_ceiling(v: f64) -> f64 {
    if v <= 0.0 || !v.is_finite() {
        return 1.0;
    }
    let p = 10f64.powf(v.log10().floor());
    [1.0, 2.0, 5.0, 10.0].iter().map(|k| k * p).find(|&n| n >= v).unwrap_or(10.0 * p)
}

impl Activity {
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
        let id = cx.open_window(WindowOptions::new("Activity Monitor", 600, 460).min_size(400, 320));
        self.windows.insert(id, Win::Main);
        self.main = Some(id);
    }

    fn source(&self, key: &str) -> Option<usize> {
        self.sources.iter().position(|s| s.key == key)
    }

    fn alert(&mut self, cx: &mut Cx, win: Win, message: &str) {
        let id = cx.open_window(WindowOptions::alert("Activity Monitor", message));
        self.windows.insert(id, win);
    }

    /// Send `signal` to the selected process.
    fn signal(&mut self, cx: &mut Cx, pid: i32, name: &str, signal: i32) {
        // SAFETY: kill(2) with a pid and signal number has no memory effects.
        if unsafe { libc::kill(pid, signal) } != 0 {
            let err = std::io::Error::last_os_error();
            let message = match err.raw_os_error() {
                Some(libc::ESRCH) => return,
                Some(libc::EPERM) => format!("“{name}” can't be quit: it belongs to another user."),
                _ => format!("“{name}” couldn't be quit. {err}"),
            };
            self.alert(cx, Win::Error { message: message.clone() }, &message);
        }
    }

    fn selected_proc(&self) -> Option<&Proc> {
        self.selected.and_then(|pid| self.processes.iter().find(|p| p.pid == pid))
    }

    fn quit_selected(&mut self, cx: &mut Cx, force: bool) {
        let Some(p) = self.selected_proc() else { return };
        let (pid, name) = (p.pid, p.name.clone());
        if force {
            let message = format!("Force “{name}” ({pid}) to quit? Any unsaved work in it will be lost.");
            self.alert(cx, Win::Confirm { pid, name, message: message.clone() }, &message);
        } else {
            self.signal(cx, pid, &name, libc::SIGTERM);
        }
    }

    fn set_tab(&mut self, tab: usize) {
        self.settings.tab = tab;
        self.control.want_processes.store(tab == 1, Ordering::Relaxed);
        self.store();
    }

    fn command(&mut self, cx: &mut Cx, command: &str) {
        if let Some(rest) = command.strip_prefix("toggle:")
            && let Some((v, key)) = rest.split_once(':')
            && let Ok(v) = v.parse::<usize>()
            && let Some(view) = self.settings.views.get_mut(v)
        {
            match view.iter().position(|k| k == key) {
                Some(i) => {
                    view.remove(i);
                }
                None => view.push(key.to_owned()),
            }
            return self.store();
        }
        if let Some(v) = command.strip_prefix("remove-graph:").and_then(|v| v.parse::<usize>().ok()) {
            if self.settings.views.len() > 1 && v < self.settings.views.len() {
                self.settings.views.remove(v);
                self.store();
            }
            return;
        }
        if let Some(ms) = command.strip_prefix("interval:").and_then(|v| v.parse::<u64>().ok()) {
            self.settings.interval_ms = ms;
            self.control.interval_ms.store(ms, Ordering::Relaxed);
            return self.store();
        }
        match command {
            "add-graph" => {
                self.settings.views.push(vec!["cpu".into()]);
                self.set_tab(0);
            }
            "graphs" => self.set_tab(0),
            "processes" => self.set_tab(1),
            "quit-process" => self.quit_selected(cx, false),
            "force-quit-process" => self.quit_selected(cx, true),
            "quit" => cx.quit(),
            _ => {}
        }
    }

    /// The context menu of graph `v`.
    fn view_menu(&self, v: usize) -> Menu {
        let shown = &self.settings.views[v];
        let item = |menu: Menu, s: &Source| menu.item(s.name.clone(), format!("toggle:{v}:{}", s.key), None).checked(shown.contains(&s.key));
        let mut menu = Menu::new("");
        let mut cpus = Menu::new("Processors");
        let mut sensors = Menu::new("Sensors");
        for s in &self.sources {
            match s.group {
                Group::Main => menu = item(menu, s),
                Group::Cpus => cpus = item(cpus, s),
                Group::Sensors => sensors = item(sensors, s),
            }
        }
        menu = menu.separator().submenu(cpus);
        if !sensors.items.is_empty() {
            menu = menu.submenu(sensors);
        }
        menu.separator()
            .item("Add Graph", "add-graph", None)
            .item("Remove Graph", format!("remove-graph:{v}"), None)
            .enabled(self.settings.views.len() > 1)
    }

    fn graphs_ui(&mut self, cx: &mut Cx, ui: &mut Ui, area: Rect) {
        let n = self.settings.views.len().max(1);
        let legend_rows: Vec<usize> = self.settings.views.iter().map(|v| v.len().div_ceil(2).max(1)).collect();
        let legends: f32 = legend_rows.iter().map(|&r| r as f32 * LEGEND_ROW + m::TIGHT).sum();
        let gaps = (n - 1) as f32 * m::SPACING;
        let graph_h = ((area.height() - legends - gaps) / n as f32).max(40.0).floor();
        let mut y = area.top();
        let mut chosen = None;
        for (v, &rows) in legend_rows.iter().enumerate() {
            let graph = Rect::from_min_size(pos2(area.left(), y), vec2(area.width(), graph_h));
            let legend = Rect::from_min_size(pos2(area.left(), graph.bottom() + m::TIGHT), vec2(area.width(), rows as f32 * LEGEND_ROW));
            y = legend.bottom() + m::SPACING;
            self.graph(ui, graph, v);
            self.legend(ui, legend, v);
            let resp = ui.interact(graph.union(legend), Id::unique(("activity-graph", v)), Sense::click());
            let menu = self.view_menu(v);
            if let Some(cmd) = widgets::context_menu(&resp, || menu) {
                chosen = Some(cmd);
            }
        }
        if let Some(cmd) = chosen {
            self.command(cx, &cmd);
        }
    }

    fn graph(&self, ui: &Ui, rect: Rect, v: usize) {
        let p = style::palette(ui.ctx());
        let painter = ui.painter();
        let inner = rect.shrink(2.0);
        painter.rect_filled(inner, 0.0, c(p.document_background));
        style::sunken(painter, rect, &p, false);
        let painter = painter.with_clip_rect(inner);
        // A grid in quarters, and time lines every 30 samples that move
        // with the graph.
        let grid = c(mix(p.document_text, p.document_background, 0.9));
        for k in 1..4 {
            let gy = (inner.top() + inner.height() * k as f32 / 4.0).round() + 0.5;
            style::hline(&painter, inner.left(), inner.right(), gy, grid);
        }
        let count = self.history.first().map_or(0, VecDeque::len);
        let samples = (inner.width() / STEP).ceil() as usize + 1;
        for i in (0..samples).filter(|i| (count + samples - i).is_multiple_of(30)) {
            let gx = (inner.right() - i as f32 * STEP).round() + 0.5;
            style::vline(&painter, gx, inner.top(), inner.bottom(), grid);
        }
        for key in &self.settings.views[v] {
            let Some(si) = self.source(key) else { continue };
            let s = &self.sources[si];
            let h = &self.history[si];
            let visible = h.iter().rev().take(samples);
            let max = s.max.unwrap_or_else(|| nice_ceiling(visible.clone().copied().filter(|v| v.is_finite()).fold(0.0, f64::max)));
            let color = Color32::from_rgb(s.color[0], s.color[1], s.color[2]);
            let mut line: Vec<egui::Pos2> = Vec::new();
            for (i, &val) in visible.enumerate() {
                if !val.is_finite() {
                    if line.len() > 1 {
                        painter.add(Shape::line(std::mem::take(&mut line), Stroke::new(1.5, color)));
                    }
                    line.clear();
                    continue;
                }
                let x = inner.right() - 1.0 - i as f32 * STEP;
                let frac = (val / max).clamp(0.0, 1.0) as f32;
                line.push(pos2(x, inner.bottom() - 1.5 - frac * (inner.height() - 3.0)));
            }
            if line.len() > 1 {
                painter.add(Shape::line(line, Stroke::new(1.5, color)));
            }
        }
    }

    fn legend(&self, ui: &Ui, rect: Rect, v: usize) {
        let p = style::palette(ui.ctx());
        let painter = ui.painter();
        let col_w = (rect.width() / 2.0).floor();
        for (i, key) in self.settings.views[v].iter().enumerate() {
            let Some(si) = self.source(key) else { continue };
            let s = &self.sources[si];
            let x = rect.left() + (i % 2) as f32 * (col_w + m::SPACING / 2.0);
            let y = rect.top() + (i / 2) as f32 * LEGEND_ROW;
            let mid = y + LEGEND_ROW / 2.0;
            let swatch = Rect::from_center_size(pos2(x + 5.0, mid), vec2(9.0, 9.0));
            painter.rect_filled(swatch, 0.0, Color32::from_rgb(s.color[0], s.color[1], s.color[2]));
            style::frame_rect(painter, swatch, c(mix(p.panel_text, p.panel_background, 0.4)));
            let font = egui::FontId::proportional(style::SMALL_SIZE);
            style::label(painter, x + 15.0, mid, Align::Min, &s.name, font.clone(), c(p.panel_text));
            let value = self.history[si].back().copied().filter(|v| v.is_finite()).map(|v| s.unit.format(v)).unwrap_or_else(|| "–".into());
            let right = x + col_w - m::SPACING;
            style::label(painter, right, mid, Align::Max, &value, font, c(p.panel_text));
        }
    }

    /// The rows the process list shows, sorted.
    fn rows(&self) -> Vec<&Proc> {
        let filter = self.filter.trim().to_lowercase();
        let mut rows: Vec<&Proc> = self
            .processes
            .iter()
            .filter(|p| !self.settings.mine_only || p.uid == self.uid)
            .filter(|p| filter.is_empty() || p.name.to_lowercase().contains(&filter) || p.pid.to_string() == filter)
            .collect();
        let (col, asc) = self.settings.sort.unwrap_or((3, false));
        rows.sort_by(|a, b| {
            let o = match col {
                0 => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                1 => a.pid.cmp(&b.pid),
                2 => a.user.cmp(&b.user),
                3 => a.cpu.total_cmp(&b.cpu),
                4 => a.memory.cmp(&b.memory),
                5 => a.threads.cmp(&b.threads),
                _ => state_name(a.state).cmp(state_name(b.state)),
            };
            let o = o.then(a.pid.cmp(&b.pid));
            if asc { o } else { o.reverse() }
        });
        rows
    }

    fn processes_ui(&mut self, cx: &mut Cx, ui: &mut Ui, area: Rect) {
        // Search and Show above the list.
        let top = Rect::from_min_size(area.min, vec2(area.width(), m::CONTROL_HEIGHT));
        let search = Rect::from_min_size(top.min, vec2((top.width() * 0.55).floor(), m::CONTROL_HEIGHT));
        ui.put(search, TextControl::new(&mut self.filter).label("Search:").hint("Name or ID"));
        let show_r = Rect::from_min_max(pos2(search.right() + m::SPACING, top.top()), top.max);
        let mut mine = usize::from(self.settings.mine_only);
        if ui.put(show_r, MenuField::new("activity-show", &mut mine, &["All processes", "My processes"]).label("Show:")).changed() {
            self.settings.mine_only = mine == 1;
            self.store();
        }

        let bottom = Rect::from_min_max(pos2(area.left(), area.bottom() - m::CONTROL_HEIGHT), area.max);
        let list_r = Rect::from_min_max(pos2(area.left(), top.bottom() + m::SPACING), pos2(area.right(), bottom.top() - m::SPACING));
        let rows = self.rows();
        let columns = [
            Column::new("Name", 148.0),
            Column::new("ID", 56.0).right(),
            Column::new("User", 56.0),
            Column::new("CPU", 56.0).right(),
            Column::new("Memory", 70.0).right(),
            Column::new("Threads", 64.0).right(),
            Column::new("State", 64.0),
        ];
        let pids: Vec<i32> = rows.iter().map(|p| p.pid).collect();
        let cells: Vec<[String; 7]> = rows
            .iter()
            .map(|p| {
                [
                    p.name.clone(),
                    p.pid.to_string(),
                    p.user.clone(),
                    format!("{:.1}", p.cpu),
                    // Kernel threads have no memory of their own.
                    if p.memory == 0 { "—".into() } else { format_size(p.memory) },
                    p.threads.to_string(),
                    state_name(p.state).to_owned(),
                ]
            })
            .collect();
        let (procs, threads) = (rows.len(), rows.iter().map(|p| p.threads as usize).sum::<usize>());
        drop(rows);
        // Rows move as they re-sort; the selection follows the process,
        // and is dropped when it leaves the list (filtered out or gone),
        // so Quit never acts on a process you can't see.
        if self.selected.is_some_and(|pid| !pids.contains(&pid)) {
            self.selected = None;
        }
        self.list.sort = self.settings.sort;
        self.list.selected = self.selected.and_then(|pid| pids.iter().position(|&p| p == pid)).into_iter().collect();
        let out = ui
            .scope_builder(egui::UiBuilder::new().max_rect(list_r), |ui| {
                ColumnListView::new("activity-processes", &columns, cells.len(), &mut self.list).show(ui, |cell: &Cell| cell.text(&cells[cell.row][cell.column]))
            })
            .inner;
        if out.sort_changed {
            self.settings.sort = self.list.sort;
            self.store();
        }
        if out.selection_changed || out.context.is_some() {
            self.selected = self.list.single().and_then(|r| pids.get(r).copied());
        }
        if let Some((row, pos)) = out.context {
            self.selected = pids.get(row).copied();
            let menu = Menu::new("").item("Quit", "quit-process", None).item("Force Quit…", "force-quit-process", None);
            widgets::popup_menu_at(ui.ctx(), Id::unique("activity-row-menu"), pos, menu);
        }
        if let Some(cmd) = widgets::popup_result(ui.ctx(), Id::unique("activity-row-menu")) {
            self.command(cx, &cmd);
        }

        let p = style::palette(ui.ctx());
        style::label(
            ui.painter(),
            bottom.left(),
            bottom.center().y,
            Align::Min,
            format!("{procs} {}, {threads} {}", if procs == 1 { "process" } else { "processes" }, if threads == 1 { "thread" } else { "threads" }),
            style::plain_font(),
            c(p.panel_text),
        );
        let bw = 96.0;
        let force_r = Rect::from_min_size(pos2(bottom.right() - bw, bottom.top()), vec2(bw, m::CONTROL_HEIGHT));
        let quit_r = force_r.translate(vec2(-(bw + m::SPACING), 0.0));
        let any = self.selected_proc().is_some();
        if ui.add_enabled_ui(any, |ui| ui.put(quit_r, Button::new("Quit"))).inner.clicked() {
            self.quit_selected(cx, false);
        }
        if ui.add_enabled_ui(any, |ui| ui.put(force_r, Button::new("Force Quit…"))).inner.clicked() {
            self.quit_selected(cx, true);
        }
    }

    fn main_ui(&mut self, cx: &mut Cx, ui: &mut Ui) {
        let full = ui.max_rect().shrink(m::TIGHT + 2.0);
        let mut tab = self.settings.tab.min(1);
        let before = tab;
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(full));
        TabView::new(&mut tab, &["Graphs", "Processes"]).show(&mut child, |ui, t| {
            let area = ui.max_rect().shrink(m::WINDOW_INSET - 2.0);
            if self.sources.is_empty() {
                return;
            }
            match t {
                0 => self.graphs_ui(cx, ui, area),
                _ => self.processes_ui(cx, ui, area),
            }
        });
        if tab != before {
            self.set_tab(tab);
        }
    }
}

impl App for Activity {
    fn ui(&mut self, cx: &mut Cx, window: WindowId, ui: &mut Ui) {
        match self.windows.get(&window) {
            Some(Win::Main) => self.main_ui(cx, ui),
            Some(Win::Confirm { pid, name, message }) => {
                let (pid, name, message) = (*pid, name.clone(), message.clone());
                let Some(choice) = widgets::alert(ui, "warning", &message, &["Cancel", "Force Quit"]) else { return };
                self.windows.remove(&window);
                cx.close_window(window);
                if choice == 1 {
                    self.signal(cx, pid, &name, libc::SIGKILL);
                }
            }
            Some(Win::Error { message }) => {
                let message = message.clone();
                if widgets::alert(ui, "stop", &message, &["OK"]).is_some() {
                    self.windows.remove(&window);
                    cx.close_window(window);
                }
            }
            None => {}
        }
    }

    fn menu_bar(&mut self, window: WindowId) -> Option<Vec<Menu>> {
        if !matches!(self.windows.get(&window), Some(Win::Main)) {
            return None;
        }
        let tab = self.settings.tab;
        let mut interval = Menu::new("Update Interval");
        for (ms, label) in INTERVALS {
            interval = interval.item(label, format!("interval:{ms}"), None).checked(self.settings.interval_ms == ms);
        }
        let any = self.selected_proc().is_some();
        Some(vec![
            Menu::new("File").item("Add Graph", "add-graph", None).separator().item("Quit", "quit", Some(Key::Q)),
            Menu::new("View")
                .item("Graphs", "graphs", Some(Key::Num1))
                .checked(tab == 0)
                .item("Processes", "processes", Some(Key::Num2))
                .checked(tab == 1)
                .separator()
                .submenu(interval),
            Menu::new("Process")
                .item("Quit", "quit-process", None)
                .enabled(tab == 1 && any)
                .item("Force Quit…", "force-quit-process", None)
                .enabled(tab == 1 && any),
        ])
    }

    fn command(&mut self, cx: &mut Cx, _window: WindowId, command: &str) {
        Activity::command(self, cx, command);
    }

    fn close_requested(&mut self, cx: &mut Cx, window: WindowId) {
        if matches!(self.windows.get(&window), Some(Win::Main)) {
            self.store();
            return cx.quit();
        }
        self.windows.remove(&window);
        cx.close_window(window);
    }

    fn message(&mut self, cx: &mut Cx, message: Box<dyn std::any::Any + Send>) {
        if message.is::<Launch>() {
            return self.open(cx);
        }
        let Ok(msg) = message.downcast::<Msg>() else { return };
        match *msg {
            Msg::Sources(s) => {
                self.history = vec![VecDeque::with_capacity(HISTORY); s.len()];
                self.sources = s;
            }
            Msg::Sample(s) => {
                for (h, v) in self.history.iter_mut().zip(s.values) {
                    if h.len() == HISTORY {
                        h.pop_front();
                    }
                    h.push_back(v);
                }
                if let Some(p) = s.processes {
                    self.processes = p;
                }
            }
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

fn main() {
    // A session or install from before the rename (Tabula) carries on.
    olo_kit::legacy::adopt();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if instance::forward(APP_ID, &args) {
        return;
    }
    olo_kit::run(APP_ID, |cx| {
        instance::listen(APP_ID, cx.proxy());
        let settings: Settings =
            std::fs::read_to_string(settings_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        let control = sample::Control {
            interval_ms: Arc::new(AtomicU64::new(settings.interval_ms)),
            want_processes: Arc::new(AtomicBool::new(settings.tab == 1)),
        };
        let (p1, p2) = (cx.proxy(), cx.proxy());
        sample::start(control.clone(), move |s| p1.send(Msg::Sources(s)), move |s| p2.send(Msg::Sample(s)));
        let mut app = Activity {
            windows: HashMap::new(),
            main: None,
            settings,
            control,
            sources: Vec::new(),
            history: Vec::new(),
            processes: Vec::new(),
            list: ListState::default(),
            selected: None,
            filter: String::new(),
            // SAFETY: getuid has no preconditions.
            uid: unsafe { libc::getuid() },
        };
        app.open(cx);
        app
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn nice_ceilings() {
        assert_eq!(super::nice_ceiling(0.0), 1.0);
        assert_eq!(super::nice_ceiling(3.2), 5.0);
        assert_eq!(super::nice_ceiling(12_000.0), 20_000.0);
        assert_eq!(super::nice_ceiling(100.0), 100.0);
    }
}
