//! Phone: an iPhone's messages and one-time codes on this computer,
//! through BlueFerry's backend. Setting it up, a step at a time, each
//! done from here (`setup`: the backend installed, through Olo Doctor;
//! the phone paired, in the backend's own wizard in Terminal; the phone's
//! switches); how the phone stands (the phone service's state file,
//! followed by inotify, read off the UI thread); and Olo's settings for it
//! (`phone.json`, which the service follows): what is told of, and where
//! one-time codes go. What is asked of the backend (trying the phone
//! again, starting it) is asked on a worker thread: the window never
//! waits.

use std::sync::{
    Arc, OnceLock,
    mpsc::{Sender, channel},
};

use olo_kit::{
    ParentElement as _, SharedString, Styled as _, WindowCx, div, metrics as m, px,
    widgets::{BBox, Button, CheckBox, EmptyState, MenuField, RadioButton, StatusBar, StringView},
};
use olo_protocol::phone::{AgentCodes, Backend, Link as PhoneLink, Notify, PhoneSettings, PhoneState};
use olo_sync::Mutex;

use crate::{Msg, form::Ui};

enum Job {
    /// Install the backend: Olo Doctor's fix (it asks for the password).
    Install,
    Save(PhoneSettings),
    /// Try the phone again now.
    Reconnect,
    /// Start the backend (the bus starts it when it's asked for).
    Start,
}

#[derive(Default)]
struct Shared {
    state: PhoneState,
    settings: PhoneSettings,
    /// The state file was read: something to show.
    known: bool,
    error: Option<String>,
    /// The backend is being installed.
    installing: bool,
}

struct Link {
    shared: Arc<Mutex<Shared>>,
    tx: Sender<Job>,
}

/// The phone service's state, followed, and the worker that asks the
/// backend (one for the app).
fn link(proxy: olo_kit::Proxy) -> &'static Link {
    static LINK: OnceLock<Link> = OnceLock::new();
    LINK.get_or_init(|| {
        let shared: Arc<Mutex<Shared>> = Default::default();
        // Tests (the layout lint): a phone connected, and no service.
        if cfg!(test) {
            let mut s = shared.lock().unwrap();
            (s.state, s.known) = (test_state(), true);
            drop(s);
            return Link { shared, tx: channel().0 };
        }
        let read = {
            let (shared, proxy) = (shared.clone(), proxy.clone());
            move |_: &[std::path::PathBuf]| {
                let (state, settings) = (PhoneState::load(), PhoneSettings::load());
                let mut s = shared.lock().unwrap();
                if s.known && s.state == state && s.settings == settings {
                    return;
                }
                (s.state, s.settings, s.known) = (state, settings, true);
                drop(s);
                proxy.send(Msg::Redraw);
            }
        };
        let paths = [PhoneState::path(), PhoneSettings::path()];
        let first = read.clone();
        std::thread::spawn(move || first(&[]));
        if let Some(w) = olo_kit::watch::watch(paths, read) {
            // The app's, for as long as it runs.
            std::mem::forget(w);
        }
        let (tx, rx) = channel::<Job>();
        let sh = shared.clone();
        std::thread::Builder::new()
            .name("phone".into())
            .spawn(move || {
                use olo_system::blueferry::{self, Client};
                while let Ok(job) = rx.recv() {
                    if matches!(job, Job::Install) {
                        sh.lock().unwrap().installing = true;
                        proxy.send(Msg::Redraw);
                    }
                    let error = match job {
                        // (The phone service sees it come and starts it;
                        // asked for here too, so the bus knows of it.)
                        Job::Install => crate::ask_doctor(&["--fix", "phone-backend"]).err().or_else(|| {
                            let _ = olo_system::env::reload_session_bus();
                            Client::connect().and_then(|c| c.status()).err().map(|e| blueferry::said(&e))
                        }),
                        Job::Save(s) => s.save().err().map(|e| format!("Couldn't save: {e}")),
                        Job::Reconnect => Client::connect().and_then(|c| c.reconnect()).err().map(|e| blueferry::said(&e)),
                        // (Its status asked: the bus starts it to answer.)
                        Job::Start => Client::connect().and_then(|c| c.status()).err().map(|e| blueferry::said(&e)),
                    };
                    let mut s = sh.lock().unwrap();
                    let was = std::mem::take(&mut s.installing);
                    if s.error != error || was {
                        s.error = error;
                        drop(s);
                        proxy.send(Msg::Redraw);
                    }
                }
            })
            .expect("thread");
        Link { shared, tx }
    })
}

/// Test data: a phone connected, with messages unread.
pub fn test_state() -> PhoneState {
    PhoneState {
        pid: 0,
        backend: Backend::Running,
        release: "0.9.1-1.fc45".into(),
        link: PhoneLink::Ready,
        messages: true,
        contacts: true,
        notifications: true,
        battery: Some(81),
        conversations: 38,
        unread: 2,
        storage: "ready".into(),
        codes: 0,
        code_at: None,
    }
}

pub struct State {
    link: &'static Link,
}

impl State {
    pub fn new() -> State {
        State { link: link(olo_kit::proxy()) }
    }
}

fn spawn(cmd: &str, app: &str) {
    if let Err(e) = olo_xdg::spawn_detached_app(cmd, app) {
        tracing::warn!("preferences: {cmd}: {e}");
    }
}

/// The backend's own pairing wizard, in Terminal: it checks the
/// Bluetooth controller, finds the phone, pairs with it (the same code
/// on both) and says which switches to turn on on the phone.
fn pair() {
    spawn("olo-terminal -e blueferry pair-setup", "olo.terminal");
}

/// A step of setting up: its number (a tick once done), what it is and
/// what it does, and its button while it's the one to do.
fn step(ui: &mut Ui, n: u32, done: bool, now: bool, title: &str, detail: &str, button: Option<Button>) {
    let mark = if done { "✓".to_owned() } else { format!("{n}.") };
    let words = div().flex_1().min_w_0().flex().flex_col().gap(px(m::STACK)).child(StringView::new(title.to_owned()).bold(now)).child(StringView::new(detail.to_owned()).dim(true).wrap(true));
    let mut row = div().flex().flex_row().items_start().gap(px(m::SPACING)).child(div().w(px(20.0)).flex_shrink_0().child(StringView::new(mark).bold(now).dim(!now && !done))).child(words);
    if let Some(b) = button {
        row = row.child(div().flex_shrink_0().child(b));
    }
    ui.add(row);
}

/// Setting the phone up, a step at a time, each done from here: the
/// backend, pairing, and the phone's own switches.
fn setup(ui: &mut Ui, link: &'static Link, state: &PhoneState, installing: bool, error: Option<String>) {
    let running = state.backend == Backend::Running;
    let paired = running && state.link != PhoneLink::NotPaired;
    ui.add(StringView::new("Your iPhone’s messages and its one-time codes can come to this computer, over Bluetooth, with no cloud or account between them.").wrap(true));
    let items = ui.collect(|ui| {
        // 1. The backend.
        let (detail, button) = match state.backend {
            Backend::Missing if installing => ("Installing… (Olo Doctor asks for your password; Bluetooth starts again once.)".to_owned(), Some(Button::new("Installing…").enabled(false))),
            Backend::Missing => (
                "BlueFerry talks to the phone. It’s installed from its makers’ latest release, checked against what they published.".to_owned(),
                Some(Button::new("Install…").default(true).on_click(move |_, _| {
                    let _ = link.tx.send(Job::Install);
                })),
            ),
            Backend::Stopped => (
                "BlueFerry is installed and not running.".to_owned(),
                Some(Button::new("Start It").default(true).on_click(move |_, _| {
                    let _ = link.tx.send(Job::Start);
                })),
            ),
            Backend::Other => (
                format!("BlueFerry {} and this Olo speak different versions: update Olo, or install BlueFerry’s latest.", state.release),
                Some(Button::new("Install Latest…").enabled(!installing).on_click(move |_, _| {
                    let _ = link.tx.send(Job::Install);
                })),
            ),
            Backend::Running => (format!("BlueFerry {} is installed and running.", state.release), None),
        };
        step(ui, 1, running, !running, "Install BlueFerry", &detail, button);
        // 2. Pairing.
        let first = (running && !paired).then(|| Button::new("Pair…").default(true).on_click(move |_, _| pair()));
        let again = paired.then(|| Button::new("Pair Again…").on_click(move |_, _| pair()));
        step(
            ui,
            2,
            paired,
            running && !paired,
            "Pair the iPhone",
            "Unlock it and keep Settings ▸ Bluetooth open on it. Pairing opens in Terminal: it finds the phone and shows the same code on both.",
            first.or(again),
        );
        // 3. The phone's own switches.
        step(
            ui,
            3,
            false,
            paired,
            "On the iPhone, let it share",
            "In Settings ▸ Bluetooth, tap ⓘ beside this computer and turn on Show Message Notifications and Sync Contacts (and Share System Notifications, for group names). They can take a minute to appear.",
            None,
        );
    });
    ui.add(BBox::new("Set Up Your iPhone").children(items));
    if let Some(e) = error {
        ui.label(e);
    }
}

pub fn ui(ui: &mut Ui, st: &mut State) {
    let link = st.link;
    let (state, settings, known, error) = {
        let s = link.shared.lock().unwrap();
        (s.state.clone(), s.settings.clone(), s.known, s.error.clone())
    };
    if !known {
        return ui.add(EmptyState::new("Looking for the phone…").icon("phone"));
    }
    // Not all set up yet: the steps, and nothing else to look at.
    if state.backend != Backend::Running || !state.link.connected() && !state.messages && matches!(state.link, PhoneLink::NotPaired | PhoneLink::Allow) {
        let installing = link.shared.lock().unwrap().installing;
        return setup(ui, link, &state, installing, error);
    }

    // How it stands.
    let connected = state.link.connected();
    let status = ui.collect(|ui| {
        let what = match state.link {
            PhoneLink::Allow => "Waiting for the phone to allow it (Settings ▸ Bluetooth ▸ ⓘ beside this computer).".to_owned(),
            PhoneLink::Refused => "The phone refused the messages’ connection. Pair it again.".to_owned(),
            PhoneLink::Reconnecting => "The phone is out of reach or its Bluetooth is off. It’s tried again by itself.".to_owned(),
            other => other.words().to_owned(),
        };
        ui.add(StringView::new(what));
        if let Some(b) = state.battery.filter(|_| connected) {
            ui.add(StatusBar::new(f32::from(b) / 100.0).label("Battery").trailing(format!("{b}%")));
        }
        if connected {
            let up = |on: bool, what: &str| format!("{what}{}", if on { "" } else { " (off on the phone)" });
            ui.add(StringView::new(format!("{} · {} · {}", up(state.messages, "Messages"), up(state.contacts, "Contacts"), up(state.notifications, "Notifications"))).dim(true));
            let unread = match state.unread {
                0 => String::new(),
                n => format!(", {n} unread"),
            };
            ui.add(StringView::new(format!("{} conversations{unread}", state.conversations)).dim(true));
        }
        if !matches!(state.storage.as_str(), "ready" | "") {
            ui.note(format!("Its store of messages is {}: new messages still come, older ones aren’t kept.", state.storage));
        }
        let mut buttons = div().flex().flex_row().flex_wrap().gap(px(m::SPACING));
        if !connected {
            buttons = buttons.child(Button::new("Try Again").on_click(move |_, _| {
                let _ = link.tx.send(Job::Reconnect);
            }));
        }
        buttons = buttons.child(Button::new("Pair Again…").on_click(move |_, _| pair()));
        ui.add(buttons);
    });
    ui.add(BBox::new("iPhone").children(status));

    let save = move |change: fn(&mut PhoneSettings, bool)| {
        move |on: &bool, _: &mut WindowCx| {
            let mut s = link.shared.lock().unwrap().settings.clone();
            change(&mut s, *on);
            link.shared.lock().unwrap().settings = s.clone();
            let _ = link.tx.send(Job::Save(s));
        }
    };

    // (Two columns: what's told of; where codes go.)
    let left = ui.collect(|ui| {
        let items = ui.collect(|ui| {
            ui.label("Tell me of:");
            for choice in Notify::ALL {
                let detail = match choice {
                    Notify::Messages => "Messages: each as it comes, answered in place",
                    Notify::Everything => "Everything the phone notifies of, as it shows it",
                    Notify::Nothing => "Nothing",
                };
                ui.add(RadioButton::new(detail, settings.notify == choice).on_select(move |_, _| {
                    let mut s = link.shared.lock().unwrap().settings.clone();
                    s.notify = choice;
                    link.shared.lock().unwrap().settings = s.clone();
                    let _ = link.tx.send(Job::Save(s));
                }));
            }
            ui.add(CheckBox::new("Show what a message says", settings.show_text).enabled(settings.notify == Notify::Messages).on_toggle(save(|s, on| s.show_text = on)));
        });
        ui.add(BBox::new("Notifications").children(items));
    });
    let right = ui.collect(|ui| {
        let items = ui.collect(|ui| {
            ui.add(CheckBox::new("Find the one-time codes in messages", settings.codes).on_toggle(save(|s, on| s.codes = on)));
            ui.enabled_if(settings.codes, |ui| {
                ui.add(CheckBox::new("Offer a code in the browser tab waiting for it", settings.codes_in_browser).enabled(ui.enabled()).on_toggle(save(|s, on| s.codes_in_browser = on)));
                let names: Vec<SharedString> = AgentCodes::ALL.iter().map(|a| SharedString::from(a.label())).collect();
                let at = AgentCodes::ALL.iter().position(|a| *a == settings.agent_screens).unwrap_or(0);
                ui.add(MenuField::new("phone-agent-codes", names, at).label("In an agent’s tab:").enabled(ui.enabled() && settings.codes_in_browser).on_change(move |i, _| {
                    let mut s = link.shared.lock().unwrap().settings.clone();
                    s.agent_screens = AgentCodes::ALL[*i];
                    link.shared.lock().unwrap().settings = s.clone();
                    let _ = link.tx.send(Job::Save(s));
                }));
                ui.note(match settings.agent_screens {
                    AgentCodes::Give => "A code that is surely an agent’s tab’s is put in there, and you’re told; one that isn’t sure is asked about.",
                    AgentCodes::Ask => "You’re asked each time a tab on an Agent Screen is the one waiting.",
                    AgentCodes::Never => "Tabs on Agent Screens are never given a code.",
                });
                ui.add(CheckBox::new("Copy a code no tab is waiting for", settings.copy_codes).enabled(ui.enabled()).on_toggle(save(|s, on| s.copy_codes = on)));
            });
        });
        ui.add(BBox::new("One-Time Codes").children(items));
    });
    ui.add_columns(left, right);

    ui.add(StringView::new(format!("BlueFerry {}", state.release)).dim(true));
    if let Some(e) = error {
        ui.label(e);
    }
}
