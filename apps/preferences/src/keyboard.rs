//! Keyboard: layout, key repeat, and the Mac keys (⌘ as the command key,
//! Caps Lock as Ctrl, tap ⌘ for Find).

use tabula_kit::{
    egui::Ui,
    widgets::{BBox, CheckBox, MenuField, Slider, TextControl},
};
use tabula_protocol::Settings;

#[derive(Default)]
pub struct State {
    test: String,
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

pub fn ui(ui: &mut Ui, s: &mut Settings, st: &mut State) {
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

    BBox::new("Mac Keys").show(ui, |ui| {
        ui.add(CheckBox::new(&mut k.command_key, "Use Command as the shortcut key (Command-C copies)"));
        ui.add(CheckBox::new(&mut k.caps_lock_control, "Caps Lock is another Control key"));
        ui.add_enabled(k.command_key, CheckBox::new(&mut k.tap_command_find, "Tap Command on its own to open Find"));
    });
}
