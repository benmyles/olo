//! Security: how Passwords keeps its secrets (syncing with 1Password,
//! locking, `op` on the command line, the clipboard). Passwords follows these live.

use olo_ui::gpui::SharedString;

use crate::{
    form::{Ui, divider_for},
    power::minutes_menu,
};

pub fn ui(ui: &mut Ui) {
    let p = ui.s.security.passwords.clone();
    ui.bbox("Passwords: Syncing with 1Password", |ui| {
        ui.radio("Automatically", p.auto_sync, |s| s.security.passwords.auto_sync = true);
        ui.radio("Only when I choose Sync Now", !p.auto_sync, |s| s.security.passwords.auto_sync = false);
    });
    ui.bbox("Passwords: Locking", |ui| {
        let label = "Lock after idle for:";
        let divider = divider_for(ui.window, &[label]);
        minutes_menu(ui, "pw-lock", label, divider, &[1, 5, 10, 30, 60, 0], p.lock_minutes, |s, m| s.security.passwords.lock_minutes = m);
        ui.check("Lock when the screen locks", p.lock_with_screen, |s, v| s.security.passwords.lock_with_screen = v);
    });
    ui.bbox("Passwords: Command line", |ui| {
        ui.check("Answer 1Password’s op command", p.op_cli, |s, v| s.security.passwords.op_cli = v);
    });
    ui.bbox("Passwords: Clipboard", |ui| {
        const CHOICES: [u32; 5] = [30, 60, 90, 180, 300];
        let mut names: Vec<SharedString> = CHOICES.iter().map(|&s| seconds_text(s).into()).collect();
        let i = match CHOICES.iter().position(|&c| c == p.clear_seconds) {
            Some(i) => i,
            None => {
                names.push(seconds_text(p.clear_seconds).into());
                names.len() - 1
            }
        };
        ui.menu("pw-clear", names, i, |f| f.label("Clear copied secrets after:").divider(170.0), |s, i| {
            if let Some(&c) = CHOICES.get(i) {
                s.security.passwords.clear_seconds = c;
            }
        });
    });
}

fn seconds_text(s: u32) -> String {
    match s {
        60 => "1 minute".into(),
        s if s % 60 == 0 => format!("{} minutes", s / 60),
        s => format!("{s} seconds"),
    }
}
