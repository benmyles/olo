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
