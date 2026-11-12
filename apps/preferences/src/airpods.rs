//! AirPods: their levels, noise control, press and hold, listening
//! features, microphone and name, and what this computer does with them
//! (ear detection, turning the sound down while you speak, connecting
//! and switching by itself). The AirPods service (`olo-airpods
//! --service`) does the talking: this panel follows what it publishes
//! (its state file, by inotify, read off the UI thread) and asks it for
//! changes on a worker thread, so the window never waits. Olo's own
//! settings for them are `airpods.json`, written on the worker too.

use std::{
    sync::{
        Arc, OnceLock,
        mpsc::{Sender, channel},
    },
    time::Duration,
};

use olo_kit::{
    Child, Cx, ParentElement as _, SharedString, Styled as _, WindowCx, div, metrics as m, px,
    widgets::{BBox, Button, CheckBox, EmptyState, MenuField, RadioButton, Slider, StatusBar, StringView, TextControl, TextField, TextFieldEvent},
};
use olo_protocol::airpods::{self as ap, AirPods, AirPodsSettings, AirPodsState, BudMicrophone, Change, Charge, ListeningMode, MicrophoneChoice, Request};
use olo_sync::Mutex;

use crate::{Msg, PanelView, form::Ui, panel};

enum Job {
    Ask(Request),
    Save(AirPodsSettings),
}

#[derive(Default)]
struct Shared {
    state: AirPodsState,
    settings: AirPodsSettings,
    /// The service answered (or its file was read): something to show.
    known: bool,
    error: Option<String>,
}

struct Link {
    shared: Arc<Mutex<Shared>>,
    tx: Sender<Job>,
}

/// The service's state, followed, and the worker that asks it (one for
/// the app).
fn link(proxy: olo_kit::Proxy) -> &'static Link {
    static LINK: OnceLock<Link> = OnceLock::new();
    LINK.get_or_init(|| {
        let shared: Arc<Mutex<Shared>> = Default::default();
        // Tests (the layout lint): a pair named as long as real ones get,
        // every setting known, and no service.
        if cfg!(test) {
            let mut s = shared.lock().unwrap();
            (s.state, s.known) = (test_state(), true);
            drop(s);
            return Link { shared, tx: channel().0 };
        }
        let read = {
            let (shared, proxy) = (shared.clone(), proxy.clone());
            move |_: &[std::path::PathBuf]| {
                let (state, settings) = (AirPodsState::load(), AirPodsSettings::load());
                let mut s = shared.lock().unwrap();
                if s.known && s.state == state && s.settings == settings {
                    return;
                }
                (s.state, s.settings, s.known) = (state, settings, true);
                drop(s);
                proxy.send(Msg::Redraw);
            }
        };
        let paths = [AirPodsState::path(), AirPodsSettings::path()];
        let first = read.clone();
        std::thread::spawn(move || first(&[]));
        if let Some(w) = olo_kit::watch::watch(paths, read) {
            // The app's, for as long as it runs.
            std::mem::forget(w);
        }
        let (tx, rx) = channel::<Job>();
        let sh = shared.clone();
        std::thread::Builder::new()
            .name("airpods".into())
            .spawn(move || {
                while let Ok(job) = rx.recv() {
                    let error = match job {
                        Job::Ask(r) => match ap::ask(&r, Duration::from_secs(20)) {
                            Ok(reply) if reply.ok => None,
                            Ok(reply) => reply.error,
                            Err(_) => Some("The AirPods service isn't running.".to_owned()),
                        },
                        Job::Save(s) => s.save().err().map(|e| format!("Couldn't save: {e}")),
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

/// Test data: AirPods Pro with a long name, connected, the case charging,
/// as the Sound and Bluetooth panels' test devices name them.
pub fn test_state() -> AirPodsState {
    use olo_protocol::airpods::{Abilities, BudSettings, Sound};
    let c = |percent, charging| Some(Charge { percent, charging });
    AirPodsState {
        pid: 0,
        listening: true,
        not_listening: None,
        devices: vec![AirPods {
            address: "02:00:00:00:A1:01".into(),
            path: "/org/bluez/hci0/dev_02_00_00_00_A1_01".into(),
            name: "Travel AirPods Pro (2nd generation) for the Office".into(),
            model: "AirPods Pro (2nd generation, USB-C)".into(),
            product: 0x2024,
            abilities: Abilities { noise_control: true, adaptive: true, buds: true },
            connected: true,
            linked: true,
            left: c(100, false),
            right: c(9, false),
            case: c(100, true),
            in_ear: 2,
            settings: BudSettings {
                listening_mode: Some(ListeningMode::NoiseCancellation),
                press_and_hold_modes: vec![ListeningMode::Transparency, ListeningMode::Adaptive, ListeningMode::NoiseCancellation],
                conversation_awareness: Some(true),
                personalized_volume: Some(true),
                adaptive_strength: Some(50),
                ear_detection: Some(true),
                one_bud_noise_cancellation: Some(false),
                volume_swipe: Some(true),
                case_sounds: Some(true),
                microphone: Some(BudMicrophone::Automatic),
                press_speed: Some(2),
                hold_duration: Some(2),
            },
            sound: Sound { output: "airpods".into(), input: String::new(), microphone: String::new(), codec: "aac".into(), high_quality: true, card: "bluez_card.02_00_00_00_A1_01".into() },
            ..Default::default()
        }],
    }
}

pub struct State {
    link: &'static Link,
    /// The pair shown (several may be paired).
    chosen: Option<String>,
    name: Child<TextField>,
}

impl State {
    pub fn new(cx: &mut Cx<PanelView>) -> State {
        let link = link(olo_kit::proxy());
        let name = crate::form::text_field(cx, |f| f);
        cx.subscribe(&name, move |v, e: &TextFieldEvent, cx| {
            if matches!(e, TextFieldEvent::Entered) {
                let (text, address) = match &v.state {
                    panel::State::AirPods(st) => (st.name.read(cx).text().trim().to_owned(), st.chosen.clone()),
                    _ => return,
                };
                if !text.is_empty() {
                    let _ = link.tx.send(Job::Ask(Request::Set { address, change: Change::Name(text) }));
                }
            }
        });
        State { link, chosen: None, name }
    }
}

fn state_of(v: &mut PanelView) -> Option<&mut State> {
    match &mut v.state {
        panel::State::AirPods(st) => Some(st),
        _ => None,
    }
}

/// "82%", with "charging" when it is.
fn percent(c: &Charge) -> String {
    if c.charging { format!("{}%, charging", c.percent) } else { format!("{}%", c.percent) }
}

/// The levels: a picture of each bud and the case with its bar under it
/// (or one for over-ear headphones), as an iPhone shows them.
fn levels(ui: &mut Ui, a: &AirPods) {
    let parts: Vec<(&'static str, &'static str, Option<Charge>)> =
        if a.abilities.buds { vec![("Left", "airpod-left", a.left), ("Right", "airpod-right", a.right), ("Case", "airpods-case", a.case)] } else { vec![("Battery", "airpods", a.whole)] };
    let columns: Vec<_> = parts
        .into_iter()
        .map(|(name, icon, c)| {
            let picture = olo_kit::canvas(move |g, b: olo_kit::Bounds<olo_kit::Pixels>| {
                let at = olo_kit::style::snap_centred(g, b.center(), 48.0, 48.0);
                olo_kit::icons::paint(g, at, icon);
            })
            .w_full()
            .h(px(48.0));
            let bar = match c {
                Some(c) => StatusBar::new(c.percent as f32 / 100.0).label(name).trailing(percent(&c)),
                None => StatusBar::new(0.0).label(name).trailing(if name == "Case" { "Closed" } else { "—" }),
            };
            div().flex_1().min_w_0().flex().flex_col().gap(px(m::SPACING)).child(picture).child(bar)
        })
        .collect();
    ui.add(div().flex().flex_row().gap(px(m::SPACING * 3.0)).children(columns));
}

pub fn ui(ui: &mut Ui, st: &mut State) {
    let link = st.link;
    let (state, settings, known, error) = {
        let s = link.shared.lock().unwrap();
        (s.state.clone(), s.settings.clone(), s.known, s.error.clone())
    };
    if !known {
        return ui.add(EmptyState::new("Looking for AirPods…").icon("airpods"));
    }
    if state.devices.is_empty() {
        return ui.add(
            EmptyState::new("No AirPods are paired with this computer.")
                .icon("airpods")
                .detail("Open the case near this computer, press and hold its button until the light flashes white, then pair them in Bluetooth.")
                .action("Bluetooth…", move |_, _cx| {
                    crate::prefs().open("bluetooth");
                }),
        );
    }
    let a = st.chosen.as_ref().and_then(|c| state.by_address(c)).or_else(|| state.connected()).unwrap_or(&state.devices[0]).clone();
    st.chosen = Some(a.address.clone());
    let address = Some(a.address.clone());
    let ask = {
        let address = address.clone();
        move |change: Change| {
            let _ = link.tx.send(Job::Ask(Request::Set { address: address.clone(), change }));
        }
    };

    header(ui, st, &state, &a);
    if a.connected {
        levels(ui, &a);
    }
    let live = a.connected && a.linked;
    let s = a.settings.clone();

    // (Two columns: what they do when pressed and how they listen; how
    // they sound and what this computer does with them.)
    let left = ui.collect(|ui| {
        if a.abilities.noise_control {
            ui.enabled_if(live, |ui| {
                let items = ui.collect(|ui| {
                    let modes: Vec<ListeningMode> = ListeningMode::ALL.into_iter().filter(|m| *m != ListeningMode::Adaptive || a.abilities.adaptive).collect();
                    let radios: Vec<_> = modes
                        .iter()
                        .map(|mode| {
                            let (ask, mode) = (ask.clone(), *mode);
                            RadioButton::new(mode.label(), s.listening_mode == Some(mode)).enabled(ui.enabled()).on_select(move |_, _| ask(Change::ListeningMode(mode)))
                        })
                        .collect();
                    ui.add(div().flex().flex_row().flex_wrap().gap_x(px(m::SPACING * 3.0)).gap_y(px(m::STACK)).children(radios));
                    if a.abilities.adaptive {
                        let strength = s.adaptive_strength.unwrap_or(50) as f32;
                        let ask = ask.clone();
                        ui.add(
                            Slider::new("adaptive-strength", strength, 0.0..=100.0)
                                .step(1.0)
                                .label("Adaptive:")
                                .limits("More noise", "Less noise")
                                .enabled(ui.enabled() && s.listening_mode == Some(ListeningMode::Adaptive))
                                .on_change(move |v, _| ask(Change::AdaptiveStrength(v.round() as u8))),
                        );
                    }
                });
                ui.add(BBox::new("Noise Control").children(items));
            });

            ui.enabled_if(live, |ui| {
                let items = ui.collect(|ui| {
                    ui.label("Press and hold a stem to switch between:");
                    let chosen = s.press_and_hold_modes.clone();
                    let checks: Vec<_> = ListeningMode::ALL
                        .into_iter()
                        .filter(|m| *m != ListeningMode::Adaptive || a.abilities.adaptive)
                        .map(|mode| {
                            let (ask, chosen) = (ask.clone(), chosen.clone());
                            let on = chosen.contains(&mode);
                            // At least two are always chosen.
                            let last_two = on && chosen.len() <= 2;
                            CheckBox::new(mode.label(), on).enabled(ui.enabled() && !last_two).on_toggle(move |now, _| {
                                let mut modes: Vec<ListeningMode> = chosen.iter().copied().filter(|m| *m != mode).collect();
                                if *now {
                                    modes.push(mode);
                                }
                                ask(Change::PressAndHoldModes(modes));
                            })
                        })
                        .collect();
                    ui.add(div().flex().flex_row().flex_wrap().gap_x(px(m::SPACING * 3.0)).gap_y(px(m::STACK)).children(checks));
                    let speeds: Vec<SharedString> = ["Default", "Slower", "Slowest"].into_iter().map(Into::into).collect();
                    let durations: Vec<SharedString> = ["Default", "Shorter", "Shortest"].into_iter().map(Into::into).collect();
                    let ask2 = ask.clone();
                    let speed = MenuField::new("press-speed", speeds, s.press_speed.unwrap_or(0).min(2) as usize)
                        .label("Press speed:")
                        .enabled(ui.enabled())
                        .on_change(move |i, _| ask2(Change::PressSpeed(*i as u8)));
                    let ask3 = ask.clone();
                    let hold = MenuField::new("hold-duration", durations, s.hold_duration.unwrap_or(0).min(2) as usize)
                        .label("Hold duration:")
                        .enabled(ui.enabled())
                        .on_change(move |i, _| ask3(Change::HoldDuration(*i as u8)));
                    ui.add(div().flex().flex_row().flex_wrap().items_center().gap_x(px(m::SPACING * 4.0)).gap_y(px(m::STACK)).child(speed).child(hold));
                });
                ui.add(BBox::new("Press and Hold").children(items));
            });
        }
    });
    let right = ui.collect(|ui| {
        ui.enabled_if(live, |ui| {
            let items = ui.collect(|ui| {
                let check = |ui: &mut Ui, label: &str, on: Option<bool>, make: fn(bool) -> Change| {
                    let ask = ask.clone();
                    ui.add(CheckBox::new(label.to_owned(), on.unwrap_or(false)).enabled(ui.enabled() && on.is_some()).on_toggle(move |v, _| ask(make(*v))));
                };
                if a.abilities.adaptive {
                    check(ui, "Conversation Awareness: lower the sound when you speak", s.conversation_awareness, Change::ConversationAwareness);
                    check(ui, "Personalized Volume: adjust the sound to your surroundings", s.personalized_volume, Change::PersonalizedVolume);
                }
                if a.abilities.noise_control && a.abilities.buds {
                    check(ui, "Noise Cancellation with one AirPod in", s.one_bud_noise_cancellation, Change::OneBudNoiseCancellation);
                }
                if a.abilities.adaptive {
                    check(ui, "Volume Swipe: slide on a stem to change the volume", s.volume_swipe, Change::VolumeSwipe);
                    check(ui, "Charging case sounds", s.case_sounds, Change::CaseSounds);
                }
                let mics: Vec<SharedString> = ["Either AirPod", "Always Left", "Always Right"].into_iter().map(Into::into).collect();
                let at = match s.microphone.unwrap_or_default() {
                    BudMicrophone::Automatic => 0,
                    BudMicrophone::Left => 1,
                    BudMicrophone::Right => 2,
                };
                if a.abilities.buds {
                    let ask = ask.clone();
                    ui.add(
                        MenuField::new("bud-microphone", mics, at)
                            .label("Microphone:")
                            .enabled(ui.enabled())
                            .on_change(move |i, _| ask(Change::Microphone([BudMicrophone::Automatic, BudMicrophone::Left, BudMicrophone::Right][*i]))),
                    );
                }
            });
            ui.add(BBox::new("Listening").children(items));
        });

        this_computer(ui, link, &a, &settings);
    });
    ui.add_columns(left, right);

    if let Some(e) = error {
        ui.label(e);
    }
}

/// The name (renamed on Return), what they are and whether they're
/// connected, with Connect or Disconnect; a menu when several are paired.
fn header(ui: &mut Ui, st: &mut State, state: &AirPodsState, a: &AirPods) {
    let link = st.link;
    if state.devices.len() > 1 {
        let names: Vec<SharedString> = state.devices.iter().map(|d| SharedString::from(d.name.clone())).collect();
        let at = state.devices.iter().position(|d| d.address == a.address).unwrap_or(0);
        let addrs: Vec<String> = state.devices.iter().map(|d| d.address.clone()).collect();
        let pick = ui.on_view(move |v, i: &usize, cx| {
            if let Some(st) = state_of(v) {
                st.chosen = addrs.get(*i).cloned();
            }
            cx.notify();
        });
        ui.add(MenuField::new("airpods-pair", names, at).label("AirPods:").on_change(pick));
    }
    let field = &st.name;
    let focused = field.is_focused(ui.cx);
    if !focused && field.read(ui.cx).text() != a.name {
        let name = a.name.clone();
        field.update(ui.cx, |f, cx| f.set_text(name, cx));
    }
    let usable = a.connected && a.linked;
    field.update(ui.cx, |f, cx| f.set_enabled(usable, cx));
    let status = match (a.connected, a.linked) {
        (true, true) => "Connected".to_owned(),
        (true, false) => "Connecting…".to_owned(),
        _ => "Not connected".to_owned(),
    };
    let address = Some(a.address.clone());
    let button = if a.connected {
        Button::new("Disconnect").on_click(move |_, _| {
            let _ = link.tx.send(Job::Ask(Request::Disconnect { address: address.clone() }));
        })
    } else {
        Button::new("Connect").default(true).on_click(move |_, _| {
            let _ = link.tx.send(Job::Ask(Request::Connect { address: address.clone() }));
        })
    };
    ui.add(div().flex().flex_row().items_center().gap(px(m::SPACING)).child(div().flex_1().child(TextControl::new(ui.cx, field).label("Name:"))).child(button));
    ui.add(StringView::new(format!("{} · {status}", a.model)).dim(true));
}

/// What this computer does with them: Olo's settings.
fn this_computer(ui: &mut Ui, link: &'static Link, a: &AirPods, settings: &AirPodsSettings) {
    let save = move |change: fn(&mut AirPodsSettings, bool)| {
        move |on: &bool, _: &mut WindowCx| {
            let mut s = link.shared.lock().unwrap().settings.clone();
            change(&mut s, *on);
            link.shared.lock().unwrap().settings = s.clone();
            let _ = link.tx.send(Job::Save(s));
        }
    };
    let items = ui.collect(|ui| {
        let address = Some(a.address.clone());
        let ear = a.settings.ear_detection;
        ui.add(CheckBox::new("Automatic Ear Detection: pause when you take an AirPod out", settings.pause_when_removed && ear != Some(false)).on_toggle(move |on: &bool, cx| {
            let on = *on;
            save(|s, on| s.pause_when_removed = on)(&on, cx);
            // The AirPods' own (they stop sending sound to a bud out).
            let _ = link.tx.send(Job::Ask(Request::Set { address: address.clone(), change: Change::EarDetection(on) }));
        }));
        if a.abilities.adaptive {
            ui.add(
                CheckBox::new("Lower this computer's sound while you speak", settings.lower_when_speaking)
                    .enabled(a.settings.conversation_awareness != Some(false))
                    .on_toggle(save(|s, on| s.lower_when_speaking = on)),
            );
        }
        let listening = link.shared.lock().unwrap().state.listening;
        ui.add(CheckBox::new("Connect automatically when you put them in near this computer", settings.connect_automatically).enabled(listening).on_toggle(save(|s, on| s.connect_automatically = on)));
        ui.add(CheckBox::new("Switch automatically between this computer and your iPhone", settings.switch_automatically).enabled(listening).on_toggle(save(|s, on| s.switch_automatically = on)));
        if !listening {
            ui.note("These need Bluetooth to listen for the AirPods: Olo Doctor can turn that on.");
        }
        let choices =
            [(MicrophoneChoice::Computer, "This Computer's Microphones"), (MicrophoneChoice::AirPods, "AirPods (High Quality)"), (MicrophoneChoice::AirPodsHeadset, "AirPods (Call Quality)")];
        let at = choices.iter().position(|(c, _)| *c == settings.microphone).unwrap_or(0);
        let names: Vec<SharedString> = choices.iter().map(|(_, n)| SharedString::from(*n)).collect();
        let values: Vec<MicrophoneChoice> = choices.iter().map(|(c, _)| *c).collect();
        ui.add(MenuField::new("airpods-record-from", names, at).label("Record from:").on_change(move |i, _| {
            let mut s = link.shared.lock().unwrap().settings.clone();
            s.microphone = values[*i];
            link.shared.lock().unwrap().settings = s.clone();
            let _ = link.tx.send(Job::Save(s));
        }));
    });
    ui.add(BBox::new("On This Computer").children(items));
}

/// What the AirPods service last published (other panels show it too:
/// Bluetooth's list, Sound's AirPods box). Followed from now on.
pub fn current(_cx: &mut WindowCx) -> (AirPodsState, AirPodsSettings) {
    let link = link(olo_kit::proxy());
    let s = link.shared.lock().unwrap();
    (s.state.clone(), s.settings.clone())
}

/// Save Olo's AirPods settings (on the worker).
pub fn save_settings(_cx: &mut WindowCx, settings: AirPodsSettings) {
    let link = link(olo_kit::proxy());
    link.shared.lock().unwrap().settings = settings.clone();
    let _ = link.tx.send(Job::Save(settings));
}

/// Ask the AirPods service something (on the worker).
pub fn ask(_cx: &mut WindowCx, request: Request) {
    let _ = link(olo_kit::proxy()).tx.send(Job::Ask(request));
}
