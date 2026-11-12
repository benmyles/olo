//! Touchpad and mouse. The Touchpad tab names the pad app_server found and
//! the tuning profile that suits it; Touch is Olo's palm and hover filter
//! and the click pressure; Gestures is scrolling, momentum, dragging and
//! what two and three fingers do; Mouse is each mouse by make and model,
//! with the profile that suits it, what is known about it and what gets
//! in its way (fixed by Olo Doctor), then the other mice. The profile
//! sets how the pad feels (protocol `touchpad.rs`; mice: `mice.rs`);
//! changing any of that by hand makes it "Custom". See docs/TOUCHPAD.md.

use std::{sync::Arc, time::Duration};

use olo_kit::{
    Cx, ParentElement as _, SharedString, Styled as _, div, metrics as m, px, style, theme,
    widgets::{Button, StringView, TabView},
};
use olo_protocol::{
    Client, Event, EventKind, Reply, Request, Settings, TouchpadSettings,
    mice::{self, Class, MouseTuning, Quirk},
    touchpad::{CLICK_PRESSURES, DeviceKind, InputDeviceInfo, PROFILES},
};
use olo_sync::Mutex;

use crate::{Msg, PanelView, form::Ui, panel};

/// The pointing devices app_server last reported: None until it has.
type Shared = Arc<Mutex<Option<Vec<InputDeviceInfo>>>>;

pub struct State {
    tab: usize,
    devices: Shared,
}

impl State {
    pub fn new(_cx: &mut Cx<PanelView>) -> State {
        State { tab: 0, devices: watch(olo_kit::proxy()) }
    }
}

fn state_of(v: &mut PanelView) -> Option<&mut State> {
    match &mut v.state {
        panel::State::Touchpad(st) => Some(st),
        _ => None,
    }
}

/// Asks app_server for its devices, then hears of each one that comes or
/// goes (an event: nothing is polled). The thread waits on app_server and
/// ends at the first event after the panel has closed.
fn watch(proxy: olo_kit::Proxy) -> Shared {
    let devices: Shared = Default::default();
    // Tests (the layout lint): a MacBook's pad and a mouse named as
    // long as they come, and no app_server.
    if cfg!(test) {
        let device = |name: &str, kind, size_mm, pressure, recommended: &str, vendor, product, bus: &str, dpi, may_sleep| InputDeviceInfo {
            name: name.into(),
            kind,
            size_mm,
            pressure,
            recommended: recommended.into(),
            // (The Apple driver's thresholds: the Click box's note.)
            driver_click: (kind == olo_protocol::touchpad::DeviceKind::AppleMtp).then_some((1200, 800)),
            vendor,
            product,
            bus: bus.into(),
            dpi,
            may_sleep,
            // (A ThinkPad's TrackPoint, as the kernel names the P1 Gen 9's.)
            pointing_stick: name.starts_with("PS/2 "),
        };
        *devices.lock().unwrap() = Some(vec![
            device("Apple MTP multi-touch", olo_protocol::touchpad::DeviceKind::AppleMtp, Some((160.0, 100.0)), false, "macbook", 0x05ac, 0x0342, "spi", None, false),
            device("Logitech G309 LIGHTSPEED Wireless Gaming Mouse (USB Receiver)", olo_protocol::touchpad::DeviceKind::Mouse, None, false, "mouse", 0x046d, 0x40b1, "usb", Some(1600), true),
            device("PS/2 Logitech Mouse", olo_protocol::touchpad::DeviceKind::Mouse, None, false, "mouse", 0x0002, 0x0001, "i8042", None, false),
        ]);
        return devices;
    }
    let out = Arc::downgrade(&devices);
    std::thread::Builder::new()
        .name("input-devices".into())
        .spawn(move || {
            let store = |found: Vec<InputDeviceInfo>| {
                let Some(shared) = out.upgrade() else { return false };
                let mut slot = shared.lock().unwrap();
                if slot.as_ref() != Some(&found) {
                    *slot = Some(found);
                    proxy.send(Msg::Redraw);
                }
                true
            };
            loop {
                // Subscribed before asking, so nothing comes between.
                let events = Client::connect().ok().and_then(|c| c.into_events_of(&[EventKind::InputDevices]).ok());
                let now = Client::connect().ok().and_then(|mut c| match c.request(&Request::InputDevices) {
                    Ok(Reply::InputDevices { devices }) => Some(devices),
                    _ => None,
                });
                if let Some(now) = now
                    && !store(now)
                {
                    return;
                }
                for e in events.into_iter().flatten() {
                    let Ok(Event::InputDevicesChanged { devices }) = e else { break };
                    if !store(devices) {
                        return;
                    }
                }
                if out.strong_count() == 0 {
                    return;
                }
                // app_server went (a restart): connect again once it's back.
                std::thread::sleep(Duration::from_secs(1));
            }
        })
        .expect("thread");
    devices
}

/// A change to the pad's tuning: by hand, it makes the profile "Custom"
/// (unless it still matches it).
fn tuned(s: &mut Settings, f: impl FnOnce(&mut TouchpadSettings)) {
    let before = s.touchpad.clone();
    f(&mut s.touchpad);
    let t = &mut s.touchpad;
    if *t != before && t.profile == before.profile && t.profile != "custom" && !t.matches_profile(&t.profile) {
        t.profile = "custom".into();
    }
}

pub fn ui(ui: &mut Ui, st: &mut State) {
    let devices = st.devices.lock().unwrap().clone();
    let pad = devices.as_ref().and_then(|d| d.iter().find(|d| d.kind != DeviceKind::Mouse).cloned());
    let t = ui.s.touchpad.clone();
    let items = ui.collect(|ui| match st.tab {
        0 => touchpad(ui, &t, devices.is_some(), pad.as_ref()),
        1 => touch(ui, &t, pad.as_ref()),
        2 => gestures(ui, &t),
        _ => {
            // (As many mice as there are: the window grows to them,
            // `panel::Fit`.)
            mice_boxes(ui, devices.as_deref());
        }
    });
    let pick = ui.on_view(|v, i: &usize, cx| {
        if let Some(st) = state_of(v) {
            st.tab = *i;
        }
        cx.notify();
    });
    let tabs: Vec<SharedString> = ["Touchpad", "Touch", "Gestures", "Mouse"].into_iter().map(SharedString::from).collect();
    ui.add(
        div()
            .flex_1()
            .flex()
            .flex_col()
            .child(TabView::new("touchpad-tabs", tabs, st.tab).grows().on_select(pick).child(div().flex_1().flex().flex_col().gap(px(m::SPACING)).p(px(m::SPACING)).children(items))),
    );
}

fn profile_name(id: &str) -> &str {
    PROFILES.iter().find(|(p, _, _)| *p == id).map_or(id, |(_, name, _)| name)
}

const ACCELS: [(&str, &str); 2] = [("adaptive", "Adaptive"), ("flat", "None (flat)")];

/// A menu of `choices` (value, name) for a string setting.
fn strings(ui: &mut Ui, id: &str, label: &str, choices: &[(&str, &str)], value: &str, set: impl Fn(&mut Settings, String) + 'static) {
    let owned: Vec<(String, &str)> = choices.iter().map(|(v, n)| (v.to_string(), *n)).collect();
    let label = SharedString::from(label.to_owned());
    ui.choose(id, &owned, &value.to_owned(), |f| f.label(label).divider(130.0), set);
}

fn touchpad(ui: &mut Ui, t: &TouchpadSettings, asked: bool, pad: Option<&InputDeviceInfo>) {
    ui.columns(
        |ui| {
            ui.bbox("This Touchpad", |ui| {
                match pad {
                    Some(pad) => {
                        let size = pad.size_mm.map(|(w, h)| format!(" · {w:.0} × {h:.0} mm")).unwrap_or_default();
                        ui.label(format!("{}{size}", pad.kind.describe()));
                        ui.note(pad.name.clone());
                    }
                    None if asked => ui.note("No touchpad found."),
                    None => ui.note("Looking…"),
                }
                let names: Vec<SharedString> = PROFILES.iter().map(|(_, name, _)| SharedString::from(*name)).collect();
                let i = PROFILES.iter().position(|(id, _, _)| *id == t.profile).unwrap_or(PROFILES.len() - 1);
                ui.menu(
                    "profile",
                    names,
                    i,
                    |f| f.label("Profile:").divider(70.0),
                    |s, i| {
                        if let Some((id, _, _)) = PROFILES.get(i)
                            && *id != s.touchpad.profile
                        {
                            s.touchpad.apply_profile(id);
                        }
                    },
                );
                if let Some((_, _, about)) = PROFILES.get(i) {
                    ui.note(*about);
                }
                if let Some(pad) = pad {
                    let recommended = pad.recommended.clone();
                    if t.profile == recommended {
                        ui.note("This is the recommended profile for this touchpad.");
                    } else {
                        let name = format!("Use {}", profile_name(&recommended));
                        let use_it = ui.edit_click(move |s| s.touchpad.apply_profile(&recommended));
                        ui.row(|ui| {
                            ui.button(name, use_it);
                            ui.note("recommended for this touchpad");
                        });
                    }
                }
            });
        },
        |ui| {
            ui.bbox("Basics", |ui| {
                ui.slider("tracking-speed", t.speed as f32, -1.0..=1.0, |s| s.step(0.05).label("Tracking speed").limits("Slow", "Fast"), |s, v| tuned(s, |t| t.speed = v as f64));
                ui.check("Tap to click", t.tap_to_click, |s, v| tuned(s, |t| t.tap_to_click = v));
                ui.enabled_if(t.tap_to_click, |ui| ui.check("Tap and drag", t.tap_and_drag, |s, v| tuned(s, |t| t.tap_and_drag = v)));
                ui.check("Natural scrolling (content follows your fingers)", t.natural_scroll, |s, v| tuned(s, |t| t.natural_scroll = v));
                ui.check("Click with two fingers for a secondary click", t.clickfinger, |s, v| tuned(s, |t| t.clickfinger = v));
            });
        },
    );
}

/// The kind of pad `pad` is, for its palm model (`olo_palm::Kind`).
fn palm_kind(pad: Option<&InputDeviceInfo>) -> olo_palm::Kind {
    if pad.is_some_and(|p| p.kind == olo_protocol::touchpad::DeviceKind::Thinkpad) { olo_palm::Kind::ThinkPad } else { olo_palm::Kind::MacBook }
}

fn touch(ui: &mut Ui, t: &TouchpadSettings, pad: Option<&InputDeviceInfo>) {
    let f = t.filter.clone();
    let kind = palm_kind(pad);
    // A ThinkPad's pad: no hover, no size, its click its own: palms only.
    let pressure_pad = kind.sized();
    ui.columns(
        |ui| {
            ui.bbox(if pressure_pad { "Palms and Hovering Fingers" } else { "Palms" }, |ui| {
                if pressure_pad {
                    ui.check("Ignore hovering fingers and palms", f.enabled, |s, v| tuned(s, |t| t.filter.enabled = v));
                } else {
                    ui.check("Keep palms from moving the pointer", f.enabled, |s, v| tuned(s, |t| t.filter.enabled = v));
                    ui.note("The touchpad says which touches are palms, but lets a resting palm go back to being a finger; Olo keeps it a palm until it lifts.");
                }
                if pressure_pad && pad.is_some_and(|p| !p.pressure) {
                    ui.note("This touchpad doesn't report pressure, so it is left to libinput.");
                }
                ui.enabled_if(f.enabled, |ui| {
                    if !pressure_pad {
                        let trained = f.palm_model.as_ref().is_some_and(|m| olo_palm::Kind::of_model(m) == kind);
                        ui.check("Tell palms apart with a palm model too", f.use_palm_model, |s, v| tuned(s, |t| t.filter.use_palm_model = v));
                        ui.note(model_status(f.palm_model.as_ref(), f.use_palm_model, kind));
                        let (palm_size, press) = (f.palm_size, f.click_press);
                        ui.row(|ui| {
                            ui.button(if trained { "Train Again…" } else { "Train for Your Palm…" }, move |_, cx| {
                                crate::palm_trainer::open(palm_size, press, cx);
                            });
                            ui.enabled_if(trained, |ui| {
                                ui.button(
                                    "Forget My Model",
                                    ui.edit_click(|s| {
                                        s.touchpad.filter.palm_model = None;
                                    }),
                                )
                            });
                        });
                        return;
                    }
                    let on = f.touch_on as f32;
                    ui.slider(
                        "touch-on",
                        on,
                        1.0..=40.0,
                        |s| s.step(1.0).label("A touch starts at pressure").value_text(format!("{on:.0}")).limits("Any touch", "Firmer"),
                        |s, v| tuned(s, |t| t.filter.touch_on = v as u32),
                    );
                    // The model and the size rule work together: a contact is a
                    // palm if either says so. The model first (it catches what
                    // size alone misses), then the size every contact as large as
                    // is a palm regardless.
                    // Olo's own model until the person trains theirs (on by
                    // default: it's made from real hands).
                    let trained = f.palm_model.as_ref().is_some_and(|m| olo_palm::Kind::of_model(m) == kind);
                    ui.check("Tell palms apart with a palm model", f.use_palm_model, |s, v| tuned(s, |t| t.filter.use_palm_model = v));
                    ui.note(model_status(f.palm_model.as_ref(), f.use_palm_model, kind));
                    let (palm_size, press) = (f.palm_size, f.click_press);
                    ui.row(|ui| {
                        // The trainer: full screen, driven from the keyboard.
                        ui.button(if trained { "Train Again…" } else { "Train for Your Palm…" }, move |_, cx| {
                            crate::palm_trainer::open(palm_size, press, cx);
                        });
                        ui.enabled_if(trained, |ui| {
                            ui.button(
                                "Forget My Model",
                                ui.edit_click(|s| {
                                    s.touchpad.filter.palm_model = None;
                                }),
                            )
                        });
                    });
                    let palm = f.palm_size as f32;
                    ui.slider(
                        "palm-size",
                        palm,
                        600.0..=2000.0,
                        |s| s.step(50.0).label("Always a palm when as large as").value_text(format!("{palm:.0}")).limits("Smaller", "Larger"),
                        |s, v| tuned(s, |t| t.filter.palm_size = v as u32),
                    );
                    ui.note(if f.use_palm_model { "Whatever the model says: it adds palms, never takes these away." } else { "Without a palm model, size alone tells a palm." });
                    ui.check("A resting palm can't click (its pressure doesn't count)", f.palm_proof_click, |s, v| tuned(s, |t| t.filter.palm_proof_click = v));
                });
            });
        },
        |ui| {
            ui.bbox("Click", |ui| {
                // (A ThinkPad's click is its own button: nothing to set.)
                if !pressure_pad {
                    ui.note("Its click is its own: a resting palm can't press.");
                    return;
                }
                {
                    let mut names: Vec<SharedString> = CLICK_PRESSURES.iter().map(|(n, _, _)| SharedString::from(*n)).collect();
                    let i = match CLICK_PRESSURES.iter().position(|(_, p, r)| *p == f.click_press && *r == f.click_release) {
                        Some(i) => i,
                        None => {
                            names.push(format!("Custom ({}/{})", f.click_press, f.click_release).into());
                            names.len() - 1
                        }
                    };
                    ui.enabled_if(f.enabled && f.palm_proof_click, |ui| {
                        ui.menu(
                            "click-pressure",
                            names,
                            i,
                            |m| m.label("Click pressure:").divider(110.0),
                            |s, i| {
                                if let Some((_, press, release)) = CLICK_PRESSURES.get(i) {
                                    tuned(s, |t| (t.filter.click_press, t.filter.click_release) = (*press, *release));
                                }
                            },
                        );
                    });
                    if let Some((press, release)) = pad.and_then(|p| p.driver_click) {
                        ui.note(format!("The pad clicks at {press}/{release} (all contacts together); with a palm on it, Olo counts the click only if the fingers press that hard alone."));
                    }
                }
            });
            ui.bbox("While Typing", |ui| {
                ui.check("Ignore the touchpad while typing", t.disable_while_typing, |s, v| tuned(s, |t| t.disable_while_typing = v));
                ui.enabled_if(t.disable_while_typing, |ui| {
                    let choices = [(0u32, "libinput's default"), (300, "0.3 seconds"), (500, "0.5 seconds"), (1000, "1 second"), (2000, "2 seconds")];
                    ui.choose("dwt-timeout", &choices, &t.dwt_timeout_ms, |f| f.label("For:").divider(130.0), |s, v| tuned(s, |t| t.dwt_timeout_ms = v));
                });
                ui.check("Ignore the touchpad while the pointing stick is used", t.dwtp, |s, v| tuned(s, |t| t.dwtp = v));
                ui.check("Turn the touchpad off while a mouse is plugged in", t.off_with_mouse, |s, v| tuned(s, |t| t.off_with_mouse = v));
            });
        },
    );
}

fn gestures(ui: &mut Ui, t: &TouchpadSettings) {
    ui.columns(
        |ui| {
            ui.bbox("Scrolling", |ui| {
                let speed = t.scroll_speed as f32;
                ui.slider(
                    "scroll-speed",
                    speed,
                    0.25..=3.0,
                    |s| s.step(0.05).label("Scrolling speed").value_text(format!("{speed:.2}×")).limits("Slow", "Fast"),
                    |s, v| tuned(s, |t| t.scroll_speed = v as f64),
                );
                ui.check("Keep scrolling after a flick", t.momentum, |s, v| tuned(s, |t| t.momentum = v));
                let glide = t.glide as f32;
                ui.enabled_if(t.momentum, |ui| {
                    ui.slider("glide", glide, 0.25..=3.0, |s| s.step(0.05).label("Glide").value_text(format!("{glide:.2}×")).limits("Short", "Long"), |s, v| tuned(s, |t| t.glide = v as f64));
                });
                strings(ui, "scroll-method", "Scroll with:", &[("two-finger", "Two fingers"), ("edge", "The right edge")], &t.scroll_method, |s, v| tuned(s, |t| t.scroll_method = v));
                ui.check("Pinch to zoom", t.pinch_zoom, |s, v| tuned(s, |t| t.pinch_zoom = v));
            });
        },
        |ui| {
            ui.bbox("Clicking and Dragging", |ui| {
                strings(ui, "button-map", "Two fingers:", &[("lrm", "Secondary click (three: middle)"), ("lmr", "Middle click (three: secondary)")], &t.tap_button_map, |s, v| {
                    tuned(s, |t| {
                        t.clickfinger_button_map = v.clone();
                        t.tap_button_map = v;
                    })
                });
                let drags = [(0u8, "Click and hold"), (3, "Three fingers"), (4, "Four fingers")];
                ui.choose("three-finger-drag", &drags, &t.three_finger_drag, |f| f.label("Drag with:").divider(130.0), |s, v| tuned(s, |t| t.three_finger_drag = v));
                ui.enabled_if(t.tap_to_click && t.tap_and_drag, |ui| {
                    strings(ui, "drag-lock", "After tap and drag:", &[("off", "Drop on lift"), ("timeout", "Keep briefly"), ("sticky", "Keep until tapped")], &t.drag_lock, |s, v| {
                        tuned(s, |t| t.drag_lock = v)
                    });
                });
                strings(ui, "accel", "Acceleration:", &ACCELS, &t.accel_profile, |s, v| tuned(s, |t| t.accel_profile = v));
            });
        },
    );
}

/// A remapper's own pointer ("keyd virtual pointer"): the mice it has
/// grabbed move through it.
fn remapper(d: &InputDeviceInfo) -> Option<&str> {
    d.name.strip_suffix(" virtual pointer").filter(|_| d.kind == DeviceKind::Mouse)
}

/// A problem in `failure`'s colour, and the button that fixes it.
fn problem(ui: &mut Ui, text: String) {
    let p = theme::palette();
    ui.add(StringView::new(text).color(style::c(p.failure)).wrap(true));
    ui.add(Button::new("Fix in Olo Doctor…").on_click(|_, _| {
        let _ = olo_xdg::open::spawn_app(&["olo-doctor".into()], "olo.doctor");
    }));
}

fn mice_boxes(ui: &mut Ui, devices: Option<&[InputDeviceInfo]>) {
    let all = devices.unwrap_or(&[]);
    if let Some(r) = all.iter().find_map(remapper) {
        problem(ui, format!("{r} is grabbing input devices: a mouse it has grabbed moves through its virtual pointer, and settings made here don't reach it."));
    }
    let found: Vec<&InputDeviceInfo> = all.iter().filter(|d| d.kind == DeviceKind::Mouse && remapper(d).is_none() && d.bus != "virtual").collect();
    // (Two columns: the mice found, and what's for every mouse, the
    // computer's own pointing stick above it.)
    // (The wheel's speed under the mice: the other column has the
    // pointing stick and what's for every mouse.)
    let mice = ui.collect(|ui| {
        for d in found.iter().filter(|d| !d.pointing_stick) {
            one_mouse(ui, d);
        }
        wheel(ui);
    });
    let every = ui.collect(|ui| {
        for d in found.iter().filter(|d| d.pointing_stick) {
            one_mouse(ui, d);
        }
        every_mouse(ui, devices, found.is_empty());
    });
    if mice.is_empty() {
        for e in every {
            ui.add(e);
        }
    } else {
        ui.add_columns(mice, every);
    }
}

/// What's for every mouse: how the others move, the buttons, focus.
fn every_mouse(ui: &mut Ui, devices: Option<&[InputDeviceInfo]>, none: bool) {
    let s = ui.s.clone();
    ui.bbox(if none { "Mice" } else { "Other Mice" }, |ui| {
        match devices {
            None => ui.note("Looking…"),
            Some(_) if none => ui.note("No mouse is connected. These settings are for any you connect."),
            Some(_) => ui.note("For the mice set to follow the others, and any you connect."),
        }
        ui.slider("mouse-speed", s.mouse_speed as f32, -1.0..=1.0, |sl| sl.step(0.05).label("Mouse speed").limits("Slow", "Fast"), |s, v| s.mouse_speed = v as f64);
        strings(ui, "mouse-accel", "Acceleration:", &ACCELS, &s.touchpad.mouse_accel_profile, |s, v| s.touchpad.mouse_accel_profile = v);
    });

    ui.bbox("Buttons", |ui| {
        ui.check("Left-handed (swap the buttons)", s.touchpad.left_handed, |s, v| s.touchpad.left_handed = v);
        ui.check("Press both buttons for a middle click", s.touchpad.middle_emulation, |s, v| s.touchpad.middle_emulation = v);
    });
    ui.bbox("Focus", |ui| {
        ui.check("Focus follows the pointer", s.focus_follows_mouse, |s, v| s.focus_follows_mouse = v);
    });
}

/// How far a wheel's notch scrolls, every program alike.
fn wheel(ui: &mut Ui) {
    let speed = ui.s.wheel_speed as f32;
    ui.bbox("Wheel", |ui| {
        ui.slider("wheel-speed", speed, 0.25..=3.0, |sl| sl.step(0.05).label("Scrolling speed").value_text(format!("{speed:.2}×")).limits("Slow", "Fast"), |s, v| s.wheel_speed = v as f64);
        ui.note("A notch scrolls as far in Chrome and other programs as in Olo’s apps.");
    });
}

/// A mouse: what it is, how it's set up, what is known about it.
fn one_mouse(ui: &mut Ui, d: &InputDeviceInfo) {
    let model = mice::identify(&d.name, d.vendor, d.product);
    // (A ThinkPad's pointing stick, whatever the kernel named it.)
    let title = if d.pointing_stick { "TrackPoint".to_owned() } else { model.map_or_else(|| d.name.clone(), mice::Model::full_name) };
    let key = mice::key(d.vendor, d.product);
    let s = ui.s.clone();
    ui.bbox(&title, |ui| {
        let mut facts: Vec<String> = Vec::new();
        match model {
            Some(m) => {
                facts.extend(m.sensor.map(|s| format!("{s} sensor")));
                facts.extend(m.max_dpi.map(|n| format!("up to {} dpi", mice::thousands(n))));
            }
            None => facts.extend(mice::maker(d.vendor).filter(|maker| !d.name.contains(maker)).map(str::to_owned)),
        }
        facts.extend(d.dpi.map(|n| format!("{} dpi as it comes", mice::thousands(n))));
        let receiver = model.is_some_and(|m| m.quirks.contains(&Quirk::Receiver));
        match d.bus.as_str() {
            "usb" if receiver => facts.push("USB receiver".into()),
            "usb" => facts.push("USB".into()),
            "bluetooth" => facts.push("Bluetooth".into()),
            _ => {}
        }
        if d.pointing_stick {
            // (Its ids are PS/2's, the same for every one.)
            facts = vec!["Built into the keyboard".into()];
        } else if d.vendor != 0 {
            facts.push(key.clone());
        }
        ui.note(facts.join(" · "));
        // Following the others, its model's profile, or its own.
        let current = match s.mice.get(&key) {
            None => "others",
            Some(t) if t.profile == "custom" => "custom",
            Some(_) => "recommended",
        };
        let (k, name, vendor, product) = (key.clone(), d.name.clone(), d.vendor, d.product);
        strings(ui, &format!("mouse-{key}"), "Settings:", &[("recommended", "Recommended for it"), ("others", "As the other mice"), ("custom", "Its own")], current, move |s, chosen| {
            match chosen.as_str() {
                "others" => drop(s.mice.remove(&k)),
                "recommended" => drop(s.mice.insert(k.clone(), MouseTuning::default())),
                _ => {
                    // Its own, starting from what it has now.
                    let (speed, accel) = mice::effective(s, &name, vendor, product);
                    s.mice.insert(k.clone(), MouseTuning { profile: "custom".into(), speed, accel });
                }
            }
        });
        let class = model.map_or(Class::Everyday, |m| m.class);
        match s.mice.get(&key) {
            None => ui.note(format!("Follows Other Mice below. Recommended for it: {}.", class.short())),
            Some(t) if t.profile == "custom" => {
                let k = key.clone();
                ui.slider(
                    &format!("mouse-speed-{key}"),
                    t.speed as f32,
                    -1.0..=1.0,
                    |sl| sl.step(0.05).label("Speed").limits("Slow", "Fast"),
                    move |s, v| {
                        if let Some(t) = s.mice.get_mut(&k) {
                            t.speed = v as f64;
                        }
                    },
                );
                let k = key.clone();
                strings(ui, &format!("mouse-accel-{key}"), "Acceleration:", &ACCELS, &t.accel, move |s, v| {
                    if let Some(t) = s.mice.get_mut(&k) {
                        t.accel = v;
                    }
                });
            }
            Some(_) => ui.note(format!("{}.", class.describe())),
        }
        if d.pointing_stick {
            ui.check("Scroll while the middle button is held", s.touchpad.pointing_stick_scroll, |s, v| s.touchpad.pointing_stick_scroll = v);
        }
        for q in model.map_or(&[][..], |m| m.quirks) {
            ui.add(StringView::new(format!("• {}", q.describe())).wrap(true));
        }
        if d.may_sleep {
            problem(ui, "USB power management may put it to sleep, and then it loses the first movement when the hand moves again.".into());
        }
    });
}

/// Which palm model the filter uses on a `kind` of pad (each kind its
/// own), and how the person's did when it was trained.
fn model_status(trained: Option<&olo_protocol::touchpad::PalmModel>, on: bool, kind: olo_palm::Kind) -> String {
    // (Trained on another kind of pad: not this one's.)
    let trained = trained.filter(|m| olo_palm::Kind::of_model(m) == kind);
    match trained {
        _ if !on && kind.sized() => "Off: palms are told apart by their size alone.".into(),
        _ if !on => "Off: palms are what the touchpad says they are.".into(),
        None => {
            let own = kind.base_model();
            format!(
                "In use: Olo's own for a {}, made from {} touches of real hands on one; it caught {:.0}% of palms and took {:.0}% of fingers for palms. Train for Your Palm fits it to yours.",
                kind.name(),
                own.contacts,
                own.palms_caught * 100.0,
                own.fingers_lost * 100.0
            )
        }
        Some(m) => {
            let when = jiff::Timestamp::from_second(m.trained as i64).ok().map(|t| t.to_zoned(jiff::tz::TimeZone::system()).strftime("%-d %B %Y").to_string()).unwrap_or_default();
            format!(
                "In use: yours, trained {when} on {} touches; on touches kept back it caught {:.0}% of palms and took {:.0}% of fingers for palms.",
                m.contacts,
                m.palms_caught * 100.0,
                m.fingers_lost * 100.0
            )
        }
    }
}
