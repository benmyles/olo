//! Keyboard: layout, key repeat, the Mac keys (⌘ as the command key,
//! Caps Lock as Ctrl, tap ⌘ for Find), the backlight; and the Shortcuts
//! tab (shortcuts.rs).

use olo_kit::{
    egui::Ui,
    metrics as m,
    widgets::{BBox, CheckBox, MenuField, Slider, StringView, TabView, TextControl},
};
use olo_protocol::{PowerStatus, Settings};

use crate::server::Server;

#[derive(Default)]
pub struct State {
    test: String,
    /// The level being dragged to, until app_server's report catches up.
    backlight: Option<(f32, std::time::Instant)>,
    tab: usize,
    shortcuts: crate::shortcuts::State,
}

/// Common layouts: xkb name and what to call it.
const LAYOUTS: &[(&str, &str)] = &[
    ("us", "English (US)"),
    ("gb", "English (UK)"),
    ("au", "English (Australia)"),
    ("ca", "French (Canada)"),
    ("de", "German"),
    ("fr", "French"),
    ("es", "Spanish"),
    ("it", "Italian"),
    ("pt", "Portuguese"),
    ("br", "Portuguese (Brazil)"),
    ("nl", "Dutch"),
    ("se", "Swedish"),
    ("no", "Norwegian"),
    ("dk", "Danish"),
    ("fi", "Finnish"),
    ("ch", "Swiss"),
    ("pl", "Polish"),
    ("cz", "Czech"),
    ("ru", "Russian"),
    ("jp", "Japanese"),
];

pub fn ui(ui: &mut Ui, s: &mut Settings, power: Option<&PowerStatus>, server: &Server, st: &mut State) {
    let mut tab = st.tab;
    TabView::new(&mut tab, &["Keyboard", "Shortcuts"]).show(ui, |ui, tab| {
        let inner = ui.available_rect_before_wrap().shrink(m::SPACING);
        ui.scope_builder(olo_kit::egui::UiBuilder::new().max_rect(inner).layout(olo_kit::egui::Layout::top_down(olo_kit::egui::Align::Min)), |ui| {
            ui.spacing_mut().item_spacing.y = m::SPACING;
            match tab {
                0 => keyboard(ui, s, power, server, st),
                _ => crate::shortcuts::ui(ui, s, server, &mut st.shortcuts),
            }
        });
    });
    if tab != st.tab {
        st.tab = tab;
    }
}

fn keyboard(ui: &mut Ui, s: &mut Settings, power: Option<&PowerStatus>, server: &Server, st: &mut State) {
    let k = &mut s.keyboard;
    let mut names: Vec<&str> = LAYOUTS.iter().map(|(_, n)| *n).collect();
    let mut index = LAYOUTS.iter().position(|(id, _)| *id == k.layout);
    if index.is_none() {
        // Something set elsewhere: show it as is.
        names.push(&k.layout);
        index = Some(names.len() - 1);
    }
    let mut i = index.unwrap_or(0);
    ui.add(MenuField::new("layout", &mut i, &names).label("Layout:").divider(60.0));
    if let Some((id, _)) = LAYOUTS.get(i)
        && *id != k.layout
    {
        k.layout = id.to_string();
        k.variant.clear();
    }

    BBox::new("Key Repeat").show(ui, |ui| {
        let mut rate = k.repeat_rate_hz as f32;
        let text = format!("{rate:.0} per second");
        ui.add(
            Slider::new(&mut rate, 5.0..=60.0).step(1.0).label("Repeat rate").value_text(text).limits("Slow", "Fast"),
        );
        k.repeat_rate_hz = rate as i32;
        let mut delay = k.repeat_delay_ms as f32;
        let text = format!("{delay:.0} ms");
        ui.add(
            Slider::new(&mut delay, 150.0..=1000.0)
                .step(25.0)
                .label("Delay until repeat")
                .value_text(text)
                .limits("Short", "Long"),
        );
        k.repeat_delay_ms = delay as i32;
        ui.add(TextControl::new(&mut st.test).label("Typing test:").hint("Type here to try the settings"));
    });

    ui.add(CheckBox::new(&mut k.num_lock, "Num Lock on when Olo starts"));

    BBox::new("Mac Keys").show(ui, |ui| {
        ui.add(CheckBox::new(&mut k.command_key, "Use Command as the shortcut key (Command-C copies)"));
        ui.add(CheckBox::new(&mut k.caps_lock_control, "Caps Lock is another Control key"));
        ui.add_enabled(k.command_key, CheckBox::new(&mut k.find_on_command_tap, "Tap Command on its own for the command palette"));
    });

    BBox::new("Backlight").show(ui, |ui| match power.and_then(|p| p.keyboard_backlight) {
        Some(level) => {
            if st.backlight.is_some_and(|(_, at)| at.elapsed() > std::time::Duration::from_millis(600)) {
                st.backlight = None;
            }
            let mut v = st.backlight.map_or(level, |(l, _)| l);
            let before = v;
            let text = if v == 0.0 { "Off".to_owned() } else { format!("{:.0}%", v * 100.0) };
            ui.add(Slider::new(&mut v, 0.0..=1.0).step(1.0 / 64.0).label("Brightness").value_text(text).limits("Off", "Bright"));
            if v != before {
                st.backlight = Some((v, std::time::Instant::now()));
                server.set_keyboard_backlight(v);
            }
        }
        None => {
            ui.add(StringView::new("This keyboard has no backlight.").dim(true));
        }
    });
}
