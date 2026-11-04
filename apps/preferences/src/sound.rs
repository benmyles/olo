//! Sound: where sound goes and comes from, and how loud. Talks to the
//! sound server natively (`olo_system::audio`): a watcher thread keeps
//! the device list current, a worker thread applies changes, newest wins,
//! so dragging a slider never waits on the server.

use std::{
    sync::{
        Arc, OnceLock,
        mpsc::{Sender, channel},
    },
    time::{Duration, Instant},
};

use olo_sync::Mutex;
use olo_system::audio::{self, Devices, Kind};
use olo_ui::{
    gpui::{Context, ParentElement as _, SharedString, Styled as _, div, px},
    widgets::{Button, CheckBox, EmptyState, MenuField, Slider},
};

use crate::{Msg, PanelView, form::Ui, panel};
use olo_protocol::voice;

/// What the watcher last saw: None until it has spoken; Some(None) while
/// there is no sound server.
type Shared = Arc<Mutex<Option<Option<Devices>>>>;

enum Change {
    Default(Kind, String),
    Volume(Kind, String, f32, usize),
    Mute(Kind, String, bool),
    Voice(voice::Settings),
}

struct Link {
    devices: Shared,
    /// The voice service's state and settings, followed.
    voice: Arc<Mutex<(voice::State, voice::Settings)>>,
    tx: Sender<Change>,
}

/// Test data for the voice processing box: listening to a microphone
/// named as long as they come, cancelling echo.
fn test_voice() -> (voice::State, voice::Settings) {
    let state = voice::State {
        pid: 1,
        running: true,
        microphone: Some("mic".into()),
        microphone_label: Some("MacBook Pro Microphone (Built-in Audio Analog Stereo)".into()),
        echo_cancelling: true,
        latency_ms: 40,
        ..voice::State::default()
    };
    (state, voice::Settings::default())
}

/// Test data: speakers, a headset and a microphone named as they come.
fn test_devices() -> Devices {
    let device = |name: &str, description: &str, level, is_default| audio::Device {
        name: name.into(),
        description: description.into(),
        volume: audio::Volume { level, muted: false },
        channels: 2,
        is_default,
        bluetooth: None,
        private: false,
    };
    Devices {
        outputs: vec![
            device("speakers", "MacBook Pro Speakers (Built-in Audio Analog Stereo)", 0.55, false),
            device("headset", "Jabra Evolve2 85 Wireless Headset (USB Audio, Digital Stereo IEC958)", 0.7, false),
            // As the AirPods panel's test pair: their output, default here
            // so their part of the panel is laid out too.
            audio::Device {
                is_default: true,
                bluetooth: Some(audio::Bluetooth { address: "02:00:00:00:A1:01".into(), codec: "msbc".into(), profile: "headset-head-unit".into() }),
                ..device("airpods", "Travel AirPods Pro (2nd generation) for the Office", 0.4, false)
            },
        ],
        inputs: vec![device("mic", "MacBook Pro Microphone Array (Built-in Audio Analog Stereo)", 0.8, true)],
    }
}

/// The sound server's watcher and the worker that changes it (one for
/// the app), telling the windows through `proxy` when anything changes.
fn link(proxy: olo_ui::Proxy) -> &'static Link {
    static LINK: OnceLock<Link> = OnceLock::new();
    LINK.get_or_init(|| {
        let devices: Shared = Default::default();
        // Tests (the layout lint): devices as long-named as real ones,
        // and no sound server.
        if cfg!(test) {
            *devices.lock().unwrap() = Some(Some(test_devices()));
            return Link { devices, voice: Arc::new(Mutex::new(test_voice())), tx: channel().0 };
        }
        let voice = Arc::new(Mutex::new((voice::State::load(), voice::Settings::load())));
        let read = {
            let (voice, proxy) = (voice.clone(), proxy.clone());
            move |_: &[std::path::PathBuf]| {
                let now = (voice::State::load(), voice::Settings::load());
                let mut v = voice.lock().unwrap();
                if *v != now {
                    *v = now;
                    drop(v);
                    proxy.send(Msg::Redraw);
                }
            }
        };
        if let Some(w) = olo_ui::watch::watch([voice::State::path(), voice::Settings::path()], read) {
            // The app's, for as long as it runs.
            std::mem::forget(w);
        }
        let shared = devices.clone();
        std::thread::Builder::new()
            .name("sound-watch".into())
            .spawn(move || {
                audio::watch_devices(|d| {
                    *shared.lock().unwrap() = Some(d);
                    proxy.send(Msg::Redraw);
                })
            })
            .expect("thread");
        let (tx, rx) = channel::<Change>();
        std::thread::Builder::new()
            .name("sound-apply".into())
            .spawn(move || {
                while let Ok(mut change) = rx.recv() {
                    // A slider drag queues many volumes: only the newest matters.
                    while let Ok(next) = rx.try_recv() {
                        let same = matches!((&change, &next),
                            (Change::Volume(k1, n1, ..), Change::Volume(k2, n2, ..)) if k1 == k2 && n1 == n2);
                        if !same {
                            apply(change);
                        }
                        change = next;
                    }
                    apply(change);
                }
            })
            .expect("thread");
        Link { devices, voice, tx }
    })
}

fn apply(change: Change) {
    let r = match &change {
        Change::Default(kind, name) => audio::set_default(*kind, name),
        Change::Volume(kind, name, level, channels) => audio::set_device_volume(*kind, name, *level, *channels),
        Change::Mute(kind, name, mute) => audio::set_device_muted(*kind, name, *mute),
        Change::Voice(settings) => {
            if let Err(e) = settings.save() {
                tracing::warn!("sound: voice settings: {e}");
            }
            Ok(())
        }
    };
    if let Err(e) = r {
        tracing::warn!("sound: {e}");
    }
}

pub struct State {
    /// The level being dragged, so the server's (slightly older) echoes
    /// don't make the knob jump: (kind, device, level, when).
    dragging: Option<(Kind, String, f32, Instant)>,
    link: &'static Link,
}

impl State {
    pub fn new(cx: &mut Context<PanelView>) -> State {
        State { dragging: None, link: link(olo_ui::proxy(cx)) }
    }
}

fn state_of(v: &mut PanelView) -> Option<&mut State> {
    match &mut v.state {
        panel::State::Sound(st) => Some(st),
        _ => None,
    }
}

pub fn ui(ui: &mut Ui, st: &mut State) {
    let devices = st.link.devices.lock().unwrap().clone();
    let devices = match devices {
        None => return ui.add(EmptyState::new("Looking for sound devices…").icon("sound")),
        Some(None) => return ui.add(EmptyState::new("No sound server is running.").icon("sound")),
        Some(Some(d)) => d,
    };
    if st.dragging.as_ref().is_some_and(|(.., at)| at.elapsed() > Duration::from_millis(600)) {
        st.dragging = None;
    }
    // (Two columns: the devices, and what's done to the voice.)
    let left = ui.collect(|ui| {
        section(ui, st, &devices, Kind::Output);
        section(ui, st, &devices, Kind::Input);
    });
    let right = ui.collect(|ui| voice_section(ui, st, &devices));
    ui.add_columns(left, right);
}

/// Voice processing (the voice service): on or off, what for, how much
/// noise to take out, echo and level; and what it's doing now.
fn voice_section(ui: &mut Ui, st: &mut State, devices: &Devices) {
    let (state, settings) = st.link.voice.lock().unwrap().clone();
    let tx = st.link.tx.clone();
    let save = move |s: voice::Settings| _ = tx.send(Change::Voice(s));
    ui.bbox("Voice Processing", |ui| {
        let s2 = save.clone();
        ui.add(CheckBox::new("Clean up the microphone's sound", settings.enabled).on_toggle(move |on, _, _| s2(voice::Settings { enabled: on, ..settings })));
        let modes: Vec<SharedString> = voice::Mode::ALL.iter().map(|m| SharedString::from(m.label())).collect();
        let at = voice::Mode::ALL.iter().position(|m| *m == settings.mode).unwrap_or(0);
        let s2 = save.clone();
        ui.add(
            MenuField::new("voice-mode", modes, at)
                .label("Tuned for:")
                .divider(130.0)
                .enabled(settings.enabled)
                .on_change(move |i, _, _| s2(voice::Settings { mode: voice::Mode::ALL[i], ..settings })),
        );
        let levels: Vec<SharedString> = voice::Noise::ALL.iter().map(|n| SharedString::from(n.label())).collect();
        let at = voice::Noise::ALL.iter().position(|n| *n == settings.noise).unwrap_or(0);
        let s2 = save.clone();
        ui.add(
            MenuField::new("voice-noise", levels, at)
                .label("Background noise:")
                .divider(130.0)
                .enabled(settings.enabled)
                .on_change(move |i, _, _| s2(voice::Settings { noise: voice::Noise::ALL[i], ..settings })),
        );
        let s2 = save.clone();
        ui.add(
            CheckBox::new("Cancel echo from the speakers", settings.echo_cancellation).enabled(settings.enabled).on_toggle(move |on, _, _| s2(voice::Settings { echo_cancellation: on, ..settings })),
        );
        let s2 = save.clone();
        // Trying it on your own voice (each setting, heard and measured) at
        // the last setting's right: alone at the box's foot it sat nearly
        // level with the other column's controls. As tall as the button,
        // the check box's text shares its baseline.
        let (prefs, microphone) = (ui.prefs(), state.microphone.clone());
        ui.add(
            div()
                .flex()
                .flex_row()
                .items_center()
                .child(
                    CheckBox::new("Even out the voice's level", settings.normalize)
                        .height(olo_ui::metrics::CONTROL_HEIGHT)
                        .enabled(settings.enabled)
                        .on_toggle(move |on, _, _| s2(voice::Settings { normalize: on, ..settings })),
                )
                .child(div().flex_1())
                .child(Button::new("Try It…").on_click(move |_, cx| {
                    if let Some(p) = prefs.upgrade() {
                        crate::voice_try::open(p, settings, microphone.clone(), cx);
                    }
                })),
        );
        ui.note(voice_note(&state, &settings, devices));
    });
}

/// What the voice service is doing, in a line.
fn voice_note(state: &voice::State, settings: &voice::Settings, devices: &Devices) -> String {
    if !settings.enabled {
        return "Apps get the microphone's sound as it is.".into();
    }
    if state.pid == 0 {
        return "The voice service isn't running.".into();
    }
    if let Some(e) = &state.error {
        return format!("Not working: {e}.");
    }
    // (Which microphone, the Input box above says.)
    let echo = if !settings.echo_cancellation {
        ""
    } else if state.echo_cancelling {
        ", echo cancelled"
    } else if devices.default_device(Kind::Output).is_some_and(|o| o.private) {
        ", no echo with headphones"
    } else {
        ""
    };
    format!("Cleaning up the microphone's sound: {} ms behind{echo}.", state.latency_ms)
}

/// The AirPods behind a sound device, if it is theirs.
fn airpods_of(ui: &mut Ui, dev: &audio::Device) -> Option<olo_protocol::airpods::AirPods> {
    let bt = dev.bluetooth.as_ref()?;
    let (state, _) = crate::airpods::current(ui.cx);
    state.by_address(&bt.address).cloned()
}

/// Under the output menu, for AirPods: how their sound is carried (music
/// quality, or call quality while their microphone is used), their noise
/// control, and the way to all their settings.
fn airpods_output(ui: &mut Ui, dev: &audio::Device) {
    use olo_protocol::airpods::{Change, ListeningMode, Request};
    let Some(a) = airpods_of(ui, dev) else { return };
    let bt = dev.bluetooth.clone().unwrap_or_default();
    let quality =
        if bt.high_quality() { format!("{} · {} · music quality", a.model, bt.codec_name()) } else { format!("{} · {} · call quality while the microphone is used", a.model, bt.codec_name()) };
    ui.note(quality);
    let address = Some(a.address.clone());
    let mut row = div().flex().flex_row().items_center().gap(px(olo_ui::metrics::SPACING));
    if a.abilities.noise_control && a.linked {
        let modes: Vec<ListeningMode> = ListeningMode::ALL.into_iter().filter(|m| *m != ListeningMode::Adaptive || a.abilities.adaptive).collect();
        let names: Vec<SharedString> = modes.iter().map(|m| SharedString::from(m.label())).collect();
        let at = a.settings.listening_mode.and_then(|m| modes.iter().position(|x| *x == m)).unwrap_or(0);
        let address = address.clone();
        row = row.child(MenuField::new("sound-noise-control", names, at).label("Noise control:").on_change(move |i, _, cx| {
            crate::airpods::ask(cx, Request::Set { address: address.clone(), change: Change::ListeningMode(modes[i]) });
        }));
    }
    if !bt.high_quality() {
        let address = address.clone();
        row = row.child(Button::new("Use Music Quality").on_click(move |_, cx| crate::airpods::ask(cx, Request::Quality { address: address.clone(), high: true })));
    }
    let prefs = ui.prefs();
    row = row.child(div().flex_1()).child(Button::new("AirPods Settings…").on_click(move |_, cx| {
        let _ = prefs.update(cx, |p, cx| p.open("airpods", cx));
    }));
    ui.add(row);
}

/// In the input section while AirPods are connected: where to record from.
fn airpods_input(ui: &mut Ui) {
    use olo_protocol::airpods::MicrophoneChoice;
    let (state, settings) = crate::airpods::current(ui.cx);
    if state.connected().is_none() {
        return;
    }
    let choices = [(MicrophoneChoice::Computer, "This Computer's Microphones"), (MicrophoneChoice::AirPods, "AirPods (High Quality)"), (MicrophoneChoice::AirPodsHeadset, "AirPods (Call Quality)")];
    let at = choices.iter().position(|(c, _)| *c == settings.microphone).unwrap_or(0);
    let names: Vec<SharedString> = choices.iter().map(|(_, n)| SharedString::from(*n)).collect();
    let note = match settings.microphone {
        MicrophoneChoice::Computer | MicrophoneChoice::AirPods => "AirPods keep playing at music quality while you record.",
        MicrophoneChoice::AirPodsHeadset => "AirPods play at call quality while their microphone is used.",
    };
    ui.add(MenuField::new("sound-airpods-mic", names, at).label("With AirPods:").divider(130.0).on_change(move |i, _, cx| {
        let mut s = settings.clone();
        s.microphone = choices[i].0;
        crate::airpods::save_settings(cx, s);
    }));
    ui.note(note);
}

fn section(ui: &mut Ui, st: &mut State, devices: &Devices, kind: Kind) {
    let (title, list, none, menu_label) = match kind {
        Kind::Output => ("Output", &devices.outputs, "No output devices.", "Play sound through:"),
        Kind::Input => ("Input", &devices.inputs, "No input devices.", "Record from:"),
    };
    let tx = st.link.tx.clone();
    let dragging = st.dragging.clone();
    ui.bbox(title, |ui| {
        if list.is_empty() {
            ui.note(none);
            return;
        }
        let names: Vec<SharedString> = list.iter().map(|d| SharedString::from(d.description.clone())).collect();
        if kind == Kind::Input {
            airpods_input(ui);
        }
        let current = list.iter().position(|d| d.is_default).unwrap_or(0);
        let (names_of, tx2) = (list.iter().map(|d| d.name.clone()).collect::<Vec<_>>(), tx.clone());
        ui.add(MenuField::new(format!("sound-device-{title}"), names, current).label(menu_label).divider(130.0).on_change(move |i, _, _| {
            if i != current {
                let _ = tx2.send(Change::Default(kind, names_of[i].clone()));
            }
        }));
        let dev = list[current].clone();
        let level = match &dragging {
            Some((k, n, l, _)) if *k == kind && *n == dev.name => *l,
            _ => dev.volume.level,
        };
        let label = if kind == Kind::Output { "Volume" } else { "Input level" };
        let (name, channels, tx2) = (dev.name.clone(), dev.channels, tx.clone());
        let on = ui.on_view(move |v, level: f32, _, cx| {
            if let Some(st) = state_of(v) {
                st.dragging = Some((kind, name.clone(), level, Instant::now()));
            }
            let _ = tx2.send(Change::Volume(kind, name.clone(), level, channels));
            cx.notify();
        });
        ui.add(Slider::new(format!("volume-{title}"), level, 0.0..=1.0).step(0.01).label(label).value_text(format!("{:.0}%", level * 100.0)).enabled(!dev.volume.muted).on_change(on));
        let name = dev.name.clone();
        ui.add(CheckBox::new("Mute", dev.volume.muted).on_toggle(move |muted, _, _| {
            let _ = tx.send(Change::Mute(kind, name.clone(), muted));
        }));
        if kind == Kind::Output {
            airpods_output(ui, &dev);
        }
    });
}
