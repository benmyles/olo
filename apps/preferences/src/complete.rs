//! Terminal ▸ Command Suggestions: brush's grey hints from olo-complete's
//! tiny model of the person's own commands (crates/complete,
//! apps/complete; HACKING, "brush's suggestions"). Its setting is
//! olo-complete's own file (`~/.config/olo/complete.json`, which brush
//! and the daemon read with or without Olo's session). Whatever waits —
//! the login shell (NSS), olo-complete starting, the first fine-tune —
//! is done on a worker, and the box says what it's doing meanwhile.

use std::{
    path::Path,
    sync::{
        Arc, OnceLock,
        mpsc::{Sender, channel},
    },
    time::{Duration, Instant},
};

use olo_complete::{
    protocol::{self, Reply, Request, Settings, Stats},
    train::Tuned,
};
use olo_sync::Mutex;
use olo_ui::{
    gpui::SharedString,
    style, theme,
    widgets::{Button, CheckBox, StringView},
};

use crate::{Msg, form::Ui};

/// The checkbox's words (the palette finds it by them, and Activity
/// Monitor opens here with it lit).
const LABEL: &str = protocol::OPTION;

enum Job {
    Look,
    Turn(bool),
}

/// The login shell, as far as suggestions go.
#[derive(Debug, Clone, Default, PartialEq)]
enum Shell {
    /// Not looked at yet.
    #[default]
    Unknown,
    /// Olo's brush: it asks olo-complete.
    Brush,
    /// Olo's brush from before it asked.
    OldBrush,
    /// Another shell (its path).
    Other(String),
}

#[derive(Default)]
struct Shared {
    shell: Shell,
    on: bool,
    /// The first fine-tune under way (turned on just now).
    learning: bool,
    /// olo-complete's own account, if it's running.
    stats: Option<Stats>,
    error: Option<String>,
}

struct Link {
    shared: Arc<Mutex<Shared>>,
    tx: Sender<Job>,
}

fn look_at_shell() -> Shell {
    let Some((_, shell)) = olo_system::accounts::login_shell() else { return Shell::Other("unknown".into()) };
    let brush = Path::new(olo_system::accounts::BRUSH);
    if shell != brush {
        return Shell::Other(shell.display().to_string());
    }
    if protocol::asks_for_hints(brush) { Shell::Brush } else { Shell::OldBrush }
}

fn ask_stats() -> Option<Stats> {
    match protocol::ask(&Request::Stats, Duration::from_millis(500)) {
        Some(Reply::Stats(s)) => Some(s),
        _ => None,
    }
}

/// Turned on: saved, olo-complete started (if it isn't), its first
/// fine-tune from the history asked for and waited for.
fn turn_on() -> Result<(), String> {
    Settings { enabled: true }.save().map_err(|e| format!("Couldn't save the setting: {e}"))?;
    if !protocol::running() {
        protocol::start();
        let since = Instant::now();
        while !protocol::running() {
            if since.elapsed() > Duration::from_secs(5) {
                return Err("olo-complete didn’t start: Deskbar’s Upgrade installs it.".into());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    match protocol::ask(&Request::Learn, Duration::from_secs(120)) {
        Some(Reply::Learned(_)) => Ok(()),
        Some(Reply::Error { message }) => Err(format!("The first fine-tune failed: {message}")),
        _ => Err("olo-complete didn't answer while learning.".into()),
    }
}

/// The worker, and what it last found (one for the app).
fn link(proxy: olo_ui::Proxy) -> &'static Link {
    static LINK: OnceLock<Link> = OnceLock::new();
    LINK.get_or_init(|| {
        let shared: Arc<Mutex<Shared>> = Default::default();
        // Tests (the layout lint): no worker; `LINT` says what to show.
        if cfg!(test) {
            return Link { shared, tx: channel().0 };
        }
        let (tx, rx) = channel::<Job>();
        let sh = shared.clone();
        std::thread::Builder::new()
            .name("complete".into())
            .spawn(move || {
                while let Ok(job) = rx.recv() {
                    match job {
                        Job::Look => {
                            let (shell, on) = (look_at_shell(), Settings::load().enabled);
                            let stats = if on { ask_stats() } else { None };
                            let mut s = sh.lock().unwrap();
                            (s.shell, s.on, s.stats) = (shell, on, stats);
                        }
                        Job::Turn(true) => {
                            {
                                let mut s = sh.lock().unwrap();
                                (s.on, s.learning, s.error) = (true, true, None);
                            }
                            proxy.send(Msg::Redraw);
                            let done = turn_on();
                            let stats = ask_stats();
                            let mut s = sh.lock().unwrap();
                            (s.learning, s.stats) = (false, stats);
                            if let Err(e) = done {
                                s.error = Some(e);
                            }
                        }
                        Job::Turn(false) => {
                            let saved = Settings { enabled: false }.save();
                            // (It reads the file again, finds itself off, goes.)
                            protocol::tell(&Request::Reload);
                            let mut s = sh.lock().unwrap();
                            (s.on, s.stats, s.error) = (false, None, saved.err().map(|e| format!("Couldn't save the setting: {e}")));
                        }
                    }
                    proxy.send(Msg::Redraw);
                }
            })
            .expect("thread");
        let _ = tx.send(Job::Look);
        Link { shared, tx }
    })
}

thread_local! {
    /// The layout lint's: which of `lint_states` the box shows.
    pub static LINT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The box's states for the layout lint, longest of each kind: in use
/// (made-up figures), off, learning, another shell, an old brush (one
/// that failed to start olo-complete, one in use).
#[cfg(test)]
pub fn lint_states() -> [&'static str; 6] {
    ["on", "off", "learning", "another shell", "an old brush, failing", "an old brush, on"]
}

fn lint_state(v: usize) -> (Shell, bool, bool, Option<Stats>, Option<String>) {
    match v {
        0 => (Shell::Brush, true, false, Some(protocol::demo_stats()), None),
        1 => (Shell::Brush, false, false, None, None),
        2 => (Shell::Brush, true, true, None, None),
        3 => (Shell::Other("/home/someone/.nix-profile/bin/zsh".into()), false, false, None, None),
        4 => (Shell::OldBrush, false, false, None, Some("olo-complete didn’t start: Deskbar’s Upgrade installs it.".into())),
        _ => (Shell::OldBrush, true, false, Some(protocol::demo_stats()), None),
    }
}

/// "98.8%", "100%".
fn percent(v: f32) -> String {
    let p = v * 100.0;
    if p >= 99.95 || p <= 0.05 { format!("{p:.0}%") } else { format!("{p:.1}%") }
}

/// "3 November at 23:24".
fn when(at: i64) -> String {
    jiff::Timestamp::from_second(at).ok().map(|t| t.to_zoned(jiff::tz::TimeZone::system()).strftime("%-d %B at %H:%M").to_string()).unwrap_or_default()
}

/// What the last fine-tune did, in words.
fn learned(t: &Tuned, at: i64) -> String {
    let when = when(at);
    let ms = t.micros as f64 / 1000.0;
    let took = if ms < 10.0 { format!("{ms:.1} ms") } else { format!("{ms:.0} ms") };
    let (n, now, was) = (style::thousands(t.examples), percent(t.after.top1), percent(t.before.top1));
    if !t.kept {
        format!("Learned {when} from {n} examples in {took}, but did no better on your newest commands, so it kept what it had: its first suggestion right {was} of the time.")
    } else if t.after.top1 > t.before.top1 {
        format!("Learned {when} from {n} examples in {took}: its first suggestion is right {now} of the time on your newest commands ({was} before).")
    } else {
        format!("Learned {when} from {n} examples in {took}: its first suggestion is right {now} of the time on your newest commands.")
    }
}

pub fn ui(ui: &mut Ui) {
    let link = link(olo_ui::proxy(ui.cx));
    let (shell, on, learning, stats, error) = if cfg!(test) {
        lint_state(LINT.with(|l| l.get()))
    } else {
        let s = link.shared.lock().unwrap();
        (s.shell.clone(), s.on, s.learning, s.stats.clone(), s.error.clone())
    };
    let usable = matches!(shell, Shell::Brush | Shell::OldBrush);
    ui.bbox("Command Suggestions", |ui| {
        let tx = link.tx.clone();
        let enabled = ui.enabled() && usable && !learning;
        ui.add(CheckBox::new(LABEL, on).enabled(enabled).on_toggle(move |v, _, _| {
            let _ = tx.send(Job::Turn(v));
        }));
        let mut text = match (&shell, on) {
            (Shell::Unknown, _) => String::new(),
            (Shell::Other(path), _) => format!("brush’s grey hint after what you type, from a tiny model of your own commands on this computer. Your shell is {path}: Olo Suggestions makes it brush."),
            (_, false) => "brush’s grey hint after what you type, from a tiny model of your own commands on this computer: it weighs what’s typed, the folder, the commands before, the hour and the day, and learns as you go.".into(),
            _ if learning => "Learning from your history…".into(),
            _ => match &stats {
                Some(st) => match &st.last_tune {
                    Some(t) => format!("{} About a microsecond each, on the {}.", learned(t, st.last_tune_at), st.backend),
                    None => format!("In use, with the weights it began from (not fine-tuned yet), on the {}.", st.backend),
                },
                None => "On: brush starts olo-complete as you next type in it.".into(),
            },
        };
        if shell == Shell::OldBrush {
            text.push_str(" Your brush is from before the model: Olo Suggestions updates it.");
        }
        if !text.is_empty() {
            ui.note(text);
        }
        if let Some(e) = error {
            let p = theme::palette(ui.cx);
            ui.add(StringView::new(SharedString::from(e)).color(style::c(p.failure)).wrap(true));
        }
        if matches!(shell, Shell::Other(_) | Shell::OldBrush) {
            ui.add(Button::new("Open Olo Suggestions…").enabled(ui.enabled()).on_click(|_, _| {
                let _ = olo_xdg::open::spawn_app(&["olo-suggestions".into()], "olo.suggestions");
            }));
        }
    });
}
