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
    gpui::{Context, SharedString},
    widgets::{CheckBox, EmptyState, MenuField, Slider},
};

use crate::{Msg, PanelView, form::Ui, panel};

/// What the watcher last saw: None until it has spoken; Some(None) while
/// there is no sound server.
type Shared = Arc<Mutex<Option<Option<Devices>>>>;

enum Change {
    Default(Kind, String),
    Volume(Kind, String, f32, usize),
    Mute(Kind, String, bool),
}

struct Link {
    devices: Shared,
    tx: Sender<Change>,
}

/// Test data: speakers, a headset and a microphone named as they come.
fn test_devices() -> Devices {
    let device = |name: &str, description: &str, level, is_default| audio::Device { name: name.into(), description: description.into(), volume: audio::Volume { level, muted: false }, channels: 2, is_default };
    Devices {
        outputs: vec![
            device("speakers", "MacBook Pro Speakers (Built-in Audio Analog Stereo)", 0.55, true),
            device("headset", "Jabra Evolve2 85 Wireless Headset (USB Audio, Digital Stereo IEC958)", 0.7, false),
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
            return Link { devices, tx: channel().0 };
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
        Link { devices, tx }
    })
}

fn apply(change: Change) {
    let r = match &change {
        Change::Default(kind, name) => audio::set_default(*kind, name),
        Change::Volume(kind, name, level, channels) => audio::set_device_volume(*kind, name, *level, *channels),
        Change::Mute(kind, name, mute) => audio::set_device_muted(*kind, name, *mute),
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
    section(ui, st, &devices, Kind::Output);
    section(ui, st, &devices, Kind::Input);
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
    });
}
