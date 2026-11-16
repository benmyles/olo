//! Kache: the build cache, watched and run from Olo (Ben, 2026-11-15: "an
//! olo native kache frontend app that lets me monitor the status of
//! kache, see stats/metrics/charts etc, configure/tweak settings, manage
//! it like restart or whatever, and anything else useful to know or do").
//!
//! Three sections down the left:
//!
//! - **Status**: the service (running since when, its process and
//!   memory; Restart, Stop or Start), the store (what it holds against
//!   its limit, what's on disk, what builds' target folders share and so
//!   can't be cleaned, the last clean-up; Clean Up Now), the day's builds
//!   (hit rate of what could be cached, hits, misses, what couldn't be,
//!   time saved), what's compiling now, and the last 24 hours by the hour.
//! - **Builds**: the latest builds, each its time, crate, project,
//!   result, how long and how big.
//! - **Settings**: kache's own, in its file (`data::SETTINGS`), saved
//!   keeping the file's comments; Restart to have kache take them.
//!
//! Read on a worker (`data::read`) as it starts and whenever kache's
//! records change (its build events, its last clean-up, its daemon
//! coming and going, its settings: an inotify watch, a look at most every two
//! seconds while builds go on), and after anything done here: nothing runs
//! while nothing changes. Everything's on its menus (agents: by
//! command); `olo-kache --print` says it all as JSON, with no window;
//! `olo-kache --glance` serves its Deskbar widget (`glance.rs`);
//! `--start`, `--restart` and `--stop` act on the service with no window
//! (Doctor's fix), and `--clean-up` cleans up the store.

mod data;
mod glance;
mod held;

use std::{rc::Rc, sync::Mutex, time::Duration};

use olo_kit::{
    ClickEvent, Cx, Element, FontWeight, IntoElement as _, Menu, MenuBar, ParentElement as _, Styled as _, View, WindowOptions, canvas, div,
    instance::{self, Launch},
    metrics as m, px, style, theme,
    widgets::{
        Button, CheckBox, Column, ColumnListView, EmptyState, ListModel, StringView, button_row,
        dashboard::{Card, Facts},
        sidebar::{Row, Sidebar},
    },
};
use serde::{Deserialize, Serialize};

pub const APP_ID: &str = "olo.kache";
const SIDEBAR: f32 = 160.0;
const MIN_WIDTH: f32 = 860.0;
const MIN_HEIGHT: f32 = 560.0;
/// The least time between two looks while builds go on.
const LOOK_EVERY: Duration = Duration::from_secs(2);
/// Status's Now card.
const NOW_CARD: f32 = 240.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
enum Section {
    #[default]
    Status,
    Builds,
    Settings,
}

impl Section {
    const ALL: [Section; 3] = [Section::Status, Section::Builds, Section::Settings];
    fn name(self) -> &'static str {
        match self {
            Section::Status => "Status",
            Section::Builds => "Builds",
            Section::Settings => "Settings",
        }
    }
    fn icon(self) -> &'static str {
        match self {
            Section::Status => "activity",
            Section::Builds => "terminal",
            Section::Settings => "preferences",
        }
    }
    fn command(self) -> &'static str {
        match self {
            Section::Status => "show-status",
            Section::Builds => "show-builds",
            Section::Settings => "show-settings",
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
struct Saved {
    section: Section,
}

pub enum Msg {
    /// Kache's records changed: look again (soon, if a look is going on).
    Changed,
    Look,
    Read(Box<data::Snapshot>),
    /// The service after a restart, start or stop (or why not).
    Service(Result<olo_system::systemd::UserUnit, String>),
    CleanedUp(Result<String, String>),
}

struct Kache {
    section: Section,
    snap: Rc<data::Snapshot>,
    /// A look going on, and another wanted after it.
    looking: bool,
    again: bool,
    /// What's being done (a restart, a clean-up), and the last word.
    doing: Option<&'static str>,
    note: Option<(String, bool)>,
    /// Settings: a field each for those typed, a switch each for the
    /// others, as in the file now.
    /// Each stepped setting's steps (`data::steps`) and the one chosen;
    /// each switch's state.
    steps: Vec<Vec<(Option<String>, String)>>,
    chosen: Vec<usize>,
    switches: Vec<bool>,
    saved_values: Vec<Option<String>>,
    builds: ListModel,
    rows: Rc<Vec<[String; 6]>>,
    /// Status's and Settings' scroll.
    status_scroll: olo_kit::ScrollHandle,
    settings_scroll: olo_kit::ScrollHandle,
    _watch: Option<olo_kit::watch::Watch>,
}

impl Kache {
    fn new(cx: &mut Cx<Self>) -> Kache {
        let saved: Saved = olo_kit::restored_state().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        let values = data::settings();
        let processors = std::thread::available_parallelism().map_or(1, |n| n.get() as u32);
        let steps: Vec<Vec<(Option<String>, String)>> =
            data::SETTINGS.iter().zip(&values).map(|(s, v)| if s.kind == data::Kind::Switch { Vec::new() } else { data::steps(s, v.as_deref(), processors) }).collect();
        let chosen = data::SETTINGS.iter().zip(&values).zip(&steps).map(|((s, v), st)| data::step_of(st, v.as_deref(), s.kind == data::Kind::Size)).collect();
        let switches = data::SETTINGS.iter().zip(&values).map(|(s, v)| data::switched(s, v.as_deref())).collect();
        // Kache's records followed: what's written there is looked at.
        // (Its daemon's socket, not its state file: that's written every
        // five seconds, a heartbeat; the socket goes and comes as the
        // daemon stops and starts.)
        let dir = data::cache_dir();
        let watched = if data::demo() { Vec::new() } else { vec![dir.join("events.jsonl"), dir.join("gc_stats.json"), data::socket_path(), data::config_path()] };
        let to = Mutex::new(cx.messenger());
        let watch = (!watched.is_empty())
            .then(|| {
                olo_kit::watch::watch(watched, move |_| {
                    to.lock().unwrap_or_else(|e| e.into_inner()).send(Msg::Changed);
                })
            })
            .flatten();
        let mut k = Kache {
            section: saved.section,
            snap: Rc::default(),
            looking: false,
            again: false,
            doing: None,
            note: None,
            steps,
            chosen,
            switches,
            saved_values: values,
            builds: ListModel::new(false),
            rows: Rc::default(),
            status_scroll: olo_kit::ScrollHandle::new(),
            settings_scroll: olo_kit::ScrollHandle::new(),
            _watch: watch,
        };
        k.look(cx);
        k
    }

    /// Read everything again, on a worker.
    fn look(&mut self, cx: &mut Cx<Self>) {
        if self.looking {
            self.again = true;
            return;
        }
        self.looking = true;
        cx.work(|| Box::new(data::read()), Msg::Read);
    }

    fn say(&mut self, text: impl Into<String>, failed: bool) {
        self.note = Some((text.into(), failed));
    }

    fn service(&mut self, method: &'static str, cx: &mut Cx<Self>) {
        if self.doing.is_some() {
            return;
        }
        self.doing = Some(match method {
            "StopUnit" => "Stopping kache…",
            "StartUnit" => "Starting kache…",
            _ => "Restarting kache…",
        });
        cx.work(move || data::service(method), Msg::Service);
        cx.invalidate();
    }

    fn clean_up(&mut self, cx: &mut Cx<Self>) {
        if self.doing.is_some() {
            return;
        }
        self.doing = Some("Cleaning up the store…");
        cx.work(data::clean_up, Msg::CleanedUp);
        cx.invalidate();
    }

    /// The settings as edited.
    fn values(&self) -> Vec<Option<String>> {
        data::SETTINGS
            .iter()
            .enumerate()
            .map(|(i, s)| match s.kind {
                data::Kind::Switch if self.saved_values[i].is_none() && self.switches[i] == data::switched(s, None) => None,
                // (A switch as kache has it unset, and never set: left out.)
                data::Kind::Switch => Some(self.switches[i].to_string()),
                _ => self.steps[i].get(self.chosen[i]).and_then(|(v, _)| v.clone()),
            })
            .collect()
    }

    fn save_settings(&mut self, cx: &mut Cx<Self>) {
        let values = self.values();
        if data::demo() {
            self.saved_values = values;
            self.say("Saved (made up). Restart kache for it to take them.", false);
            return cx.invalidate();
        }
        match data::save_settings(&values) {
            Ok(()) => {
                self.saved_values = values;
                self.say("Saved. kache takes them as it starts again: Restart.", false);
            }
            Err(e) => self.say(e, true),
        }
        cx.invalidate();
    }

    fn revert_settings(&mut self, cx: &mut Cx<Self>) {
        let values = self.saved_values.clone();
        for (i, (s, v)) in data::SETTINGS.iter().zip(&values).enumerate() {
            if s.kind == data::Kind::Switch {
                self.switches[i] = data::switched(s, v.as_deref());
            } else {
                self.chosen[i] = data::step_of(&self.steps[i], v.as_deref(), s.kind == data::Kind::Size);
            }
        }
        self.note = None;
        cx.invalidate();
    }

    fn run(&mut self, command: &str, cx: &mut Cx<Self>) -> bool {
        match command {
            "refresh" => self.look(cx),
            "restart" => self.service("RestartUnit", cx),
            "stop" => self.service("StopUnit", cx),
            "start" => self.service("StartUnit", cx),
            "clean-up" => self.clean_up(cx),
            "save-settings" => self.save_settings(cx),
            "revert-settings" => self.revert_settings(cx),
            c if c.starts_with("end-test:") => {
                if let Ok(pid) = c["end-test:".len()..].parse::<i32>() {
                    self.end_test(pid, cx);
                }
            }
            "open-log" => {
                let log = data::cache_dir().join("daemon.log");
                if let Err(e) = olo_xdg::spawn_detached(&format!("olo-logs '{}'", log.display().to_string().replace('\'', "'\\''"))) {
                    self.say(format!("The log can't be opened: {e}"), true);
                }
            }
            "close" | "quit" => olo_kit::quit(),
            c => match Section::ALL.iter().find(|s| s.command() == c) {
                Some(s) => self.section = *s,
                None => return false,
            },
        }
        cx.invalidate();
        true
    }

    /// End a test holding slots (on a worker: it's looked for again
    /// first).
    fn end_test(&mut self, pid: i32, cx: &mut Cx<Self>) {
        if data::demo() {
            self.say(format!("Ended the test (process {pid}, made up)."), false);
            return cx.invalidate();
        }
        let dir = data::cache_dir();
        cx.work(move || held::end(&dir, pid).map(|()| format!("Ended the test (process {pid}): its slots go back to the pool.")), Msg::CleanedUp);
    }

    fn running(&self) -> Option<bool> {
        self.snap.unit.as_ref().map(|u| u.running())
    }

    fn menus(&self) -> Vec<Menu> {
        let busy = self.doing.is_some();
        let running = self.running();
        let mut view = Menu::new("View");
        for s in Section::ALL {
            view = view.item(s.name(), s.command()).checked(self.section == s);
        }
        vec![
            Menu::new("File").app_items(&[]).item("Refresh", "refresh").key("r").separator().item("Open kache's Log", "open-log").close_quit(),
            Menu::new("Kache")
                .item("Restart", "restart")
                .enabled(!busy && running.is_some())
                .item("Stop", "stop")
                .enabled(!busy && running == Some(true))
                .item("Start", "start")
                .enabled(!busy && running == Some(false))
                .separator()
                .item("Clean Up Now", "clean-up")
                .enabled(!busy && self.snap.daemon.is_some()),
            view,
        ]
    }

    fn sidebar(&mut self, cx: &mut Cx<Self>) -> Element {
        let mut rows = vec![Row::heading("Kache")];
        for s in Section::ALL {
            let row = Row::new(s.icon(), s.name());
            rows.push(match s {
                Section::Status => match self.running() {
                    Some(true) => row.value("on"),
                    Some(false) => row.value("off"),
                    None => row,
                },
                Section::Builds => match self.snap.day.rate() {
                    Some(r) => row.value(format!("{}%", (r * 100.0).round())),
                    None => row,
                },
                _ => row,
            });
        }
        let chosen = Section::ALL.iter().position(|s| *s == self.section).unwrap_or(0) + 1;
        Sidebar::new("kache-sections", rows, chosen)
            .bare()
            .on_choose(cx.listener(|v: &mut Kache, i: &usize, cx| {
                if let Some(s) = i.checked_sub(1).and_then(|i| Section::ALL.get(i)) {
                    v.section = *s;
                    cx.invalidate();
                }
            }))
            .build(cx)
    }

    /// What's being done, or the last word, under a section.
    fn status_line(&self) -> Option<Element> {
        let (text, failed) = match (&self.doing, &self.note) {
            (Some(d), _) => ((*d).to_owned(), false),
            (None, Some((t, f))) => (t.clone(), *f),
            (None, None) => return None,
        };
        let p = theme::palette();
        let mut s = StringView::new(text).wrap(true);
        s = if failed { s.color(style::c(p.failure)) } else { s.dim(true) };
        Some(s.into_element())
    }

    fn status(&mut self, cx: &mut Cx<Self>) -> Element {
        let snap = self.snap.clone();
        let busy = self.doing.is_some();
        let mut col = div().flex().flex_col().gap(px(m::SPACING));
        // The service.
        let (title, detail) = match &snap.unit {
            Some(u) if u.loaded && u.running() => {
                let since = olo_kit::ago::ago_ms(u.since_us / 1000, |a| olo_kit::ago::long(a).replace(" ago", ""));
                let version = snap.daemon.as_ref().map(|d| format!(" {}", d.version)).unwrap_or_default();
                let memory = u.memory.map(|b| format!(" · {}", data::bytes(b))).unwrap_or_default();
                (format!("kache{version} is running"), format!("For {since} · process {}{memory}", u.pid))
            }
            Some(u) if u.loaded => (format!("kache is {}", if u.active == "failed" { "stopped: it failed" } else { "stopped" }), "Builds go on without the cache until it starts.".to_owned()),
            Some(_) => ("kache isn't set up as a service".into(), format!("No {} for this account.", data::UNIT)),
            None => ("kache's service can't be asked".into(), "The user session's systemd didn't answer.".into()),
        };
        let running = self.running();
        let buttons = div()
            .flex()
            .gap(px(m::SPACING))
            .child(Button::new("Clean Up Now").enabled(!busy && snap.daemon.is_some()).on_click(cx.listener(|v: &mut Kache, _: &ClickEvent, cx| v.clean_up(cx))))
            .child(Button::new("Restart").enabled(!busy && running.is_some()).on_click(cx.listener(|v: &mut Kache, _: &ClickEvent, cx| v.service("RestartUnit", cx))))
            .child(if running == Some(true) {
                Button::new("Stop").enabled(!busy).on_click(cx.listener(|v: &mut Kache, _: &ClickEvent, cx| v.service("StopUnit", cx)))
            } else {
                Button::new("Start").enabled(!busy && running.is_some()).on_click(cx.listener(|v: &mut Kache, _: &ClickEvent, cx| v.service("StartUnit", cx)))
            });
        col = col.child(
            div()
                .flex()
                .items_center()
                .gap(px(m::SPACING))
                .child(div().flex_1().min_w_0().flex().flex_col().child(StringView::new(title).bold(true).size(style::LEAD_SIZE).cut(true)).child(StringView::new(detail).dim(true).cut(true)))
                .child(buttons),
        );
        // The store and the day, side by side.
        let store = match &snap.daemon {
            Some(d) => {
                let mut facts: Vec<(olo_kit::SharedString, olo_kit::SharedString)> =
                    vec![("On disk".into(), data::bytes(d.disk).into()), ("Entries".into(), style::thousands(d.entries as usize).into())];
                if let Some(c) = &snap.cleanup {
                    // (What builds' target folders still share: no clean-up
                    // takes it. Known only from a clean-up that looked.)
                    if let Some(held) = c.held {
                        facts.push(("Held by builds".into(), data::bytes(held).into()));
                    }
                    if c.at_ms > 0 {
                        let when = olo_kit::ago::ago_ms(c.at_ms, olo_kit::ago::long);
                        let freed = if c.freed > 0 { format!("{} freed", data::bytes(c.freed)) } else { "nothing freed".to_owned() };
                        facts.push(("Last clean-up".into(), format!("{when}, {freed}").into()));
                    }
                }
                let over = d.limit > 0 && d.size > d.limit;
                let caption = if d.limit > 0 { format!("of a {} limit{}", data::bytes(d.limit), if over { ": over it" } else { "" }) } else { String::new() };
                let mut card = Card::new("kache-store", "Store").figure(data::bytes(d.size)).caption(caption);
                if over {
                    card = card.figure_ink(style::c(theme::palette().failure));
                }
                card.child(Facts::new(facts))
            }
            None => Card::new("kache-store", "Store").figure("—").caption("kache's daemon isn't answering").child(Facts::new(Vec::new())),
        };
        let day = &snap.day;
        let rate = day.rate().map_or("—".to_owned(), |r| format!("{}%", (r * 100.0).round()));
        let mut facts: Vec<(olo_kit::SharedString, olo_kit::SharedString)> = vec![
            ("Hits".into(), style::thousands(day.hits as usize).into()),
            ("Misses".into(), style::thousands(day.misses as usize).into()),
            ("Couldn't be cached".into(), style::thousands(day.others as usize).into()),
        ];
        if day.saved_ms > 0 {
            facts.push(("Time saved".into(), format!("about {}", data::span(day.saved_ms)).into()));
        }
        let today = Card::new("kache-day", "The last 24 hours").figure(rate).caption("of what could be cached came from it").child(Facts::new(facts));
        col = col.child(div().flex().gap(px(m::SPACING)).child(div().flex_1().min_w_0().child(store)).child(div().flex_1().min_w_0().child(today)));
        // Now (the pool's slots, what's compiling), and the day by the hour.
        let compiling: Vec<String> = snap.daemon.as_ref().map(|d| d.in_flight.clone()).unwrap_or_default();
        let mut now: Vec<(olo_kit::SharedString, olo_kit::SharedString)> = Vec::new();
        let card = match &snap.pool {
            Some(p) => {
                now.push(("Compiles".into(), format!("{} slots", p.compiles).into()));
                now.push(("Tests".into(), format!("{} slots", p.tests).into()));
                now.push(("Waiting".into(), style::thousands(p.waiting as usize).into()));
                Card::new("kache-now", "Now").figure(format!("{} of {}", p.compiles + p.tests, p.size)).caption("slots of the pool in use")
            }
            None => Card::new("kache-now", "Now").figure(compiling.len().to_string()).caption(if compiling.len() == 1 { "compile going on" } else { "compiles going on" }),
        };
        if !compiling.is_empty() {
            let more = compiling.len().saturating_sub(2);
            let names = compiling.iter().take(2).cloned().collect::<Vec<_>>().join(", ");
            now.push(("Compiling".into(), if more > 0 { format!("{names} and {more} more") } else { names }.into()));
        }
        let hours =
            Card::new("kache-hours", "Builds by the hour").caption("the last 24 hours").child(div().flex().flex_col().gap(px(m::STACK)).child(hours_chart(&snap.day)).child(hours_legend(&snap.day)));
        col = col.child(div().flex().gap(px(m::SPACING)).child(div().w(px(NOW_CARD)).flex_shrink_0().child(card.child(Facts::new(now)))).child(div().flex_1().min_w_0().child(hours)));
        // Tests holding slots, each to end.
        if !snap.held.is_empty() {
            let mut list = div().flex().flex_col().gap(px(m::STACK));
            for h in &snap.held {
                let since = if h.since_ms > 0 { format!(", for {}", olo_kit::ago::ago_ms(h.since_ms, |a| olo_kit::ago::long(a).replace(" ago", ""))) } else { String::new() };
                let what = format!(
                    "{}{} · {} slot{}{since} · process {}",
                    h.name,
                    if h.project.is_empty() { String::new() } else { format!(" ({})", h.project) },
                    h.slots,
                    if h.slots == 1 { "" } else { "s" },
                    h.pid
                );
                let pid = h.pid;
                list = list.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(m::SPACING))
                        .child(div().flex_1().min_w_0().child(StringView::new(what).cut(true)))
                        .child(Button::new("End").id(format!("end-test-{pid}")).small().on_click(cx.listener(move |v: &mut Kache, _: &ClickEvent, cx| v.end_test(pid, cx)))),
                );
            }
            col = col.child(Card::new("kache-tests", "Tests holding slots").caption("ending one gives its slots back to the pool").child(list));
        }
        if let Some(line) = self.status_line() {
            col = col.child(line);
        }
        olo_kit::widgets::ScrollView::new(&self.status_scroll).border(false).auto_hide(true).child(col.pr(px(m::SPACING))).into_element()
    }

    /// The latest builds, each a row of the list: made as they're read.
    fn rows(recent: &[data::Event]) -> Vec<[String; 6]> {
        recent
            .iter()
            .map(|e| {
                let when = chrono::DateTime::from_timestamp_millis(e.at_ms as i64).map(|t| t.with_timezone(&chrono::Local).format("%H:%M:%S").to_string()).unwrap_or_default();
                let result = match e.result.as_str() {
                    "local_hit" => "Hit",
                    "remote_hit" => "Hit (remote)",
                    "miss" => "Miss",
                    "passthrough" => "Not cacheable",
                    "dup" => "Shared",
                    "error" => "Error",
                    other => other,
                };
                // (A miss says why: "Miss: its source changed".)
                let result = match e.cause {
                    Some(c) if e.result == "miss" => format!("{result}: {c}"),
                    _ => result.to_owned(),
                };
                [when, e.crate_name.clone(), e.project.clone(), result, data::span(e.elapsed_ms), if e.size > 0 { data::bytes(e.size) } else { String::new() }]
            })
            .collect()
    }

    fn builds(&mut self, cx: &mut Cx<Self>) -> Element {
        if self.rows.is_empty() {
            return EmptyState::new("No builds yet").detail("Each crate kache sees compiled is listed here as it's built.").icon("terminal").into_element();
        }
        // The day's projects, and its slowest compiles, over the list.
        let day = &self.snap.day;
        let projects: Vec<(olo_kit::SharedString, olo_kit::SharedString)> = day
            .projects
            .iter()
            .map(|t| {
                let n = t.hits + t.misses + t.others;
                let rate = if t.hits + t.misses > 0 { format!(", {}% hits", (t.hits as f64 * 100.0 / (t.hits + t.misses) as f64).round()) } else { String::new() };
                (t.name.clone().into(), format!("{} builds{rate}", style::thousands(n as usize)).into())
            })
            .collect();
        let slowest: Vec<(olo_kit::SharedString, olo_kit::SharedString)> = day.slowest.iter().map(|(c, ms)| (c.clone().into(), data::span(*ms).into())).collect();
        // Why the misses missed, most first.
        let causes: Vec<(olo_kit::SharedString, olo_kit::SharedString)> =
            day.causes.iter().take(data::TOP + 1).map(|(c, n)| (olo_kit::SharedString::from(sentence_case(c)), style::thousands(*n as usize).into())).collect();
        let cards = div()
            .flex()
            .flex_shrink_0()
            .gap(px(m::SPACING))
            .child(div().flex_1().min_w_0().child(Card::new("kache-projects", "Projects").caption("built most, the last 24 hours").child(Facts::new(projects))))
            .child(div().flex_1().min_w_0().child(Card::new("kache-slowest", "Slowest compiles").caption("misses, the last 24 hours").child(Facts::new(slowest))))
            .child(div().flex_1().min_w_0().child(Card::new("kache-causes", "Why misses missed").caption("the last 24 hours").child(Facts::new(causes))));
        let columns = vec![
            Column::new("When", 90.0),
            Column::new("Crate", 160.0),
            Column::new("Project", 110.0),
            Column::new("Result", 210.0),
            Column::new("Took", 80.0).right(),
            Column::new("Size", 80.0).right(),
        ];
        let rows = self.rows.clone();
        let n = rows.len();
        let list = ColumnListView::new(cx, "kache-builds", &self.builds, columns, n, move |_: &Kache, cell, g| {
            if let Some(r) = rows.get(cell.row) {
                cell.text(g, r[cell.column].clone());
            }
        });
        div().size_full().flex().flex_col().gap(px(m::SPACING)).child(cards).child(div().flex_1().min_h_0().child(list)).into_element()
    }

    fn settings(&mut self, cx: &mut Cx<Self>) -> Element {
        let mut col = div()
            .flex()
            .flex_col()
            .gap(px(m::SPACING))
            .child(StringView::new(format!("kache's own settings, in {}; kache takes them as it starts again.", data::config_path().display())).dim(true).wrap(true));
        // Each setting's block: a slider over its steps alone, the one
        // chosen said with its unit, what it is under it; a switch, its
        // check box. Then laid out in rows of two, the store's on the
        // left and the pool's on the right, each row's two starting level,
        // each switch in the block of the setting it qualifies.
        let mut blocks: Vec<Option<Element>> = Vec::new();
        for (i, s) in data::SETTINGS.iter().enumerate() {
            if s.kind == data::Kind::Switch {
                let on = self.switches[i];
                blocks.push(Some(
                    div()
                        .child(CheckBox::new(s.about, on).on_toggle(cx.listener(move |v: &mut Kache, on: &bool, cx| {
                            v.switches[i] = *on;
                            cx.invalidate();
                        })))
                        .into_element(),
                ));
                continue;
            }
            let steps = &self.steps[i];
            let at = self.chosen[i].min(steps.len().saturating_sub(1));
            let mut said = steps.get(at).map(|(_, n)| n.clone()).unwrap_or_default();
            // (kache's own store limit as it stands now, from its daemon.)
            if s.key == "local_max_size"
                && at == 0
                && let Some(d) = self.snap.daemon.as_ref().filter(|d| d.limit > 0)
            {
                said = format!("kache's own: {} now", data::bytes(d.limit));
            }
            let short = |n: &str| n.split(':').next().unwrap_or(n).to_owned();
            let slider = olo_kit::widgets::Slider::new(format!("kache-{}", s.key), at as f32, 0.0..=(steps.len().max(2) - 1) as f32)
                .step(1.0)
                .label(s.label)
                .value_text(said)
                .limits(steps.first().map_or(String::new(), |(_, n)| short(n)), steps.last().map_or(String::new(), |(_, n)| n.clone()))
                .on_change(cx.listener(move |v: &mut Kache, x: &f32, cx| {
                    let k = x.round().max(0.0) as usize;
                    if v.chosen[i] != k {
                        v.chosen[i] = k;
                        cx.invalidate();
                    }
                }));
            blocks.push(Some(div().flex().flex_col().gap(px(m::TIGHT)).child(slider).child(StringView::new(s.about).dim(true).small(true).wrap(true)).into_element()));
        }
        let at = |key: &str| data::SETTINGS.iter().position(|s| s.key == key);
        let mut take = |keys: &[&str]| -> Element {
            let mut cell = div().flex_1().min_w_0().flex().flex_col().gap(px(m::SPACING));
            for k in keys {
                if let Some(b) = at(k).and_then(|i| blocks.get_mut(i)).and_then(Option::take) {
                    cell = cell.child(b);
                }
            }
            cell.into_element()
        };
        let rows: [(&[&str], &[&str]); 4] = [
            (&["local_max_size", "count_held_in_max_size"], &["scheduler_permits"]),
            (&["gc_max_age_hours"], &["scheduler_test_slots"]),
            (&["auto_clean_idle_targets_days", "auto_clean_orphaned_targets"], &["scheduler_test_wait_secs"]),
            (&[], &["scheduler_dynamic", "scheduler_cpu_percent"]),
        ];
        let gap = m::SPACING * 3.0;
        col = col.child(
            div().flex().gap(px(gap)).child(div().flex_1().min_w_0().child(StringView::new("The store").bold(true))).child(div().flex_1().min_w_0().child(StringView::new("The pool").bold(true))),
        );
        for (left, right) in rows {
            let (l, r) = (take(left), take(right));
            col = col.child(div().flex().items_start().gap(px(gap)).child(l).child(r));
        }
        let buttons = button_row()
            .child(Button::new("Revert").on_click(cx.listener(|v: &mut Kache, _: &ClickEvent, cx| v.revert_settings(cx))))
            .child(Button::new("Save").default(true).on_click(cx.listener(|v: &mut Kache, _: &ClickEvent, cx| v.save_settings(cx))))
            .child(Button::new("Restart kache").enabled(self.doing.is_none() && self.running().is_some()).on_click(cx.listener(|v: &mut Kache, _: &ClickEvent, cx| v.service("RestartUnit", cx))));
        col = col.child(buttons);
        if let Some(line) = self.status_line() {
            col = col.child(line);
        }
        olo_kit::widgets::ScrollView::new(&self.settings_scroll).border(false).auto_hide(true).child(col.pr(px(m::SPACING))).into_element()
    }
}

/// "its source changed" → "Its source changed".
fn sentence_case(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

/// The last 24 hours, a bar an hour: hits, misses, and what couldn't be
/// cached, stacked; the hours under them every six.
fn hours_chart(day: &data::Day) -> Element {
    let hours = day.hours.clone();
    let from = day.from_ms;
    let key = style::key((&hours, from));
    canvas(move |g, b| {
        let p = theme::palette();
        let s = g.scale_factor();
        let label_h = 16.0;
        let chart = olo_kit::Bounds::from_corners(b.origin, olo_kit::point(b.right(), b.bottom() - px(label_h)));
        let most = hours.iter().map(|h| h[0] + h[1] + h[2]).max().unwrap_or(0).max(1) as f32;
        let n = hours.len().max(1) as f32;
        let w = f32::from(chart.size.width) / n;
        let colours = hour_colours(&p);
        style::hline(g, chart.left(), chart.right(), chart.bottom(), style::c(olo_theme::mix(p.panel_background, p.panel_text, 0.3)));
        let mut said = Vec::new();
        for (i, h) in hours.iter().enumerate() {
            let x0 = f32::from(chart.left()) + w * i as f32 + 1.0;
            let x1 = x0 + (w - 2.0).max(1.0);
            let mut y = f32::from(chart.bottom());
            for (k, count) in h.iter().enumerate() {
                if *count == 0 {
                    continue;
                }
                let hgt = (f32::from(chart.size.height) - 4.0) * *count as f32 / most;
                let r = style::snap(g, olo_kit::Bounds::from_corners(olo_kit::point(px(x0), px(y - hgt)), olo_kit::point(px(x1), px(y))));
                style::fill_rect(g, r, colours[k]);
                y -= hgt;
            }
            let at = from + i as u64 * 3_600_000;
            let hour = chrono::DateTime::from_timestamp_millis(at as i64).map(|t| t.with_timezone(&chrono::Local).format("%H").to_string()).unwrap_or_default();
            if i % 6 == 0 {
                style::label(g, px(x0), chart.bottom() + px(label_h / 2.0 + 1.0), style::Align::Left, hour.clone(), style::SMALL_SIZE, style::ui_font(FontWeight::NORMAL), style::secondary_text(&p));
            }
            said.push(format!("{hour}:00 {} hits, {} misses, {} not cacheable", h[0], h[1], h[2]));
        }
        let _ = s;
        olo_kit::agent::record(g, olo_kit::agent::Node { role: "Image", name: Some("Builds by the hour".into()), value: Some(said.join("; ").into()), bounds: b, enabled: true, ..Default::default() });
    })
    .paint_key(key)
    .w_full()
    .h(px(120.0))
    .into_element()
}

/// The chart's colours: hits, misses, what couldn't be cached.
fn hour_colours(p: &olo_theme::Palette) -> [olo_kit::Hsla; 3] {
    [style::c(p.hue(olo_theme::Hue::Green)), style::c(p.hue(olo_theme::Hue::Orange)), style::c(olo_theme::mix(p.panel_background, p.panel_text, 0.25))]
}

/// What the chart's colours are, with the day's counts.
fn hours_legend(day: &data::Day) -> Element {
    let counts = [day.hits, day.misses, day.others];
    canvas(move |g, b| {
        let p = theme::palette();
        let colours = hour_colours(&p);
        let names = ["Hits", "Misses", "Couldn't be cached"];
        let items: Vec<olo_kit::widgets::charts::LegendItem> =
            (0..3).map(|i| olo_kit::widgets::charts::LegendItem { color: colours[i], name: names[i].into(), value: style::thousands(counts[i] as usize).into() }).collect();
        olo_kit::widgets::charts::legend(g, b, &items);
    })
    .paint_key(style::key(counts))
    .w_full()
    .h(px(olo_kit::widgets::charts::FACT_ROW))
    .into_element()
}

impl View for Kache {
    type Message = Msg;

    fn message(&mut self, message: Msg, cx: &mut Cx<Self>) {
        match message {
            // (A burst of builds: one look, then the next at most every two
            // seconds.)
            Msg::Changed => {
                if !self.looking && !self.again {
                    self.again = true;
                    cx.after(LOOK_EVERY, Msg::Look);
                }
                return;
            }
            Msg::Look => {
                self.again = false;
                self.look(cx);
                return;
            }
            Msg::Read(snap) => {
                self.looking = false;
                self.rows = Rc::new(Kache::rows(&snap.recent));
                self.snap = Rc::new(*snap);
                if self.again {
                    self.again = false;
                    cx.after(LOOK_EVERY, Msg::Look);
                }
            }
            Msg::Service(r) => {
                self.doing = None;
                match r {
                    Ok(u) if u.active == "failed" => self.say("kache failed as it started: its log says why (File ▸ Open kache's Log).", true),
                    Ok(u) => self.say(if u.running() { "kache is running." } else { "kache is stopped." }, false),
                    Err(e) => self.say(e, true),
                }
                self.look(cx);
            }
            Msg::CleanedUp(r) => {
                self.doing = None;
                match r {
                    Ok(t) => self.say(t, false),
                    Err(e) => self.say(e, true),
                }
                self.look(cx);
            }
        }
        cx.invalidate();
    }

    fn command(&mut self, command: &str, cx: &mut Cx<Self>) -> bool {
        self.run(command, cx)
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let bar = MenuBar::new(cx, self.menus());
        let p = theme::palette();
        let sidebar = self.sidebar(cx);
        let content = match self.section {
            Section::Status => self.status(cx),
            Section::Builds => self.builds(cx),
            Section::Settings => self.settings(cx),
        };
        let row = div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_row()
            .gap(px(m::SPACING))
            .px(px(m::WINDOW_INSET))
            .pt(px(m::SPACING))
            .pb(px(m::WINDOW_INSET))
            .child(div().w(px(SIDEBAR)).flex_shrink_0().h_full().child(well(cx, p.list_background, sidebar)))
            .child(div().flex_1().min_w_0().h_full().child(content));
        div().size_full().flex().flex_col().bg(style::c(p.panel_background)).child(bar).child(row).into_element()
    }

    fn save_state(&mut self, _: &mut Cx<Self>) -> Option<String> {
        serde_json::to_string(&Saved { section: self.section }).ok()
    }
}

/// A sunken well round `content`.
fn well(cx: &olo_kit::WindowCx, background: olo_theme::Rgba, content: impl olo_kit::IntoElement) -> Element {
    let s = cx.scale_factor();
    let e = style::line_pixels(s) * 2.0 / s;
    let ground = canvas(move |g, b| {
        let p = theme::palette();
        let b = style::snap(g, b);
        style::fill_rect(g, b, style::c(background));
        style::sunken(g, b, &p, false);
    })
    .paint_key(style::key(background))
    .absolute()
    .inset_0();
    div().relative().size_full().child(ground).child(div().absolute().left(px(e)).top(px(e)).right(px(e)).bottom(px(e)).overflow_hidden().child(content)).into_element()
}

/// `olo-kache --print`: what Kache shows, as JSON, with no window.
fn print() -> std::process::ExitCode {
    let s = data::read();
    let out = serde_json::json!({
        "service": s.unit.as_ref().map(|u| serde_json::json!({"running": u.running(), "state": u.active, "since_ms": u.since_us / 1000, "pid": u.pid, "memory": u.memory})),
        "daemon": s.daemon.as_ref().map(|d| serde_json::json!({"version": d.version, "store_bytes": d.size, "disk_bytes": d.disk, "limit_bytes": d.limit, "entries": d.entries, "in_flight": d.in_flight})),
        "last_cleanup": s.cleanup.as_ref().map(|c| serde_json::json!({"at_ms": c.at_ms, "freed_bytes": c.freed, "held_bytes": c.held, "held_entries": c.unreclaimable})),
        "pool": s.pool.as_ref().map(|p| serde_json::json!({"slots": p.size, "compiles": p.compiles, "tests": p.tests, "waiting": p.waiting})),
        "last_24_hours": {"hits": s.day.hits, "misses": s.day.misses, "not_cacheable": s.day.others, "hit_rate": s.day.rate(), "saved_ms": s.day.saved_ms, "by_hour": s.day.hours,
            "projects": s.day.projects.iter().map(|t| serde_json::json!({"name": t.name, "hits": t.hits, "misses": t.misses, "not_cacheable": t.others})).collect::<Vec<_>>(),
            "slowest_compiles": s.day.slowest.iter().map(|(c, ms)| serde_json::json!({"crate": c, "ms": ms})).collect::<Vec<_>>(),
            "miss_causes": s.day.causes.iter().map(|(c, n)| serde_json::json!({"cause": c, "misses": n})).collect::<Vec<_>>()},
        "tests_holding_slots": s.held.iter().map(|h| serde_json::json!({"pid": h.pid, "test": h.name, "project": h.project, "slots": h.slots, "since_ms": h.since_ms})).collect::<Vec<_>>(),
        "settings": data::SETTINGS.iter().zip(data::settings()).map(|(k, v)| (k.key.to_owned(), serde_json::json!(v))).collect::<serde_json::Map<_, _>>(),
    });
    println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
    std::process::ExitCode::SUCCESS
}

fn main() -> std::process::ExitCode {
    olo_crash::install();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "--print") {
        return print();
    }
    if args.first().is_some_and(|a| a == "--glance") {
        return glance::serve();
    }
    // A clean-up now, with no window (agents), saying what it did.
    if args.first().is_some_and(|a| a == "--clean-up") {
        return match data::clean_up() {
            Ok(said) => {
                println!("{said}");
                std::process::ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("olo-kache: {e}");
                std::process::ExitCode::FAILURE
            }
        };
    }
    // The service started, restarted or stopped, with no window (Doctor's
    // fix; agents).
    if let Some(method) = args.first().and_then(|a| match a.as_str() {
        "--start" => Some("StartUnit"),
        "--restart" => Some("RestartUnit"),
        "--stop" => Some("StopUnit"),
        _ => None,
    }) {
        return match data::service(method) {
            Ok(u) if u.active == "failed" => {
                eprintln!("olo-kache: kache failed as it started: see {}", data::cache_dir().join("daemon.log").display());
                std::process::ExitCode::FAILURE
            }
            Ok(u) => {
                println!("kache is {}", if u.running() { "running" } else { "stopped" });
                std::process::ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("olo-kache: {e}");
                std::process::ExitCode::FAILURE
            }
        };
    }
    if instance::forward(APP_ID, &args) {
        return std::process::ExitCode::SUCCESS;
    }
    olo_kit::run(APP_ID, |app| {
        let window = olo_kit::open_window(WindowOptions::new("Kache", 980.0, 680.0).min_size(MIN_WIDTH, MIN_HEIGHT).keep_size("main"), Kache::new);
        instance::listen(APP_ID, olo_kit::proxy());
        olo_kit::message_handler(app, move |message, _| {
            if message.is::<Launch>() {
                olo_kit::raise(Some(&window));
            }
        });
    });
    std::process::ExitCode::SUCCESS
}
