//! Olo Setup's window: Windows 95's Setup in BeOS's look. Full screen, a
//! deep blue backdrop fading to black with "Olo Setup" in big white
//! italic serif at its top left, as Windows 95's had; on it, one window
//! at a time drawn as BeOS draws them (the yellow tab with its title, the
//! grey panel), each a page of the wizard: Welcome, Licence Agreement,
//! Choose What to Install, Installing (the progress bar and what it's
//! doing), and the last, Olo Is Installed. A picture panel down its left
//! as Setup's had, the words on the right, and `< Back`, `Next >` and
//! `Cancel` along the bottom. About shows over it, with the dedication.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use olo_kit::{
    Bounds, Cx, Element, Font, FontFallbacks, FontStyle, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, ScrollHandle, SharedString, Styled as _, View, black, canvas, div,
    icons, metrics as m, point, px, size, style, theme, white,
    widgets::{Button, CheckBox, RadioButton, ScrollView, StatusBar, StringView},
};

use crate::plan::{self, Choices, Computer, Event};

/// The pages, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Welcome,
    Licence,
    Choose,
    Installing,
    Finish,
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
    accepted: bool,
    pub choices: Choices,
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
    /// Going through the pages by itself (`--yes`: tests, an agent).
    auto: bool,
    licence: ScrollHandle,
    notes_scroll: ScrollHandle,
}

/// What the window is told: by the worker thread, by `--yes`, and by its
/// own controls.
#[derive(Clone)]
pub enum Msg {
    /// What the worker thread says.
    Told(Event),
    /// Moving on by itself (`--yes`), after a while on a page.
    Advance,
    Next,
    Back,
    Accept(bool),
    LoginScreen(bool),
    BootSplash(bool),
    RpmFusion(bool),
    About(bool),
    /// Install again, after it failed.
    Again,
    /// Stop installing.
    Stop,
    /// Log out or restart, Olo installed.
    Leave(bool),
    Quit,
}

const LICENCE: &str = include_str!("licence.txt");

impl Setup {
    pub fn new(given: Option<String>, auto: bool, choices: Choices) -> Setup {
        let computer = plan::look();
        Setup {
            page: Page::Welcome,
            // (What it has already chosen again: run again, Setup updates it.)
            choices: choices.and(computer.had),
            computer,
            accepted: false,
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
            notes_scroll: ScrollHandle::new(),
        }
    }

    /// The next page (Installing starts the work).
    fn next(&mut self) {
        self.page = match self.page {
            Page::Welcome => Page::Licence,
            Page::Licence => Page::Choose,
            Page::Choose => {
                self.start();
                Page::Installing
            }
            Page::Installing | Page::Finish => Page::Finish,
        };
    }

    fn back(&mut self) {
        self.page = match self.page {
            Page::Licence => Page::Welcome,
            Page::Choose => Page::Licence,
            p => p,
        };
    }

    /// Install, on a worker thread.
    fn start(&mut self) {
        self.stop.store(false, Ordering::Relaxed);
        self.outcome = None;
        self.stage = None;
        self.share = None;
        self.doing.clear();
        self.notes.clear();
        let (choices, given, stop, proxy) = (self.choices, self.given.clone(), self.stop.clone(), olo_kit::proxy());
        std::thread::Builder::new().name("setup".into()).spawn(move || plan::install(choices, given, stop, move |e| proxy.send(Msg::Told(e)))).expect("spawn");
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
                self.page = Page::Finish;
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
                self.outcome = Some(Outcome::Stopped);
                self.page = Page::Finish;
            }
        }
        self.notes_scroll.scroll_to_bottom(cx);
        cx.invalidate();
    }

    /// `--yes`: on to the next page.
    fn advance(&mut self, cx: &mut Cx<Self>) {
        match self.page {
            Page::Welcome if plan::unsuitable(&self.computer).is_none() => self.next(),
            Page::Licence => {
                self.accepted = true;
                self.next();
            }
            Page::Choose => self.next(),
            Page::Finish => olo_kit::quit(),
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
}

/// The backdrop's blue at the top and black at the bottom.
const SKY: olo_theme::Rgba = [0x0a, 0x3a, 0xa8, 255];
const NIGHT: olo_theme::Rgba = [0, 0, 5, 255];

/// The title's face: Times New Roman's, as Windows 95's Setup had (its
/// metric twin, Liberation Serif), bold and italic.
fn title_font() -> Font {
    let mut f = style::ui_font(FontWeight::BOLD);
    f.family = "Liberation Serif".into();
    f.style = FontStyle::Italic;
    f.fallbacks = Some(FontFallbacks::from_fonts(vec!["Noto Serif".into(), "DejaVu Serif".into(), "Nimbus Roman".into()]));
    f
}

/// The window's size on the backdrop.
const DIALOG: (f32, f32) = (660.0, 470.0);
/// Its tab's height.
const TAB: f32 = 24.0;
/// The picture panel's width.
const PICTURE: f32 = 168.0;

/// A BeOS window drawn on the backdrop: the tab (its title in it), then
/// the grey panel with its frame, holding `body`.
fn beos_window(title: &'static str, body: impl IntoElement, w: f32, h: f32) -> Element {
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
        .h(px(h - TAB))
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
        .child(div().absolute().size_full().p(px(m::WINDOW_INSET)).child(body));
    div().flex().flex_col().child(tab).child(panel)
}

/// Setup's picture down the window's left: a sunken well, dark, with an
/// icon in it.
fn picture(icon: &'static str) -> Element {
    canvas(move |g, b| {
        let p = theme::palette();
        let r = style::snap(g, b);
        style::vgradient(g, r, style::c(SKY), style::c(NIGHT));
        style::bevel(g, r, style::c(p.frame_focused[4]), style::c(p.frame_focused[1]));
        let at = style::snap_centred(g, point(r.center().x, r.origin.y + px(96.0)), 128.0, 128.0);
        icons::paint(g, at, icon);
    })
    .paint_key(style::key(icon))
    .w(px(PICTURE))
    .h_full()
    .flex_shrink_0()
}

/// A heading, as Setup's pages begin.
fn heading(text: impl Into<SharedString>) -> StringView {
    StringView::new(text).bold(true).size(15.0).height(22.0)
}

fn para(text: impl Into<SharedString>) -> StringView {
    StringView::new(text).wrap(true)
}

impl Setup {
    /// This page's title, picture, words and controls.
    fn page_body(&mut self, cx: &mut Cx<Self>) -> (&'static str, &'static str, Element) {
        let p = theme::palette();
        let col = || div().flex().flex_col().gap(px(m::STACK));
        match self.page {
            Page::Welcome => {
                let mut c = col()
                    .child(heading("Welcome to Olo Setup"))
                    .child(para("This program installs Olo, a desktop in the spirit of BeOS, on this computer, beside Fedora's own: you choose Olo or GNOME each time you log in."))
                    .child(para("Setup asks for the administrator's password once, to install what Olo needs from Fedora and to put Olo on the login screen. It can be stopped and run again at any time: it picks up where it left off."))
                    .child(para("It is strongly recommended that you save your work and close other programs before going on."));
                if let Some(v) = &self.computer.installed {
                    c = c.child(para(format!("Olo {v} is installed already: Setup puts the newest in its place.")));
                }
                match plan::unsuitable(&self.computer) {
                    Some(why) => c = c.child(StringView::new(why).wrap(true).color(style::c(p.failure))),
                    None => c = c.child(para("Click Next to go on.")),
                }
                ("Welcome", "installer", c)
            }
            Page::Licence => {
                let c = col()
                    .h_full()
                    .child(heading("Licence Agreement"))
                    .child(para("Please read the licence agreement. Scroll down to see the rest of it."))
                    .child(div().flex_1().min_h_0().child(ScrollView::new(&self.licence).child(div().p(px(m::TEXT_INSET)).child(StringView::new(LICENCE).wrap(true)))))
                    .child(RadioButton::new("I accept the agreement", self.accepted).id("accept").on_select(cx.msg(Msg::Accept(true))))
                    .child(RadioButton::new("I don't accept the agreement", !self.accepted).id("refuse").on_select(cx.msg(Msg::Accept(false))));
                ("Licence Agreement", "document", c)
            }
            Page::Choose => {
                let ch = self.choices;
                // Each choice with its words under it, a little apart from the next.
                let choice = |check: CheckBox, t: &str| {
                    div().flex().flex_col().gap(px(2.0)).child(check).child(div().pl(px(m::MARK_SIZE + m::MARK_GAP + 4.0)).child(StringView::new(t.to_owned()).wrap(true).dim(true)))
                };
                let nvidia = if self.computer.nvidia { " This computer has an NVIDIA graphics card." } else { "" };
                // (Installed before: kept chosen, and said so.)
                let had = self.computer.had;
                let updated = |on: bool, words: &str| if on { format!("{words} Installed already: Setup brings it up to date.") } else { words.to_owned() };
                let c = col()
                    .child(heading("Choose What to Install"))
                    .child(choice(
                        CheckBox::new("Olo Desktop", true).id("desktop").enabled(false),
                        "The desktop itself and its apps, in your home folder (~/.local), and Olo on the login screen's session menu. Always installed.",
                    ))
                    .child(choice(
                        CheckBox::new("Olo's login screen", ch.login_screen).id("login-screen").on_toggle(cx.msg_with(|on: &bool| Msg::LoginScreen(*on))),
                        &updated(had.login_screen, "Olo's own login screen in place of GNOME's (GDM, which stays installed). Your password there also opens Olo's keyring."),
                    ))
                    .child(choice(
                        CheckBox::new("Olo's boot splash", ch.boot_splash).id("boot-splash").on_toggle(cx.msg_with(|on: &bool| Msg::BootSplash(*on))),
                        &updated(had.boot_splash, "Olo's picture while the computer starts, in place of Fedora's."),
                    ))
                    .child(choice(
                        CheckBox::new("RPM Fusion", ch.rpm_fusion).id("rpm-fusion").on_toggle(cx.msg_with(|on: &bool| Msg::RpmFusion(*on))),
                        &if had.rpm_fusion {
                            "Fedora's companion repositories: NVIDIA's own driver and more video formats. Added already.".to_owned()
                        } else {
                            format!("Fedora's companion repositories: NVIDIA's own driver and more video formats.{nvidia}")
                        },
                    ));
                ("Choose What to Install", "preferences", c)
            }
            Page::Installing => {
                let failed = matches!(self.outcome, Some(Outcome::Failed { .. }));
                // As BeOS's status bars: what it's doing over the bar, how
                // far through at its right.
                let (title, through) = match &self.stage {
                    Some((n, of, t)) => (format!("{t}…"), format!("{n} of {of}")),
                    None => ("Starting…".into(), String::new()),
                };
                let doing = if self.password { "Waiting for the administrator's password…".to_owned() } else { self.doing.clone() };
                let mut c = col()
                    .h_full()
                    // (What has a size of its own keeps it: the notes give way.)
                    .child(div().flex_shrink_0().child(heading("Installing Olo")))
                    .child(div().flex().flex_shrink_0().child(div().flex_1().min_w_0().child(StatusBar::new(self.overall()).label(title).trailing(through))))
                    .child(div().flex_shrink_0().child(StringView::new(doing).wrap(true).dim(true)));
                if let Some(Outcome::Failed { what, why, advice }) = &self.outcome {
                    // ("Adding Olo to…" as part of a sentence: its first letter small.)
                    let what = what.chars().next().map(|f| f.to_lowercase().chain(what.chars().skip(1)).collect::<String>()).unwrap_or_default();
                    c = c
                        .child(div().flex_shrink_0().child(StringView::new(format!("Setup couldn't finish {what}: {why}")).wrap(true).color(style::c(p.failure))))
                        .child(div().flex_shrink_0().child(para(advice.clone())));
                }
                let mut notes = div().flex().flex_col().gap(px(2.0)).p(px(m::TEXT_INSET));
                for n in &self.notes {
                    notes = notes.child(StringView::new(n.clone()).wrap(true).dim(true));
                }
                c = c.child(div().flex_1().min_h_0().child(ScrollView::new(&self.notes_scroll).child(notes)));
                (if failed { "Setup Stopped" } else { "Installing" }, if failed { "warning" } else { "archive" }, c)
            }
            Page::Finish => {
                let c = match &self.outcome {
                    Some(Outcome::Done { version, restart }) => {
                        let how = if *restart {
                            "Restart the computer: Olo's login screen greets you, and Olo starts when you log in."
                        } else {
                            "Log out, then choose “Olo Desktop” from the login screen's session menu (the gear by Sign In) before you log in again. Fedora's GNOME stays there too."
                        };
                        col().child(heading("Olo Is Installed")).child(para(format!("Olo {version} is installed on this computer."))).child(para(how))
                    }
                    _ => col().child(heading("Setup Stopped")).child(para("Olo isn't installed yet. Run Setup again to go on: it keeps what it has done and downloaded.")),
                };
                let c = c.child(
                    div().pt(px(m::SECTION)).child(StringView::new("Olo is a work of love, dedicated to Sir Samwise, the bestest English Labrador that ever walked the earth.").wrap(true).dim(true)),
                );
                ("Finish", "olo", c)
            }
        }
    }

    /// The buttons along the bottom.
    fn buttons(&self, cx: &mut Cx<Self>) -> Element {
        let failed = matches!(self.outcome, Some(Outcome::Failed { .. }));
        let mut left = div().flex().gap(px(m::SPACING));
        if self.page == Page::Welcome {
            left = left.child(Button::new("About…").id("about").on_click(cx.msg(Msg::About(true))));
        }
        let mut right = div().flex().gap(px(m::SPACING));
        match self.page {
            Page::Finish => {
                let restart = matches!(self.outcome, Some(Outcome::Done { restart: true, .. }));
                let installed = matches!(self.outcome, Some(Outcome::Done { .. }));
                if installed {
                    right = right.child(Button::new(if restart { "Restart" } else { "Log Out" }).id("leave").on_click(cx.msg(Msg::Leave(restart))));
                }
                right = right.child(Button::new("Finish").id("finish").default(true).on_click(cx.msg(Msg::Quit)));
            }
            Page::Installing if failed => {
                right = right.child(Button::new("Try Again").id("again").default(true).on_click(cx.msg(Msg::Again))).child(Button::new("Quit").id("quit").on_click(cx.msg(Msg::Quit)));
            }
            Page::Installing => {
                right = right.child(Button::new("< Back").enabled(false)).child(Button::new("Next >").enabled(false)).child(Button::new("Cancel").id("cancel").on_click(cx.msg(Msg::Stop)));
            }
            _ => {
                let can_next = match self.page {
                    Page::Welcome => plan::unsuitable(&self.computer).is_none(),
                    Page::Licence => self.accepted,
                    _ => true,
                };
                let label = if self.page == Page::Choose { "Install" } else { "Next >" };
                right = right
                    .child(Button::new("< Back").id("back").enabled(self.page != Page::Welcome).on_click(cx.msg(Msg::Back)))
                    .child(Button::new(label).id("next").default(true).enabled(can_next).on_click(cx.msg(Msg::Next)))
                    .child(Button::new("Cancel").id("cancel").on_click(cx.msg(Msg::Quit)));
            }
        }
        div().flex().items_center().justify_between().child(left).child(right)
    }
}

/// The About window: what Setup is, and for whom.
fn about(cx: &mut Cx<Setup>) -> Element {
    let icon = canvas(|g, b| {
        let at = style::snap_centred(g, b.center(), 64.0, 64.0);
        icons::paint(g, at, "olo");
    })
    .paint_key(0)
    .size(px(64.0))
    .flex_shrink_0();
    let body = div()
        .flex()
        .flex_col()
        .gap(px(m::STACK))
        .child(
            div()
                .flex()
                .gap(px(m::SPACING))
                .items_center()
                .child(icon)
                .child(div().flex().flex_col().child(heading("Olo Setup")).child(StringView::new(format!("Version {}", env!("CARGO_PKG_VERSION"))).dim(true))),
        )
        .child(para("Olo is a desktop in the spirit of BeOS: fast, light, and made to be understood, for people and for the agents working beside them."))
        .child(para("Olo is a work of love, dedicated to Sir Samwise, the bestest English Labrador that ever walked the earth."))
        .child(div().flex().justify_end().child(Button::new("OK").id("about-ok").default(true).on_click(cx.msg(Msg::About(false)))));
    beos_window("About Olo Setup", body, 420.0, 236.0)
}

impl View for Setup {
    type Message = Msg;

    fn message(&mut self, message: Msg, cx: &mut Cx<Self>) {
        match message {
            Msg::Told(e) => return self.told(e, cx),
            Msg::Advance => self.advance(cx),
            Msg::Next => self.next(),
            Msg::Back => self.back(),
            Msg::Accept(on) => self.accepted = on,
            Msg::LoginScreen(on) => self.choices.login_screen = on,
            Msg::BootSplash(on) => self.choices.boot_splash = on,
            Msg::RpmFusion(on) => self.choices.rpm_fusion = on,
            Msg::About(on) => self.about = on,
            Msg::Again => self.start(),
            Msg::Stop => {
                self.stop.store(true, Ordering::Relaxed);
                olo_net::nudge();
                self.doing = "Stopping…".into();
            }
            Msg::Leave(restart) => crate::session::leave(restart),
            Msg::Quit => olo_kit::quit(),
        }
        cx.invalidate();
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let screen = olo_kit::viewport_size();
        let (sw, sh) = (f32::from(screen.width), f32::from(screen.height));
        let backdrop = canvas(|g, b| {
            style::vgradient(g, b, style::c(SKY), style::c(NIGHT));
            let font = title_font();
            let (x, y) = (b.origin.x + px(44.0), b.origin.y + px(58.0));
            // Its shadow, then it: as Windows 95's Setup drew its title.
            style::label(g, x + px(4.0), y + px(4.0), style::Align::Left, "Olo Setup", 46.0, font.clone(), black());
            style::label(g, x, y, style::Align::Left, "Olo Setup", 46.0, font, white());
        })
        .paint_key(0)
        .absolute()
        .size_full();
        let (title, icon, body) = self.page_body(cx);
        let buttons = self.buttons(cx);
        let content = div()
            .size_full()
            .flex()
            .gap(px(m::SPACING + 4.0))
            .child(picture(icon))
            .child(div().flex_1().min_w_0().h_full().flex().flex_col().gap(px(m::SPACING)).child(div().flex_1().min_h_0().child(body)).child(buttons));
        let (w, h) = DIALOG;
        let dialog = div().absolute().left(px(((sw - w) / 2.0).round())).top(px(((sh - h) / 2.0).round().max(110.0))).child(beos_window(title, content, w, h));
        let mut root = div().relative().size_full().child(backdrop).child(dialog);
        if self.about {
            let (aw, ah) = (420.0, 236.0);
            // (What's behind it takes no clicks while it shows.)
            root = root.child(div().absolute().size_full().occlude()).child(div().absolute().left(px(((sw - aw) / 2.0).round() + 40.0)).top(px(((sh - ah) / 2.0).round() + 40.0)).child(about(cx)));
        }
        root
    }
}
