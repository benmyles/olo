//! The Background portal (`org.freedesktop.impl.portal.Background`): a
//! sandboxed app (a Flatpak's Telegram or Discord) asking to go on running
//! with no window, or to start at login. xdg-desktop-portal asks the
//! person through the Access portal (Olo's permission dialog) and then
//! here: to write or remove the app's login item (`~/.config/autostart`,
//! which olo-session starts at login), and which apps are running and
//! which is in front (from app_server's windows), so it can tell which run
//! with none. One found running in the background is allowed to: Olo
//! doesn't stop programs for keeping on.

use std::collections::HashMap;

use zbus::zvariant::{OwnedObjectPath, OwnedValue};

/// `org.freedesktop.impl.portal.Background`.
pub struct Background;

/// An app's state, as the portal counts it.
const RUNNING: u32 = 1;
const ACTIVE: u32 = 2;

/// Each app with windows: running, or active if one has the focus.
fn states() -> HashMap<String, u32> {
    use olo_protocol::{Client, Reply, Request};
    let mut out = HashMap::new();
    let Ok(Reply::Windows { windows }) = Client::connect().and_then(|mut c| c.request(&Request::ListWindows)) else { return out };
    for w in windows.iter().filter(|w| !w.app_id.is_empty()) {
        let state = if w.focused { ACTIVE } else { RUNNING };
        let s = out.entry(w.app_id.clone()).or_insert(state);
        *s = (*s).max(state);
    }
    out
}

/// One word of an Exec line, quoted as the desktop entry spec says when
/// it must be.
fn exec_word(w: &str) -> String {
    if !w.is_empty() && !w.contains(|c: char| c.is_whitespace() || "\"'\\><~|&;$*?#()`".contains(c)) {
        return w.replace('%', "%%");
    }
    let mut out = String::from("\"");
    for c in w.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out.replace('%', "%%")
}

/// The login item for `app_id` running `commandline` (flag 1: the app is
/// started by D-Bus activation).
pub fn login_item(app_id: &str, commandline: &[String], flags: u32) -> String {
    let exec: Vec<String> = commandline.iter().map(|w| exec_word(w)).collect();
    let mut text = format!(
        "[Desktop Entry]\nType=Application\nName={app_id}\nExec={}\nX-Flatpak={app_id}\n# Written by Olo's Background portal at the app's asking.\n",
        exec.join(" ")
    );
    if flags & 1 != 0 {
        text.push_str("DBusActivatable=true\n");
    }
    text
}

fn autostart_dir() -> Option<std::path::PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME").map(std::path::PathBuf::from).or_else(|| Some(std::path::PathBuf::from(std::env::var_os("HOME")?).join(".config")))?;
    Some(config.join("autostart"))
}

#[zbus::interface(name = "org.freedesktop.impl.portal.Background")]
impl Background {
    async fn get_app_state(&self) -> HashMap<String, OwnedValue> {
        let states = blocking::unblock(states).await;
        states.into_iter().map(|(app, s)| (app, OwnedValue::from(s))).collect()
    }

    /// An app runs with no window: let it.
    async fn notify_background(&self, _handle: OwnedObjectPath, app_id: String, name: String) -> (u32, HashMap<String, OwnedValue>) {
        tracing::info!("portal: {name} ({app_id}) runs in the background");
        (0, HashMap::from([("result".to_owned(), OwnedValue::from(1u32))]))
    }

    async fn enable_autostart(&self, app_id: String, enable: bool, commandline: Vec<String>, flags: u32) -> bool {
        // An id is a reverse-DNS name: nothing that walks out of the folder.
        if app_id.is_empty() || app_id.contains('/') || app_id.starts_with('.') {
            return false;
        }
        blocking::unblock(move || {
            let Some(dir) = autostart_dir() else { return false };
            let path = dir.join(format!("{app_id}.desktop"));
            if !enable {
                return std::fs::remove_file(&path).is_ok() || !path.exists();
            }
            let tmp = dir.join(format!(".{app_id}.desktop.tmp"));
            std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&tmp, login_item(&app_id, &commandline, flags))).and_then(|_| std::fs::rename(&tmp, &path)).is_ok()
        })
        .await
    }

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_items() {
        let t = login_item("org.telegram.desktop", &["flatpak".into(), "run".into(), "--command=telegram-desktop".into(), "org.telegram.desktop".into(), "-autostart".into()], 0);
        assert!(t.contains("\nExec=flatpak run --command=telegram-desktop org.telegram.desktop -autostart\n"));
        assert!(t.contains("\nX-Flatpak=org.telegram.desktop\n") && !t.contains("DBusActivatable"));
        assert_eq!(exec_word("/opt/My App/run"), "\"/opt/My App/run\"");
        assert_eq!(exec_word("say \"hi\" $HOME"), "\"say \\\"hi\\\" \\$HOME\"");
        assert_eq!(exec_word("100%"), "100%%");
        assert!(login_item("a.b", &["a".into()], 1).contains("DBusActivatable=true"));
        // What olo-session's login items start from it: the words back.
        let e = olo_xdg::desktop_entry::DesktopEntry { exec: Some(format!("{} --x", exec_word("/opt/My App/run"))), ..Default::default() };
        assert_eq!(olo_xdg::open::launch_line(&e).unwrap(), ["/opt/My App/run", "--x"]);
    }
}
