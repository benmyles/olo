//! Phone: an iPhone's messages and one-time codes on this computer,
//! through BlueFerry's backend. How the phone stands (the phone service's
//! state file, followed by inotify, read off the UI thread), pairing it
//! (the backend's own wizard, in Terminal), and Olo's settings for it
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
                    let error = match job {
                        Job::Save(s) => s.save().err().map(|e| format!("Couldn't save: {e}")),
                        Job::Reconnect => Client::connect().and_then(|c| c.reconnect()).err().map(|e| blueferry::said(&e)),
                        // (Its status asked: the bus starts it to answer.)
                        Job::Start => Client::connect().and_then(|c| c.status()).err().map(|e| blueferry::said(&e)),
                    };
                    let mut s = sh.lock().unwrap();
                    if s.error != error {
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

pub fn ui(ui: &mut Ui, st: &mut State) {
    let link = st.link;
    let (state, settings, known, error) = {
        let s = link.shared.lock().unwrap();
        (s.state.clone(), s.settings.clone(), s.known, s.error.clone())
    };
    if !known {
        return ui.add(EmptyState::new("Looking for the phone…").icon("phone"));
    }
    match state.backend {
        Backend::Missing => {
            return ui.add(
                EmptyState::new("Your iPhone’s messages and codes can come to this computer.")
                    .icon("phone")
                    .detail("They come over Bluetooth through BlueFerry, with no cloud or account between. Olo Doctor installs it; then pair the phone here.")
                    .action("Olo Doctor…", move |_, _cx| spawn("olo-doctor", "olo.doctor")),
            );
        }
        Backend::Stopped => {
            return ui.add(EmptyState::new("The phone’s backend isn’t running.").icon("phone").detail("BlueFerry starts with your session; it has stopped.").action("Start It", move |_, _cx| {
                let _ = link.tx.send(Job::Start);
            }));
        }
        Backend::Other => {
            return ui.add(
                EmptyState::new("BlueFerry and Olo don’t speak the same version.")
                    .icon("phone")
                    .detail(format!("BlueFerry {} is installed. Update Olo, or BlueFerry (Olo Doctor installs its latest release), so they match.", state.release))
                    .action("Olo Doctor…", move |_, _cx| spawn("olo-doctor", "olo.doctor")),
            );
        }
        Backend::Running => {}
    }
    if state.link == PhoneLink::NotPaired {
        return ui.add(
            EmptyState::new("No iPhone is paired for messages.")
                .icon("phone")
                .detail("Unlock the iPhone and keep Settings ▸ Bluetooth open on it. Pairing opens in Terminal: it finds the phone, shows the same code on both, and says which switches to turn on on the phone.")
                .action("Pair an iPhone…", move |_, _cx| pair()),
        );
    }

    // How it stands.
    let connected = state.link.connected();
    let status = ui.collect(|ui| {
        let what = match state.link {
            PhoneLink::Allow => "Waiting for the phone: on it, in Settings ▸ Bluetooth, tap ⓘ beside this computer and turn on Show Message Notifications and Sync Contacts.".to_owned(),
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
