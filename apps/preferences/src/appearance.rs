//! Appearance: light, dark or automatic theme; night mode (warmer colours
//! in the evening, at once or gradually from its start); the window tab font size and longest title; icons in
//! Deskbar's menu; new windows glowing a moment.

use olo_protocol::{NightLightMode, ThemePreference};
use olo_ui::{gpui::SharedString, metrics as m};

use crate::form::Ui;

pub fn ui(ui: &mut Ui) {
    let a = ui.s.appearance.clone();
    ui.bbox("Theme", |ui| {
        ui.row_gap(m::SPACING * 2.0, |ui| {
            for (pref, label) in [(ThemePreference::Light, "Light"), (ThemePreference::Dark, "Dark"), (ThemePreference::Auto, "Automatic")] {
                ui.radio(label, a.theme == pref, move |s| s.appearance.theme = pref);
            }
        });
    });
    let night = a.night_light.clone();
    ui.bbox("Night Mode", |ui| {
        ui.row_gap(m::SPACING * 2.0, |ui| {
            for (mode, label) in [(NightLightMode::Off, "Off"), (NightLightMode::On, "On"), (NightLightMode::Scheduled, "Scheduled")] {
                ui.radio(label, night.mode == mode, move |s| s.appearance.night_light.mode = mode);
            }
        });
        // The schedule also sets when Automatic turns dark.
        let schedule = night.mode == NightLightMode::Scheduled || a.theme == ThemePreference::Auto;
        ui.enabled_if(schedule, |ui| {
            ui.row(|ui| {
                ui.text("night-start", &night.start, |f| f, |c| c.label("From:").width(110.0), |s, v| s.appearance.night_light.start = v);
                ui.text("night-end", &night.end, |f| f, |c| c.label("To:").width(90.0), |s, v| s.appearance.night_light.end = v);
            });
        });
        // Warming gradually from the start (Ben, 2026-11-08), back to
        // neutral at the end as ever.
        ui.enabled_if(night.mode == NightLightMode::Scheduled, |ui| {
            ui.check("Shift gradually, starting at From", night.gradual, |s, v| s.appearance.night_light.gradual = v);
            ui.enabled_if(night.gradual, |ui| {
                let n = night.gradual_minutes.clamp(1, 180);
                ui.slider(
                    "night-gradual",
                    n as f32,
                    1.0..=180.0,
                    |s| s.step(1.0).label("Shift over").value_text(minutes_text(n)).limits("1 minute", "3 hours"),
                    |s, v| s.appearance.night_light.gradual_minutes = (v.round() as u32).clamp(1, 180),
                );
            });
        });
        ui.enabled_if(night.mode != NightLightMode::Off, |ui| {
            let text = format!("{} K", night.temperature);
            ui.slider(
                "warmth",
                night.temperature as f32,
                2500.0..=6000.0,
                |s| s.step(100.0).label("Warmth").value_text(text).limits("Warmer", "Cooler"),
                |s, k| s.appearance.night_light.temperature = k as u32,
            );
        });
    });
    let size = ui.s.tab_font_size;
    ui.slider("title-size", size, 10.0..=16.0, |s| s.step(1.0).label("Window title size").value_text(format!("{size:.0} pt")), |s, v| s.tab_font_size = v);
    let chars = ui.s.tab_title_chars as f32;
    ui.slider("title-chars", chars, 20.0..=200.0, |s| s.step(10.0).label("Longest window title").value_text(format!("{chars:.0} characters")), |s, v| s.tab_title_chars = v as u32);
    ui.check("Show icons in the Deskbar menu", a.menu_icons, |s, v| s.appearance.menu_icons = v);
    // A new window glows a moment, to catch the eye.
    let choices = [2u32, 3, 5, 8, 10, 15, 30];
    let mut names: Vec<SharedString> = choices.iter().map(|n| format!("{n} seconds").into()).collect();
    let i = match choices.iter().position(|&c| c == a.glow_seconds) {
        Some(i) => i,
        None => {
            names.push(format!("{} seconds", a.glow_seconds).into());
            names.len() - 1
        }
    };
    ui.row(|ui| {
        ui.check("Glow new windows for", a.glow_new_windows, |s, v| s.appearance.glow_new_windows = v);
        ui.enabled_if(a.glow_new_windows, |ui| {
            ui.menu(
                "glow-seconds",
                names,
                i,
                |f| f.width(110.0),
                move |s, i| {
                    if let Some(&c) = choices.get(i) {
                        s.appearance.glow_seconds = c;
                    }
                },
            );
        });
    });
}

/// "45 minutes", "1 hour", "2 hours 30 minutes".
fn minutes_text(n: u32) -> String {
    let plural = |n: u32, one: &str| if n == 1 { format!("1 {one}") } else { format!("{n} {one}s") };
    match (n / 60, n % 60) {
        (0, m) => plural(m, "minute"),
        (h, 0) => plural(h, "hour"),
        (h, m) => format!("{} {}", plural(h, "hour"), plural(m, "minute")),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn minutes_said_plainly() {
        assert_eq!(super::minutes_text(1), "1 minute");
        assert_eq!(super::minutes_text(45), "45 minutes");
        assert_eq!(super::minutes_text(60), "1 hour");
        assert_eq!(super::minutes_text(90), "1 hour 30 minutes");
        assert_eq!(super::minutes_text(180), "3 hours");
    }
}
