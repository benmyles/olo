//! Sound: where sound goes and comes from, and how loud. Talks to the
//! sound server natively (`olo_system::audio`): a watcher thread keeps
//! the device list current, a worker thread applies changes, newest wins,
//! so dragging a slider never waits on the server.

use std::{
    sync::{
        Arc, Mutex, OnceLock,
        mpsc::{Sender, channel},
    },
    time::{Duration, Instant},
};

use olo_kit::{
    egui::{Context, Ui},
    widgets::{BBox, CheckBox, MenuField, Slider, StringView},
};
use olo_system::audio::{self, Devices, Kind};

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
    /// Every context that shows the panel, to wake on a change.
    contexts: Arc<Mutex<Vec<Context>>>,
}

fn link() -> &'static Link {
    static LINK: OnceLock<Link> = OnceLock::new();
    LINK.get_or_init(|| {
        let devices: Shared = Default::default();
        let contexts: Arc<Mutex<Vec<Context>>> = Default::default();
        let (shared, wake) = (devices.clone(), contexts.clone());
        std::thread::Builder::new()
            .name("sound-watch".into())
            .spawn(move || {
                audio::watch_devices(|d| {
                    *shared.lock().unwrap() = Some(d);
                    for ctx in wake.lock().unwrap().iter() {
                        ctx.request_repaint();
                    }
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
        Link { devices, tx, contexts }
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

#[derive(Default)]
pub struct State {
    /// The level being dragged, so the server's (slightly older) echoes
    /// don't make the knob jump: (kind, device, level, when).
    dragging: Option<(Kind, String, f32, Instant)>,
}

pub fn ui(ui: &mut Ui, st: &mut State) {
    let link = link();
    {
        let mut ctxs = link.contexts.lock().unwrap();
        if !ctxs.iter().any(|c| c == ui.ctx()) {
            ctxs.push(ui.ctx().clone());
        }
    }
    let devices = link.devices.lock().unwrap().clone();
    let devices = match devices {
        None => {
            ui.add(StringView::new("Looking for sound devices…").dim(true));
            return;
        }
        Some(None) => {
            ui.add(StringView::new("No sound server is running.").dim(true));
            return;
        }
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
    BBox::new(title).show(ui, |ui| {
        if list.is_empty() {
            ui.add(StringView::new(none).dim(true));
            return;
        }
        let names: Vec<&str> = list.iter().map(|d| d.description.as_str()).collect();
        let current = list.iter().position(|d| d.is_default).unwrap_or(0);
        let mut i = current;
        ui.add(MenuField::new(("sound-device", title), &mut i, &names).label(menu_label).divider(130.0).width(ui.available_width()));
        if i != current {
            let _ = link().tx.send(Change::Default(kind, list[i].name.clone()));
        }
        let dev = &list[current];
        let mut level = match &st.dragging {
            Some((k, n, l, _)) if *k == kind && *n == dev.name => *l,
            _ => dev.volume.level,
        };
        let before = level;
        let text = format!("{:.0}%", level * 100.0);
        let label = if kind == Kind::Output { "Volume" } else { "Input level" };
        ui.add_enabled_ui(!dev.volume.muted, |ui| {
            ui.add(Slider::new(&mut level, 0.0..=1.0).step(0.01).label(label).value_text(text));
        });
        if level != before {
            st.dragging = Some((kind, dev.name.clone(), level, Instant::now()));
            let _ = link().tx.send(Change::Volume(kind, dev.name.clone(), level, dev.channels));
        }
        let mut muted = dev.volume.muted;
        ui.add(CheckBox::new(&mut muted, "Mute"));
        if muted != dev.volume.muted {
            let _ = link().tx.send(Change::Mute(kind, dev.name.clone(), muted));
        }
    });
}
