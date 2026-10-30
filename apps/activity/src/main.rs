//! Activity Monitor: everything the machine is doing, live. A sidebar of
//! sections (a Mac's Activity Monitor and Haiku's ActivityMonitor, met
//! halfway): an overview of cards, every process (as a list or a tree,
//! searched and filtered with the kit's search; inspect one for its
//! threads, open files, memory, environment and limits; signal, renice,
//! pin to cores), the CPU (each core, frequencies, governors), memory
//! (zram, pressure, every counter), disks and volumes, the network and
//! its connections, the GPU and who uses it, sensors and the battery, the
//! system's tunables, and Haiku-style graphs of whatever you choose.
//!
//! Everything is measured on a worker thread (sample.rs), never the UI
//! thread; graphs scroll smoothly between samples, a pixel at a time. With
//! full access (helper.rs: the same program as root, through polkit) it
//! sees and does everything; without, what this account may.

mod actions;
mod details;
mod netdiag;
mod helper;
mod inspector;
mod processes;
mod sample;
mod sections;
mod smart;

use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{atomic::Ordering, mpsc},
    time::Instant,
};

use actions::{Action, Refusal};
use details::Details;
use helper::{FromHelper, Helper, ToHelper, Way};
use olo_kit::{
    App, Cx, Menu, WindowId, WindowOptions,
    egui::{self, Key, Rect, Ui, pos2, vec2},
    instance::{self, Launch},
    sidebar::{Row, Sidebar},
    widgets::{self, ListState, TextControl},
};
use sample::{Connection, Proc, Sample, Source, System};
use serde::{Deserialize, Serialize};

const APP_ID: &str = "olo.activity";

/// How long graphs remember (the longest span offered, and some).
const KEEP_SECONDS: f64 = 31.0 * 60.0;
const INTERVALS: [(u64, &str); 5] = [(250, "¼ Second"), (500, "½ Second"), (1000, "1 Second"), (2000, "2 Seconds"), (5000, "5 Seconds")];
const SPANS: [(u32, &str); 4] = [(60, "1 Minute"), (300, "5 Minutes"), (900, "15 Minutes"), (1800, "30 Minutes")];
const SIDEBAR_WIDTH: f32 = 176.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Section {
    #[default]
    Overview,
    Processes,
    Cpu,
    Memory,
    Disks,
    Network,
    Gpu,
    Sensors,
    System,
    Graphs,
}

impl Section {
    const ALL: [Section; 10] =
        [Section::Overview, Section::Processes, Section::Cpu, Section::Memory, Section::Disks, Section::Network, Section::Gpu, Section::Sensors, Section::System, Section::Graphs];

    pub fn name(self) -> &'static str {
        match self {
            Section::Overview => "Overview",
            Section::Processes => "Processes",
            Section::Cpu => "CPU",
            Section::Memory => "Memory",
            Section::Disks => "Disks",
            Section::Network => "Network",
            Section::Gpu => "GPU",
            Section::Sensors => "Sensors",
            Section::System => "System",
            Section::Graphs => "Graphs",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Section::Overview => "activity",
            Section::Processes => "process",
            Section::Cpu => "cpu",
            Section::Memory => "memory",
            Section::Disks => "disk",
            Section::Network => "network",
            Section::Gpu => "gpu",
            Section::Sensors => "sensors",
            Section::System => "system",
            Section::Graphs => "disk-usage",
        }
    }

    fn command(self) -> String {
        format!("section:{}", serde_json::to_string(&self).unwrap_or_default().trim_matches('"'))
    }
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Each of the Graphs section's graphs: its sources, by key.
    pub views: Vec<Vec<String>>,
    pub interval_ms: u64,
    pub section: Section,
    /// The process list's sort column and direction.
    pub sort: Option<(usize, bool)>,
    pub search: String,
    pub tree: bool,
    /// Seconds a graph shows.
    pub span: u32,
    pub ask_full_access: bool,
    /// Graphs scroll smoothly between samples, a device pixel at a time.
    pub smooth: bool,
    pub connections_sort: Option<(usize, bool)>,
    /// How many columns the process list had when `sort` was saved (0:
    /// from before Sent and Received came in at 8, `migrate`).
    #[serde(default)]
    pub columns: usize,
}

/// The process list's columns now.
const COLUMNS: usize = 14;

impl Settings {
    /// Settings saved by an older Activity Monitor, brought up to date.
    fn migrate(mut self) -> Settings {
        if self.columns < COLUMNS {
            // Sent and Received went in before Nice.
            if let Some((col, _)) = self.sort.as_mut()
                && *col >= 8
            {
                *col += 2;
            }
            self.columns = COLUMNS;
        }
        self
    }
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            views: vec![
                vec!["cpu".into(), "memory".into(), "cached".into()],
                vec!["net-rx".into(), "net-tx".into(), "disk-read".into(), "disk-write".into()],
            ],
            interval_ms: 1000,
            section: Section::Overview,
            sort: Some((3, false)),
            search: String::new(),
            tree: false,
            span: 60,
            ask_full_access: true,
            smooth: true,
            connections_sort: None,
            columns: COLUMNS,
        }
    }
}

fn settings_path() -> std::path::PathBuf {
    olo_kit::config_dir().join("activity.json")
}

pub enum Msg {
    /// From the sampler here (false) or the helper's (true).
    Sources(Vec<Source>, bool),
    Sample(Box<Sample>, bool),
    Details(i32, Result<Box<Details>, String>),
    Helper(Option<FromHelper>),
}

/// What this app may see and do.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Access {
    /// What this account may.
    Mine,
    /// polkit is asking for the password.
    Asking,
    Full,
}

enum Win {
    Main,
    Inspector(Box<inspector::Inspector>),
    Confirm { pid: i32, message: String },
    Error { message: String },
    /// An action this account may not do: Cancel, or Get Full Access and
    /// do it then.
    NeedsAccess { action: Action, message: String },
    Tune { name: String, value: String },
}

/// The busiest processes at a moment, for looking back from a graph.
pub struct Moment {
    pub at: f64,
    pub cpu: Vec<(i32, String, f64)>,
    pub disk: Vec<(i32, String, f64)>,
}

pub struct Activity {
    windows: HashMap<WindowId, Win>,
    main: Option<WindowId>,
    pub settings: Settings,
    control: sample::Control,
    pub sources: Vec<Source>,
    /// Each source's samples, (time, value), oldest first.
    pub history: Vec<VecDeque<(f64, f64)>>,
    by_key: HashMap<String, usize>,
    /// Each sampler's source order into ours.
    map_local: Vec<usize>,
    map_helper: Vec<usize>,
    clock: Instant,
    /// When the latest sample came.
    last: Option<f64>,
    pub system: Box<System>,
    pub processes: Vec<Proc>,
    /// Bumped with each new list (for the process list's sorted rows).
    pub generation: u64,
    pub connections: Vec<Connection>,
    pub moments: VecDeque<Moment>,
    /// A time chosen on a graph, to see what was busy then.
    pub drill: Option<f64>,
    /// CPU and memory of inspected processes over time.
    pub proc_history: HashMap<i32, VecDeque<(f64, f64)>>,
    pub list: ListState,
    pub selected: Option<i32>,
    pub collapsed: HashSet<i32>,
    pub rows_cache: Option<(u64, processes::RowsKey, Vec<processes::RowRef>)>,
    pub reveal_selected: bool,
    pub connections_list: ListState,
    pub connections_search: String,
    pub uid: u32,
    pub hz: f64,
    pub access: Access,
    helper: Option<Helper>,
    /// Actions to do once full access comes.
    after_access: Vec<Action>,
    next_action: u64,
    inspect_tx: mpsc::Sender<i32>,
    pub tunables_list: ListState,
    /// Find: the search takes the keyboard next frame.
    pub focus_search: bool,
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
        let id = cx.open_window(WindowOptions::new("Activity Monitor", 900, 620).min_size(560, 400));
        self.windows.insert(id, Win::Main);
        self.main = Some(id);
    }

    /// Seconds since the app started: samples' times.
    pub fn t(&self) -> f64 {
        self.clock.elapsed().as_secs_f64()
    }

    /// The time graphs show at their right edge: an interval behind the
    /// latest sample, so their lines reach the edge and scroll smoothly
    /// into it rather than jump at each sample.
    pub fn now(&self) -> f64 {
        let Some(at) = self.last else { return self.t() };
        let interval = self.settings.interval_ms as f64 / 1000.0;
        if self.settings.smooth { at + (self.t() - at).min(interval) - interval } else { at }
    }

    pub fn source(&self, key: &str) -> Option<usize> {
        self.by_key.get(key).copied()
    }

    /// The latest value of source `key`.
    pub fn latest(&self, key: &str) -> Option<f64> {
        self.source(key).and_then(|i| self.history[i].back()).map(|(_, v)| *v).filter(|v| v.is_finite())
    }

    fn take_sources(&mut self, sources: Vec<Source>, from_helper: bool) {
        let mut map = Vec::with_capacity(sources.len());
        for s in sources {
            let i = match self.by_key.get(&s.key) {
                Some(&i) => i,
                None => {
                    self.by_key.insert(s.key.clone(), self.sources.len());
                    self.sources.push(s);
                    self.history.push(VecDeque::new());
                    self.sources.len() - 1
                }
            };
            map.push(i);
        }
        if from_helper {
            self.map_helper = map;
        } else {
            self.map_local = map;
        }
    }

    fn take_sample(&mut self, cx: &mut Cx, s: Sample, from_helper: bool) {
        // A sample from whoever isn't measuring now (one in flight as the
        // helper came or went) is dropped.
        if from_helper != (self.access == Access::Full) {
            return;
        }
        let at = self.t();
        let map = if from_helper { &self.map_helper } else { &self.map_local };
        for (i, v) in s.values.iter().enumerate() {
            if let Some(&j) = map.get(i) {
                self.history[j].push_back((at, *v));
            }
        }
        for h in &mut self.history {
            while h.front().is_some_and(|(t, _)| *t < at - KEEP_SECONDS) {
                h.pop_front();
            }
        }
        self.last = Some(at);
        self.system = s.system;
        if let Some(list) = s.processes {
            let top = |f: &dyn Fn(&Proc) -> f64| {
                let mut v: Vec<(i32, String, f64)> = list.iter().map(|p| (p.pid, p.name.clone(), f(p))).filter(|(_, _, x)| *x > 0.0).collect();
                v.sort_by(|a, b| b.2.total_cmp(&a.2));
                v.truncate(8);
                v
            };
            self.moments.push_back(Moment { at, cpu: top(&|p| p.cpu), disk: top(&|p| p.read.unwrap_or(0.0) + p.written.unwrap_or(0.0)) });
            while self.moments.front().is_some_and(|m| m.at < at - KEEP_SECONDS) {
                self.moments.pop_front();
            }
            let inspected: HashSet<i32> = self.inspected().collect();
            self.proc_history.retain(|pid, _| inspected.contains(pid));
            for p in list.iter().filter(|p| inspected.contains(&p.pid)) {
                let h = self.proc_history.entry(p.pid).or_default();
                h.push_back((at, p.cpu));
                while h.front().is_some_and(|(t, _)| *t < at - KEEP_SECONDS) {
                    h.pop_front();
                }
            }
            self.processes = list;
            self.generation += 1;
        }
        if let Some(c) = s.connections {
            self.connections = c;
        }
        for pid in self.inspected().collect::<HashSet<_>>() {
            self.request_details(pid);
        }
        cx.redraw_all();
    }

    fn inspected(&self) -> impl Iterator<Item = i32> + '_ {
        self.windows.values().filter_map(|w| if let Win::Inspector(i) = w { Some(i.pid) } else { None })
    }

    fn request_details(&self, pid: i32) {
        match (&self.helper, self.access) {
            (Some(h), Access::Full) => h.send(&ToHelper::Inspect { pid }),
            _ => {
                let _ = self.inspect_tx.send(pid);
            }
        }
    }

    /// Tell whoever measures what to measure.
    pub fn wants_changed(&mut self) {
        let section = self.settings.section;
        self.control.interval_ms.store(self.settings.interval_ms, Ordering::Relaxed);
        // Processes always (the overview's busiest, and looking back);
        // connections and the GPU's users only while they show.
        self.control.want_processes.store(true, Ordering::Relaxed);
        self.control.want_connections.store(section == Section::Network, Ordering::Relaxed);
        self.control.want_gpu_clients.store(section == Section::Gpu, Ordering::Relaxed);
        if let (Some(h), Access::Full) = (&self.helper, self.access) {
            h.send(&ToHelper::Wants(self.control.wants()));
        }
    }

    pub fn set_section(&mut self, section: Section) {
        self.settings.section = section;
        self.wants_changed();
        self.store();
    }

    /// Ask for full access (polkit asks for the password).
    pub fn get_full_access(&mut self, cx: &mut Cx) {
        if self.helper.is_some() {
            return;
        }
        let proxy = cx.proxy();
        match Helper::start(helper::way(), move |m| proxy.send(Msg::Helper(m))) {
            Ok(h) => {
                self.helper = Some(h);
                self.access = Access::Asking;
            }
            Err(e) => {
                let message = format!("Full access couldn't be asked for. {e}");
                self.alert(cx, Win::Error { message: message.clone() }, &message);
            }
        }
        cx.redraw_all();
    }

    fn give_up_access(&mut self, cx: &mut Cx) {
        self.helper = None;
        self.access = Access::Mine;
        self.control.paused.store(false, Ordering::Relaxed);
        cx.redraw_all();
    }

    fn heard(&mut self, cx: &mut Cx, m: Option<FromHelper>) {
        match m {
            Some(FromHelper::Ready) => {
                self.access = Access::Full;
                self.control.paused.store(true, Ordering::Relaxed);
                self.wants_changed();
                for a in std::mem::take(&mut self.after_access) {
                    self.act(cx, a);
                }
                for pid in self.inspected().collect::<HashSet<_>>() {
                    self.request_details(pid);
                }
            }
            Some(FromHelper::Sources(s)) => self.take_sources(s, true),
            Some(FromHelper::Sample(s)) => self.take_sample(cx, *s, true),
            Some(FromHelper::Details { pid, details }) => self.take_details(pid, details.map(|d| *d)),
            Some(FromHelper::Done { result, .. }) => {
                if let Err(r) = result {
                    self.alert(cx, Win::Error { message: r.message.clone() }, &r.message);
                }
                self.refresh_soon();
            }
            // Refused (the password dialog was cancelled) or gone.
            None => {
                self.after_access.clear();
                self.give_up_access(cx);
            }
        }
        cx.redraw_all();
    }

    fn take_details(&mut self, pid: i32, details: Result<Details, String>) {
        let hz = self.hz;
        for w in self.windows.values_mut() {
            if let Win::Inspector(i) = w
                && i.pid == pid
            {
                i.take(details.clone(), hz);
            }
        }
    }

    /// After acting: measure the inspected again now, not at the next
    /// sample.
    fn refresh_soon(&self) {
        for pid in self.inspected().collect::<HashSet<_>>() {
            self.request_details(pid);
        }
    }

    /// Do `action`: here when this account may, else by the helper.
    pub fn act(&mut self, cx: &mut Cx, action: Action) {
        if let (Some(h), Access::Full) = (&self.helper, self.access) {
            self.next_action += 1;
            h.send(&ToHelper::Act { id: self.next_action, action });
            return;
        }
        match action.run() {
            Ok(()) => self.refresh_soon(),
            Err(Refusal { message, needs_access: true }) if helper::way() != Way::Off => {
                let message = format!("{message} Activity Monitor can do it once you give it full access, with an administrator's password.");
                let id = cx.open_window(WindowOptions::alert("Activity Monitor", &message));
                self.windows.insert(id, Win::NeedsAccess { action, message });
            }
            Err(r) => self.alert(cx, Win::Error { message: r.message.clone() }, &r.message),
        }
    }

    fn alert(&mut self, cx: &mut Cx, win: Win, message: &str) {
        let id = cx.open_window(WindowOptions::alert("Activity Monitor", message));
        self.windows.insert(id, win);
    }

    pub fn selected_proc(&self) -> Option<&Proc> {
        self.selected.and_then(|pid| self.processes.iter().find(|p| p.pid == pid))
    }

    pub fn proc(&self, pid: i32) -> Option<&Proc> {
        self.processes.iter().find(|p| p.pid == pid)
    }

    pub fn quit_process(&mut self, cx: &mut Cx, pid: i32, force: bool) {
        let name = self.proc(pid).map_or_else(|| pid.to_string(), |p| p.name.clone());
        if force {
            let message = format!("Force “{name}” ({pid}) to quit? Any unsaved work in it will be lost.");
            self.alert(cx, Win::Confirm { pid, message: message.clone() }, &message);
        } else {
            self.act(cx, Action::Signal { pid, signal: libc::SIGTERM });
        }
    }

    /// Open (or bring forward) the inspector of `pid`.
    pub fn inspect(&mut self, cx: &mut Cx, pid: i32) {
        if let Some((&id, _)) = self.windows.iter().find(|(_, w)| matches!(w, Win::Inspector(i) if i.pid == pid)) {
            return cx.activate(id);
        }
        let name = self.proc(pid).map_or_else(|| pid.to_string(), |p| p.name.clone());
        let id = cx.open_window(WindowOptions::new(format!("{name} ({pid})"), 620, 520).min_size(460, 380));
        self.windows.insert(id, Win::Inspector(Box::new(inspector::Inspector::new(pid, name))));
        self.request_details(pid);
    }

    pub fn tune(&mut self, cx: &mut Cx, name: &str) {
        let value = self.system.tunables.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone()).unwrap_or_default();
        let id = cx.open_window(WindowOptions::new(format!("Set {name}"), 420, 150).min_size(420, 150).max_size(420, 150).dialog());
        self.windows.insert(id, Win::Tune { name: name.into(), value });
    }

    pub fn command(&mut self, cx: &mut Cx, window: Option<WindowId>, command: &str) {
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
            self.wants_changed();
            return self.store();
        }
        if let Some(s) = command.strip_prefix("span:").and_then(|v| v.parse::<u32>().ok()) {
            self.settings.span = s;
            return self.store();
        }
        if let Some(name) = command.strip_prefix("section:")
            && let Ok(s) = serde_json::from_str::<Section>(&format!("\"{name}\""))
        {
            return self.set_section(s);
        }
        // A process's commands: "signal:PID:N", "nice:PID:N".
        if let Some(rest) = command.strip_prefix("signal:")
            && let Some((pid, sig)) = rest.split_once(':')
            && let (Ok(pid), Ok(signal)) = (pid.parse(), sig.parse())
        {
            return self.act(cx, Action::Signal { pid, signal });
        }
        if let Some(rest) = command.strip_prefix("nice:")
            && let Some((pid, n)) = rest.split_once(':')
            && let (Ok(pid), Ok(nice)) = (pid.parse(), n.parse())
        {
            return self.act(cx, Action::Nice { pid, nice });
        }
        if let Some(pid) = command.strip_prefix("inspect:").and_then(|p| p.parse().ok()) {
            return self.inspect(cx, pid);
        }
        if let Some(pid) = command.strip_prefix("quit:").and_then(|p| p.parse().ok()) {
            return self.quit_process(cx, pid, false);
        }
        if let Some(pid) = command.strip_prefix("force-quit:").and_then(|p| p.parse().ok()) {
            return self.quit_process(cx, pid, true);
        }
        if let Some(pid) = command.strip_prefix("copy-pid:").and_then(|p| p.parse::<i32>().ok()) {
            return cx.copy(pid.to_string());
        }
        if let Some(pid) = command.strip_prefix("copy-command:").and_then(|p| p.parse::<i32>().ok()) {
            let text = self.proc(pid).map(|p| p.command.clone()).unwrap_or_default();
            return cx.copy(text);
        }
        if let Some(pid) = command.strip_prefix("show-program:").and_then(|p| p.parse::<i32>().ok()) {
            if let Ok(exe) = std::fs::read_link(format!("/proc/{pid}/exe")) {
                let _ = olo_kit::tracker::open(&exe);
            }
            return;
        }
        if let Some(path) = command.strip_prefix("open-path:") {
            let _ = olo_kit::tracker::open(std::path::Path::new(path));
            return;
        }
        if let Some(text) = command.strip_prefix("copy-text:") {
            return cx.copy(text.to_owned());
        }
        if let Some(name) = command.strip_prefix("tune:") {
            return self.tune(cx, name);
        }
        if let Some(pid) = command.strip_prefix("select:").and_then(|p| p.parse::<i32>().ok()) {
            self.selected = Some(pid);
            self.reveal_selected = true;
            return self.set_section(Section::Processes);
        }
        let target = window.and_then(|w| match self.windows.get(&w) {
            Some(Win::Inspector(i)) => Some(i.pid),
            _ => None,
        });
        let pid = target.or(self.selected.filter(|_| self.settings.section == Section::Processes));
        match command {
            "add-graph" => {
                self.settings.views.push(vec!["cpu".into()]);
                self.set_section(Section::Graphs);
            }
            "quit-process" => {
                if let Some(pid) = pid {
                    self.quit_process(cx, pid, false);
                }
            }
            "force-quit-process" => {
                if let Some(pid) = pid {
                    self.quit_process(cx, pid, true);
                }
            }
            "inspect-process" => {
                if let Some(pid) = pid {
                    self.inspect(cx, pid);
                }
            }
            "tree" => {
                self.settings.tree = !self.settings.tree;
                self.rows_cache = None;
                self.store();
            }
            "smooth" => {
                self.settings.smooth = !self.settings.smooth;
                self.store();
            }
            "full-access" => self.get_full_access(cx),
            "ask-full-access" => {
                self.settings.ask_full_access = !self.settings.ask_full_access;
                self.store();
            }
            "find" => {
                self.set_section(Section::Processes);
                self.focus_search = true;
            }
            "close" => {
                if let Some(w) = window {
                    self.close_requested(cx, w);
                }
            }
            "quit" => cx.quit(),
            _ => {}
        }
    }

    fn sections(&self) -> Vec<Section> {
        Section::ALL
            .into_iter()
            .filter(|s| match s {
                Section::Gpu => self.source("gpu-busy").is_some(),
                Section::Sensors => self.sources.iter().any(|s| s.group == sample::Group::Sensors) || self.source("battery").is_some(),
                _ => true,
            })
            .collect()
    }

    fn main_ui(&mut self, cx: &mut Cx, ui: &mut Ui) {
        let full = ui.max_rect();
        let side = Rect::from_min_size(full.min, vec2(SIDEBAR_WIDTH, full.height()));
        let content = Rect::from_min_max(pos2(side.right(), full.top()), full.max);
        let sections = self.sections();
        if !sections.contains(&self.settings.section) {
            self.settings.section = Section::Overview;
        }
        // The sidebar, each row with its figure now.
        let values: Vec<String> = sections.iter().map(|s| sections::sidebar_value(self, *s)).collect();
        let headings = [(0, "Activity"), (2, "Hardware"), (sections.len() - 2, "System")];
        let mut rows: Vec<Row> = Vec::new();
        let mut index_of = Vec::new();
        for (i, s) in sections.iter().enumerate() {
            if let Some((_, h)) = headings.iter().find(|(at, _)| *at == i) {
                rows.push(Row::heading(h));
                index_of.push(None);
            }
            let row = Row::new(s.icon(), s.name());
            rows.push(if values[i].is_empty() { row } else { row.value(&values[i]) });
            index_of.push(Some(*s));
        }
        let mut chosen = index_of.iter().position(|s| *s == Some(self.settings.section)).unwrap_or(1);
        let before = chosen;
        Sidebar::new("activity-sections", &rows, &mut chosen).show(ui, side);
        if chosen != before
            && let Some(Some(s)) = index_of.get(chosen)
        {
            self.set_section(*s);
        }
        if self.sources.is_empty() {
            return;
        }
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(content));
        child.set_clip_rect(content);
        match self.settings.section {
            Section::Processes => processes::show(self, cx, &mut child, content),
            s => sections::show(self, cx, &mut child, content, s),
        }
    }
}

impl App for Activity {
    fn ui(&mut self, cx: &mut Cx, window: WindowId, ui: &mut Ui) {
        let Some(win) = self.windows.get_mut(&window) else { return };
        match win {
            Win::Main => self.main_ui(cx, ui),
            Win::Inspector(_) => {
                let Some(Win::Inspector(mut i)) = self.windows.remove(&window) else { return };
                let ask = i.ui(self, ui);
                self.windows.insert(window, Win::Inspector(i));
                match ask {
                    Some(inspector::Ask::Command(cmd)) => self.command(cx, Some(window), &cmd),
                    Some(inspector::Ask::Act(a)) => self.act(cx, a),
                    None => {}
                }
            }
            Win::Confirm { pid, message, .. } => {
                let (pid, message) = (*pid, message.clone());
                let Some(choice) = widgets::alert(ui, "warning", &message, &["Cancel", "Force Quit"]) else { return };
                self.windows.remove(&window);
                cx.close_window(window);
                if choice == 1 {
                    self.act(cx, Action::Signal { pid, signal: libc::SIGKILL });
                }
            }
            Win::Error { message } => {
                let message = message.clone();
                if widgets::alert(ui, "stop", &message, &["OK"]).is_some() {
                    self.windows.remove(&window);
                    cx.close_window(window);
                }
            }
            Win::NeedsAccess { action, message } => {
                let (action, message) = (action.clone(), message.clone());
                let Some(choice) = widgets::alert(ui, "authentication", &message, &["Cancel", "Get Full Access…"]) else { return };
                self.windows.remove(&window);
                cx.close_window(window);
                if choice == 1 {
                    self.after_access.push(action);
                    self.get_full_access(cx);
                }
            }
            Win::Tune { name, value } => {
                let name = name.clone();
                let column = widgets::alert_frame(ui, "system");
                let about = sample::TUNABLES.iter().find(|(n, _)| *n == name).map_or("", |(_, d)| *d);
                let top = widgets::alert_text(ui, column, column.top(), &format!("Set {name} ({}).", about.to_lowercase()));
                let field = Rect::from_min_size(pos2(column.left(), top + olo_kit::metrics::SPACING), vec2(column.width(), olo_kit::metrics::CONTROL_HEIGHT));
                let resp = ui.put(field, TextControl::new(value).label("Value:").hint("A value"));
                if !resp.has_focus() && ui.ctx().memory(|m| m.focused().is_none()) {
                    resp.request_focus();
                }
                let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                let ready = !value.trim().is_empty();
                let choice = widgets::alert_buttons(ui, column, &["Cancel", "Set"], ready).or(if enter && ready { Some(1) } else { None });
                if let Some(choice) = choice {
                    let value = value.trim().to_owned();
                    self.windows.remove(&window);
                    cx.close_window(window);
                    if choice == 1 {
                        self.act(cx, Action::Sysctl { name, value });
                    }
                }
            }
        }
    }

    fn menu_bar(&mut self, window: WindowId) -> Option<Vec<Menu>> {
        let inspector = matches!(self.windows.get(&window), Some(Win::Inspector(_)));
        if !matches!(self.windows.get(&window), Some(Win::Main)) && !inspector {
            return None;
        }
        let mut interval = Menu::new("Update Interval");
        for (ms, label) in INTERVALS {
            interval = interval.item(label, format!("interval:{ms}"), None).checked(self.settings.interval_ms == ms);
        }
        let mut span = Menu::new("Graphs Show");
        for (s, label) in SPANS {
            span = span.item(label, format!("span:{s}"), None).checked(self.settings.span == s);
        }
        let mut view = Menu::new("View");
        for s in self.sections() {
            // Each section's key is its own, whichever this machine has.
            let key = [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6, Key::Num7, Key::Num8, Key::Num9].get(Section::ALL.iter().position(|x| *x == s).unwrap_or(9)).copied();
            view = view.item(s.name(), s.command(), key).checked(self.settings.section == s);
        }
        let view = view
            .separator()
            .item("Processes as a Tree", "tree", None)
            .checked(self.settings.tree)
            .item("Smooth Scrolling Graphs", "smooth", None)
            .checked(self.settings.smooth)
            .separator()
            .submenu(span)
            .submenu(interval);
        let pid = match self.windows.get(&window) {
            Some(Win::Inspector(i)) => Some(i.pid),
            _ => self.selected.filter(|_| self.settings.section == Section::Processes && self.selected_proc().is_some()),
        };
        let any = pid.is_some();
        let mut process = Menu::new("Process")
            .item("Inspect", "inspect-process", Some(Key::I))
            .enabled(any && !inspector)
            .separator()
            .item("Quit", "quit-process", None)
            .enabled(any)
            .item("Force Quit…", "force-quit-process", None)
            .enabled(any);
        if let Some(pid) = pid {
            process = process.separator().submenu(processes::signal_menu(pid)).submenu(processes::priority_menu(pid, self.proc(pid).map_or(0, |p| p.nice)));
        }
        let full = match self.access {
            Access::Full => "Full Access",
            Access::Asking => "Asking for Full Access…",
            Access::Mine => "Get Full Access…",
        };
        Some(vec![
            Menu::new("File")
                .item("Find Process…", "find", Some(Key::F))
                .separator()
                .item("Add Graph", "add-graph", None)
                .separator()
                .item(full, "full-access", None)
                .enabled(self.access == Access::Mine && helper::way() != Way::Off)
                .checked(self.access == Access::Full)
                .item("Ask for Full Access at Start", "ask-full-access", None)
                .checked(self.settings.ask_full_access)
                .separator()
                .item("Close", "close", Some(Key::W))
                .item("Quit", "quit", Some(Key::Q)),
            view,
            process,
        ])
    }

    fn command(&mut self, cx: &mut Cx, window: WindowId, command: &str) {
        Activity::command(self, cx, Some(window), command);
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
            Msg::Sources(s, from_helper) => self.take_sources(s, from_helper),
            Msg::Sample(s, from_helper) => self.take_sample(cx, *s, from_helper),
            Msg::Details(pid, d) => {
                // Full access answers for inspectors now.
                if self.access != Access::Full {
                    self.take_details(pid, d.map(|d| *d));
                }
            }
            Msg::Helper(m) => return self.heard(cx, m),
        }
        cx.redraw_all();
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
    if args.first().map(String::as_str) == Some("--helper") {
        helper::serve();
    }
    if instance::forward(APP_ID, &args) {
        return;
    }
    olo_kit::run(APP_ID, |cx| {
        instance::listen(APP_ID, cx.proxy());
        let settings: Settings =
            std::fs::read_to_string(settings_path()).ok().and_then(|t| serde_json::from_str::<Settings>(&t).ok()).map(Settings::migrate).unwrap_or_default();
        let control = sample::Control::new(settings.interval_ms);
        let (p1, p2) = (cx.proxy(), cx.proxy());
        sample::start(control.clone(), move |s| p1.send(Msg::Sources(s, false)), move |s| p2.send(Msg::Sample(Box::new(s), false)));
        // Inspectors' details, read off the UI thread.
        let (inspect_tx, inspect_rx) = mpsc::channel::<i32>();
        let proxy = cx.proxy();
        std::thread::Builder::new()
            .name("activity-inspect".into())
            .spawn(move || {
                while let Ok(pid) = inspect_rx.recv() {
                    // Asked for several times while busy: once will do.
                    let mut pids = vec![pid];
                    pids.extend(inspect_rx.try_iter());
                    pids.sort_unstable();
                    pids.dedup();
                    for pid in pids {
                        proxy.send(Msg::Details(pid, details::read_details(pid).map(Box::new)));
                    }
                }
            })
            .expect("the inspect thread");
        let ask = settings.ask_full_access;
        let mut app = Activity {
            windows: HashMap::new(),
            main: None,
            settings,
            control,
            sources: Vec::new(),
            history: Vec::new(),
            by_key: HashMap::new(),
            map_local: Vec::new(),
            map_helper: Vec::new(),
            clock: Instant::now(),
            last: None,
            system: Box::default(),
            processes: Vec::new(),
            generation: 0,
            connections: Vec::new(),
            moments: VecDeque::new(),
            drill: None,
            proc_history: HashMap::new(),
            list: ListState::default(),
            selected: None,
            collapsed: HashSet::new(),
            rows_cache: None,
            reveal_selected: false,
            connections_list: ListState::default(),
            connections_search: String::new(),
            // SAFETY: getuid has no preconditions.
            uid: unsafe { libc::getuid() },
            // SAFETY: sysconf has no preconditions.
            hz: unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as f64,
            access: Access::Mine,
            helper: None,
            after_access: Vec::new(),
            next_action: 0,
            inspect_tx,
            tunables_list: ListState::default(),
            focus_search: false,
        };
        app.wants_changed();
        app.open(cx);
        // SAFETY: geteuid has no preconditions.
        let root = unsafe { libc::geteuid() } == 0;
        if ask && !root && helper::way() != Way::Off {
            app.get_full_access(cx);
        }
        app
    });
}
