//! Never locked in silence (Ben, 2026-11-15: "it should never just
//! silently fail like this something should tell me that it needs
//! unlocking and let me do it"). While Olo's keyring is on but locked
//! (the password not given, or something went wrong opening it), a
//! notification says so, with Unlock (shown again each time it stays
//! locked), until it's open. And anyone can ask
//! for it to be opened, over the session bus:
//!
//! ```text
//! busctl --user call org.olo.Secrets1 /org/olo/Secrets1 org.olo.Secrets1 Unlock b true
//! ```
//!
//! `Unlock(asked_by_person)` asks for the login password (Olo's
//! prompter) if the keyring is locked and answers whether it's open.
//! Passwords' Unlock button and the notification's ask as the person
//! (true): always asked. A program's request for a secret, through Olo's
//! stand-in for the keyring (`activated.rs`), asks as a program (false):
//! asked unless the person said Not Now in the last two minutes, so a
//! program asking again and again doesn't bring the prompt back each
//! time. An agent can read whether it's locked in the status
//! (`olo-secrets --status`: `locked`) and ask; the person types the
//! password.

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU32, Ordering},
        mpsc,
    },
    time::Duration,
};

use zbus::zvariant::Value;

/// What olo-secrets' loop is told.
pub enum Msg {
    /// Olo's keyring turned on or off (Preferences ▸ Security).
    Wanted(bool),
    /// Open it, if it's locked; whether it's open is sent on `reply`.
    Unlock { asked_by_person: bool, reply: Option<mpsc::Sender<bool>> },
}

pub const NAME: &str = "org.olo.Secrets1";
const PATH: &str = "/org/olo/Secrets1";

/// How long after a Not Now a program's request doesn't ask again.
pub const QUIET_AFTER_NOT_NOW: Duration = Duration::from_secs(120);

struct Unlocker {
    tx: Mutex<mpsc::Sender<Msg>>,
}

#[zbus::interface(name = "org.olo.Secrets1")]
impl Unlocker {
    /// Open Olo's keyring if it's locked, asking for the login password;
    /// whether it's open. Waits as long as the person takes.
    async fn unlock(&self, asked_by_person: bool) -> bool {
        let (reply, answer) = mpsc::channel();
        if self.tx.lock().unwrap_or_else(|e| e.into_inner()).send(Msg::Unlock { asked_by_person, reply: Some(reply) }).is_err() {
            return false;
        }
        blocking::unblock(move || answer.recv().unwrap_or(false)).await
    }
}

/// Answer `Unlock` on the session bus (its own connection, kept for the
/// life of the process). The next version takes the name over.
pub fn serve(tx: mpsc::Sender<Msg>) {
    let served = (|| -> zbus::Result<zbus::blocking::Connection> {
        let conn = zbus::blocking::connection::Builder::session()?.serve_at(PATH, Unlocker { tx: Mutex::new(tx) })?.build()?;
        conn.request_name_with_flags(NAME, zbus::fdo::RequestNameFlags::ReplaceExisting | zbus::fdo::RequestNameFlags::AllowReplacement)?;
        Ok(conn)
    })();
    match served {
        Ok(conn) => std::mem::forget(conn),
        Err(e) => tracing::warn!("secrets: couldn't offer {NAME} on the bus: {e}"),
    }
}

/// Ask the session's olo-secrets to open the keyring (the stand-in, as a
/// program asked): whether it's open. Waits as long as the person takes.
pub fn ask_to_unlock(conn: &zbus::blocking::Connection, asked_by_person: bool) -> zbus::Result<bool> {
    let p = zbus::blocking::Proxy::new(conn, NAME, PATH, NAME)?;
    p.call("Unlock", &(asked_by_person,))
}

/// The notification that says it's locked.
pub struct Note {
    conn: Option<zbus::blocking::Connection>,
    /// The service's id for it while it's shown (0: not shown).
    id: Arc<AtomicU32>,
}

const NOTIFICATIONS: &str = "org.freedesktop.Notifications";
const NOTIFICATIONS_PATH: &str = "/org/freedesktop/Notifications";

impl Note {
    /// Ready to show it; its Unlock sends an `Unlock` as the person on `tx`.
    pub fn new(tx: mpsc::Sender<Msg>) -> Note {
        let conn = zbus::blocking::Connection::session().ok();
        let id = Arc::new(AtomicU32::new(0));
        if let Some(conn) = conn.clone() {
            let ours = id.clone();
            std::thread::Builder::new()
                .name("note actions".into())
                .spawn(move || {
                    let Ok(p) = zbus::blocking::Proxy::new(&conn, NOTIFICATIONS, NOTIFICATIONS_PATH, NOTIFICATIONS) else { return };
                    let Ok(actions) = p.receive_signal("ActionInvoked") else { return };
                    for m in actions {
                        let Ok((id, key)) = m.body().deserialize::<(u32, String)>() else { continue };
                        // (Only ours: the signal may be everyone's.)
                        let shown = ours.load(Ordering::Relaxed);
                        if shown != 0 && id == shown && (key == "unlock" || key == "default") && tx.send(Msg::Unlock { asked_by_person: true, reply: None }).is_err() {
                            return;
                        }
                    }
                })
                .ok();
        }
        Note { conn, id }
    }

    /// Show (or update) it: locked, and why if it's something typing the
    /// password won't mend.
    pub fn show(&mut self, why: &str) {
        let Some(conn) = &self.conn else { return };
        let body = if why == crate::NOT_GIVEN {
            "Programs can’t read their sign-ins, keys and Wi-Fi passwords until you unlock them with your login password.".to_owned()
        } else {
            format!("They couldn’t be opened: {why}.")
        };
        let mut hints: HashMap<&str, Value<'_>> = HashMap::new();
        hints.insert(olo_protocol::notifications::hint::TYPE, Value::from("important"));
        // (Not resident: Unlock closes it, as the prompt comes up where it
        // was; still locked after, it's shown again.)
        hints.insert("urgency", Value::from(2u8));
        let actions = vec!["default", "", "unlock", "Unlock…"];
        let sent: zbus::Result<u32> = zbus::blocking::Proxy::new(conn, NOTIFICATIONS, NOTIFICATIONS_PATH, NOTIFICATIONS)
            .and_then(|p| p.call("Notify", &("Programs’ Secrets", self.id.load(Ordering::Relaxed), "key", "Programs’ secrets are locked", olo_kit_base::notify::escape(&body), actions, hints, 0i32)));
        match sent {
            Ok(id) => self.id.store(id, Ordering::Relaxed),
            Err(e) => tracing::warn!("secrets: couldn't say the keyring is locked: {e}"),
        }
    }

    /// Take it away (opened, or turned off).
    pub fn close(&mut self) {
        let (Some(conn), id) = (&self.conn, self.id.swap(0, Ordering::Relaxed)) else { return };
        if id != 0 {
            let _ = zbus::blocking::Proxy::new(conn, NOTIFICATIONS, NOTIFICATIONS_PATH, NOTIFICATIONS).and_then(|p| p.call::<_, _, ()>("CloseNotification", &(id,)));
        }
    }
}
