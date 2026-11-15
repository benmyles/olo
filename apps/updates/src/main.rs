//! OS Updates: Fedora's packages kept up to date, for people who want to
//! see and choose everything (Ben, 2026-11-14: "a GUI manager for fedora
//! dnf for upgrading to the latest available fedora packages. design it
//! for power users where you can drill into all the relevant details and
//! take all the actions you might want to take either one thing at a time
//! or all at once. also have it support adding/managing 3rd party repos
//! everything else that makes sense. make it intuitive, polished and look
//! great.").
//!
//! Everything goes through DNF5's own daemon (`olo_system::dnf`), nothing
//! shelled out to. Four sections, down the left as Activity Monitor's:
//!
//! - updates.rs: every update, each with a check box (Olo Doctor's
//!   pattern for all, some or one: `olo_kit::chosen`), searched and
//!   filtered by kind, severity and repository; the selected one's
//!   versions, advisories (CVEs, bugs), changelog since what's installed
//!   and dependencies beside the list.
//! - repos.rs: the repositories, turned on and off, added (a .repo file's
//!   address, a Copr project, RPM Fusion) and removed, each one's every
//!   setting and keys.
//! - history.rs: what was done, package by package, and Undo.
//! - release.rs: the next Fedora, prepared now and installed as the
//!   computer restarts; what waits for the next start.
//! - review.rs: what a change comes to, every package it pulls in or
//!   takes out, sizes and problems, shown before it runs; then run with
//!   each download's and package's progress, cancellable while it
//!   downloads, a repository's key asked about when one is.
//! - helper.rs: writing and removing .repo files, as root, through
//!   pkexec (the daemon has no call for it).
//!
//! `olo-updates --list` prints the updates as JSON (from the cache, with
//! no window), for agents and scripts. Tests point it at `dnf-fake` with
//! `OLO_DNF_BUS`; packages are only ever changed from the person's own
//! session at the computer (`dnf::may_change`).

mod helper;
mod history;
mod release;
mod repos;
mod review;
mod updates;
mod words;

use std::{rc::Rc, sync::Arc, time::Duration};

use olo_kit::{
    Cx, Element, IntoElement as _, Menu, MenuBar, ParentElement as _, Styled as _, View, WindowOptions, canvas, chosen, div,
    instance::{self, Launch},
    metrics as m, px, style, theme,
    widgets::{
        StatusBar, StringView,
        sidebar::{Row, Sidebar},
    },
};
use olo_system::dnf::{self, Event, Offline, Open, Repo, Session, Transaction};
use serde::{Deserialize, Serialize};

pub const APP_ID: &str = "olo.updates";

/// The sidebar's width.
const SIDEBAR: f32 = 172.0;
pub const MIN_WIDTH: f32 = 960.0;
pub const MIN_HEIGHT: f32 = 600.0;

/// The four sections.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Section {
    #[default]
    Updates,
    Repos,
    History,
    Upgrade,
}

impl Section {
    pub const ALL: [Section; 4] = [Section::Updates, Section::Repos, Section::History, Section::Upgrade];
    pub fn name(self) -> &'static str {
        match self {
            Section::Updates => "Updates",
            Section::Repos => "Repositories",
            Section::History => "History",
            Section::Upgrade => "Upgrade Fedora",
        }
    }
    fn icon(self) -> &'static str {
        match self {
            Section::Updates => "software-update",
            Section::Repos => "database",
            Section::History => "logs",
            Section::Upgrade => "installer",
        }
    }
    fn command(self) -> &'static str {
        match self {
            Section::Updates => "section:updates",
            Section::Repos => "section:repos",
            Section::History => "section:history",
            Section::Upgrade => "section:upgrade",
        }
    }
    fn of(word: &str) -> Option<Section> {
        Section::ALL.into_iter().find(|s| s.command()[8..] == *word || s.name().eq_ignore_ascii_case(word))
    }
}

/// What a load read, and the session it read it with (kept for details).
pub struct Loaded {
    pub updates: dnf::Updates,
    pub repos: Vec<Repo>,
    pub history: Vec<Transaction>,
    pub offline: Offline,
    pub session: Arc<Session>,
}

/// A load under way: its number and what it's doing.
pub struct Loading {
    pub n: u64,
    pub what: String,
    pub fraction: Option<f32>,
    pub fresh: bool,
}

#[derive(Clone)]
pub enum Msg {
    /// Read everything again (`true`: the metadata fetched fresh).
    Load(bool),
    Progress(u64, String, Option<f32>),
    Loaded(u64, Result<Arc<std::sync::Mutex<Option<Loaded>>>, String>),
    Updates(updates::Msg),
    Repos(repos::Msg),
    History(history::Msg),
    Release(release::Msg),
    /// A change ran (from the review; whether it went through): read
    /// everything again.
    Changed(bool),
    /// Something to say in the status line (true: it went wrong).
    Note(String, bool),
}

/// What's kept for the window across an upgrade.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Saved {
    section: Section,
    chosen: chosen::Chosen,
    search: String,
    selected: Option<String>,
}

pub struct OsUpdates {
    pub section: Section,
    pub session: Option<Arc<Session>>,
    pub updates: Rc<dnf::Updates>,
    pub repos: Rc<Vec<Repo>>,
    pub history: Rc<Vec<Transaction>>,
    pub offline: Offline,
    pub loading: Option<Loading>,
    loads: u64,
    /// Why the last load failed.
    pub error: Option<String>,
    /// When the updates were last read (Unix time), and fetched fresh.
    pub read_at: Option<i64>,
    pub fresh_at: Option<i64>,
    pub note: Option<(String, bool)>,
    pub u: updates::State,
    pub r: repos::State,
    pub h: history::State,
    pub rel: release::State,
}

impl OsUpdates {
    fn new(cx: &mut Cx<Self>) -> OsUpdates {
        let saved: Saved = olo_kit::restored_state().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        let u = updates::State::new(cx, &saved.search, saved.chosen, saved.selected);
        let mut app = OsUpdates {
            section: saved.section,
            session: None,
            updates: Rc::default(),
            repos: Rc::default(),
            history: Rc::default(),
            offline: Offline::default(),
            loading: None,
            loads: 0,
            error: None,
            read_at: None,
            fresh_at: None,
            note: None,
            u,
            r: repos::State::default(),
            h: history::State::default(),
            rel: release::State::default(),
        };
        app.load(false, cx);
        release::check(&mut app, cx);
        app
    }

    /// Read everything again, on a worker (`fresh`: the repositories'
    /// metadata fetched now, not only when it's out of date).
    pub fn load(&mut self, fresh: bool, cx: &mut Cx<Self>) {
        self.loads += 1;
        let n = self.loads;
        self.loading = Some(Loading { n, what: if fresh { "Checking for updates…".into() } else { "Reading the packages…".into() }, fraction: None, fresh });
        let progress = cx.messenger();
        cx.work(
            move || {
                load(fresh, &mut |what, f| {
                    progress.send(Msg::Progress(n, what, f));
                })
                .map(|l| Arc::new(std::sync::Mutex::new(Some(l))))
            },
            move |r| Msg::Loaded(n, r),
        );
        cx.invalidate();
    }

    pub fn say(&mut self, text: impl Into<String>, failed: bool) {
        self.note = Some((text.into(), failed));
    }

    pub fn menus(&self) -> Vec<Menu> {
        let busy = self.loading.is_some();
        let mut update = Menu::new("Updates");
        update = chosen::menu(update, "update-chosen", &self.u.counts(&self.updates), !busy && self.session.is_some());
        update = update
            .separator()
            .item("Update Selected Only…", "update-selected")
            .enabled(!busy && !self.u.selected_keys(&self.updates).is_empty())
            .item("Reinstall Selected…", "reinstall-selected")
            .enabled(!busy && !self.u.selected_keys(&self.updates).is_empty());
        let edit =
            Menu::new("Edit").item("Copy", "copy").key("c").enabled(self.section == Section::Updates && !self.u.selected_keys(&self.updates).is_empty()).separator().item("Find…", "find").key("f");
        let repo = self.r.selected(&self.repos);
        let repos = Menu::new("Repositories")
            .item("Add Repository…", "add-repo")
            .separator()
            .item("Turn On", "repo-on")
            .enabled(repo.is_some_and(|r| !r.enabled))
            .item("Turn Off", "repo-off")
            .enabled(repo.is_some_and(|r| r.enabled))
            .item("Remove Repository…", "remove-repo")
            .enabled(repo.is_some_and(repos::removable))
            .separator()
            .item("Show File in Tracker", "repo-file")
            .enabled(repo.is_some_and(|r| !r.repofile.is_empty()));
        let history = Menu::new("History").item("Undo Transaction…", "undo-transaction").enabled(self.h.selected(&self.history).is_some_and(history::undoable));
        let mut view = Menu::new("View");
        for s in Section::ALL {
            view = view.item(s.name(), s.command()).checked(self.section == s);
        }
        vec![
            Menu::new("File").app_items(&[]).item("Check for Updates", "check").key("r").enabled(!busy).separator().item("Restart Now…", "restart").enabled(self.offline.pending).close_quit(),
            edit,
            update,
            repos,
            history,
            view,
        ]
    }

    /// The sidebar: the sections, each with its count.
    fn sidebar(&mut self, cx: &mut Cx<Self>) -> Element {
        let mut rows: Vec<Row> = vec![Row::heading("Fedora")];
        for s in Section::ALL {
            let value = match s {
                Section::Updates if self.session.is_some() => self.updates.updates.len().to_string(),
                Section::Repos if self.session.is_some() => format!("{} on", self.repos.iter().filter(|r| r.enabled).count()),
                _ => String::new(),
            };
            let row = Row::new(s.icon(), s.name());
            rows.push(if value.is_empty() { row } else { row.value(value) });
        }
        let chosen = Section::ALL.iter().position(|s| *s == self.section).unwrap_or(0) + 1;
        Sidebar::new("updates-sections", rows, chosen)
            .bare()
            .on_choose(cx.listener(|v: &mut OsUpdates, i: &usize, cx| {
                if let Some(s) = i.checked_sub(1).and_then(|i| Section::ALL.get(i)) {
                    v.section = *s;
                    cx.invalidate();
                }
            }))
            .build(cx)
    }

    /// The line under a section: what's being done, or the last word.
    pub fn status(&self, cx: &mut Cx<Self>) -> Element {
        if let Some(l) = &self.loading {
            return div()
                .flex()
                .items_center()
                .gap(px(m::SPACING))
                .h(px(m::CONTROL_HEIGHT))
                .child(div().w(px(160.0)).child(StatusBar::new(l.fraction.unwrap_or(0.0)).bar_height(10.0)))
                .child(StringView::new(l.what.clone()).dim(true).cut(true))
                .into_element();
        }
        let _ = cx;
        let (text, failed) = match (&self.note, &self.error) {
            (Some((t, f)), _) => (t.clone(), *f),
            (None, Some(e)) => (e.clone(), true),
            (None, None) => (self.checked_words(), false),
        };
        let p = theme::palette();
        let mut s = StringView::new(text).cut(true);
        s = if failed { s.color(style::c(p.failure)) } else { s.dim(true) };
        div().flex().items_center().h(px(m::CONTROL_HEIGHT)).min_w_0().child(s).into_element()
    }

    fn checked_words(&self) -> String {
        match (self.fresh_at.or(self.read_at), self.session.is_some()) {
            (Some(t), true) => format!("Checked {}.", words::ago(t)),
            _ => String::new(),
        }
    }

    /// Restart the computer (what waits for the next start is done then).
    pub fn restart(&mut self, cx: &mut Cx<Self>) {
        if !(dnf::real() && olo_system::env::at_the_computer()) {
            self.say("This isn't the session at the computer, so the computer isn't restarted.", true);
            cx.invalidate();
            return;
        }
        cx.work(
            || olo_system::power::reboot().map_err(|e| e.to_string()),
            |r| match r {
                Ok(()) => Msg::Note("Restarting…".into(), false),
                Err(e) => Msg::Note(format!("The computer couldn't be restarted: {e}"), true),
            },
        );
    }

    fn run(&mut self, command: &str, cx: &mut Cx<Self>) -> bool {
        if let Some(choice) = chosen::Choice::of_command(command) {
            self.u.choose(&self.updates, choice);
            self.section = Section::Updates;
            cx.invalidate();
            return true;
        }
        match command {
            "check" => {
                self.note = None;
                self.load(true, cx);
            }
            "restart" => self.restart(cx),
            "close" | "quit" => olo_kit::quit(),
            c if c.starts_with("section:") => match Section::of(&c[8..]) {
                Some(s) => {
                    self.section = s;
                    cx.invalidate();
                }
                None => return false,
            },
            c => {
                return updates::command(self, c, cx) || repos::command(self, c, cx) || history::command(self, c, cx) || release::command(self, c, cx);
            }
        }
        true
    }
}

/// Read everything: the repositories (fetching what's out of date, or
/// everything when `fresh`), the updates, repositories, history and what
/// waits for the next start.
fn load(fresh: bool, progress: &mut dyn FnMut(String, Option<f32>)) -> Result<Loaded, String> {
    let text = |e: olo_system::Error| dnf::error_text(&e);
    let s = Session::open(&Open::default()).map_err(text)?;
    if fresh {
        // (Not allowed to expire the cache, it's read as it is.)
        if let Err(e) = s.expire_cache() {
            tracing::info!("the cache couldn't be expired: {}", dnf::error_text(&e));
        }
    }
    let mut downloads: Vec<(String, String, i64, i64)> = Vec::new();
    let read = s.hearing(
        &mut |e| {
            match e {
                Event::DownloadStart { id, what, total } => downloads.push((id, what, total.max(0), 0)),
                Event::Download { id, done, total } => {
                    if let Some(d) = downloads.iter_mut().find(|d| d.0 == id) {
                        d.2 = total.max(d.2);
                        d.3 = done;
                    }
                }
                Event::DownloadEnd { id, .. } => {
                    if let Some(d) = downloads.iter_mut().find(|d| d.0 == id) {
                        d.3 = d.2;
                    }
                }
                _ => return,
            }
            let (done, total) = downloads.iter().fold((0i64, 0i64), |(a, b), d| (a + d.3, b + d.2));
            let now = downloads.iter().rev().find(|d| d.3 < d.2).or(downloads.last()).map(|d| d.1.clone()).unwrap_or_default();
            progress(format!("Fetching {now}…"), (total > 0).then(|| done as f32 / total as f32));
        },
        |s| s.read_repos(),
    );
    match read {
        Ok(true) => {}
        // Some couldn't be read (offline, a mirror down): what was, is.
        Ok(false) => tracing::info!("not every repository could be read"),
        Err(e) => return Err(text(e)),
    }
    progress("Reading the updates…".into(), None);
    let updates = s.updates().map_err(text)?;
    let repos = s.repos().map_err(text)?;
    let history = s.history(500).map_err(text)?;
    let offline = s.offline().map_err(text)?;
    Ok(Loaded { updates, repos, history, offline, session: Arc::new(s) })
}

/// A sunken well of `background` with `content` inside its lines.
pub fn well(cx: &olo_kit::WindowCx, background: olo_theme::Rgba, content: impl olo_kit::IntoElement) -> Element {
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

impl View for OsUpdates {
    type Message = Msg;

    fn message(&mut self, message: Msg, cx: &mut Cx<Self>) {
        match message {
            Msg::Load(fresh) => self.load(fresh, cx),
            Msg::Progress(n, what, f) => match &mut self.loading {
                Some(l) if l.n == n => {
                    l.what = what;
                    l.fraction = f;
                }
                _ => return,
            },
            Msg::Loaded(n, r) => {
                let Some(l) = self.loading.take_if(|l| l.n == n) else { return };
                match r {
                    Ok(loaded) => {
                        let Some(loaded) = loaded.lock().ok().and_then(|mut l| l.take()) else { return };
                        let now = words::now();
                        self.read_at = Some(now);
                        if l.fresh {
                            self.fresh_at = Some(now);
                        }
                        self.error = None;
                        self.updates = Rc::new(loaded.updates);
                        self.repos = Rc::new(loaded.repos);
                        self.history = Rc::new(loaded.history);
                        self.offline = loaded.offline;
                        self.session = Some(loaded.session);
                        self.u.took(&self.updates, cx);
                        self.r.took(&self.repos);
                        self.h.took(&self.history);
                        updates::details_for_selected(self, cx);
                    }
                    Err(e) => self.error = Some(e),
                }
            }
            Msg::Updates(m) => updates::message(self, m, cx),
            Msg::Repos(m) => repos::message(self, m, cx),
            Msg::History(m) => history::message(self, m, cx),
            Msg::Release(m) => release::message(self, m, cx),
            Msg::Changed(ok) => {
                // What was chosen was done: the next time, what's left
                // goes by its kind again.
                if ok {
                    self.u.chosen = olo_kit::chosen::Chosen::default();
                }
                self.load(false, cx);
                release::check(self, cx);
            }
            Msg::Note(text, failed) => self.say(text, failed),
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
            Section::Updates => updates::body(self, cx),
            Section::Repos => repos::body(self, cx),
            Section::History => history::body(self, cx),
            Section::Upgrade => release::body(self, cx),
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
            .child(div().flex_1().min_w_0().h_full().flex().flex_col().gap(px(m::SPACING)).child(content));
        div().size_full().flex().flex_col().bg(style::c(p.panel_background)).child(bar).child(row).into_element()
    }

    fn save_state(&mut self, cx: &mut Cx<Self>) -> Option<String> {
        let search = self.u.search.read(cx).text(cx).to_owned();
        serde_json::to_string(&Saved { section: self.section, chosen: self.u.chosen.clone(), search, selected: self.u.selected.clone() }).ok()
    }
}

/// `olo-updates --list`: the updates as JSON, from what's cached (no
/// window): each one's installed and new versions, repository, sizes and
/// advisories.
fn list() -> std::process::ExitCode {
    let s = match Session::open(&Open { cache_only: true, changelogs: false, ..Default::default() }) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("olo-updates: {}", dnf::error_text(&e));
            return std::process::ExitCode::FAILURE;
        }
    };
    match s.updates() {
        Ok(u) => {
            let out: Vec<serde_json::Value> = u
                .updates
                .iter()
                .map(|x| {
                    serde_json::json!({
                        "name": x.new.name, "arch": x.new.arch, "installed": x.installed.evr, "new": x.new.evr,
                        "repository": x.new.repo_id, "download_size": x.new.download_size, "kind": x.kind(&u.advisories),
                        "advisories": x.advisories.iter().filter_map(|id| u.advisories.get(id)).map(|a| serde_json::json!({
                            "id": a.id, "type": a.kind, "severity": a.severity, "title": a.title,
                            "references": a.references.iter().map(|r| &r.id).collect::<Vec<_>>(),
                        })).collect::<Vec<_>>(),
                    })
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
            std::process::ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("olo-updates: {}", dnf::error_text(&e));
            std::process::ExitCode::FAILURE
        }
    }
}

fn main() -> std::process::ExitCode {
    olo_crash::install();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--list") => return list(),
        Some("--helper") => return helper::serve(),
        _ => {}
    }
    if instance::forward(APP_ID, &args) {
        return std::process::ExitCode::SUCCESS;
    }
    olo_kit::run(APP_ID, |app| {
        let window = olo_kit::open_window(WindowOptions::new("OS Updates", 1220.0, 740.0).min_size(MIN_WIDTH, MIN_HEIGHT).keep_size("main"), OsUpdates::new);
        instance::listen(APP_ID, olo_kit::proxy());
        olo_kit::message_handler(app, move |message, _| {
            if message.is::<Launch>() {
                olo_kit::raise(Some(&window));
            }
        });
    });
    std::process::ExitCode::SUCCESS
}

/// How often what's waiting is looked at again while the window is open
/// (the metadata's own expiry decides whether anything is fetched).
pub const LOOK_AGAIN: Duration = Duration::from_secs(6 * 3600);
