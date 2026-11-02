//! Olo Doctor: looks the computer over for what gets in Olo's way (an
//! input remapper grabbing the mice, another desktop's daemons, USB
//! receivers allowed to sleep, Olo's udev rules missing, rtkit down,
//! programs left on an old version), shows what it found and fixes it
//! with a button: each finding's own, or Fix All. What needs root is
//! done by the program run again as root (helper.rs), asked for once.
//!
//! `olo-doctor --report` prints the report as JSON and opens nothing
//! (for agents and scripts); `--watch` is the session's: the look at
//! login and the Wi-Fi healer's news (notify.rs).

mod checks;
mod fix;
mod helper;
mod notify;

use std::collections::BTreeMap;

use checks::{Report, Roots, Severity};
use fix::Step;
use helper::{FromHelper, Helper, ToHelper};
use olo_ui::{
    Frame, Menu, MenuBar, Proxy, WindowOptions,
    gpui::{self, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, ScrollHandle, Styled as _, Window, WindowHandle, canvas, div, px},
    icons,
    instance::{self, Launch},
    metrics as m, style, theme,
    widgets::{BBox, Button, ScrollView, StringView, heading},
};
use serde::{Deserialize, Serialize};

const APP_ID: &str = "olo.doctor";

enum Msg {
    Scanned(Report),
    /// A fix run as this user: the finding and its steps' outcomes.
    Ran(String, Vec<Result<(), String>>),
    Helper(Option<FromHelper>),
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
    proxy: Proxy,
    window: Option<WindowHandle<Frame>>,
    report: Option<Report>,
    scanning: bool,
    /// Another look is due once this one is in (a fix finished meanwhile).
    rescan: bool,
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
    next_run: u64,
    scroll: ScrollHandle,
}

impl Doctor {
    fn scan(&mut self, cx: &mut Context<Self>) {
        if self.scanning {
            self.rescan = true;
            return;
        }
        self.scanning = true;
        let proxy = self.proxy.clone();
        // Off the UI thread: it asks systemd and reads /proc and /sys.
        std::thread::Builder::new().name("doctor-scan".into()).spawn(move || proxy.send(Msg::Scanned(checks::scan(&Roots::real())))).expect("spawn");
        cx.notify();
    }

    fn scanned(&mut self, report: Report, cx: &mut Context<Self>) {
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
        cx.notify();
    }

    fn busy(&self) -> bool {
        self.outcomes.values().any(|o| *o == Outcome::Fixing)
    }

    /// Fix the findings `ids` (those with a fix): as this user at once,
    /// as root through the helper (one password for them all).
    fn fix(&mut self, ids: &[String], cx: &mut Context<Self>) {
        let Some(report) = &self.report else { return };
        let mut root = Vec::new();
        for id in ids {
            let Some(f) = report.find(id) else { continue };
            let Some(fix) = &f.fix else { continue };
            if self.outcomes.get(id) == Some(&Outcome::Fixing) {
                continue;
            }
            self.outcomes.insert(id.clone(), Outcome::Fixing);
            if fix.root {
                root.push((id.clone(), fix.steps.clone()));
            } else {
                let (proxy, id, steps) = (self.proxy.clone(), id.clone(), fix.steps.clone());
                std::thread::spawn(move || proxy.send(Msg::Ran(id, helper::run_steps(&steps))));
            }
        }
        if !root.is_empty() {
            if self.helper.is_none() {
                let proxy = self.proxy.clone();
                match Helper::start(helper::way(), move |m| proxy.send(Msg::Helper(m))) {
                    Ok(h) => {
                        self.helper = Some(h);
                        self.helper_ready = false;
                    }
                    Err(e) => {
                        for (id, _) in root {
                            self.outcomes.insert(id, Outcome::Failed(format!("The administrator couldn't be asked: {e}.")));
                        }
                        cx.notify();
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
        cx.notify();
    }

    fn fix_all(&mut self, cx: &mut Context<Self>) {
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

    fn finished(&mut self, id: String, results: Vec<Result<(), String>>, cx: &mut Context<Self>) {
        let outcome = match results.into_iter().find_map(Result::err) {
            Some(e) => Outcome::Failed(e),
            None => Outcome::Fixed,
        };
        self.outcomes.insert(id, outcome);
        if !self.busy() {
            self.scan(cx);
        }
        cx.notify();
    }

    fn heard(&mut self, m: Option<FromHelper>, cx: &mut Context<Self>) {
        match m {
            Some(FromHelper::Ready) => {
                self.helper_ready = true;
                self.send_queued();
            }
            Some(FromHelper::Done { id, results }) => {
                if let Some(finding) = self.running.remove(&id) {
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
        cx.notify();
    }

    fn message(&mut self, message: Box<dyn std::any::Any + Send>, cx: &mut Context<Self>) {
        if message.downcast_ref::<Launch>().is_some() {
            // Opened again: look again, and come forward.
            if let Some(w) = self.window {
                cx.defer(move |cx| {
                    let _ = w.update(cx, |_, window, _| window.activate_window());
                });
            }
            return self.scan(cx);
        }
        let Ok(msg) = message.downcast::<Msg>() else { return };
        match *msg {
            Msg::Scanned(r) => self.scanned(r, cx),
            Msg::Ran(id, results) => self.finished(id, results, cx),
            Msg::Helper(m) => self.heard(m, cx),
        }
    }

    fn menus(&self) -> Vec<Menu> {
        let fixable = self.report.as_ref().is_some_and(|r| r.fixable().any(|f| self.outcomes.get(&f.id) != Some(&Outcome::Fixing)));
        vec![
            Menu::new("File").item("Check Again", "check").key("r").enabled(!self.scanning).separator().item("Close", "close").key("w").item("Quit", "quit").key("q"),
            Menu::new("Fix").item("Fix All", "fix-all").shift_key("f").enabled(fixable),
        ]
    }

    fn command(&mut self, command: &str, cx: &mut Context<Self>) -> bool {
        match command {
            "check" => self.scan(cx),
            "fix-all" => self.fix_all(cx),
            "close" | "quit" => cx.quit(),
            _ => return false,
        }
        true
    }

    fn save_state(&self) -> Option<String> {
        serde_json::to_string(&Saved { fixed: self.fixed.clone() }).ok()
    }

    /// The summary at the top.
    fn summary(&self) -> (String, String) {
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
fn mark(color: gpui::Hsla) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |b, _, window, _| {
            let r = style::snap_centred(window, b.center(), 10.0, 8.0);
            olo_ui::menu::check_mark(window, r, color);
        },
    )
    .w(px(m::MARK_SIZE))
    .h(px(m::LABEL_ROW))
    .flex_shrink_0()
}

impl Render for Doctor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let me = cx.entity();
        let p = theme::palette(cx);
        let bar_me = me.clone();
        let bar = MenuBar::new(self.menus(), move |cmd, _, cx| {
            bar_me.update(cx, |d, cx| d.command(cmd, cx));
        });
        let (title, detail) = self.summary();
        let fixable = self.report.as_ref().is_some_and(|r| r.fixable().any(|f| self.outcomes.get(&f.id) != Some(&Outcome::Fixing)));
        let icon = canvas(
            |_, _, _| {},
            |b, _, window, _| {
                icons::paint(window, style::snap_centred(window, b.center(), 32.0, 32.0), "doctor");
            },
        )
        .size(px(32.0))
        .flex_shrink_0();
        let (check, all) = (me.clone(), me.clone());
        let header = div()
            .flex()
            .items_center()
            .gap(px(m::SPACING + 2.0))
            .child(icon)
            .child(div().flex_1().min_w_0().flex().flex_col().child(StringView::new(title).bold(true).height(m::LABEL_ROW + 2.0)).child(StringView::new(detail).dim(true).height(m::LABEL_ROW + 2.0)))
            .child(Button::new("Check Again").enabled(!self.scanning).on_click(move |_, cx| check.update(cx, |d, cx| d.scan(cx))))
            .child(Button::new("Fix All").default(fixable).enabled(fixable).on_click(move |_, cx| all.update(cx, |d, cx| d.fix_all(cx))));
        let mut list = div().flex().flex_col().gap(px(m::SPACING)).pb(px(m::SPACING)).pr(px(m::SPACING));
        if let Some(r) = &self.report {
            for f in &r.findings {
                let outcome = self.outcomes.get(&f.id);
                let status = match outcome {
                    Some(Outcome::Fixing) => StringView::new("Fixing…").dim(true),
                    Some(Outcome::Fixed) => StringView::new("Fixed.").color(style::c(p.success)),
                    Some(Outcome::Failed(e)) => StringView::new(e.clone()).wrap(true).color(style::c(p.failure)),
                    None if f.severity == Severity::Problem => StringView::new("Gets in Olo's way").color(style::c(p.failure)),
                    None => StringView::new("Suggested").dim(true),
                };
                let mut row = div().flex().items_center().gap(px(m::SPACING)).child(div().flex_1().min_w_0().child(status));
                match &f.fix {
                    Some(fix) => {
                        let (me, id) = (me.clone(), f.id.clone());
                        let label = if matches!(outcome, Some(Outcome::Failed(_))) { "Try Again".to_owned() } else { fix.label.clone() };
                        row = row.child(Button::new(label).id(format!("fix-{}", f.id)).enabled(outcome != Some(&Outcome::Fixing)).on_click(move |_, cx| me.update(cx, |d, cx| d.fix(std::slice::from_ref(&id), cx))));
                    }
                    None => row = row.child(StringView::new("Fixed by hand").dim(true)),
                }
                list = list.child(BBox::new(f.title.clone()).child(StringView::new(f.detail.clone()).wrap(true)).child(row));
            }
            let fine_ink = style::c(p.success);
            if !self.fixed.is_empty() {
                list = list.child(div().pt(px(m::TIGHT)).child(heading("Fixed")));
                for t in &self.fixed {
                    list = list.child(div().flex().gap(px(m::MARK_GAP)).child(mark(fine_ink)).child(StringView::new(t.clone()).height(m::LABEL_ROW)));
                }
            }
            if !r.fine.is_empty() {
                list = list.child(div().pt(px(m::TIGHT)).child(heading("As It Should Be")));
                for f in &r.fine {
                    let mut text = div().flex_1().min_w_0().flex().flex_col().child(StringView::new(f.title.clone()).height(m::LABEL_ROW));
                    if !f.detail.is_empty() {
                        text = text.child(StringView::new(f.detail.clone()).wrap(true).dim(true));
                    }
                    list = list.child(div().flex().gap(px(m::MARK_GAP)).child(mark(fine_ink)).child(text));
                }
            }
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(bar)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .gap(px(m::SPACING + 2.0))
                    .p(px(m::WINDOW_INSET))
                    .child(header)
                    .child(div().flex_1().min_h_0().child(ScrollView::new("doctor-findings", &self.scroll).border(false).child(list))),
            )
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--helper") => helper::serve(),
        Some("--notify") => return notify::run(),
        Some("--watch") => notify::watch(),
        Some("--report") => {
            println!("{}", serde_json::to_string_pretty(&checks::scan(&Roots::real())).unwrap_or_default());
            return;
        }
        _ => {}
    }
    if instance::forward(APP_ID, &args) {
        return;
    }
    olo_ui::run(APP_ID, move |cx| {
        instance::listen(APP_ID, olo_ui::proxy(cx));
        let saved: Saved = olo_ui::restored_state().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        let proxy = olo_ui::proxy(cx);
        let app: Entity<Doctor> = cx.new(|_| Doctor {
            proxy,
            window: None,
            report: None,
            scanning: false,
            rescan: false,
            outcomes: BTreeMap::new(),
            fixed: saved.fixed,
            helper: None,
            helper_ready: false,
            queued: Vec::new(),
            running: BTreeMap::new(),
            next_run: 0,
            scroll: ScrollHandle::new(),
        });
        let view = app.clone();
        let window = olo_ui::open_window(cx, WindowOptions::new("Olo Doctor", 600.0, 560.0).min_size(460.0, 360.0), move |_, _| view);
        app.update(cx, |d, cx| {
            d.window = window;
            d.scan(cx);
        });
        let messages = app.clone();
        olo_ui::message_handler(cx, move |message, cx| messages.update(cx, |d, cx| d.message(message, cx)));
        let saving = app.clone();
        olo_ui::on_upgrade(cx, move |cx| saving.read(cx).save_state());
    });
}
