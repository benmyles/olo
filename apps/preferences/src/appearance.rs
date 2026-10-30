//! Appearance: light, dark or automatic theme; night mode (warmer colours
//! in the evening); the window tab font size and longest title; icons in
//! Deskbar's menu; new windows glowing a moment.

use olo_kit::{
    egui::Ui,
    metrics as m,
    widgets::{BBox, CheckBox, MenuField, RadioButton, Slider, TextControl},
};
use olo_protocol::{NightLightMode, Settings, ThemePreference};

pub fn ui(ui: &mut Ui, s: &mut Settings) {
    BBox::new("Theme").show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = m::SPACING * 2.0;
            for (pref, label) in [
                (ThemePreference::Light, "Light"),
                (ThemePreference::Dark, "Dark"),
                (ThemePreference::Auto, "Automatic"),
            ] {
                if ui.add(RadioButton::new(s.appearance.theme == pref, label)).clicked() {
                    s.appearance.theme = pref;
                }
            }
        });
    });
    let night = &mut s.appearance.night_light;
    BBox::new("Night Mode").show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = m::SPACING * 2.0;
            for (mode, label) in
                [(NightLightMode::Off, "Off"), (NightLightMode::On, "On"), (NightLightMode::Scheduled, "Scheduled")]
            {
                if ui.add(RadioButton::new(night.mode == mode, label)).clicked() {
                    night.mode = mode;
                }
            }
        });
        // The schedule also sets when Automatic turns dark.
        let schedule = night.mode == NightLightMode::Scheduled || s.appearance.theme == ThemePreference::Auto;
        ui.add_enabled_ui(schedule, |ui| {
            ui.horizontal(|ui| {
                ui.add(TextControl::new(&mut night.start).label("From:").width(110.0));
                ui.add(TextControl::new(&mut night.end).label("To:").width(90.0));
            });
        });
        ui.add_enabled_ui(night.mode != NightLightMode::Off, |ui| {
            let mut k = night.temperature as f32;
            let text = format!("{} K", night.temperature);
            ui.add(
                Slider::new(&mut k, 2500.0..=6000.0)
                    .step(100.0)
                    .label("Warmth")
                    .value_text(text)
                    .limits("Warmer", "Cooler"),
            );
            night.temperature = k as u32;
        });
    });
    let mut size = s.tab_font_size;
    let text = format!("{size:.0} pt");
    ui.add(Slider::new(&mut size, 10.0..=16.0).step(1.0).label("Window title size").value_text(text));
    s.tab_font_size = size;
    let mut chars = s.tab_title_chars as f32;
    let text = format!("{chars:.0} characters");
    ui.add(Slider::new(&mut chars, 20.0..=200.0).step(10.0).label("Longest window title").value_text(text));
    s.tab_title_chars = chars as u32;
    ui.add(CheckBox::new(&mut s.appearance.menu_icons, "Show icons in the Deskbar menu"));
    // A new window glows a moment, to catch the eye.
    let choices = [2u32, 3, 5, 8, 10, 15, 30];
    let names: Vec<String> = choices.iter().map(|n| format!("{n} seconds")).collect();
    let mut names: Vec<&str> = names.iter().map(String::as_str).collect();
    let custom = format!("{} seconds", s.appearance.glow_seconds);
    let mut i = match choices.iter().position(|&c| c == s.appearance.glow_seconds) {
        Some(i) => i,
        None => {
            names.push(&custom);
            names.len() - 1
        }
    };
    ui.horizontal(|ui| {
        ui.add(CheckBox::new(&mut s.appearance.glow_new_windows, "Glow new windows for"));
        ui.add_enabled_ui(s.appearance.glow_new_windows, |ui| ui.add(MenuField::new("glow-seconds", &mut i, &names).width(110.0)));
    });
    if let Some(&c) = choices.get(i) {
        s.appearance.glow_seconds = c;
    }
}
