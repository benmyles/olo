//! A change, reviewed before it's made: what it comes to once resolved
//! (every package it upgrades, installs as something else needs it,
//! removes or downgrades, from where, how big, and why), the download
//! and the change in disk space, and what's wrong if it can't be done.
//! Then, when told, it runs: now, or (the system's own packages being
//! safest so) downloaded now and installed as the computer restarts.
//! While it runs: each download and each package as it goes, Stop while
//! it downloads (the daemon can't stop RPM once it starts), a
//! repository's key asked about when one is, and at the end what it did
//! and whether the computer should restart.
//!
//! Its own window (a dialog over the main one) with a session of its
//! own: the main window keeps reading while it's open, and is told to
//! read again when it's done (even if this window was closed meanwhile).

use std::{
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

use olo_kit::{
    Cx, Element, IntoElement as _, Messenger, ParentElement as _, ScrollHandle, Styled as _, View, WindowOptions, div, metrics as m, px, style, theme,
    widgets::{Banner, Button, Column, ColumnListView, ListModel, ScrollView, StatusBar, StringView, Tone, button_row},
};
use olo_system::dnf::{self, Event, KeyRequest, Open, Resolved, Session};

use crate::{Msg as AppMsg, words};

/// What a plan marks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Upgrade,
    Install,
    Remove,
    Downgrade,
    Reinstall,
    SystemUpgrade,
}

/// A change to review.
#[derive(Clone, Debug)]
pub struct Plan {
    pub title: String,
    pub ops: Vec<(Op, Vec<String>)>,
    /// Another release's repositories (the system upgrade).
    pub release: Option<String>,
    /// Only at the next start (the system upgrade).
    pub offline_only: bool,
    /// What the history says of it.
    pub comment: String,
}

pub fn open(plan: Plan, to_main: Messenger<AppMsg>) {
    let title = plan.title.clone();
    olo_kit::open_window(WindowOptions::new(title, 780.0, 560.0).min_size(640.0, 440.0).dialog(), move |cx| Review::new(plan, to_main, cx));
}

/// Packages that need the computer started again to take effect.
fn needs_restart(name: &str) -> bool {
    name.starts_with("kernel")
        || name.starts_with("linux-firmware")
        || name.starts_with("kmod-")
        || name.starts_with("akmod-")
        || matches!(name, "glibc" | "systemd" | "systemd-libs" | "dbus" | "dbus-broker" | "dbus-daemon" | "microcode_ctl" | "zlib-ng-compat" | "openssl-libs")
}

enum Stage {
    Resolving,
    Resolved(Resolved),
    Failed(String),
    Running(Run),
    Done(Result<String, String>),
}

/// How a run goes.
#[derive(Default)]
struct Run {
    offline: bool,
    /// Each download: id, what, total, done.
    downloads: Vec<(String, String, i64, i64)>,
    /// What's being done now.
    now: String,
    fraction: f32,
    /// Past the downloads: RPM has it, and it can't be stopped.
    installing: bool,
    stopping: bool,
    /// The key being asked about.
    key: Option<KeyRequest>,
    /// What went by, newest last.
    log: Vec<String>,
    packages: (u64, u64),
}

#[derive(Clone)]
pub enum Msg {
    /// The session, what it comes to, and what dnf wouldn't take (each
    /// argument and why), left out.
    Resolved(Result<(Arc<Session>, Arc<Resolved>, Vec<(String, String)>), String>),
    Event(Box<Event>),
    Ran(Result<(), String>),
    Go(bool),
    Stop,
    Trust(bool),
    Close,
    Restart,
}

pub struct Review {
    plan: Plan,
    to_main: Messenger<AppMsg>,
    session: Option<Arc<Session>>,
    stage: Stage,
    list: ListModel,
    /// The items shown: (action, name, version, from, size, why).
    rows: Rc<Vec<[String; 6]>>,
    log_scroll: ScrollHandle,
    restart: bool,
    /// What dnf wouldn't take, left out: each argument and why.
    left_out: Vec<(String, String)>,
}

impl Review {
    fn new(plan: Plan, to_main: Messenger<AppMsg>, cx: &mut Cx<Self>) -> Review {
        let p = plan.clone();
        cx.work(move || resolve(&p), |r| Msg::Resolved(r.map(|(s, r, left)| (s, Arc::new(r), left))));
        Review { plan, to_main, session: None, stage: Stage::Resolving, list: ListModel::new(true), rows: Rc::default(), log_scroll: ScrollHandle::new(), restart: false, left_out: Vec::new() }
    }

    fn rows_of(r: &Resolved) -> Vec<[String; 6]> {
        let order = |a: &str| match a {
            "Upgrade" => 0,
            "Install" => 1,
            "Downgrade" => 2,
            "Reinstall" => 3,
            "Remove" => 4,
            _ => 5,
        };
        let mut items: Vec<&dnf::Item> = r.items.iter().filter(|i| i.action != "Replaced").collect();
        items.sort_by(|a, b| order(&a.action).cmp(&order(&b.action)).then(a.package.name.cmp(&b.package.name)).then(a.package.evr.cmp(&b.package.evr)));
        // Each replaced version goes with one new one: of several (kernels,
        // installed side by side), the one of the same version first.
        let mut used = vec![false; r.items.len()];
        items
            .into_iter()
            .map(|i| {
                let p = &i.package;
                // (Its own architecture's first; then one it moves from or to,
                // noarch and an architecture: python3-wcwidth.)
                let same = |k: usize| r.items[k].package.arch == p.arch;
                let candidates: Vec<usize> = (0..r.items.len())
                    .filter(|&k| {
                        let q = &r.items[k].package;
                        !used[k] && r.items[k].action == "Replaced" && q.name == p.name && (q.arch == p.arch || q.arch == "noarch" || p.arch == "noarch")
                    })
                    .collect();
                let pick =
                    candidates.iter().copied().find(|&k| same(k) && r.items[k].package.version == p.version).or_else(|| candidates.iter().copied().find(|&k| same(k))).or(candidates.first().copied());
                let old = pick.map(|k| {
                    used[k] = true;
                    r.items[k].package.evr.clone()
                });
                let version = match old {
                    Some(o) => format!("{o} → {}", p.evr),
                    None => p.evr.clone(),
                };
                let action = match i.action.as_str() {
                    "Upgrade" => "Update",
                    "Install" => "Install",
                    "Downgrade" => "Downgrade",
                    "Reinstall" => "Reinstall",
                    "Remove" => "Remove",
                    other => other,
                };
                let why = match i.reason.as_str() {
                    "User" => "Chosen",
                    "Dependency" => "Needed",
                    "Weak Dependency" => "Recommended",
                    "Clean" => "No longer needed",
                    "Group" => "In a group",
                    other => other,
                };
                let size = if i.action == "Remove" { dnf::size_words(p.install_size) } else { dnf::size_words(p.download_size) };
                [action.to_owned(), format!("{}.{}", p.name, p.arch), version, if p.repo_id == "@System" { p.from_repo_id.clone() } else { p.repo_id.clone() }, size, why.to_owned()]
            })
            .collect()
    }

    fn summary(r: &Resolved) -> String {
        let n = |a: &str| r.items.iter().filter(|i| i.action == a).count();
        // (Each said whole, counted: "update 3 packages", for "This will
        // {}." in any language.)
        let mut parts = Vec::new();
        let mut add = |c: usize, said: fn(u64) -> String| {
            if c > 0 {
                parts.push(said(c as u64));
            }
        };
        add(n("Upgrade"), |c| olo_kit::words::count(c, "update 1 package", "update {n} packages"));
        add(n("Install"), |c| olo_kit::words::count(c, "install 1 package", "install {n} packages"));
        add(n("Downgrade"), |c| olo_kit::words::count(c, "downgrade 1 package", "downgrade {n} packages"));
        add(n("Reinstall"), |c| olo_kit::words::count(c, "reinstall 1 package", "reinstall {n} packages"));
        add(n("Remove"), |c| olo_kit::words::count(c, "remove 1 package", "remove {n} packages"));
        let (download, disk) = r.sizes();
        let mut s = match parts.as_slice() {
            [] => olo_kit::words::tr("Nothing to do.").to_owned(),
            [one] => olo_kit::tr!("This will {}.", one),
            more => {
                let (last, rest) = more.split_last().expect("two");
                olo_kit::tr!("This will {} and {}.", rest.join(", "), last)
            }
        };
        if download > 0 {
            s.push_str(&format!(" {}", olo_kit::tr!("It downloads {}", dnf::size_words(download))));
            s.push_str(&match disk {
                0 => format!(" {}", olo_kit::words::tr("and needs no more disk space.")).to_owned(),
                1.. => format!(" {}", olo_kit::tr!("and takes {} more disk space.", dnf::size_words(disk as u64))),
                _ => format!(" {}", olo_kit::tr!("and frees {}.", dnf::size_words((-disk) as u64))),
            });
        }
        s
    }

    fn go(&mut self, offline: bool, cx: &mut Cx<Self>) {
        let Some(session) = self.session.clone() else { return };
        if !dnf::may_change() {
            return;
        }
        let Stage::Resolved(r) = &self.stage else { return };
        self.restart = r.items.iter().any(|i| i.action != "Replaced" && needs_restart(&i.package.name));
        let total = r.items.iter().filter(|i| i.action != "Replaced").count() as u64;
        self.stage = Stage::Running(Run { offline, now: "Starting…".into(), packages: (0, total), ..Default::default() });
        let events = cx.messenger();
        let to_main = self.to_main.clone();
        let comment = self.plan.comment.clone();
        cx.work(
            move || {
                // (Progress at most twenty times a second; the rest as it comes.)
                let mut last = Instant::now() - Duration::from_secs(1);
                let r = session.hearing(
                    &mut |e| {
                        let flood = matches!(e, Event::Download { .. } | Event::ActionProgress { .. } | Event::Verify { .. } | Event::Prepare { .. });
                        if flood && last.elapsed() < Duration::from_millis(50) {
                            return;
                        }
                        last = Instant::now();
                        events.send(Msg::Event(Box::new(e)));
                    },
                    |s| s.run(offline, &comment),
                );
                // The main window reads again, open or not.
                to_main.send(AppMsg::Changed(r.is_ok()));
                r.map_err(|e| dnf::error_text(&e))
            },
            Msg::Ran,
        );
    }

    fn take(&mut self, e: Event) {
        let Stage::Running(run) = &mut self.stage else { return };
        match e {
            Event::DownloadStart { id, what, total } => {
                run.now = format!("Downloading {what}");
                run.log.push(if total > 0 { format!("Downloading {what} ({})", dnf::size_words(total as u64)) } else { format!("Downloading {what}") });
                run.downloads.push((id, what, total.max(0), 0));
            }
            Event::Download { id, done, total } => {
                if let Some(d) = run.downloads.iter_mut().find(|d| d.0 == id) {
                    d.2 = total.max(d.2);
                    d.3 = done;
                }
            }
            Event::DownloadEnd { id, ok, message } => {
                if let Some(d) = run.downloads.iter_mut().find(|d| d.0 == id) {
                    d.3 = d.2;
                    if !ok {
                        run.log.push(olo_kit::tr!("{} didn’t download: {}", d.1, message));
                    }
                }
            }
            Event::MirrorFailure { message, url, .. } => run.log.push(olo_kit::tr!("A mirror failed ({}): {}", url, message)),
            Event::Key(k) => {
                run.now = olo_kit::words::tr("A repository’s key needs to be trusted").into();
                run.key = Some(k);
            }
            Event::KeyImported { key_id } => run.log.push(olo_kit::tr!("Key {} trusted.", key_id)),
            Event::Begin { .. } | Event::Verify { .. } => {
                run.installing = true;
                run.now = olo_kit::words::tr("Checking the packages…").into();
            }
            Event::Prepare { .. } => {
                run.installing = true;
                run.now = "Preparing…".into();
            }
            Event::Action { nevra, action, .. } => {
                run.installing = true;
                run.now = format!("{} {nevra}", dnf::action_words(action));
                run.log.push(run.now.clone());
            }
            Event::Element { done, total, .. } => {
                run.packages = (done, total.max(run.packages.1));
            }
            Event::ScriptError { nevra, code, .. } => run.log.push(olo_kit::tr!("A script of {} failed (code {}).", nevra, code)),
            Event::UnpackError { nevra } => run.log.push(olo_kit::tr!("{} couldn’t be unpacked.", nevra)),
            Event::ActionProgress { .. } | Event::ActionDone { .. } | Event::Finished { .. } => {}
        }
        // How far: the downloads are the first half (all of it offline),
        // the packages the second.
        let (done, total) = run.downloads.iter().fold((0i64, 0i64), |(a, b), d| (a + d.3, b + d.2));
        let dl = if total > 0 { done as f32 / total as f32 } else { 0.0 };
        run.fraction = if run.offline {
            dl
        } else if run.installing {
            0.5 + 0.5 * (run.packages.0 as f32 / run.packages.1.max(1) as f32)
        } else {
            0.5 * dl
        };
        if !run.installing && total > 0 {
            run.now = format!("Downloading: {} of {}", dnf::size_words(done as u64), dnf::size_words(total as u64));
        }
        let keep = 200;
        if run.log.len() > keep {
            run.log.drain(..run.log.len() - keep);
        }
    }

    fn items_list(&self, cx: &mut Cx<Self>) -> Element {
        let columns: Rc<[Column]> = Rc::from(vec![
            Column::new("Action", 70.0),
            Column::new("Package", 168.0),
            Column::new("Version", 200.0),
            Column::new("From", 92.0),
            Column::new("Size", 62.0).right(),
            Column::new("Why", 104.0),
        ]);
        let rows = self.rows.clone();
        let names = rows.clone();
        ColumnListView::new(cx, "review-items", &self.list, columns, rows.len(), move |_: &Review, cell, g| {
            if let Some(r) = rows.get(cell.row) {
                cell.text(g, r[cell.column].clone());
            }
        })
        .type_select(move |_: &Review, i| names.get(i).map(|r| r[1].clone().into()).unwrap_or_default())
        .into_element()
    }
}

/// Mark the plan in a new session and resolve it. An argument dnf won't
/// take (one refused empties the whole transaction: "Update 517
/// packages", nothing to do) is left out and it's resolved again without
/// it, a few times at most: what it comes to, and what was left out and
/// why.
#[allow(clippy::type_complexity)]
fn resolve(plan: &Plan) -> Result<(Arc<Session>, Resolved, Vec<(String, String)>), String> {
    resolve_leaving_out(plan, resolve_once)
}

/// [`resolve`], each try by `once`.
#[allow(clippy::type_complexity)]
fn resolve_leaving_out<S>(plan: &Plan, once: impl Fn(&Plan) -> Result<(S, Resolved), String>) -> Result<(S, Resolved, Vec<(String, String)>), String> {
    let resolve_once = once;
    let mut plan = plan.clone();
    let mut left_out: Vec<(String, String)> = Vec::new();
    for _ in 0..3 {
        let (s, r) = resolve_once(&plan)?;
        let refused: Vec<(String, String)> = if r.ok() { Vec::new() } else { dnf::refused_arguments(&r.problems) };
        // Only what was asked for (a problem naming another isn't ours to drop).
        let ours: Vec<(String, String)> = refused.into_iter().filter(|(a, _)| plan.ops.iter().any(|(_, specs)| specs.contains(a))).collect();
        let rest: usize = plan.ops.iter().map(|(_, specs)| specs.iter().filter(|x| !ours.iter().any(|(a, _)| a == *x)).count()).sum();
        if ours.is_empty() || rest == 0 {
            return Ok((s, r, left_out));
        }
        for (_, specs) in &mut plan.ops {
            specs.retain(|x| !ours.iter().any(|(a, _)| a == x));
        }
        left_out.extend(ours);
    }
    let (s, r) = resolve_once(&plan)?;
    Ok((s, r, left_out))
}

fn resolve_once(plan: &Plan) -> Result<(Arc<Session>, Resolved), String> {
    let text = |e: olo_system::Error| dnf::error_text(&e);
    let s = Session::open(&Open { release: plan.release.clone(), changelogs: false, ..Default::default() }).map_err(text)?;
    if plan.release.is_some() {
        s.read_repos().map_err(text)?;
    }
    for (op, specs) in &plan.ops {
        match op {
            Op::Upgrade => s.upgrade(specs),
            Op::Install => s.install(specs),
            Op::Remove => s.remove(specs),
            Op::Downgrade => s.downgrade(specs),
            Op::Reinstall => s.reinstall(specs),
            Op::SystemUpgrade => s.system_upgrade(),
        }
        .map_err(text)?;
    }
    let r = s.resolve(plan.ops.iter().any(|(op, _)| matches!(op, Op::Remove | Op::SystemUpgrade | Op::Downgrade))).map_err(text)?;
    Ok((Arc::new(s), r))
}

impl View for Review {
    type Message = Msg;

    fn message(&mut self, message: Msg, cx: &mut Cx<Self>) {
        match message {
            Msg::Resolved(Ok((session, r, left_out))) => {
                self.left_out = left_out;
                self.session = Some(session);
                self.rows = Rc::new(Self::rows_of(&r));
                self.stage = Stage::Resolved(Arc::try_unwrap(r).unwrap_or_else(|r| (*r).clone()));
            }
            Msg::Resolved(Err(e)) => self.stage = Stage::Failed(e),
            Msg::Event(e) => {
                self.take(*e);
                self.log_scroll.scroll_to_bottom(cx);
            }
            Msg::Ran(r) => {
                let offline = matches!(&self.stage, Stage::Running(run) if run.offline);
                self.stage = Stage::Done(r.map(|()| {
                    let n = self.rows.len();
                    if offline {
                        format!("{} downloaded: they’re installed as the computer next starts.", words::count(n, "package", "packages"))
                    } else {
                        let did = |verb: &str| self.rows.iter().filter(|r| r[0] == verb).count();
                        if let [one] = self.rows.as_slice() {
                            let done = match one[0].as_str() {
                                "Update" => "updated",
                                "Install" => "installed",
                                "Downgrade" => "downgraded",
                                "Reinstall" => "reinstalled",
                                _ => "removed",
                            };
                            return format!("{} was {done}.", one[1].rsplit_once('.').map(|(n, _)| n).unwrap_or(&one[1]));
                        }
                        let parts: Vec<String> = [("Update", "updated"), ("Install", "installed"), ("Downgrade", "downgraded"), ("Reinstall", "reinstalled"), ("Remove", "removed")]
                            .iter()
                            .filter(|(v, _)| did(v) > 0)
                            .map(|(v, w)| format!("{} {w}", did(v)))
                            .collect();
                        format!("{}: {}.", words::count(n, "package changed", "packages changed"), parts.join(", "))
                    }
                }));
                if offline {
                    self.restart = true;
                }
            }
            Msg::Go(offline) => self.go(offline, cx),
            Msg::Stop => {
                if let (Some(s), Stage::Running(run)) = (self.session.clone(), &mut self.stage)
                    && !run.installing
                {
                    run.stopping = true;
                    run.now = "Stopping…".into();
                    cx.work(
                        move || s.cancel().map_err(|e| dnf::error_text(&e)),
                        |r| Msg::Event(Box::new(Event::MirrorFailure { id: String::new(), message: r.err().unwrap_or_default(), url: String::new() })),
                    );
                }
            }
            Msg::Trust(yes) => {
                if let (Some(s), Stage::Running(run)) = (self.session.clone(), &mut self.stage)
                    && let Some(k) = run.key.take()
                {
                    run.now = if yes { olo_kit::words::tr("Trusting the key…").into() } else { olo_kit::words::tr("Not trusted: stopping…").into() };
                    cx.work(move || s.confirm_key(&k.key_id, yes).map_err(|e| dnf::error_text(&e)), |_| Msg::Event(Box::new(Event::ActionDone { nevra: String::new() })));
                }
            }
            Msg::Close => cx.close_window(),
            Msg::Restart => {
                self.to_main.send(AppMsg::Note("Restarting…".into(), false));
                if dnf::real() && olo_system::env::at_the_computer() {
                    cx.work(|| olo_system::power::reboot().map_err(|e| e.to_string()), |_| Msg::Close);
                } else {
                    self.to_main.send(AppMsg::Note(olo_kit::words::tr("This isn't the session at the computer, so the computer isn't restarted.").into(), true));
                    cx.close_window();
                }
            }
        }
        cx.invalidate();
    }

    fn command(&mut self, command: &str, cx: &mut Cx<Self>) -> bool {
        match command {
            "close" => {
                if !matches!(self.stage, Stage::Running(_)) {
                    cx.close_window();
                }
            }
            "install-now" => self.go(false, cx),
            "install-at-restart" => self.go(true, cx),
            "stop" => cx.send(Msg::Stop),
            _ => return false,
        }
        true
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let p = theme::palette();
        let may = dnf::may_change();
        let mut col = div().size_full().flex().flex_col().gap(px(m::SPACING)).p(px(m::WINDOW_INSET)).bg(style::c(p.panel_background));
        let title = |s: String| StringView::new(s).bold(true).size(style::LEAD_SIZE).height(style::line_row(style::LEAD_SIZE));
        match &self.stage {
            Stage::Resolving => {
                col = col
                    .child(title(self.plan.title.clone()))
                    .child(StringView::new("Working out what it comes to: every package it needs, and what it replaces…").dim(true).wrap(true))
                    .child(div().w(px(240.0)).child(StatusBar::new(0.0).bar_height(10.0)))
                    .child(div().flex_1())
                    .child(button_row().child(Button::new("Cancel").on_click(cx.msg(Msg::Close))));
            }
            Stage::Failed(e) => {
                col = col
                    .child(Banner::new(Tone::Failure, "It can’t be worked out", e.clone()))
                    .child(div().flex_1())
                    .child(button_row().child(Button::new("Close").default(true).on_click(cx.msg(Msg::Close))));
            }
            Stage::Resolved(r) => {
                col = col.child(title(self.plan.title.clone())).child(StringView::new(Self::summary(r)).wrap(true));
                // What dnf wouldn't take, left out so the rest goes ahead.
                if !self.left_out.is_empty() {
                    let names: Vec<&str> = self.left_out.iter().map(|(a, _)| a.as_str()).collect();
                    let why: Vec<&str> = self.left_out.iter().map(|(_, w)| w.as_str()).collect();
                    let said = if names.len() == 1 {
                        olo_kit::tr!("Left out, as dnf wouldn’t take it: {}. ({})", names.join(", "), why.join(" "))
                    } else {
                        olo_kit::tr!("Left out, as dnf wouldn’t take them: {}. ({})", names.join(", "), why.join(" "))
                    };
                    col = col.child(StringView::new(said).dim(true).wrap(true));
                }
                if !r.problems.is_empty() {
                    let color = if r.ok() { p.panel_text } else { p.failure };
                    col = col.child(StringView::new(r.problems.join("\n")).wrap(true).color(style::c(color)));
                }
                let items = self.items_list(cx);
                col = col.child(div().flex_1().min_h_0().child(crate::well(cx, p.list_background, items)));
                if !may {
                    col = col.child(StringView::new(dnf::why_not()).dim(true).wrap(true));
                }
                let can = may && r.ok() && !self.rows.is_empty();
                let mut buttons = button_row().child(Button::new("Cancel").on_click(cx.msg(Msg::Close)));
                if self.plan.offline_only {
                    buttons = buttons.child(Button::new("Download, Then Restart to Install").id("install-at-restart").default(can).enabled(can).on_click(cx.msg(Msg::Go(true))));
                } else {
                    buttons = buttons
                        .child(Button::new("Install When Restarting").id("install-at-restart").enabled(can).on_click(cx.msg(Msg::Go(true))))
                        .child(Button::new("Install Now").id("install-now").default(can).enabled(can).on_click(cx.msg(Msg::Go(false))));
                }
                col = col.child(buttons);
            }
            Stage::Running(run) => {
                col = col.child(title(self.plan.title.clone())).child(StringView::new(run.now.clone()).cut(true)).child(StatusBar::new(run.fraction).bar_height(12.0));
                if let Some(k) = &run.key {
                    let who = if k.user_ids.is_empty() { k.key_id.clone() } else { k.user_ids.join(", ") };
                    let detail =
                        format!("To install its packages, the key that signs them has to be trusted:\n{who}\nFingerprint: {}\nFrom: {}\nMade on {}", k.fingerprint, k.url, words::date(k.created));
                    col = col.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(m::TIGHT))
                            .p(px(m::SPACING))
                            .bg(style::c(p.list_background))
                            .child(StringView::new("Trust this repository’s key?").bold(true))
                            .child(StringView::new(detail).wrap(true))
                            .child(button_row().child(Button::new("Don’t Trust").on_click(cx.msg(Msg::Trust(false)))).child(Button::new("Trust").default(true).on_click(cx.msg(Msg::Trust(true))))),
                    );
                }
                let mut log = div().w_full().flex().flex_col().p(px(m::TIGHT));
                for l in &run.log {
                    log = log.child(StringView::new(l.clone()).small(true).cut(true));
                }
                col = col.child(div().flex_1().min_h_0().child(crate::well(cx, p.document_background, ScrollView::new(&self.log_scroll).border(false).background(p.document_background).child(log))));
                let stop = Button::new(if run.stopping { "Stopping…" } else { "Stop" }).id("stop").enabled(!run.installing && !run.stopping).on_click(cx.msg(Msg::Stop));
                col = col.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(m::SPACING))
                        .child(div().flex_1().child(StringView::new(if run.installing { "Installing: this can’t be stopped now." } else { "" }).dim(true).small(true)))
                        .child(stop),
                );
            }
            Stage::Done(r) => {
                col = col.child(match r {
                    Ok(said) => Banner::new(Tone::Success, if said.contains("downloaded") { "Ready for the restart" } else { "Done" }, said.clone()),
                    Err(e) => Banner::new(Tone::Failure, "It didn’t finish", e.clone()),
                });
                if r.is_ok() && self.restart {
                    col = col.child(StringView::new("Some of what changed takes effect only once the computer starts again.").wrap(true));
                }
                // What it was, still there to read.
                let items = self.items_list(cx);
                col = col.child(div().flex_1().min_h_0().child(crate::well(cx, p.list_background, items)));
                let mut buttons = button_row().child(Button::new("Close").default(!(self.restart && r.is_ok())).on_click(cx.msg(Msg::Close)));
                if self.restart && r.is_ok() {
                    buttons = buttons.child(Button::new("Restart Now").id("restart").default(true).on_click(cx.msg(Msg::Restart)));
                }
                col = col.child(buttons);
            }
        }
        col.into_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// dnf refusing one argument (a whole transaction empty, as \"Update
    /// 517 packages\" was): resolved again without it, and it's said.
    #[test]
    fn what_dnf_refuses_is_left_out_and_the_rest_goes_ahead() {
        let plan = Plan { title: "Update 3 packages".into(), ops: vec![(Op::Upgrade, vec!["a".into(), "b.x86_64".into(), "c".into()])], release: None, offline_only: false, comment: String::new() };
        let once = |p: &Plan| -> Result<((), Resolved), String> {
            let specs = &p.ops[0].1;
            Ok((
                (),
                if specs.iter().any(|s| s == "b.x86_64") {
                    Resolved { items: Vec::new(), result: 2, problems: vec!["Packages for argument 'b.x86_64' available, but installed for a different architecture.".into()] }
                } else {
                    Resolved { items: Vec::new(), result: 0, problems: Vec::new() }
                },
            ))
        };
        let ((), r, left) = resolve_leaving_out(&plan, once).unwrap();
        assert!(r.ok());
        assert_eq!(left.iter().map(|(a, _)| a.as_str()).collect::<Vec<_>>(), ["b.x86_64"]);
        // Everything refused: nothing left to leave out from, said as dnf said it.
        let all = Plan { ops: vec![(Op::Upgrade, vec!["b.x86_64".into()])], ..plan.clone() };
        let ((), r, left) = resolve_leaving_out(&all, once).unwrap();
        assert!(!r.ok() && left.is_empty());
    }
}
