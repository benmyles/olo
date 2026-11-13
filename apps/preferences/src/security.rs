//! Security, in two tabs. Sign In: every way in besides the password,
//! together (Ben, 2026-11-08: "centralize the settings for all of these
//! under prefs -> security"): Touch ID's fingers (`touchid`), security keys
//! (`security_keys`) and Face Unlock (`face_unlock`), each standing for
//! the password wherever it's asked, all at once. Passwords: how Passwords
//! keeps its secrets (syncing with 1Password, locking, `op` on the command
//! line, the clipboard; Passwords follows these live) and whether Olo keeps
//! programs' secrets (olo-secrets, its state followed from the file it
//! writes).

use std::sync::{Arc, OnceLock};

use olo_kit::SharedString;
use olo_sync::Mutex;
use serde::Deserialize;

use olo_kit::{Cx, ParentElement as _, Styled as _, div, metrics as m, px, widgets::TabView};

use crate::{
    PanelView,
    form::{Ui, divider_for},
    panel,
    power::minutes_menu,
    security_keys, touchid,
};

/// The panel's tabs.
pub const TABS: [&str; 2] = ["Sign In", "Passwords"];

pub struct State {
    tab: usize,
    /// Security keys', with Face Unlock's.
    pub keys: security_keys::State,
    pub touchid: touchid::State,
}

impl State {
    pub fn new(cx: &mut Cx<PanelView>) -> State {
        State { tab: 0, keys: security_keys::State::new(cx), touchid: touchid::State::new(cx) }
    }
}

/// The tab named `tab` shown (`--tab`, and the names that opened Touch ID
/// and Passwords' settings before: `Panel::tab_of`).
pub fn show_tab(st: &mut State, tab: &str) {
    if let Some(i) = TABS.iter().position(|t| t.eq_ignore_ascii_case(tab)) {
        st.tab = i;
    }
}

pub fn ui(ui: &mut Ui, st: &mut State) {
    let items = ui.collect(|ui| match st.tab {
        0 => sign_in(ui, st),
        _ => ui.columns(passwords, passwords_more),
    });
    let pick = ui.on_view(|v, i: &usize, cx| {
        if let panel::State::Security(st) = &mut v.state {
            st.tab = *i;
        }
        cx.notify();
    });
    let tabs: Vec<SharedString> = TABS.into_iter().map(SharedString::from).collect();
    ui.add(
        div()
            .flex_1()
            .flex()
            .flex_col()
            .child(TabView::new("security-tabs", tabs, st.tab).grows().on_select(pick).child(div().flex_1().flex().flex_col().gap(px(m::SPACING)).p(px(m::SPACING)).children(items))),
    );
}

/// Every way in besides the password, side by side.
fn sign_in(ui: &mut Ui, st: &mut State) {
    ui.note("Each of these set up stands for your password wherever it's asked (the lock screen, Passwords, sudo in Terminal, administrator requests), all at once: whichever you use first.");
    let left = ui.collect(|ui| {
        touchid::ui(ui, &mut st.touchid);
        crate::face_unlock::ui(ui, &mut st.keys.face);
    });
    let right = ui.collect(|ui| security_keys::ui(ui, &mut st.keys));
    ui.add_columns(left, right);
}

fn passwords(ui: &mut Ui) {
    let p = ui.s.security.passwords.clone();
    ui.bbox("Passwords: Syncing with 1Password", |ui| {
        ui.radio("Automatically", p.auto_sync, |s| s.security.passwords.auto_sync = true);
        ui.radio("Only when I choose Sync Now", !p.auto_sync, |s| s.security.passwords.auto_sync = false);
    });
    ui.bbox("Passwords: Locking", |ui| {
        let label = "Lock after idle for:";
        let divider = divider_for(&[label]);
        minutes_menu(ui, "pw-lock", label, divider, &[1, 5, 10, 30, 60, 0], p.lock_minutes, |s, m| s.security.passwords.lock_minutes = m);
        ui.check("Lock when the screen locks", p.lock_with_screen, |s, v| s.security.passwords.lock_with_screen = v);
    });
    clipboard(ui, &p);
}

/// Passwords' tab's second column.
fn passwords_more(ui: &mut Ui) {
    let p = ui.s.security.passwords.clone();
    ui.bbox("Passwords: Command line", |ui| {
        ui.check("Answer 1Password’s op command", p.op_cli, |s, v| s.security.passwords.op_cli = v);
        // (Served from the next login, or as soon as Passwords is opened:
        // nothing started from here, which the layout lint toggles too.)
        ui.check("Be the SSH agent (ssh, git)", p.ssh_agent, |s, v| s.security.passwords.ssh_agent = v);
        if p.ssh_agent {
            ssh_config_note(ui);
        }
    });
    ui.bbox("Programs’ Secrets", |ui| {
        ui.check("Keep programs’ secrets in Olo", p.secret_service, |s, v| s.security.passwords.secret_service = v);
        secrets_note(ui, p.secret_service);
    });
}

fn clipboard(ui: &mut Ui, p: &olo_protocol::PasswordsSecurity) {
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
    let (path, other, ours) = ssh_setup();
    let Some(other) = other else {
        ui.note("ssh, git and other programs sign in with the SSH keys in Passwords, each one allowed the first time it asks.");
        return;
    };
    ui.note(format!("Your ssh configuration (~/.ssh/config) sends ssh to another agent, {other}, so it doesn’t use the keys in Passwords."));
    if !ours {
        ui.note("From your next login, Passwords can be ssh’s agent here too.");
        return;
    }
    let done = ui.on_click_view(move |_, cx| {
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

/// `~/.ssh/config`, the other agent it sends ssh to (if any), and whether
/// this session's programs have Passwords' agent.
#[cfg(not(test))]
fn ssh_setup() -> (std::path::PathBuf, Option<String>, bool) {
    let path = olo_keychain::ssh_config::path();
    let other = std::fs::read_to_string(&path).ok().and_then(|t| olo_keychain::ssh_config::other_agent(&t));
    let ours = std::env::var_os("SSH_AUTH_SOCK").is_some_and(|s| std::path::Path::new(&s) == olo_xdg::ssh_agent_socket());
    (path, other, ours)
}

/// (The lint's, whoever runs it: never the person's own ssh setup. The
/// panel's tallest, with its button, as on a machine that used
/// 1Password's agent; a config nowhere, should the button be pressed.)
#[cfg(test)]
fn ssh_setup() -> (std::path::PathBuf, Option<String>, bool) {
    ("/nonexistent/.ssh/config".into(), Some("~/.1password/agent.sock".into()), true)
}

/// What olo-secrets says it's doing (`$XDG_RUNTIME_DIR/olo-secrets.json`).
#[derive(Debug, Default, Clone, PartialEq, Deserialize)]
#[serde(default)]
struct SecretsStatus {
    /// Its file is there: olo-secrets runs.
    #[serde(skip)]
    running: bool,
    serving: bool,
    collections: Vec<SecretsCollection>,
    programs: Vec<String>,
    skipped: Vec<String>,
    error: Option<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Deserialize)]
#[serde(default)]
struct SecretsCollection {
    items: usize,
}

/// olo-secrets' state, followed from its file (one watch for the app).
fn secrets_status(proxy: olo_kit::Proxy) -> SecretsStatus {
    static SHARED: OnceLock<Arc<Mutex<SecretsStatus>>> = OnceLock::new();
    let shared = SHARED.get_or_init(|| {
        let shared: Arc<Mutex<SecretsStatus>> = Arc::default();
        // Tests (the layout lint): Olo keeping a long list's worth.
        if cfg!(test) {
            *shared.lock().unwrap() = SecretsStatus {
                running: true,
                serving: true,
                collections: vec![SecretsCollection { items: 137 }],
                programs: ["chrome", "Slack", "org.gnome.NetworkManager", "olo", "evolution-data-server", "discord"].map(String::from).to_vec(),
                skipped: vec!["Work Keyring".into()],
                error: None,
            };
            return shared;
        }
        let path = olo_xdg::runtime_dir().join("olo-secrets.json");
        let read = {
            let (shared, path) = (shared.clone(), path.clone());
            move |_: &[std::path::PathBuf]| {
                let now: SecretsStatus = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).map(|s| SecretsStatus { running: true, ..s }).unwrap_or_default();
                let mut s = shared.lock().unwrap();
                if *s != now {
                    *s = now;
                    drop(s);
                    proxy.send(crate::Msg::Redraw);
                }
            }
        };
        read(&[]);
        if let Some(w) = olo_kit::watch::watch([path], read) {
            // The app's, for as long as it runs.
            std::mem::forget(w);
        }
        shared
    });
    shared.lock().unwrap().clone()
}

/// What keeping programs' secrets means, and how it's going.
fn secrets_note(ui: &mut Ui, on: bool) {
    if !on {
        ui.note("Chrome, Slack and other programs keep their secrets (the keys to their saved passwords and sign-ins) in GNOME Keyring. Turned on, Olo keeps them instead, GNOME Keyring’s copied over first; turned off, what changed goes back.");
        return;
    }
    let st = secrets_status(olo_kit::proxy());
    if !st.running {
        ui.note("Olo’s keyring service isn’t running here; it starts with your session.");
        return;
    }
    if let Some(e) = &st.error {
        ui.note(format!("Olo doesn’t keep them yet: {e}."));
        return;
    }
    if !st.serving {
        ui.note("Copying GNOME Keyring’s secrets…");
        return;
    }
    let n: usize = st.collections.iter().map(|c| c.items).sum();
    let secrets = if n == 1 { "1 secret".to_owned() } else { format!("{n} secrets") };
    let programs = match st.programs.len() {
        0 => String::new(),
        k if k <= 3 => format!(", of {}", st.programs.join(", ")),
        k => format!(", of {} and {} more", st.programs[..3].join(", "), k - 3),
    };
    ui.note(format!("Olo keeps {secrets}{programs}, in place of GNOME Keyring."));
    if !st.skipped.is_empty() {
        ui.note(format!("Left in GNOME Keyring, locked: {}.", st.skipped.join(", ")));
    }
}

fn seconds_text(s: u32) -> String {
    match s {
        60 => "1 minute".into(),
        s if s % 60 == 0 => format!("{} minutes", s / 60),
        s => format!("{s} seconds"),
    }
}
