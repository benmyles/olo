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
//!
//! On olo-kit: the app's looper is a hub ([`Hub`]). The samplers, the
//! helper and the windows tell it things ([`Msg`], through the app's
//! proxy); it keeps who measures and what this app may do, and tells
//! the main window (the [`Activity`] view, on its window's looper) and
//! each inspector (on its own) what they show.

mod actions;
mod details;
// (Its GPUs, shared with Deskbar's widgets.)
use olo_system::gpu_load as gpus;
mod helper;
mod inspector;
use olo_system::netdiag;
mod palms;
mod processes;
mod sample;
mod sections;
mod smart;

use std::{
    collections::{HashMap, HashSet, VecDeque},
    rc::Rc,
    sync::{Arc, atomic::Ordering, mpsc},
    time::Instant,
};

use actions::{Action, Refusal};
use details::Details;
use helper::{FromHelper, Helper, ToHelper, Way};
use olo_kit::{
    Bounds, Child, ClickEvent, ClipboardItem, Cx, Element, FocusHandle, InteractiveElement as _, IntoElement as _, KeyDownEvent, LocalWindow, Menu, MenuBar, ParentElement as _, Pixels, ScrollHandle,
    Styled as _, View, WindowCx, WindowHandle, WindowOptions, clipboard, div,
    instance::{self, Launch},
    metrics as m, px,
    search::{SearchEvent, SearchField, search_field},
    widgets::{AlertFrame, Button, ListModel, Sidebar, TextControl, TextField, TextFieldEvent, button_row, open_alert, paragraph, sidebar::Row, text_field},
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
    /// Each display's frames: how app_server keeps up (`Request::Frames`).
    Displays,
    /// The pointer, the touchpad and what's done with each kind of input.
    Inputs,
    /// The models on this computer: brush's command suggestions
    /// (olo-complete's account of itself).
    Ai,
    Sensors,
    System,
    Graphs,
}

impl Section {
    const ALL: [Section; 14] = [
        Section::Overview,
        Section::Processes,
        Section::Supervisor,
        Section::Cpu,
        Section::Memory,
        Section::Disks,
        Section::Network,
        Section::Gpu,
        Section::Displays,
        Section::Inputs,
        Section::Ai,
        Section::Sensors,
        Section::System,
        Section::Graphs,
    ];
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
            Section::Displays => "Displays",
            Section::Inputs => "Inputs",
            Section::Ai => "AI",
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
            Section::Displays => "display",
            Section::Inputs => "touchpad",
            Section::Ai => "ai",
            Section::Sensors => "sensors",
            Section::System => "system",
            Section::Graphs => "disk-usage",
        }
    }

    fn command(self) -> String {
        format!("section:{}", serde_json::to_string(&self).unwrap_or_default().trim_matches('"'))
    }
}

#[derive(Clone, Serialize, Deserialize)]
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
            views: vec![vec!["cpu".into(), "memory".into(), "cached".into()], vec!["net-rx".into(), "net-tx".into(), "disk-read".into(), "disk-write".into()]],
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
    olo_xdg::config_dir().join("activity.json")
}

/// What the app is told (through its proxy, by the samplers, the helper
/// and its windows), and what it tells the main window.
pub enum Msg {
    /// From the sampler here (false) or the helper's (true).
    Sources(Vec<Source>, bool),
    Sample(Box<Sample>, bool),
    Details(i32, Result<Box<Details>, String>),
    Helper(Option<FromHelper>),
    /// What the supervisor published (`olo-supervisor.json`).
    Supervisor(Box<olo_protocol::performance::SupervisorState>),
    /// How each display keeps up, whether drops are captured, and the
    /// pointer's reports (`Request::Frames`, from `frames_poller`).
    Frames(Vec<olo_protocol::DisplayFrames>, bool, olo_protocol::PointerHealth, olo_protocol::TouchHealth, olo_protocol::InputActivity),
    /// Whether brush's suggestions are on, and olo-complete's account
    /// (None: not running), from `model_poller`.
    Model(bool, Option<Box<olo_complete::protocol::Stats>>),
    /// What the palm model made of the touchpad's touches in the last
    /// minute (`palms`, while the Inputs section shows).
    Palms(palms::PalmStats),

    // A window's, to the app.
    /// The main window's settings changed: kept, and inspectors told.
    Settings(Box<Settings>),
    /// What's measured changed (`sample::Control`): whoever measures is told.
    Wants,
    /// With full access: the helper does these.
    Act(Vec<Action>),
    /// Something was done: the inspected are measured again now, not at
    /// the next sample.
    Acted,
    /// Ask for full access (polkit asks for the password), and do these
    /// once it comes.
    GetAccess(Vec<Action>),
    /// Open (or bring forward) a process's inspector at a tab, with what
    /// the asker knows of it.
    Inspect(i32, usize, Option<Box<inspector::Seen>>),
    InspectorClosed(i32),
    /// A command from an inspector's menus that is the app's.
    Command(String),

    // The app's, to the main window.
    Access(Access),
    Alert(String),
    /// From the main window's own dialogs: do these.
    Do(Vec<Action>),
}

/// Tells the app (its looper, from any thread).
pub fn tell(msg: Msg) {
    olo_kit::proxy().send(msg);
}

/// Asks olo-complete how its model does, every second while the AI
/// section shows and at once when it comes to show; nothing otherwise
/// (`want`: whether it shows). It answers in microseconds; one not
/// running is asked nothing (the socket refuses at once).
fn model_poller(send: impl Fn(Msg) + Send + 'static, rx: mpsc::Receiver<bool>) {
    use olo_complete::protocol::{self, Reply, Request, Settings};
    let mut want = false;
    loop {
        let ask = if want { rx.recv_timeout(std::time::Duration::from_secs(1)) } else { rx.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected) };
        match ask {
            Ok(w) => want = w,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
        if !want {
            continue;
        }
        // (Headless: a model a while in use, to look at.)
        if std::env::var_os("OLO_ACTIVITY_DEMO_AI").is_some() {
            send(Msg::Model(true, Some(Box::new(protocol::demo_stats()))));
            continue;
        }
        let on = Settings::load().enabled;
        let stats = match on.then(|| protocol::ask(&Request::Stats, std::time::Duration::from_millis(200))).flatten() {
            Some(Reply::Stats(s)) => Some(Box::new(s)),
            _ => None,
        };
        send(Msg::Model(on, stats));
    }
}

/// The touchpad to look at under OLO_ACTIVITY_DEMO_FRAMES: a MacBook's,
/// its trained palm model in use.
fn demo_touch() -> olo_protocol::TouchHealth {
    olo_protocol::TouchHealth {
        reports_second: 125,
        reports_minute: 6_480,
        latency_p50_us: 410,
        latency_p99_us: 1_900,
        latency_max_us: 3_600,
        filter: true,
        model: true,
        trained: 1_793_577_600,
        contacts: 412,
        palms_caught: 0.97,
        fingers_lost: 0.012,
        nice: -10,
        realtime: false,
        util_min: 1024,
    }
}

/// Displays to look at under OLO_ACTIVITY_DEMO_FRAMES (a headless
/// session has none): a 5K at 60 Hz with a video playing and drops, a
/// 120 Hz built-in, a resting one.
fn demo_frames() -> Vec<olo_protocol::DisplayFrames> {
    use olo_protocol::DisplayFrames;
    vec![
        DisplayFrames {
            output: "DP-1".into(),
            refresh_mhz: 60_000,
            fps: 48.0,
            frames: 184_211,
            late: 1_902,
            late_minute: 7,
            render_min_us: 410,
            render_p50_us: 4_900,
            render_p95_us: 7_400,
            render_max_us: 9_600,
            budget_us: 9_750,
            delay_us: 6_917,
            cursor_plane: true,
            scanout: "1 on a plane; 7 drawn (1 overlaps, 6 not tried)".into(),
            resting: false,
        },
        DisplayFrames {
            output: "eDP-1".into(),
            refresh_mhz: 120_000,
            fps: 120.0,
            frames: 402_118,
            late: 3,
            late_minute: 0,
            render_min_us: 180,
            render_p50_us: 700,
            render_p95_us: 2_900,
            render_max_us: 4_100,
            budget_us: 4_125,
            delay_us: 4_208,
            cursor_plane: true,
            scanout: "1 on a plane; 7 drawn (1 overlaps, 6 not tried)".into(),
            resting: false,
        },
        DisplayFrames {
            output: "DP-2".into(),
            refresh_mhz: 60_000,
            fps: 0.0,
            frames: 9_120,
            late: 0,
            late_minute: 0,
            render_min_us: 90,
            render_p50_us: 110,
            render_p95_us: 700,
            render_max_us: 1_300,
            budget_us: 6_667,
            delay_us: 10_000,
            cursor_plane: false,
            scanout: "0 on a plane; 3 drawn (3 not tried)".into(),
            resting: true,
        },
    ]
}

/// To `frames_poller`: whether the Displays section shows (asking every
/// second while it does), and to capture drops or not.
enum FramesAsk {
    Want(bool),
    Capture(bool),
}

/// Asks app_server how each display keeps up, every second while the
/// Displays section shows and at once when it comes to show; nothing
/// otherwise. Capturing drops is app_server's to do (`CaptureDrops`).
fn frames_poller(send: impl Fn(Msg) + Send + 'static, rx: mpsc::Receiver<FramesAsk>) {
    use olo_protocol::{Client, Reply, Request};
    let mut want = false;
    let mut capture = false;
    let mut client: Option<Client> = None;
    loop {
        let ask = if want { rx.recv_timeout(std::time::Duration::from_secs(1)) } else { rx.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected) };
        match ask {
            Ok(FramesAsk::Want(w)) => want = w,
            Ok(FramesAsk::Capture(on)) => {
                capture = on;
                if client.is_none() {
                    client = Client::connect().ok();
                }
                if let Some(c) = client.as_mut()
                    && c.request(&Request::CaptureDrops { on }).is_err()
                {
                    client = None;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
        if !want {
            continue;
        }
        // (Headless test sessions have no displays: made-up ones to look at.)
        if std::env::var_os("OLO_ACTIVITY_DEMO_FRAMES").is_some() {
            send(Msg::Frames(
                demo_frames(),
                capture,
                olo_protocol::PointerHealth { reports_second: 998, gaps_minute: 3, longest_gap_us: 31_000, interval_us: 1_000 },
                demo_touch(),
                olo_protocol::InputActivity { clicks_minute: 42, scrolls_minute: 1_280, gestures_minute: 7, keys_minute: 310 },
            ));
            continue;
        }
        if client.is_none() {
            client = Client::connect().ok();
        }
        let Some(c) = client.as_mut() else { continue };
        match c.request(&Request::Frames) {
            Ok(Reply::Frames { displays, capture, pointer, touch, inputs, .. }) => send(Msg::Frames(displays, capture, pointer, touch, inputs)),
            _ => client = None,
        }
    }
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

/// How long the frames' figures are kept (the longest a graph shows).
pub const FRAME_HISTORY_SECONDS: f64 = 1800.0;

/// The busiest processes at a moment, for looking back from a graph.
pub struct Moment {
    pub at: f64,
    pub cpu: Vec<(i32, String, f64)>,
    pub disk: Vec<(i32, String, f64)>,
}

/// What every window knows of the app, as the hub last said: an
/// inspector's menus, graph and tabs go by it.
#[derive(Clone)]
pub struct Shared {
    pub settings: Settings,
    pub access: Access,
    /// The sections this machine has.
    pub sections: Vec<Section>,
    pub supervisor: Arc<olo_protocol::performance::SupervisorState>,
    /// When the app started: samples' times count from it.
    pub clock: Instant,
}

impl Shared {
    /// The time graphs show at their right edge, the latest sample
    /// having come at `last`: an interval behind it, so their lines
    /// reach the edge and scroll smoothly into it rather than jump at
    /// each sample. A clock for `Graph::live`.
    pub fn graph_clock(&self, last: Option<f64>) -> impl Fn() -> f64 + 'static {
        let (clock, interval, smooth) = (self.clock, self.settings.interval_ms as f64 / 1000.0, self.settings.smooth);
        move || graph_now(clock.elapsed().as_secs_f64(), last, interval, smooth)
    }
}

fn graph_now(t: f64, last: Option<f64>, interval: f64, smooth: bool) -> f64 {
    let Some(at) = last else { return t };
    if smooth { at + (t - at).min(interval) - interval } else { at }
}

/// The sections a machine with these has.
fn sections_for(gpu: bool, sensors: bool) -> Vec<Section> {
    Section::ALL
        .into_iter()
        .filter(|s| match s {
            Section::Gpu => gpu,
            // (Under app_server: a window of its own to ask.)
            Section::Displays | Section::Inputs => olo_protocol::socket_path().is_some(),
            Section::Sensors => sensors,
            _ => true,
        })
        .collect()
}

/// Whether `sources` have a GPU's, and a sensor's or the battery's.
fn has_gpu_and_sensors<'a>(sources: impl IntoIterator<Item = &'a Source>) -> (bool, bool) {
    sources.into_iter().fold((false, false), |(gpu, sensors), s| (gpu || s.key == "gpu-busy", sensors || s.group == sample::Group::Sensors || s.key == "battery"))
}

/// The menu bar: the main window's, or an inspector's (`inspected`),
/// its Process menu for `pids` (those chosen, or the inspected one),
/// whose nice values are `nices`.
pub fn menus(settings: &Settings, sections: &[Section], access: Access, pids: &[i32], nices: &[i32], inspected: bool) -> Vec<Menu> {
    let mut interval = Menu::new("Update Interval");
    for (ms, label) in INTERVALS {
        interval = interval.item(label, format!("interval:{ms}")).checked(settings.interval_ms == ms);
    }
    let mut span = Menu::new("Graphs Show");
    for (s, label) in SPANS {
        span = span.item(label, format!("span:{s}")).checked(settings.span == s);
    }
    let mut view = Menu::new("View");
    for s in sections {
        // Each section's key is its own, whichever this machine has.
        view = view.item(s.name(), s.command());
        if let Some(n) = Section::KEYED.iter().position(|x| x == s) {
            view = view.key(&(n + 1).to_string());
        }
        view = view.checked(settings.section == *s);
    }
    let view =
        view.separator().item("Processes as a Tree", "tree").checked(settings.tree).item("Smooth Scrolling Graphs", "smooth").checked(settings.smooth).separator().submenu(span).submenu(interval);
    let any = !pids.is_empty();
    let mut process = Menu::new("Process")
        .item("Inspect", "inspect-process")
        .key("i")
        .enabled(any && !inspected)
        .separator()
        .item("Quit", "quit-process")
        .enabled(any)
        .item("Force Quit…", "force-quit-process")
        .enabled(any);
    if any {
        process = process.separator().submenu(processes::signal_menu(pids)).submenu(processes::priority_menu(pids, nices));
    }
    let full = match access {
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
            .enabled(access == Access::Mine && helper::way() != Way::Off)
            .checked(access == Access::Full)
            .item("Ask for Full Access at Start", "ask-full-access")
            .checked(settings.ask_full_access)
            .separator()
            .item("Close", "close")
            .key("w")
            .item("Quit", "quit")
            .key("q"),
        view,
        process,
    ]
}

/// Do `actions` (one for each process chosen) from a window: here when
/// this account may, else by the helper. Those refused for want of
/// access are asked about once, the rest's refusals said once.
pub fn act_all(access: Access, actions: Vec<Action>, cx: &mut WindowCx) {
    if actions.is_empty() {
        return;
    }
    if access == Access::Full {
        return tell(Msg::Act(actions));
    }
    let one = actions.len() == 1;
    let (mut needs, mut other, mut why) = (Vec::new(), Vec::new(), String::new());
    for action in actions {
        match action.run() {
            Ok(()) => {}
            Err(Refusal { needs_access: true, message }) if helper::way() != Way::Off => {
                why = message;
                needs.push(action);
            }
            Err(r) => other.push(r.message),
        }
    }
    tell(Msg::Acted);
    if let Some(first) = other.first() {
        let message = if other.len() == 1 { first.clone() } else { format!("{first} ({} more were refused too.)", other.len() - 1) };
        alert(&message, cx);
    }
    if !needs.is_empty() {
        let message = if one {
            format!("{why} Activity Monitor can do it once you give it full access, with an administrator's password.")
        } else {
            format!("{} of them belong to someone else. Activity Monitor can act on those once you give it full access, with an administrator's password.", needs.len())
        };
        open_alert(cx, "Activity Monitor", "authentication", &message, &["Cancel", "Get Full Access…"], move |choice, _| {
            if *choice == Some(1) {
                tell(Msg::GetAccess(needs.clone()));
            }
        });
    }
}

/// Quit `pids` (SIGTERM), or force them to (SIGKILL), asked first: one
/// alert naming them all (`names`).
pub fn quit_processes(access: Access, pids: Vec<i32>, names: String, force: bool, cx: &mut WindowCx) {
    if pids.is_empty() {
        return;
    }
    if !force {
        return act_all(access, pids.into_iter().map(|pid| Action::Signal { pid, signal: libc::SIGTERM }).collect(), cx);
    }
    let message = match pids.as_slice() {
        [pid] => format!("Force “{names}” ({pid}) to quit? Any unsaved work in it will be lost."),
        _ => format!("Force {} processes to quit ({names})? Any unsaved work in them will be lost.", pids.len()),
    };
    open_alert(cx, "Activity Monitor", "warning", &message, &["Cancel", "Force Quit"], move |choice, cx| {
        if *choice == Some(1) {
            act_all(access, pids.iter().map(|&pid| Action::Signal { pid, signal: libc::SIGKILL }).collect(), cx);
        }
    });
}

/// Something that couldn't be done, said.
pub fn alert(message: &str, cx: &mut WindowCx) {
    open_alert(cx, "Activity Monitor", "stop", message, &["OK"], |_, _| {});
}

/// What the main window is made with that only it has: who to ask for
/// the displays' frames and the model's account.
struct Pollers {
    frames: mpsc::Sender<FramesAsk>,
    model_ask: mpsc::Sender<bool>,
}

/// The main window.
pub struct Activity {
    pub settings: Settings,
    control: sample::Control,
    pub sources: Vec<Source>,
    /// Each source's samples, (time, value), oldest first.
    pub history: Vec<Rc<VecDeque<(f64, f64)>>>,
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
    pub list: ListModel,
    /// The processes chosen in the table, in its order: what its row
    /// menu, the buttons under it and the Process menu act on.
    pub selected: Vec<i32>,
    pub collapsed: HashSet<i32>,
    pub rows_cache: Option<(u64, processes::RowsKey, Rc<processes::Rows>)>,
    /// Where each tree row's triangle was drawn, for a press on it (kept
    /// from frame to frame: a row drawn as it was isn't drawn again to
    /// say).
    pub triangles: Rc<std::cell::RefCell<HashMap<usize, Bounds<Pixels>>>>,
    /// The process list's filters, for the processes of a generation.
    pub facets: Option<(u64, Rc<Vec<olo_kit::search::Facet>>)>,
    pub reveal_selected: bool,
    pub search: Child<SearchField>,
    pub connections_list: ListModel,
    pub connections_search: Child<TextField>,
    pub uid: u32,
    pub hz: f64,
    pub access: Access,
    pub tunables_list: ListModel,
    /// What the supervisor is doing, as it published it.
    pub supervisor: olo_protocol::performance::SupervisorState,
    /// How each display keeps up (the Displays section), as app_server
    /// last said, and whether it captures drops.
    pub displays: Vec<olo_protocol::DisplayFrames>,
    pub frames_capture: bool,
    pub pointer_health: olo_protocol::PointerHealth,
    /// The touchpad's reports and the palm model (the Touchpad card).
    pub touch_health: olo_protocol::TouchHealth,
    /// The palm model's judging, live (`palms`), and its watch.
    pub palm_stats: Option<palms::PalmStats>,
    palm_watch: palms::Watch,
    /// Clicks, scrolls, gestures and keys in the last minute.
    pub inputs: olo_protocol::InputActivity,
    /// The frames' figures over time, by key ("pointer.hz",
    /// "display.NAME.fps", …), as each `Request::Frames` answer comes;
    /// the model's too ("ai.p50", …).
    pub frame_history: HashMap<String, Rc<VecDeque<(f64, f64)>>>,
    frames: mpsc::Sender<FramesAsk>,
    /// brush's suggestions: on or not, and olo-complete's account (the
    /// AI section), as `model_poller` last heard.
    pub model_on: bool,
    pub model: Option<Box<olo_complete::protocol::Stats>>,
    model_ask: mpsc::Sender<bool>,
    /// The sections' scrolling, kept while each shows.
    pub scroll: ScrollHandle,
    /// The dialog setting a tunable, while it's open.
    tune: Option<LocalWindow<Tune>>,
}

impl Activity {
    fn new(shared: Shared, control: sample::Control, sources: [Vec<Source>; 2], pollers: Pollers, cx: &mut Cx<Self>) -> Activity {
        let search = search_field(cx, "Search: name, command, user:, state:, kind:");
        let text = shared.settings.search.clone();
        search.update(cx, |s, cx| s.set_text(text, cx));
        cx.subscribe(&search, |a: &mut Activity, e: &SearchEvent, cx| {
            if let SearchEvent::Changed = e {
                a.settings.search = a.search.read(cx).text(cx).to_owned();
                a.rows_cache = None;
                a.store();
                cx.invalidate();
            }
        });
        let connections_search = text_field(cx, |f| f.name("Search").hint("Address, port, program or state"));
        cx.subscribe(&connections_search, |_: &mut Activity, e: &TextFieldEvent, cx| {
            if let TextFieldEvent::Changed = e {
                cx.invalidate();
            }
        });
        let mut a = Activity {
            settings: shared.settings,
            control,
            sources: Vec::new(),
            history: Vec::new(),
            by_key: HashMap::new(),
            map_local: Vec::new(),
            map_helper: Vec::new(),
            clock: shared.clock,
            last: None,
            system: Box::default(),
            processes: Vec::new(),
            generation: 0,
            connections: Vec::new(),
            moments: VecDeque::new(),
            drill: None,
            // (Several processes may be chosen, and acted on at once.)
            list: ListModel::new(true),
            selected: Vec::new(),
            collapsed: HashSet::new(),
            rows_cache: None,
            triangles: Default::default(),
            facets: None,
            reveal_selected: false,
            search,
            connections_list: ListModel::new(false),
            connections_search,
            // SAFETY: getuid has no preconditions.
            uid: unsafe { libc::getuid() },
            // SAFETY: sysconf has no preconditions.
            hz: unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as f64,
            access: shared.access,
            tunables_list: ListModel::new(false),
            supervisor: (*shared.supervisor).clone(),
            displays: Vec::new(),
            frames_capture: false,
            pointer_health: Default::default(),
            touch_health: Default::default(),
            palm_stats: None,
            palm_watch: Default::default(),
            inputs: Default::default(),
            frame_history: HashMap::new(),
            frames: pollers.frames,
            model_on: false,
            model: None,
            model_ask: pollers.model_ask,
            scroll: ScrollHandle::new(),
            tune: None,
        };
        // (What the samplers measure, as they said before it opened.)
        let [local, helper] = sources;
        a.take_sources(local, false);
        a.take_sources(helper, true);
        a.wants_changed();
        a
    }

    /// The settings kept (the app keeps them, on a worker, and tells
    /// the inspectors).
    pub fn store(&self) {
        tell(Msg::Settings(Box::new(self.settings.clone())));
    }

    /// The settings kept now (before the process ends or is replaced).
    fn store_now(&self) {
        let _ = olo_kit::store::write(&settings_path(), &serde_json::to_vec_pretty(&self.settings).unwrap_or_default());
    }

    /// Seconds since the app started: samples' times.
    pub fn t(&self) -> f64 {
        self.clock.elapsed().as_secs_f64()
    }

    /// The time graphs show at their right edge now (`graph_now`).
    pub fn now(&self) -> f64 {
        graph_now(self.t(), self.last, self.settings.interval_ms as f64 / 1000.0, self.settings.smooth)
    }

    /// The same as time passes: a moving graph's clock.
    pub fn graph_clock(&self) -> impl Fn() -> f64 + 'static {
        let (clock, last, interval, smooth) = (self.clock, self.last, self.settings.interval_ms as f64 / 1000.0, self.settings.smooth);
        move || graph_now(clock.elapsed().as_secs_f64(), last, interval, smooth)
    }

    /// The time itself, for graphs of what's asked for as it's shown
    /// (the displays' frames, the model's account).
    pub fn time_clock(&self) -> impl Fn() -> f64 + 'static {
        let clock = self.clock;
        move || clock.elapsed().as_secs_f64()
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

    /// A sample from whoever measures now (the app passes on no other).
    fn take_sample(&mut self, s: Sample, from_helper: bool, cx: &mut Cx<Self>) {
        let at = self.t();
        let map = if from_helper { &self.map_helper } else { &self.map_local };
        for (i, v) in s.values.iter().enumerate() {
            if let Some(&j) = map.get(i) {
                Rc::make_mut(&mut self.history[j]).push_back((at, *v));
            }
        }
        for h in &mut self.history {
            if h.front().is_some_and(|(t, _)| *t < at - KEEP_SECONDS) {
                let h = Rc::make_mut(h);
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
            self.processes = list;
            self.generation += 1;
        }
        if let Some(c) = s.connections {
            self.connections = c;
        }
        cx.invalidate();
    }

    /// Tell whoever measures what to measure.
    pub fn wants_changed(&mut self) {
        let section = self.settings.section;
        self.control.interval_ms.store(self.settings.interval_ms, Ordering::Relaxed);
        // Processes always (the overview's busiest, and looking back);
        // connections and the GPU's users only while they show.
        self.control.want_processes.store(true, Ordering::Relaxed);
        self.control.want_connections.store(section == Section::Network, Ordering::Relaxed);
        // (Processes show each GPU's use too.)
        self.control.want_gpu_clients.store(matches!(section, Section::Gpu | Section::Processes), Ordering::Relaxed);
        let _ = self.frames.send(FramesAsk::Want(matches!(section, Section::Displays | Section::Inputs)));
        // (The pad's touches judged as the filter does, only while shown.)
        self.palm_watch.want(section == Section::Inputs, move |s| tell(Msg::Palms(s)));
        let _ = self.model_ask.send(section == Section::Ai);
        // (The helper, if it measures, hears of it from the app.)
        tell(Msg::Wants);
    }

    /// Have app_server log what went into every dropped frame (`on`).
    pub fn set_frames_capture(&mut self, on: bool, cx: &mut Cx<Self>) {
        self.frames_capture = on;
        let _ = self.frames.send(FramesAsk::Capture(on));
        cx.invalidate();
    }

    pub fn set_section(&mut self, section: Section, cx: &mut Cx<Self>) {
        if self.settings.section != section {
            self.scroll = ScrollHandle::new();
        }
        self.settings.section = section;
        self.wants_changed();
        self.store();
        cx.invalidate();
    }

    /// Ask for full access (polkit asks for the password).
    pub fn get_full_access(&mut self, _: &mut Cx<Self>) {
        tell(Msg::GetAccess(Vec::new()));
    }

    /// Do `action`: here when this account may, else by the helper.
    pub fn act(&mut self, action: Action, cx: &mut Cx<Self>) {
        act_all(self.access, vec![action], cx);
    }

    pub fn act_all(&mut self, actions: Vec<Action>, cx: &mut Cx<Self>) {
        act_all(self.access, actions, cx);
    }

    /// The first process chosen (the one Inspect opens).
    pub fn selected_proc(&self) -> Option<&Proc> {
        self.selected.first().and_then(|&pid| self.processes.iter().find(|p| p.pid == pid))
    }

    /// The processes an action on the chosen ones reaches: those chosen
    /// and still running, and in Tree, a collapsed one's children with
    /// it (its row stands for them).
    pub fn chosen_pids(&self) -> Vec<i32> {
        let mut out: Vec<i32> = self.selected.iter().copied().filter(|&pid| self.proc(pid).is_some()).collect();
        if self.settings.tree {
            // A collapsed row stands for everything under it.
            let mut stack: Vec<i32> = out.iter().copied().filter(|p| self.collapsed.contains(p)).collect();
            while let Some(parent) = stack.pop() {
                for p in &self.processes {
                    if p.ppid == parent && p.pid != parent && !out.contains(&p.pid) {
                        out.push(p.pid);
                        stack.push(p.pid);
                    }
                }
            }
        }
        out
    }

    /// The nice values of `pids` (those still running).
    pub fn nices(&self, pids: &[i32]) -> Vec<i32> {
        pids.iter().filter_map(|&pid| self.proc(pid).map(|p| p.nice)).collect()
    }

    /// "chrome ×11, olo-fuzz": what `pids` are, by name, most first.
    pub fn names_of(&self, pids: &[i32]) -> String {
        let mut counts: Vec<(String, usize)> = Vec::new();
        for pid in pids {
            let name = self.proc(*pid).map_or_else(|| pid.to_string(), |p| p.name.clone());
            match counts.iter_mut().find(|(n, _)| *n == name) {
                Some((_, c)) => *c += 1,
                None => counts.push((name, 1)),
            }
        }
        counts.sort_by(|a, b| b.1.cmp(&a.1));
        counts.into_iter().map(|(n, c)| if c > 1 { format!("{n} ×{c}") } else { n }).collect::<Vec<_>>().join(", ")
    }

    pub fn proc(&self, pid: i32) -> Option<&Proc> {
        self.processes.iter().find(|p| p.pid == pid)
    }

    pub fn quit_process(&mut self, pid: i32, force: bool, cx: &mut Cx<Self>) {
        self.quit_processes(vec![pid], force, cx);
    }

    /// Quit `pids`, or force them to, asked first.
    pub fn quit_processes(&mut self, pids: Vec<i32>, force: bool, cx: &mut Cx<Self>) {
        let names = self.names_of(&pids);
        quit_processes(self.access, pids, names, force, cx);
    }

    /// Open (or bring forward) the inspector of `pid`: the app's to do,
    /// told what's known of the process now.
    pub fn inspect(&mut self, pid: i32, cx: &mut Cx<Self>) {
        self.inspect_at(pid, 0, cx);
    }

    /// The inspector of `pid` at `tab` (the supervisor's findings).
    pub fn inspect_at(&mut self, pid: i32, tab: usize, _: &mut Cx<Self>) {
        let seen = inspector::Seen::of(&self.processes, &self.system, pid);
        tell(Msg::Inspect(pid, tab, Some(Box::new(seen))));
    }

    /// A dialog to set one of the kernel's tunables.
    pub fn tune(&mut self, name: &str, cx: &mut Cx<Self>) {
        if let Some(open) = self.tune.take() {
            open.close(cx);
        }
        let value = self.system.tunables.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone()).unwrap_or_default();
        let (name, main) = (name.to_owned(), cx.messenger());
        let options = WindowOptions { fixed: true, ..WindowOptions::new(format!("Set {name}"), 420.0, 150.0).dialog() };
        self.tune = olo_kit::open_window_here(cx, options, move |cx| Tune::new(name, value, main, cx));
    }

    /// A command by name: a menu's, a button's, a row's.
    pub fn run(&mut self, command: &str, cx: &mut Cx<Self>) {
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
            cx.invalidate();
            return self.store();
        }
        if let Some(v) = command.strip_prefix("remove-graph:").and_then(|v| v.parse::<usize>().ok()) {
            if self.settings.views.len() > 1 && v < self.settings.views.len() {
                self.settings.views.remove(v);
                self.store();
                cx.invalidate();
            }
            return;
        }
        if let Some(ms) = command.strip_prefix("interval:").and_then(|v| v.parse::<u64>().ok()) {
            self.settings.interval_ms = ms;
            self.wants_changed();
            cx.invalidate();
            return self.store();
        }
        if let Some(s) = command.strip_prefix("span:").and_then(|v| v.parse::<u32>().ok()) {
            self.settings.span = s;
            cx.invalidate();
            return self.store();
        }
        if let Some(name) = command.strip_prefix("section:")
            && let Ok(s) = serde_json::from_str::<Section>(&format!("\"{name}\""))
        {
            return self.set_section(s, cx);
        }
        // A process's commands, each for one or several: "signal:PIDS:N",
        // "nice:PIDS:N", PIDS being "12" or "12,40,41".
        if let Some(actions) = process_actions(command) {
            return self.act_all(actions, cx);
        }
        if let Some(pid) = command.strip_prefix("inspect:").and_then(pid_list).and_then(|p| p.first().copied()) {
            return self.inspect(pid, cx);
        }
        if command == "supervisor-preferences" {
            return sections::supervise(sections::Supervise::Preferences);
        }
        if let Some(name) = command.strip_prefix("supervisor-ignore:") {
            return sections::supervise(sections::Supervise::Ignore(name.to_owned()));
        }
        if let Some(pids) = command.strip_prefix("quit:").and_then(pid_list) {
            return self.quit_processes(pids, false, cx);
        }
        if let Some(pids) = command.strip_prefix("force-quit:").and_then(pid_list) {
            return self.quit_processes(pids, true, cx);
        }
        // Copied one a line.
        if let Some(pids) = command.strip_prefix("copy-pid:").and_then(pid_list) {
            let text = pids.iter().map(|p| p.to_string()).collect::<Vec<_>>().join("\n");
            return clipboard::write(ClipboardItem::new_string(text));
        }
        if let Some(pids) = command.strip_prefix("copy-command:").and_then(pid_list) {
            let text = pids.iter().filter_map(|&pid| self.proc(pid).map(|p| p.command.clone())).collect::<Vec<_>>().join("\n");
            return clipboard::write(ClipboardItem::new_string(text));
        }
        // Each program once, however many of its processes.
        if let Some(pids) = command.strip_prefix("show-program:").and_then(pid_list) {
            let mut shown = Vec::new();
            for pid in pids {
                if let Ok(exe) = std::fs::read_link(format!("/proc/{pid}/exe"))
                    && !shown.contains(&exe)
                {
                    let _ = olo_kit::tracker::open(&exe);
                    shown.push(exe);
                }
            }
            return;
        }
        if let Some(path) = command.strip_prefix("open-path:") {
            let _ = olo_kit::tracker::open(std::path::Path::new(path));
            return;
        }
        if let Some(text) = command.strip_prefix("copy-text:") {
            return clipboard::write(ClipboardItem::new_string(text.to_owned()));
        }
        if let Some(name) = command.strip_prefix("tune:") {
            return self.tune(name, cx);
        }
        if let Some(pid) = command.strip_prefix("select:").and_then(|p| p.parse::<i32>().ok()) {
            self.selected = vec![pid];
            self.reveal_selected = true;
            return self.set_section(Section::Processes, cx);
        }
        let in_processes = self.settings.section == Section::Processes;
        let pids = if in_processes { self.chosen_pids() } else { Vec::new() };
        let pid = pids.first().copied();
        match command {
            "add-graph" => {
                self.settings.views.push(vec!["cpu".into()]);
                self.set_section(Section::Graphs, cx);
            }
            "quit-process" => self.quit_processes(pids, false, cx),
            "force-quit-process" => self.quit_processes(pids, true, cx),
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
                // (Once it's shown: its field is in the window then.)
                let me = cx.view_ref();
                cx.on_next_frame(move |cx| me.update(cx, |a, cx| a.search.update(cx, |s, cx| s.focus(cx))));
            }
            "close" => cx.close_window(),
            "quit" => {
                self.store_now();
                olo_kit::quit();
            }
            _ => {}
        }
        cx.invalidate();
    }

    fn sections(&self) -> Vec<Section> {
        let (gpu, sensors) = has_gpu_and_sensors(&self.sources);
        sections_for(gpu, sensors)
    }

    /// The main window's menu bar.
    pub fn menus(&self) -> Vec<Menu> {
        let chosen = if self.settings.section == Section::Processes { self.chosen_pids() } else { Vec::new() };
        menus(&self.settings, &self.sections(), self.access, &chosen, &self.nices(&chosen), false)
    }

    /// The sidebar, each row with its figure now.
    fn sidebar(&mut self, cx: &mut Cx<Self>) -> Element {
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
        Sidebar::new("activity-sections", rows, chosen)
            .on_choose(cx.listener(move |a: &mut Activity, i: &usize, cx| {
                if let Some(Some(s)) = index_of.get(*i) {
                    a.set_section(*s, cx);
                }
            }))
            .build(cx)
    }
}

/// A handler that has the main window's view run the command it's
/// handed (a menu's choice).
pub fn runs(me: olo_kit::ViewRef<Activity>) -> impl Fn(&str, &mut WindowCx) + 'static {
    move |command, cx| {
        let command = command.to_owned();
        me.update(cx, move |a, cx| a.run(&command, cx));
    }
}

/// "signal:PIDS:N" and "nice:PIDS:N" as what they do.
pub fn process_actions(command: &str) -> Option<Vec<Action>> {
    if let Some(rest) = command.strip_prefix("signal:")
        && let Some((pids, sig)) = rest.split_once(':')
        && let (Some(pids), Ok(signal)) = (pid_list(pids), sig.parse())
    {
        return Some(pids.into_iter().map(|pid| Action::Signal { pid, signal }).collect());
    }
    if let Some(rest) = command.strip_prefix("nice:")
        && let Some((pids, n)) = rest.split_once(':')
        && let (Some(pids), Ok(nice)) = (pid_list(pids), n.parse())
    {
        return Some(pids.into_iter().map(|pid| Action::Nice { pid, nice }).collect());
    }
    None
}

impl View for Activity {
    type Message = Msg;

    fn message(&mut self, msg: Msg, cx: &mut Cx<Self>) {
        match msg {
            Msg::Sources(s, from_helper) => self.take_sources(s, from_helper),
            Msg::Sample(s, from_helper) => return self.take_sample(*s, from_helper, cx),
            Msg::Supervisor(s) => self.supervisor = *s,
            Msg::Palms(stats) => self.palm_stats = Some(stats),
            Msg::Frames(displays, capture, pointer, touch, inputs) => {
                // Each figure kept over time, for the sections' graphs.
                let t = self.t();
                let mut keep = |key: String, v: f64| {
                    let q = Rc::make_mut(self.frame_history.entry(key).or_default());
                    q.push_back((t, v));
                    while q.front().is_some_and(|(at, _)| t - at > FRAME_HISTORY_SECONDS) {
                        q.pop_front();
                    }
                };
                keep("pointer.hz".into(), pointer.reports_second as f64);
                keep("touch.reports".into(), touch.reports_second as f64);
                keep("touch.p50".into(), touch.latency_p50_us as f64 / 1000.0);
                keep("touch.p99".into(), touch.latency_p99_us as f64 / 1000.0);
                keep("inputs.clicks".into(), inputs.clicks_minute as f64);
                keep("inputs.scrolls".into(), inputs.scrolls_minute as f64);
                keep("inputs.gestures".into(), inputs.gestures_minute as f64);
                keep("inputs.keys".into(), inputs.keys_minute as f64);
                for d in &displays {
                    keep(format!("display.{}.fps", d.output), d.fps);
                    keep(format!("display.{}.late", d.output), d.late_minute as f64);
                    keep(format!("display.{}.p95", d.output), d.render_p95_us as f64 / 1000.0);
                }
                self.displays = displays;
                self.frames_capture = capture;
                self.pointer_health = pointer;
                self.touch_health = touch;
                self.inputs = inputs;
            }
            Msg::Model(on, stats) => {
                if let Some(s) = stats.as_ref().filter(|s| s.latency_count > 0) {
                    let t = self.t();
                    for (key, v) in [("ai.p50", s.latency_p50_us), ("ai.p95", s.latency_p95_us), ("ai.worst", s.latency_worst_us)] {
                        let q = Rc::make_mut(self.frame_history.entry(key.into()).or_default());
                        q.push_back((t, v as f64 / 1000.0));
                        while q.front().is_some_and(|(at, _)| t - at > FRAME_HISTORY_SECONDS) {
                            q.pop_front();
                        }
                    }
                }
                // (Nothing new: nothing to draw.)
                if self.model_on == on && self.model == stats {
                    return;
                }
                self.model_on = on;
                self.model = stats;
            }
            Msg::Access(access) => self.access = access,
            Msg::Alert(message) => alert(&message, cx),
            Msg::Command(command) => return self.run(&command, cx),
            Msg::Do(actions) => self.act_all(actions, cx),
            _ => return,
        }
        cx.invalidate();
    }

    fn command(&mut self, command: &str, cx: &mut Cx<Self>) -> bool {
        self.run(command, cx);
        true
    }

    /// The main window closed: the app ends, its inspectors with it,
    /// its settings kept first.
    fn window_closing(&mut self, _: &mut Cx<Self>) {
        self.store_now();
        olo_kit::quit();
    }

    fn save_state(&mut self, _: &mut Cx<Self>) -> Option<String> {
        self.store_now();
        Some(String::new())
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        if !self.sections().contains(&self.settings.section) {
            self.settings.section = Section::Overview;
        }
        let bar = MenuBar::new(cx, self.menus());
        let width = f32::from(cx.viewport_size().width) - SIDEBAR_WIDTH;
        let content = if self.sources.is_empty() {
            div().into_any_element()
        } else if self.settings.section == Section::Processes {
            processes::view(self, width, cx)
        } else {
            sections::view(self, self.settings.section, width, cx)
        };
        let sidebar = self.sidebar(cx);
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(bar)
            .child(div().flex_1().min_h_0().flex().flex_row().child(div().w(px(SIDEBAR_WIDTH)).h_full().flex_shrink_0().child(sidebar)).child(div().flex_1().min_w_0().h_full().child(content)))
            .into_any_element()
    }
}

/// The dialog setting one of the kernel's tunables.
struct Tune {
    name: String,
    field: Child<TextField>,
    /// The main window, which does it.
    main: olo_kit::Messenger<Msg>,
    focus: FocusHandle,
}

enum TuneMsg {
    Cancel,
    Set,
}

impl Tune {
    fn new(name: String, value: String, main: olo_kit::Messenger<Msg>, cx: &mut Cx<Self>) -> Tune {
        let field = text_field(cx, |f| f.name("Value").hint("A value").with_text(value));
        field.update(cx, |f, cx| f.select_all_text(cx));
        // (Set only once there's a value.)
        cx.subscribe(&field, |_: &mut Tune, _: &TextFieldEvent, cx| cx.invalidate());
        field.focus(cx);
        Tune { name, field, main, focus: FocusHandle::new() }
    }

    fn done(&mut self, set: bool, cx: &mut Cx<Self>) {
        let value = self.field.read(cx).text().trim().to_owned();
        if set && value.is_empty() {
            return;
        }
        if set {
            self.main.send(Msg::Do(vec![Action::Sysctl { name: self.name.clone(), value }]));
        }
        cx.close_window();
    }
}

impl View for Tune {
    type Message = TuneMsg;

    fn message(&mut self, msg: TuneMsg, cx: &mut Cx<Self>) {
        self.done(matches!(msg, TuneMsg::Set), cx);
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let about = sample::TUNABLES.iter().find(|(n, _)| *n == self.name).map_or("", |(_, d)| *d);
        let ready = !self.field.read(cx).text().trim().is_empty();
        self.focus.focus_if_none(cx);
        div()
            .size_full()
            .track_focus(&self.focus)
            // Return and Escape, whatever has the keyboard.
            .on_key_down(cx.listener(|t: &mut Tune, e: &KeyDownEvent, cx| {
                if e.keystroke.modifiers.modified() {
                    return;
                }
                match e.keystroke.key.as_str() {
                    "escape" => t.done(false, cx),
                    "enter" => t.done(true, cx),
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .child(
                AlertFrame::new("system")
                    .child(paragraph(format!("Set {} ({}).", self.name, about.to_lowercase())))
                    .child(div().mt(px(m::SPACING)).child(TextControl::new(cx, &self.field).label("Value:")))
                    .child(div().flex_1())
                    .child(
                        button_row()
                            .child(Button::new("Cancel").on_click(cx.msg_with(|_: &ClickEvent| TuneMsg::Cancel)))
                            .child(Button::new("Set").default(true).enabled(ready).on_click(cx.msg_with(|_: &ClickEvent| TuneMsg::Set))),
                    ),
            )
            .into_any_element()
    }
}

/// The app's looper: who measures, what this app may do, and its
/// windows, each told what it shows.
struct Hub {
    main: Option<WindowHandle<Activity>>,
    /// What the main window is made with, until it is (it's made once:
    /// the app ends when it closes).
    pollers: Option<Pollers>,
    /// Each inspector open: its process and window.
    inspectors: Vec<(i32, WindowHandle<inspector::Inspector>)>,
    shared: Shared,
    control: sample::Control,
    /// What each sampler measures (here, the helper), as it said.
    sources: [Vec<Source>; 2],
    helper: Option<Helper>,
    /// Actions to do once full access comes.
    after_access: Vec<Action>,
    next_action: u64,
    inspect_tx: mpsc::Sender<i32>,
}

impl Hub {
    /// The main window: opened, or brought forward.
    fn open_main(&mut self) {
        if olo_kit::raise(self.main.as_ref()) {
            return;
        }
        let Some(pollers) = self.pollers.take() else { return };
        let (shared, control, sources) = (self.shared.clone(), self.control.clone(), self.sources.clone());
        let options = WindowOptions::new("Activity Monitor", 900.0, 620.0).min_size(560.0, 400.0).keep_size("main");
        self.main = Some(olo_kit::open_window(options, move |cx| Activity::new(shared, control, sources, pollers, cx)));
    }

    fn to_main(&self, msg: Msg) {
        if let Some(main) = &self.main {
            main.messenger().send(msg);
        }
    }

    /// The inspectors told what every window knows, again.
    fn tell_inspectors(&mut self) {
        self.inspectors.retain(|(_, w)| w.is_open());
        for (_, w) in &self.inspectors {
            w.messenger().send(inspector::Msg::Shared(Box::new(self.shared.clone())));
        }
    }

    fn inspected(&self) -> HashSet<i32> {
        self.inspectors.iter().map(|(pid, _)| *pid).collect()
    }

    fn full(&self) -> bool {
        self.shared.access == Access::Full
    }

    fn request_details(&self, pid: i32) {
        match &self.helper {
            Some(h) if self.full() => h.send(&ToHelper::Inspect { pid }),
            _ => {
                let _ = self.inspect_tx.send(pid);
            }
        }
    }

    fn refresh_soon(&self) {
        for pid in self.inspected() {
            self.request_details(pid);
        }
    }

    /// Something that couldn't be done, said by whichever window is open.
    fn alert(&self, message: String) {
        match (&self.main, self.inspectors.first()) {
            (Some(main), _) => {
                main.messenger().send(Msg::Alert(message));
            }
            (None, Some((_, w))) => {
                w.messenger().send(inspector::Msg::Alert(message));
            }
            _ => {}
        }
    }

    fn set_access(&mut self, access: Access) {
        self.shared.access = access;
        self.to_main(Msg::Access(access));
        self.tell_inspectors();
    }

    /// Ask for full access (polkit asks for the password).
    fn get_full_access(&mut self) {
        if self.helper.is_some() {
            return;
        }
        match Helper::start(helper::way(), move |m| tell(Msg::Helper(m))) {
            Ok(h) => {
                self.helper = Some(h);
                self.set_access(Access::Asking);
            }
            Err(e) => self.alert(format!("Full access couldn't be asked for. {e}")),
        }
    }

    fn act(&mut self, actions: Vec<Action>) {
        let Some(h) = self.helper.as_ref().filter(|_| self.shared.access == Access::Full) else { return };
        for action in actions {
            self.next_action += 1;
            h.send(&ToHelper::Act { id: self.next_action, action });
        }
    }

    /// Open (or bring forward) the inspector of `pid` at `tab`.
    fn inspect(&mut self, pid: i32, tab: usize, seen: Option<Box<inspector::Seen>>) {
        self.inspectors.retain(|(_, w)| w.is_open());
        if let Some((_, w)) = self.inspectors.iter().find(|(p, _)| *p == pid) {
            if tab > 0 {
                w.messenger().send(inspector::Msg::Tab(tab));
            }
            olo_kit::raise(Some(w));
            return;
        }
        // (Before the first sample, its name as the kernel has it.)
        let name = seen
            .as_ref()
            .and_then(|s| s.proc.as_ref().map(|p| p.name.clone()))
            .or_else(|| Some(std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?.trim().to_owned()))
            .unwrap_or_else(|| pid.to_string());
        let shared = self.shared.clone();
        let options = WindowOptions::new(format!("{name} ({pid})"), 620.0, 520.0).min_size(460.0, 380.0).keep_size("inspector");
        let window = olo_kit::open_window(options, move |cx| inspector::Inspector::new(pid, name, tab, shared, seen.map(|s| *s), cx));
        self.inspectors.push((pid, window));
        self.request_details(pid);
    }

    fn take_sources(&mut self, sources: Vec<Source>, from_helper: bool) {
        self.sources[from_helper as usize] = sources.clone();
        let (gpu, sensors) = has_gpu_and_sensors(self.sources.iter().flatten());
        let sections = sections_for(gpu, sensors);
        if sections != self.shared.sections {
            self.shared.sections = sections;
            self.tell_inspectors();
        }
        self.to_main(Msg::Sources(sources, from_helper));
    }

    fn take_sample(&mut self, s: Box<Sample>, from_helper: bool) {
        // A sample from whoever isn't measuring now (one in flight as the
        // helper came or went) is dropped.
        if from_helper != self.full() {
            return;
        }
        self.inspectors.retain(|(_, w)| w.is_open());
        if let Some(list) = &s.processes {
            for (pid, w) in &self.inspectors {
                w.messenger().send(inspector::Msg::Seen(Box::new(inspector::Seen::of(list, &s.system, *pid))));
            }
        }
        self.refresh_soon();
        self.to_main(Msg::Sample(s, from_helper));
    }

    fn take_details(&self, pid: i32, details: Result<Box<Details>, String>) {
        if let Some((_, w)) = self.inspectors.iter().find(|(p, _)| *p == pid) {
            w.messenger().send(inspector::Msg::Details(details));
        }
    }

    fn heard(&mut self, m: Option<FromHelper>) {
        match m {
            Some(FromHelper::Ready) => {
                self.control.paused.store(true, Ordering::Relaxed);
                self.set_access(Access::Full);
                if let Some(h) = &self.helper {
                    h.send(&ToHelper::Wants(self.control.wants()));
                }
                let after = std::mem::take(&mut self.after_access);
                self.act(after);
                self.refresh_soon();
            }
            Some(FromHelper::Sources(s)) => self.take_sources(s, true),
            Some(FromHelper::Sample(s)) => self.take_sample(s, true),
            Some(FromHelper::Details { pid, details }) => self.take_details(pid, details),
            Some(FromHelper::Done { result, .. }) => {
                if let Err(r) = result {
                    self.alert(r.message);
                }
                self.refresh_soon();
            }
            // Refused (the password dialog was cancelled) or gone.
            None => {
                self.after_access.clear();
                self.helper = None;
                self.control.paused.store(false, Ordering::Relaxed);
                self.set_access(Access::Mine);
            }
        }
    }

    /// The settings changed: kept (on a worker), and the inspectors told
    /// if it's something of theirs.
    fn settings_changed(&mut self, settings: Settings) {
        let theirs = |s: &Settings| (s.interval_ms, s.span, s.smooth, s.section, s.tree, s.ask_full_access);
        let tell = theirs(&settings) != theirs(&self.shared.settings);
        self.shared.settings = settings;
        olo_kit::store::save(settings_path(), &self.shared.settings);
        if tell {
            self.tell_inspectors();
        }
    }

    /// A command from an inspector's menus that isn't about its process:
    /// the main window's, opened for it unless it only sets something.
    fn command(&mut self, command: String) {
        if self.main.as_ref().is_some_and(|m| m.is_open()) {
            return self.to_main(Msg::Command(command));
        }
        let mut settings = self.shared.settings.clone();
        if let Some(ms) = command.strip_prefix("interval:").and_then(|v| v.parse::<u64>().ok()) {
            settings.interval_ms = ms;
            self.control.interval_ms.store(ms, Ordering::Relaxed);
            self.wants_changed();
        } else if let Some(s) = command.strip_prefix("span:").and_then(|v| v.parse::<u32>().ok()) {
            settings.span = s;
        } else {
            match command.as_str() {
                "tree" => settings.tree = !settings.tree,
                "smooth" => settings.smooth = !settings.smooth,
                "ask-full-access" => settings.ask_full_access = !settings.ask_full_access,
                "quit" => return olo_kit::quit(),
                _ => {
                    self.open_main();
                    return self.to_main(Msg::Command(command));
                }
            }
        }
        self.settings_changed(settings);
    }

    /// Whoever measures hears what to measure (the sampler here reads
    /// `control` itself).
    fn wants_changed(&self) {
        if let Some(h) = self.helper.as_ref().filter(|_| self.full()) {
            h.send(&ToHelper::Wants(self.control.wants()));
        }
    }

    /// `--section NAME`: that section; `--inspect PID`: that process's
    /// inspector, at what the supervisor found.
    fn follow_args(&mut self, args: &[String]) {
        let after = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1));
        // (Asked for a process: its inspector alone, in front.)
        if !inspecting(args) {
            self.open_main();
        }
        if let Some(name) = after("--section")
            && let Some(s) = Section::ALL.into_iter().find(|s| s.name().eq_ignore_ascii_case(name))
        {
            self.command(s.command());
        }
        // (From the supervisor: at what it found.)
        if let Some(pid) = after("--inspect").and_then(|p| p.parse().ok()) {
            self.inspect(pid, inspector::SUPERVISOR_TAB, None);
        }
    }

    fn message(&mut self, message: Box<dyn std::any::Any + Send>) {
        let message = match message.downcast::<Launch>() {
            Ok(launch) => return self.follow_args(&launch.args),
            Err(other) => other,
        };
        let Ok(msg) = message.downcast::<Msg>() else { return };
        match *msg {
            Msg::Sources(s, from_helper) => self.take_sources(s, from_helper),
            Msg::Sample(s, from_helper) => self.take_sample(s, from_helper),
            Msg::Details(pid, d) => {
                // Full access answers for inspectors now.
                if !self.full() {
                    self.take_details(pid, d);
                }
            }
            Msg::Helper(m) => self.heard(m),
            Msg::Supervisor(s) => {
                self.shared.supervisor = Arc::new((*s).clone());
                self.tell_inspectors();
                self.to_main(Msg::Supervisor(s));
            }
            Msg::Settings(s) => self.settings_changed(*s),
            Msg::Wants => self.wants_changed(),
            Msg::Act(actions) => self.act(actions),
            Msg::Acted => self.refresh_soon(),
            Msg::GetAccess(actions) => {
                self.after_access.extend(actions);
                self.get_full_access();
            }
            Msg::Inspect(pid, tab, seen) => self.inspect(pid, tab, seen),
            Msg::InspectorClosed(pid) => self.inspectors.retain(|(p, w)| *p != pid && w.is_open()),
            Msg::Command(command) => self.command(command),
            // (The pollers', which only the main window shows.)
            msg @ (Msg::Frames(..) | Msg::Model(..) | Msg::Palms(..)) => self.to_main(msg),
            Msg::Access(_) | Msg::Alert(_) | Msg::Do(_) => {}
        }
    }
}

fn main() {
    olo_crash::install();
    // A session or install from before the rename (Tabula) carries on.
    olo_xdg::legacy::adopt();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--helper") {
        helper::serve();
    }
    if instance::forward(APP_ID, &args) {
        return;
    }
    olo_kit::run(APP_ID, move |app| {
        instance::listen(APP_ID, olo_kit::proxy());
        let settings: Settings = olo_kit::store::load::<Settings>(&settings_path()).map(Settings::migrate).unwrap_or_default();
        let control = sample::Control::new(settings.interval_ms);
        // (Processes always: the overview's busiest, and inspectors'.)
        control.want_processes.store(true, Ordering::Relaxed);
        sample::start(control.clone(), move |s| tell(Msg::Sources(s, false)), move |s| tell(Msg::Sample(Box::new(s), false)));
        // How the displays keep up, asked while the Displays section shows.
        let (frames, frames_rx) = mpsc::channel::<FramesAsk>();
        std::thread::Builder::new().name("activity-frames".into()).spawn(move || frames_poller(tell, frames_rx)).expect("spawn");
        // How the command model does, asked while the AI section shows.
        let (model_ask, model_rx) = mpsc::channel::<bool>();
        std::thread::Builder::new().name("activity-model".into()).spawn(move || model_poller(tell, model_rx)).expect("spawn");
        // Inspectors' details, read off the UI thread.
        let (inspect_tx, inspect_rx) = mpsc::channel::<i32>();
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
                        tell(Msg::Details(pid, details::read_details(pid).map(Box::new)));
                    }
                }
            })
            .expect("the inspect thread");
        // The supervisor's work, followed as it publishes it.
        {
            let read = || tell(Msg::Supervisor(Box::new(olo_protocol::performance::SupervisorState::load())));
            std::thread::spawn(read);
            if let Some(w) = olo_kit::watch::watch([olo_protocol::performance::SupervisorState::path()], move |_| read()) {
                // For as long as the app runs.
                std::mem::forget(w);
            }
        }
        let ask = settings.ask_full_access;
        let mut hub = Hub {
            main: None,
            pollers: Some(Pollers { frames, model_ask }),
            inspectors: Vec::new(),
            shared: Shared { settings, access: Access::Mine, sections: sections_for(false, false), supervisor: Default::default(), clock: Instant::now() },
            control,
            sources: Default::default(),
            helper: None,
            after_access: Vec::new(),
            next_action: 0,
            inspect_tx,
        };
        // Started to show a process (the supervisor's alert), or a section.
        hub.follow_args(&args);
        // SAFETY: geteuid has no preconditions.
        let root = unsafe { libc::geteuid() } == 0;
        if ask && !root && helper::way() != Way::Off {
            hub.get_full_access();
        }
        olo_kit::message_handler(app, move |message, _| hub.message(message));
    });
}

/// Whether `args` ask for a process's inspector (`--inspect PID`).
fn inspecting(args: &[String]) -> bool {
    args.iter().any(|a| a == "--inspect")
}

/// "12" or "12,40,41": the pids a command is for (none: not one).
pub fn pid_list(text: &str) -> Option<Vec<i32>> {
    let pids: Vec<i32> = text.split(',').map(|p| p.trim().parse().ok()).collect::<Option<_>>()?;
    (!pids.is_empty()).then_some(pids)
}

#[cfg(test)]
mod pid_tests {
    #[test]
    fn commands_for_several() {
        assert_eq!(super::pid_list("12"), Some(vec![12]));
        assert_eq!(super::pid_list("12,40,41"), Some(vec![12, 40, 41]));
        assert_eq!(super::pid_list("12,x"), None);
        assert_eq!(super::pid_list(""), None);
        assert_eq!(crate::processes::pids_text(&[12, 40]), "12,40");
    }
}
