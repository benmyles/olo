//! Security: how Passwords keeps its secrets (syncing with 1Password,
//! locking, `op` on the command line, the clipboard; Passwords follows
//! these live), and beside them security keys (`security_keys`).

use olo_ui::gpui::SharedString;

use crate::{
    form::{Ui, divider_for},
    power::minutes_menu,
    security_keys,
};

pub fn ui(ui: &mut Ui, keys: &mut security_keys::State) {
    ui.columns(passwords, |ui| security_keys::ui(ui, keys));
}

fn passwords(ui: &mut Ui) {
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
        // (Served from the next login, or as soon as Passwords is opened:
        // nothing started from here, which the layout lint toggles too.)
        ui.check("Be the SSH agent (ssh, git)", p.ssh_agent, |s, v| s.security.passwords.ssh_agent = v);
        if p.ssh_agent {
            ssh_config_note(ui);
        }
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
        ui.menu(
            "pw-clear",
            names,
            i,
            |f| f.label("Clear copied secrets after:").divider(170.0),
            |s, i| {
                if let Some(&c) = CHOICES.get(i) {
                    s.security.passwords.clear_seconds = c;
                }
            },
        );
    });
}

/// Where `~/.ssh/config` sends ssh to another agent (1Password's app's),
/// said, and changed to Passwords' when the person asks: only once this
/// session's programs have Passwords' agent in `SSH_AUTH_SOCK` (olo-session
/// sets it as a login starts), so ssh is never left with none.
fn ssh_config_note(ui: &mut Ui) {
    let path = olo_keychain::ssh_config::path();
    let Some(other) = std::fs::read_to_string(&path).ok().and_then(|t| olo_keychain::ssh_config::other_agent(&t)) else {
        ui.note("ssh, git and other programs sign in with the SSH keys in Passwords, each one allowed the first time it asks.");
        return;
    };
    let ours = std::env::var_os("SSH_AUTH_SOCK").is_some_and(|s| std::path::Path::new(&s) == olo_xdg::ssh_agent_socket());
    ui.note(format!("Your ssh configuration (~/.ssh/config) sends ssh to another agent, {other}, so it doesn’t use the keys in Passwords."));
    if !ours {
        ui.note("From your next login, Passwords can be ssh’s agent here too.");
        return;
    }
    let done = ui.on_click_view(move |_, _, cx| {
        if let Ok(text) = std::fs::read_to_string(&path) {
            let new = olo_keychain::ssh_config::use_session_agent(&text);
            let tmp = path.with_extension("olo-new");
            if std::fs::write(&tmp, new).and_then(|_| std::fs::rename(&tmp, &path)).is_err() {
                let _ = std::fs::remove_file(&tmp);
            }
        }
        cx.notify();
    });
    ui.button("Use Passwords for ssh", done);
}

fn seconds_text(s: u32) -> String {
    match s {
        60 => "1 minute".into(),
        s if s % 60 == 0 => format!("{} minutes", s / 60),
        s => format!("{s} seconds"),
    }
}
