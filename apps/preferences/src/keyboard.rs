//! Keyboard: layout, the keyboard profile (Mac, Windows or BeOS keys:
//! the command key, Caps Lock as Ctrl, a tap of the Super key for the
//! palette, and every shortcut, in the Shortcuts tab, shortcuts.rs), key
//! repeat, hints, the backlight. A platform's profile changed becomes
//! "Mac (Customized)" by itself; Save As keeps one under a name of its
//! own, to pick again.

use olo_kit::{
    egui::{self, Ui},
    metrics as m,
    widgets::{BBox, Button, CheckBox, MenuField, Slider, StringView, TabView, TextControl},
};
use olo_protocol::{CommandKey, KeyboardProfile, Platform, PowerStatus, Settings};

use crate::server::Server;

#[derive(Default)]
pub struct State {
    test: String,
    /// The level being dragged to, until app_server's report catches up.
    backlight: Option<(f32, std::time::Instant)>,
    tab: usize,
    shortcuts: crate::shortcuts::State,
}

#[cfg(test)]
impl State {
    /// On `tab` (for the layout lint).
    pub fn showing(tab: usize) -> State {
        State { tab, ..Default::default() }
    }
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

    BBox::new("Keyboard Hints").show(ui, |ui| {
        ui.add(CheckBox::new(&mut k.hints, "Press Control twice for keyboard hints"));
        let mut speed = k.double_press_ms as f32;
        let text = format!("{speed:.0} ms");
        ui.add_enabled(
            k.hints,
            Slider::new(&mut speed, 150.0..=800.0)
                .step(25.0)
                .label("Double-press speed")
                .value_text(text)
                .limits("Fast", "Slow"),
        );
        k.double_press_ms = speed as u32;
        ui.add(
            StringView::new("A letter goes on everything you can pick: controls, menu items, windows, Deskbar. Type one to pick it, or to letter what’s inside; Backspace goes back, Esc puts them away.")
                .dim(true)
                .wrap(true),
        );
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

/// The profile, at the top of Shortcuts: which (Save As… keeps it under
/// a name of its own), its command key and its keys' ways. `saving` is
/// the name being typed for Save As.
pub fn profile(ui: &mut Ui, s: &mut Settings, saving: &mut Option<String>) {
    let k = &mut s.keyboard;
    BBox::new("Keyboard Profile").show(ui, |ui| {
        let saved = !Platform::ALL.iter().any(|p| p.name() == k.profile.name);
        match saving {
            None => {
                let choices = k.choices();
                let names: Vec<&str> = choices.iter().map(|p| p.name.as_str()).collect();
                let mut i = names.iter().position(|n| *n == k.profile.name).unwrap_or(0);
                ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), m::CONTROL_HEIGHT), egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add_enabled(saved, Button::new("Delete")).clicked() {
                        let name = k.profile.name.clone();
                        k.profiles.retain(|p| p.name != name);
                        // Back to its platform's, as it comes.
                        k.profile = KeyboardProfile::builtin(k.profile.platform);
                    }
                    if ui.add(Button::new("Save As…")).clicked() {
                        *saving = Some(if saved && !k.profile.name.ends_with("(Customized)") { k.profile.name.clone() } else { String::new() });
                    }
                    let w = ui.available_width();
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.add(MenuField::new("profile", &mut i, &names).label("Profile:").divider(DIVIDER).width(w));
                    });
                });
                if let Some(name) = names.get(i)
                    && *name != k.profile.name
                {
                    k.pick_profile(name);
                }
            }
            Some(name) => {
                let mut done = false;
                ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), m::CONTROL_HEIGHT), egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add(Button::new("Cancel")).clicked() {
                        done = true;
                    }
                    let taken = Platform::ALL.iter().any(|p| p.name() == name.trim() || KeyboardProfile::customized_name(*p) == name.trim());
                    let ok = !name.trim().is_empty() && !taken;
                    let save = ui.add_enabled(ok, Button::new("Save").default(true)).clicked();
                    let w = ui.available_width();
                    let enter = ui
                        .with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            let r = ui.add(TextControl::new(name).label("Name:").divider(DIVIDER).width(w).hint("My keys").autofocus(true));
                            r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))
                        })
                        .inner;
                    if ok && (save || enter) {
                        k.profile.name = name.trim().to_owned();
                        k.save_profile();
                        done = true;
                    }
                });
                if done {
                    *saving = None;
                }
            }
        }
        let before = k.profile.clone();
        let p = &mut k.profile;
        let titles: Vec<&str> = CommandKey::ALL.iter().map(|c| c.title()).collect();
        let mut c = CommandKey::ALL.iter().position(|c| *c == p.command_key).unwrap_or(0);
        let w = ui.available_width();
        ui.add(MenuField::new("command-key", &mut c, &titles).label("Command key:").divider(DIVIDER).width(w));
        p.command_key = CommandKey::ALL[c];
        ui.add(CheckBox::new(&mut p.caps_lock_control, "Caps Lock is another Control key"));
        let super_key = if p.platform == Platform::Windows { "the Windows key" } else { "⌘ (the Super key)" };
        ui.add(CheckBox::new(&mut p.palette_on_tap, &format!("Tap {super_key} on its own for the command palette")));
        if k.profile != before {
            k.profile_changed();
        }
    });
}

/// Where the profile box's fields begin.
const DIVIDER: f32 = 96.0;
