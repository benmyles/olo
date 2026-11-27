//! Agent Screens, the window: the agents' screens in a sidebar (running,
//! then stopped ones with a history kept), and the one chosen shown in
//! [`olo_screen_pane::ScreenPane`] under a header saying what it is and how
//! it's doing, in four tabs: Live (the screen as it is, with the agent's
//! pointer, and Take Control to step in), History, Stats and Passwords
//! (besides or without what every screen has, Screen ▸ Passwords for Every
//! Screen…).

use olo_sync::Mutex;
use std::{
    os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use olo_kit::{
    Child, Cx, Element, IntoElement as _, Menu, MenuBar, Modifiers, ParentElement as _, Pixels, Point, Proxy, Styled as _, View, WindowCx, WindowHandle, canvas, div, metrics as m, px, style, theme,
    widgets::{Button, EmptyState, Sidebar, open_alert, sidebar},
};
use olo_screen_pane::{
    LocalScreenSource, Padded, ScreenPane, ScreenPaneEvent, ScreenSource as _, Tab, history, local,
    screens::{self, Screen},
    shares,
};
use serde::{Deserialize, Serialize};

/// The sidebar's width, less in a small window.
const SIDEBAR_W: f32 = 196.0;
const SIDEBAR_MIN_W: f32 = 160.0;
/// New Screen's place under the sidebar.
const FOOTER_H: f32 = m::CONTROL_HEIGHT + 2.0 * m::SPACING;

pub enum Msg {
    Screens(u64, Vec<Screen>),
    /// Every screen to list: running, or stopped with a history kept;
    /// each with when it was last used, most recent first.
    Rows(u64, Vec<Row>),
    /// Both running screens and rows refreshed together off the UI thread.
    Refreshed(u64, Vec<Screen>, Vec<Row>),
    /// A listed screen's history grew: used then (ms since 1970).
    Used(String, u64),
    Started(Result<Screen, String>),
    Stopped(Result<(), String>),
    /// Screens archived, unarchived, stopped or deleted together: what
    /// went wrong, if anything.
    Changed(Vec<String>),
}

#[derive(Default, Serialize, Deserialize)]
struct Saved {
    selected: Option<String>,
    #[serde(default)]
    tab: Tab,
    /// The Archived section hidden (View ▸ Show Archived off): listed
    /// unless.
    #[serde(default)]
    hide_archived: bool,
}

/// A screen in the list.
#[derive(Debug, Clone)]
pub struct Row {
    name: String,
    last_used: Option<u64>,
    /// The name the person gave it (Rename…), shown for its own.
    title: Option<String>,
    /// Archived (stopped, listed only with View ▸ Show Archived).
    archived: bool,
}

impl Row {
    fn shown(&self) -> &str {
        self.title.as_deref().unwrap_or(&self.name)
    }
}

/// The screens running (`running`) and those stopped with a history, by
/// last use, most recent first. Reads the histories: off the UI thread.
fn rows(running: &[Screen]) -> Vec<Row> {
    let own = std::env::var("OLO_SCREEN").ok();
    let mut names: Vec<String> = running.iter().map(|s| s.name.clone()).collect();
    let kept: Vec<String> = history::names().into_iter().filter(|n| !names.contains(n) && Some(n) != own.as_ref()).collect();
    names.extend(kept);
    let mut rows: Vec<Row> = names
        .into_iter()
        .map(|name| {
            let last_used = history::read(&name).iter().map(|a| a.at.saturating_add(a.ms)).max();
            let title = history::title(&name);
            // (A screen started again under its name isn't archived.)
            let on = running.iter().any(|s| s.name == name);
            let archived = history::archived(&name) && (!on || history::set_archived(&name, false).is_err());
            Row { name, last_used, title, archived }
        })
        .collect();
    rows.sort_by_key(|r| (std::cmp::Reverse(r.last_used), r.name.clone()));
    rows
}

/// The screens to show: all but the one this viewer itself is on, if it
/// runs on one (it would show itself, endlessly).
fn listed() -> Vec<Screen> {
    let own = std::env::var("OLO_SCREEN").ok();
    screens::list().into_iter().filter(|s| Some(&s.name) != own.as_ref()).collect()
}

/// Open a non-blocking `pidfd` for a running screen session so kernel exit
/// notifications replace the legacy 30 s polling loop (0 wakeups/s at rest).
fn open_pidfd(pid: i32) -> Option<OwnedFd> {
    if pid <= 0 {
        return None;
    }
    // SAFETY: Linux `pidfd_open(pid, PIDFD_NONBLOCK)` returns an owned fd or -1.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, libc::PIDFD_NONBLOCK) } as i32;
    if fd < 0 { None } else { Some(unsafe { OwnedFd::from_raw_fd(fd) }) }
}

fn make_wake_pipe() -> Option<(OwnedFd, OwnedFd)> {
    let mut fds = [0i32; 2];
    // SAFETY: valid 2-int buffer.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) } != 0 {
        return None;
    }
    // SAFETY: `pipe2` succeeded and returned two freshly opened fds.
    Some((unsafe { OwnedFd::from_raw_fd(fds[0]) }, unsafe { OwnedFd::from_raw_fd(fds[1]) }))
}

fn poke_pipe(fd: &OwnedFd) {
    let b = [1u8; 1];
    // SAFETY: valid owned write-end pipe fd.
    let _ = unsafe { libc::write(fd.as_raw_fd(), b.as_ptr().cast(), 1) };
}

/// Retains both the directory watcher and the event-driven `pidfd` exit
/// watcher for running screens (0 wakeups/s at rest; dropped cleanly with
/// [`Viewer`]).
struct ListWatcher {
    _watch: Option<olo_kit::watch::Watch>,
    stop: Arc<AtomicBool>,
    wake_tx: Option<Arc<OwnedFd>>,
}

impl Drop for ListWatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(tx) = &self.wake_tx {
            poke_pipe(tx);
        }
    }
}

/// Follow the list of screens as their files come and go (`watch`), and watch
/// every running session PID via `pidfd_open` + `poll` so a killed session is
/// noticed immediately with zero periodic timer wakeups at rest.
fn watch_list(proxy: Proxy, seq: Arc<AtomicU64>) -> ListWatcher {
    let last = Arc::new(Mutex::new(None::<Vec<Screen>>));
    let pids = Arc::new(Mutex::new(Vec::<i32>::new()));
    let stop = Arc::new(AtomicBool::new(false));
    let pipe = make_wake_pipe();
    let wake_tx = pipe.as_ref().map(|(_, w)| Arc::new(w.try_clone().unwrap()));
    let wake_rx = pipe.map(|(r, _)| r);

    let pids_update = pids.clone();
    let wake_update = wake_tx.clone();
    let look = Arc::new(move || {
        let gen_id = seq.fetch_add(1, Ordering::SeqCst) + 1;
        let now = listed();
        let mut last_g = last.lock().unwrap_or_else(|e| e.into_inner());
        if last_g.as_ref() != Some(&now) {
            let new_pids: Vec<i32> = now.iter().map(|s| s.pid).collect();
            *pids_update.lock().unwrap_or_else(|e| e.into_inner()) = new_pids;
            if let Some(tx) = &wake_update {
                poke_pipe(tx);
            }
            proxy.send(Msg::Screens(gen_id, now.clone()));
            *last_g = Some(now);
        }
    });
    look();

    let on_change = look.clone();
    let dir = screens::dir();
    let followed = dir.clone();
    let watch = olo_kit::watch::watch([dir], move |changed| {
        // A screen's own file, not a viewer's frame passing through.
        let items: Vec<&std::path::PathBuf> = changed.iter().filter(|p| p.parent() == Some(followed.as_path())).collect();
        if items.is_empty() || items.iter().any(|p| p.extension().is_some_and(|e| e == "json")) {
            on_change();
        }
    });

    if let Some(rx) = wake_rx {
        let stop_bg = stop.clone();
        let pids_bg = pids.clone();
        let look_bg = look.clone();
        let _ = std::thread::Builder::new().name("screen-pids".into()).spawn(move || {
            let mut drain = [0u8; 64];
            loop {
                if stop_bg.load(Ordering::Acquire) {
                    return;
                }
                let cur_pids = pids_bg.lock().unwrap_or_else(|e| e.into_inner()).clone();
                let mut pidfds: Vec<OwnedFd> = Vec::with_capacity(cur_pids.len());
                let mut missing = false;
                for pid in cur_pids {
                    match open_pidfd(pid) {
                        Some(fd) => pidfds.push(fd),
                        None => missing = true,
                    }
                }
                if missing {
                    look_bg();
                    if stop_bg.load(Ordering::Acquire) {
                        return;
                    }
                }
                let mut pfds = Vec::with_capacity(1 + pidfds.len());
                pfds.push(libc::pollfd { fd: rx.as_raw_fd(), events: libc::POLLIN, revents: 0 });
                for fd in &pidfds {
                    pfds.push(libc::pollfd { fd: fd.as_raw_fd(), events: libc::POLLIN, revents: 0 });
                }
                // Sleep indefinitely until either the running PID list changes or a session exits.
                // SAFETY: `pfds` holds valid open fds for the duration of `poll`.
                let rc = unsafe { libc::poll(pfds.as_mut_ptr(), pfds.len() as libc::nfds_t, -1) };
                if rc < 0 {
                    if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                        continue;
                    }
                    return;
                }
                if pfds[0].revents != 0 {
                    while unsafe { libc::read(rx.as_raw_fd(), drain.as_mut_ptr().cast(), drain.len()) } > 0 {}
                }
                if stop_bg.load(Ordering::Acquire) {
                    return;
                }
                if pfds[1..].iter().any(|p| p.revents != 0) {
                    look_bg();
                }
            }
        });
    }

    ListWatcher { _watch: watch, stop, wake_tx }
}

pub struct Viewer {
    screens: Vec<Screen>,
    rows: Vec<Row>,
    selected: Option<String>,
    /// The shared pane showing the chosen screen.
    pane: Child<ScreenPane>,
    /// Current local source bound to `pane` (kept so metadata refreshes update
    /// it in place without rebuilding `Replay` or restarting the live stream).
    source: Option<Arc<LocalScreenSource>>,
    busy: bool,
    /// Shared monotonic sequence number ordering sidebar list & row snapshots
    /// so an older `Refreshed` worker reply can never overwrite newer state.
    list_seq: Arc<AtomicU64>,
    applied_seq: u64,
    /// Retained directory + `pidfd` watcher (dropped cleanly with `Viewer`).
    _list_watch: ListWatcher,
    /// Every listed screen's history followed (its row's last use as it
    /// grows, not only the chosen one's: Ben, 2026-11-15, a row's time
    /// moved on only when it was clicked).
    used_watch: Option<olo_kit::watch::Watch>,
    proxy: Proxy,
    /// Screens chosen with the one shown (⌘- and ⇧-clicks, Select All),
    /// for what's done to several at once; and where a ⇧ run starts.
    also: Vec<String>,
    anchor: Option<String>,
    /// The archived screens listed too, in a section of their own.
    show_archived: bool,
    /// The window of what every screen has.
    shares_everywhere: Option<WindowHandle<Padded>>,
}

impl Viewer {
    pub fn new(show: Option<String>, cx: &mut Cx<Viewer>) -> Viewer {
        let saved: Saved = olo_kit::restored_state().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        let proxy = olo_kit::proxy();
        let list_seq = Arc::new(AtomicU64::new(0));
        let list_watch = watch_list(proxy.clone(), list_seq.clone());
        let screens = listed();
        let selected = saved
            .selected
            .filter(|s| screens::valid_name(s) && (screens.iter().any(|x| &x.name == s) || history::dir_checked(s).is_ok_and(|d| d.is_dir())))
            .or_else(|| screens.first().map(|s| s.name.clone()));
        let pane = cx.new_view(|cx| {
            let mut p = ScreenPane::new(saved.tab, cx);
            p.set_host_lifecycle(true);
            p
        });
        cx.subscribe(&pane, |v: &mut Viewer, e: &ScreenPaneEvent, cx| match e {
            ScreenPaneEvent::StartRequested(_) => {
                v.start_chosen(cx);
            }
            ScreenPaneEvent::StopRequested(_) => {
                v.ask_stop(cx);
            }
            ScreenPaneEvent::Started(info) => {
                v.refresh_all();
                v.select(Some(info.id.name.clone()), cx);
                cx.invalidate();
            }
            ScreenPaneEvent::Stopped(_) | ScreenPaneEvent::Cleared(_) | ScreenPaneEvent::RenameFailed(_) => {
                v.refresh_all();
                cx.invalidate();
            }
            ScreenPaneEvent::Renamed(id, title) => {
                if let Some(r) = v.rows.iter_mut().find(|r| r.name == id.name) {
                    r.title = title.clone();
                }
                cx.invalidate();
            }
            ScreenPaneEvent::Used(id, at) => {
                if let Some(r) = v.rows.iter_mut().find(|r| r.name == id.name)
                    && r.last_used.is_none_or(|t| t < *at)
                {
                    r.last_used = Some(*at);
                }
                cx.invalidate();
            }
            ScreenPaneEvent::Controlling(_, _) | ScreenPaneEvent::TabChanged(_) => {
                cx.invalidate();
            }
        });
        let mut v = Viewer {
            screens,
            rows: Vec::new(),
            selected,
            pane,
            source: None,
            busy: false,
            list_seq,
            applied_seq: 0,
            _list_watch: list_watch,
            used_watch: None,
            proxy,
            also: Vec::new(),
            anchor: None,
            show_archived: !saved.hide_archived,
            shares_everywhere: None,
        };
        v.sync_pane(cx);
        if show.is_some() {
            v.show(show, cx);
        }
        v
    }

    /// Choose `name` (from the command line, or a forwarded start), and
    /// bring the window forward. Ignores invalid or unknown names so a
    /// forwarded request cannot bind a synthetic unlisted screen.
    pub fn show(&mut self, name: Option<String>, cx: &mut Cx<Self>) {
        if let Some(name) = name
            && screens::valid_name(&name)
            && (self.screens.iter().any(|s| s.name == name) || self.rows.iter().any(|r| r.name == name) || history::dir_checked(&name).is_ok_and(|d| d.is_dir()))
        {
            self.select(Some(name), cx);
            self.pane.update(cx, |p, cx| p.set_tab(Tab::Live, cx));
        }
        cx.activate_window();
    }

    /// The window closed, or Quit: let go of the screen, and end.
    pub fn quit(&mut self, cx: &mut Cx<Self>) {
        self.pane.update(cx, |p, cx| p.stop_all(cx));
        olo_kit::quit();
    }

    /// What to keep over an upgrade.
    pub fn saved(&self, cx: &WindowCx) -> String {
        let tab = self.pane.read(cx).tab();
        serde_json::to_string(&Saved { selected: self.selected.clone(), tab, hide_archived: !self.show_archived }).unwrap_or_default()
    }

    fn refresh_rows(&self, seq: u64) {
        let (proxy, running) = (self.proxy.clone(), self.screens.clone());
        std::thread::spawn(move || proxy.send(Msg::Rows(seq, rows(&running))));
    }

    /// Re-enumerate running screens and history rows off the UI thread.
    fn refresh_all(&self) {
        let proxy = self.proxy.clone();
        let seq = self.list_seq.fetch_add(1, Ordering::SeqCst) + 1;
        std::thread::spawn(move || {
            let screens = listed();
            let rows = rows(&screens);
            proxy.send(Msg::Refreshed(seq, screens, rows));
        });
    }

    fn set_busy(&mut self, busy: bool, cx: &mut Cx<Self>) {
        self.busy = busy;
        self.pane.update(cx, |p, cx| p.set_busy(busy, cx));
    }

    fn info_for(&self, name: &str) -> olo_screen_pane::ScreenInfo {
        let running = self.screens.iter().find(|s| s.name == name);
        let row = self.rows.iter().find(|r| r.name == name);
        local::screen_info_for(name, running, row.and_then(|r| r.title.clone()), row.is_some_and(|r| r.archived), row.and_then(|r| r.last_used))
    }

    fn sync_pane(&mut self, cx: &mut Cx<Self>) {
        let Some(name) = self.selected.clone() else {
            if self.source.take().is_some() {
                self.pane.update(cx, |p, cx| p.set_source(None, cx));
            }
            return;
        };
        let running_started = self.screens.iter().find(|s| s.name == name).map(|s| s.started);
        let info = self.info_for(&name);
        if let Some(src) = &self.source
            && src.id().name == name
        {
            src.set_info(info.clone());
            src.set_session_started(running_started);
            self.pane.update(cx, |p, cx| p.update_info(info, cx));
        } else {
            let src = Arc::new(LocalScreenSource::new(info));
            src.set_session_started(running_started);
            self.source = Some(src.clone());
            self.pane.update(cx, |p, cx| p.set_source(Some(src), cx));
        }
    }

    fn select(&mut self, name: Option<String>, cx: &mut Cx<Self>) {
        if self.selected != name {
            self.selected = name;
            self.sync_pane(cx);
            cx.invalidate();
        }
    }

    fn alert(&self, message: &str, cx: &mut Cx<Self>) {
        open_alert(cx, "Agent Screens", "stop", message, &["OK"], |_, _| {});
    }

    fn new_screen(&mut self, cx: &mut Cx<Self>) {
        if self.busy {
            return;
        }
        self.set_busy(true, cx);
        let proxy = self.proxy.clone();
        // (A new one: never a stopped or archived screen's name.)
        std::thread::spawn(move || proxy.send(Msg::Started(screens::start(&screens::new_name(), &screens::Options::default()))));
        cx.invalidate();
    }

    /// The stopped (and archived) screens chosen started again, each as
    /// it was (`screens::kept`), out of the archive; the one shown is
    /// then watched.
    fn start_chosen(&mut self, cx: &mut Cx<Self>) {
        let names: Vec<String> = self.chosen().into_iter().filter(|n| !self.screens.iter().any(|s| &s.name == n)).collect();
        if self.busy || names.is_empty() {
            return;
        }
        self.set_busy(true, cx);
        // (The one shown last, so it is the one watched.)
        let shown = self.selected.clone();
        let mut order = names;
        if let Some(i) = shown.as_ref().and_then(|s| order.iter().position(|n| n == s)) {
            let s = order.remove(i);
            order.push(s);
        }
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let mut failed = Vec::new();
            let mut last = None;
            for n in &order {
                match screens::start(n, &screens::kept(n).unwrap_or_default()) {
                    Ok(s) => last = Some(Ok(s)),
                    Err(e) => failed.push(format!("{n}: {e}")),
                }
            }
            // (Those that didn't start said too, when others did.)
            if last.is_some() && !failed.is_empty() {
                proxy.send(Msg::Changed(failed.clone()));
            }
            proxy.send(Msg::Started(last.unwrap_or_else(|| Err(failed.join("\n")))));
        });
        cx.invalidate();
    }

    /// Ask, once for all, before the running screens chosen are stopped.
    fn ask_stop(&mut self, cx: &mut Cx<Self>) {
        // Capture `(name, started)` generation at dialog-open time so confirming
        // Stop can never stop a replacement session started while the dialog was open.
        let targets: Vec<(String, u64)> = self.chosen().into_iter().filter_map(|n| self.screens.iter().find(|s| s.name == n).map(|s| (n, s.started))).collect();
        let shown = |n: &String| self.rows.iter().find(|r| &r.name == n).map_or(n.as_str(), Row::shown).to_owned();
        let (message, button) = match targets.as_slice() {
            [] => return,
            [(one, _)] => (
                format!(
                    "Stop the screen “{}”? The agent’s programs on it are closed, and anything it hasn’t saved is lost. The screen is kept, with its history: Start Screen starts it again.",
                    shown(one)
                ),
                "Stop Screen".to_owned(),
            ),
            many => (
                format!(
                    "Stop {} screens? The agents’ programs on them are closed, and anything they haven’t saved is lost. The screens are kept, with their histories: Start starts them again.",
                    many.len()
                ),
                format!("Stop {} Screens", many.len()),
            ),
        };
        let me = cx.view_ref();
        open_alert(cx, "Stop Screen", "warning", &message, &["Cancel", &button], move |choice, cx| {
            if *choice != Some(1) {
                return;
            }
            let targets = targets.clone();
            me.update(cx, move |v, cx| {
                let controlled_src = if v.selected.as_ref().is_some_and(|s| targets.iter().any(|(n, _)| n == s)) {
                    v.pane.update(cx, |p, cx| p.stop_controlling(cx));
                    v.source.clone()
                } else {
                    None
                };
                let (proxy, targets) = (v.proxy.clone(), targets.clone());
                std::thread::spawn(move || {
                    if let Some(src) = controlled_src {
                        let _ = src.release_control_and_wait(Vec::new());
                    }
                    let failed: Vec<String> = targets.iter().filter_map(|(n, started)| screens::stop_if_generation(n, Some(*started)).err()).collect();
                    proxy.send(Msg::Stopped(if failed.is_empty() { Ok(()) } else { Err(failed.join("\n")) }));
                });
                cx.invalidate();
            });
        });
    }

    fn told(&mut self, message: Box<dyn std::any::Any + Send>, cx: &mut Cx<Self>) {
        // Another start: `olo-screen [watch NAME]`.
        if let Some(launch) = message.downcast_ref::<olo_kit::instance::Launch>() {
            self.show(launch.args.get(1).cloned(), cx);
            return;
        }
        let Ok(msg) = message.downcast::<Msg>() else { return };
        match *msg {
            Msg::Screens(seq, list) => {
                if !accept_list_seq(&mut self.applied_seq, seq, true) {
                    return;
                }
                self.screens = list;
                self.refresh_rows(seq);
            }
            Msg::Rows(seq, rows) => {
                if !accept_list_seq(&mut self.applied_seq, seq, false) {
                    return;
                }
                self.rows = rows;
                self.follow_use();
                self.keep_choice_listed(cx);
            }
            Msg::Refreshed(seq, screens, rows) => {
                if !accept_list_seq(&mut self.applied_seq, seq, true) {
                    return;
                }
                self.screens = screens;
                self.rows = rows;
                self.follow_use();
                self.keep_choice_listed(cx);
            }
            Msg::Used(name, at) => {
                if let Some(r) = self.rows.iter_mut().find(|r| r.name == name)
                    && r.last_used.is_none_or(|t| t < at)
                {
                    r.last_used = Some(at);
                }
            }
            Msg::Changed(failed) => {
                // (Shown at once; the list read again, to show what is.)
                if !failed.is_empty() {
                    self.alert(&failed.join("\n"), cx);
                }
                self.refresh_all();
            }
            Msg::Started(r) => {
                self.set_busy(false, cx);
                match r {
                    Ok(screen) => {
                        if !self.screens.contains(&screen) {
                            self.screens.push(screen.clone());
                        }
                        self.select(Some(screen.name), cx);
                        self.pane.update(cx, |p, cx| p.set_tab(Tab::Live, cx));
                    }
                    Err(e) => self.alert(&format!("The screen couldn't be started: {e}"), cx),
                }
            }
            Msg::Stopped(r) => {
                if let Err(e) = r {
                    self.alert(&e, cx);
                }
                self.refresh_all();
            }
        }
        // A screen come or gone changes what Live shows.
        self.sync_pane(cx);
        cx.invalidate();
    }

    fn menus(&self, cx: &WindowCx) -> Vec<Menu> {
        let st = self.pane.read(cx).menu_state(cx);
        let running = st.running;
        let chosen = self.selected.is_some();
        let tab = st.shown_tab;
        let history = st.has_history;
        let playing = st.playing;
        // (Its settings: AirPlay's receivers, and Terminal's, which gives
        // agents their screens.)
        let screen = self.chosen_items(
            Menu::new("Screen").app_items(&["airplay", "terminal"]).item("New Screen", "new").key("n").enabled(!self.busy).item("Passwords for Every Screen…", "shares-everywhere").separator(),
        );
        vec![
            screen
                .item("Clear History…", "clear")
                .enabled(chosen && !running)
                .separator()
                .item("Export Video…", "export")
                .key("e")
                .enabled(st.can_export)
                .item("Take Screenshot…", "screenshot")
                .shift_key("e")
                .enabled(st.can_screenshot)
                .close_quit(),
            Menu::new("Edit")
                .item("Select All Screens", "select-all")
                .enabled(!self.rows.is_empty())
                .item("Deselect All Screens", "deselect")
                .enabled(!self.also.is_empty())
                .separator()
                .item("Find…", "find")
                .key("f")
                .enabled(chosen),
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
                .item("Show Archived", "show-archived")
                .checked(self.show_archived),
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
            {
                let control = Menu::new("Control").item(if st.controlling { "Stop Controlling" } else { "Take Control" }, "control").key("k").enabled(st.can_control);
                // (From the person's own display: a viewer on a screen has
                // no one to give them to.)
                if olo_kit::moving::menus_offer() { control.separator().submenu(self.pane.read(cx).give_menu()) } else { control }
            },
        ]
    }

    fn run(&mut self, command: &str, cx: &mut Cx<Self>) {
        if let [one] = self.chosen().as_slice()
            && olo_kit::airplay::show_from_menu(command, &olo_protocol::airplay::Shown::Screen { name: one.clone() })
        {
            return;
        }
        match command {
            "new" => self.new_screen(cx),
            "shares-everywhere" => shares::open_everywhere(&mut self.shares_everywhere),
            "archive" => self.archive(true, cx),
            "start-stopped" => self.start_chosen(cx),
            "unarchive" => self.archive(false, cx),
            "delete" => self.ask_delete(cx),
            "select-all" => self.select_all(cx),
            "deselect" => self.also.clear(),
            "show-archived" => {
                self.show_archived = !self.show_archived;
                self.keep_choice_listed(cx);
            }
            "stop" => self.ask_stop(cx),
            "close" => cx.close_window(),
            "quit" => self.quit(cx),
            other => {
                self.pane.update(cx, |p, cx| {
                    p.run_command(other, cx);
                });
            }
        }
        cx.invalidate();
    }

    /// The list's sections, as the sidebar shows them: running, stopped,
    /// and (with View ▸ Show Archived) archived.
    fn groups(&self) -> Vec<(&'static str, Vec<&Row>)> {
        let running = |n: &str| self.screens.iter().any(|s| s.name == n);
        let mut groups = vec![("Running", self.rows.iter().filter(|r| running(&r.name)).collect()), ("Stopped", self.rows.iter().filter(|r| !running(&r.name) && !r.archived).collect())];
        if self.show_archived {
            groups.push(("Archived", self.rows.iter().filter(|r| !running(&r.name) && r.archived).collect()));
        }
        groups
    }

    /// The screens listed, in the list's order.
    fn listed_names(&self) -> Vec<String> {
        self.groups().into_iter().flat_map(|(_, rs)| rs.into_iter().map(|r| r.name.clone())).collect()
    }

    /// The screens chosen: the one shown and those chosen with it, in the
    /// list's order.
    fn chosen(&self) -> Vec<String> {
        self.listed_names().into_iter().filter(|n| self.selected.as_ref() == Some(n) || self.also.contains(n)).collect()
    }

    /// A ⌘-click (`run`: a ⇧-click) on `name`: it chosen too, or not; or
    /// every screen from where the last began to it.
    fn pick(&mut self, name: String, run: bool, cx: &mut Cx<Self>) {
        if run {
            let order = self.listed_names();
            let from = self.anchor.clone().or_else(|| self.selected.clone()).and_then(|f| order.iter().position(|n| *n == f));
            if let Some(to) = order.iter().position(|n| *n == name) {
                let from = from.unwrap_or(to);
                self.also = order[from.min(to)..=from.max(to)].iter().filter(|n| self.selected.as_ref() != Some(*n)).cloned().collect();
            }
        } else if self.selected.as_ref() != Some(&name) {
            match self.also.iter().position(|n| *n == name) {
                Some(i) => {
                    self.also.remove(i);
                }
                None => self.also.push(name.clone()),
            }
            self.anchor = Some(name);
        }
        cx.invalidate();
    }

    /// Every screen listed chosen (⌘A in the list, Edit ▸ Select All
    /// Screens).
    fn select_all(&mut self, cx: &mut Cx<Self>) {
        self.also = self.listed_names().into_iter().filter(|n| self.selected.as_ref() != Some(n)).collect();
        cx.invalidate();
    }

    /// A screen's menu (a secondary click): for every screen chosen if it
    /// is one of them, else for it alone (then chosen).
    fn row_menu(&mut self, name: String, at: Point<Pixels>, cx: &mut Cx<Self>) {
        if !self.chosen().contains(&name) {
            self.also.clear();
            self.anchor = Some(name.clone());
            self.select(Some(name), cx);
        }
        let menu = self.chosen_items(Menu::new(""));
        self.menu_at(menu, at, cx);
        cx.invalidate();
    }

    /// `menu` at `at`: what's chosen from it is done here.
    fn menu_at(&mut self, menu: Menu, at: Point<Pixels>, cx: &mut Cx<Self>) {
        let me = cx.view_ref();
        olo_kit::menu::context_menu(cx, menu, at, move |cmd, cx| {
            let cmd = cmd.to_owned();
            me.update(cx, move |v, cx| v.run(&cmd, cx));
        });
    }

    /// What can be done to the screens chosen, as menu items: each says
    /// how many it will act on.
    fn chosen_items(&self, menu: Menu) -> Menu {
        let chosen = self.chosen();
        let running = |n: &String| self.screens.iter().any(|s| &s.name == n);
        let archived = |n: &String| self.rows.iter().any(|r| &r.name == n && r.archived);
        let count = |f: &dyn Fn(&String) -> bool| chosen.iter().filter(|n| f(n)).count();
        let (stop, archive, unarchive, delete) = (count(&running), count(&|n| !running(n) && !archived(n)), count(&archived), count(&|n| !running(n)));
        let say = |verb: &str, k: usize, dots: &str| if k > 1 { format!("{verb} {k} Screens{dots}") } else { format!("{verb} Screen{dots}") };
        // One running, shown live on a television (Ben, 2026-11-12:
        // "broadcast/stream … an agent screen … to an airplay compatible
        // display"): `olo-airplay show --screen`.
        let menu = match chosen.as_slice() {
            [one] if running(one) => menu.submenu(Menu::airplay("Show on AirPlay", &olo_kit::airplay::paired(), "")).separator(),
            _ => menu,
        };
        menu.item("Rename…", "rename")
            .enabled(chosen.len() == 1)
            .separator()
            .item(say("Start", delete, ""), "start-stopped")
            .enabled(delete > 0 && !self.busy)
            .item(say("Archive", archive, ""), "archive")
            .enabled(archive > 0)
            .item(say("Unarchive", unarchive, ""), "unarchive")
            .enabled(unarchive > 0)
            .item(say("Stop", stop, "…"), "stop")
            .enabled(stop > 0)
            .separator()
            .item(say("Delete", delete, "…"), "delete")
            .enabled(delete > 0)
    }

    /// The stopped screens chosen archived (`on`) or brought back.
    fn archive(&mut self, on: bool, cx: &mut Cx<Self>) {
        let running: Vec<String> = self.screens.iter().map(|s| s.name.clone()).collect();
        let names: Vec<String> = self.chosen().into_iter().filter(|n| !running.contains(n) && self.rows.iter().any(|r| &r.name == n && r.archived != on)).collect();
        for r in self.rows.iter_mut().filter(|r| names.contains(&r.name)) {
            r.archived = on;
        }
        let proxy = self.proxy.clone();
        std::thread::spawn(move || proxy.send(Msg::Changed(names.iter().filter_map(|n| history::set_archived(n, on).err()).collect())));
        self.keep_choice_listed(cx);
    }

    /// Ask, once for all, before the stopped screens chosen go to the
    /// Trash with their histories, recordings and stats.
    fn ask_delete(&mut self, cx: &mut Cx<Self>) {
        let running: Vec<String> = self.screens.iter().map(|s| s.name.clone()).collect();
        let names: Vec<String> = self.chosen().into_iter().filter(|n| !running.contains(n)).collect();
        let shown = |n: &String| self.rows.iter().find(|r| &r.name == n).map_or(n.as_str(), Row::shown).to_owned();
        let (message, button) = match names.as_slice() {
            [] => return,
            [one] => (format!("Delete the screen “{}”? Its history, recordings and stats go to the Trash.", shown(one)), "Delete Screen".to_owned()),
            many => (format!("Delete {} screens? Their histories, recordings and stats go to the Trash.", many.len()), format!("Delete {} Screens", many.len())),
        };
        let me = cx.view_ref();
        open_alert(cx, "Delete", "warning", &message, &["Cancel", &button], move |choice, cx| {
            if *choice != Some(1) {
                return;
            }
            let names = names.clone();
            me.update(cx, move |v, cx| {
                let (proxy, names) = (v.proxy.clone(), names.clone());
                std::thread::spawn(move || proxy.send(Msg::Changed(names.iter().filter_map(|n| screens::delete_if_stopped(n).err()).collect())));
                cx.invalidate();
            });
        });
    }

    /// What's chosen kept to what's listed (a screen archived out of
    /// sight, deleted): the first listed shown if the one shown went, and
    /// detached cleanly when no rows remain visible.
    fn keep_choice_listed(&mut self, cx: &mut Cx<Self>) {
        let listed = self.listed_names();
        self.also.retain(|n| listed.contains(n));
        if self.selected.as_ref().is_none_or(|n| !listed.contains(n)) {
            let first = listed.first().cloned();
            self.pane.update(cx, |p, cx| p.stop_controlling(cx));
            self.selected = first;
            self.sync_pane(cx);
        } else {
            self.sync_pane(cx);
        }
        cx.invalidate();
    }

    /// Every listed screen's history followed as it grows (its folder
    /// watched: a file's time read as an action is added, nothing read
    /// whole), each row's last use said as it changes.
    fn follow_use(&mut self) {
        let dirs: Vec<std::path::PathBuf> = self.rows.iter().filter_map(|r| history::dir_checked(&r.name).ok()).collect();
        let proxy = self.proxy.clone();
        self.used_watch = olo_kit::watch::watch(dirs, move |changed| {
            for path in changed.iter().filter(|p| p.file_name().is_some_and(|n| n == "actions.jsonl")) {
                let Some(name) = path.parent().and_then(|d| d.file_name()).and_then(|n| n.to_str()) else { continue };
                let Some(at) = std::fs::metadata(path).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()) else { continue };
                proxy.send(Msg::Used(name.to_owned(), at.as_millis() as u64));
            }
        });
    }

    /// The screens: running, then stopped ones with a history; New
    /// Screen under them.
    fn sidebar(&mut self, side_w: f32, cx: &mut Cx<Self>) -> Element {
        // The chosen one's last use as it happens; the others' as their
        // histories grow (`follow_use`).
        let latest = self.pane.read(cx).latest_used(cx).map(|(id, t)| (id.name, t));
        let last_used = |r: &Row| match &latest {
            Some((n, t)) if *n == r.name => Some(*t),
            _ => r.last_used,
        };
        let groups = self.groups();
        let mut items = Vec::new();
        let mut names: Vec<Option<String>> = Vec::new();
        for (heading, rs) in &groups {
            if rs.is_empty() {
                continue;
            }
            items.push(sidebar::Row::heading(*heading));
            names.push(None);
            for r in rs {
                let row = sidebar::Row::new("agent-screens", r.shown().to_owned());
                items.push(match last_used(r) {
                    Some(t) => row.value(olo_kit::ago::ago_ms(t, olo_kit::ago::short)),
                    None => row,
                });
                names.push(Some(r.name.clone()));
            }
        }
        let first = names.iter().position(Option::is_some).unwrap_or(0);
        let chosen = names.iter().position(|n| n.as_deref() == self.selected.as_deref()).unwrap_or(first);
        let also: Vec<usize> = names.iter().enumerate().filter(|(_, n)| n.as_ref().is_some_and(|n| self.also.contains(n))).map(|(i, _)| i).collect();
        let names = std::rc::Rc::new(names);
        let (n1, n2, n3) = (names.clone(), names.clone(), names.clone());
        let list = Sidebar::new("screens", items, chosen)
            .also(also)
            .on_choose(cx.listener(move |v: &mut Viewer, i: &usize, cx| {
                if let Some(Some(name)) = n1.get(*i) {
                    // (One clicked alone: it alone is chosen.)
                    v.also.clear();
                    v.anchor = Some(name.clone());
                    v.select(Some(name.clone()), cx);
                    cx.invalidate();
                }
            }))
            .on_pick(cx.listener(move |v: &mut Viewer, (i, mods): &(usize, Modifiers), cx| {
                if let Some(Some(name)) = n2.get(*i) {
                    v.pick(name.clone(), mods.shift, cx);
                }
            }))
            .on_menu(cx.listener(move |v: &mut Viewer, (i, at): &(usize, Point<Pixels>), cx| {
                if let Some(Some(name)) = n3.get(*i) {
                    v.row_menu(name.clone(), *at, cx);
                }
            }))
            .on_select_all(cx.listener(|v: &mut Viewer, _: &(), cx| v.select_all(cx)))
            .build(cx);
        // The sidebar's ground down the column, and the line over New
        // Screen, from the column's own box (its two parts are laid out
        // a device pixel apart at some scales).
        let ground = canvas(|g, b| {
            let p = theme::palette();
            style::fill_rect(g, b, sidebar::ground(&p));
            style::vline_left_of(g, b.right(), b.top(), b.bottom(), sidebar::edge(&p));
            let top = b.bottom() - px(FOOTER_H);
            style::hline(g, b.left(), b.right() - style::hairline(g), top, style::shade(&p, p.panel_background, 1.18));
        })
        .absolute()
        .inset_0();
        let busy = self.busy;
        let new = Button::new(if busy { "Starting…" } else { "New Screen" }).id("new-screen").enabled(!busy).on_click(cx.listener(|v: &mut Viewer, _, cx| v.new_screen(cx)));
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
            .into_any_element()
    }
}

impl View for Viewer {
    type Message = Box<dyn std::any::Any + Send>;

    fn message(&mut self, message: Self::Message, cx: &mut Cx<Self>) {
        self.told(message, cx);
    }

    fn command(&mut self, command: &str, cx: &mut Cx<Self>) -> bool {
        self.run(command, cx);
        true
    }

    /// Closing it ends the app (and any control of a screen).
    fn close_requested(&mut self, cx: &mut Cx<Self>) -> bool {
        self.quit(cx);
        true
    }

    fn save_state(&mut self, cx: &mut Cx<Self>) -> Option<String> {
        Some(self.saved(cx))
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let bar = MenuBar::of(self, cx, |v, cx| v.menus(cx));
        let root = div().size_full().flex().flex_col().child(bar);
        if self.rows.is_empty() {
            let busy = self.busy;
            let empty = EmptyState::new("No agent screens yet. An agent starts one with “olo-screen start”, or start one yourself.")
                .icon("agent-screens")
                .action(if busy { "Starting…" } else { "New Screen" }, cx.listener(|v: &mut Viewer, _, cx| v.new_screen(cx)));
            return root.child(div().flex_1().min_h_0().p(px(m::WINDOW_INSET)).child(empty)).into_any_element();
        }
        let full_w = f32::from(cx.viewport_size().width);
        let side_w = (full_w * 0.2).round().clamp(SIDEBAR_MIN_W, SIDEBAR_W);
        let sidebar = self.sidebar(side_w, cx);
        if self.selected.is_none() {
            return root.child(div().flex_1().min_h_0().flex().flex_row().child(sidebar)).into_any_element();
        }
        let pane_w = full_w - side_w;
        self.pane.update(cx, |p, cx| p.set_width(pane_w, cx));
        // (Times said move on by themselves: `olo_kit::ago`.)
        root.child(div().flex_1().min_h_0().flex().flex_row().child(sidebar).child(div().flex_1().min_w_0().h_full().child(&self.pane))).into_any_element()
    }
}

/// Sequence gate for `Msg::Screens`, `Msg::Rows`, and `Msg::Refreshed` so an
/// older worker snapshot (`seq < applied`) can never overwrite a newer list.
fn accept_list_seq(applied: &mut u64, seq: u64, advance: bool) -> bool {
    if seq < *applied {
        return false;
    }
    if advance {
        *applied = seq;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review2_f53_stale_refreshed_cannot_overwrite_newer_screens() {
        let mut applied = 0u64;
        // Newer Msg::Screens(2) arrives before slow Msg::Refreshed(1).
        assert!(accept_list_seq(&mut applied, 2, true));
        assert_eq!(applied, 2);
        assert!(!accept_list_seq(&mut applied, 1, true), "older Refreshed(1) must be discarded");
        assert!(!accept_list_seq(&mut applied, 1, false), "older Rows(1) must be discarded");
        // Matching Rows(2) for Screens(2) is accepted without regressing applied_seq.
        assert!(accept_list_seq(&mut applied, 2, false));
        // Newer Refreshed(3) advances the watermark.
        assert!(accept_list_seq(&mut applied, 3, true));
        assert_eq!(applied, 3);
        assert!(!accept_list_seq(&mut applied, 2, false));
    }

    #[test]
    fn review2_f52_pidfd_wakes_immediately_on_process_exit() {
        let mut child = std::process::Command::new("/usr/bin/sleep").arg("60").spawn().expect("spawn sleep");
        let pfd = open_pidfd(child.id() as i32).expect("pidfd_open");
        let mut p = libc::pollfd { fd: pfd.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        // Alive right now: 0ms poll reports not ready.
        assert_eq!(unsafe { libc::poll(&mut p, 1, 0) }, 0);
        let _ = child.kill();
        let _ = child.wait();
        // Exited: poll reports POLLIN immediately without any timer sleep.
        let rc = unsafe { libc::poll(&mut p, 1, 500) };
        assert!(rc > 0 && (p.revents & libc::POLLIN) != 0, "pidfd must wake immediately on exit: rc={rc} revents={}", p.revents);
    }
}
