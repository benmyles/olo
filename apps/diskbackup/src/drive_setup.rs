//! Back Up to Google Drive (Back Up To ▸ Google Drive…): Google asks each
//! person to make their own sign-in for an app like DiskBackup (a "Desktop
//! app" OAuth client in a Google Cloud project of theirs), so this window
//! walks them through it, a step at a time, each with an Open button for
//! its page in the browser; then takes the client (its downloaded file,
//! or its ID and secret pasted) and signs in with it (the browser to
//! Google's consent page, the answer back to DiskBackup). A client kept
//! in Passwords (another computer's) skips the steps; or an agent does
//! them (Let an Agent Do It…: `agent_setup`).

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering::Relaxed},
};

use olo_diskbackup::oauth::Client;
use olo_keychain::Shareable;
use olo_ui::{
    Proxy,
    file_panel::{self, FilePanel, PanelResult},
    gpui::{self, Context, Entity, IntoElement, ParentElement as _, Render, SharedString, Styled as _, Window, div, px},
    metrics as m, style, theme,
    widgets::{Button, MenuField, StringView, TextControl, TextField, TextFieldEvent},
};

use crate::{app::Msg, target};

/// The steps: what, how, and the page that does it.
pub const STEPS: &[(&str, &str, &str)] = &[
    ("Make a project", "In Google Cloud, signed in with the account whose Drive gets the backups: name it (“Olo DiskBackup”), then Create.", "https://console.cloud.google.com/projectcreate"),
    ("Turn on the Drive API", "With that project chosen at the top of the page: Enable.", "https://console.cloud.google.com/flows/enableapi?apiid=drive.googleapis.com"),
    (
        "Set up its sign-in",
        "Get Started: the app's name (“Olo DiskBackup”) and your email; Audience: Internal for a Google Workspace account (you@yourcompany.com: no publishing, and its sign-in never expires), External for a Gmail one; your email again; agree, then Create.",
        "https://console.cloud.google.com/auth/overview",
    ),
    (
        "Let it see its own files",
        "Add or Remove Scopes: tick “…/auth/drive.file”, then Update and Save. It sees only the files DiskBackup makes, nothing else in your Drive.",
        "https://console.cloud.google.com/auth/scopes",
    ),
    (
        "Publish it (External only)",
        "Google asks first for a homepage and a privacy-policy address (Branding); then Publish App, and Confirm. Until it's published Google signs DiskBackup out every 7 days; seeing only its own files, it needs no review. Internal: nothing to do.",
        "https://console.cloud.google.com/auth/audience",
    ),
    ("Make its desktop client", "Create Client: Application type Desktop app, any name, then Create; Download JSON.", "https://console.cloud.google.com/auth/clients/create"),
];

/// What a worker tells the window.
pub enum Done {
    /// The client's file read (or why not).
    Chosen(Result<Client, String>),
    /// Passwords' client, if it keeps one.
    Kept(Result<Option<Client>, String>),
    /// The consent page's address (to copy, if the browser didn't open).
    Started(String),
    /// Signed in and set up: whose Drive (or why not).
    SetUp(Result<String, String>),
    /// Agent Setup: the person's Google logins in Passwords (or why not).
    Logins(Result<Vec<Shareable>, String>),
    /// Agent Setup started: its screen (or why not).
    AgentStarted(Result<String, String>),
}

/// Agent Setup, as it goes.
enum Agent {
    Off,
    /// Choosing the Google login (None: Passwords asked).
    Choosing {
        logins: Option<Result<Vec<Shareable>, String>>,
        chosen: usize,
    },
    Starting,
    /// The agent at work, on this screen.
    Started(String),
}

pub struct DriveSetup {
    proxy: Proxy,
    id: Entity<TextField>,
    secret: Entity<TextField>,
    /// What to say (and whether it's an error).
    status: (String, bool),
    /// Signing in: its Stop.
    signing: Option<Arc<AtomicBool>>,
    /// Looking in Passwords.
    looking: bool,
    /// Set up: whose Drive.
    done: Option<String>,
    /// The consent page's address, while signing in.
    url: Option<String>,
    agent: Agent,
    _subs: Vec<gpui::Subscription>,
}

impl DriveSetup {
    pub fn new(proxy: Proxy, cx: &mut Context<Self>) -> DriveSetup {
        let id = cx.new(|cx| TextField::new(cx).hint("123456789012-abc….apps.googleusercontent.com").name("Client ID"));
        let secret = cx.new(|cx| TextField::new(cx).hint("GOCSPX-…").password(true).name("Client secret"));
        let subs = [&id, &secret]
            .into_iter()
            .map(|f| {
                cx.subscribe(f, |w: &mut DriveSetup, _, e: &TextFieldEvent, cx| {
                    if matches!(e, TextFieldEvent::Changed) {
                        w.status = (String::new(), false);
                        cx.notify();
                    }
                })
            })
            .collect();
        DriveSetup { proxy, id, secret, status: (String::new(), false), signing: None, looking: false, done: None, url: None, agent: Agent::Off, _subs: subs }
    }

    pub fn message(&mut self, d: Done, cx: &mut Context<Self>) {
        match d {
            Done::Chosen(Ok(c)) => {
                self.fill(&c, cx);
                self.status = ("Read. Now Sign In with Google.".into(), false);
            }
            Done::Chosen(Err(e)) => self.status = (format!("That file can't be used: {e}."), true),
            Done::Kept(r) => {
                self.looking = false;
                match r {
                    Ok(Some(c)) => {
                        self.fill(&c, cx);
                        return self.sign_in(cx);
                    }
                    Ok(None) => self.status = ("Passwords keeps no Google client of DiskBackup's: make one with the steps above.".into(), true),
                    Err(e) => self.status = (format!("Passwords couldn't say: {e}."), true),
                }
            }
            Done::Started(url) => self.url = Some(url),
            Done::Logins(r) => {
                if let Agent::Choosing { logins, .. } = &mut self.agent {
                    *logins = Some(r);
                }
            }
            Done::AgentStarted(Ok(screen)) => {
                self.status = (String::new(), false);
                self.agent = Agent::Started(screen);
            }
            Done::AgentStarted(Err(e)) => {
                self.agent = Agent::Off;
                self.status = (format!("The agent didn't start: {e}."), true);
            }
            Done::SetUp(r) => {
                self.signing = None;
                self.url = None;
                match r {
                    Ok(email) => {
                        self.status = (format!("Backing up to Google Drive ({email}), in the folder “{}”.", olo_diskbackup::drive::FOLDER), false);
                        self.done = Some(email);
                    }
                    Err(e) => self.status = (format!("Not set up: {e}."), true),
                }
            }
        }
        cx.notify();
    }

    fn fill(&mut self, c: &Client, cx: &mut Context<Self>) {
        self.id.update(cx, |f, cx| f.set_text(c.id.clone(), cx));
        self.secret.update(cx, |f, cx| f.set_text(c.secret.clone(), cx));
    }

    fn client(&self, cx: &gpui::App) -> Client {
        Client { id: self.id.read(cx).text().trim().to_owned(), secret: self.secret.read(cx).text().trim().to_owned() }
    }

    fn choose_file(&mut self, cx: &mut Context<Self>) {
        let proxy = self.proxy.clone();
        let downloads = target::home().join("Downloads");
        let dir = if downloads.is_dir() { downloads } else { target::home() };
        cx.defer(move |cx| {
            file_panel::show(cx, "Choose the Client's File", FilePanel::open(dir).types(&["json"]), move |a, _| {
                if let PanelResult::Chosen(path) = a.result {
                    let r = std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| Client::from_json(&t));
                    proxy.send(Msg::Drive(Done::Chosen(r)));
                }
            });
        });
    }

    /// Agent Setup: the Google logins to choose from, asked of Passwords.
    fn choose_agent_login(&mut self, cx: &mut Context<Self>) {
        self.agent = Agent::Choosing { logins: None, chosen: 0 };
        self.status = (String::new(), false);
        cx.notify();
        let proxy = self.proxy.clone();
        std::thread::spawn(move || proxy.send(Msg::Drive(Done::Logins(crate::agent_setup::google_logins()))));
    }

    /// Agent Setup with the login chosen: its screen, the login shared,
    /// Claude Code started (on a worker).
    fn start_agent(&mut self, cx: &mut Context<Self>) {
        let Agent::Choosing { logins: Some(Ok(logins)), chosen } = &self.agent else { return };
        let Some(login) = logins.get(*chosen).cloned() else { return };
        self.agent = Agent::Starting;
        cx.notify();
        let proxy = self.proxy.clone();
        std::thread::spawn(move || proxy.send(Msg::Drive(Done::AgentStarted(crate::agent_setup::start(&login)))));
    }

    fn use_passwords_client(&mut self, cx: &mut Context<Self>) {
        self.looking = true;
        self.status = ("Looking in Passwords (unlock it if it asks)…".into(), false);
        cx.notify();
        let proxy = self.proxy.clone();
        std::thread::spawn(move || proxy.send(Msg::Drive(Done::Kept(target::client_in_passwords()))));
    }

    /// Sign in (the browser to Google's page), then set up, on a worker.
    fn sign_in(&mut self, cx: &mut Context<Self>) {
        let client = self.client(cx);
        if let Err(e) = client.check() {
            self.status = (format!("Not yet: {e}."), true);
            cx.notify();
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.signing = Some(cancel.clone());
        self.status = ("Sign in to Google in your browser (didn't it open? Copy Address, and open it there): DiskBackup waits for it here…".into(), false);
        cx.notify();
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let r = crate::sign_in(&client, &cancel, |url| {
                proxy.send(Msg::Drive(Done::Started(url.to_owned())));
                // (A test's own Google: the address in the test's folder,
                // for it to play the browser.)
                if std::env::var_os("OLO_DISKBACKUP_GOOGLE_API").is_some()
                    && let Some(dir) = std::env::var_os("OLO_DISKBACKUP_DIR")
                {
                    let _ = std::fs::write(std::path::Path::new(&dir).join("sign-in.url"), url);
                }
                // (Not with a test's own Google: no browser.)
                if std::env::var_os("OLO_DISKBACKUP_GOOGLE_API").is_none()
                    && let Err(e) = olo_xdg::open::open(&olo_xdg::open::Target::Url(url.to_owned()))
                {
                    tracing::warn!("diskbackup: the browser didn't open: {e}");
                }
            })
            .and_then(|signed| crate::set_up_drive(client, signed));
            proxy.send(Msg::Drive(Done::SetUp(r)));
        });
    }
}

fn open_page(url: &'static str) {
    if let Err(e) = olo_xdg::open::open(&olo_xdg::open::Target::Url(url.into())) {
        tracing::warn!("diskbackup: {url}: {e}");
    }
}

impl Render for DriveSetup {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = theme::palette(cx);
        let me = cx.entity();
        let dim = |text: &str| StringView::new(SharedString::from(text.to_owned())).wrap(true).dim(true);
        let busy = self.signing.is_some() || self.looking;

        // (As wide as its frame: a tab beside DiskBackup's window, it shares
        // that one's; 640 points at least.)
        let mut body = div().flex().flex_col().gap(px(m::SPACING)).p(px(m::WINDOW_INSET)).w_full().min_w(px(640.0));
        body = body.child(dim(
            "Google asks each person to make their own sign-in for an app like DiskBackup. It takes a few minutes, once: follow these steps in your browser (each Open shows its page), then sign in below. Your backups are encrypted before they leave this computer.",
        ));
        // Made on another computer: kept in Passwords.
        let kept = me.clone();
        body = body.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(m::SPACING))
                .child(div().flex_1().min_w_0().child(dim("Made one on another computer? Passwords keeps it.")))
                .child(Button::new("Sign In with the One in Passwords").enabled(!busy && self.done.is_none()).on_click(move |_, cx| kept.update(cx, |w, cx| w.use_passwords_client(cx)))),
        );
        // Or an agent does the steps.
        body = body.child(self.agent_part(&me, busy, cx));
        // The steps.
        let number = |n: usize| div().w(px(22.0)).flex_none().child(StringView::new(SharedString::from(format!("{n}."))).bold(true));
        for (i, (title, how, url)) in STEPS.iter().enumerate() {
            let url = *url;
            body = body.child(
                div()
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap(px(m::SPACING))
                    .child(number(i + 1))
                    .child(div().flex_1().min_w_0().flex().flex_col().child(StringView::new(SharedString::from(*title)).bold(true)).child(dim(how)))
                    .child(Button::new("Open").id(format!("open-{i}")).on_click(move |_, _| open_page(url))),
            );
        }
        // The client, then signing in.
        let (file, sign, stop) = (me.clone(), me.clone(), self.signing.clone());
        let last = div()
            .flex()
            .flex_row()
            .items_start()
            .gap(px(m::SPACING))
            .child(number(STEPS.len() + 1))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(m::TIGHT))
                    .child(StringView::new("Give it to DiskBackup").bold(true))
                    .child(dim("Choose the file you downloaded, or copy its Client ID and Client secret in."))
                    .child(TextControl::new(self.id.clone()).label("Client ID:").divider(100.0))
                    .child(TextControl::new(self.secret.clone()).label("Client secret:").divider(100.0)),
            )
            .child(Button::new("Choose File…").enabled(!busy).on_click(move |_, cx| file.update(cx, |w, cx| w.choose_file(cx))));
        body = body.child(last);
        let (text, error) = self.status.clone();
        body = body.child(StringView::new(if text.is_empty() { " ".into() } else { text }).wrap(true).color(if error { style::c(p.failure) } else { style::disabled_text(&p) }));
        let mut buttons = div().flex().flex_row().items_center().justify_end().gap(px(m::SPACING));
        // The browser didn't open (or it's another computer's): its address.
        if let Some(url) = self.url.clone() {
            buttons = buttons.child(Button::new("Copy Address").on_click(move |_, cx| cx.write_to_clipboard(gpui::ClipboardItem::new_string(url.clone()))));
        }
        if let Some(stop) = stop {
            buttons = buttons.child(Button::new("Stop").on_click(move |_, _| stop.store(true, Relaxed)));
        }
        if self.done.is_some() {
            buttons = buttons.child(Button::new("Done").default(true).on_click(|window, _| window.remove_window()));
        } else {
            let ready = self.client(cx).check().is_ok();
            // (The default once it can be pressed.)
            buttons = buttons.child(Button::new("Sign In with Google").default(ready && !busy).enabled(ready && !busy).on_click(move |_, cx| sign.update(cx, |w, cx| w.sign_in(cx))));
        }
        body.child(buttons)
    }
}

impl DriveSetup {
    /// Agent Setup's part of the window: its button, then the Google login
    /// to choose, then what the agent is doing.
    fn agent_part(&self, me: &Entity<Self>, busy: bool, cx: &mut Context<Self>) -> gpui::Div {
        let p = theme::palette(cx);
        let dim = |text: String| StringView::new(SharedString::from(text)).wrap(true).dim(true);
        let row = || div().flex().flex_row().items_center().gap(px(m::SPACING));
        match &self.agent {
            Agent::Off => {
                let go = me.clone();
                row()
                    .child(div().flex_1().min_w_0().child(dim("Or let an agent do the steps: Claude Code, in a Terminal window you can follow, on an agent screen of its own signed in with your Google login from Passwords.".into())))
                    .child(Button::new("Let an Agent Do It…").enabled(!busy && self.done.is_none()).on_click(move |_, cx| go.update(cx, |w, cx| w.choose_agent_login(cx))))
            }
            Agent::Choosing { logins, chosen } => {
                let mut col = div().flex().flex_col().gap(px(m::TIGHT)).child(StringView::new("Agent Setup").bold(true)).child(dim(
                    "Which Google account do the backups go in? Its login in Passwords is shared with the agent’s screen, to sign in there; the agent never sees its password."
                        .into(),
                ));
                let (cancel, start) = (me.clone(), me.clone());
                let cancel = Button::new("Cancel").id("agent-cancel").on_click(move |_, cx| {
                    cancel.update(cx, |w, cx| {
                        w.agent = Agent::Off;
                        cx.notify();
                    })
                });
                match logins {
                    None => col = col.child(dim("Asking Passwords (unlock it if it asks)…".into())).child(row().justify_end().child(cancel)),
                    Some(Err(e)) => col = col.child(StringView::new(SharedString::from(format!("Passwords couldn't say: {e}."))).wrap(true).color(style::c(p.failure))).child(row().justify_end().child(cancel)),
                    Some(Ok(l)) if l.is_empty() => col = col.child(dim("Passwords has no logins to share: add your Google account’s there first.".into())).child(row().justify_end().child(cancel)),
                    Some(Ok(l)) => {
                        let names: Vec<SharedString> = l.iter().map(|i| SharedString::from(crate::agent_setup::label(i))).collect();
                        let pick = me.clone();
                        col = col.child(MenuField::new("agent-login", names, *chosen).label("Google login:").divider(100.0).on_change(move |i, _, cx| {
                            pick.update(cx, |w, cx| {
                                if let Agent::Choosing { chosen, .. } = &mut w.agent {
                                    *chosen = i;
                                }
                                cx.notify();
                            })
                        }));
                        col = col.child(row().justify_end().child(cancel).child(Button::new("Start").id("agent-start").default(true).on_click(move |_, cx| start.update(cx, |w, cx| w.start_agent(cx)))));
                    }
                }
                col
            }
            Agent::Starting => row().child(dim("Starting the agent’s screen, sharing the login with it, and opening Claude Code in Terminal…".into())),
            Agent::Started(screen) => row().child(dim(format!(
                "Claude Code is doing the steps in a Terminal window, on agent screen “{screen}” (Agent Screens shows it live). Follow along, and tell it anything there. DiskBackup is set up here when it’s done."
            ))),
        }
    }
}

/// The window's options.
pub fn options() -> olo_ui::WindowOptions {
    olo_ui::WindowOptions::new("Back Up to Google Drive", 640.0, 600.0).fits_content()
}

#[allow(unused_imports)]
use gpui::AppContext as _;
