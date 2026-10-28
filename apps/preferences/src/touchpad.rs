//! Touchpad and mouse. The Touchpad tab names the pad app_server found and
//! the tuning profile that suits it; Touch is Olo's palm and hover filter
//! and the click pressure; Gestures is scrolling, momentum, dragging and
//! what two and three fingers do; Mouse is mice. The profile sets how the
//! pad feels (protocol `touchpad.rs`); changing any of that by hand makes
//! it "Custom". See docs/TOUCHPAD.md.

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use olo_kit::{
    egui::{self, Ui},
    metrics as m,
    widgets::{BBox, Button, CheckBox, MenuField, Slider, StringView, TabView},
};
use olo_protocol::{
    Client, Reply, Request, Settings, TouchpadSettings,
    touchpad::{CLICK_PRESSURES, DeviceKind, InputDeviceInfo, PROFILES},
};

/// The pointing devices app_server last reported: None until it has.
type Shared = Arc<Mutex<Option<Vec<InputDeviceInfo>>>>;

#[derive(Default)]
pub struct State {
    tab: usize,
    devices: Option<Shared>,
}

#[cfg(test)]
impl State {
    /// On `tab`, with `devices` already reported (for the layout lint).
    pub fn showing(tab: usize, devices: Vec<InputDeviceInfo>) -> State {
        State { tab, devices: Some(Arc::new(Mutex::new(Some(devices)))) }
    }
}

/// Asks app_server for its devices now and every few seconds (a Magic
/// Trackpad may come and go), until the panel closes.
fn poll(ctx: &egui::Context) -> Shared {
    let devices: Shared = Default::default();
    let (out, ctx) = (Arc::downgrade(&devices), ctx.clone());
    std::thread::Builder::new()
        .name("input-devices".into())
        .spawn(move || {
            while let Some(shared) = out.upgrade() {
                let found = Client::connect().ok().and_then(|mut c| match c.request(&Request::InputDevices) {
                    Ok(Reply::InputDevices { devices }) => Some(devices),
                    _ => None,
                });
                if let Some(found) = found {
                    let mut slot = shared.lock().unwrap();
                    if slot.as_ref() != Some(&found) {
                        *slot = Some(found);
                        ctx.request_repaint();
                    }
                }
                drop(shared);
                std::thread::sleep(Duration::from_secs(3));
            }
        })
        .expect("thread");
    devices
}

pub fn ui(ui: &mut Ui, s: &mut Settings, st: &mut State) {
    let devices = st.devices.get_or_insert_with(|| poll(ui.ctx())).lock().unwrap().clone();
    let pad = devices.as_ref().and_then(|d| d.iter().find(|d| d.kind != DeviceKind::Mouse).cloned());
    let mut tab = st.tab;
    TabView::new(&mut tab, &["Touchpad", "Touch", "Gestures", "Mouse"]).show(ui, |ui, tab| {
        let inner = ui.available_rect_before_wrap().shrink(m::SPACING);
        ui.scope_builder(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::top_down(egui::Align::Min)), |ui| {
            ui.spacing_mut().item_spacing.y = m::SPACING;
            let before = s.touchpad.clone();
            match tab {
                0 => touchpad(ui, &mut s.touchpad, devices.is_some(), pad.as_ref()),
                1 => touch(ui, &mut s.touchpad, pad.as_ref()),
                2 => gestures(ui, &mut s.touchpad),
                _ => mouse(ui, s),
            }
            let t = &mut s.touchpad;
            if *t != before && t.profile == before.profile && t.profile != "custom" && !t.matches_profile(&t.profile) {
                t.profile = "custom".into();
            }
        });
    });
    st.tab = tab;
}

fn profile_name(id: &str) -> &str {
    PROFILES.iter().find(|(p, _, _)| *p == id).map_or(id, |(_, name, _)| name)
}

fn touchpad(ui: &mut Ui, t: &mut TouchpadSettings, asked: bool, pad: Option<&InputDeviceInfo>) {
    BBox::new("This Touchpad").show(ui, |ui| {
        match pad {
            Some(pad) => {
                let size = pad.size_mm.map(|(w, h)| format!(" · {w:.0} × {h:.0} mm")).unwrap_or_default();
                ui.add(StringView::new(&format!("{}{size}", pad.kind.describe())));
                ui.add(StringView::new(&pad.name).dim(true));
            }
            None if asked => {
                ui.add(StringView::new("No touchpad found.").dim(true));
            }
            None => {
                ui.add(StringView::new("Looking…").dim(true));
            }
        }
        let names: Vec<&str> = PROFILES.iter().map(|(_, name, _)| *name).collect();
        let mut i = PROFILES.iter().position(|(id, _, _)| *id == t.profile).unwrap_or(PROFILES.len() - 1);
        ui.add(MenuField::new("profile", &mut i, &names).label("Profile:").divider(70.0));
        if let Some((id, _, about)) = PROFILES.get(i) {
            if *id != t.profile {
                t.apply_profile(id);
            }
            ui.add(StringView::new(about).dim(true));
        }
        if let Some(pad) = pad {
            let recommended = pad.recommended.as_str();
            if t.profile == recommended {
                ui.add(StringView::new("This is the recommended profile for this touchpad.").dim(true));
            } else {
                ui.horizontal(|ui| {
                    if ui.add(Button::new(&format!("Use {}", profile_name(recommended)))).clicked() {
                        t.apply_profile(recommended);
                    }
                    ui.add(StringView::new("recommended for this touchpad").dim(true));
                });
            }
        }
    });
    BBox::new("Basics").show(ui, |ui| {
        let mut speed = t.speed as f32;
        ui.add(Slider::new(&mut speed, -1.0..=1.0).step(0.05).label("Tracking speed").limits("Slow", "Fast"));
        t.speed = speed as f64;
        ui.add(CheckBox::new(&mut t.tap_to_click, "Tap to click"));
        ui.add_enabled(t.tap_to_click, CheckBox::new(&mut t.tap_and_drag, "Tap and drag"));
        ui.add(CheckBox::new(&mut t.natural_scroll, "Natural scrolling (content follows your fingers)"));
        ui.add(CheckBox::new(&mut t.clickfinger, "Click with two fingers for a secondary click"));
    });
}

fn touch(ui: &mut Ui, t: &mut TouchpadSettings, pad: Option<&InputDeviceInfo>) {
    let f = &mut t.filter;
    BBox::new("Palms and Hovering Fingers").show(ui, |ui| {
        ui.add(CheckBox::new(&mut f.enabled, "Ignore hovering fingers and palms"));
        if pad.is_some_and(|p| !p.pressure) {
            ui.add(StringView::new("This touchpad doesn't report pressure, so it is left to libinput.").dim(true).wrap(true));
        }
        ui.add_enabled_ui(f.enabled, |ui| {
            let mut on = f.touch_on as f32;
            let text = format!("{on:.0}");
            ui.add(Slider::new(&mut on, 1.0..=40.0).step(1.0).label("A touch starts at pressure").value_text(text).limits("Any touch", "Firmer"));
            f.touch_on = on as u32;
            let mut palm = f.palm_size as f32;
            let text = format!("{palm:.0}");
            ui.add(Slider::new(&mut palm, 600.0..=2000.0).step(50.0).label("A palm is a contact as large as").value_text(text).limits("Smaller", "Larger"));
            f.palm_size = palm as u32;
            ui.add(CheckBox::new(&mut f.palm_proof_click, "A resting palm can't click (its pressure doesn't count)"));
        });
    });
    BBox::new("Click").show(ui, |ui| {
        let custom = format!("Custom ({}/{})", f.click_press, f.click_release);
        let mut names: Vec<&str> = CLICK_PRESSURES.iter().map(|(n, _, _)| *n).collect();
        let mut i = match CLICK_PRESSURES.iter().position(|(_, p, r)| *p == f.click_press && *r == f.click_release) {
            Some(i) => i,
            None => {
                names.push(&custom);
                names.len() - 1
            }
        };
        ui.add_enabled(f.enabled && f.palm_proof_click, MenuField::new("click-pressure", &mut i, &names).label("Click pressure:").divider(110.0));
        if let Some((_, press, release)) = CLICK_PRESSURES.get(i) {
            (f.click_press, f.click_release) = (*press, *release);
        }
        if let Some((press, release)) = pad.and_then(|p| p.driver_click) {
            let note = format!("The pad clicks at {press}/{release} (all contacts together); with a palm on it, Olo counts the click only if the fingers press that hard alone.");
            ui.add(StringView::new(&note).dim(true).wrap(true));
        }
    });
    BBox::new("While Typing").show(ui, |ui| {
        ui.add(CheckBox::new(&mut t.disable_while_typing, "Ignore the touchpad while typing"));
        menu(ui, "dwt-timeout", "For:", &[(0, "libinput's default"), (300, "0.3 seconds"), (500, "0.5 seconds"), (1000, "1 second"), (2000, "2 seconds")], &mut t.dwt_timeout_ms, t.disable_while_typing);
        ui.add(CheckBox::new(&mut t.dwtp, "Ignore the touchpad while the pointing stick is used"));
        ui.add(CheckBox::new(&mut t.off_with_mouse, "Turn the touchpad off while a mouse is plugged in"));
    });
}

fn gestures(ui: &mut Ui, t: &mut TouchpadSettings) {
    BBox::new("Scrolling").show(ui, |ui| {
        let mut speed = t.scroll_speed as f32;
        let text = format!("{speed:.2}×");
        ui.add(Slider::new(&mut speed, 0.25..=3.0).step(0.05).label("Scrolling speed").value_text(text).limits("Slow", "Fast"));
        t.scroll_speed = speed as f64;
        ui.add(CheckBox::new(&mut t.momentum, "Keep scrolling after a flick"));
        let mut glide = t.glide as f32;
        let text = format!("{glide:.2}×");
        ui.add_enabled(t.momentum, Slider::new(&mut glide, 0.25..=3.0).step(0.05).label("Glide").value_text(text).limits("Short", "Long"));
        t.glide = glide as f64;
        let mut method = t.scroll_method.clone();
        menu(ui, "scroll-method", "Scroll with:", &[("two-finger".into(), "Two fingers"), ("edge".into(), "The right edge")], &mut method, true);
        t.scroll_method = method;
    });
    BBox::new("Clicking and Dragging").show(ui, |ui| {
        let mut map = t.tap_button_map.clone();
        menu(ui, "button-map", "Two fingers:", &[("lrm".into(), "Secondary click (three: middle)"), ("lmr".into(), "Middle click (three: secondary)")], &mut map, true);
        if map != t.tap_button_map {
            t.clickfinger_button_map = map.clone();
            t.tap_button_map = map;
        }
        menu(ui, "three-finger-drag", "Drag with:", &[(0u8, "Click and hold"), (3, "Three fingers"), (4, "Four fingers")], &mut t.three_finger_drag, true);
        let mut lock = t.drag_lock.clone();
        menu(ui, "drag-lock", "After tap and drag:", &[("off".into(), "Drop on lift"), ("timeout".into(), "Keep briefly"), ("sticky".into(), "Keep until tapped")], &mut lock, t.tap_to_click && t.tap_and_drag);
        t.drag_lock = lock;
        let mut accel = t.accel_profile.clone();
        menu(ui, "accel", "Acceleration:", &[("adaptive".into(), "Adaptive"), ("flat".into(), "None (flat)")], &mut accel, true);
        t.accel_profile = accel;
    });
}

fn mouse(ui: &mut Ui, s: &mut Settings) {
    BBox::new("Mouse").show(ui, |ui| {
        let mut speed = s.mouse_speed as f32;
        ui.add(Slider::new(&mut speed, -1.0..=1.0).step(0.05).label("Mouse speed").limits("Slow", "Fast"));
        s.mouse_speed = speed as f64;
        let t = &mut s.touchpad;
        let mut accel = t.mouse_accel_profile.clone();
        menu(ui, "mouse-accel", "Acceleration:", &[("adaptive".into(), "Adaptive"), ("flat".into(), "None (flat)")], &mut accel, true);
        t.mouse_accel_profile = accel;
        ui.add(CheckBox::new(&mut t.left_handed, "Left-handed (swap the buttons)"));
        ui.add(CheckBox::new(&mut t.middle_emulation, "Press both buttons for a middle click"));
    });
    BBox::new("Focus").show(ui, |ui| {
        ui.add(CheckBox::new(&mut s.focus_follows_mouse, "Focus follows the pointer"));
    });
}

/// A menu of `choices` (value, name) for `value`; a value not among them
/// is shown as is.
fn menu<T: PartialEq + Clone + std::fmt::Debug>(ui: &mut Ui, id: &str, label: &str, choices: &[(T, &str)], value: &mut T, enabled: bool) {
    let other = format!("{value:?}");
    let mut names: Vec<&str> = choices.iter().map(|(_, n)| *n).collect();
    let mut i = match choices.iter().position(|(v, _)| v == value) {
        Some(i) => i,
        None => {
            names.push(&other);
            names.len() - 1
        }
    };
    ui.add_enabled(enabled, MenuField::new(id, &mut i, &names).label(label).divider(130.0));
    if let Some((v, _)) = choices.get(i) {
        *value = v.clone();
    }
}
