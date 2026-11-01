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
use olo_ui::{
    Frame, Menu, MenuBar, Proxy, WindowOptions, menu,
    gpui::{
        AnyElement, AnyView, App, AppContext as _, ClipboardItem, Context, Entity, FocusHandle, InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _, Render, StyleRefinement, Styled as _,
        Subscription, Window, WindowHandle, div, px,
    },
    instance::{self, Launch},
    metrics as m,
    search::{SearchEvent, SearchField, search_field},
    widgets::{
        AlertFrame, Button, GraphLayer, GraphShelf, ListModel, Sidebar, TextControl, TextField, TextFieldEvent, button_row, open_alert, paragraph,
        sidebar::Row,
        text_field,
    },
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
    /// The supervisor's work (Preferences ▸ Performance).
    Supervisor,
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
    const ALL: [Section; 11] =
        [Section::Overview, Section::Processes, Section::Supervisor, Section::Cpu, Section::Memory, Section::Disks, Section::Network, Section::Gpu, Section::Sensors, Section::System, Section::Graphs];
    /// The sections with keys, ⌘1 to ⌘9 in this order (as
    /// `olo_protocol::app_keys` lists them).
    const KEYED: [Section; 9] = [Section::Overview, Section::Processes, Section::Cpu, Section::Memory, Section::Disks, Section::Network, Section::Gpu, Section::Sensors, Section::System];

    pub fn name(self) -> &'static str {
        match self {
            Section::Overview => "Overview",
            Section::Processes => "Processes",
            Section::Supervisor => "Supervisor",
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
            Section::Supervisor => "performance",
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
    olo_ui::config_dir().join("activity.json")
}

/// Written whole or not at all: to a file beside it, then put in its
/// place.
fn write_settings(text: &str) {
    let path = settings_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let new = path.with_extension("json.new");
    if std::fs::write(&new, text).is_ok() {
        let _ = std::fs::rename(&new, &path);
    }
}

pub enum Msg {
    /// From the sampler here (false) or the helper's (true).
    Sources(Vec<Source>, bool),
    Sample(Box<Sample>, bool),
    Details(i32, Result<Box<Details>, String>),
    Helper(Option<FromHelper>),
    /// What the supervisor published (`olo-supervisor.json`).
    Supervisor(Box<olo_protocol::performance::SupervisorState>),
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

/// The busiest processes at a moment, for looking back from a graph.
pub struct Moment {
    pub at: f64,
    pub cpu: Vec<(i32, String, f64)>,
    pub disk: Vec<(i32, String, f64)>,
}

pub struct Activity {
    main: Option<WindowHandle<Frame>>,
    /// Each inspector open: its process, window and view.
    inspectors: Vec<(i32, WindowHandle<Frame>, Entity<inspector::Inspector>)>,
    pub settings: Settings,
    control: sample::Control,
    pub sources: Vec<Source>,
    /// Each source's samples, (time, value), oldest first.
    pub history: Vec<std::rc::Rc<VecDeque<(f64, f64)>>>,
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
    pub proc_history: HashMap<i32, std::rc::Rc<VecDeque<(f64, f64)>>>,
    pub list: Entity<ListModel>,
    pub selected: Option<i32>,
    pub collapsed: HashSet<i32>,
    pub rows_cache: Option<(u64, processes::RowsKey, std::rc::Rc<processes::Rows>)>,
    /// The process list's filters, for the processes of a generation.
    pub facets: Option<(u64, std::rc::Rc<Vec<olo_ui::search::Facet>>)>,
    pub reveal_selected: bool,
    pub search: Entity<SearchField>,
    pub connections_list: Entity<ListModel>,
    pub connections_search: Entity<TextField>,
    pub uid: u32,
    pub hz: f64,
    pub access: Access,
    helper: Option<Helper>,
    /// Actions to do once full access comes.
    after_access: Vec<Action>,
    next_action: u64,
    inspect_tx: mpsc::Sender<i32>,
    pub tunables_list: Entity<ListModel>,
    /// What the supervisor is doing, as it published it.
    pub supervisor: olo_protocol::performance::SupervisorState,
    proxy: Proxy,
    /// The sections' scrolling, kept while each shows.
    pub scroll: olo_ui::gpui::ScrollHandle,
    /// Where the section's graphs go on their layer.
    pub shelf: GraphShelf,
    parts: Option<Parts>,
    _subscriptions: Vec<Subscription>,
}

impl Activity {
    /// The settings kept, on a worker.
    fn store(&self) {
        let text = serde_json::to_string_pretty(&self.settings).unwrap_or_default();
        std::thread::spawn(move || write_settings(&text));
    }

    /// The settings kept now (before the process ends or is replaced).
    fn store_now(&self) {
        write_settings(&serde_json::to_string_pretty(&self.settings).unwrap_or_default());
    }

    /// The main window: opened, or brought forward.
    fn open(&mut self, cx: &mut Context<Self>) {
        if let Some(w) = self.main {
            cx.defer(move |cx| {
                let _ = w.update(cx, |_, window, _| window.activate_window());
            });
            return;
        }
        let me = cx.entity();
        cx.defer(move |cx| {
            let view = me.clone();
            let w = olo_ui::open_window(cx, WindowOptions::new("Activity Monitor", 900.0, 620.0).min_size(560.0, 400.0), move |window, cx| {
                let closing = view.clone();
                window.on_window_should_close(cx, move |_, cx| {
                    closing.update(cx, |a, cx| a.quit(cx));
                    true
                });
                view
            });
            me.update(cx, |a, _| a.main = w);
        });
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
                    self.history.push(Default::default());
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

    fn take_sample(&mut self, s: Sample, from_helper: bool, cx: &mut Context<Self>) {
        // A sample from whoever isn't measuring now (one in flight as the
        // helper came or went) is dropped.
        if from_helper != (self.access == Access::Full) {
            return;
        }
        let at = self.t();
        // (The graphs shown hold the samples: let go, they're added to in
        // place rather than copied. The section puts them back as it draws.)
        self.shelf.clear();
        let map = if from_helper { &self.map_helper } else { &self.map_local };
        for (i, v) in s.values.iter().enumerate() {
            if let Some(&j) = map.get(i) {
                std::rc::Rc::make_mut(&mut self.history[j]).push_back((at, *v));
            }
        }
        for h in &mut self.history {
            if h.front().is_some_and(|(t, _)| *t < at - KEEP_SECONDS) {
                let h = std::rc::Rc::make_mut(h);
                while h.front().is_some_and(|(t, _)| *t < at - KEEP_SECONDS) {
                    h.pop_front();
                }
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
                let h = std::rc::Rc::make_mut(self.proc_history.entry(p.pid).or_default());
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
        cx.notify();
    }

    fn inspected(&self) -> impl Iterator<Item = i32> + '_ {
        self.inspectors.iter().map(|(pid, ..)| *pid)
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

    /// `--section NAME`: that section; `--inspect PID`: that process's
    /// inspector, at what the supervisor found.
    fn follow_args(&mut self, args: &[String], cx: &mut Context<Self>) {
        let after = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1));
        if let Some(name) = after("--section")
            && let Some(s) = Section::ALL.into_iter().find(|s| s.name().eq_ignore_ascii_case(name))
        {
            self.set_section(s, cx);
        }
        // (From the supervisor: at what it found.)
        if let Some(pid) = after("--inspect").and_then(|p| p.parse().ok()) {
            self.inspect_at(pid, inspector::SUPERVISOR_TAB, cx);
        }
    }

    pub fn set_section(&mut self, section: Section, cx: &mut Context<Self>) {
        if self.settings.section != section {
            self.scroll = Default::default();
        }
        self.settings.section = section;
        self.wants_changed();
        self.store();
        cx.notify();
    }

    /// Ask for full access (polkit asks for the password).
    pub fn get_full_access(&mut self, cx: &mut Context<Self>) {
        if self.helper.is_some() {
            return;
        }
        let proxy = self.proxy.clone();
        match Helper::start(helper::way(), move |m| proxy.send(Msg::Helper(m))) {
            Ok(h) => {
                self.helper = Some(h);
                self.access = Access::Asking;
            }
            Err(e) => self.alert(&format!("Full access couldn't be asked for. {e}"), cx),
        }
        cx.notify();
    }

    fn give_up_access(&mut self, cx: &mut Context<Self>) {
        self.helper = None;
        self.access = Access::Mine;
        self.control.paused.store(false, Ordering::Relaxed);
        cx.notify();
    }

    fn heard(&mut self, m: Option<FromHelper>, cx: &mut Context<Self>) {
        match m {
            Some(FromHelper::Ready) => {
                self.access = Access::Full;
                self.control.paused.store(true, Ordering::Relaxed);
                self.wants_changed();
                for a in std::mem::take(&mut self.after_access) {
                    self.act(a, cx);
                }
                for pid in self.inspected().collect::<HashSet<_>>() {
                    self.request_details(pid);
                }
            }
            Some(FromHelper::Sources(s)) => self.take_sources(s, true),
            Some(FromHelper::Sample(s)) => self.take_sample(*s, true, cx),
            Some(FromHelper::Details { pid, details }) => self.take_details(pid, details.map(|d| *d), cx),
            Some(FromHelper::Done { result, .. }) => {
                if let Err(r) = result {
                    self.alert(&r.message, cx);
                }
                self.refresh_soon();
            }
            // Refused (the password dialog was cancelled) or gone.
            None => {
                self.after_access.clear();
                self.give_up_access(cx);
            }
        }
        cx.notify();
    }

    fn take_details(&mut self, pid: i32, details: Result<Details, String>, cx: &mut Context<Self>) {
        let hz = self.hz;
        for (p, _, i) in &self.inspectors {
            if *p == pid {
                i.update(cx, |i, cx| i.take(details.clone(), hz, cx));
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
    pub fn act(&mut self, action: Action, cx: &mut Context<Self>) {
        if let (Some(h), Access::Full) = (&self.helper, self.access) {
            self.next_action += 1;
            h.send(&ToHelper::Act { id: self.next_action, action });
            return;
        }
        match action.run() {
            Ok(()) => self.refresh_soon(),
            Err(Refusal { message, needs_access: true }) if helper::way() != Way::Off => {
                let message = format!("{message} Activity Monitor can do it once you give it full access, with an administrator's password.");
                let me = cx.entity();
                cx.defer(move |cx| {
                    open_alert(cx, "Activity Monitor", "authentication", &message, &["Cancel", "Get Full Access…"], move |choice, cx| {
                        if choice == Some(1) {
                            me.update(cx, |a, cx| {
                                a.after_access.push(action.clone());
                                a.get_full_access(cx);
                            });
                        }
                    });
                });
            }
            Err(r) => self.alert(&r.message, cx),
        }
    }

    fn alert(&mut self, message: &str, cx: &mut Context<Self>) {
        let message = message.to_owned();
        cx.defer(move |cx| {
            open_alert(cx, "Activity Monitor", "stop", &message, &["OK"], |_, _| {});
        });
    }

    pub fn selected_proc(&self) -> Option<&Proc> {
        self.selected.and_then(|pid| self.processes.iter().find(|p| p.pid == pid))
    }

    pub fn proc(&self, pid: i32) -> Option<&Proc> {
        self.processes.iter().find(|p| p.pid == pid)
    }

    pub fn quit_process(&mut self, pid: i32, force: bool, cx: &mut Context<Self>) {
        let name = self.proc(pid).map_or_else(|| pid.to_string(), |p| p.name.clone());
        if !force {
            return self.act(Action::Signal { pid, signal: libc::SIGTERM }, cx);
        }
        let message = format!("Force “{name}” ({pid}) to quit? Any unsaved work in it will be lost.");
        let me = cx.entity();
        cx.defer(move |cx| {
            open_alert(cx, "Activity Monitor", "warning", &message, &["Cancel", "Force Quit"], move |choice, cx| {
                if choice == Some(1) {
                    me.update(cx, |a, cx| a.act(Action::Signal { pid, signal: libc::SIGKILL }, cx));
                }
            });
        });
    }

    /// Open (or bring forward) the inspector of `pid`.
    pub fn inspect(&mut self, pid: i32, cx: &mut Context<Self>) {
        self.inspect_at(pid, 0, cx);
    }

    /// The inspector of `pid` at `tab` (the supervisor's, from its alert).
    pub fn inspect_at(&mut self, pid: i32, tab: usize, cx: &mut Context<Self>) {
        if let Some((_, w, i)) = self.inspectors.iter().find(|(p, ..)| *p == pid) {
            if tab > 0 {
                i.update(cx, |i, cx| i.set_tab(tab, cx));
            }
            let w = *w;
            cx.defer(move |cx| {
                let _ = w.update(cx, |_, window, _| window.activate_window());
            });
            return;
        }
        // (Before the first sample, its name as the kernel has it.)
        let name = self.proc(pid).map(|p| p.name.clone()).or_else(|| Some(std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?.trim().to_owned())).unwrap_or_else(|| pid.to_string());
        let me = cx.entity();
        let view = cx.new(|cx| inspector::Inspector::new(pid, name.clone(), tab, me.clone(), cx));
        cx.defer(move |cx| {
            let v = view.clone();
            let opened = me.clone();
            let w = olo_ui::open_window(cx, WindowOptions::new(format!("{name} ({pid})"), 620.0, 520.0).min_size(460.0, 380.0), move |window, cx| {
                let closing = opened.clone();
                window.on_window_should_close(cx, move |window, cx| {
                    let id = window.window_handle().window_id();
                    closing.update(cx, |a, cx| a.inspector_closed(id, cx));
                    true
                });
                v
            });
            if let Some(w) = w {
                me.update(cx, |a, _| {
                    a.inspectors.push((pid, w, view));
                    a.request_details(pid);
                });
            }
        });
    }

    fn inspector_closed(&mut self, id: olo_ui::gpui::WindowId, cx: &mut Context<Self>) {
        self.inspectors.retain(|(_, w, _)| w.window_id() != id);
        // (Started for an inspector alone: its last window gone.)
        if self.main.is_none() && self.inspectors.is_empty() {
            self.quit(cx);
        }
    }

    /// A dialog to set one of the kernel's tunables.
    pub fn tune(&mut self, name: &str, cx: &mut Context<Self>) {
        let value = self.system.tunables.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone()).unwrap_or_default();
        let (me, name) = (cx.entity(), name.to_owned());
        cx.defer(move |cx| {
            let title = format!("Set {name}");
            olo_ui::open_window(cx, WindowOptions { fixed: true, ..WindowOptions::new(title, 420.0, 150.0).dialog() }, move |window, cx| {
                let view = cx.new(|cx| Tune::new(name, value, me, cx));
                let field = view.read(cx).field.clone();
                field.read(cx).focus(window);
                view
            });
        });
    }

    pub fn command(&mut self, command: &str, window: Option<&mut Window>, cx: &mut Context<Self>) {
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
            cx.notify();
            return self.store();
        }
        if let Some(v) = command.strip_prefix("remove-graph:").and_then(|v| v.parse::<usize>().ok()) {
            if self.settings.views.len() > 1 && v < self.settings.views.len() {
                self.settings.views.remove(v);
                self.store();
                cx.notify();
            }
            return;
        }
        if let Some(ms) = command.strip_prefix("interval:").and_then(|v| v.parse::<u64>().ok()) {
            self.settings.interval_ms = ms;
            self.wants_changed();
            cx.notify();
            return self.store();
        }
        if let Some(s) = command.strip_prefix("span:").and_then(|v| v.parse::<u32>().ok()) {
            self.settings.span = s;
            cx.notify();
            return self.store();
        }
        if let Some(name) = command.strip_prefix("section:")
            && let Ok(s) = serde_json::from_str::<Section>(&format!("\"{name}\""))
        {
            return self.set_section(s, cx);
        }
        // A process's commands: "signal:PID:N", "nice:PID:N".
        if let Some(rest) = command.strip_prefix("signal:")
            && let Some((pid, sig)) = rest.split_once(':')
            && let (Ok(pid), Ok(signal)) = (pid.parse(), sig.parse())
        {
            return self.act(Action::Signal { pid, signal }, cx);
        }
        if let Some(rest) = command.strip_prefix("nice:")
            && let Some((pid, n)) = rest.split_once(':')
            && let (Ok(pid), Ok(nice)) = (pid.parse(), n.parse())
        {
            return self.act(Action::Nice { pid, nice }, cx);
        }
        if let Some(pid) = command.strip_prefix("inspect:").and_then(|p| p.parse().ok()) {
            return self.inspect(pid, cx);
        }
        if command == "supervisor-preferences" {
            return sections::supervise(sections::Supervise::Preferences);
        }
        if let Some(name) = command.strip_prefix("supervisor-ignore:") {
            return sections::supervise(sections::Supervise::Ignore(name.to_owned()));
        }
        if let Some(pid) = command.strip_prefix("quit:").and_then(|p| p.parse().ok()) {
            return self.quit_process(pid, false, cx);
        }
        if let Some(pid) = command.strip_prefix("force-quit:").and_then(|p| p.parse().ok()) {
            return self.quit_process(pid, true, cx);
        }
        if let Some(pid) = command.strip_prefix("copy-pid:").and_then(|p| p.parse::<i32>().ok()) {
            return cx.write_to_clipboard(ClipboardItem::new_string(pid.to_string()));
        }
        if let Some(pid) = command.strip_prefix("copy-command:").and_then(|p| p.parse::<i32>().ok()) {
            let text = self.proc(pid).map(|p| p.command.clone()).unwrap_or_default();
            return cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
        if let Some(pid) = command.strip_prefix("show-program:").and_then(|p| p.parse::<i32>().ok()) {
            if let Ok(exe) = std::fs::read_link(format!("/proc/{pid}/exe")) {
                let _ = olo_ui::tracker::open(&exe);
            }
            return;
        }
        if let Some(path) = command.strip_prefix("open-path:") {
            let _ = olo_ui::tracker::open(std::path::Path::new(path));
            return;
        }
        if let Some(text) = command.strip_prefix("copy-text:") {
            return cx.write_to_clipboard(ClipboardItem::new_string(text.to_owned()));
        }
        if let Some(name) = command.strip_prefix("tune:") {
            return self.tune(name, cx);
        }
        if let Some(pid) = command.strip_prefix("select:").and_then(|p| p.parse::<i32>().ok()) {
            self.selected = Some(pid);
            self.reveal_selected = true;
            return self.set_section(Section::Processes, cx);
        }
        let pid = self.selected.filter(|_| self.settings.section == Section::Processes);
        match command {
            "add-graph" => {
                self.settings.views.push(vec!["cpu".into()]);
                self.set_section(Section::Graphs, cx);
            }
            "quit-process" => {
                if let Some(pid) = pid {
                    self.quit_process(pid, false, cx);
                }
            }
            "force-quit-process" => {
                if let Some(pid) = pid {
                    self.quit_process(pid, true, cx);
                }
            }
            "inspect-process" => {
                if let Some(pid) = pid {
                    self.inspect(pid, cx);
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
                self.set_section(Section::Processes, cx);
                if let Some(window) = window {
                    let search = self.search.clone();
                    cx.defer_in(window, move |_, window, cx| search.update(cx, |s, cx| s.focus(window, cx)));
                }
            }
            "close" => {
                if let Some(window) = window {
                    window.remove_window();
                }
            }
            "quit" => self.quit(cx),
            _ => {}
        }
        cx.notify();
    }

    fn quit(&mut self, cx: &mut Context<Self>) {
        self.store_now();
        cx.quit();
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

    /// The menu bar: the main window's (`pid`: an inspector's process).
    pub fn menus(&self, inspected: Option<i32>) -> Vec<Menu> {
        let mut interval = Menu::new("Update Interval");
        for (ms, label) in INTERVALS {
            interval = interval.item(label, format!("interval:{ms}")).checked(self.settings.interval_ms == ms);
        }
        let mut span = Menu::new("Graphs Show");
        for (s, label) in SPANS {
            span = span.item(label, format!("span:{s}")).checked(self.settings.span == s);
        }
        let mut view = Menu::new("View");
        for s in self.sections() {
            // Each section's key is its own, whichever this machine has.
            view = view.item(s.name(), s.command());
            if let Some(n) = Section::KEYED.iter().position(|x| *x == s) {
                view = view.key(&(n + 1).to_string());
            }
            view = view.checked(self.settings.section == s);
        }
        let view = view
            .separator()
            .item("Processes as a Tree", "tree")
            .checked(self.settings.tree)
            .item("Smooth Scrolling Graphs", "smooth")
            .checked(self.settings.smooth)
            .separator()
            .submenu(span)
            .submenu(interval);
        let pid = inspected.or(self.selected.filter(|_| self.settings.section == Section::Processes && self.selected_proc().is_some()));
        let any = pid.is_some();
        let mut process = Menu::new("Process")
            .item("Inspect", "inspect-process")
            .key("i")
            .enabled(any && inspected.is_none())
            .separator()
            .item("Quit", "quit-process")
            .enabled(any)
            .item("Force Quit…", "force-quit-process")
            .enabled(any);
        if let Some(pid) = pid {
            process = process.separator().submenu(processes::signal_menu(pid)).submenu(processes::priority_menu(pid, self.proc(pid).map_or(0, |p| p.nice)));
        }
        let full = match self.access {
            Access::Full => "Full Access",
            Access::Asking => "Asking for Full Access…",
            Access::Mine => "Get Full Access…",
        };
        vec![
            Menu::new("File")
                .item("Find Process…", "find")
                .key("f")
                .separator()
                .item("Add Graph", "add-graph")
                .separator()
                .item(full, "full-access")
                .enabled(self.access == Access::Mine && helper::way() != Way::Off)
                .checked(self.access == Access::Full)
                .item("Ask for Full Access at Start", "ask-full-access")
                .checked(self.settings.ask_full_access)
                .separator()
                .item("Close", "close")
                .key("w")
                .item("Quit", "quit")
                .key("q"),
            view,
            process,
        ]
    }

    fn message(&mut self, message: Box<dyn std::any::Any + Send>, cx: &mut Context<Self>) {
        if let Some(launch) = message.downcast_ref::<Launch>() {
            let args = launch.args.clone();
            if !inspecting(&args) {
                self.open(cx);
            }
            return self.follow_args(&args, cx);
        }
        let Ok(msg) = message.downcast::<Msg>() else { return };
        match *msg {
            Msg::Sources(s, from_helper) => self.take_sources(s, from_helper),
            Msg::Sample(s, from_helper) => return self.take_sample(*s, from_helper, cx),
            Msg::Details(pid, d) => {
                // Full access answers for inspectors now.
                if self.access != Access::Full {
                    self.take_details(pid, d.map(|d| *d), cx);
                }
                return;
            }
            Msg::Helper(m) => return self.heard(m, cx),
            Msg::Supervisor(s) => self.supervisor = *s,
        }
        cx.notify();
    }
}

impl Activity {
    fn menu_bar(&mut self, _: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let me = cx.entity();
        MenuBar::new(self.menus(None), move |cmd, window, cx| me.update(cx, |a, cx| a.command(cmd, Some(window), cx))).into_any_element()
    }

    /// The sidebar, each row with its figure now.
    fn sidebar(&mut self, _: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let sections = self.sections();
        let headings = [(0, "Activity"), (3, "Hardware"), (sections.len() - 2, "System")];
        let mut rows: Vec<Row> = Vec::new();
        let mut index_of: Vec<Option<Section>> = Vec::new();
        for (i, s) in sections.iter().enumerate() {
            if let Some((_, h)) = headings.iter().find(|(at, _)| *at == i) {
                rows.push(Row::heading(*h));
                index_of.push(None);
            }
            let value = sections::sidebar_value(self, *s);
            let row = Row::new(s.icon(), s.name());
            rows.push(if value.is_empty() { row } else { row.value(value) });
            index_of.push(Some(*s));
        }
        let chosen = index_of.iter().position(|s| *s == Some(self.settings.section)).unwrap_or(1);
        let pick = cx.entity();
        Sidebar::new("activity-sections", rows, chosen)
            .on_choose(move |i, _, cx| {
                if let Some(Some(s)) = index_of.get(i) {
                    let s = *s;
                    pick.update(cx, |a, cx| a.set_section(s, cx));
                }
            })
            .into_any_element()
    }
}

/// A part of the window drawn from the app, cached (`AnyView::cached`):
/// drawn again as the app changes (or a list it shows), its last frame
/// kept otherwise. GPUI draws again every view above one that changed:
/// the section's graphs moving a pixel redraw their layer and, around
/// it, only the window's few boxes; these parts are kept.
struct Part {
    app: Entity<Activity>,
    draw: fn(&mut Activity, &mut Window, &mut Context<Activity>) -> AnyElement,
    _observe: Vec<Subscription>,
}

impl Part {
    fn new(app: &Entity<Activity>, lists: &[Entity<ListModel>], draw: fn(&mut Activity, &mut Window, &mut Context<Activity>) -> AnyElement, cx: &mut Context<Self>) -> Part {
        // (A cached view isn't drawn again for a model it reads, only for
        // what it observes.)
        let mut observe = vec![cx.observe(app, |_, _, cx| cx.notify())];
        observe.extend(lists.iter().map(|l| cx.observe(l, |_, _, cx| cx.notify())));
        Part { app: app.clone(), draw, _observe: observe }
    }
}

impl Render for Part {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let draw = self.draw;
        self.app.update(cx, |a, cx| draw(a, window, cx))
    }
}

/// The window's cached parts, and the section's graphs' layer.
struct Parts {
    menus: Entity<Part>,
    sidebar: Entity<Part>,
    processes: Entity<Part>,
    cards: Entity<Part>,
    graphs: Entity<GraphLayer>,
}

impl Parts {
    fn new(app: &Entity<Activity>, cx: &mut App) -> Parts {
        let (lists, list) = {
            let a = app.read(cx);
            ([a.connections_list.clone(), a.tunables_list.clone()], a.list.clone())
        };
        let menus = cx.new(|cx| Part::new(app, &[], Activity::menu_bar, cx));
        let sidebar = cx.new(|cx| Part::new(app, &[], Activity::sidebar, cx));
        let processes = cx.new(|cx| Part::new(app, &[list], processes::list, cx));
        let cards = cx.new(|cx| Part::new(app, &lists, sections::cards, cx));
        let (shelf, now_of) = (app.read(cx).shelf.clone(), app.downgrade());
        let graphs = cx.new(|cx| {
            cx.observe(app, |_, _, cx| cx.notify()).detach();
            GraphLayer::new(shelf, move |cx| now_of.upgrade().map_or(0.0, |a| a.read(cx).now()))
        });
        Parts { menus, sidebar, processes, cards, graphs }
    }
}

impl Render for Activity {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.sections().contains(&self.settings.section) {
            self.settings.section = Section::Overview;
        }
        let Some(parts) = &self.parts else { return div().into_any_element() };
        let cached = |part: AnyView, style: StyleRefinement| part.cached(style);
        let (menus, sidebar, list, cards, graphs) = (parts.menus.clone(), parts.sidebar.clone(), parts.processes.clone(), parts.cards.clone(), parts.graphs.clone());
        let width = f32::from(window.viewport_size().width) - SIDEBAR_WIDTH;
        let content = if self.sources.is_empty() {
            div().into_any_element()
        } else if self.settings.section == Section::Processes {
            processes::view(self, width, list.into(), window, cx)
        } else {
            // The cards, and their graphs moving over them.
            div()
                .relative()
                .size_full()
                .child(cached(cards.into(), StyleRefinement::default().size_full()))
                .child(cached(graphs.into(), StyleRefinement::default().absolute().inset_0()))
                .into_any_element()
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(cached(menus.into(), StyleRefinement::default().w_full().h(px(menu::BAR_HEIGHT)).flex_shrink_0()))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_row()
                    .child(cached(sidebar.into(), StyleRefinement::default().w(px(SIDEBAR_WIDTH)).h_full().flex_shrink_0()))
                    .child(div().flex_1().min_w_0().h_full().child(content)),
            )
            .into_any_element()
    }
}

/// The dialog setting one of the kernel's tunables.
struct Tune {
    name: String,
    field: Entity<TextField>,
    app: Entity<Activity>,
    focus: FocusHandle,
    _sub: Subscription,
}

impl Tune {
    fn new(name: String, value: String, app: Entity<Activity>, cx: &mut Context<Self>) -> Tune {
        let field = text_field(cx, |f| f.name("Value").hint("A value"));
        field.update(cx, |f, cx| {
            f.set_text(value, cx);
            f.select_all_text(cx);
        });
        // (Set only once there's a value.)
        let sub = cx.subscribe(&field, |_: &mut Tune, _, _: &TextFieldEvent, cx| cx.notify());
        Tune { name, field, app, focus: cx.focus_handle(), _sub: sub }
    }

    fn done(&mut self, set: bool, window: &mut Window, cx: &mut Context<Self>) {
        let value = self.field.read(cx).text().trim().to_owned();
        if set && value.is_empty() {
            return;
        }
        if set {
            let name = self.name.clone();
            self.app.update(cx, |a, cx| a.act(Action::Sysctl { name, value }, cx));
        }
        window.remove_window();
    }
}

impl Render for Tune {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let me = cx.entity();
        let about = sample::TUNABLES.iter().find(|(n, _)| *n == self.name).map_or("", |(_, d)| *d);
        let ready = !self.field.read(cx).text().trim().is_empty();
        let (cancel, set) = (me.clone(), me);
        div()
            .size_full()
            .track_focus(&self.focus)
            // Return and Escape before the field has them.
            .capture_key_down(cx.listener(|t, e: &KeyDownEvent, window, cx| match e.keystroke.key.as_str() {
                "escape" => {
                    t.done(false, window, cx);
                    cx.stop_propagation();
                }
                "enter" => {
                    t.done(true, window, cx);
                    cx.stop_propagation();
                }
                _ => {}
            }))
            .child(
                AlertFrame::new("system")
                    .child(paragraph(format!("Set {} ({}).", self.name, about.to_lowercase())))
                    .child(div().mt(px(m::SPACING)).child(TextControl::new(self.field.clone()).label("Value:")))
                    .child(div().flex_1())
                    .child(
                        button_row()
                            .child(Button::new("Cancel").on_click(move |window, cx| cancel.update(cx, |t, cx| t.done(false, window, cx))))
                            .child(Button::new("Set").default(true).enabled(ready).on_click(move |window, cx| set.update(cx, |t, cx| t.done(true, window, cx)))),
                    ),
            )
    }
}

fn main() {
    // A session or install from before the rename (Tabula) carries on.
    olo_xdg::legacy::adopt();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--helper") {
        helper::serve();
    }
    if instance::forward(APP_ID, &args) {
        return;
    }
    olo_ui::run(APP_ID, move |cx| {
        let proxy = olo_ui::proxy(cx);
        instance::listen(APP_ID, proxy.clone());
        let settings: Settings =
            std::fs::read_to_string(settings_path()).ok().and_then(|t| serde_json::from_str::<Settings>(&t).ok()).map(Settings::migrate).unwrap_or_default();
        let control = sample::Control::new(settings.interval_ms);
        let (p1, p2) = (proxy.clone(), proxy.clone());
        sample::start(control.clone(), move |s| p1.send(Msg::Sources(s, false)), move |s| p2.send(Msg::Sample(Box::new(s), false)));
        // Inspectors' details, read off the UI thread.
        let (inspect_tx, inspect_rx) = mpsc::channel::<i32>();
        let details_proxy = proxy.clone();
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
                        details_proxy.send(Msg::Details(pid, details::read_details(pid).map(Box::new)));
                    }
                }
            })
            .expect("the inspect thread");
        let ask = settings.ask_full_access;
        let search_text = settings.search.clone();
        let app = cx.new(|cx| {
            let list = ListModel::new(cx, false);
            let search = search_field(cx, "Search: name, command, user:, state:, kind:");
            search.update(cx, |s, cx| s.set_text(search_text, cx));
            let connections_search = text_field(cx, |f| f.name("Search").hint("Address, port, program or state"));
            let subs = vec![
                cx.subscribe(&search, |a: &mut Activity, s, e: &SearchEvent, cx| {
                    if let SearchEvent::Changed = e {
                        a.settings.search = s.read(cx).text(cx).to_owned();
                        a.rows_cache = None;
                        a.store();
                        cx.notify();
                    }
                }),
                cx.subscribe(&connections_search, |_: &mut Activity, _, e: &TextFieldEvent, cx| {
                    if let TextFieldEvent::Changed = e {
                        cx.notify();
                    }
                }),
            ];
            Activity {
                main: None,
                inspectors: Vec::new(),
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
                list,
                selected: None,
                collapsed: HashSet::new(),
                rows_cache: None,
                facets: None,
                reveal_selected: false,
                search,
                connections_list: ListModel::new(cx, false),
                connections_search,
                // SAFETY: getuid has no preconditions.
                uid: unsafe { libc::getuid() },
                // SAFETY: sysconf has no preconditions.
                hz: unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as f64,
                access: Access::Mine,
                helper: None,
                after_access: Vec::new(),
                next_action: 0,
                inspect_tx,
                tunables_list: ListModel::new(cx, false),
                supervisor: Default::default(),
                proxy: proxy.clone(),
                scroll: Default::default(),
                shelf: GraphShelf::default(),
                parts: None,
                _subscriptions: subs,
            }
        });
        let parts = Parts::new(&app, cx);
        app.update(cx, |a, _| a.parts = Some(parts));
        let messages = app.clone();
        olo_ui::message_handler(cx, move |message, cx| messages.update(cx, |a, cx| a.message(message, cx)));
        // Its settings kept as it ends, by hand or for an upgrade.
        let saving = app.clone();
        olo_ui::on_upgrade(cx, move |cx| {
            saving.read(cx).store_now();
            Some(String::new())
        });
        // The supervisor's work, followed as it publishes it.
        {
            let read = move || proxy.send(Msg::Supervisor(Box::new(olo_protocol::performance::SupervisorState::load())));
            let read = std::sync::Arc::new(read);
            let first = read.clone();
            std::thread::spawn(move || first());
            if let Some(w) = olo_ui::watch::watch([olo_protocol::performance::SupervisorState::path()], move |_| read()) {
                // For as long as the app runs.
                std::mem::forget(w);
            }
        }
        // Started to show a process (the supervisor's alert), or a section.
        let start_args: Vec<String> = std::env::args().skip(1).collect();
        app.update(cx, |a, cx| {
            a.wants_changed();
            // (Asked for a process: its inspector alone, in front.)
            if !inspecting(&start_args) {
                a.open(cx);
            }
            a.follow_args(&start_args, cx);
            // SAFETY: geteuid has no preconditions.
            let root = unsafe { libc::geteuid() } == 0;
            if ask && !root && helper::way() != Way::Off {
                a.get_full_access(cx);
            }
        });
    });
}

/// Whether `args` ask for a process's inspector (`--inspect PID`).
fn inspecting(args: &[String]) -> bool {
    args.iter().any(|a| a == "--inspect")
}
