//! Olo Doctor's window and command line. Doctor looks the computer over
//! for what gets in Olo's way (an input remapper grabbing the mice,
//! another desktop's daemons, USB receivers allowed to sleep, Olo's udev
//! rules missing, rtkit down, programs left on an old version) and what
//! Olo suggests for the best of it (suggest.rs: brush as the login shell,
//! programs worth having; once an app of its own, Olo Suggestions), and
//! shows it as one list, most pressing first, each item tagged with its
//! kind (Critical, Problem, Recommended, Suggestion) and fixed or applied
//! with its button, or with the others checked (`olo_kit::chosen`: the
//! problems and recommendations checked to begin with, the suggestions
//! when the person wants them; Choose All, Problems, Suggestions,
//! Recommendations, None; one button saying what it will do, "Fix 3
//! Problems", "Fix 10 and Apply 3"). Any of it can be ignored. What
//! needs root is done by the program run again as root (helper.rs),
//! asked for once.
//!
//! `--report` prints the report as JSON and opens nothing (for agents
//! and scripts); `--fix` (`--apply`) does what the buttons do,
//! `--ignore` and `--unignore` what Ignore and Stop Ignoring do
//! (ignored.rs: set aside, untold, listed last); `--watch` is the
//! session's: the look at login and the Wi-Fi healer's news
//! (notify.rs).

use std::{collections::BTreeMap, time::Duration};

use crate::{
    checks::{self, Finding, Fix, Report, Severity},
    fix::Step,
    helper::{self, FromHelper, Helper, ToHelper},
    notify,
};
use olo_kit::{
    ClickEvent, Cx, Element, Hsla, IntoElement as _, Menu, MenuBar, ParentElement as _, ScrollHandle, Styled as _, View, WindowOptions, canvas,
    chosen::{self, Choice, Chosen, Kind},
    div, icons,
    instance::{self, Launch},
    metrics as m, px, style, theme,
    widgets::{Button, CheckBox, LinkButton, MENU_MARK_ROOM, PaneSwitch, ScrollView, StringView, Tag, button_width, link_button_width},
};
use serde::{Deserialize, Serialize};

const APP_ID: &str = "olo.doctor";
const TITLE: &str = "Olo Doctor";
const PROGRAM: &str = "olo-doctor";
/// The window's least: a third of the smallest screen, either way, as
/// tiling has it beside Deskbar (349 points of content across;
/// CLAUDE.md, "Every tiling layout").
const MIN_WIDTH: f32 = 346.0;
const MIN_HEIGHT: f32 = 220.0;
/// In a window less tall than this (a third of a screen's height), the
/// summary is one line, without the icon: the list has the room.
const SUMMARY_FOLDS: f32 = 300.0;
/// The least of the summary's second part beside its first on that line.
const DETAIL_LEAST: f32 = 80.0;
/// A short window this wide has each card's actions beside its title.
const INLINE_LEAST: f32 = 640.0;

enum Msg {
    /// Opened again: look again.
    Again,
    /// A root fix's `prepare` done: its steps, for the helper.
    Prepared(String, Vec<Step>),
    Scanned(Report),
    /// A fix run as this user: the finding and its steps' outcomes.
    /// A fix's steps ran (as this user): how each went, and what undoes
    /// those that did.
    Ran(String, helper::Ran),
    /// How a fix run as this user is going (a download's share).
    Progress(String, String),
    Helper(Option<FromHelper>),
    /// What a fix left to do as this user is done (`checks::after_fix`).
    Followed,
    /// A fix has been at it a while: how long is said again (`status`).
    Tick,
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
    chosen: Chosen,
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
    /// When each fix under way began its work (its first word, or its
    /// start as this user), for how long it has been at it; and whether
    /// the window is being told again as that grows (`tick`).
    began: BTreeMap<String, std::time::Instant>,
    ticking: bool,
    next_run: u64,
    scroll: ScrollHandle,
    /// What's checked to be fixed or applied together (`chosen`: the
    /// problems to begin with, the suggestions when the person checks
    /// them).
    chosen: Chosen,
    /// Says what's waiting in Olo Doctor (`notify::waiting`) after each
    /// look and each Ignore here: made at the first.
    notes: Option<olo_kit_base::notify::Notifier>,
}

/// What's fixed: the critical and the problems, and what's recommended,
/// checked to begin with (all but the suggestions, as Fix All was). Two
/// kinds of one verb, so the button names what it fixes: "Fix 3
/// Problems", "Fix 7 Recommendations", or both together "Fix 10" (one
/// kind, its button said "Fix 10 Problems" under "3 problems, 8
/// recommendations and 8 suggestions": QA, 2026-11-28).
const PROBLEMS: Kind = Kind { verb: "Fix", one: "Problem", many: "Problems", by_default: true };
const RECOMMENDED: Kind = Kind { verb: "Fix", one: "Recommendation", many: "Recommendations", by_default: true };
/// What's applied, checked when the person wants it (Ben, 2026-11-14:
/// "the fix all button only fixes all problems but there's no way to
/// apply all suggestions. we need a way to do both and either/or").
const SUGGESTED: Kind = Kind { verb: "Apply", one: "Suggestion", many: "Suggestions", by_default: false };
/// The kinds, in the order the Choose links and the button have them.
/// (Suggestions second, as they always were: `choose-kind-1`, a
/// command and a control agents use, is still them; Astra's review,
/// 2026-11-28. Recommendations, new, come last.)
const KINDS: [&Kind; 3] = [&PROBLEMS, &SUGGESTED, &RECOMMENDED];

/// How the footer is set out ([`footer`]): the Choose links over the
/// buttons, or beside them, or folded into "Choose ▾"; and whether, even
/// so, the button (with what's beside it) has a row of its own under
/// Check Again.
#[derive(Debug, PartialEq)]
struct FooterFit {
    links_over: bool,
    links_beside: bool,
    menu: bool,
    under: bool,
}

/// A default button's ring, outside its face: the face is set in by it
/// at the footer's end, so the ring ends where everything else does
/// (Opus's review, 2026-11-28).
const DEFAULT_RING: f32 = 3.0;

/// [`FooterFit`] for a footer `room` wide, `beside` of it beside Check
/// Again; the links `links_w` wide, Choose ▾ `menu_w`, the button `act`
/// (its own width, by the rule every button has, its ring's room after
/// it). One row where it can be: "Fix 10" sized for "Fix 8
/// Recommendations" put it under Check Again in a third, a row of its
/// own; and wider than Check Again by half at the window's own width
/// (Opus's and Neon's reviews, 2026-11-28).
fn footer_fit(room: f32, beside: f32, links_w: f32, menu_w: f32, act: f32) -> FooterFit {
    let (links_over, links_beside, menu) = if links_w + m::SPACING + act <= beside {
        (false, true, false)
    } else if links_w <= room {
        (true, false, false)
    } else {
        (false, false, true)
    };
    let with = act
        + if menu {
            menu_w + m::SPACING
        } else if links_beside {
            links_w + m::SPACING
        } else {
            0.0
        };
    FooterFit { links_over, links_beside, menu, under: with > beside }
}

/// The footer: Check Again at the left; the Choose links (or Choose ▾)
/// and the counted default button at the right, as [`footer_fit`] sets
/// them out. The counts in the order shown ([`SHOWN`]).
fn footer(
    counts: &[chosen::Count<'static>],
    enabled: bool,
    room: f32,
    check_again: Button,
    on_choice: impl Fn(&Choice, &mut olo_kit::WindowCx) + 'static,
    on_act: impl Fn(&ClickEvent, &mut olo_kit::WindowCx) + 'static,
) -> Element {
    let checked: Vec<(&Kind, usize)> = counts.iter().map(|&(k, n, _)| (k, n)).collect();
    let label = chosen::label(&checked);
    let any = counts.iter().any(|(_, n, _)| *n > 0);
    let beside = room - button_width("Check Again", false) - m::SPACING;
    let menu_w = link_button_width(olo_kit::words::tr("Choose")) + MENU_MARK_ROOM;
    let fit = footer_fit(room, beside, chosen::choose_width(counts), menu_w, button_width(&label, false) + DEFAULT_RING);
    let act = div().mr(px(DEFAULT_RING)).child(Button::new(label).id("chosen-act").default(any && enabled).enabled(any && enabled).on_click(on_act));
    let choose = if fit.menu { chosen::choose_menu("chosen", counts, enabled, on_choice) } else { chosen::choose("chosen", counts, enabled, on_choice) };
    let (over, beside_act) = if fit.links_over { (Some(div().flex().justify_end().child(choose).into_element()), None) } else { (None, Some(choose)) };
    let group = div().flex().items_center().gap(px(m::SPACING)).children(beside_act).child(act);
    // (The links a stack's spacing over the buttons: a default
    // button's ring takes some of a tight one.)
    let rows = div().flex().flex_col().gap(px(m::STACK)).children(over);
    if fit.under {
        rows.child(div().flex().child(check_again)).child(div().flex().justify_end().child(group)).into_element()
    } else {
        rows.child(div().flex().items_center().gap(px(m::SPACING)).child(check_again).child(div().flex_1()).child(group)).into_element()
    }
}

/// The kinds in the order the window shows them, most pressing first, as
/// its summary says them ("3 problems, 8 recommendations and 8
/// suggestions"): each one's place in [`KINDS`], whose order the
/// commands keep (`choose-kind-1` is still the suggestions'). Shown in
/// that order, the chooser said "Problems · Suggestions ·
/// Recommendations" (Opus's and Neon's reviews, 2026-11-28).
const SHOWN: [usize; 3] = [0, 2, 1];

/// A choice made among the kinds as shown ([`SHOWN`]), as the commands
/// have it.
fn as_command(choice: Choice) -> Choice {
    match choice {
        Choice::Kind(i) => Choice::Kind(SHOWN.get(i).copied().unwrap_or(i)),
        other => other,
    }
}

/// The Fix menu: the button's command (its words as the button's), then
/// Choose All (⌘A), each kind in the order shown, by its command
/// ([`SHOWN`]), and None: `chosen::menu`'s, its kinds' commands by their
/// place in [`KINDS`], not in what's shown.
fn fix_menu(counts: &[chosen::Count<'static>], enabled: bool) -> Menu {
    let checked: Vec<(&Kind, usize)> = counts.iter().map(|&(k, n, _)| (k, n)).collect();
    let any = counts.iter().any(|(_, n, _)| *n > 0);
    let mut m = Menu::new("Fix").item(chosen::label(&checked), "fix-all").enabled(any && enabled).separator();
    m = m.item("Choose All", "choose-all").key("a").enabled(enabled);
    for (shown, (kind, ..)) in counts.iter().enumerate() {
        m = m.item(format!("Choose {}", kind.many), format!("choose-kind-{}", SHOWN[shown])).enabled(enabled);
    }
    m.item("Choose None", "choose-none").enabled(enabled)
}

fn kind_of(f: &Finding) -> &'static Kind {
    match f.severity {
        Severity::Suggestion => &SUGGESTED,
        Severity::Advice => &RECOMMENDED,
        _ => &PROBLEMS,
    }
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
pub(crate) fn and_list(parts: &[String]) -> String {
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

/// From when a fix says how long it has been at it, and from when that
/// it may never end.
const SAY_HOW_LONG: Duration = Duration::from_secs(15);
const SAY_STILL: Duration = Duration::from_secs(180);

/// What a fix under way says of itself, beside its buttons: what it is
/// doing (`said`, as it last told; `doing` before it told anything),
/// that it waits where another fix as the administrator is ahead of it
/// (they go one at a time), and, once that is worth saying, how long it
/// has been at it. (A fix that called into a driver never came back,
/// and the window said "Fixing…" for hours under a finding whose own
/// fix was only waiting behind it: 2026-11-20.)
fn status(doing: &'static str, said: Option<&str>, waiting: bool, at_it: Option<Duration>) -> String {
    let Some(said) = said else {
        return if waiting { olo_kit::words::tr("Waiting for another fix to finish…").to_owned() } else { olo_kit::words::tr(doing).to_owned() };
    };
    match at_it.filter(|t| *t >= SAY_HOW_LONG) {
        None => said.to_owned(),
        Some(t) => olo_kit::tr!("{} ({})", said, how_long(t)),
    }
}

/// What a fix run as the administrator says once it has been at it so
/// long that it may never end, on a line of its own under the
/// finding's words (those call into the system, and go one at a
/// time): None before then, and of a fix run as this user.
fn still(at_it: Option<Duration>, root: bool) -> Option<String> {
    let t = at_it.filter(|t| root && *t >= SAY_STILL)?;
    Some(olo_kit::tr!("Still going after {}. Other fixes wait until it's done; if it never finishes, restarting the computer stops it.", how_long(t)))
}

/// "20 s", "55 s" (to five seconds), then "1 min", "12 min" (a count
/// that moved every few seconds would move the words' wrap with it).
fn how_long(t: Duration) -> String {
    match t.as_secs() {
        s if s < 60 => olo_kit::tr!("{} s", s / 5 * 5),
        s => olo_kit::tr!("{} min", s / 60),
    }
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
        self.chosen.keep(report.findings.iter().map(|f| f.id.as_str()));
        self.report = Some(report);
        self.tell_waiting();
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
        for id in ids {
            let Some(fix) = report.action(id) else { continue };
            if self.outcomes.get(id) == Some(&Outcome::Fixing) {
                continue;
            }
            self.outcomes.insert(id.clone(), Outcome::Fixing);
            // (As this user it begins at once; as the administrator, when
            // the helper comes to it and says its first word: `heard`.)
            if !fix.root || crate::demo::on() {
                self.began.insert(id.clone(), std::time::Instant::now());
            }
            // On a worker: what's done as the person first (`prepare`:
            // the start-up screen drawn, a suggestion's state kept), then
            // the steps here, or handed to the root helper.
            let (to, id, steps, root, demo) = (cx.messenger(), id.clone(), fix.steps.clone(), fix.root, crate::demo::on());
            std::thread::spawn(move || {
                if demo {
                    let results = crate::demo::fix(&id, &mut |text| {
                        to.send(Msg::Progress(id.clone(), text));
                    });
                    to.send(Msg::Ran(id, helper::Ran { results, ..Default::default() }));
                } else if let Err(e) = crate::checks::prepare(&id) {
                    to.send(Msg::Ran(id, helper::Ran { results: vec![Err(e)], ..Default::default() }));
                } else if root {
                    to.send(Msg::Prepared(id, steps));
                } else {
                    let ran = helper::run_steps_keeping(&steps, &mut |text| {
                        to.send(Msg::Progress(id.clone(), text));
                    });
                    to.send(Msg::Ran(id, ran));
                }
            });
        }
        self.tick(cx);
        cx.invalidate();
    }

    /// While a fix is under way the window is told again every five
    /// seconds, for how long it has been at it (`status`); not once
    /// none is.
    fn tick(&mut self, cx: &mut Cx<Self>) {
        if !self.ticking && self.outcomes.values().any(|o| *o == Outcome::Fixing) {
            self.ticking = true;
            // (As often as what is said changes: by five seconds in a
            // fix's first minute, by the minute after.)
            let young = self.began.is_empty() || self.began.values().any(|t| t.elapsed() < Duration::from_secs(60));
            cx.after(Duration::from_secs(if young { 5 } else { 20 }), Msg::Tick);
        }
    }

    /// Root steps for finding `id`, once prepared: to the helper (started
    /// the first time: one password for all that come while it waits).
    fn as_root(&mut self, id: String, steps: Vec<Step>, cx: &mut Cx<Self>) {
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
                    self.outcomes.insert(id, Outcome::Failed(format!("The administrator couldn't be asked: {e}.")));
                    return cx.invalidate();
                }
            }
        }
        self.next_run += 1;
        self.queued.push((self.next_run, id, steps));
        self.send_queued();
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
        self.tell_waiting();
        cx.invalidate();
    }

    /// What's waiting here, as this report has it, to Deskbar and
    /// Notifications (the watcher says it too, at its own looks). Not the
    /// demo's: it tells nothing.
    fn tell_waiting(&mut self) {
        let Some(r) = &self.report else { return };
        if crate::demo::on() {
            return;
        }
        let n = crate::notify::waiting(r);
        self.notes.get_or_insert_with(|| olo_kit_base::notify::Notifier::with_events("Olo Doctor", "olo-doctor", |_| {})).badge(n);
    }

    /// The findings that can be checked (those with a fix, not being
    /// fixed now).
    fn choosable(&self) -> impl Iterator<Item = &Finding> {
        self.report.iter().flat_map(|r| r.findings.iter()).filter(|f| f.fix.as_ref().is_some_and(|fix| fix.batchable()) && self.outcomes.get(&f.id) != Some(&Outcome::Fixing))
    }

    /// How many of each kind are checked, of how many, in the order shown
    /// ([`SHOWN`]): the button's words, and its width.
    fn counts(&self) -> [chosen::Count<'static>; 3] {
        let n = |kind: &Kind| self.choosable().filter(|f| kind_of(f) == kind && self.chosen.is(&f.id, kind)).count();
        // (Of all there are, those being fixed too: the button keeps its
        // width while they run.)
        let of = |kind: &Kind| self.report.iter().flat_map(|r| r.findings.iter()).filter(|f| f.fix.as_ref().is_some_and(|fix| fix.batchable()) && kind_of(f) == kind).count();
        SHOWN.map(|i| (KINDS[i], n(KINDS[i]), of(KINDS[i])))
    }

    /// The button ("Fix 3 Problems", "Fix 10 and Apply 3"): what's
    /// checked, fixed and applied.
    fn act(&mut self, cx: &mut Cx<Self>) {
        let ids: Vec<String> = self.choosable().filter(|f| self.chosen.is(&f.id, kind_of(f))).map(|f| f.id.clone()).collect();
        self.fix(&ids, cx);
    }

    /// Choose: All, Problems, Suggestions, Recommendations or None.
    fn choose(&mut self, choice: Choice, cx: &mut Cx<Self>) {
        let items: Vec<(String, &'static Kind)> = self.choosable().map(|f| (f.id.clone(), kind_of(f))).collect();
        self.chosen.choose(items.iter().map(|(id, k)| (id.as_str(), *k)), |k| KINDS.iter().position(|o| *o == k).is_some_and(|i| choice.picks(i)));
        cx.invalidate();
    }

    /// Check finding `id` (`on`), or uncheck it.
    fn check(&mut self, id: &str, on: bool, cx: &mut Cx<Self>) {
        if let Some(f) = self.report.as_ref().and_then(|r| r.find(id)) {
            let kind = kind_of(f);
            self.chosen.set(id, kind, on);
        }
        cx.invalidate();
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

    fn finished(&mut self, id: String, ran: helper::Ran, cx: &mut Cx<Self>) {
        let outcome = match ran.results.iter().find_map(|r| r.as_ref().err()) {
            Some(e) => Outcome::Failed(e.clone()),
            None => Outcome::Fixed,
        };
        let fixed = outcome == Outcome::Fixed;
        // What it did is kept with what undoes it (applied.rs); an undo
        // done is forgotten.
        if !crate::demo::on() {
            let fix = self.report.as_ref().and_then(|r| r.action(&id).map(|fix| (r.find(&id).map(|f| f.title.clone()), fix.clone())));
            if let Some(kept) = keep_applied(&id, fix, &ran, fixed)
                && let Err(e) = kept
            {
                tracing::warn!("doctor: what undoes {id} couldn't be kept: {e}");
            }
        }
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
                    self.began.entry(finding.clone()).or_insert_with(std::time::Instant::now);
                    self.progress.insert(finding.clone(), text);
                }
            }
            Some(FromHelper::Done { id, results, undo, kept }) => {
                if let Some(finding) = self.running.remove(&id) {
                    self.progress.remove(&finding);
                    self.began.remove(&finding);
                    self.finished(finding, helper::Ran { results, undo, kept }, cx);
                }
            }
            None => {
                // Refused (no password) or gone: what it had is undone.
                let why = if self.helper_ready { "The administrator's helper quit." } else { "The administrator's password wasn't given." };
                let left: Vec<String> = self.queued.drain(..).map(|(_, id, _)| id).chain(std::mem::take(&mut self.running).into_values()).collect();
                for id in left {
                    self.progress.remove(&id);
                    self.began.remove(&id);
                    self.outcomes.insert(id, Outcome::Failed(why.into()));
                }
                self.helper = None;
                self.helper_ready = false;
            }
        }
        cx.invalidate();
    }

    fn menus(&self) -> Vec<Menu> {
        vec![
            // (What it checks most: the power modes and the scheduler.)
            Menu::new("File").app_items(&["performance", "power"]).item("Check Again", "check").key("r").enabled(!self.scanning).close_quit(),
            // The button's command ("Fix 10 and Apply 3", ⇧⌘F), and
            // Choose All (⌘A), Problems, Suggestions, Recommendations,
            // None.
            fix_menu(&self.counts(), self.report.is_some()),
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
            let doing = words(suggestion.then_some(Severity::Suggestion)).0;
            // (With how long the longest of them has been at it, as its
            // card says: the card may be out of sight.)
            match self.began.values().map(|t| t.elapsed()).max().filter(|t| *t >= SAY_HOW_LONG) {
                Some(t) => olo_kit::tr!("{} ({})", olo_kit::words::tr(doing), how_long(t)),
                None => doing.to_owned(),
            }
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

    /// How the fix of finding `id` under way says it goes: beside its
    /// buttons (`status`), and on a line of its own once it may never
    /// end (`still`).
    fn status_of(&self, id: &str, doing: &'static str) -> (String, Option<String>) {
        // (Waiting: it has told nothing yet, and another fix as the
        // administrator has.)
        let waiting = self.running.values().any(|other| other != id && self.progress.contains_key(other)) || self.queued.iter().any(|(_, queued, _)| queued == id) && !self.running.is_empty();
        let root = self.report.as_ref().and_then(|r| r.action(id)).is_some_and(|fix| fix.root);
        let at_it = self.began.get(id).map(|t| t.elapsed());
        (status(doing, self.progress.get(id).map(String::as_str), waiting, at_it), still(at_it, root))
    }

    /// A finding's card: its kind's tag and its title, what it is, and
    /// its buttons (Ignore, its fix) under it, with how its fix goes.
    /// `inline`: its Ignore and its fix at the right of its title, not
    /// in a row under its words (a short, wide window: each card whole,
    /// with what does it, in the room there is).
    fn finding_card(&self, f: &Finding, ink: &style::CardInk, inline: bool, cx: &mut Cx<Self>) -> Element {
        let p = theme::palette();
        let outcome = self.outcomes.get(&f.id);
        let (doing, done) = words(Some(f.severity));
        // (What a fix under way says is read, not set aside: darker than
        // the dimmed buttons beside it. Both reviewers' looks.)
        let going = {
            let (_, paper) = style::card_paper(&p);
            style::c(olo_theme::mix(paper, p.document_text, 0.72))
        };
        let (said, still) = if outcome == Some(&Outcome::Fixing) { self.status_of(&f.id, doing) } else { (String::new(), None) };
        let still = still.map(|text| StringView::new(text).wrap(true).color(going));
        let status = match outcome {
            // (What it's doing now, while it says: a download; that it
            // waits behind another; how long it has been at it.)
            Some(Outcome::Fixing) => Some(StringView::new(said).wrap(true).color(going)),
            Some(Outcome::Fixed) => Some(StringView::new(done).color(style::c(p.hue_text(olo_theme::Hue::Green)))),
            Some(Outcome::Failed(e)) => Some(StringView::new(curled(e)).wrap(true).color(style::c(p.failure))),
            None => None,
        };
        // Checked to be done with the others (the button under the list),
        // named by its title for agents; a finding with nothing to do
        // keeps the room, so the tags line up.
        let choose = match &f.fix {
            Some(fix) if fix.batchable() => {
                let id = f.id.clone();
                CheckBox::new(curled(&f.title), self.chosen.is(&f.id, kind_of(f)))
                    .id(format!("choose-{}", f.id))
                    .mark_only()
                    .height(style::line_row(style::PLAIN_SIZE))
                    .enabled(outcome != Some(&Outcome::Fixing))
                    .on_toggle(cx.listener(move |d: &mut Doctor, on: &bool, cx| d.check(&id, *on, cx)))
                    .into_element()
            }
            // A fix that is never done with the others (`Fix::batchable`):
            // its mark is there, not to be checked, and says why. (Left
            // empty, the place read as a mark that had failed to draw:
            // Opus's look, 2026-11-21.)
            Some(_) => CheckBox::new(curled(&f.title), false)
                .id(format!("choose-{}", f.id))
                .mark_only()
                .height(style::line_row(style::PLAIN_SIZE))
                .enabled(false)
                .tip("Not fixed with the others: it has its own button.")
                .into_element(),
            None => div().w(px(m::MARK_SIZE)).flex_shrink_0(),
        };
        let top = div()
            .flex()
            .items_start()
            .gap(px(m::MARK_GAP))
            .child(choose)
            .child(Tag::new(f.severity.label()).fill(tag_fill(f.severity, &p)).beside_paragraph(style::PLAIN_SIZE))
            .child(div().flex_1().min_w_0().child(StringView::new(curled(&f.title)).bold(true).wrap(true).color(ink.text)));
        let mut buttons = div().flex().items_center().gap(px(m::SPACING));
        let status = if inline {
            status
        } else {
            buttons = buttons.child(div().flex_1().min_w_0().children(status));
            None
        };
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
            let label = if matches!(outcome, Some(Outcome::Failed(_))) { "Try Again".to_owned() } else { curled(&fix.label) };
            buttons = buttons.child(
                Button::new(label)
                    .id(format!("fix-{}", f.id))
                    .enabled(outcome != Some(&Outcome::Fixing))
                    .on_click(cx.listener(move |d: &mut Doctor, _: &ClickEvent, cx| d.fix(std::slice::from_ref(&id), cx))),
            );
        }
        // (Its paragraphs, a blank line between them in the text, set
        // half a line apart: a whole empty line was as much as the room
        // over the buttons, and the card fell into equal blocks.)
        let detail = div().flex().flex_col().gap(px(m::TIGHT)).children(f.detail.split("\n\n").map(|paragraph| StringView::new(curled(paragraph)).wrap(true).color(ink.text)));
        if inline {
            // (The title's first line on the buttons' middle, as a row of
            // controls reads.)
            let top = top.items_center().child(buttons.flex_shrink_0());
            return card(Some(curled(&f.title))).gap(px(m::TIGHT)).child(top).child(detail).children(status).children(still).into_element();
        }
        // (That a fix may never end is said on a line of its own, the
        // card's whole width, over the row of buttons: beside them it
        // wrapped to three short lines and pushed them off their row.)
        card(Some(curled(&f.title))).gap(px(m::TIGHT)).child(top).child(detail).children(still).child(div().pt(px(m::TIGHT)).child(buttons))
    }
}

/// A card (a finding, or a group of lines), on the list's gutter; named
/// for agents by `name` (a finding's title).
/// Words as the window shows them: an apostrophe between two letters
/// curled (Olo’s, isn’t), and one closing a plural’s (readers’), as on the
/// rest of the desktop. Only what the window draws: the checks' words
/// stay as they are where they are read or written (the files a fix
/// installs, their own errors matched again, `--report` for agents and
/// scripts). (Straight, Doctor's were the only ones left: Opus's review,
/// 2026-11-28.)
pub(crate) fn curled(text: &str) -> String {
    // (Never a word of the catalogs: its translation is found by its
    // English as it is.)
    static KEYS: std::sync::OnceLock<std::collections::HashSet<&'static str>> = std::sync::OnceLock::new();
    if !text.contains('\'') || KEYS.get_or_init(|| olo_kit::words::english().iter().copied().collect()).contains(text) {
        return text.to_owned();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut word_start = 0;
    let mut out = String::with_capacity(text.len() + 8);
    for (i, &c) in chars.iter().enumerate() {
        if c.is_whitespace() {
            word_start = i + 1;
        }
        let before = i.checked_sub(1).map(|b| chars[b]);
        let after = chars.get(i + 1).copied();
        let between = before.is_some_and(char::is_alphabetic) && after.is_some_and(char::is_alphabetic);
        // (A plural's: "readers' " — not the end of a word quoted 'so'.)
        let plural = before == Some('s') && after.is_none_or(|a| a.is_whitespace() || ",.;:)".contains(a)) && chars[word_start] != '\'';
        out.push(if c == '\'' && (between || plural) { '’' } else { c });
    }
    out
}

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
            Msg::Prepared(id, steps) => self.as_root(id, steps, cx),
            Msg::Scanned(r) => self.scanned(r, cx),
            Msg::Followed => {
                self.following = self.following.saturating_sub(1);
                if !self.busy() {
                    self.scan(cx);
                }
            }
            Msg::Ran(id, ran) => {
                self.progress.remove(&id);
                self.began.remove(&id);
                self.finished(id, ran, cx)
            }
            Msg::Tick => {
                self.ticking = false;
                self.tick(cx);
                // (Only where a fix has been at it long enough to say so.)
                if self.began.values().any(|t| t.elapsed() >= SAY_HOW_LONG) {
                    cx.invalidate();
                }
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
            "fix-all" => self.act(cx),
            c if Choice::of_command(c).is_some() => self.choose(Choice::of_command(c).expect("just seen"), cx),
            "close" | "quit" => olo_kit::quit(),
            _ => return false,
        }
        true
    }

    /// What comes back after an upgrade: what was fixed this time.
    fn save_state(&mut self, _: &mut Cx<Self>) -> Option<String> {
        serde_json::to_string(&Saved { fixed: self.fixed.clone(), show_fine: self.show_fine, show_fixed: Some(self.show_fixed), show_ignored: self.show_ignored, chosen: self.chosen.clone() }).ok()
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let p = theme::palette();
        let ink = style::card_ink(&p);
        let bar = MenuBar::of(self, cx, |v, _| v.menus());
        let (title, detail) = self.summary();
        let (title, detail) = (curled(&title), curled(&detail));
        let counts = self.counts();
        let icon = canvas(|g, b| {
            icons::paint(g, style::snap_centred(g, b.center(), 32.0, 32.0), "doctor");
        })
        .paint_key(style::key("doctor-icon"))
        .size(px(32.0))
        .flex_shrink_0();
        let vs = cx.viewport_size();
        let room = f32::from(vs.width) - 2.0 * m::WINDOW_INSET;
        // A short window (a third of a screen high) as wide as a screen:
        // each card's actions beside its title, the card a row shorter.
        let inline = f32::from(vs.height) < SUMMARY_FOLDS && room >= INLINE_LEAST;
        let header = if f32::from(vs.height) >= SUMMARY_FOLDS {
            // (Its lines wrap in a narrow window, never cut: a third of a
            // screen's width has the height for them.)
            div()
                .flex()
                .items_center()
                .gap(px(m::SPACING))
                .child(icon)
                .child(div().flex_1().min_w_0().flex().flex_col().child(StringView::new(title).bold(true).wrap(true)).child(StringView::new(detail).dim(true).wrap(true)))
        } else {
            // (A window a third of a screen high: one line, what's said
            // first whole and the rest cut where it must be; in one narrow
            // too, what's said first alone, cut.)
            let gap = m::TIGHT * 2.0;
            let title_w = f32::from(style::text_width(title.clone(), style::PLAIN_SIZE, style::ui_font(olo_kit::FontWeight::BOLD))).ceil();
            let line = div().flex().items_center().gap(px(gap));
            if title_w + gap + DETAIL_LEAST <= room {
                line.child(div().flex_shrink_0().child(StringView::new(title).bold(true))).child(div().flex_1().min_w_0().child(StringView::new(detail).dim(true).cut(true)))
            } else {
                line.child(div().flex_1().min_w_0().child(StringView::new(title).bold(true).cut(true)))
            }
        };
        // (Under the list, as a dialog's: the summary has the width.)
        // What's checked, done together: Choose All, a kind, None, and the
        // button saying what it will do (olo_kit::chosen); the links over
        // the row where it hasn't room for them beside the buttons.
        let check_again = Button::new("Check Again").enabled(!self.scanning).on_click(cx.listener(|d: &mut Doctor, _: &ClickEvent, cx| d.scan(cx)));
        let footer = footer(
            &counts,
            self.report.is_some(),
            room,
            check_again,
            cx.listener(|d: &mut Doctor, c: &Choice, cx| d.choose(as_command(*c), cx)),
            cx.listener(|d: &mut Doctor, _: &ClickEvent, cx| d.act(cx)),
        );
        let mut list = div().flex().flex_col().gap(px(m::SPACING)).p(px(m::SPACING));
        if let Some(r) = &self.report {
            // What's found, most pressing first, each tagged with its kind.
            for f in &r.findings {
                list = list.child(self.finding_card(f, &ink, inline, cx));
            }
            let fine_ink = style::c(p.hue_text(olo_theme::Hue::Green));
            // What was fixed: each thing applied that can be put back,
            // with Undo (applied.rs: kept from one day to the next), then
            // what was fixed this time that left nothing to undo.
            let applied: Vec<&checks::Undo> = r.undo.iter().filter(|u| crate::applied::of(&u.id).is_some()).collect();
            let plain: Vec<&String> = self.fixed.iter().filter(|t| !applied.iter().any(|u| u.title == **t)).collect();
            if !applied.is_empty() || !plain.is_empty() {
                let open = self.show_fixed;
                list = list.child(latch(format!("Fixed ({})", applied.len() + plain.len()), "fixed", open, |d, open| d.show_fixed = open, cx));
                if open {
                    let mut lines = card(None).gap(px(m::STACK));
                    let kept = crate::applied::all();
                    for u in applied {
                        let outcome = self.outcomes.get(&u.id);
                        let id = u.id.clone();
                        let label = match outcome {
                            Some(Outcome::Fixing) => "Undoing…".to_owned(),
                            Some(Outcome::Failed(_)) => "Try Again".to_owned(),
                            _ => curled(&u.fix.label),
                        };
                        let mut text = div().flex().flex_col().gap(px(2.0)).child(StringView::new(curled(&u.title)).wrap(true).color(ink.text));
                        if let Some(a) = crate::applied::of(&u.id).and_then(|finding| kept.iter().find(|a| a.id == finding)) {
                            text = text.child(StringView::new(a.line()).wrap(true).color(ink.dim));
                        }
                        let mut side = div().flex().flex_col().items_end().gap(px(m::TIGHT)).child(
                            Button::new(label)
                                .id(format!("fix-{}", u.id))
                                .enabled(outcome != Some(&Outcome::Fixing))
                                .on_click(cx.listener(move |d: &mut Doctor, _: &ClickEvent, cx| d.fix(std::slice::from_ref(&id), cx))),
                        );
                        if let Some(Outcome::Failed(e)) = outcome {
                            side = side.child(StringView::new(curled(e)).color(style::c(p.failure)));
                        }
                        lines = lines.child(row(mark(fine_ink), text, Some(side)));
                    }
                    for t in plain {
                        lines = lines.child(row(mark(fine_ink), StringView::new(curled(t)).wrap(true).color(ink.text).into_element(), None));
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
                        let mut text = div().flex().flex_col().gap(px(2.0)).child(StringView::new(curled(&f.title)).wrap(true).color(ink.text));
                        if !f.detail.is_empty() {
                            text = text.child(StringView::new(curled(&f.detail)).wrap(true).color(ink.dim));
                        }
                        // What's applied can be undone.
                        let side = r.undo.iter().find(|u| u.title == f.title).map(|u| {
                            let outcome = self.outcomes.get(&u.id);
                            let id = u.id.clone();
                            let label = match outcome {
                                Some(Outcome::Fixing) => "Undoing…".to_owned(),
                                Some(Outcome::Failed(_)) => "Try Again".to_owned(),
                                _ => curled(&u.fix.label),
                            };
                            let mut side = div().flex().flex_col().items_end().gap(px(m::TIGHT)).child(
                                Button::new(label)
                                    .id(format!("fix-{}", u.id))
                                    .enabled(outcome != Some(&Outcome::Fixing))
                                    .on_click(cx.listener(move |d: &mut Doctor, _: &ClickEvent, cx| d.fix(std::slice::from_ref(&id), cx))),
                            );
                            if let Some(Outcome::Failed(e)) = outcome {
                                side = side.child(StringView::new(curled(e)).color(style::c(p.failure)));
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
                        let mut text = div().flex().flex_col().gap(px(2.0)).child(StringView::new(curled(&f.title)).wrap(true).color(ink.dim));
                        match outcome {
                            Some(Outcome::Fixing) => {
                                text = text.child(StringView::new(self.progress.get(&f.id).cloned().unwrap_or_else(|| words(Some(f.severity)).0.to_owned())).wrap(true).color(ink.dim))
                            }
                            Some(Outcome::Failed(e)) => text = text.child(StringView::new(curled(e)).wrap(true).color(style::c(p.failure))),
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
                            let label = if matches!(outcome, Some(Outcome::Failed(_))) { "Try Again".to_owned() } else { curled(&fix.label) };
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

/// Keep what a fix that ran did (its undo), or forget an undo that's
/// done: what `finished` and the command line both do. `fix`: the
/// finding's title (None: it was an undo's button) and the fix that ran.
fn keep_applied(id: &str, fix: Option<(Option<String>, Fix)>, ran: &helper::Ran, fixed: bool) -> Option<std::io::Result<()>> {
    // An undo of ours, done: forgotten. (One that failed stays, to try
    // again.)
    if let Some(finding) = crate::applied::of(id) {
        return fixed.then(|| crate::applied::remove(finding));
    }
    // A finding's fix (not a suggestion's own undo button): kept, with
    // what undoes the steps that ran, all of them or the first of them.
    let (Some(title), fix) = fix? else { return None };
    let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    Some(crate::applied::add(crate::applied::Applied { id: id.to_owned(), title, label: fix.label, at, root: fix.root, undo: ran.undo.clone(), kept: ran.kept.clone() }))
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
        Some("--keep-core") => std::process::exit(crate::kept_core::from_the_command_line(&args[1..])),
        Some("--kernel") => std::process::exit(crate::kernel::from_the_command_line(&args[1..])),
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
        let window = olo_kit::open_window(WindowOptions::new(TITLE, 600.0, 580.0).min_size(MIN_WIDTH, MIN_HEIGHT).keep_size(TITLE), move |cx| {
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
                began: BTreeMap::new(),
                ticking: false,
                next_run: 0,
                scroll: ScrollHandle::new(),
                chosen: saved.chosen,
                notes: None,
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
/// with its kind; `all` is what the window checks to begin with (every
/// finding's fix but the suggestions', never an undo), `suggestions`
/// every suggestion (Choose Suggestions, then the button).
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
    let chosen: Vec<Item> = fixable
        .iter()
        .copied()
        .filter(|(id, _, fix, s)| (fix.batchable() && in_all(*s)) || ids.iter().any(|i| i == id))
        .chain(ignored.iter().copied().filter(|(id, ..)| ids.iter().any(|i| i == id)))
        .collect();
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
        let ran = if crate::demo::on() {
            helper::Ran { results: crate::demo::fix(id, &mut |_| {}), ..Default::default() }
        } else if let Err(e) = crate::checks::prepare(id) {
            helper::Ran { results: vec![Err(e)], ..Default::default() }
        } else if !fix.root || root {
            helper::run_steps_keeping(&fix.steps, &mut |text| println!("  {text}"))
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
                    Ok(Some(FromHelper::Done { id, results, undo, kept })) if id == n as u64 => break helper::Ran { results, undo, kept },
                    Ok(Some(FromHelper::Progress { text, .. })) => println!("  {text}"),
                    Ok(Some(_)) => continue,
                    _ => break helper::Ran { results: vec![Err("the root helper stopped".into())], ..Default::default() },
                }
            }
        };
        // Kept with what undoes it (`olo-doctor --fix applied:ID`), as
        // the window keeps it; an undo done is forgotten.
        if !crate::demo::on() {
            let done = ran.results.iter().all(Result::is_ok);
            let finding = report.find(id).map(|f| f.title.clone());
            if let Some(Err(e)) = keep_applied(id, Some((finding, (*fix).clone())), &ran, done) {
                eprintln!("{program}: what undoes it couldn't be kept: {e}");
            }
            for k in &ran.kept {
                println!("  (Can't be undone: {k})");
            }
        }
        let results = ran.results;
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

    /// A fix under way says what it is doing; that it waits where
    /// another is ahead of it; after a while how long it has been at
    /// it; and, as the administrator, at last that it may never end.
    #[test]
    fn a_fix_under_way_says_how_it_goes() {
        let secs = Duration::from_secs;
        // Nothing told yet: its own word, or that it waits.
        assert_eq!(status("Fixing…", None, false, None), "Fixing…");
        assert_eq!(status("Fixing…", None, true, None), "Waiting for another fix to finish…");
        // What it told, alone until it has been a while.
        assert_eq!(status("Fixing…", Some("Writing /etc/olo/x"), false, Some(secs(3))), "Writing /etc/olo/x");
        assert_eq!(status("Fixing…", Some("Writing /etc/olo/x"), true, Some(secs(14))), "Writing /etc/olo/x");
        assert_eq!(status("Fixing…", Some("Installing kernel"), false, Some(secs(43))), "Installing kernel (40 s)");
        assert_eq!(status("Fixing…", Some("Installing kernel"), false, Some(secs(60 * 14 + 7))), "Installing kernel (14 min)");
        // That it may never end: of one as the administrator, after
        // three minutes, and not of one as this user (a download).
        assert_eq!(still(Some(secs(179)), true), None);
        assert_eq!(still(Some(secs(400)), false), None);
        assert_eq!(still(None, true), None);
        let long = still(Some(secs(60 * 14 + 7)), true).unwrap();
        assert!(long.starts_with("Still going after 14 min. Other fixes wait") && long.ends_with("restarting the computer stops it."), "{long}");
        assert_eq!([how_long(secs(17)), how_long(secs(59)), how_long(secs(60)), how_long(secs(125)), how_long(secs(3600))], ["15 s", "55 s", "1 min", "2 min", "60 min"]);
    }

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

    /// What's fixed (`--fix all`, checked to begin with) leaves the
    /// suggestions to be chosen; a merged report lists what's most
    /// pressing first.
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

    /// The button names what it fixes as the summary does: problems
    /// (the critical with them), recommendations, or both together as
    /// many as they are, never recommendations called problems.
    #[test]
    fn the_button_names_what_it_fixes() {
        let kinds = |severities: &[Severity]| -> Vec<(&'static Kind, usize)> { KINDS.iter().map(|&k| (k, severities.iter().filter(|s| kind_of(&finding("x", **s)) == k).count())).collect() };
        assert_eq!(chosen::label(&kinds(&[Severity::Critical, Severity::Problem, Severity::Problem])), "Fix 3 Problems");
        assert_eq!(chosen::label(&kinds(&[Severity::Advice; 7])), "Fix 7 Recommendations");
        assert_eq!(chosen::label(&kinds(&[Severity::Advice])), "Fix 1 Recommendation");
        let mixed = [[Severity::Problem; 3].as_slice(), &[Severity::Advice; 7]].concat();
        assert_eq!(chosen::label(&kinds(&mixed)), "Fix 10");
        let all = [mixed.as_slice(), &[Severity::Suggestion; 2]].concat();
        assert_eq!(chosen::label(&kinds(&all)), "Fix 10 and Apply 2");
        // `choose-kind-1` (a command and a control agents use) is still
        // the suggestions', while the window shows the kinds most
        // pressing first: Problems, Recommendations, Suggestions, each
        // choosing its own (by kind, not by where a constant is kept).
        assert_eq!(Choice::of_command("choose-kind-1"), Some(Choice::Kind(1)));
        assert_eq!((*KINDS[0], *KINDS[1], *KINDS[2]), (PROBLEMS, SUGGESTED, RECOMMENDED));
        let shown: Vec<Kind> = SHOWN.iter().map(|&i| *KINDS[i]).collect();
        assert_eq!(shown, [PROBLEMS, RECOMMENDED, SUGGESTED]);
        for (at, kind) in shown.iter().enumerate() {
            let Choice::Kind(i) = as_command(Choice::Kind(at)) else { panic!("a kind") };
            assert_eq!(*KINDS[i], *kind, "the link shown {at} chooses its own kind");
        }
        assert_eq!(as_command(Choice::All), Choice::All);
        // The menu's items, as shown, each by its kind's command.
        let counts = SHOWN.map(|i| (KINDS[i], 1, 1));
        let commands: Vec<String> = SHOWN.iter().map(|i| format!("choose-kind-{i}")).collect();
        assert_eq!(commands, ["choose-kind-0", "choose-kind-2", "choose-kind-1"]);
        let _ = fix_menu(&counts, true);
    }

    /// The footer on one row where it can be: in a third (317 points,
    /// 207 beside Check Again) Choose ▾ and "Fix 10" sized to what it
    /// says; wider, the links over the row, the button as wide as the
    /// widest it can say; wider still, all on one row.
    #[test]
    fn the_footer_keeps_one_row() {
        // ("Fix 10", 80 points, and its ring's 3 after it.)
        let act = 80.0 + DEFAULT_RING;
        let third = footer_fit(317.0, 207.0, 330.0, 54.0, act);
        assert_eq!(third, FooterFit { links_over: false, links_beside: false, menu: true, under: false });
        // (Its own width: at 560 all of it on one row, the links beside.)
        assert_eq!(footer_fit(560.0, 450.0, 330.0, 54.0, act), FooterFit { links_over: false, links_beside: true, menu: false, under: false });
        assert_eq!(footer_fit(450.0, 340.0, 330.0, 54.0, act), FooterFit { links_over: true, links_beside: false, menu: false, under: false });
        assert_eq!(footer_fit(900.0, 790.0, 330.0, 54.0, act), FooterFit { links_over: false, links_beside: true, menu: false, under: false });
        // Narrower than even that: a row of its own, under Check Again.
        assert!(footer_fit(220.0, 110.0, 330.0, 54.0, act).under);
    }

    /// What the window shows has its apostrophes curled; a quoted word
    /// and a catalog's words are left as they are.
    #[test]
    fn words_are_shown_curled() {
        assert_eq!(curled("Olo's threads can't be realtime"), "Olo’s threads can’t be realtime");
        assert_eq!(curled("the readers' group"), "the readers’ group");
        assert_eq!(curled("set 'rtkit' up"), "set 'rtkit' up");
        assert_eq!(curled("nothing to curl"), "nothing to curl");
    }
}
