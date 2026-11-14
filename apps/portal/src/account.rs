//! The Account portal (`org.freedesktop.impl.portal.Account`): a program
//! asking who you are (for a profile, "signed in as", a comment's
//! author). Asked in Olo's permission dialog (`access.rs`), naming the
//! program, what it would get and its reason; your user name, full name
//! and picture are shared only if you say so.

use std::{collections::HashMap, ffi::CStr, path::PathBuf};

use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

use crate::portal::ToUi;

/// `org.freedesktop.impl.portal.Account`.
pub struct Account {
    pub to_ui: std::sync::Arc<dyn Fn(ToUi) + Send + Sync>,
}

/// Who is signed in: the user name and the full name (GECOS's first
/// field, else the user name).
pub fn who() -> (String, String) {
    // SAFETY: getpwuid returns static storage (or null); copied out at once.
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if pw.is_null() {
            return (String::new(), String::new());
        }
        let user = CStr::from_ptr((*pw).pw_name).to_string_lossy().into_owned();
        let gecos = if (*pw).pw_gecos.is_null() { String::new() } else { CStr::from_ptr((*pw).pw_gecos).to_string_lossy().into_owned() };
        let full = gecos.split(',').next().unwrap_or("").trim().to_owned();
        let full = if full.is_empty() { user.clone() } else { full };
        (user, full)
    }
}

/// Their picture, where the login screens keep it: AccountsService's,
/// else `~/.face`.
pub fn picture(user: &str) -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    [Some(PathBuf::from("/var/lib/AccountsService/icons").join(user)), home.map(|h| h.join(".face"))].into_iter().flatten().find(|p| std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.len() > 0))
}

/// The question, as the dialog asks it.
pub fn question(app: &str, full: &str, user: &str, picture: bool) -> (String, String) {
    let app = if app.is_empty() { "this program" } else { app };
    let what = if picture { "your name and picture" } else { "your name" };
    let title = format!("Share {what} with {app}?");
    let subtitle = if full == user { full.to_owned() } else { format!("{full} ({user})") };
    (title, subtitle)
}

fn string(options: &HashMap<String, OwnedValue>, key: &str) -> String {
    options.get(key).and_then(|v| String::try_from(v.try_clone().ok()?).ok()).unwrap_or_default()
}

fn value(v: impl Into<Value<'static>>) -> OwnedValue {
    OwnedValue::try_from(v.into()).expect("no file descriptors")
}

#[zbus::interface(name = "org.freedesktop.impl.portal.Account")]
impl Account {
    async fn get_user_information(
        &self,
        handle: OwnedObjectPath,
        app_id: String,
        parent_window: String,
        options: HashMap<String, OwnedValue>,
        #[zbus(object_server)] server: &zbus::ObjectServer,
    ) -> (u32, HashMap<String, OwnedValue>) {
        let (user, full) = who();
        let pic = picture(&user);
        let (title, subtitle) = question(&crate::app_name(&app_id), &full, &user, pic.is_some());
        let (tx, rx) = async_channel::bounded(1);
        let request = crate::portal::RequestObject { handle: handle.to_string(), to_ui: self.to_ui.clone() };
        let _ = server.at(handle.as_ref(), request).await;
        (self.to_ui)(ToUi::Access(crate::access::Ask {
            request: handle.to_string(),
            parent: parent_window,
            title,
            subtitle,
            body: string(&options, "reason"),
            deny: "Don't Share".into(),
            grant: "Share".into(),
            icon: pic.as_ref().map_or_else(|| "people".to_owned(), |p| p.display().to_string()),
            choices: Vec::new(),
            reply: tx,
        }));
        let answer = rx.recv().await.ok().flatten();
        let _ = server.remove::<crate::portal::RequestObject, _>(handle.as_ref()).await;
        if answer.is_none() {
            return (1, HashMap::new());
        }
        let image = pic.map(|p| olo_kit::files::file_uri(&p)).unwrap_or_default();
        (0, HashMap::from([("id".to_owned(), value(user)), ("name".to_owned(), value(full)), ("image".to_owned(), value(image))]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asks_by_name() {
        assert_eq!(question("Slack", "Ann Lee", "ann", true), ("Share your name and picture with Slack?".to_owned(), "Ann Lee (ann)".to_owned()));
        assert_eq!(question("", "ann", "ann", false), ("Share your name with this program?".to_owned(), "ann".to_owned()));
        let (user, full) = who();
        assert!(!user.is_empty() && !full.is_empty());
    }
}
