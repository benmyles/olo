//! The Installer's window: BeOS's Installer, for Olo. Full screen, the
//! plain blue of BeOS's desktop (its Installer ran on the bare desktop
//! of the CD it started from), and on it one window drawn as BeOS draws
//! them (the yellow tab with its title, the grey panel). First the
//! licence, as BeOS showed its own: the words in a scroll box, Disagree
//! and Agree. Then "Installer", the one window everything happens in:
//! Olo's mark at its top left and a box of words beside it, saying what
//! to do and then what is being done; "Install from:" and "Onto:" as
//! pop-ups under it; "More Options", a latch that opens the optional
//! parts under it (the window grows to hold them, as BeOS's did); a
//! status bar while it works; and Begin, which is Stop while it works.
//! About comes in front of it, with the dedication.
//!
//! Reminiscent, not a copy: BeOS's pop-ups chose a CD and a volume, and
//! here there is one of each (Olo's newest release; this computer), so
//! they say what will happen rather than ask. Its optional parts showed
//! what each was only under the pointer; here each has its words under
//! it, for whoever reads the window without a pointer.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use olo_kit::{
    Bounds, Cx, Element, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, ScrollHandle, SharedString, Styled as _, View, canvas, div, icons, metrics as m, point, px, size, style,
    theme,
    widgets::{Button, CheckBox, MenuField, PaneSwitch, ScrollView, StatusBar, StringView},
};

use crate::plan::{self, Choices, Computer, Event};

/// What shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    /// The licence, to agree to.
    Licence,
    /// The Installer, before Begin (and again after Stop).
    Ready,
    /// The Installer at work (or stopped by a failure: Try Again).
    Installing,
    /// The Installer, Olo installed.
    Done,
}

/// How the install ended.
#[derive(Debug, Clone)]
enum Outcome {
    Done { version: String, restart: bool },
    Failed { what: String, why: String, advice: String },
    Stopped,
}

pub struct Setup {
    pub page: Page,
    computer: Computer,
    pub choices: Choices,
    /// More Options is open.
    more: bool,
    /// Stage n of of, and its title.
    stage: Option<(usize, usize, String)>,
    share: Option<f32>,
    doing: String,
    notes: Vec<String>,
    password: bool,
    outcome: Option<Outcome>,
    stop: Arc<AtomicBool>,
    about: bool,
    /// The archive asked for (`--bundle`), else the newest release.
    given: Option<String>,
    /// Going through it by itself (`--yes`: tests, an agent).
    auto: bool,
    licence: ScrollHandle,
    words: ScrollHandle,
}

/// What the window is told: by the worker thread, by `--yes`, and by its
/// own controls.
#[derive(Clone)]
pub enum Msg {
    /// What the worker thread says.
    Told(Event),
    /// Moving on by itself (`--yes`), after a while.
    Advance,
    /// The licence agreed to.
    Agree,
    Begin,
    /// Stop installing.
    Stop,
    More(bool),
    LoginScreen(bool),
    BootSplash(bool),
    RpmFusion(bool),
    About(bool),
    /// Log out or restart, Olo installed.
    Leave(bool),
    Quit,
}

const LICENCE: &str = include_str!("licence.txt");

/// `OLO_SETUP_DEMO`: Begin installs nothing and plays an install's
/// events instead (`done`, `restart`, `fail`), Log Out and Restart only
/// say what they would do, and the computer is a made-up stock Fedora.
/// For looking at the window (a test, the layout review, a picture of
/// it) on a computer that mustn't be changed.
fn demo() -> Option<String> {
    std::env::var("OLO_SETUP_DEMO").ok().filter(|v| !v.is_empty())
}

impl Setup {
    pub fn new(given: Option<String>, auto: bool, choices: Choices) -> Setup {
        let computer = match demo() {
            // (A made-up one, as it comes from Fedora: what's shown doesn't
            // depend on, or tell of, the computer it's shown on.)
            Some(_) => Computer {
                fedora: Some(45),
                variant: "Workstation Edition".into(),
                atomic: false,
                user: "sam".into(),
                home: "/home/sam".into(),
                nvidia: false,
                free: 212 << 30,
                installed: None,
                had: Choices::default(),
            },
            None => plan::look(),
        };
        // (What it has already chosen again: run again, it's updated.)
        let choices = choices.and(computer.had);
        Setup {
            page: Page::Licence,
            // (Open when something in it is chosen: what will be
            // installed is to be seen.)
            more: choices != Choices::default(),
            choices,
            computer,
            stage: None,
            share: None,
            doing: String::new(),
            notes: Vec::new(),
            password: false,
            outcome: None,
            stop: Arc::new(AtomicBool::new(false)),
            about: false,
            given,
            auto,
            licence: ScrollHandle::new(),
            words: ScrollHandle::new(),
        }
    }

    /// Install, on a worker thread.
    fn begin(&mut self) {
        self.stop.store(false, Ordering::Relaxed);
        self.page = Page::Installing;
        self.outcome = None;
        self.stage = None;
        self.share = None;
        self.doing.clear();
        self.notes.clear();
        let (choices, given, stop, proxy) = (self.choices, self.given.clone(), self.stop.clone(), olo_kit::proxy());
        let tell = move |e| proxy.send(Msg::Told(e));
        let work = move || match demo() {
            Some(how) => pretend(&how, &stop, tell),
            None => plan::install(choices, given, stop, tell),
        };
        std::thread::Builder::new().name("setup".into()).spawn(work).expect("spawn");
    }

    fn told(&mut self, e: Event, cx: &mut Cx<Self>) {
        match e {
            Event::Stage { n, of, title } => {
                self.notes.push(title.clone());
                self.stage = Some((n, of, title));
                self.share = None;
                self.doing.clear();
                self.password = false;
            }
            Event::Progress { share, text } => {
                self.share = share;
                self.doing = text;
                self.password = false;
            }
            Event::Note(t) => self.notes.push(format!("  {t}")),
            Event::Password => self.password = true,
            Event::Done { version, restart } => {
                self.outcome = Some(Outcome::Done { version, restart });
                self.page = Page::Done;
            }
            Event::Failed { what, why, advice } => {
                tracing::warn!("setup: {what}: {why}");
                self.notes.push(format!("  {what}: {why}"));
                self.outcome = Some(Outcome::Failed { what, why, advice });
                if self.auto {
                    eprintln!("olo-installer: failed: {}", self.notes.last().cloned().unwrap_or_default());
                }
            }
            Event::Stopped => {
                // (As BeOS's: stopped, it can be begun again.)
                self.outcome = Some(Outcome::Stopped);
                self.page = Page::Ready;
            }
        }
        self.words.scroll_to_bottom(cx);
        cx.invalidate();
    }

    /// `--yes`: on to the next thing.
    fn advance(&mut self) {
        match self.page {
            Page::Licence => self.page = Page::Ready,
            // (Stopped: it isn't begun again by itself.)
            Page::Ready if matches!(self.outcome, Some(Outcome::Stopped)) => olo_kit::quit(),
            Page::Ready if plan::unsuitable(&self.computer).is_none() => self.begin(),
            Page::Done => olo_kit::quit(),
            _ => {}
        }
    }

    /// The overall share done, from the stage and its share.
    fn overall(&self) -> f32 {
        match (&self.stage, &self.outcome) {
            (_, Some(Outcome::Done { .. })) => 1.0,
            (Some((n, of, _)), _) => ((*n as f32 - 1.0) + self.share.unwrap_or(0.0)) / *of as f32,
            _ => 0.0,
        }
    }

    fn failed(&self) -> bool {
        matches!(self.outcome, Some(Outcome::Failed { .. }))
    }

    /// At work: nothing about what's installed can be changed.
    fn working(&self) -> bool {
        self.page == Page::Installing && !self.failed()
    }
}

/// An install's events without an install (`OLO_SETUP_DEMO`).
fn pretend(how: &str, stop: &AtomicBool, tell: impl Fn(Event)) {
    let stages = ["Checking this computer", "Getting what Olo needs", "Downloading Olo", "Unpacking Olo", "Copying files", "Adding Olo to the login screen"];
    let nap = |ms| std::thread::sleep(std::time::Duration::from_millis(ms));
    for (i, title) in stages.iter().enumerate() {
        tell(Event::Stage { n: i + 1, of: stages.len(), title: (*title).to_owned() });
        if i == 0 {
            tell(Event::Note("Fedora 45 Workstation Edition, x86_64; 212 GB free.".into()));
        }
        if i == 1 {
            tell(Event::Password);
            nap(900);
        }
        for step in 0..=10 {
            if stop.load(Ordering::Relaxed) {
                return tell(Event::Stopped);
            }
            let share = step as f32 / 10.0;
            tell(Event::Progress { share: Some(share), text: if i == 2 { format!("olo-2026.11.12-x86_64.tar.zst: {:.0} of 212 MB", share * 212.0) } else { format!("{title}…") } });
            nap(70);
        }
        if how == "fail" && i == 2 {
            return tell(Event::Failed {
                what: "Downloading Olo".into(),
                why: "the network is unreachable".into(),
                advice: "Check that this computer is on the network, then try again: what was downloaded is kept.".into(),
            });
        }
        tell(Event::Note(format!("{title}: done.")));
    }
    tell(Event::Done { version: "2026.11.12".into(), restart: how == "restart" });
}

/// BeOS's desktop, as it came: the blue its Installer ran on.
const DESKTOP: olo_theme::Rgba = [51, 102, 152, 255];

/// The windows' width on the backdrop, and where their tops are: fixed,
/// so the Installer grows downwards when More Options opens.
const WIDE: f32 = 560.0;
const TOP: f32 = 96.0;
/// Its tab's height.
const TAB: f32 = 24.0;
/// The column Olo's mark is in, at the Installer's left.
const MARK: f32 = 64.0;
/// How tall the box of words is, and the licence's.
const WORDS: f32 = 148.0;
const LICENCE_TALL: f32 = 380.0;
/// Where the pop-ups' fields start, after their labels.
const DIVIDER: f32 = 84.0;

/// A BeOS window drawn on the backdrop: the tab (its title in it), then
/// the grey panel with its frame, holding `body` and as tall as it.
fn beos_window(title: &'static str, body: impl IntoElement, w: f32) -> Element {
    let tab = canvas(move |g, b| {
        let p = theme::palette();
        let font = style::ui_font(FontWeight::BOLD);
        let tw = f32::from(style::text_width(title, 12.0, font.clone()));
        let r = style::snap(g, Bounds::new(b.origin, size(px(tw + 2.0 * 14.0), px(TAB))));
        style::fill_rect(g, r, style::c(p.tab_focused));
        style::bevel(g, r, style::c(p.tab_focused_light), style::c(p.tab_focused_shadow));
        style::frame_rect(g, Bounds::new(r.origin - point(px(1.0), px(1.0)), r.size + size(px(2.0), px(1.0))), style::c(p.frame_focused[0]));
        style::label(g, r.origin.x + px(14.0), r.center().y, style::Align::Left, title, 12.0, font, style::c(p.tab_text_focused));
    })
    .paint_key(style::key(title))
    .w(px(w))
    .h(px(TAB));
    let panel = div()
        .relative()
        .w(px(w))
        .child(
            canvas(|g, b| {
                let p = theme::palette();
                let r = style::snap(g, b);
                style::fill_rect(g, r, style::c(p.panel_background));
                style::frame_rect(g, r, style::c(p.frame_focused[0]));
                let inside = style::inset(g, r, 1.0);
                style::bevel(g, inside, style::c(p.frame_focused[1]), style::c(p.frame_focused[4]));
            })
            .paint_key(0)
            .absolute()
            .size_full(),
        )
        .child(div().relative().p(px(m::WINDOW_INSET)).child(body));
    div().flex().flex_col().child(tab).child(panel)
}

fn para(text: impl Into<SharedString>) -> StringView {
    StringView::new(text).wrap(true)
}

/// Words in a box of words: on white, as BeOS's text views are (the
/// document's colours, not the panel's).
fn words(text: impl Into<SharedString>) -> StringView {
    StringView::new(text).wrap(true).color(style::c(theme::palette().document_text))
}

/// Quieter words in one.
fn quiet_words(text: impl Into<SharedString>) -> StringView {
    let p = theme::palette();
    StringView::new(text).wrap(true).color(style::c(olo_theme::mix(p.document_text, p.document_background, 0.5)))
}

/// Olo's mark at the Installer's top left, where BeOS's had Be's.
fn mark(icon: &'static str) -> Element {
    canvas(move |g, b| {
        let at = style::snap_centred(g, b.center(), MARK, MARK);
        icons::paint(g, at, icon);
    })
    .paint_key(style::key(icon))
    .size(px(MARK))
    .flex_shrink_0()
}

impl Setup {
    /// The licence's window.
    fn licence_window(&mut self, cx: &mut Cx<Self>) -> Element {
        let body = div()
            .flex()
            .flex_col()
            .gap(px(m::SPACING))
            .child(para("Please read Olo's licence. To install Olo you must agree to it."))
            .child(div().h(px(LICENCE_TALL)).child(ScrollView::new(&self.licence).background(theme::palette().document_background).child(div().p(px(m::TEXT_INSET)).child(words(LICENCE)))))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(m::SPACING))
                    .child(Button::new("Disagree").id("disagree").on_click(cx.msg(Msg::Quit)))
                    .child(Button::new("Agree").id("agree").default(true).on_click(cx.msg(Msg::Agree))),
            );
        beos_window("Licence", body, WIDE)
    }

    /// What the box of words says: what to do, then what is being done.
    fn says(&self) -> Element {
        let p = theme::palette();
        let mut c = div().flex().flex_col().gap(px(m::STACK)).p(px(m::TEXT_INSET));
        let red = |t: String| StringView::new(t).wrap(true).color(style::c(p.failure));
        let dedication = || quiet_words("Olo is a work of love, dedicated to Sir Samwise, the bestest English Labrador that ever walked the earth.");
        match (self.page, &self.outcome) {
            (Page::Done, Some(Outcome::Done { version, restart })) => {
                let how = if *restart {
                    "Restart the computer: Olo's login screen greets you, and Olo starts when you log in."
                } else {
                    "Log out, then choose “Olo Desktop” from the login screen's session menu (the gear by Sign In) before you log in again. Fedora's GNOME stays there too."
                };
                c = c.child(words(format!("Installation completed. Olo {version} is on this computer."))).child(words(how)).child(dedication());
            }
            (Page::Installing, _) => {
                // The stages and their notes so far, the newest last.
                let mut notes = div().flex().flex_col().gap(px(2.0));
                for n in &self.notes {
                    notes = notes.child(words(n.clone()));
                }
                c = c.child(notes);
                if let Some(Outcome::Failed { what, why, advice }) = &self.outcome {
                    // ("Adding Olo to…" as part of a sentence: its first letter small.)
                    let what = what.chars().next().map(|f| f.to_lowercase().chain(what.chars().skip(1)).collect::<String>()).unwrap_or_default();
                    c = c.child(red(format!("The Installer couldn't finish {what}: {why}"))).child(words(advice.clone()));
                }
            }
            _ => match plan::unsuitable(&self.computer) {
                Some(why) => c = c.child(red(why)),
                None => {
                    if matches!(self.outcome, Some(Outcome::Stopped)) {
                        c = c.child(words("Installation stopped. Olo isn't installed yet: what was done and downloaded is kept, and Begin goes on from there."));
                    } else {
                        c = c.child(words(
                            "Olo is a desktop in the spirit of BeOS. It is installed onto this computer beside Fedora's own desktop, in your home folder (~/.local), and added to the login screen's session menu: you choose Olo or GNOME each time you log in.",
                        ));
                        if let Some(v) = &self.computer.installed {
                            c = c.child(words(format!("Olo {v} is installed already: the newest takes its place.")));
                        }
                    }
                    c = c.child(words(
                        "The administrator's password is asked for once. Installing can be stopped and begun again at any time: it goes on from where it was. Save your work and close other programs, then click “Begin”.",
                    ));
                }
            },
        }
        c
    }

    /// More Options' pane: the optional parts, each with its words (the
    /// desktop itself isn't one: the box of words says where it goes).
    fn options(&self, cx: &mut Cx<Self>) -> Element {
        let ch = self.choices;
        let can = !self.working() && self.page != Page::Done;
        // Each with its words under it, a little apart from the next.
        let choice =
            |check: CheckBox, t: &str| div().flex().flex_col().gap(px(m::TIGHT)).child(check).child(div().pl(px(m::MARK_SIZE + m::MARK_GAP)).child(StringView::new(t.to_owned()).wrap(true).dim(true)));
        let nvidia = if self.computer.nvidia { " This computer has an NVIDIA graphics card." } else { "" };
        // (Installed before: kept chosen, and said so.)
        let had = self.computer.had;
        let updated = |on: bool, words: &str| if on { format!("{words} Installed already: it's brought up to date.") } else { words.to_owned() };
        let gb = |bytes: u64| bytes as f64 / (1u64 << 30) as f64;
        div()
            .flex()
            .flex_col()
            .gap(px(m::STACK))
            .child(choice(
                CheckBox::new("Olo's login screen", ch.login_screen).id("login-screen").enabled(can).on_toggle(cx.msg_with(|on: &bool| Msg::LoginScreen(*on))),
                &updated(had.login_screen, "Olo's own login screen in place of GNOME's (GDM, which stays installed). Your password there also opens Olo's keyring."),
            ))
            .child(choice(
                CheckBox::new("Olo's boot splash", ch.boot_splash).id("boot-splash").enabled(can).on_toggle(cx.msg_with(|on: &bool| Msg::BootSplash(*on))),
                &updated(had.boot_splash, "Olo's picture while the computer starts, in place of Fedora's."),
            ))
            .child(choice(
                CheckBox::new("RPM Fusion", ch.rpm_fusion).id("rpm-fusion").enabled(can).on_toggle(cx.msg_with(|on: &bool| Msg::RpmFusion(*on))),
                &if had.rpm_fusion {
                    "Fedora's companion repositories: NVIDIA's own driver and more video formats. Added already.".to_owned()
                } else {
                    format!("Fedora's companion repositories: NVIDIA's own driver and more video formats.{nvidia}")
                },
            ))
            // (As BeOS's pane ended: what it takes, and what there is.)
            .child(StringView::new(format!("Disk space required: about 6 GB. {:.0} GB are free in your home folder.", gb(self.computer.free))))
            .into_element()
    }

    /// The buttons along the Installer's bottom: About at the left (where
    /// BeOS's had "Setup partitions…"), Begin at the right.
    fn buttons(&self, cx: &mut Cx<Self>) -> Element {
        let left = div().flex().gap(px(m::SPACING)).child(Button::new("About…").id("about").enabled(!self.working()).on_click(cx.msg(Msg::About(true))));
        let mut right = div().flex().gap(px(m::SPACING));
        match (self.page, &self.outcome) {
            (Page::Done, Some(Outcome::Done { restart, .. })) => {
                // (Quit is what Return does: leaving the desktop it runs
                // in is asked for with a click of its own.)
                right = right
                    .child(Button::new(if *restart { "Restart" } else { "Log Out" }).id("leave").on_click(cx.msg(Msg::Leave(*restart))))
                    .child(Button::new("Quit").id("quit").default(true).on_click(cx.msg(Msg::Quit)));
            }
            (Page::Installing, Some(Outcome::Failed { .. })) => {
                right = right.child(Button::new("Quit").id("quit").on_click(cx.msg(Msg::Quit))).child(Button::new("Try Again").id("again").default(true).on_click(cx.msg(Msg::Begin)));
            }
            (Page::Installing, _) => {
                right = right.child(Button::new("Stop").id("stop").on_click(cx.msg(Msg::Stop)));
            }
            _ => {
                let can = plan::unsuitable(&self.computer).is_none();
                right = right.child(Button::new("Quit").id("quit").on_click(cx.msg(Msg::Quit))).child(Button::new("Begin").id("begin").default(true).enabled(can).on_click(cx.msg(Msg::Begin)));
            }
        }
        div().flex().items_center().justify_between().child(left).child(right)
    }

    /// The Installer's window.
    fn installer_window(&mut self, cx: &mut Cx<Self>) -> Element {
        // What it installs, and onto what: one of each, said as BeOS's
        // pop-ups said its CD and the volume chosen.
        let from: SharedString = match &self.given {
            Some(given) => given.rsplit('/').next().unwrap_or(given).to_owned().into(),
            None => "Olo's newest release".into(),
        };
        let onto: SharedString = match self.computer.fedora {
            Some(v) => format!("Fedora {v} on this computer, for {}", self.computer.user).into(),
            None => "This computer".into(),
        };
        let can = !self.working() && self.page != Page::Done;
        let mut column = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(m::SPACING))
            .child(div().h(px(WORDS)).child(ScrollView::new(&self.words).background(theme::palette().document_background).child(self.says())))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(m::STACK))
                    .child(MenuField::new("from", vec![from], 0).label("Install from:").divider(DIVIDER).enabled(can))
                    .child(MenuField::new("onto", vec![onto], 0).label("Onto:").divider(DIVIDER).enabled(can)),
            )
            .child(div().flex().child(PaneSwitch::new(if self.more { "Fewer Options" } else { "More Options" }, self.more).id("more").on_toggle(cx.msg_with(|on: &bool| Msg::More(*on)))));
        if self.more {
            column = column.child(self.options(cx));
        }
        // At work (or stopped by a failure, where it got to): how far,
        // as BeOS's status bars say it (what over the bar, how far
        // through at its right), and what it's doing now under it.
        if self.page == Page::Installing || self.page == Page::Done {
            let (title, through) = match (&self.stage, &self.outcome) {
                (_, Some(Outcome::Done { .. })) => ("Installation completed".to_owned(), String::new()),
                (Some((n, of, t)), _) => (format!("{t}…"), format!("{n} of {of}")),
                _ => ("Starting…".into(), String::new()),
            };
            let doing = match () {
                _ if self.page == Page::Done || self.failed() => String::new(),
                _ if self.password => "Waiting for the administrator's password…".to_owned(),
                _ => self.doing.clone(),
            };
            column = column.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(m::STACK))
                    .child(StatusBar::new(self.overall()).label(title).trailing(through))
                    // (A line kept for it: the window doesn't jump as it comes and goes.)
                    .child(StringView::new(doing).dim(true).cut(true).height(m::LABEL_ROW)),
            );
        }
        let icon = if self.failed() { "warning" } else { "olo" };
        let body = div().flex().flex_col().gap(px(m::SECTION)).child(div().flex().gap(px(m::SECTION)).child(mark(icon)).child(column)).child(self.buttons(cx));
        beos_window("Installer", body, WIDE)
    }
}

/// The About window: what the Installer is, and for whom.
fn about(cx: &mut Cx<Setup>) -> Element {
    let body = div()
        .flex()
        .flex_col()
        .gap(px(m::STACK))
        .child(div().flex().gap(px(m::SPACING)).items_center().child(mark("olo")).child(
            div().flex().flex_col().child(StringView::new("Olo Installer").bold(true).size(15.0).height(22.0)).child(StringView::new(format!("Version {}", env!("CARGO_PKG_VERSION"))).dim(true)),
        ))
        .child(para("Olo is a desktop in the spirit of BeOS: fast, light, and made to be understood, for people and for the agents working beside them."))
        .child(para("Olo is a work of love, dedicated to Sir Samwise, the bestest English Labrador that ever walked the earth."))
        .child(div().flex().justify_end().child(Button::new("OK").id("about-ok").default(true).on_click(cx.msg(Msg::About(false)))));
    beos_window("About Installer", body, ABOUT_WIDE)
}

const ABOUT_WIDE: f32 = 420.0;

impl View for Setup {
    type Message = Msg;

    fn message(&mut self, message: Msg, cx: &mut Cx<Self>) {
        match message {
            Msg::Told(e) => return self.told(e, cx),
            Msg::Advance => self.advance(),
            Msg::Agree => self.page = Page::Ready,
            Msg::Begin => self.begin(),
            Msg::Stop => {
                self.stop.store(true, Ordering::Relaxed);
                olo_net::nudge();
                self.doing = "Stopping…".into();
            }
            Msg::More(on) => self.more = on,
            Msg::LoginScreen(on) => self.choices.login_screen = on,
            Msg::BootSplash(on) => self.choices.boot_splash = on,
            Msg::RpmFusion(on) => self.choices.rpm_fusion = on,
            Msg::About(on) => self.about = on,
            Msg::Leave(restart) => match demo() {
                Some(_) => eprintln!("olo-installer: demo: would {}", if restart { "restart the computer" } else { "log out" }),
                None => crate::session::leave(restart),
            },
            Msg::Quit => olo_kit::quit(),
        }
        cx.invalidate();
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let screen = olo_kit::viewport_size();
        let sw = f32::from(screen.width);
        let backdrop = canvas(|g, b| style::fill_rect(g, b, style::c(DESKTOP))).paint_key(0).absolute().size_full();
        // (About comes in front of the Installer, alone on the desktop
        // until OK: the one surface has no windows of its own to stack,
        // and one drawn over another reads, to the layout review and to
        // an agent, as its parts among the other's.)
        let (window, wide) = match self.page {
            _ if self.about => (about(cx), ABOUT_WIDE),
            Page::Licence => (self.licence_window(cx), WIDE),
            _ => (self.installer_window(cx), WIDE),
        };
        let root = div().relative().size_full().child(backdrop).child(div().absolute().left(px(((sw - wide) / 2.0).round())).top(px(TOP)).child(window));
        root
    }
}
