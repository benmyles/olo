//! tabula-chrome: Google Chrome in Tabula's applications. When Chrome is
//! installed it starts it, with the arguments it was given; when it isn't,
//! it offers to download it from Google (install.rs), shows the download
//! (which can be cancelled) and starts Chrome once it's in. New profiles
//! get Tabula's title bar and the system's colours (prefs.rs).

mod extension;
mod host;
mod install;
mod prefs;

use std::{
    os::unix::process::CommandExt,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use install::Progress;
use tabula_kit::{
    App, Cx, WindowId, WindowOptions,
    egui::{self, Rect, pos2},
    instance::{self, Launch},
    metrics as m,
    widgets::{self, Button, StatusBar},
};

const APP_ID: &str = "tabula.chrome";

const OFFER: &str = "Google Chrome isn't on this computer yet.\n\nTabula can download it from Google and keep it in your home folder, \
                     with nothing else to install.";

enum State {
    Offer,
    Working(Progress),
    Failed(String),
}

struct Installer {
    args: Vec<String>,
    window: Option<WindowId>,
    state: State,
    cancel: Arc<AtomicBool>,
}

impl Installer {
    fn open(&mut self, cx: &mut Cx) {
        if let Some(w) = self.window.filter(|w| cx.is_open(*w)) {
            cx.activate(w);
            return;
        }
        let h = widgets::alert_height(OFFER) + 8;
        self.window = Some(cx.open_window(WindowOptions::new("Google Chrome", widgets::ALERT_WIDTH, h).min_size(widgets::ALERT_WIDTH, h)));
    }

    fn start(&mut self, cx: &mut Cx) {
        self.cancel = Arc::new(AtomicBool::new(false));
        self.state = State::Working(Progress::Looking);
        let (proxy, cancel) = (cx.proxy(), self.cancel.clone());
        std::thread::Builder::new()
            .name("install".into())
            .spawn(move || install::install(&cancel, &|p| proxy.send(p)))
            .expect("thread");
    }
}

impl App for Installer {
    fn ui(&mut self, cx: &mut Cx, _window: WindowId, ui: &mut egui::Ui) {
        match &self.state {
            State::Offer => match widgets::alert(ui, "installer", OFFER, &["Cancel", "Download"]) {
                Some(0) => cx.quit(),
                Some(_) => self.start(cx),
                None => {}
            },
            State::Failed(why) => {
                let message = format!("Google Chrome couldn't be installed.\n\n{why}");
                match widgets::alert(ui, "installer", &message, &["Cancel", "Try Again"]) {
                    Some(0) => cx.quit(),
                    Some(_) => self.start(cx),
                    None => {}
                }
            }
            State::Working(progress) => {
                let full = ui.available_rect_before_wrap();
                let column = widgets::alert_frame(ui, "installer");
                let (title, fraction, amount, version) = match progress {
                    Progress::Downloading { version, got, total } => (
                        "Downloading Google Chrome…",
                        if *total > 0 { *got as f32 / *total as f32 } else { 0.0 },
                        format!("{} of {}", size_text(*got), size_text(*total)),
                        Some(version),
                    ),
                    Progress::Unpacking { version } => ("Installing Google Chrome…", 1.0, String::new(), Some(version)),
                    _ => ("Looking for the newest Google Chrome…", 0.0, String::new(), None),
                };
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(column).layout(egui::Layout::top_down(egui::Align::Min)));
                child.spacing_mut().item_spacing.y = m::SPACING;
                child.add(StatusBar::new(fraction).label(title).trailing(&amount));
                if let Some(v) = version {
                    child.add(widgets::StringView::new(&format!("Version {v}, from Google's repository")).dim(true));
                }
                let row = Rect::from_min_max(pos2(column.left(), full.bottom() - m::WINDOW_INSET - m::CONTROL_HEIGHT), pos2(column.right(), column.bottom()));
                let unpacking = matches!(progress, Progress::Unpacking { .. });
                ui.scope_builder(egui::UiBuilder::new().max_rect(row), |ui| {
                    widgets::button_row(ui, |ui| {
                        if ui.add_enabled(!unpacking, Button::new("Cancel")).clicked() {
                            self.cancel.store(true, Ordering::Relaxed);
                        }
                    });
                });
            }
        }
    }

    fn close_requested(&mut self, cx: &mut Cx, _window: WindowId) {
        self.cancel.store(true, Ordering::Relaxed);
        cx.quit();
    }

    fn message(&mut self, cx: &mut Cx, message: Box<dyn std::any::Any + Send>) {
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
                if let Some(w) = self.window {
                    cx.redraw(w);
                }
                return;
            }
            Err(m) => m,
        };
        if message.is::<Launch>() {
            self.open(cx);
        }
    }
}

fn size_text(bytes: u64) -> String {
    let mb = bytes as f64 / 1_000_000.0;
    if mb >= 10.0 { format!("{mb:.0} MB") } else { format!("{mb:.1} MB") }
}

/// Chrome, told to draw with Wayland under Tabula (left to itself it
/// goes by `XDG_SESSION_TYPE`, which a session needn't set, and falls back
/// to X11).
fn chrome_command(chrome: &Path, args: &[String]) -> std::process::Command {
    let mut cmd = std::process::Command::new(chrome);
    if std::env::var_os("WAYLAND_DISPLAY").is_some() && !args.iter().any(|a| a.starts_with("--ozone-platform")) {
        cmd.arg("--ozone-platform=wayland");
    }
    cmd.args(args);
    cmd
}

/// Start Chrome on its own (it outlives us).
fn start_chrome(chrome: &Path, args: &[String]) {
    let mut cmd = chrome_command(chrome, args);
    cmd.process_group(0);
    if let Err(e) = cmd.spawn() {
        tracing::warn!("couldn't start Chrome: {e}");
    }
}

fn main() {
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
        if args.first().map(String::as_str) == Some("--tabula-update") {
            install::update_quietly();
            return;
        }
        if install::update_due()
            && let Ok(exe) = std::env::current_exe()
        {
            let _ = std::process::Command::new(exe)
                .arg("--tabula-update")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .process_group(0)
                .spawn();
        }
        if let Some(dir) = prefs::user_data_dir(&args) {
            prefs::apply(&dir);
            // Passwords in Chrome: the extension and its host.
            if let (Ok(exe), Some(install)) = (std::env::current_exe(), chrome.parent().and_then(|d| d.canonicalize().ok()))
                && let Err(e) = extension::set_up(&install::root(), &install, &dir, &exe)
            {
                eprintln!("tabula-chrome: couldn't set up Passwords in Chrome: {e}");
            }
        }
        let e = chrome_command(&chrome, &args).exec();
        eprintln!("tabula-chrome: couldn't start {}: {e}", chrome.display());
        std::process::exit(1);
    }
    if instance::forward(APP_ID, &args) {
        return;
    }
    tabula_kit::run(APP_ID, |cx| {
        instance::listen(APP_ID, cx.proxy());
        let mut app = Installer { args, window: None, state: State::Offer, cancel: Arc::new(AtomicBool::new(false)) };
        app.open(cx);
        app
    });
}
