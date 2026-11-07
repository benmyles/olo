//! The layout lint of every panel (`cargo ui-lint`; HACKING, "The layout
//! lint"): each drawn headlessly at its window's size, light and dark at
//! 1×, 1.5× and 2×, with the longest things real machines show (made-up
//! long display and device names), and nothing may run past its area,
//! sit over another control, or be found by the layout review. No
//! app_server and no devices: the panels' links answer with their test
//! data (`cfg(test)`).

use olo_protocol::{Mode, OutputInfo, PowerStatus, Settings};
use olo_ui::{
    WindowOptions,
    gpui::AppContext as _,
    lint::{Lint, assert_clean},
};

use crate::{PanelView, Prefs, panel::Panel, server::Server};

/// Two displays with names as long as real ones come.
fn outputs() -> Vec<OutputInfo> {
    let mode = |width, height, refresh| Mode { width, height, refresh };
    let display = |name: &str, make: &str, model: &str, size, modes: Vec<Mode>, scale, position, main, number| OutputInfo {
        name: name.into(),
        key: format!("{make} {model}"),
        make: make.into(),
        model: model.into(),
        physical_size_mm: size,
        current_mode: modes.first().cloned(),
        modes,
        scale,
        position,
        rotation: 0,
        main,
        mirror_of: None,
        brightness: Some(0.62),
        number,
        // (The built-in one's facts: the Display panel's tallest.)
        panel: (number == Some(0)).then(|| olo_protocol::panels::Panel {
            ppi: Some(254.0),
            internal: true,
            technology: olo_protocol::panels::Technology::MiniLed,
            refresh_hz: 120.0,
            text_why: "Shown at 2× or more: text is drawn exactly as designed, as on Retina displays.".into(),
            ..Default::default()
        }),
        // (The built-in one varies its refresh rate, off.)
        vrr: (number == Some(0)).then_some(false),
        // (The built-in one wide and HDR, on: the Colour tab's tallest.)
        color: (number == Some(0)).then(|| olo_protocol::color::DisplayColor {
            primaries: Some(olo_color::Primaries::DISPLAY_P3),
            peak_nits: Some(1600.0),
            full_nits: Some(1000.0),
            pq: true,
            ..Default::default()
        }),
        color_mode: (number == Some(0)).then_some(olo_protocol::color::ColorMode::Accurate),
        hdr: (number == Some(0)).then_some(true),
        sdr_nits: 203.0,
        hdr_needs_boot: false,
        // (The built-in one a touch screen, taking touches.)
        touch: (number == Some(0)).then_some(true),
    };
    vec![
        display(
            "eDP-1",
            "Apple Computer Inc",
            "MacBook Pro Liquid Retina XDR Display (16-inch, 2023)",
            (345, 223),
            vec![mode(3456, 2234, 120_000), mode(3024, 1964, 120_000), mode(1728, 1117, 60_000)],
            2.0,
            (0, 0),
            true,
            Some(0),
        ),
        display(
            "DP-2",
            "Dell Inc.",
            "DELL UltraSharp U4025QW 40\" Curved Thunderbolt Hub Monitor",
            (946, 399),
            vec![mode(5120, 2160, 120_000), mode(3840, 2160, 60_000)],
            1.5,
            (3456, 0),
            false,
            Some(1),
        ),
        // The smallest screen Olo is laid out for: 1280 × 800 points
        // (a 13" laptop at 2×). Every panel's window fits it (`fits`:
        // 736 tall), else it scrolls, which the lint finds.
        display("HDMI-A-1", "Ancor Communications Inc", "ASUS VS228", (476, 268), vec![mode(1280, 800, 60_000)], 1.0, (-1280, 0), false, Some(-1)),
    ]
}

fn prefs() -> Prefs {
    Prefs {
        settings: Some(Settings::default()),
        outputs: outputs(),
        outputs_in: true,
        holds: Vec::new(),
        power: Some(PowerStatus { brightness: Some(0.62), keyboard_backlight: Some(0.4), ..Default::default() }),
        server: Server::detached(),
        windows: Vec::new(),
    }
}

#[test]
fn ui_lint_every_panel() {
    let mut lint = Lint::new();
    let mut found = Vec::new();
    // (AirPods isn't in the launcher's grid, but is a panel all the same.
    // Then again with every box's Reset showing, as when changed.)
    let panels = Panel::ALL.iter().chain(&[Panel::AirPods]);
    for (&panel, every_reset) in panels.clone().map(|p| (p, false)).chain(panels.filter(|p| p.resettable()).map(|p| (p, true))) {
        crate::form::EVERY_RESET.with(|c| c.set(every_reset));
        let (w, h) = panel.size();
        // (As Preferences opens it: following what it shows.)
        let options = WindowOptions::new(panel.title(), w as f32, h as f32).fits_content();
        // Room kept for what the lint doesn't draw.
        match panel {
            Panel::Integrations => lint.keeps_room(240.0, "Telegram's sign-in: a QR code to scan, and the code and password steps"),
            Panel::TouchId => lint.keeps_room(100.0, "a finger being enrolled: what to do, a progress bar and Cancel"),
            _ => &mut lint,
        };
        for f in lint.window(&options, move |window, cx| {
            let prefs = cx.new(|_| prefs());
            cx.new(|cx| PanelView::new(prefs, panel, Settings::default(), window, cx))
        }) {
            let with = if every_reset { " (every Reset showing)" } else { "" };
            found.push(olo_ui::lint::Found { text: format!("{}{with}: {}", panel.title(), f.text), ..f });
        }
    }
    assert_clean("Preferences' panels", &found);
}

/// Display's Colour tab (beside the Panel tab the panel lint sees): a
/// wide HDR display with HDR on, its tallest.
#[test]
fn ui_lint_display_colour() {
    let mut lint = Lint::new();
    let (w, h) = Panel::Display.size();
    let options = WindowOptions::new(Panel::Display.title(), w as f32, h as f32).fits_content();
    let found = lint.window(&options, move |window, cx| {
        let prefs = cx.new(|_| prefs());
        let view = cx.new(|cx| PanelView::new(prefs, Panel::Display, Settings::default(), window, cx));
        view.update(cx, |v, _| {
            if let crate::panel::State::Display(st) = &mut v.state {
                crate::display::colour_tab(st);
            }
        });
        view
    });
    assert_clean("Display's Colour tab", &found);
}

/// Keyboard's Keyboards tab (beside the Keyboard tab the panel lint
/// sees): the made-up keyboards plugged in, the Magic Keyboard with
/// Touch ID among them, known, in its own box with its picture.
#[test]
fn ui_lint_keyboards_tab() {
    let mut lint = Lint::new();
    let (w, h) = Panel::Keyboard.size();
    let options = WindowOptions::new(Panel::Keyboard.title(), w as f32, h as f32).fits_content();
    let found = lint.window(&options, move |window, cx| {
        let prefs = cx.new(|_| prefs());
        let view = cx.new(|cx| PanelView::new(prefs, Panel::Keyboard, Settings::default(), window, cx));
        view.update(cx, |v, _| {
            if let crate::panel::State::Keyboard(st) = &mut v.state {
                crate::keyboard::keyboards_tab(st);
            }
        });
        view
    });
    assert_clean("Keyboard's Keyboards tab", &found);
}

/// Terminal ▸ Command Suggestions in each of its states (Terminal's
/// size is the tallest's).
#[test]
fn ui_lint_command_suggestions() {
    let mut lint = Lint::new();
    let mut found = Vec::new();
    for (v, name) in crate::complete::lint_states().into_iter().enumerate() {
        let (w, h) = Panel::Terminal.size();
        let options = WindowOptions::new(Panel::Terminal.title(), w as f32, h as f32);
        for f in lint.window(&options, move |window, cx| {
            crate::complete::LINT.with(|l| l.set(v));
            let prefs = cx.new(|_| prefs());
            cx.new(|cx| PanelView::new(prefs, Panel::Terminal, Settings::default(), window, cx))
        }) {
            found.push(olo_ui::lint::Found { text: format!("Terminal, {name}: {}", f.text), ..f });
        }
    }
    assert_clean("Terminal's command suggestions", &found);
}

/// Train for Your Palm, full screen: each step in each of its states (a
/// touch judged, the longest miss, done, Esc pressed once; the summary's
/// outcomes) on the screen it's laid out for and on a MacBook's at 2×.
#[test]
fn ui_lint_the_palm_trainer() {
    let mut lint = Lint::new();
    let mut found = Vec::new();
    for (w, h) in [(1280.0, 800.0), (1512.0, 982.0)] {
        for (name, step, v) in crate::palm_trainer::lint_states() {
            let options = WindowOptions::new("Train for Your Palm", w, h).fullscreen();
            for f in lint.window(&options, move |_, cx| {
                let prefs = cx.new(|_| prefs());
                cx.new(|cx| crate::palm_trainer::TrainerWindow::for_lint(prefs, step, v, cx))
            }) {
                found.push(olo_ui::lint::Found { text: format!("{w}×{h}, {name}: {}", f.text), ..f });
            }
        }
    }
    assert_clean("the palm trainer", &found);
}

/// Try Voice Processing (Sound ▸ Voice Processing ▸ Try It…) in each
/// state: nothing recorded, recording, trying, tried.
#[test]
fn ui_lint_try_voice() {
    let mut lint = Lint::new();
    let mut found = Vec::new();
    for (name, phase) in crate::voice_try::lint_states() {
        let options = WindowOptions::new("Try Voice Processing", 760.0, 600.0);
        for f in lint.window(&options, move |_, cx| {
            let prefs = cx.new(|_| prefs());
            cx.new(|cx| crate::voice_try::TryWindow::for_lint(prefs, phase, cx))
        }) {
            found.push(olo_ui::lint::Found { text: format!("{name}: {}", f.text), ..f });
        }
    }
    assert_clean("Try Voice Processing", &found);
}

/// A name from the test data or this computer: a display's ("D0 Built-in
/// Display"), as the Display panel's buttons for them are named.
fn this_machines(name: &str) -> bool {
    let mut c = name.chars();
    c.next() == Some('D') && c.next().is_some_and(|d| d.is_ascii_digit() || d == '−' || d == '-')
}

/// A box's title as every computer has it: a display's box is "Display",
/// a mouse's "This Mouse", Touch ID's user's "Fingerprints".
fn generic_group(group: &str) -> String {
    if this_machines(group) {
        "Display".into()
    } else if group.starts_with("Fingerprints for ") {
        "Fingerprints".into()
    } else if group == "Logitech G309 LIGHTSPEED" {
        "This Mouse".into()
    } else {
        group.to_owned()
    }
}

/// What the palette finds of Preferences: every control of every panel
/// (each of its tabs too), drawn headlessly as it shows, with the box
/// it's in, written to `olo_xdg::settings` (`OLO_WRITE_SETTINGS=1 cargo
/// ui-lint` to write it again). Fails when the panels have a setting it
/// lacks or one they don't, so a new one is found without listing it.
#[test]
fn ui_lint_settings_index() {
    use std::fmt::Write as _;
    const ROLES: &[&str] = &["CheckBox", "RadioButton", "Slider", "ComboBox", "TextInput", "Button", "ColorWell"];
    let mut lint = Lint::new();
    let mut rows: Vec<(String, String, String, String, String)> = Vec::new();
    for &panel in Panel::ALL.iter().filter(|p| **p != Panel::About) {
        let (w, h) = panel.size();
        // (As Preferences opens it: following what it shows.)
        let options = WindowOptions::new(panel.title(), w as f32, h as f32).fits_content();
        let trees = lint.trees(&options, move |window, cx| {
            let prefs = cx.new(|_| prefs());
            cx.new(|cx| PanelView::new(prefs, panel, Settings::default(), window, cx))
        });
        let tabs = trees.len();
        let mut found: Vec<(String, String, String, String)> = Vec::new();
        for (tab, nodes) in trees {
            let inside = |n: &olo_ui::commands::UiNode, o: &olo_ui::commands::UiNode| {
                let (cx, cy) = (n.x + n.width / 2.0, n.y + n.height / 2.0);
                o.x <= cx && cx <= o.x + o.width && o.y <= cy && cy <= o.y + o.height
            };
            for n in nodes.iter().filter(|n| ROLES.contains(&n.role.as_str()) && n.role != "Window") {
                let name = n.name.as_deref().unwrap_or("").trim().trim_end_matches(':').trim().to_owned();
                // (A list's rows are what's in it, not settings: a device's
                // buttons, a made-up one's name.)
                if name.is_empty() || nodes.iter().any(|o| matches!(o.role.as_str(), "Row" | "ListItem") && inside(n, o)) {
                    continue;
                }
                // Not settings: a scroll bar's arrows, every panel's
                // Defaults and Revert, a state shown as a button, a
                // problem's fix (there only while it is), the made-up
                // displays' buttons.
                if (n.role == "Button" && (name.starts_with("Scroll ") || matches!(name.as_str(), "Defaults" | "Revert" | "Searching…" | "Fix in Olo Doctor…"))) || this_machines(&name) {
                    continue;
                }
                // The innermost box round it, by a name that is the same
                // on every computer (not the made-up device's or the user's).
                let group =
                    nodes.iter().filter(|o| o.role == "Group" && inside(n, o)).min_by(|a, b| (a.width * a.height).total_cmp(&(b.width * b.height))).and_then(|o| o.name.clone()).unwrap_or_default();
                // (Default Apps' own kinds are the person's, as a list's
                // rows are: a made-up one's.)
                if group == "Your Kinds" {
                    continue;
                }
                let group = generic_group(&group);
                // (Each keyboard's check box, by the made-up keyboards'
                // names: one, as every computer has it.)
                let name = if group == "Swap Alt and the Windows Key" { "This Keyboard".to_owned() } else { name };
                let row = (tab.clone().unwrap_or_default(), group, name, n.role.clone());
                if !found.contains(&row) {
                    found.push(row);
                }
            }
        }
        // A control in every tab is in none (beside tabs, as Display's
        // settings beside its Panel and Colour).
        let everywhere = |r: &(String, String, String, String)| tabs > 1 && found.iter().filter(|o| (&o.1, &o.2, &o.3) == (&r.1, &r.2, &r.3)).count() == tabs;
        for r in &found {
            let tab = if everywhere(r) { String::new() } else { r.0.clone() };
            let row = (panel.arg().to_owned(), tab, r.1.clone(), r.2.clone(), r.3.clone());
            if !rows.contains(&row) {
                rows.push(row);
            }
        }
    }
    let q = |s: &str| format!("{s:?}");
    let mut out = String::from(
        "//! Every setting in Preferences' panels, for the command palette:\n//! made by Preferences' `ui_lint_settings_index` test from the panels as\n//! they're drawn (`OLO_WRITE_SETTINGS=1 cargo ui-lint` writes it again).\n//! Not edited by hand.\n\n/// A control in a panel: the panel's argument (`apps::PREFERENCE_PANELS`),\n/// the tab it's on and the box it's in (empty: none), its name as shown,\n/// and what kind of control it is (AccessKit's role).\npub struct Setting {\n    pub panel: &'static str,\n    pub tab: &'static str,\n    pub group: &'static str,\n    pub name: &'static str,\n    pub role: &'static str,\n}\n\npub const PREFERENCE_SETTINGS: &[Setting] = &[\n",
    );
    for (panel, tab, group, name, role) in &rows {
        let _ = writeln!(out, "    Setting {{ panel: {}, tab: {}, group: {}, name: {}, role: {} }},", q(panel), q(tab), q(group), q(name), q(role));
    }
    out.push_str("];\n");
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates/xdg/src/settings.rs");
    let now = std::fs::read_to_string(&path).unwrap_or_default();
    if std::env::var_os("OLO_WRITE_SETTINGS").is_some() {
        std::fs::write(&path, &out).unwrap();
    } else {
        assert!(now == out, "crates/xdg/src/settings.rs isn't the panels' settings as drawn now: OLO_WRITE_SETTINGS=1 cargo ui-lint writes it again");
    }
}
