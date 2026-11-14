//! Olo Doctor's window and command line. Doctor looks the computer over
//! for what gets in Olo's way (an input remapper grabbing the mice,
//! another desktop's daemons, USB receivers allowed to sleep, Olo's udev
//! rules missing, rtkit down, programs left on an old version) and what
//! Olo suggests for the best of it (suggest.rs: brush as the login shell,
//! programs worth having; once an app of its own, Olo Suggestions), and
//! shows it as one list, most pressing first, each item tagged with its
//! kind (Critical, Problem, Recommended, Suggestion) and fixed or applied
//! with its button, or Fix All (all but the suggestions). Any of it can
//! be ignored. What needs root is done by the program run again as root
//! (helper.rs), asked for once.
//!
//! `--report` prints the report as JSON and opens nothing (for agents
//! and scripts); `--fix` (`--apply`) does what the buttons do,
//! `--ignore` and `--unignore` what Ignore and Stop Ignoring do
//! (ignored.rs: set aside, untold, listed last); `--watch` is the
//! session's: the look at login and the Wi-Fi healer's news
//! (notify.rs).

use std::collections::BTreeMap;

use crate::{
    checks::{self, Finding, Fix, Report, Severity},
    fix::Step,
    helper::{self, FromHelper, Helper, ToHelper},
    notify,
};
use olo_kit::{
    ClickEvent, Cx, Element, Hsla, IntoElement as _, Menu, MenuBar, ParentElement as _, ScrollHandle, Styled as _, View, WindowOptions, canvas, div, icons,
    instance::{self, Launch},
    metrics as m, px, style, theme,
    widgets::{Button, LinkButton, PaneSwitch, ScrollView, StringView, Tag},
};
use serde::{Deserialize, Serialize};

const APP_ID: &str = "olo.doctor";
const TITLE: &str = "Olo Doctor";
const PROGRAM: &str = "olo-doctor";

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

/// What comes back after an upgrade: what was fixed this time, and
/// which sections are open.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Saved {
    fixed: Vec<String>,
    show_fine: Option<bool>,
    show_fixed: Option<bool>,
    show_ignored: bool,
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
    /// What's fine shown (its latch): None as it comes, shown only while
    /// nothing is found.
    show_fine: Option<bool>,
    /// What was fixed this time shown (its latch; open as it comes).
    show_fixed: bool,
    /// What's ignored shown (its latch; folded as it comes: it isn't to
    /// draw attention).
    show_ignored: bool,
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

/// A kind's tag colour: critical red, a problem orange, what's
/// recommended blue, a suggestion green.
fn tag_fill(s: Severity, p: &olo_theme::Palette) -> olo_theme::Rgba {
    p.hue(match s {
        Severity::Critical => olo_theme::Hue::Red,
        Severity::Problem => olo_theme::Hue::Orange,
        Severity::Advice => olo_theme::Hue::Blue,
        Severity::Suggestion => olo_theme::Hue::Green,
    })
}

/// "1 critical", "2 problems", "3 recommendations", "1 suggestion"
/// (`alone`: "1 critical problem").
fn counted(n: usize, s: Severity, alone: bool) -> String {
    let plural = |one: &str| if n == 1 { one.to_owned() } else { format!("{one}s") };
    match s {
        Severity::Critical if alone => format!("{n} critical {}", plural("problem")),
        Severity::Critical => format!("{n} critical"),
        Severity::Problem => format!("{n} {}", plural("problem")),
        Severity::Advice => format!("{n} {}", plural("recommendation")),
        Severity::Suggestion => format!("{n} {}", plural("suggestion")),
    }
}

/// "a, b and c".
fn and_list(parts: &[String]) -> String {
    match parts {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// What a fix of `s` is doing and has done: "Fixing…" and "Fixed.", a
/// suggestion's "Applying…" and "Applied.".
fn words(s: Option<Severity>) -> (&'static str, &'static str) {
    if s == Some(Severity::Suggestion) { ("Applying…", "Applied.") } else { ("Fixing…", "Fixed.") }
}

impl Doctor {
    fn scan(&mut self, cx: &mut Cx<Self>) {
        if self.scanning {
            self.rescan = true;
            return;
        }
        self.scanning = true;
        // Off the window's looper: it asks systemd and reads /proc and /sys.
        cx.work(checks::look, Msg::Scanned);
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

    /// Fix the findings `ids` (those with a fix, or an undo): as this
    /// user at once, as root through the helper (one password for them
    /// all).
    fn fix(&mut self, ids: &[String], cx: &mut Cx<Self>) {
        let Some(report) = &self.report else { return };
        let mut root = Vec::new();
        for id in ids {
            let Some(fix) = report.action(id) else { continue };
            if self.outcomes.get(id) == Some(&Outcome::Fixing) {
                continue;
            }
            crate::suggest::before(id);
            self.outcomes.insert(id.clone(), Outcome::Fixing);
            if crate::demo::on() {
                let (to, id) = (cx.messenger(), id.clone());
                std::thread::spawn(move || {
                    let results = crate::demo::fix(&id);
                    to.send(Msg::Ran(id, results));
                });
            } else if fix.root {
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
        if let Err(e) = crate::ignored::set(id, on) {
            self.outcomes.insert(id.to_owned(), Outcome::Failed(format!("It couldn't be kept: {e}.")));
            return cx.invalidate();
        }
        if let Some(r) = self.report.as_mut() {
            r.move_ignored(id, on);
            r.sort();
            checks::tell_tray(r);
        }
        cx.invalidate();
    }

    /// Fix All: every finding's fix but the suggestions'.
    fn fix_all(&mut self, cx: &mut Cx<Self>) {
        let ids: Vec<String> = self.report.iter().flat_map(|r| r.fixable()).map(|f| f.id.clone()).collect();
        self.fix(&ids, cx);
    }

    /// Apply All Suggestions.
    fn apply_all(&mut self, cx: &mut Cx<Self>) {
        let ids: Vec<String> = self.report.iter().flat_map(|r| r.applicable()).map(|f| f.id.clone()).collect();
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
        if let Some(then) = crate::checks::after_fix(&id).filter(|_| fixed) {
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

    /// Whether Fix All (`suggestions`: Apply All Suggestions) has
    /// something to do.
    fn can_fix_all(&self, suggestions: bool) -> bool {
        self.report.as_ref().is_some_and(|r| {
            let mut those: Box<dyn Iterator<Item = &Finding>> = if suggestions { Box::new(r.applicable()) } else { Box::new(r.fixable()) };
            those.any(|f| self.outcomes.get(&f.id) != Some(&Outcome::Fixing))
        })
    }

    fn menus(&self) -> Vec<Menu> {
        vec![
            // (What it checks most: the power modes and the scheduler.)
            Menu::new("File").app_items(&["performance", "power"]).item("Check Again", "check").key("r").enabled(!self.scanning).close_quit(),
            Menu::new("Fix").item("Fix All", "fix-all").shift_key("f").enabled(self.can_fix_all(false)).item("Apply All Suggestions", "apply-all").shift_key("a").enabled(self.can_fix_all(true)),
        ]
    }

    /// The summary at the top: what was found, by kind ("1 critical, 2
    /// problems and 3 suggestions."), and how it goes.
    fn summary(&self) -> (String, String) {
        let Some(r) = &self.report else { return ("Looking this computer over…".into(), "Programs, services, devices, Olo's own setup, and what Olo suggests.".into()) };
        let kinds: Vec<(Severity, usize)> = [Severity::Critical, Severity::Problem, Severity::Advice, Severity::Suggestion].into_iter().map(|s| (s, r.count(s))).filter(|(_, n)| *n > 0).collect();
        let parts: Vec<String> = kinds.iter().map(|&(s, n)| counted(n, s, kinds.len() == 1)).collect();
        let title = if parts.is_empty() { "Everything is as Olo wants it.".to_owned() } else { format!("{}.", and_list(&parts)) };
        let detail = if !self.queued.is_empty() && !self.helper_ready {
            "Waiting for the administrator's password…".to_owned()
        } else if self.busy() {
            let suggestion = self.outcomes.iter().filter(|(_, o)| **o == Outcome::Fixing).all(|(id, _)| r.find(id).is_some_and(|f| f.severity == Severity::Suggestion));
            words(suggestion.then_some(Severity::Suggestion)).0.to_owned()
        } else if self.scanning {
            "Looking again…".to_owned()
        } else {
            let n = r.findings.len() + r.fine.len() + r.ignored.len();
            let mut d = format!("{n} things looked at; {} {} fine", r.fine.len(), if r.fine.len() == 1 { "is" } else { "are" });
            if !r.ignored.is_empty() {
                d.push_str(&format!(", {} ignored", r.ignored.len()));
            }
            d.push('.');
            d
        };
        (title, detail)
    }

    /// A finding's card: its kind's tag and its title, what it is, and
    /// its buttons (Ignore, its fix) under it, with how its fix goes.
    fn finding_card(&self, f: &Finding, ink: &style::CardInk, cx: &mut Cx<Self>) -> Element {
        let p = theme::palette();
        let outcome = self.outcomes.get(&f.id);
        let (doing, done) = words(Some(f.severity));
        let status = match outcome {
            // (What it's doing now, while it says: a download.)
            Some(Outcome::Fixing) => Some(StringView::new(self.progress.get(&f.id).cloned().unwrap_or_else(|| doing.to_owned())).wrap(true).color(ink.dim)),
            Some(Outcome::Fixed) => Some(StringView::new(done).color(style::c(p.hue_text(olo_theme::Hue::Green)))),
            Some(Outcome::Failed(e)) => Some(StringView::new(e.clone()).wrap(true).color(style::c(p.failure))),
            None => None,
        };
        let top = div()
            .flex()
            .items_start()
            .gap(px(m::MARK_GAP))
            .child(Tag::new(f.severity.label()).fill(tag_fill(f.severity, &p)).beside_paragraph(style::PLAIN_SIZE))
            .child(div().flex_1().min_w_0().child(StringView::new(f.title.clone()).bold(true).wrap(true).color(ink.text)));
        let mut buttons = div().flex().items_center().gap(px(m::SPACING)).child(div().flex_1().min_w_0().children(status));
        // Not wanted: set aside, quietly (ignored.rs). A link, so the
        // fix is what stands out.
        {
            let id = f.id.clone();
            buttons = buttons.child(
                LinkButton::new("Ignore")
                    .id(format!("ignore-{}", f.id))
                    .enabled(outcome != Some(&Outcome::Fixing))
                    .on_click(cx.listener(move |d: &mut Doctor, _: &ClickEvent, cx| d.ignore(&id, true, cx))),
            );
        }
        if let Some(fix) = &f.fix {
            let id = f.id.clone();
            let label = if matches!(outcome, Some(Outcome::Failed(_))) { "Try Again".to_owned() } else { fix.label.clone() };
            buttons = buttons.child(
                Button::new(label)
                    .id(format!("fix-{}", f.id))
                    .enabled(outcome != Some(&Outcome::Fixing))
                    .on_click(cx.listener(move |d: &mut Doctor, _: &ClickEvent, cx| d.fix(std::slice::from_ref(&id), cx))),
            );
        }
        card(Some(f.title.clone())).gap(px(m::TIGHT)).child(top).child(StringView::new(f.detail.clone()).wrap(true).color(ink.text)).child(div().pt(px(m::TIGHT)).child(buttons))
    }
}

/// A card (a finding, or a group of lines), on the list's gutter; named
/// for agents by `name` (a finding's title).
fn card(name: Option<String>) -> Element {
    let paper = canvas(move |g, b| {
        style::card(g, b, &theme::palette(), false, false);
        if let Some(n) = &name {
            g.agent(olo_kit::agent::Node { role: "Group", name: Some(n.clone().into()), bounds: b, enabled: true, ..Default::default() });
        }
    })
    .paint_key(0)
    .absolute()
    .inset_0();
    div().relative().flex().flex_col().p(px(m::CARD_INSET)).child(paper)
}

/// A section's latch on the gutter ("As It Should Be (12)"), folding
/// what's under it: `set` keeps whether it's open.
fn latch(label: String, id: &'static str, open: bool, set: fn(&mut Doctor, bool), cx: &mut Cx<Doctor>) -> Element {
    // (Its latch over the cards' marks and tags, its label over their
    // lines' text.)
    div().pl(px(m::CARD_INSET)).child(PaneSwitch::new(label, open).id(id).on_toggle(cx.listener(move |d: &mut Doctor, open: &bool, cx| {
        set(d, *open);
        cx.invalidate();
    })))
}

/// A line in a card: its marker (a check mark, a tag) level with the
/// first line of its text, and what acts on it (`side`) at the right,
/// centred on the whole.
fn row(marker: Element, text: Element, side: Option<Element>) -> Element {
    let main = div().flex_1().min_w_0().flex().items_start().gap(px(m::MARK_GAP)).child(marker).child(div().flex_1().min_w_0().child(text));
    div().flex().items_center().gap(px(m::SPACING)).child(main).children(side.map(|s| s.flex_shrink_0()))
}

/// A check mark in `color`, level with the first line of a paragraph
/// beside it.
fn mark(color: Hsla) -> Element {
    let middle = style::paragraph_caps_middle(style::PLAIN_SIZE);
    canvas(move |g, b| {
        let r = style::snap_centred(g, olo_kit::point(b.center().x, b.top() + px(middle)), 10.0, 8.0);
        style::check_mark(g, r, color);
    })
    .paint_key(style::key(("mark", color.h.to_bits(), color.s.to_bits(), color.l.to_bits())))
    .w(px(m::MARK_SIZE))
    .h(px(style::line_row(style::PLAIN_SIZE)))
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
            "apply-all" => self.apply_all(cx),
            "close" | "quit" => olo_kit::quit(),
            _ => return false,
        }
        true
    }

    /// What comes back after an upgrade: what was fixed this time.
    fn save_state(&mut self, _: &mut Cx<Self>) -> Option<String> {
        serde_json::to_string(&Saved { fixed: self.fixed.clone(), show_fine: self.show_fine, show_fixed: Some(self.show_fixed), show_ignored: self.show_ignored }).ok()
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let p = theme::palette();
        let ink = style::card_ink(&p);
        let bar = MenuBar::new(cx, self.menus());
        let (title, detail) = self.summary();
        let fixable = self.can_fix_all(false);
        let icon = canvas(|g, b| {
            icons::paint(g, style::snap_centred(g, b.center(), 32.0, 32.0), "doctor");
        })
        .paint_key(style::key("doctor-icon"))
        .size(px(32.0))
        .flex_shrink_0();
        let header = div().flex().items_center().gap(px(m::SPACING)).child(icon).child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(StringView::new(title).bold(true).cut(true).height(m::LABEL_ROW + 2.0))
                .child(StringView::new(detail).dim(true).cut(true).height(m::LABEL_ROW + 2.0)),
        );
        // (Under the list, as a dialog's: the summary has the width.)
        let footer = div()
            .flex()
            .items_center()
            .gap(px(m::SPACING))
            .child(Button::new("Check Again").enabled(!self.scanning).on_click(cx.listener(|d: &mut Doctor, _: &ClickEvent, cx| d.scan(cx))))
            .child(div().flex_1())
            .child(Button::new("Fix All").default(fixable).enabled(fixable).on_click(cx.listener(|d: &mut Doctor, _: &ClickEvent, cx| d.fix_all(cx))));
        let mut list = div().flex().flex_col().gap(px(m::SPACING)).p(px(m::SPACING));
        if let Some(r) = &self.report {
            // What's found, most pressing first, each tagged with its kind.
            for f in &r.findings {
                list = list.child(self.finding_card(f, &ink, cx));
            }
            let fine_ink = style::c(p.hue_text(olo_theme::Hue::Green));
            if !self.fixed.is_empty() {
                let open = self.show_fixed;
                list = list.child(latch(format!("Fixed ({})", self.fixed.len()), "fixed", open, |d, open| d.show_fixed = open, cx));
                if open {
                    let mut lines = card(None).gap(px(m::TIGHT));
                    for t in &self.fixed {
                        lines = lines.child(row(mark(fine_ink), StringView::new(t.clone()).wrap(true).color(ink.text).into_element(), None));
                    }
                    list = list.child(lines);
                }
            }
            // What's as it should be: folded away while something is
            // found (it's the long part), open while nothing is.
            if !r.fine.is_empty() {
                let open = self.show_fine.unwrap_or(r.findings.is_empty());
                list = list.child(latch(format!("As It Should Be ({})", r.fine.len()), "as-it-should-be", open, |d, open| d.show_fine = Some(open), cx));
                if open {
                    let mut lines = card(None).gap(px(m::STACK));
                    for f in &r.fine {
                        // (Its detail a step under it: a pair, not one block.)
                        let mut text = div().flex().flex_col().gap(px(2.0)).child(StringView::new(f.title.clone()).wrap(true).color(ink.text));
                        if !f.detail.is_empty() {
                            text = text.child(StringView::new(f.detail.clone()).wrap(true).color(ink.dim));
                        }
                        // What's applied can be undone.
                        let side = r.undo.iter().find(|u| u.title == f.title).map(|u| {
                            let outcome = self.outcomes.get(&u.id);
                            let id = u.id.clone();
                            let label = match outcome {
                                Some(Outcome::Fixing) => "Undoing…".to_owned(),
                                Some(Outcome::Failed(_)) => "Try Again".to_owned(),
                                _ => u.fix.label.clone(),
                            };
                            let mut side = div().flex().flex_col().items_end().gap(px(m::TIGHT)).child(
                                Button::new(label)
                                    .id(format!("fix-{}", u.id))
                                    .enabled(outcome != Some(&Outcome::Fixing))
                                    .on_click(cx.listener(move |d: &mut Doctor, _: &ClickEvent, cx| d.fix(std::slice::from_ref(&id), cx))),
                            );
                            if let Some(Outcome::Failed(e)) = outcome {
                                side = side.child(StringView::new(e.clone()).color(style::c(p.failure)));
                            }
                            side
                        });
                        lines = lines.child(row(mark(fine_ink), text, side));
                    }
                    list = list.child(lines);
                }
            }
            // Last, folded and quiet: what the person said to ignore (its
            // tag grey), each still fixable (its own button) or to be
            // shown again.
            if !r.ignored.is_empty() {
                let open = self.show_ignored;
                list = list.child(latch(format!("Ignored ({})", r.ignored.len()), "ignored", open, |d, open| d.show_ignored = open, cx));
                if open {
                    let mut lines = card(None).gap(px(m::STACK));
                    // (The tags in a column as wide as the widest, so the
                    // titles line up.)
                    let tags = r.ignored.iter().map(|f| style::tag_width(f.severity.label(), style::DETAIL_SIZE, &style::ui_font(olo_kit::FontWeight::BOLD))).fold(0.0, f32::max);
                    for f in &r.ignored {
                        let outcome = self.outcomes.get(&f.id);
                        let mut text = div().flex().flex_col().gap(px(2.0)).child(StringView::new(f.title.clone()).wrap(true).color(ink.dim));
                        match outcome {
                            Some(Outcome::Fixing) => {
                                text = text.child(StringView::new(self.progress.get(&f.id).cloned().unwrap_or_else(|| words(Some(f.severity)).0.to_owned())).wrap(true).color(ink.dim))
                            }
                            Some(Outcome::Failed(e)) => text = text.child(StringView::new(e.clone()).wrap(true).color(style::c(p.failure))),
                            _ => {}
                        }
                        // (Back among the rest with a link, as Ignore set
                        // it aside.)
                        let id = f.id.clone();
                        text = text.child(
                            div().flex().child(
                                LinkButton::new("Stop Ignoring")
                                    .id(format!("unignore-{}", f.id))
                                    .enabled(outcome != Some(&Outcome::Fixing))
                                    .on_click(cx.listener(move |d: &mut Doctor, _: &ClickEvent, cx| d.ignore(&id, false, cx))),
                            ),
                        );
                        let side = f.fix.as_ref().map(|fix| {
                            let id = f.id.clone();
                            let label = if matches!(outcome, Some(Outcome::Failed(_))) { "Try Again".to_owned() } else { fix.label.clone() };
                            Button::new(label)
                                .id(format!("fix-{}", f.id))
                                .enabled(outcome != Some(&Outcome::Fixing))
                                .on_click(cx.listener(move |d: &mut Doctor, _: &ClickEvent, cx| d.fix(std::slice::from_ref(&id), cx)))
                                .into_element()
                        });
                        let tag = div().w(px(tags)).flex_shrink_0().child(Tag::new(f.severity.label()).beside_paragraph(style::PLAIN_SIZE));
                        lines = lines.child(row(tag, text, side));
                    }
                    list = list.child(lines);
                }
            }
        }
        div().size_full().flex().flex_col().child(bar).child(
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .gap(px(m::SPACING))
                .p(px(m::WINDOW_INSET))
                .child(header)
                .child(div().flex_1().min_h_0().child(ScrollView::new(&self.scroll).background(style::card_paper(&p).0).child(list)))
                .child(footer),
        )
    }
}

/// The program.
pub fn run() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--helper") => helper::serve(),
        Some("--notify") => return notify::run(),
        Some("--watch") => notify::watch(),
        Some("--report") => {
            println!("{}", serde_json::to_string_pretty(&checks::look()).unwrap_or_default());
            return;
        }
        Some("--fix" | "--apply") => std::process::exit(fix_from_the_command_line(&args[1..])),
        Some("--fans") => std::process::exit(crate::fans::from_the_command_line(&args[1..])),
        Some("--scheduler") => std::process::exit(crate::scheduler::from_the_command_line(&args[1..])),
        Some(flag @ ("--ignore" | "--unignore")) => std::process::exit(ignore_from_the_command_line(flag == "--ignore", &args[1..])),
        Some("--crashes") => return crate::crashes::print(args.get(1).is_some_and(|a| a == "--json")),
        _ => {}
    }
    if instance::forward(APP_ID, &args) {
        return;
    }
    olo_kit::run(APP_ID, move |app| {
        instance::listen(APP_ID, olo_kit::proxy());
        let saved: Saved = olo_kit::restored_state().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        let window = olo_kit::open_window(WindowOptions::new(TITLE, 600.0, 580.0).min_size(460.0, 360.0).keep_size(TITLE), move |cx| {
            let mut doctor = Doctor {
                report: None,
                scanning: false,
                rescan: false,
                following: 0,
                outcomes: BTreeMap::new(),
                fixed: saved.fixed,
                show_fine: saved.show_fine,
                show_fixed: saved.show_fixed.unwrap_or(true),
                show_ignored: saved.show_ignored,
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

/// `olo-doctor --fix [all | suggestions | ID…]` (`--apply` the same):
/// what the window's buttons do, from a terminal or an agent. No IDs:
/// the findings that have a fix, and what can be undone, listed, each
/// with its kind; `all` is what Fix All fixes (every finding's fix but
/// the suggestions', never an undo), `suggestions` what Apply All
/// Suggestions applies.
/// Steps that need root go through the helper and pkexec as the window's
/// do (polkit asks for the password); under sudo, everything runs here,
/// the scan being the sudo user's. Exit status 0 when everything asked
/// for was fixed.
fn fix_from_the_command_line(ids: &[String]) -> i32 {
    let program = PROGRAM;
    let report = checks::look();
    // (id, title, fix, its kind: None for an undo)
    type Item<'a> = (&'a str, &'a str, &'a Fix, Option<Severity>);
    let fixable: Vec<Item> = report
        .findings
        .iter()
        .filter_map(|f| Some((f.id.as_str(), f.title.as_str(), f.fix.as_ref()?, Some(f.severity))))
        .chain(report.undo.iter().map(|u| (u.id.as_str(), u.title.as_str(), &u.fix, None)))
        .collect();
    // (What's ignored is fixed only when named: listed apart, never in "all".)
    let ignored: Vec<Item> = report.ignored.iter().filter_map(|f| Some((f.id.as_str(), f.title.as_str(), f.fix.as_ref()?, Some(f.severity)))).collect();
    let kind = |s: Option<Severity>| s.map_or("undo", |s| s.label());
    if ids.is_empty() {
        if fixable.is_empty() {
            println!("Nothing to fix.");
        }
        for (id, title, fix, s) in &fixable {
            println!("{id}  [{}] {title} ({}{})", kind(*s), fix.label, if fix.root { ", as root" } else { "" });
        }
        for (id, title, fix, s) in &ignored {
            println!("{id}  [{}] {title} ({}{}; ignored)", kind(*s), fix.label, if fix.root { ", as root" } else { "" });
        }
        return 0;
    }
    let all = ids.iter().any(|i| i == "all");
    let suggestions = ids.iter().any(|i| i == "suggestions");
    let in_all = |s: Option<Severity>| match s {
        None => false,
        Some(Severity::Suggestion) => suggestions,
        Some(_) => all,
    };
    let chosen: Vec<Item> =
        fixable.iter().copied().filter(|(id, _, _, s)| in_all(*s) || ids.iter().any(|i| i == id)).chain(ignored.iter().copied().filter(|(id, ..)| ids.iter().any(|i| i == id))).collect();
    // One asked for that isn't there (mistyped, an option it doesn't
    // take): nothing is done.
    let unknown: Vec<&String> = ids.iter().filter(|i| *i != "all" && *i != "suggestions" && !chosen.iter().any(|(c, ..)| c == i)).collect();
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
        crate::suggest::before(id);
        let results = if crate::demo::on() {
            crate::demo::fix(id)
        } else if !fix.root || root {
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
        // (The demo's, which has no steps.)
        for e in results.iter().skip(fix.steps.len()).filter_map(|r| r.as_ref().err()) {
            println!("  {e}");
            failed = true;
        }
    }
    i32::from(failed)
}

/// `olo-doctor --ignore ID…` (`--unignore`): what the window's Ignore
/// and Stop Ignoring do, for agents and scripts. No
/// IDs: what's ignored, listed. Each ID is kept whether or not it's found
/// now (a finding can come and go).
fn ignore_from_the_command_line(on: bool, ids: &[String]) -> i32 {
    if ids.is_empty() {
        for id in crate::ignored::all() {
            println!("{id}");
        }
        return 0;
    }
    for id in ids {
        if let Err(e) = crate::ignored::set(id, on) {
            eprintln!("{PROGRAM}: {id}: {e}");
            return 1;
        }
        println!("{id}: {}", if on { "ignored" } else { "no longer ignored" });
    }
    // (The tray's bag as it now says.)
    checks::look();
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finding(id: &str, severity: Severity) -> Finding {
        Finding { id: id.into(), severity, title: id.into(), detail: String::new(), fix: Some(Fix { label: "Fix".into(), root: false, steps: vec![] }) }
    }

    /// The summary counts each kind; a critical one alone says what it is.
    #[test]
    fn the_summary_counts_each_kind() {
        let summary = |findings: Vec<Finding>| {
            let r = Report { findings, ..Report::default() };
            let kinds: Vec<(Severity, usize)> = [Severity::Critical, Severity::Problem, Severity::Advice, Severity::Suggestion].into_iter().map(|s| (s, r.count(s))).filter(|(_, n)| *n > 0).collect();
            and_list(&kinds.iter().map(|&(s, n)| counted(n, s, kinds.len() == 1)).collect::<Vec<_>>())
        };
        assert_eq!(summary(vec![finding("a", Severity::Critical)]), "1 critical problem");
        assert_eq!(
            summary(vec![
                finding("a", Severity::Critical),
                finding("b", Severity::Problem),
                finding("c", Severity::Problem),
                finding("d", Severity::Advice),
                finding("e", Severity::Suggestion),
                finding("f", Severity::Suggestion)
            ]),
            "1 critical, 2 problems, 1 recommendation and 2 suggestions"
        );
        assert_eq!(summary(vec![finding("e", Severity::Suggestion)]), "1 suggestion");
        assert_eq!(summary(vec![]), "");
    }

    /// Fix All leaves the suggestions to Apply All Suggestions; a merged
    /// report lists what's most pressing first.
    #[test]
    fn fix_all_leaves_suggestions_and_the_worst_comes_first() {
        let mut r = Report { findings: vec![finding("tool:just", Severity::Suggestion), finding("rtkit", Severity::Problem)], ..Report::default() };
        r.merge(Report { findings: vec![finding("advice", Severity::Advice), finding("backup", Severity::Critical)], ..Report::default() });
        r.sort();
        assert_eq!(r.findings.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(), ["backup", "rtkit", "advice", "tool:just"]);
        assert_eq!(r.fixable().map(|f| f.id.as_str()).collect::<Vec<_>>(), ["backup", "rtkit", "advice"]);
        assert_eq!(r.applicable().map(|f| f.id.as_str()).collect::<Vec<_>>(), ["tool:just"]);
        assert_eq!(r.problems(), 2);
    }
}
