//! The layout lint over every panel (`olo_kit::layout_lint`): each panel
//! and each of its tabs at its window's size, light and dark, at 1×, 1.5×
//! and 2×, showing the longest things it may have to (made-up devices,
//! networks, displays). `cargo test ui_lint` runs it.

use olo_kit::{egui::vec2, layout_lint};
use olo_protocol::{
    Mode, OutputInfo, PowerStatus, Settings,
    touchpad::{DeviceKind, InputDeviceInfo},
};
use olo_system::battery::{Battery, State as Charge};

use crate::{
    Ctx, about, datetime, keyboard, network, notifications,
    panel::{self, Panel, State},
    power, screensaver,
    server::Server,
    touchpad,
};

/// What `panel` gets wrong in `state` with `settings`, as "Panel (how):
/// problem" lines.
fn lint(panel: Panel, how: &str, state: State, settings: Settings) -> Vec<String> {
    run(panel, how, state, settings, |size, view| layout_lint::check(size, view))
}

/// `panel` in `state` with `settings` through `checker` (the lint, or the
/// layout review), its lines as "Panel (how): line".
fn run(panel: Panel, how: &str, mut state: State, mut settings: Settings, checker: impl FnOnce(olo_kit::egui::Vec2, &mut dyn FnMut(&mut olo_kit::egui::Ui)) -> Vec<String>) -> Vec<String> {
    let server = Server::detached();
    let opened = settings.clone();
    let outputs = outputs();
    let power = PowerStatus { brightness: Some(0.6), keyboard_backlight: Some(0.4), ..Default::default() };
    let (w, h) = panel.size();
    checker(vec2(w as f32, h as f32), &mut |ui| {
        let ctx = Ctx { settings: &mut settings, outputs: &outputs, power: Some(&power), opened: &opened, server: &server };
        panel::ui(panel, &mut state, ui, ctx);
    })
    .into_iter()
    .map(|p| format!("{} ({how}): {p}", panel.title()))
    .collect()
}

#[track_caller]
fn assert_none(problems: Vec<String>) {
    assert!(problems.is_empty(), "the layout lint found:\n  - {}", problems.join("\n  - "));
}

/// A built-in display and an external one with a long name.
fn outputs() -> Vec<OutputInfo> {
    let mode = |width, height| Mode { width, height, refresh: 60_000 };
    let output = |name: &str, model: &str, size, main| OutputInfo {
        name: name.into(),
        key: name.into(),
        make: "Apple".into(),
        model: model.into(),
        physical_size_mm: size,
        modes: vec![mode(3456, 2234), mode(2560, 1600), mode(1920, 1200)],
        current_mode: Some(mode(3456, 2234)),
        scale: 2.0,
        position: (0, 0),
        rotation: 0,
        main,
        mirror_of: None,
        brightness: Some(0.7),
    };
    vec![output("eDP-1", "", (345, 223), true), output("DP-2", "Studio Display XDR Pro (27-inch, 5K Retina)", (600, 340), false)]
}

/// A pad of `kind` called `name`.
fn pad(kind: DeviceKind, name: &str, pressure: bool, driver_click: Option<(u32, u32)>) -> InputDeviceInfo {
    InputDeviceInfo { name: name.into(), kind, size_mm: Some((160.0, 115.0)), pressure, recommended: kind.recommended().into(), driver_click }
}

#[test]
fn ui_lint_touchpad() {
    let pads = [
        // Both of Touch's notes: no pressure, and the Apple driver's
        // click thresholds.
        ("MacBook pad, no pressure", vec![pad(DeviceKind::AppleMtp, "Apple MTP multi-touch", false, Some((1500, 1000)))]),
        ("MacBook pad", vec![pad(DeviceKind::AppleMtp, "Apple MTP multi-touch", true, Some((1500, 1000)))]),
        ("other pad", vec![pad(DeviceKind::Touchpad, "SYNA30B4:00 06CB:CE7E Touchpad with a Very Long Kernel Name", true, None)]),
        ("no pad", Vec::new()),
    ];
    let mut problems = Vec::new();
    for (name, devices) in &pads {
        for tab in 0..4 {
            // With the pad's own profile (a note) and another (a button).
            for (profile, which) in [(devices.first().map_or("default", |d| d.recommended.as_str()), "its profile"), ("custom", "custom")] {
                let mut settings = Settings::default();
                settings.touchpad.profile = profile.into();
                let state = State::Touchpad(touchpad::State::showing(tab, devices.clone()));
                problems.extend(lint(Panel::Touchpad, &format!("{name}, {which}, tab {}", tab + 1), state, settings));
            }
        }
    }
    problems.dedup();
    assert_none(problems);
}

/// Every panel and tab, as `ui_lint_panels` shows them.
fn every_state() -> Vec<(Panel, String, State)> {
    let mut out = Vec::new();
    for &panel in Panel::ALL {
        let states: Vec<(String, State)> = match panel {
            Panel::Touchpad => continue,
            Panel::Keyboard => (0..2).map(|t| (format!("tab {}", t + 1), State::Keyboard(keyboard::State::showing(t)))).collect(),
            Panel::ScreenSaver => (0..2).map(|t| (format!("tab {}", t + 1), State::ScreenSaver(screensaver::State::showing(t)))).collect(),
            Panel::DateTime => (0..3).map(|t| (format!("tab {}", t + 1), State::DateTime(Box::new(datetime::State::showing(t))))).collect(),
            Panel::Network => (0..2).map(|t| (format!("tab {}", t + 1), State::Network(Box::new(network::State::showing(t))))).collect(),
            Panel::About => vec![("facts".into(), State::About(about::State::sample()))],
            Panel::Notifications => vec![("apps".into(), State::Notifications(Box::new(notifications::State::sample())))],
            Panel::Power => {
                let battery = Battery {
                    percent: 100,
                    state: Charge::NotCharging,
                    time_to_empty: None,
                    time_to_full: None,
                    cycle_count: Some(1234),
                    health: Some(87),
                    charge_limit: Some(80),
                    power_watts: Some(-12.5),
                };
                vec![
                    ("battery".into(), State::Power(power::State::showing(Some(battery)))),
                    ("no battery".into(), State::Power(power::State::showing(None))),
                ]
            }
            _ => vec![("as it opens".into(), State::new(panel))],
        };
        out.extend(states.into_iter().map(|(how, state)| (panel, how, state)));
    }
    out
}

#[test]
fn ui_lint_panels() {
    let mut problems = Vec::new();
    for (panel, how, state) in every_state() {
        problems.extend(lint(panel, &how, state, Settings::default()));
    }
    assert_none(problems);
}

/// The layout review of every panel (errors fail `ui_lint`): what almost
/// lines up, bands of nearly one height, text off-centre. Each is a place
/// to look at closely; `cargo test -p olo-preferences ui_review --
/// --nocapture` prints them.
#[test]
fn ui_review_panels() {
    let mut lines = Vec::new();
    for (panel, how, state) in every_state() {
        lines.extend(run(panel, &how, state, Settings::default(), |size, view| layout_lint::review(size, view)));
    }
    println!("layout review, {} findings:", lines.len());
    for l in &lines {
        println!("  - {l}");
    }
}
