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
        panel: None,
    };
    vec![
        display("eDP-1", "Apple Computer Inc", "MacBook Pro Liquid Retina XDR Display (16-inch, 2023)", (345, 223), vec![mode(3456, 2234, 120_000), mode(3024, 1964, 120_000), mode(1728, 1117, 60_000)], 2.0, (0, 0), true, Some(0)),
        display("DP-2", "Dell Inc.", "DELL UltraSharp U4025QW 40\" Curved Thunderbolt Hub Monitor", (946, 399), vec![mode(5120, 2160, 120_000), mode(3840, 2160, 60_000)], 1.5, (3456, 0), false, Some(1)),
    ]
}

fn prefs() -> Prefs {
    Prefs {
        settings: Some(Settings::default()),
        outputs: outputs(),
        power: Some(PowerStatus { brightness: Some(0.62), keyboard_backlight: Some(0.4), ..Default::default() }),
        server: Server::detached(),
        windows: Vec::new(),
    }
}

#[test]
fn ui_lint_every_panel() {
    let mut lint = Lint::new();
    let mut found = Vec::new();
    for &panel in Panel::ALL {
        let (w, h) = panel.size();
        let options = WindowOptions::new(panel.title(), w as f32, h as f32);
        for f in lint.window(&options, move |window, cx| {
            let prefs = cx.new(|_| prefs());
            cx.new(|cx| PanelView::new(prefs, panel, Settings::default(), window, cx))
        }) {
            found.push(olo_ui::lint::Found { text: format!("{}: {}", panel.title(), f.text), ..f });
        }
    }
    assert_clean("Preferences' panels", &found);
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
        let options = WindowOptions::new(panel.title(), w as f32, h as f32);
        for (tab, nodes) in lint.trees(&options, move |window, cx| {
            let prefs = cx.new(|_| prefs());
            cx.new(|cx| PanelView::new(prefs, panel, Settings::default(), window, cx))
        }) {
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
                let group = nodes.iter().filter(|o| o.role == "Group" && inside(n, o)).min_by(|a, b| (a.width * a.height).total_cmp(&(b.width * b.height))).and_then(|o| o.name.clone()).unwrap_or_default();
                let group = generic_group(&group);
                let row = (panel.arg().to_owned(), tab.clone().unwrap_or_default(), group, name, n.role.clone());
                if !rows.contains(&row) {
                    rows.push(row);
                }
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
