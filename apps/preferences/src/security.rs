//! Security: how Passwords keeps its secrets (syncing with 1Password,
//! locking, the clipboard). Passwords follows these live.

use tabula_kit::{
    egui::Ui,
    widgets::{BBox, CheckBox, MenuField, RadioButton},
};
use tabula_protocol::Settings;

use crate::power::minutes_menu;

pub fn ui(ui: &mut Ui, s: &mut Settings) {
    let p = &mut s.security.passwords;
    BBox::new("Passwords: Syncing with 1Password").show(ui, |ui| {
        if ui.add(RadioButton::new(p.auto_sync, "Automatically")).clicked() {
            p.auto_sync = true;
        }
        if ui.add(RadioButton::new(!p.auto_sync, "Only when I choose Sync Now")).clicked() {
            p.auto_sync = false;
        }
    });
    BBox::new("Passwords: Locking").show(ui, |ui| {
        minutes_menu(ui, "pw-lock", "Lock after idle for:", &[1, 5, 10, 30, 60, 0], &mut p.lock_minutes);
        ui.add(CheckBox::new(&mut p.lock_with_screen, "Lock when the screen locks"));
    });
    BBox::new("Passwords: Clipboard").show(ui, |ui| {
        let choices = [30, 60, 90, 180, 300];
        let names: Vec<String> = choices.iter().map(|&s| seconds_text(s)).collect();
        let mut names: Vec<&str> = names.iter().map(String::as_str).collect();
        let custom = seconds_text(p.clear_seconds);
        let mut i = match choices.iter().position(|&c| c == p.clear_seconds) {
            Some(i) => i,
            None => {
                names.push(&custom);
                names.len() - 1
            }
        };
        ui.add(MenuField::new("pw-clear", &mut i, &names).label("Clear copied secrets after:").divider(170.0));
        if let Some(&c) = choices.get(i) {
            p.clear_seconds = c;
        }
    });
}

fn seconds_text(s: u32) -> String {
    match s {
        60 => "1 minute".into(),
        s if s % 60 == 0 => format!("{} minutes", s / 60),
        s => format!("{s} seconds"),
    }
}
