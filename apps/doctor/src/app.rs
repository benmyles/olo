//! Olo Doctor's and Olo Suggestions' window and command line: one
//! program body for both (`Kind`), each its own app. Doctor looks the
//! computer over for what gets in Olo's way (an input remapper grabbing
//! the mice, another desktop's daemons, USB receivers allowed to sleep,
//! Olo's udev rules missing, rtkit down, programs left on an old
//! version), shows what it found and fixes it with a button: each
//! finding's own, or Fix All. Suggestions (suggest.rs) shows what Olo
//! suggests for the best of it, none of it needed, each applied with a
//! button or all with Apply All, and undone with another. What needs
//! root is done by the program run again as root (helper.rs), asked for
//! once.
//!
//! `--report` prints the report as JSON and opens nothing (for agents
//! and scripts); `--fix` (`--apply`) does what the buttons do,
//! `--ignore` and `--unignore` what Ignore and Stop Ignoring do
//! (ignored.rs: set aside, untold, listed last); Doctor's `--watch` is
//! the session's: the look at login and the Wi-Fi healer's news
//! (notify.rs).

use std::sync::OnceLock;

use std::collections::BTreeMap;

use crate::{
    checks::{self, Fix, Report, Severity},
    fix::Step,
    helper::{self, FromHelper, Helper, ToHelper},
    notify, suggest,
};
use olo_kit::{
    ClickEvent, Cx, Element, Hsla, Menu, MenuBar, ParentElement as _, ScrollHandle, Styled as _, View, WindowOptions, canvas, div, icons,
    instance::{self, Launch},
    metrics as m, px, style, theme,
    widgets::{BBox, Button, ScrollView, StringView, heading},
};
use serde::{Deserialize, Serialize};

/// Which app this is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Doctor,
    Suggestions,
}

static KIND: OnceLock<Kind> = OnceLock::new();

fn kind() -> Kind {
    *KIND.get().unwrap_or(&Kind::Doctor)
}

impl Kind {
    fn app_id(self) -> &'static str {
        match self {
            Kind::Doctor => "olo.doctor",
            Kind::Suggestions => "olo.suggestions",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Kind::Doctor => "Olo Doctor",
            Kind::Suggestions => "Olo Suggestions",
        }
    }

    fn program(self) -> &'static str {
        match self {
            Kind::Doctor => "olo-doctor",
            Kind::Suggestions => "olo-suggestions",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Kind::Doctor => "doctor",
            Kind::Suggestions => "suggestions",
        }
    }

    /// What it finds now.
    fn scan(self) -> Report {
        match self {
            Kind::Doctor => checks::look(),
            Kind::Suggestions => suggest::look(),
        }
    }

    /// Its words: Fix All or Apply All, Fixing… or Applying…, Fixed. or
    /// Applied.
    fn all(self) -> &'static str {
        if self == Kind::Doctor { "Fix All" } else { "Apply All" }
    }

    fn doing(self) -> &'static str {
        if self == Kind::Doctor { "Fixing…" } else { "Applying…" }
    }

    fn done(self) -> &'static str {
        if self == Kind::Doctor { "Fixed." } else { "Applied." }
    }

    /// Before the button `id`'s steps run (Suggestions keeps what it
    /// changes, to undo it).
    fn before(self, id: &str) {
        if self == Kind::Suggestions {
            suggest::before(id);
        }
    }
}

enum Msg {
    /// Opened again: look again.
    Again,
    Scanned(Report),
    /// A fix run as this user: the finding and its steps' outcomes.
    Ran(String, Vec<Result<(), String>>),
    /// How a fix run as this user is going (a download's share).
    Progress(String, String),
    Helper(Option<FromHelper>),
    /// What a fix left to do as this user is done (`checks::after_fix`).
    Followed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
enum Outcome {
    Fixing,
    Fixed,
    Failed(String),
}

/// What comes back after an upgrade: what was fixed this time.
#[derive(Default, Serialize, Deserialize)]
struct Saved {
    fixed: Vec<String>,
}

struct Doctor {
    report: Option<Report>,
    scanning: bool,
    /// Another look is due once this one is in (a fix finished meanwhile).
    rescan: bool,
    /// Fixes whose after-step (as this user) is still running.
    following: usize,
    /// By finding id.
    outcomes: BTreeMap<String, Outcome>,
    /// What was fixed and has since gone from the report, by title.
    fixed: Vec<String>,
    helper: Option<Helper>,
    helper_ready: bool,
    /// Root fixes waiting for the helper: run id, finding id, steps.
    queued: Vec<(u64, String, Vec<Step>)>,
    /// Root fixes the helper is running: run id → finding id.
    running: BTreeMap<u64, String>,
    /// How each fix running goes, as the helper last said (a package
    /// download's progress): finding id → its words.
    progress: BTreeMap<String, String>,
    next_run: u64,
    scroll: ScrollHandle,
}

impl Doctor {
    fn scan(&mut self, cx: &mut Cx<Self>) {
        if self.scanning {
            self.rescan = true;
            return;
        }
        self.scanning = true;
        // Off the window's looper: it asks systemd and reads /proc and /sys.
        cx.work(|| kind().scan(), Msg::Scanned);
        cx.invalidate();
    }

    fn scanned(&mut self, report: Report, cx: &mut Cx<Self>) {
        self.scanning = false;
        // What was fixed and isn't found any more is listed as fixed;
        // what failed or is still found keeps its outcome.
        let found = |id: &str| report.find(id).is_some();
        let old = self.report.take();
        self.outcomes.retain(|id, o| {
            if found(id) || *o == Outcome::Fixing {
                return true;
            }
            if *o == Outcome::Fixed
                && let Some(f) = old.as_ref().and_then(|r| r.find(id))
                && !self.fixed.contains(&f.title)
            {
                self.fixed.push(f.title.clone());
            }
            false
        });
        self.report = Some(report);
        if std::mem::take(&mut self.rescan) {
            self.scan(cx);
        }
        cx.invalidate();
    }

    fn busy(&self) -> bool {
        self.following > 0 || self.outcomes.values().any(|o| *o == Outcome::Fixing)
    }

    /// Fix the findings `ids` (those with a fix): as this user at once,
    /// as root through the helper (one password for them all).
    fn fix(&mut self, ids: &[String], cx: &mut Cx<Self>) {
        let Some(report) = &self.report else { return };
        let mut root = Vec::new();
        for id in ids {
            let Some(fix) = report.action(id) else { continue };
            if self.outcomes.get(id) == Some(&Outcome::Fixing) {
                continue;
            }
            kind().before(id);
            self.outcomes.insert(id.clone(), Outcome::Fixing);
            if fix.root {
                root.push((id.clone(), fix.steps.clone()));
            } else {
                let (to, id, steps) = (cx.messenger(), id.clone(), fix.steps.clone());
                std::thread::spawn(move || {
                    let results = helper::run_steps_with(&steps, &mut |text| {
                        to.send(Msg::Progress(id.clone(), text));
                    });
                    to.send(Msg::Ran(id, results));
                });
            }
        }
        if !root.is_empty() {
            if self.helper.is_none() {
                let to = cx.messenger();
                match Helper::start(helper::way(), move |m| {
                    to.send(Msg::Helper(m));
                }) {
                    Ok(h) => {
                        self.helper = Some(h);
                        self.helper_ready = false;
                    }
                    Err(e) => {
                        for (id, _) in root {
                            self.outcomes.insert(id, Outcome::Failed(format!("The administrator couldn't be asked: {e}.")));
                        }
                        cx.invalidate();
                        return;
                    }
                }
            }
            for (id, steps) in root {
                self.next_run += 1;
                self.queued.push((self.next_run, id, steps));
            }
            self.send_queued();
        }
        cx.invalidate();
    }

    /// Ignore finding `id` (`on`), or stop: kept (ignored.rs), the window
    /// and the tray as it says at once.
    fn ignore(&mut self, id: &str, on: bool, cx: &mut Cx<Self>) {
        if let Err(e) = crate::ignored::set(kind(), id, on) {
            self.outcomes.insert(id.to_owned(), Outcome::Failed(format!("It couldn't be kept: {e}.")));
            return cx.invalidate();
        }
        if let Some(r) = self.report.as_mut() {
            r.move_ignored(id, on);
            if kind() == Kind::Doctor {
                checks::tell_tray(r);
            }
        }
        cx.invalidate();
    }

    fn fix_all(&mut self, cx: &mut Cx<Self>) {
        let ids: Vec<String> = self.report.iter().flat_map(|r| r.fixable()).map(|f| f.id.clone()).collect();
        self.fix(&ids, cx);
    }

    fn send_queued(&mut self) {
        if !self.helper_ready {
            return;
        }
        let Some(h) = &self.helper else { return };
        for (run, id, steps) in self.queued.drain(..) {
            h.send(&ToHelper::Run { id: run, steps });
            self.running.insert(run, id);
        }
    }

    fn finished(&mut self, id: String, results: Vec<Result<(), String>>, cx: &mut Cx<Self>) {
        let outcome = match results.into_iter().find_map(Result::err) {
            Some(e) => Outcome::Failed(e),
            None => Outcome::Fixed,
        };
        let fixed = outcome == Outcome::Fixed;
        self.outcomes.insert(id.clone(), outcome);
        if let Some(then) = crate::checks::after_fix(&id).filter(|_| fixed && kind() == Kind::Doctor) {
            // What the fix leaves to do as the person (off this thread:
            // it asks rtkit), then the look.
            self.following += 1;
            cx.work(then, |()| Msg::Followed);
        } else if !self.busy() {
            self.scan(cx);
        }
        cx.invalidate();
    }

    fn heard(&mut self, m: Option<FromHelper>, cx: &mut Cx<Self>) {
        match m {
            Some(FromHelper::Ready) => {
                self.helper_ready = true;
                self.send_queued();
            }
            Some(FromHelper::Progress { id, text }) => {
                if let Some(finding) = self.running.get(&id) {
                    self.progress.insert(finding.clone(), text);
                }
            }
            Some(FromHelper::Done { id, results }) => {
                if let Some(finding) = self.running.remove(&id) {
                    self.progress.remove(&finding);
                    self.finished(finding, results, cx);
                }
            }
            None => {
                // Refused (no password) or gone: what it had is undone.
                let why = if self.helper_ready { "The administrator's helper quit." } else { "The administrator's password wasn't given." };
                let left: Vec<String> = self.queued.drain(..).map(|(_, id, _)| id).chain(std::mem::take(&mut self.running).into_values()).collect();
                for id in left {
                    self.outcomes.insert(id, Outcome::Failed(why.into()));
                }
                self.helper = None;
                self.helper_ready = false;
            }
        }
        cx.invalidate();
    }

    fn menus(&self) -> Vec<Menu> {
        let fixable = self.report.as_ref().is_some_and(|r| r.fixable().any(|f| self.outcomes.get(&f.id) != Some(&Outcome::Fixing)));
        vec![
            Menu::new("File").item("Check Again", "check").key("r").enabled(!self.scanning).separator().item("Close", "close").key("w").item("Quit", "quit").key("q"),
            Menu::new(if kind() == Kind::Doctor { "Fix" } else { "Apply" }).item(kind().all(), "fix-all").shift_key(if kind() == Kind::Doctor { "f" } else { "a" }).enabled(fixable),
        ]
    }

    /// The summary at the top.
    fn summary(&self) -> (String, String) {
        if kind() == Kind::Suggestions {
            let Some(r) = &self.report else { return ("Looking at what Olo could suggest…".into(), "Nothing here is needed: Olo works as well without.".into()) };
            let n = r.findings.len();
            let title = match n {
                0 => "Everything Olo suggests is applied.".to_owned(),
                1 => "1 suggestion.".to_owned(),
                n => format!("{n} suggestions."),
            };
            let detail = if !self.queued.is_empty() && !self.helper_ready {
                "Waiting for the administrator's password…".to_owned()
            } else if self.busy() {
                "Applying…".to_owned()
            } else {
                "None of these is needed: Olo works as well without them.".to_owned()
            };
            return (title, detail);
        }
        let Some(r) = &self.report else { return ("Looking this computer over…".into(), "Programs, services, devices and Olo's own setup.".into()) };
        let problems = r.problems();
        let advice = r.findings.len() - problems;
        let title = match (problems, advice) {
            (0, 0) => "Everything is as Olo wants it.".to_owned(),
            (p, 0) => format!("{p} {} found.", if p == 1 { "problem" } else { "problems" }),
            (0, a) => format!("{a} {}.", if a == 1 { "suggestion" } else { "suggestions" }),
            (p, a) => format!("{p} {} and {a} {} found.", if p == 1 { "problem" } else { "problems" }, if a == 1 { "suggestion" } else { "suggestions" }),
        };
        let detail = if !self.queued.is_empty() && !self.helper_ready {
            "Waiting for the administrator's password…".to_owned()
        } else if self.busy() {
            "Fixing…".to_owned()
        } else if self.scanning {
            "Looking again…".to_owned()
        } else {
            let n = r.findings.len() + r.fine.len();
            format!("{n} things looked at; {} {} fine.", r.fine.len(), if r.fine.len() == 1 { "is" } else { "are" })
        };
        (title, detail)
    }
}

/// A check mark in a row of its own height, in `color`.
fn mark(color: Hsla) -> Element {
    canvas(move |g, b| {
        let r = style::snap_centred(g, b.center(), 10.0, 8.0);
        style::check_mark(g, r, color);
    })
    .paint_key(style::key(("mark", color.h.to_bits(), color.s.to_bits(), color.l.to_bits())))
    .w(px(m::MARK_SIZE))
    .h(px(m::LABEL_ROW))
    .flex_shrink_0()
}

impl View for Doctor {
    type Message = Msg;

    fn message(&mut self, msg: Msg, cx: &mut Cx<Self>) {
        match msg {
            Msg::Again => self.scan(cx),
            Msg::Scanned(r) => self.scanned(r, cx),
            Msg::Followed => {
                self.following = self.following.saturating_sub(1);
                if !self.busy() {
                    self.scan(cx);
                }
            }
            Msg::Ran(id, results) => {
                self.progress.remove(&id);
                self.finished(id, results, cx)
            }
            Msg::Progress(id, text) => {
                if self.outcomes.get(&id) == Some(&Outcome::Fixing) {
                    self.progress.insert(id, text);
                    cx.invalidate();
                }
            }
            Msg::Helper(m) => self.heard(m, cx),
        }
    }

    fn command(&mut self, command: &str, cx: &mut Cx<Self>) -> bool {
        match command {
            "check" => self.scan(cx),
            "fix-all" => self.fix_all(cx),
            "close" | "quit" => olo_kit::quit(),
            _ => return false,
        }
        true
    }

    /// What comes back after an upgrade: what was fixed this time.
    fn save_state(&mut self, _: &mut Cx<Self>) -> Option<String> {
        serde_json::to_string(&Saved { fixed: self.fixed.clone() }).ok()
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let p = theme::palette();
        let bar = MenuBar::new(cx, self.menus());
        let (title, detail) = self.summary();
        let fixable = self.report.as_ref().is_some_and(|r| r.fixable().any(|f| self.outcomes.get(&f.id) != Some(&Outcome::Fixing)));
        let icon = canvas(|g, b| {
            icons::paint(g, style::snap_centred(g, b.center(), 32.0, 32.0), kind().icon());
        })
        .paint_key(style::key("doctor-icon"))
        .size(px(32.0))
        .flex_shrink_0();
        let header = div()
            .flex()
            .items_center()
            .gap(px(m::SPACING + 2.0))
            .child(icon)
            .child(div().flex_1().min_w_0().flex().flex_col().child(StringView::new(title).bold(true).height(m::LABEL_ROW + 2.0)).child(StringView::new(detail).dim(true).height(m::LABEL_ROW + 2.0)))
            .child(Button::new("Check Again").enabled(!self.scanning).on_click(cx.listener(|d: &mut Doctor, _: &ClickEvent, cx| d.scan(cx))))
            .child(Button::new(kind().all()).default(fixable).enabled(fixable).on_click(cx.listener(|d: &mut Doctor, _: &ClickEvent, cx| d.fix_all(cx))));
        let mut list = div().flex().flex_col().gap(px(m::SPACING)).pb(px(m::SPACING)).pr(px(m::SPACING));
        if let Some(r) = &self.report {
            for f in &r.findings {
                let outcome = self.outcomes.get(&f.id);
                let status = match outcome {
                    // (What it's doing now, while it says: a download.)
                    Some(Outcome::Fixing) => StringView::new(self.progress.get(&f.id).cloned().unwrap_or_else(|| kind().doing().to_owned())).wrap(true).dim(true),
                    Some(Outcome::Fixed) => StringView::new(kind().done()).color(style::c(p.success)),
                    Some(Outcome::Failed(e)) => StringView::new(e.clone()).wrap(true).color(style::c(p.failure)),
                    None if f.severity == Severity::Problem => StringView::new("Gets in Olo's way").color(style::c(p.failure)),
                    None => StringView::new("Suggested").dim(true),
                };
                let mut row = div().flex().items_center().gap(px(m::SPACING)).child(div().flex_1().min_w_0().child(status));
                // Not wanted: set aside, quietly (ignored.rs).
                {
                    let id = f.id.clone();
                    row = row.child(
                        Button::new("Ignore")
                            .id(format!("ignore-{}", f.id))
                            .enabled(outcome != Some(&Outcome::Fixing))
                            .on_click(cx.listener(move |d: &mut Doctor, _: &ClickEvent, cx| d.ignore(&id, true, cx))),
                    );
                }
                match &f.fix {
                    Some(fix) => {
                        let id = f.id.clone();
                        let label = if matches!(outcome, Some(Outcome::Failed(_))) { "Try Again".to_owned() } else { fix.label.clone() };
                        row = row.child(
                            Button::new(label)
                                .id(format!("fix-{}", f.id))
                                .enabled(outcome != Some(&Outcome::Fixing))
                                .on_click(cx.listener(move |d: &mut Doctor, _: &ClickEvent, cx| d.fix(std::slice::from_ref(&id), cx))),
                        );
                    }
                    None => row = row.child(StringView::new(if kind() == Kind::Doctor { "Fixed by hand" } else { "Not yet" }).dim(true)),
                }
                list = list.child(BBox::new(f.title.clone()).child(StringView::new(f.detail.clone()).wrap(true)).child(row));
            }
            let fine_ink = style::c(p.success);
            if !self.fixed.is_empty() {
                list = list.child(div().pt(px(m::TIGHT)).child(heading(if kind() == Kind::Doctor { "Fixed" } else { "Applied" })));
                for t in &self.fixed {
                    list = list.child(div().flex().gap(px(m::MARK_GAP)).child(mark(fine_ink)).child(StringView::new(t.clone()).height(m::LABEL_ROW)));
                }
            }
            if !r.fine.is_empty() {
                list = list.child(div().pt(px(m::TIGHT)).child(heading(if kind() == Kind::Doctor { "As It Should Be" } else { "Already Applied" })));
                for f in &r.fine {
                    // (Its detail a step under it: a pair, not one block.)
                    let mut text = div().flex_1().min_w_0().flex().flex_col().gap(px(2.0)).child(StringView::new(f.title.clone()).height(m::LABEL_ROW));
                    if !f.detail.is_empty() {
                        text = text.child(StringView::new(f.detail.clone()).wrap(true).dim(true));
                    }
                    let mut line = div().flex().items_start().gap(px(m::MARK_GAP)).child(mark(fine_ink)).child(text);
                    // What's applied can be undone.
                    if let Some(u) = r.undo.iter().find(|u| u.title == f.title) {
                        let outcome = self.outcomes.get(&u.id);
                        let id = u.id.clone();
                        let label = match outcome {
                            Some(Outcome::Fixing) => "Undoing…".to_owned(),
                            Some(Outcome::Failed(_)) => "Try Again".to_owned(),
                            _ => u.fix.label.clone(),
                        };
                        line = line.child(
                            Button::new(label)
                                .id(format!("fix-{}", u.id))
                                .enabled(outcome != Some(&Outcome::Fixing))
                                .on_click(cx.listener(move |d: &mut Doctor, _: &ClickEvent, cx| d.fix(std::slice::from_ref(&id), cx))),
                        );
                        if let Some(Outcome::Failed(e)) = outcome {
                            line = line.child(StringView::new(e.clone()).wrap(true).color(style::c(p.failure)));
                        }
                    }
                    list = list.child(line);
                }
            }
            // Last and quiet: what the person said to ignore, each still
            // fixable (its own button) or to be shown again.
            if !r.ignored.is_empty() {
                list = list.child(div().pt(px(m::TIGHT)).child(heading("Ignored")));
                for f in &r.ignored {
                    let outcome = self.outcomes.get(&f.id);
                    let mut line = div().flex().items_center().gap(px(m::SPACING)).child(div().flex_1().min_w_0().child(StringView::new(f.title.clone()).dim(true).wrap(true)));
                    match outcome {
                        Some(Outcome::Fixing) => {
                            line = line.child(StringView::new(self.progress.get(&f.id).cloned().unwrap_or_else(|| kind().doing().to_owned())).dim(true));
                        }
                        Some(Outcome::Failed(e)) => line = line.child(StringView::new(e.clone()).wrap(true).color(style::c(p.failure))),
                        _ => {}
                    }
                    let id = f.id.clone();
                    line = line.child(
                        Button::new("Stop Ignoring")
                            .id(format!("unignore-{}", f.id))
                            .enabled(outcome != Some(&Outcome::Fixing))
                            .on_click(cx.listener(move |d: &mut Doctor, _: &ClickEvent, cx| d.ignore(&id, false, cx))),
                    );
                    if let Some(fix) = &f.fix {
                        let id = f.id.clone();
                        let label = if matches!(outcome, Some(Outcome::Failed(_))) { "Try Again".to_owned() } else { fix.label.clone() };
                        line = line.child(
                            Button::new(label)
                                .id(format!("fix-{}", f.id))
                                .enabled(outcome != Some(&Outcome::Fixing))
                                .on_click(cx.listener(move |d: &mut Doctor, _: &ClickEvent, cx| d.fix(std::slice::from_ref(&id), cx))),
                        );
                    }
                    list = list.child(line);
                }
            }
        }
        div().size_full().flex().flex_col().child(bar).child(
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .gap(px(m::SPACING + 2.0))
                .p(px(m::WINDOW_INSET))
                .child(header)
                .child(div().flex_1().min_h_0().child(ScrollView::new(&self.scroll).border(false).child(list))),
        )
    }
}

/// The program: Doctor's or Suggestions'.
pub fn run(which: Kind) {
    let _ = KIND.set(which);
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--helper") => helper::serve(),
        Some("--notify") if which == Kind::Doctor => return notify::run(),
        Some("--watch") if which == Kind::Doctor => notify::watch(),
        Some("--report") => {
            println!("{}", serde_json::to_string_pretty(&which.scan()).unwrap_or_default());
            return;
        }
        Some("--fix" | "--apply") => std::process::exit(fix_from_the_command_line(&args[1..])),
        Some("--fans") if which == Kind::Doctor => std::process::exit(crate::fans::from_the_command_line(&args[1..])),
        Some("--scheduler") if which == Kind::Doctor => std::process::exit(crate::scheduler::from_the_command_line(&args[1..])),
        Some(flag @ ("--ignore" | "--unignore")) => std::process::exit(ignore_from_the_command_line(flag == "--ignore", &args[1..])),
        Some("--crashes") if which == Kind::Doctor => return crate::crashes::print(args.get(1).is_some_and(|a| a == "--json")),
        _ => {}
    }
    let app_id = which.app_id();
    if instance::forward(app_id, &args) {
        return;
    }
    olo_kit::run(app_id, move |app| {
        instance::listen(app_id, olo_kit::proxy());
        let saved: Saved = olo_kit::restored_state().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        let window = olo_kit::open_window(WindowOptions::new(which.title(), 600.0, 560.0).min_size(460.0, 360.0).keep_size(which.title()), move |cx| {
            let mut doctor = Doctor {
                report: None,
                scanning: false,
                rescan: false,
                following: 0,
                outcomes: BTreeMap::new(),
                fixed: saved.fixed,
                helper: None,
                helper_ready: false,
                queued: Vec::new(),
                running: BTreeMap::new(),
                progress: BTreeMap::new(),
                next_run: 0,
                scroll: ScrollHandle::new(),
            };
            doctor.scan(cx);
            doctor
        });
        // Opened again: look again, and come forward.
        olo_kit::message_handler(app, move |message, _| {
            if message.downcast_ref::<Launch>().is_some() {
                olo_kit::raise(Some(&window));
                window.messenger().send(Msg::Again);
            }
        });
    });
}

/// `olo-doctor --fix [all | ID…]` (`olo-suggestions --apply`): what the
/// window's buttons do, from a terminal or an agent. No IDs: the
/// findings that have a fix (and, for Suggestions, what can be undone),
/// listed; `all` is every finding's fix, never an undo.
/// Steps that need root go through the helper and pkexec as the window's
/// do (polkit asks for the password); under sudo, everything runs here,
/// the scan being the sudo user's. Exit status 0 when everything asked
/// for was fixed.
fn fix_from_the_command_line(ids: &[String]) -> i32 {
    let program = kind().program();
    let report = kind().scan();
    // (id, title, fix, an undo)
    let fixable: Vec<(&str, &str, &Fix, bool)> = report
        .findings
        .iter()
        .filter_map(|f| Some((f.id.as_str(), f.title.as_str(), f.fix.as_ref()?, false)))
        .chain(report.undo.iter().map(|u| (u.id.as_str(), u.title.as_str(), &u.fix, true)))
        .collect();
    // (What's ignored is fixed only when named: listed apart, never in "all".)
    let ignored: Vec<(&str, &str, &Fix, bool)> = report.ignored.iter().filter_map(|f| Some((f.id.as_str(), f.title.as_str(), f.fix.as_ref()?, false))).collect();
    if ids.is_empty() {
        if fixable.is_empty() {
            println!("Nothing to {}.", if kind() == Kind::Doctor { "fix" } else { "apply" });
        }
        for (id, title, fix, _) in &fixable {
            println!("{id}  {title} ({}{})", fix.label, if fix.root { ", as root" } else { "" });
        }
        for (id, title, fix, _) in &ignored {
            println!("{id}  {title} ({}{}; ignored)", fix.label, if fix.root { ", as root" } else { "" });
        }
        return 0;
    }
    let all = ids.iter().any(|i| i == "all");
    let chosen: Vec<(&str, &str, &Fix, bool)> =
        fixable.iter().copied().filter(|(id, _, _, undo)| (all && !undo) || ids.iter().any(|i| i == id)).chain(ignored.iter().copied().filter(|(id, ..)| ids.iter().any(|i| i == id))).collect();
    // One asked for that isn't there (mistyped, an option it doesn't
    // take): nothing is done.
    let unknown: Vec<&String> = ids.iter().filter(|i| *i != "all" && !chosen.iter().any(|(c, ..)| c == i)).collect();
    if !unknown.is_empty() {
        for id in unknown {
            eprintln!("{program}: {id}: no such finding with a fix now");
        }
        eprintln!("{program}: nothing was done");
        return 1;
    }
    // SAFETY: geteuid has no preconditions.
    let root = unsafe { libc::geteuid() } == 0;
    let mut helper: Option<(Helper, std::sync::mpsc::Receiver<Option<FromHelper>>)> = None;
    let mut failed = false;
    for (n, (id, title, fix, _)) in chosen.iter().enumerate() {
        println!("{title}: {}", fix.label);
        kind().before(id);
        let results = if !fix.root || root {
            helper::run_steps_with(&fix.steps, &mut |text| println!("  {text}"))
        } else {
            if helper.is_none() {
                let way = if std::env::var_os("OLO_DOCTOR_HELPER").is_some() { helper::way() } else { helper::Way::Pkexec };
                let (tx, rx) = std::sync::mpsc::channel();
                match Helper::start(way, move |m| _ = tx.send(m)) {
                    Ok(h) => {
                        // Ready once the password is given.
                        if !matches!(rx.recv(), Ok(Some(FromHelper::Ready))) {
                            eprintln!("{program}: root wasn't given");
                            return 1;
                        }
                        helper = Some((h, rx));
                    }
                    Err(e) => {
                        eprintln!("{program}: can't run as root: {e}");
                        return 1;
                    }
                }
            }
            let (h, rx) = helper.as_ref().unwrap();
            h.send(&ToHelper::Run { id: n as u64, steps: fix.steps.clone() });
            loop {
                match rx.recv() {
                    Ok(Some(FromHelper::Done { id, results })) if id == n as u64 => break results,
                    Ok(Some(FromHelper::Progress { text, .. })) => println!("  {text}"),
                    Ok(Some(_)) => continue,
                    _ => break vec![Err("the root helper stopped".into())],
                }
            }
        };
        for (step, r) in fix.steps.iter().zip(&results) {
            match r {
                Ok(()) => println!("  {}: done", step.describe()),
                Err(e) => {
                    println!("  {}: {e}", step.describe());
                    failed = true;
                }
            }
        }
    }
    i32::from(failed)
}

/// `olo-doctor --ignore ID…` (`--unignore`; `olo-suggestions` too): what
/// the window's Ignore and Stop Ignoring do, for agents and scripts. No
/// IDs: what's ignored, listed. Each ID is kept whether or not it's found
/// now (a finding can come and go).
fn ignore_from_the_command_line(on: bool, ids: &[String]) -> i32 {
    if ids.is_empty() {
        for id in crate::ignored::of(kind()) {
            println!("{id}");
        }
        return 0;
    }
    for id in ids {
        if let Err(e) = crate::ignored::set(kind(), id, on) {
            eprintln!("{}: {id}: {e}", kind().program());
            return 1;
        }
        println!("{id}: {}", if on { "ignored" } else { "no longer ignored" });
    }
    // (The tray's bag as it now says.)
    if kind() == Kind::Doctor {
        checks::look();
    }
    0
}
