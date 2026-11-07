//! The Notification portal (`org.freedesktop.impl.portal.Notification`):
//! sandboxed programs' notifications, handed to Olo's notification centre
//! (`org.freedesktop.Notifications`, olo-notifications) in place of GTK's
//! backend, which did the same. A program names each of its
//! notifications (`id`); the centre numbers them, so each is kept by both
//! (one sent again with the same id replaces the one shown), and what the
//! person clicks (the notification, or one of its buttons) goes back to
//! the program as the action it gave (`ActionInvoked`).

use std::{collections::HashMap, sync::Arc};

use olo_sync::Mutex;
use zbus::{
    object_server::SignalEmitter,
    zvariant::{OwnedValue, Value},
};

const NOTIFICATIONS: &str = "org.freedesktop.Notifications";
const NOTIFICATIONS_PATH: &str = "/org/freedesktop/Notifications";

/// One notification shown for a program.
struct Shown {
    app_id: String,
    id: String,
    /// The centre's action keys, each with the program's action and its
    /// target.
    actions: HashMap<String, (String, Option<OwnedValue>)>,
}

#[derive(Default)]
struct Kept {
    /// By the centre's number.
    shown: HashMap<u32, Shown>,
}

/// `org.freedesktop.impl.portal.Notification`.
#[derive(Clone, Default)]
pub struct Notification {
    kept: Arc<Mutex<Kept>>,
}

fn text(n: &HashMap<String, OwnedValue>, key: &str) -> String {
    n.get(key).and_then(|v| String::try_from(v.try_clone().ok()?).ok()).unwrap_or_default()
}

/// What the centre is asked: (app icon, summary, body, actions, hints),
/// and the actions' meanings, for program `app_id`'s `notification`.
pub fn translate(app_id: &str, n: &HashMap<String, OwnedValue>) -> (String, String, String, Vec<String>, HashMap<String, OwnedValue>, HashMap<String, (String, Option<OwnedValue>)>) {
    let mut actions = Vec::new();
    let mut meanings = HashMap::new();
    let target = |v: Option<&OwnedValue>| v.and_then(|v| v.try_clone().ok());
    let default = text(n, "default-action");
    if !default.is_empty() {
        actions.extend(["default".to_owned(), String::new()]);
        meanings.insert("default".to_owned(), (default, target(n.get("default-action-target"))));
    }
    if let Some(buttons) = n.get("buttons").and_then(|v| <Vec<HashMap<String, OwnedValue>>>::try_from(v.try_clone().ok()?).ok()) {
        for (i, b) in buttons.iter().enumerate() {
            let (label, action) = (text(b, "label"), text(b, "action"));
            if action.is_empty() {
                continue;
            }
            let key = format!("button-{i}");
            actions.extend([key.clone(), label]);
            meanings.insert(key, (action, target(b.get("target"))));
        }
    }
    let mut hints: HashMap<String, OwnedValue> = HashMap::new();
    if !app_id.is_empty() {
        hints.insert("desktop-entry".into(), OwnedValue::from(zbus::zvariant::Str::from(app_id.to_owned())));
    }
    let urgency: u8 = match text(n, "priority").as_str() {
        "low" => 0,
        "high" | "urgent" => 2,
        _ => 1,
    };
    hints.insert("urgency".into(), OwnedValue::from(urgency));
    // The icon, a serialized GIcon: a themed name, or a picture's bytes
    // (kept as a file the centre reads).
    let mut icon = String::new();
    if let Some(v) = n.get("icon") {
        if let Ok((kind, data)) = <(String, OwnedValue)>::try_from(v.try_clone().unwrap_or_else(|_| OwnedValue::from(0u8))) {
            // (The data is a variant in the structure.)
            let data = match &*data {
                Value::Value(inner) => OwnedValue::try_from((**inner).try_clone().unwrap_or(Value::U8(0))).unwrap_or(data),
                _ => data,
            };
            match kind.as_str() {
                "themed" => {
                    if let Ok(names) = <Vec<String>>::try_from(data) {
                        icon = names.into_iter().next().unwrap_or_default();
                    }
                }
                "bytes" => {
                    if let Ok(bytes) = <Vec<u8>>::try_from(data)
                        && let Some(path) = keep_picture(&bytes)
                    {
                        hints.insert("image-path".into(), OwnedValue::from(zbus::zvariant::Str::from(path)));
                    }
                }
                _ => {}
            }
        }
    }
    (icon, text(n, "title"), text(n, "body"), actions, hints, meanings)
}

/// A notification's picture, kept in the runtime directory by its
/// contents (the same picture sent again is the same file).
fn keep_picture(bytes: &[u8]) -> Option<String> {
    use std::hash::{Hash as _, Hasher as _};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut h);
    let dir = olo_xdg::runtime_dir().join("olo-portal-pictures");
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(format!("{:016x}", h.finish()));
    if !path.exists() {
        std::fs::write(&path, bytes).ok()?;
    }
    Some(path.display().to_string())
}

#[zbus::interface(name = "org.freedesktop.impl.portal.Notification")]
impl Notification {
    async fn add_notification(&self, app_id: String, id: String, notification: HashMap<String, OwnedValue>, #[zbus(connection)] conn: &zbus::Connection) -> zbus::fdo::Result<()> {
        let (icon, summary, body, actions, hints, meanings) = translate(&app_id, &notification);
        let replaces = self.kept.lock().unwrap().shown.iter().find(|(_, s)| s.app_id == app_id && s.id == id).map_or(0, |(n, _)| *n);
        let name = crate::app_name(&app_id);
        let reply = conn.call_method(Some(NOTIFICATIONS), NOTIFICATIONS_PATH, Some(NOTIFICATIONS), "Notify", &(name, replaces, icon, summary, body, actions, hints, -1i32)).await?;
        let number: u32 = reply.body().deserialize()?;
        let mut kept = self.kept.lock().unwrap();
        kept.shown.remove(&replaces);
        kept.shown.insert(number, Shown { app_id, id, actions: meanings });
        Ok(())
    }

    async fn remove_notification(&self, app_id: String, id: String, #[zbus(connection)] conn: &zbus::Connection) -> zbus::fdo::Result<()> {
        let number = {
            let mut kept = self.kept.lock().unwrap();
            let n = kept.shown.iter().find(|(_, s)| s.app_id == app_id && s.id == id).map(|(n, _)| *n);
            if let Some(n) = n {
                kept.shown.remove(&n);
            }
            n
        };
        if let Some(n) = number {
            conn.call_method(Some(NOTIFICATIONS), NOTIFICATIONS_PATH, Some(NOTIFICATIONS), "CloseNotification", &(n,)).await?;
        }
        Ok(())
    }

    #[zbus(signal)]
    async fn action_invoked(emitter: &SignalEmitter<'_>, app_id: &str, id: &str, action: &str, parameter: Vec<Value<'_>>) -> zbus::Result<()>;

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        1
    }
}

/// Pass what the person does with the centre's notifications back to
/// the programs (on `conn`, the portal's bus): an action as the program
/// named it, and forget a notification once it's closed.
pub fn follow(conn: zbus::blocking::Connection, portal: Notification) {
    std::thread::Builder::new()
        .name("notifications".into())
        .spawn(move || {
            let Ok(proxy) = zbus::blocking::Proxy::new(&conn, NOTIFICATIONS, NOTIFICATIONS_PATH, NOTIFICATIONS) else { return };
            let (Ok(invoked), Ok(closed)) = (proxy.receive_signal("ActionInvoked"), proxy.receive_signal("NotificationClosed")) else { return };
            let kept = portal.kept.clone();
            std::thread::spawn(move || {
                for msg in closed {
                    if let Ok((n, _reason)) = msg.body().deserialize::<(u32, u32)>() {
                        kept.lock().unwrap().shown.remove(&n);
                    }
                }
            });
            for msg in invoked {
                let Ok((n, key)) = msg.body().deserialize::<(u32, String)>() else { continue };
                let found = {
                    let kept = portal.kept.lock().unwrap();
                    kept.shown.get(&n).and_then(|s| {
                        let (action, target) = s.actions.get(&key)?;
                        Some((s.app_id.clone(), s.id.clone(), action.clone(), target.as_ref().and_then(|t| t.try_clone().ok())))
                    })
                };
                let Some((app_id, id, action, target)) = found else { continue };
                let Ok(iface) = conn.object_server().interface::<_, Notification>(crate::portal::PATH) else { continue };
                let parameter: Vec<Value> = target.into_iter().map(Value::from).collect();
                let _ = zbus::block_on(Notification::action_invoked(iface.signal_emitter(), &app_id, &id, &action, parameter));
            }
        })
        .ok();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_program_s_notification_as_the_centre_takes_it() {
        let v = |x: Value<'_>| OwnedValue::try_from(x).unwrap();
        let mut n = HashMap::new();
        n.insert("title".to_owned(), v(Value::from("Build finished")));
        n.insert("body".to_owned(), v(Value::from("olo: 0 errors")));
        n.insert("priority".to_owned(), v(Value::from("high")));
        n.insert("default-action".to_owned(), v(Value::from("app.show")));
        n.insert("default-action-target".to_owned(), v(Value::from("build-7")));
        let button = |label: &str, action: &str| {
            let mut b: HashMap<String, Value> = HashMap::new();
            b.insert("label".into(), Value::from(label.to_owned()));
            b.insert("action".into(), Value::from(action.to_owned()));
            b
        };
        n.insert("buttons".to_owned(), v(Value::from(vec![button("Open", "app.open"), button("Dismiss", "app.dismiss")])));
        n.insert("icon".to_owned(), v(Value::from(("themed".to_owned(), Value::from(vec!["dialog-information".to_owned()])))));
        let (icon, summary, body, actions, hints, meanings) = translate("org.example.Builder", &n);
        assert_eq!((icon.as_str(), summary.as_str(), body.as_str()), ("dialog-information", "Build finished", "olo: 0 errors"));
        assert_eq!(actions, ["default", "", "button-0", "Open", "button-1", "Dismiss"]);
        assert_eq!(u8::try_from(hints["urgency"].try_clone().unwrap()).unwrap(), 2);
        assert_eq!(String::try_from(hints["desktop-entry"].try_clone().unwrap()).unwrap(), "org.example.Builder");
        assert_eq!(meanings["default"].0, "app.show");
        assert_eq!(String::try_from(meanings["default"].1.as_ref().unwrap().try_clone().unwrap()).unwrap(), "build-7");
        assert_eq!(meanings["button-1"].0, "app.dismiss");
    }
}
