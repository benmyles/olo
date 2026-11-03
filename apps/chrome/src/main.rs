//! olo-chrome: Google Chrome in Olo's applications. When Chrome is
//! installed it starts it, with the arguments it was given; when it isn't,
//! it offers to download it from Google (install.rs), shows the download
//! (which can be cancelled) and starts Chrome once it's in. New profiles
//! get Olo's title bar and the system's colours (prefs.rs). With a Chrome
//! already running on the profile, the link is handed to it directly
//! (singleton.rs), with no second Chrome started.

mod downloads;
mod extension;
mod host;
mod install;
mod prefs;
mod profile;
mod singleton;

use std::{
    os::unix::process::CommandExt,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use install::Progress;
use olo_ui::{
    WindowOptions,
    gpui::{AppContext as _, Context, Entity, FocusHandle, InteractiveElement as _, IntoElement, ParentElement as _, Render, Styled as _, Window, WindowHandle, div, px},
    instance::{self, Launch},
    metrics as m,
    widgets::{
        AlertFrame, Button, StatusBar, StringView,
        alert::{self, ALERT_WIDTH},
        alert_height, button_row, paragraph,
    },
};

const APP_ID: &str = "olo.chrome";

const OFFER: &str = "Google Chrome isn't on this computer yet.\n\nOlo can download it from Google and keep it in your home folder, \
                     with nothing else to install.";

enum State {
    Offer,
    Working(Progress),
    Failed(String),
}

struct Installer {
    args: Vec<String>,
    window: Option<WindowHandle<olo_ui::Frame>>,
    state: State,
    cancel: Arc<AtomicBool>,
    /// The window's keyboard (Return and Escape are the alert's).
    focus: FocusHandle,
}

impl Installer {
    fn open(&mut self, cx: &mut Context<Self>) {
        if let Some(w) = self.window
            && olo_ui::raise(Some(w), cx)
        {
            return;
        }
        let me = cx.entity();
        cx.defer(move |cx| {
            let h = alert_height(cx, OFFER, ALERT_WIDTH) + 8.0;
            let view = me.clone();
            let w = olo_ui::open_window(cx, WindowOptions::new("Google Chrome", ALERT_WIDTH, h).min_size(ALERT_WIDTH, h), move |_, _| view);
            me.update(cx, |i, _| i.window = w);
        });
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        self.cancel = Arc::new(AtomicBool::new(false));
        self.state = State::Working(Progress::Looking);
        let (proxy, cancel) = (olo_ui::proxy(cx), self.cancel.clone());
        std::thread::Builder::new().name("install".into()).spawn(move || install::install(&cancel, &|p| proxy.send(p))).expect("thread");
        cx.notify();
    }

    /// The offer's or the failure's buttons: Cancel quits, the other
    /// (the default) starts the download.
    fn choose(&mut self, i: usize, cx: &mut Context<Self>) {
        if i == 0 {
            self.cancel.store(true, Ordering::Relaxed);
            cx.quit();
        } else {
            self.start(cx);
        }
    }

    fn message(&mut self, message: Box<dyn std::any::Any + Send>, cx: &mut Context<Self>) {
        let message = match message.downcast::<Progress>() {
            Ok(p) => {
                match *p {
                    Progress::Done(chrome) => {
                        start_chrome(&chrome, &self.args);
                        cx.quit();
                    }
                    // Back where it started, to offer again.
                    Progress::Cancelled => self.state = State::Offer,
                    Progress::Failed(why) => self.state = State::Failed(why),
                    p => self.state = State::Working(p),
                }
                cx.notify();
                return;
            }
            Err(m) => m,
        };
        if message.is::<Launch>() {
            self.open(cx);
        }
    }
}

impl Render for Installer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.focus.is_focused(window) {
            window.focus(&self.focus);
        }
        let me = cx.entity();
        let alert_buttons = |labels: [&'static str; 2], me: Entity<Installer>| {
            let mut row = button_row();
            for (i, label) in labels.into_iter().enumerate() {
                let me = me.clone();
                row = row.child(Button::new(label).default(i == 1).on_click(move |_, cx| me.update(cx, |a, cx| a.choose(i, cx))));
            }
            row
        };
        let frame = AlertFrame::new("installer");
        let content = match &self.state {
            State::Offer => frame.child(paragraph(OFFER)).child(div().flex_1()).child(alert_buttons(["Cancel", "Download"], me.clone())),
            State::Failed(why) => frame.child(paragraph(format!("Google Chrome couldn't be installed.\n\n{why}"))).child(div().flex_1()).child(alert_buttons(["Cancel", "Try Again"], me.clone())),
            State::Working(progress) => {
                let (title, fraction, amount, version) = match progress {
                    Progress::Downloading { version, got, total } => {
                        ("Downloading Google Chrome…", if *total > 0 { *got as f32 / *total as f32 } else { 0.0 }, format!("{} of {}", size_text(*got), size_text(*total)), Some(version.clone()))
                    }
                    Progress::Unpacking { version } => ("Installing Google Chrome…", 1.0, String::new(), Some(version.clone())),
                    _ => ("Looking for the newest Google Chrome…", 0.0, String::new(), None),
                };
                let unpacking = matches!(progress, Progress::Unpacking { .. });
                let cancel = self.cancel.clone();
                let mut col = div().flex().flex_col().gap(px(m::SPACING)).child(StatusBar::new(fraction).label(title).trailing(amount));
                if let Some(v) = version {
                    col = col.child(StringView::new(format!("Version {v}, from Google's repository")).dim(true));
                }
                frame.child(col).child(div().flex_1()).child(button_row().child(Button::new("Cancel").enabled(!unpacking).on_click(move |_, _| cancel.store(true, Ordering::Relaxed))))
            }
        };
        // Return takes the default, Escape cancels, as an alert's.
        let (ok, cancel) = (me.clone(), me.clone());
        let offering = !matches!(self.state, State::Working(_));
        let working_cancel = self.cancel.clone();
        div()
            .size_full()
            .key_context(alert::CONTEXT)
            .track_focus(&self.focus)
            .on_action(move |_: &alert::Default, _, cx| {
                if offering {
                    ok.update(cx, |a, cx| a.choose(1, cx));
                }
            })
            .on_action(move |_: &alert::Cancel, _, cx| {
                if offering {
                    cancel.update(cx, |a, cx| a.choose(0, cx));
                } else {
                    working_cancel.store(true, Ordering::Relaxed);
                }
            })
            .child(content)
    }
}

fn size_text(bytes: u64) -> String {
    let mb = bytes as f64 / 1_000_000.0;
    if mb >= 10.0 { format!("{mb:.0} MB") } else { format!("{mb:.1} MB") }
}

/// Chrome's arguments: `args`, told to draw with Wayland under Olo (left
/// to itself it goes by `XDG_SESSION_TYPE`, which a session needn't set,
/// and falls back to X11).
fn chrome_args(args: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(args.len() + 2);
    if std::env::var_os("WAYLAND_DISPLAY").is_some() && !args.iter().any(|a| a.starts_with("--ozone-platform")) {
        out.push("--ozone-platform=wayland".to_owned());
    }
    // (An agent's Chrome isn't asked to be the default browser.)
    if on_screen() && !args.iter().any(|a| a == "--no-default-browser-check") {
        out.push("--no-default-browser-check".to_owned());
    }
    out.extend(args.iter().cloned());
    out
}

/// Started on an agent screen (its profile starts ready: prefs.rs).
fn on_screen() -> bool {
    std::env::var_os("OLO_SCREEN").is_some_and(|s| !s.is_empty())
}

/// A token to bring Chrome's window forward with: a link opened from a
/// shell or an agent has no click to take one from, and without one
/// Chrome opens it in a window that may be on another workspace with
/// nothing to show it.
fn activation_token() -> Option<String> {
    // A fresh one, though the opener may have put its own in our
    // environment: that one is handed to a Chrome started here anyway, but
    // a Chrome running already got none (a link from an app that passes
    // its own token opened in a tab of a window that stayed behind), and
    // one inherited may be long spent. Else the opener's.
    olo_protocol::issue_activation_token(std::time::Duration::from_millis(500)).or_else(|| std::env::var("XDG_ACTIVATION_TOKEN").ok().filter(|t| !t.is_empty()))
}

fn chrome_command(chrome: &Path, args: &[String], token: Option<&str>) -> std::process::Command {
    let mut cmd = std::process::Command::new(chrome);
    cmd.args(chrome_args(args));
    if let Some(token) = token {
        cmd.env("XDG_ACTIVATION_TOKEN", token);
    }
    cmd
}

/// Profile folder `dir` made ready for `chrome`: Olo's defaults (an
/// agent screen's too) and Passwords in it (the extension and its host).
fn set_up_profile(chrome: &Path, dir: &Path) {
    prefs::apply(dir, on_screen());
    if let (Ok(exe), Some(install)) = (std::env::current_exe(), chrome.parent().and_then(|d| d.canonicalize().ok()))
        && let Err(e) = extension::set_up(&install::root(), &install, dir, &exe)
    {
        eprintln!("olo-chrome: couldn't set up Passwords in Chrome: {e}");
    }
}

/// Start Chrome on its own (it outlives us), its profile set up first
/// (the first start after it's installed).
fn start_chrome(chrome: &Path, args: &[String]) {
    if let Some(dir) = prefs::user_data_dir(args) {
        set_up_profile(chrome, &dir);
    }
    let mut cmd = chrome_command(chrome, args, activation_token().as_deref());
    cmd.process_group(0);
    if let Err(e) = cmd.spawn() {
        tracing::warn!("couldn't start Chrome: {e}");
    }
}

fn main() {
    // A session or install from before the rename (Tabula) carries on.
    olo_xdg::legacy::adopt();
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Chrome starting us as the Passwords extension's host.
    if args.first().is_some_and(|a| a.starts_with("chrome-extension://")) {
        return host::run();
    }
    if let Some(chrome) = install::installed() {
        // Its icons and entry (an install from before, or another place
        // this program now lives in).
        if let Ok(exe) = std::env::current_exe()
            && !install::entry_is_current(&exe)
            && let Some(dir) = chrome.parent()
        {
            let _ = install::set_up(&dir.canonicalize().unwrap_or_else(|_| dir.to_owned()));
        }
        // Once a day, a newer Chrome is fetched in the background (it is
        // used from the next start on).
        if args.first().map(String::as_str) == Some("--olo-update") {
            install::update_quietly();
            return;
        }
        if install::update_due()
            && let Ok(exe) = std::env::current_exe()
        {
            let _ = std::process::Command::new(exe).arg("--olo-update").stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).process_group(0).spawn();
        }
        let token = activation_token();
        let dir = prefs::user_data_dir(&args);
        // A Chrome running on the profile: the link handed to it.
        if let Some(dir) = &dir {
            let argv: Vec<std::ffi::OsString> = std::iter::once(chrome.clone().into_os_string()).chain(chrome_args(&args).into_iter().map(Into::into)).collect();
            if singleton::hand_over(dir, &argv, token.as_deref()) {
                return;
            }
        }
        if let Some(dir) = dir {
            set_up_profile(&chrome, &dir);
        }
        let e = chrome_command(&chrome, &args, token.as_deref()).exec();
        eprintln!("olo-chrome: couldn't start {}: {e}", chrome.display());
        std::process::exit(1);
    }
    if instance::forward(APP_ID, &args) {
        return;
    }
    olo_ui::run(APP_ID, move |cx| {
        instance::listen(APP_ID, olo_ui::proxy(cx));
        let app = cx.new(|cx| Installer { args, window: None, state: State::Offer, cancel: Arc::new(AtomicBool::new(false)), focus: cx.focus_handle() });
        app.update(cx, |a, cx| a.open(cx));
        let messages = app.clone();
        olo_ui::message_handler(cx, move |message, cx| messages.update(cx, |a, cx| a.message(message, cx)));
        // Closed from its tab: the download stops.
        let closing = app.clone();
        cx.on_app_quit(move |cx| {
            closing.read(cx).cancel.store(true, Ordering::Relaxed);
            async {}
        })
        .detach();
    });
}
