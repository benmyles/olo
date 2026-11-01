//! Agent Screens, the window: the agents' screens in a sidebar (running,
//! then stopped ones with a history kept), and the one chosen under a
//! header saying what it is and how it's doing, in three tabs: Live (the
//! screen as it is, with the agent's pointer, and Take Control to step
//! in: the person's pointer and keys over the picture then go to that
//! screen, never to their own, until they choose Stop Controlling),
//! History (`replay`: its actions as they come, to search, filter and
//! play back) and Stats (`stats`).
//!
//! Live takes a picture only when the screen has drawn something new
//! (its app_server says so, `EventKind::Painted`), at most eight a second:
//! a screen at rest costs nothing to watch. The pictures are taken by a
//! worker, input goes out through another (moves run together, over one
//! connection) and the history is followed by a third, so the window
//! never waits on a screen or a disk.

use std::{
    collections::HashSet,
    os::unix::net::UnixStream,
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, Sender, channel},
    },
    time::{Duration, Instant},
};

use olo_protocol::{Client, Event, EventKind, ImageFormat, Reply, Request, SyntheticEvent};
use olo_ui::{
    Frame, Menu, MenuBar, Proxy, agent, file_panel,
    gpui::{
        App, AppContext as _, Context, Entity, FocusHandle, FontWeight, HitboxBehavior, InteractiveElement as _, IntoElement, KeyDownEvent, KeyUpEvent, Modifiers, ModifiersChangedEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _, Render, ScrollDelta,
        ScrollHandle, ScrollWheelEvent, SharedString, Styled as _, Subscription, Task, Window, WindowHandle, canvas, div, point, px, size,
    },
    icons, keys, metrics as m,
    style::{self, c},
    theme,
    widgets::{Button, EmptyState, Sidebar, TabView, open_alert, sidebar},
};
use serde::{Deserialize, Serialize};

use crate::{
    picture::{PictureWell, Shot},
    replay::{self, Exported, Replay, ReplayMsg},
    screens::{self, Screen},
    stats,
};

/// Frames a second, at most.
const FPS: f64 = 8.0;
/// The sidebar's width, less in a small window.
const SIDEBAR_W: f32 = 196.0;
const SIDEBAR_MIN_W: f32 = 160.0;
const HEADER_H: f32 = 40.0;
/// New Screen's place under the sidebar.
const FOOTER_H: f32 = m::CONTROL_HEIGHT + 2.0 * m::SPACING;
/// The key context of the picture being controlled: Tab and ⇧Tab go to
/// the screen there, not round the window's controls.
pub const CONTROLLED: &str = "AgentScreen";

pub enum Msg {
    Screens(Vec<Screen>),
    /// Every screen to list: running, or stopped with a history kept;
    /// each with when it was last used, most recent first.
    Rows(Vec<Row>),
    Frame { name: String, shot: Arc<Shot> },
    Failed { name: String, message: String },
    Started(Result<Screen, String>),
    Stopped(Result<(), String>),
    Cleared(Result<(), String>),
}

/// The chosen screen's tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
enum Tab {
    #[default]
    Live,
    History,
    Stats,
}

impl Tab {
    fn label(self) -> &'static str {
        match self {
            Tab::Live => "Live",
            Tab::History => "History",
            Tab::Stats => "Stats",
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
struct Saved {
    selected: Option<String>,
    #[serde(default)]
    tab: Tab,
}

/// A screen in the list.
#[derive(Debug, Clone)]
pub struct Row {
    name: String,
    last_used: Option<u64>,
}

/// The screens running (`running`) and those stopped with a history, by
/// last use, most recent first. Reads the histories: off the UI thread.
fn rows(running: &[Screen]) -> Vec<Row> {
    let own = std::env::var("OLO_SCREEN").ok();
    let mut names: Vec<String> = running.iter().map(|s| s.name.clone()).collect();
    let kept: Vec<String> = crate::history::names().into_iter().filter(|n| !names.contains(n) && Some(n) != own.as_ref()).collect();
    names.extend(kept);
    let mut rows: Vec<Row> = names
        .into_iter()
        .map(|name| {
            let last_used = crate::history::read(&name).iter().map(|a| a.at + a.ms).max();
            Row { name, last_used }
        })
        .collect();
    rows.sort_by_key(|r| (std::cmp::Reverse(r.last_used), r.name.clone()));
    rows
}

/// How long ago, as a sidebar says it: "now", "5 min", "3 h", "2 d".
fn ago_short(ms: u64, now: u64) -> String {
    let s = now.saturating_sub(ms) / 1000;
    match s {
        0..60 => "now".into(),
        60..3600 => format!("{} min", s / 60),
        3600..86_400 => format!("{} h", s / 3600),
        _ => format!("{} d", s / 86_400),
    }
}

/// When what's said of a time `age` ms ago next changes ("12 s ago",
/// "3 min ago"): a second on while under a minute, else the next minute.
fn next_change(age: u64) -> Duration {
    if age < 60_000 { Duration::from_millis(1000 - age % 1000) } else { Duration::from_millis(60_000 - age % 60_000) }
}

/// The screens to show: all but the one this viewer itself is on, if it
/// runs on one (it would show itself, endlessly).
fn listed() -> Vec<Screen> {
    let own = std::env::var("OLO_SCREEN").ok();
    screens::list().into_iter().filter(|s| Some(&s.name) != own.as_ref()).collect()
}

/// Follow the list of screens as their files come and go (`watch`), and
/// now and then for one whose session was killed without a word (its
/// file left behind).
fn watch_list(proxy: Proxy) {
    let last = Arc::new(Mutex::new(None::<Vec<Screen>>));
    let look = move || {
        let now = listed();
        let mut last = last.lock().unwrap_or_else(|e| e.into_inner());
        if last.as_ref() != Some(&now) {
            proxy.send(Msg::Screens(now.clone()));
            *last = Some(now);
        }
    };
    look();
    let look = Arc::new(look);
    let on_change = look.clone();
    let dir = screens::dir();
    let followed = dir.clone();
    if let Some(w) = olo_ui::watch::watch([dir], move |changed| {
        // A screen's own file, not a viewer's frame passing through.
        let items: Vec<&std::path::PathBuf> = changed.iter().filter(|p| p.parent() == Some(followed.as_path())).collect();
        if items.is_empty() || items.iter().any(|p| p.extension().is_some_and(|e| e == "json")) {
            on_change();
        }
    }) {
        // For as long as the app runs.
        std::mem::forget(w);
    }
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_secs(30));
            look();
        }
    });
}

/// A screen being watched for the Live tab: a picture is taken each time
/// it draws something new, at most [`FPS`] a second, until it's dropped.
struct Watching {
    name: String,
    stop: Arc<AtomicBool>,
    /// The connection the screen's events come on (shut to stop).
    events: Arc<Mutex<Option<UnixStream>>>,
    /// Something new to take a picture of.
    dirty: Arc<(Mutex<bool>, Condvar)>,
}

impl Drop for Watching {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(s) = self.events.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }
        let (_, woken) = &*self.dirty;
        woken.notify_all();
    }
}

fn mark(dirty: &(Mutex<bool>, Condvar)) {
    *dirty.0.lock().unwrap_or_else(|e| e.into_inner()) = true;
    dirty.1.notify_all();
}

/// Watch screen `name`: one worker hears when it draws something new
/// (`Painted`; from an app_server before those, every frame's time), the
/// other takes the picture then.
fn watch_screen(name: &str, proxy: Proxy) -> Watching {
    let w = Watching { name: name.to_owned(), stop: Arc::default(), events: Arc::default(), dirty: Arc::new((Mutex::new(true), Condvar::new())) };
    let period = Duration::from_secs_f64(1.0 / FPS);
    {
        let (name, proxy, stop, dirty) = (name.to_owned(), proxy.clone(), w.stop.clone(), w.dirty.clone());
        let _ = std::thread::Builder::new().name("screen-picture".into()).spawn(move || {
            // In a folder of their own: the screens' folder is followed
            // for screens coming and going, not for frames.
            let views = screens::dir().join("views");
            let path = views.join(format!("view-{}.rgba", std::process::id()));
            let _ = std::fs::create_dir_all(&views);
            loop {
                {
                    let mut d = dirty.0.lock().unwrap_or_else(|e| e.into_inner());
                    while !*d && !stop.load(Ordering::Relaxed) {
                        d = dirty.1.wait(d).unwrap_or_else(|e| e.into_inner());
                    }
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    *d = false;
                }
                let began = Instant::now();
                let shot = screens::get(&name).and_then(|s| {
                    s.picture(&path, None, true, ImageFormat::Rgba)?;
                    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
                    decode(bytes).ok_or_else(|| "the picture was cut short".to_owned())
                });
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                match shot {
                    Ok(shot) => proxy.send(Msg::Frame { name: name.clone(), shot: Arc::new(shot) }),
                    Err(message) => proxy.send(Msg::Failed { name: name.clone(), message }),
                }
                std::thread::sleep(period.saturating_sub(began.elapsed()));
            }
            let _ = std::fs::remove_file(&path);
        });
    }
    {
        let (name, stop, events, dirty) = (name.to_owned(), w.stop.clone(), w.events.clone(), w.dirty.clone());
        let _ = std::thread::Builder::new().name("screen-events".into()).spawn(move || {
            let client = screens::get(&name).and_then(|s| Client::connect_to(&s.olo_socket).map_err(|e| e.to_string()));
            let Ok(mut client) = client else { return };
            *events.lock().unwrap_or_else(|e| e.into_inner()) = client.closer().ok();
            if stop.load(Ordering::Relaxed) {
                return;
            }
            match client.request(&Request::SubscribeTo { events: vec![EventKind::Painted] }) {
                Ok(Reply::Ok) => {
                    while let Ok(reply) = client.read() {
                        if stop.load(Ordering::Relaxed) {
                            return;
                        }
                        if let Reply::Event { event: Event::Painted { .. } } = reply {
                            mark(&dirty);
                        }
                    }
                }
                // An app_server from before it said when it drew: a
                // picture each frame's time.
                _ => {
                    while !stop.load(Ordering::Relaxed) {
                        mark(&dirty);
                        std::thread::sleep(period);
                    }
                }
            }
        });
    }
    w
}

/// Width, height (little-endian u32), then straight RGBA rows (opaque).
fn decode(mut bytes: Vec<u8>) -> Option<Shot> {
    let w = u32::from_le_bytes(bytes.get(0..4)?.try_into().ok()?) as usize;
    let h = u32::from_le_bytes(bytes.get(4..8)?.try_into().ok()?) as usize;
    if bytes.len() < 8 + w * h * 4 {
        return None;
    }
    bytes.drain(..8);
    bytes.truncate(w * h * 4);
    Some(Shot { size: [w, h], rgba: bytes })
}

/// Send the person's input on, in order, off the UI thread: what has
/// come by the time the last was sent goes in one request (moves run
/// together to the last), over a connection kept to the screen.
fn send_input() -> Sender<(Screen, Vec<SyntheticEvent>)> {
    let (tx, rx): (Sender<(Screen, Vec<SyntheticEvent>)>, Receiver<_>) = channel();
    std::thread::spawn(move || {
        let mut client: Option<(String, Client)> = None;
        while let Ok((mut screen, mut events)) = rx.recv() {
            while let Ok((next, more)) = rx.try_recv() {
                if next.name != screen.name {
                    send(&mut client, &screen, std::mem::take(&mut events));
                    screen = next;
                }
                events.extend(more);
            }
            send(&mut client, &screen, events);
        }
    });
    tx
}

/// Moves one after another: only the last counts.
fn run_together(events: &mut Vec<SyntheticEvent>) {
    events.dedup_by(|b, a| {
        let moves = matches!((&*a, &*b), (SyntheticEvent::Move { .. }, SyntheticEvent::Move { .. }));
        if moves {
            *a = b.clone();
        }
        moves
    });
}

fn send(client: &mut Option<(String, Client)>, screen: &Screen, mut events: Vec<SyntheticEvent>) {
    run_together(&mut events);
    if events.is_empty() {
        return;
    }
    for _ in 0..2 {
        if client.as_ref().is_none_or(|(n, _)| *n != screen.name) {
            *client = Client::connect_to(&screen.olo_socket).ok().map(|c| (screen.name.clone(), c));
        }
        let Some((_, c)) = client.as_mut() else { break };
        match c.request(&Request::Synthetic { events: events.clone() }) {
            Ok(_) => return,
            // (Closed: the screen restarted; once more on a new one.)
            Err(e) => {
                tracing::warn!("input to screen {}: {e}", screen.name);
                *client = None;
            }
        }
    }
}

/// The evdev code of the key GPUI names `key` ("a", "f3", "!"): its place
/// on a US keyboard (the agent's), Shift aside.
fn evdev(key: &str) -> Option<u32> {
    match key {
        "insert" => return Some(110),
        "space" => return Some(57),
        _ => {}
    }
    match keys::evdev(key) {
        0 => {
            let mut chars = key.chars();
            let (Some(ch), None) = (chars.next(), chars.next()) else { return None };
            screens::key_for(ch).map(|(code, _)| code)
        }
        code => Some(code),
    }
}

/// A wheel's notches one way or the other: 1, −1, or none (`signum`
/// makes 0 one).
fn notch(v: f32) -> i32 {
    if v > 0.0 {
        1
    } else if v < 0.0 {
        -1
    } else {
        0
    }
}

/// The modifiers the screen is kept in step with: Ctrl, Shift, Alt.
const MODIFIERS: [u32; 3] = [29, 42, 56];

pub struct Viewer {
    win: Option<WindowHandle<Frame>>,
    screens: Vec<Screen>,
    rows: Vec<Row>,
    selected: Option<String>,
    tab: Tab,
    /// The screen the Live tab shows, being watched.
    watching: Option<Watching>,
    live: Entity<PictureWell>,
    failure: Option<String>,
    controlling: bool,
    /// Modifiers held down on the screen while controlling, and keys.
    held: [bool; 3],
    down: Vec<(String, u32)>,
    /// A button held on the screen (a drag goes on outside the picture).
    pressed: HashSet<u32>,
    live_focus: FocusHandle,
    input: Sender<(Screen, Vec<SyntheticEvent>)>,
    busy: bool,
    /// The chosen screen's history, followed as it grows.
    replay: Option<(Entity<Replay>, Subscription)>,
    stats_scroll: ScrollHandle,
    /// Times said ("3 s ago") move on: when the next changes, and its
    /// timer.
    ticking: Option<(Instant, Task<()>)>,
    proxy: Proxy,
}

impl Viewer {
    pub fn new(cx: &mut Context<Viewer>) -> Viewer {
        let saved: Saved = olo_ui::restored_state().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        let proxy = olo_ui::proxy(cx);
        watch_list(proxy.clone());
        let screens = listed();
        let selected = saved.selected.filter(|s| screens.iter().any(|x| &x.name == s) || crate::history::dir(s).is_dir()).or_else(|| screens.first().map(|s| s.name.clone()));
        let mut v = Viewer {
            win: None,
            screens,
            rows: Vec::new(),
            selected,
            tab: saved.tab,
            watching: None,
            live: cx.new(|_| PictureWell::new("Screen")),
            failure: None,
            controlling: false,
            held: [false; 3],
            down: Vec::new(),
            pressed: HashSet::new(),
            live_focus: cx.focus_handle(),
            input: send_input(),
            busy: false,
            replay: None,
            stats_scroll: ScrollHandle::new(),
            ticking: None,
            proxy,
        };
        v.watch(cx);
        v
    }

    pub fn set_window(&mut self, win: Option<WindowHandle<Frame>>) {
        self.win = win;
    }

    /// Choose `name` (from the command line, or a forwarded start), and
    /// bring the window forward.
    pub fn show(&mut self, name: Option<String>, cx: &mut Context<Self>) {
        if let Some(name) = name {
            self.select(Some(name), cx);
            self.set_tab(Tab::Live, cx);
        }
        if let Some(w) = self.win {
            cx.defer(move |cx| {
                let _ = w.update(cx, |_, window, _| window.activate_window());
            });
        }
    }

    /// The window closed, or Quit: let go of the screen, and end.
    pub fn quit(&mut self, cx: &mut Context<Self>) {
        self.stop_controlling();
        self.watching = None;
        cx.quit();
    }

    /// What to keep over an upgrade.
    pub fn saved(&self) -> String {
        serde_json::to_string(&Saved { selected: self.selected.clone(), tab: self.tab }).unwrap_or_default()
    }

    fn screen(&self) -> Option<&Screen> {
        self.selected.as_ref().and_then(|n| self.screens.iter().find(|s| &s.name == n))
    }

    fn select(&mut self, name: Option<String>, cx: &mut Context<Self>) {
        if self.selected != name {
            self.stop_controlling();
            self.selected = name;
            self.failure = None;
            self.live.update(cx, |w, cx| w.clear(None, cx));
            self.watch(cx);
            cx.notify();
        }
    }

    /// Watch the chosen screen while its Live tab shows (nothing is
    /// taken otherwise).
    fn watch(&mut self, cx: &mut Context<Self>) {
        let live = (self.shown_tab() == Tab::Live).then(|| self.screen().map(|s| s.name.clone())).flatten();
        if self.watching.as_ref().map(|w| &w.name) != live.as_ref() {
            self.watching = live.map(|n| watch_screen(&n, self.proxy.clone()));
        }
        let shape = self.screen().map(|s| (s.width as f32, s.height as f32));
        self.live.update(cx, |w, _| w.set_shape(shape));
    }

    /// The tab showing: Live only for a running screen.
    fn shown_tab(&self) -> Tab {
        if self.tab == Tab::Live && self.screen().is_none() { Tab::History } else { self.tab }
    }

    fn set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        if tab != Tab::Live {
            self.stop_controlling();
        }
        self.tab = tab;
        self.watch(cx);
        cx.notify();
    }

    fn stop_controlling(&mut self) {
        if !self.controlling {
            return;
        }
        // Nothing left held down on the screen.
        let mut up: Vec<SyntheticEvent> = self.down.drain(..).map(|(_, key)| SyntheticEvent::Key { key, pressed: false }).collect();
        up.extend(MODIFIERS.iter().zip(self.held).filter(|(_, held)| *held).map(|(&key, _)| SyntheticEvent::Key { key, pressed: false }));
        up.extend(self.pressed.drain().map(|button| SyntheticEvent::Button { button, pressed: false }));
        if let Some(screen) = self.screen().cloned()
            && !up.is_empty()
        {
            let _ = self.input.send((screen, up));
        }
        self.held = [false; 3];
        self.controlling = false;
    }

    fn take_control(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.screen().is_none() {
            return;
        }
        self.set_tab(Tab::Live, cx);
        self.controlling = true;
        window.focus(&self.live_focus);
        cx.notify();
    }

    fn alert(&mut self, message: &str, cx: &mut Context<Self>) {
        let message = message.to_owned();
        cx.defer(move |cx| {
            open_alert(cx, "Agent Screens", "stop", &message, &["OK"], |_, _| {});
        });
    }

    fn new_screen(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.busy = true;
        let proxy = self.proxy.clone();
        std::thread::spawn(move || proxy.send(Msg::Started(screens::start(&screens::free_name(), &screens::Options::default()))));
        cx.notify();
    }

    fn ask_stop(&mut self, cx: &mut Context<Self>) {
        let Some(name) = self.screen().map(|s| s.name.clone()) else { return };
        let message = format!("Stop the screen “{name}”? The agent’s programs on it are closed, and anything it hasn’t saved is lost. Its history is kept.");
        let me = cx.entity();
        cx.defer(move |cx| {
            open_alert(cx, "Stop Screen", "warning", &message, &["Cancel", "Stop Screen"], move |choice, cx| {
                if choice != Some(1) {
                    return;
                }
                me.update(cx, |v, cx| {
                    if v.selected.as_ref() == Some(&name) {
                        v.stop_controlling();
                    }
                    let (proxy, name) = (v.proxy.clone(), name.clone());
                    std::thread::spawn(move || proxy.send(Msg::Stopped(screens::stop(&name))));
                    cx.notify();
                });
            });
        });
    }

    fn ask_clear(&mut self, cx: &mut Context<Self>) {
        let Some(name) = self.selected.clone().filter(|_| self.screen().is_none()) else { return };
        let message = format!("Clear the history of “{name}”? Its actions and the frames kept with them are deleted. This can’t be undone.");
        let me = cx.entity();
        cx.defer(move |cx| {
            open_alert(cx, "Clear History", "warning", &message, &["Cancel", "Clear History"], move |choice, cx| {
                if choice != Some(1) {
                    return;
                }
                let proxy = me.read(cx).proxy.clone();
                let name = name.clone();
                std::thread::spawn(move || proxy.send(Msg::Cleared(crate::history::clear(&name))));
            });
        });
    }

    fn can_export(&self, cx: &App) -> bool {
        self.replay.as_ref().is_some_and(|(r, _)| {
            let r = r.read(cx);
            r.loaded() && !r.exporting() && !r.actions().is_empty()
        })
    }

    fn ask_export(&mut self, cx: &mut Context<Self>) {
        if !self.can_export(cx) {
            return;
        }
        let Some(replay) = self.replay.as_ref().map(|(r, _)| r.clone()) else { return };
        let name = replay.read(cx).name.clone();
        let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
        let dir = [home.join("Videos"), home.clone()].into_iter().find(|d| d.is_dir()).unwrap_or(home);
        let panel = file_panel::FilePanel::save(dir, format!("{name}.mp4"));
        let proxy = self.proxy.clone();
        cx.defer(move |cx| {
            file_panel::show(cx, "Export Video", panel, move |a, cx| {
                if let file_panel::PanelResult::Chosen(path) = a.result {
                    replay.update(cx, |r, cx| r.start_export(path, proxy, cx));
                }
            });
        });
    }

    /// An export ended: where the video is, or why not.
    fn exported(&mut self, result: &Result<PathBuf, String>, cx: &mut Context<Self>) {
        match result {
            Ok(path) => {
                let message = format!("The video “{}” is saved.", path.file_name().and_then(|n| n.to_str()).unwrap_or(""));
                let path = path.clone();
                let me = cx.entity();
                cx.defer(move |cx| {
                    open_alert(cx, "Export Video", "video", &message, &[olo_ui::tracker::LABEL, "OK"], move |choice, cx| {
                        if choice == Some(0)
                            && let Err(e) = olo_ui::tracker::open(&path)
                        {
                            me.update(cx, |v, cx| v.alert(&format!("Tracker couldn’t show it: {e}"), cx));
                        }
                    });
                });
            }
            Err(e) if e == "cancelled" => {}
            Err(e) => self.alert(&format!("The video couldn’t be made: {e}"), cx),
        }
    }

    pub fn message(&mut self, message: Box<dyn std::any::Any + Send>, cx: &mut Context<Self>) {
        // Another start: `olo-screen [watch NAME]`.
        if let Some(launch) = message.downcast_ref::<olo_ui::instance::Launch>() {
            self.show(launch.args.get(1).cloned(), cx);
            return;
        }
        let message = match message.downcast::<ReplayMsg>() {
            Ok(m) => {
                if let Some((r, _)) = &self.replay {
                    r.update(cx, |r, cx| r.message(*m, cx));
                }
                // (The header and sidebar say the newest action.)
                cx.notify();
                return;
            }
            Err(m) => m,
        };
        let Ok(msg) = message.downcast::<Msg>() else { return };
        match *msg {
            Msg::Screens(list) => {
                self.screens = list;
                let (proxy, running) = (self.proxy.clone(), self.screens.clone());
                std::thread::spawn(move || proxy.send(Msg::Rows(rows(&running))));
            }
            Msg::Rows(rows) => {
                self.rows = rows;
                if self.selected.as_ref().is_none_or(|n| !self.rows.iter().any(|r| &r.name == n)) {
                    let first = self.rows.first().map(|r| r.name.clone());
                    self.stop_controlling();
                    self.selected = None;
                    self.select(first, cx);
                }
            }
            Msg::Frame { name, shot } => {
                if self.selected.as_ref() == Some(&name) && self.watching.as_ref().is_some_and(|w| w.name == name) {
                    self.failure = None;
                    self.live.update(cx, |w, cx| w.set_shot(shot, cx));
                }
                return;
            }
            Msg::Failed { name, message } => {
                if self.selected.as_ref() == Some(&name) {
                    self.failure = Some(message);
                }
            }
            Msg::Started(r) => {
                self.busy = false;
                match r {
                    Ok(screen) => {
                        if !self.screens.contains(&screen) {
                            self.screens.push(screen.clone());
                        }
                        self.select(Some(screen.name), cx);
                        self.set_tab(Tab::Live, cx);
                    }
                    Err(e) => self.alert(&format!("The screen couldn't be started: {e}"), cx),
                }
            }
            Msg::Stopped(r) => {
                if let Err(e) = r {
                    self.alert(&e, cx);
                }
                self.screens = listed();
                let (proxy, running) = (self.proxy.clone(), self.screens.clone());
                std::thread::spawn(move || proxy.send(Msg::Rows(rows(&running))));
            }
            Msg::Cleared(r) => {
                if let Err(e) = r {
                    self.alert(&e, cx);
                }
                self.replay = None;
                let (proxy, running) = (self.proxy.clone(), self.screens.clone());
                std::thread::spawn(move || proxy.send(Msg::Rows(rows(&running))));
            }
        }
        // A screen come or gone changes what Live shows.
        self.watch(cx);
        cx.notify();
    }

    fn menus(&self, cx: &App) -> Vec<Menu> {
        let running = self.screen().is_some();
        let chosen = self.selected.is_some();
        let tab = self.shown_tab();
        let replay = self.replay.as_ref().map(|(r, _)| r.read(cx));
        let history = tab == Tab::History && replay.is_some_and(|r| r.loaded() && !r.actions().is_empty());
        let playing = replay.is_some_and(Replay::playing);
        vec![
            Menu::new("Screen")
                .item("New Screen", "new")
                .key("n")
                .enabled(!self.busy)
                .item("Stop Screen…", "stop")
                .enabled(running)
                .item("Clear History…", "clear")
                .enabled(chosen && !running)
                .separator()
                .item("Export Video…", "export")
                .key("e")
                .enabled(self.can_export(cx))
                .separator()
                .item("Quit", "quit")
                .key("q"),
            Menu::new("View")
                .item("Live", "tab-live")
                .key("1")
                .enabled(running)
                .checked(tab == Tab::Live)
                .item("History", "tab-history")
                .key("2")
                .enabled(chosen)
                .checked(tab == Tab::History)
                .item("Stats", "tab-stats")
                .key("3")
                .enabled(chosen)
                .checked(tab == Tab::Stats)
                .separator()
                .item("Find", "find")
                .key("f")
                .enabled(chosen),
            Menu::new("Playback")
                .item(if playing { "Pause" } else { "Play" }, "play")
                .enabled(history)
                .item("Previous Action", "previous")
                .enabled(history)
                .item("Next Action", "next")
                .enabled(history)
                .separator()
                .item(if running { "Go Live" } else { "Go to Latest" }, "latest")
                .enabled(history),
            Menu::new("Control").item(if self.controlling { "Stop Controlling" } else { "Take Control" }, "control").key("k").enabled(running),
        ]
    }

    fn command(&mut self, command: &str, window: &mut Window, cx: &mut Context<Self>) {
        match command {
            "new" => self.new_screen(cx),
            "stop" => self.ask_stop(cx),
            "clear" => self.ask_clear(cx),
            "export" => self.ask_export(cx),
            "tab-live" if self.screen().is_some() => self.set_tab(Tab::Live, cx),
            "tab-history" => self.set_tab(Tab::History, cx),
            "tab-stats" => self.set_tab(Tab::Stats, cx),
            "find" => {
                self.set_tab(Tab::History, cx);
                if let Some((r, _)) = &self.replay {
                    let r = r.clone();
                    // Once the tab has drawn its search.
                    cx.defer_in(window, move |_, window, cx| {
                        let search = r.clone();
                        search.update(cx, |r, cx| r.focus_search(window, cx));
                    });
                }
            }
            "play" | "previous" | "next" | "latest" => {
                if let Some((r, _)) = &self.replay {
                    r.update(cx, |r, cx| match command {
                        "play" => r.play_pause(cx),
                        "previous" => r.step(-1, cx),
                        "next" => r.step(1, cx),
                        _ => r.latest(cx),
                    });
                }
            }
            "control" => {
                if self.controlling {
                    self.stop_controlling();
                } else {
                    self.take_control(window, cx);
                }
            }
            "quit" => self.quit(cx),
            _ => {}
        }
        cx.notify();
    }

    /// The history of the chosen screen, made when it's first wanted.
    fn replay(&mut self, name: &str, cx: &mut Context<Self>) -> Entity<Replay> {
        if let Some((r, _)) = &self.replay
            && r.read(cx).name == name
        {
            return r.clone();
        }
        let proxy = self.proxy.clone();
        let replay = cx.new(|cx| Replay::new(name, proxy, cx));
        let sub = cx.subscribe(&replay, |v: &mut Viewer, _, e: &Exported, cx| v.exported(&e.0, cx));
        self.replay = Some((replay.clone(), sub));
        replay
    }

    /// Times said move on: draw again when the next one changes.
    fn tick(&mut self, ages: impl IntoIterator<Item = u64>, cx: &mut Context<Self>) {
        let Some(wait) = ages.into_iter().map(next_change).min() else {
            self.ticking = None;
            return;
        };
        // (Drawn again before then: the timer set stands.)
        let due = Instant::now() + wait;
        if self.ticking.as_ref().is_some_and(|(at, _)| *at <= due + Duration::from_millis(20)) {
            return;
        }
        let task = cx.spawn(async move |me, cx| {
            cx.background_executor().timer(wait).await;
            let _ = me.update(cx, |v, cx| {
                v.ticking = None;
                cx.notify();
            });
        });
        self.ticking = Some((due, task));
    }

    /// The screens: running, then stopped ones with a history; New
    /// Screen under them.
    fn sidebar(&mut self, side_w: f32, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let now = crate::history::now_ms();
        // The chosen one's last use as it happens; the others' as listed.
        let latest = self.replay.as_ref().and_then(|(r, _)| {
            let r = r.read(cx);
            r.actions().last().map(|a| (r.name.clone(), a.at + a.ms))
        });
        let last_used = |r: &Row| match &latest {
            Some((n, t)) if *n == r.name => Some(*t),
            _ => r.last_used,
        };
        let running = |n: &str| self.screens.iter().any(|s| s.name == n);
        let groups: [(&str, Vec<&Row>); 2] = [("Running", self.rows.iter().filter(|r| running(&r.name)).collect()), ("Stopped", self.rows.iter().filter(|r| !running(&r.name)).collect())];
        let mut items = Vec::new();
        let mut names: Vec<Option<String>> = Vec::new();
        for (heading, rs) in &groups {
            if rs.is_empty() {
                continue;
            }
            items.push(sidebar::Row::heading(*heading));
            names.push(None);
            for r in rs {
                let row = sidebar::Row::new("agent-screens", r.name.clone());
                items.push(match last_used(r) {
                    Some(t) => row.value(ago_short(t, now)),
                    None => row,
                });
                names.push(Some(r.name.clone()));
            }
        }
        let first = names.iter().position(Option::is_some).unwrap_or(0);
        let chosen = names.iter().position(|n| n.as_deref() == self.selected.as_deref()).unwrap_or(first);
        let me = cx.entity();
        let pick = me.clone();
        let list = Sidebar::new("screens", items, chosen).on_choose(move |i, _, cx| {
            if let Some(Some(name)) = names.get(i) {
                let name = name.clone();
                pick.update(cx, |v, cx| v.select(Some(name), cx));
            }
        });
        // The sidebar's ground down the column, and the line over New
        // Screen, from the column's own box (its two parts are laid out
        // a device pixel apart at some scales).
        let ground = canvas(
            |_, _, _| {},
            |b, _, window, cx| {
                let p = theme::palette(cx);
                style::fill_rect(window, b, sidebar::ground(&p));
                style::vline_left_of(window, b.right(), b.top(), b.bottom(), sidebar::edge(&p));
                let top = b.bottom() - px(FOOTER_H);
                style::hline(window, b.left(), b.right() - style::hairline(window), top, style::shade(&p, p.panel_background, 1.18));
            },
        )
        .absolute()
        .inset_0();
        let busy = self.busy;
        let new = Button::new(if busy { "Starting…" } else { "New Screen" }).id("new-screen").enabled(!busy).on_click(move |_, cx| me.update(cx, |v, cx| v.new_screen(cx)));
        div()
            .w(px(side_w))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .relative()
            .child(ground)
            .child(div().flex_1().min_h_0().child(list))
            .child(div().h(px(FOOTER_H)).flex_shrink_0().flex().justify_center().pt(px(m::SPACING)).child(new))
    }

    /// The chosen screen's name, how it's doing, and what can be done
    /// with it.
    fn header(&mut self, name: &str, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let now = crate::history::now_ms();
        let screen = self.screen().cloned();
        let (actions, last, started) = match &self.replay {
            Some((r, _)) => {
                let r = r.read(cx);
                (r.actions().len(), r.actions().last().map(|a| a.at + a.ms), r.started())
            }
            None => (0, None, None),
        };
        let mut parts = Vec::new();
        let running = screen.is_some();
        if let Some(s) = &screen {
            parts.push("Running".to_owned());
            parts.push(format!("{} × {}", s.width, s.height));
            if let Some(t) = started {
                parts.push(format!("up {}", stats::span(now.saturating_sub(t))));
            }
        } else {
            parts.push("Stopped".to_owned());
        }
        if actions > 0 {
            parts.push(format!("{} action{}", style::thousands(actions), if actions == 1 { "" } else { "s" }));
        }
        if let Some(t) = last {
            parts.push(format!("last {}", stats::ago(t, now)));
        }
        let line = parts.join("  ·  ");
        let title = SharedString::from(name.to_owned());
        let face = canvas(
            |_, _, _| {},
            move |r, _, window, cx| {
                let p = theme::palette(cx);
                let icon = style::snap(window, olo_ui::gpui::Bounds { origin: point(r.left(), r.center().y - px(16.0)), size: size(px(32.0), px(32.0)) });
                icons::paint(window, icon, "agent-screens");
                let x = icon.right() + px(m::SPACING);
                let w = (r.right() - x).max(px(0.0));
                let t = style::label_fit(window, cx, x, r.top() + px(11.0), style::Align::Left, title.clone(), 15.0, style::ui_font(FontWeight::BOLD), c(p.panel_text), w);
                agent::record(window, cx, agent::Node { role: "Heading", name: Some(title.clone()), bounds: t, enabled: true, ..Default::default() });
                let line_y = r.top() + px(29.0);
                let dot = if running { c(p.success) } else { style::disabled_text(&p) };
                let d = style::snap_centred(window, point(x + px(4.0), line_y), 8.0, 8.0);
                window.paint_quad(olo_ui::gpui::fill(d, dot).corner_radii(d.size.width / 2.0));
                let t = style::label_fit(window, cx, x + px(14.0), line_y, style::Align::Left, line.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), style::disabled_text(&p), (w - px(14.0)).max(px(0.0)));
                agent::record(window, cx, agent::Node::label(line.clone(), t));
            },
        )
        .flex_1()
        .min_w_0()
        .h_full();
        let me = cx.entity();
        let (stop, export) = (me.clone(), me);
        let first = if running {
            Button::new("Stop Screen…").min_width(96.0).on_click(move |_, cx| stop.update(cx, |v, cx| v.ask_stop(cx)))
        } else {
            Button::new("Clear History…").min_width(96.0).on_click(move |_, cx| stop.update(cx, |v, cx| v.ask_clear(cx)))
        };
        let export = Button::new("Export Video…").min_width(96.0).enabled(self.can_export(cx)).on_click(move |_, cx| export.update(cx, |v, cx| v.ask_export(cx)));
        div().flex().flex_row().items_center().gap(px(m::SPACING)).h(px(HEADER_H)).flex_shrink_0().child(face).child(export).child(first)
    }

    /// The screen as it is now, and Take Control.
    fn live_tab(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let Some(screen) = self.screen().cloned() else { return div().into_any_element() };
        let p = theme::palette(cx);
        let text: Option<SharedString> = if self.live.read(cx).has_shot() { None } else { Some(self.failure.clone().unwrap_or_else(|| "Connecting…".into()).into()) };
        if let Some(t) = text {
            self.live.update(cx, |w, cx| w.clear(Some(t), cx));
        }
        let controlling = self.controlling;
        let me = cx.entity();
        // Over the picture: the ring saying the keys go there, and the
        // person's pointer and keys sent on to the screen.
        let well = self.live.clone();
        let forward = me.clone();
        let input = canvas(
            move |b, window, _| window.insert_hitbox(b, HitboxBehavior::Normal),
            move |_, hitbox, window, cx| {
                let Some(pic) = well.read(cx).shown() else { return };
                if !controlling {
                    return;
                }
                let p = theme::palette(cx);
                let ring = pic.dilate(px(3.0));
                window.paint_quad(olo_ui::gpui::outline(ring, c(p.keyboard_navigation), olo_ui::gpui::BorderStyle::Solid).border_widths(px(2.0)));
                let (sw, sh) = (screen.width as f64, screen.height as f64);
                let to_screen = move |pos: olo_ui::gpui::Point<olo_ui::gpui::Pixels>| -> (f64, f64) {
                    let x = (f32::from(pos.x - pic.left()) / f32::from(pic.size.width)).clamp(0.0, 1.0) as f64 * sw;
                    let y = (f32::from(pos.y - pic.top()) / f32::from(pic.size.height)).clamp(0.0, 1.0) as f64 * sh;
                    (x, y)
                };
                let button = |b: MouseButton| match b {
                    MouseButton::Left => Some(screens::LEFT),
                    MouseButton::Right => Some(screens::RIGHT),
                    MouseButton::Middle => Some(screens::MIDDLE),
                    _ => None,
                };
                let (h, f) = (hitbox.clone(), forward.clone());
                window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, cx| {
                    if !phase.bubble() {
                        return;
                    }
                    let held = !f.read(cx).pressed.is_empty();
                    if (h.is_hovered(window) && pic.contains(&e.position)) || held {
                        let (x, y) = to_screen(e.position);
                        f.update(cx, |v, _| v.send(vec![SyntheticEvent::Move { x, y }]));
                    }
                });
                let (h, f) = (hitbox.clone(), forward.clone());
                window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
                    if !phase.bubble() || !h.is_hovered(window) || !pic.contains(&e.position) {
                        return;
                    }
                    let Some(button) = button(e.button) else { return };
                    let (x, y) = to_screen(e.position);
                    f.update(cx, |v, cx| {
                        window.focus(&v.live_focus);
                        v.pressed.insert(button);
                        v.send(vec![SyntheticEvent::Move { x, y }, SyntheticEvent::Button { button, pressed: true }]);
                        cx.stop_propagation();
                    });
                });
                let f = forward.clone();
                window.on_mouse_event(move |e: &MouseUpEvent, phase, _, cx| {
                    if !phase.bubble() {
                        return;
                    }
                    let Some(button) = button(e.button) else { return };
                    if !f.read(cx).pressed.contains(&button) {
                        return;
                    }
                    let (x, y) = to_screen(e.position);
                    f.update(cx, |v, _| {
                        v.pressed.remove(&button);
                        v.send(vec![SyntheticEvent::Move { x, y }, SyntheticEvent::Button { button, pressed: false }]);
                    });
                });
                let (h, f) = (hitbox, forward.clone());
                window.on_mouse_event(move |e: &ScrollWheelEvent, phase, window, cx| {
                    if !phase.bubble() || !h.is_hovered(window) {
                        return;
                    }
                    let (x, y) = to_screen(e.position);
                    let scroll = match e.delta {
                        ScrollDelta::Pixels(d) => SyntheticEvent::Scroll { dx: -f64::from(f32::from(d.x)), dy: -f64::from(f32::from(d.y)) },
                        ScrollDelta::Lines(d) => SyntheticEvent::Wheel { dx: -notch(d.x), dy: -notch(d.y) },
                    };
                    f.update(cx, |v, _| v.send(vec![SyntheticEvent::Move { x, y }, scroll]));
                });
            },
        )
        .absolute()
        .inset_0();
        let picture = div()
            .id("live-picture")
            .key_context(CONTROLLED)
            .track_focus(&self.live_focus)
            .flex_1()
            .min_h_0()
            .relative()
            .on_key_down(cx.listener(|v, e: &KeyDownEvent, window, cx| v.key(e.keystroke.clone(), true, e.is_held, window, cx)))
            .on_key_up(cx.listener(|v, e: &KeyUpEvent, window, cx| v.key(e.keystroke.clone(), false, false, window, cx)))
            .on_modifiers_changed(cx.listener(|v, e: &ModifiersChangedEvent, _, cx| {
                if v.controlling {
                    let mut out = Vec::new();
                    v.sync_modifiers(&e.modifiers, &mut out);
                    v.send(out);
                    cx.stop_propagation();
                }
            }))
            .child(self.live.clone())
            .child(input);

        // What is going on, and the control button.
        let now = crate::history::now_ms();
        let (status, color) = if controlling {
            ("You have control: your pointer and keys go to this screen.".to_owned(), c(p.panel_text))
        } else {
            let latest = self.replay.as_ref().and_then(|(r, _)| r.read(cx).actions().last().cloned());
            match latest {
                Some(a) => {
                    let what = replay::command(&a);
                    let mark = if a.error.is_some() { "  ✗" } else { "" };
                    (format!("Latest: {what}{mark}  ·  {}", stats::ago(a.at + a.ms, now)), style::disabled_text(&p))
                }
                None => ("Watching. Nothing has been done on it yet.".to_owned(), style::disabled_text(&p)),
            }
        };
        let status = canvas(
            |_, _, _| {},
            move |b, _, window, cx| {
                let t = style::label_fit(window, cx, b.left(), b.center().y, style::Align::Left, status.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL), color, b.size.width);
                agent::record(window, cx, agent::Node::label(status.clone(), t));
            },
        )
        .flex_1()
        .min_w_0()
        .h_full();
        let control = Button::new(if controlling { "Stop Controlling" } else { "Take Control" }).id("take-control").min_width(140.0).pushed(controlling).on_click(move |window, cx| {
            me.update(cx, |v, cx| {
                if v.controlling {
                    v.stop_controlling();
                    cx.notify();
                } else {
                    v.take_control(window, cx);
                }
            })
        });
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap(px(m::SPACING))
            .child(picture)
            .child(div().flex().flex_row().items_center().gap(px(m::SPACING)).h(px(m::CONTROL_HEIGHT)).flex_shrink_0().child(status).child(control))
            .into_any_element()
    }

    /// Send `events` to the screen being controlled.
    fn send(&self, events: Vec<SyntheticEvent>) {
        if !self.controlling || events.is_empty() {
            return;
        }
        if let Some(screen) = self.screen().cloned() {
            let _ = self.input.send((screen, events));
        }
    }

    /// Press or let go of Ctrl, Shift and Alt on the screen to match
    /// `modifiers`.
    fn sync_modifiers(&mut self, modifiers: &Modifiers, out: &mut Vec<SyntheticEvent>) {
        let now = [modifiers.control, modifiers.shift, modifiers.alt];
        for (i, key) in MODIFIERS.into_iter().enumerate() {
            if now[i] != self.held[i] {
                out.push(SyntheticEvent::Key { key, pressed: now[i] });
                self.held[i] = now[i];
            }
        }
    }

    /// A key on the picture being controlled: to the screen, but for the
    /// one that stops controlling (⌘K).
    fn key(&mut self, k: olo_ui::gpui::Keystroke, pressed: bool, held: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !self.controlling {
            return;
        }
        if pressed && olo_ui::menu::shortcut_command(window, cx, &k).as_deref() == Some("control") {
            return;
        }
        cx.stop_propagation();
        // (The screen repeats a key held itself.)
        if held {
            return;
        }
        let mut out = Vec::new();
        self.sync_modifiers(&k.modifiers, &mut out);
        // A character that needs Shift on the agent's keyboard come
        // without it (typed by an input method, a virtual keyboard):
        // typed there with Shift, at once.
        let mut chars = k.key.chars();
        let shifted = match (chars.next(), chars.next()) {
            (Some(ch), None) => screens::key_for(ch).is_some_and(|(_, shift)| shift) && !k.modifiers.shift,
            _ => false,
        };
        if shifted {
            if pressed && let Some(code) = evdev(&k.key) {
                out.extend([(42, true), (code, true), (code, false), (42, false)].map(|(key, pressed)| SyntheticEvent::Key { key, pressed }));
            }
        } else if pressed {
            if let Some(code) = evdev(&k.key) {
                self.down.push((k.key.clone(), code));
                out.push(SyntheticEvent::Key { key: code, pressed: true });
            }
        } else if let Some(i) = self.down.iter().position(|(name, _)| *name == k.key).or_else(|| evdev(&k.key).and_then(|code| self.down.iter().position(|(_, c)| *c == code))) {
            let (_, code) = self.down.remove(i);
            out.push(SyntheticEvent::Key { key: code, pressed: false });
        }
        self.send(out);
    }
}

impl Render for Viewer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let me = cx.entity();
        let bar_me = me.clone();
        let bar = MenuBar::new(self.menus(cx), move |cmd, window, cx| bar_me.update(cx, |v, cx| v.command(cmd, window, cx)));
        let root = div().size_full().flex().flex_col().child(bar);
        if self.rows.is_empty() {
            let busy = self.busy;
            let empty = EmptyState::new("No agent screens yet. An agent starts one with “olo-screen start”, or start one yourself.")
                .icon("agent-screens")
                .action(if busy { "Starting…" } else { "New Screen" }, move |_, cx| me.update(cx, |v, cx| v.new_screen(cx)));
            self.tick(None, cx);
            return root.child(div().flex_1().min_h_0().p(px(m::WINDOW_INSET)).child(empty));
        }
        let full_w = f32::from(window.viewport_size().width);
        let side_w = (full_w * 0.2).round().clamp(SIDEBAR_MIN_W, SIDEBAR_W);
        let sidebar = self.sidebar(side_w, cx);
        let Some(name) = self.selected.clone() else {
            return root.child(div().flex_1().min_h_0().flex().flex_row().child(sidebar));
        };
        let replay = self.replay(&name, cx);
        let running = self.screen().is_some();
        // The tab's area: inside the window's inset, then the tab view's.
        let area_w = full_w - side_w - 4.0 * m::WINDOW_INSET;
        replay.update(cx, |r, _| {
            r.width = area_w;
            r.running = running;
        });
        let header = self.header(&name, cx);
        let tabs: Vec<Tab> = if running { vec![Tab::Live, Tab::History, Tab::Stats] } else { vec![Tab::History, Tab::Stats] };
        let labels: Vec<SharedString> = tabs.iter().map(|t| SharedString::from(t.label())).collect();
        let shown = self.shown_tab();
        let i = tabs.iter().position(|t| *t == shown).unwrap_or(0);
        let content = match shown {
            Tab::Live => self.live_tab(cx).into_any_element(),
            Tab::History => replay.clone().into_any_element(),
            Tab::Stats => stats::view(&replay, running, area_w, &self.stats_scroll, cx),
        };
        let pick = me.clone();
        let tab_view = TabView::new("screen-tabs", labels, i).on_select(move |i, _, cx| {
            let tab = tabs[i];
            pick.update(cx, |v, cx| v.set_tab(tab, cx));
        });
        // Times said move on ("3 s ago", "up 2 min").
        let now = crate::history::now_ms();
        let (last, started) = {
            let r = replay.read(cx);
            (r.actions().last().map(|a| a.at + a.ms), r.started())
        };
        let mut ages: Vec<u64> = self.rows.iter().filter_map(|r| r.last_used).chain(last).map(|t| now.saturating_sub(t)).collect();
        if running {
            ages.extend(started.map(|t| now.saturating_sub(t)));
        }
        self.tick(ages, cx);
        root.child(
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_row()
                .child(sidebar)
                .child(div().flex_1().min_w_0().p(px(m::WINDOW_INSET)).flex().flex_col().gap(px(m::SPACING)).child(header).child(div().flex_1().min_h_0().child(tab_view.child(content)))),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_and_notches() {
        assert_eq!(evdev("a"), Some(30));
        assert_eq!(evdev("!"), Some(2));
        assert_eq!(evdev("?"), Some(53));
        assert_eq!(evdev("enter"), Some(28));
        assert_eq!(evdev("space"), Some(57));
        assert_eq!(evdev("f12"), Some(88));
        assert_eq!(evdev("insert"), Some(110));
        assert_eq!(evdev("é"), None);
        assert_eq!((notch(0.0), notch(-3.0), notch(0.5)), (0, -1, 1));
    }

    #[test]
    fn moves_run_together() {
        let mut events = vec![
            SyntheticEvent::Move { x: 1.0, y: 1.0 },
            SyntheticEvent::Move { x: 2.0, y: 2.0 },
            SyntheticEvent::Button { button: screens::LEFT, pressed: true },
            SyntheticEvent::Move { x: 3.0, y: 3.0 },
            SyntheticEvent::Move { x: 4.0, y: 4.0 },
        ];
        run_together(&mut events);
        assert_eq!(events, vec![SyntheticEvent::Move { x: 2.0, y: 2.0 }, SyntheticEvent::Button { button: screens::LEFT, pressed: true }, SyntheticEvent::Move { x: 4.0, y: 4.0 }]);
    }
}
