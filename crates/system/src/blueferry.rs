//! BlueFerry's backend (github.com/erikwb/blueferry): an iPhone's
//! messages, contacts and notifications over Bluetooth, on the session
//! bus as `io.weirdware.BlueFerry`. This is Olo's side of its `Messages1`
//! and `Events1` interfaces, written from its published contract
//! (`data/io.weirdware.BlueFerry.xml`, `docs/en/developer/dbus-api.md`):
//! commands and snapshots, most of them JSON in a string, and signals
//! that say only that something changed (fetch again). The backend is
//! its makers' program (GPL, Python), installed by Olo Doctor; nothing of
//! it is in Olo, which only talks to it.
//!
//! Every call blocks on the bus (and some on the phone): call them off
//! the UI thread. What it returns is read leniently, as the backend's own
//! clients read it: a key of another type, or missing, is as if unset, and
//! keys Olo doesn't know are passed over, so a newer backend still reads.
//!
//! `OLO_PHONE_BUS=ADDRESS` points it at another bus (tests: `blueferry-fake`).

use serde_json::Value;
use zbus::blocking::Connection;

use crate::Result;

pub const NAME: &str = "io.weirdware.BlueFerry";
const PATH: &str = "/io/weirdware/BlueFerry";
const MESSAGES: &str = "io.weirdware.BlueFerry.Messages1";
const EVENTS: &str = "io.weirdware.BlueFerry.Events1";
/// Its expected errors' names begin with this ("…Error.NotReady").
const ERRORS: &str = "io.weirdware.BlueFerry.Error.";

/// The backend's D-Bus activation file: there once it's installed (the
/// bus starts it by this when it's asked for).
pub const ACTIVATION: &str = "/usr/share/dbus-1/services/io.weirdware.BlueFerry.service";

/// Whether the backend is installed here (its D-Bus activation file, or
/// its program).
pub fn installed() -> bool {
    [ACTIVATION, "/usr/bin/blueferry", "/usr/local/bin/blueferry"].iter().any(|p| std::path::Path::new(p).exists())
}

/// The backend's release as its package says it ("0.9.1-1.fc45"), if
/// it's installed from one.
pub fn installed_release() -> Option<String> {
    std::fs::read_to_string("/usr/share/blueferry/package-release").ok().map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())
}

fn bus() -> Result<Connection> {
    let builder = match std::env::var("OLO_PHONE_BUS") {
        Ok(address) if !address.is_empty() => zbus::blocking::connection::Builder::address(address.as_str())?,
        _ => zbus::blocking::connection::Builder::session()?,
    };
    // (Sending waits on the phone: the backend answers within its own
    // limits, well inside this.)
    builder.method_timeout(std::time::Duration::from_secs(60)).build()
}

/// What `GetStatus` says, as much as Olo uses.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Status {
    /// The messaging API's generation ([`olo_protocol::phone::API`]).
    pub api: u32,
    pub release: String,
    /// A phone is paired for messages (its profile's id; never its
    /// address).
    pub profile: Option<String>,
    pub initializing: bool,
    /// Messages (MAP), contacts (PBAP), notifications (ANCS) are up.
    pub map: bool,
    pub pbap: bool,
    pub ancs: bool,
    /// "initializing", "connecting", "ready", "degraded", "reconnecting",
    /// "authorization-required", "map-connection-refused", "stopping".
    pub connectivity: String,
    pub detail: String,
    pub battery: Option<u8>,
    pub contacts: u64,
    pub events: u64,
    /// The backend's own popups: "messages", "all" or "none".
    pub notification_policy: String,
    /// Its own copying of one-time codes to the clipboard.
    pub otp_autocopy: bool,
    /// "encrypted", "plaintext" or "none"; and how the store stands.
    pub storage_policy: String,
    pub storage_state: String,
    pub storage_detail: String,
    /// What the phone has been seen to allow ("contacts", …).
    pub verified: Vec<String>,
}

/// A message in a conversation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Message {
    /// The phone's handle for it (opaque).
    pub handle: String,
    pub body: String,
    /// When, ISO 8601 ("2026-08-12T10:00:00+00:00"; the phone may leave
    /// the zone out).
    pub at: String,
    pub outgoing: bool,
    pub read: bool,
    /// Who sent it (a group's member; empty for one's own).
    pub sender: String,
}

/// A conversation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Thread {
    /// The backend's key for it (opaque: replies go to a key, never to
    /// addresses a client made up).
    pub key: String,
    pub name: String,
    pub is_group: bool,
    pub recipients: Vec<String>,
    /// A reply can be sent to it as it stands.
    pub reply_ready: bool,
    /// Oldest first.
    pub messages: Vec<Message>,
    pub last: String,
    /// A group whose members must be said before it can be answered.
    pub participants_required: bool,
    /// Its members changed since they were last confirmed.
    pub roster_changed: bool,
    pub roster_warning_id: String,
    pub starred: bool,
}

impl Thread {
    pub fn unread(&self) -> usize {
        self.messages.iter().filter(|m| !m.outgoing && !m.read).count()
    }

    /// What a reply to a group must carry: the members it was shown
    /// with, so the backend refuses one made for another roster (the
    /// warning's id, then each distinct address in order, a line each).
    pub fn group_token(&self) -> String {
        if !self.is_group {
            return String::new();
        }
        let mut who: Vec<&str> = self.recipients.iter().map(String::as_str).filter(|r| !r.is_empty()).collect();
        who.sort_unstable();
        who.dedup();
        std::iter::once(self.roster_warning_id.as_str()).chain(who).collect::<Vec<_>>().join("\n")
    }
}

/// Someone in the phone's contacts, with one of their addresses.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Contact {
    pub name: String,
    pub address: String,
}

fn text(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or_default().to_owned()
}

fn flag(v: &Value, key: &str) -> bool {
    v.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn number(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(Value::as_u64).unwrap_or(0)
}

fn texts(v: &Value, key: &str) -> Vec<String> {
    v.get(key).and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect()).unwrap_or_default()
}

impl Status {
    pub fn read(json: &str) -> Status {
        let v: Value = serde_json::from_str(json).unwrap_or(Value::Null);
        Status {
            api: number(&v, "api_version") as u32,
            release: text(&v, "backend_release"),
            profile: v.get("phone_profile_id").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_owned),
            initializing: flag(&v, "initializing"),
            map: flag(&v, "map"),
            pbap: flag(&v, "pbap"),
            ancs: flag(&v, "ancs"),
            connectivity: text(&v, "connectivity_state"),
            detail: text(&v, "connectivity_detail"),
            battery: v.get("phone_battery_level").and_then(Value::as_u64).map(|b| b.min(100) as u8),
            contacts: number(&v, "contacts"),
            events: number(&v, "events"),
            notification_policy: text(&v, "notification_policy"),
            otp_autocopy: flag(&v, "otp_autocopy"),
            storage_policy: text(&v, "storage_policy"),
            storage_state: text(&v, "storage_state"),
            storage_detail: text(&v, "storage_detail"),
            verified: texts(&v, "verified_iphone_setup"),
        }
    }
}

impl Thread {
    fn of(v: &Value) -> Option<Thread> {
        v.as_object()?;
        let messages = v.get("messages").and_then(Value::as_array).map(|a| {
            a.iter()
                .filter(|m| m.is_object())
                .map(|m| Message { handle: text(m, "handle"), body: text(m, "body"), at: text(m, "timestamp"), outgoing: flag(m, "outgoing"), read: flag(m, "read"), sender: text(m, "sender") })
                .collect()
        });
        Some(Thread {
            key: text(v, "key"),
            name: Some(text(v, "name")).filter(|n| !n.is_empty()).unwrap_or_else(|| "Unknown".into()),
            is_group: flag(v, "is_group"),
            recipients: texts(v, "recipients"),
            reply_ready: flag(v, "reply_ready"),
            messages: messages.unwrap_or_default(),
            last: text(v, "last_ts"),
            participants_required: flag(v, "participants_required"),
            roster_changed: flag(v, "roster_changed"),
            roster_warning_id: text(v, "roster_warning_id"),
            starred: flag(v, "starred"),
        })
    }

    /// `ListThreads`' answer: an array of conversations.
    pub fn read_all(json: &str) -> Vec<Thread> {
        let v: Value = serde_json::from_str(json).unwrap_or(Value::Null);
        v.as_array().map(|a| a.iter().filter_map(Thread::of).collect()).unwrap_or_default()
    }
}

/// The name of one of the backend's own errors ("NotReady",
/// "RateLimited", "SendFailed", …), if `e` is one.
pub fn error_name(e: &zbus::Error) -> Option<&str> {
    match e {
        zbus::Error::MethodError(name, _, _) => name.as_str().strip_prefix(ERRORS),
        _ => None,
    }
}

/// `e` for a person: the backend's own words for its expected errors
/// (they're bounded and say nothing private), ours for the rest.
pub fn said(e: &zbus::Error) -> String {
    match e {
        zbus::Error::MethodError(name, message, _) => match (name.as_str().strip_prefix(ERRORS), message) {
            (Some("NotReady"), _) => "The phone isn't ready yet.".into(),
            (Some("RateLimited"), _) => "Too many at once: try again in a moment.".into(),
            (Some("SendOutcomeUnknown"), _) => "The phone didn't say whether it was sent: look on the phone before sending it again.".into(),
            (Some(_), Some(m)) if !m.is_empty() => m.clone(),
            (Some(name), _) => format!("The phone's backend refused ({name})."),
            (None, _) if name.as_str() == "org.freedesktop.DBus.Error.ServiceUnknown" => "The phone's backend isn't running.".into(),
            (None, m) => m.clone().unwrap_or_else(|| name.to_string()),
        },
        e => e.to_string(),
    }
}

/// What the backend tells of by itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// The conversations changed: fetch them again.
    History,
    /// So did the status.
    Status,
    /// A notification of the backend's was clicked: the message with this
    /// handle is wanted.
    Open(String),
    /// The backend came to the bus, or left it.
    Up,
    Down,
}

/// A connection to the bus the backend is on.
pub struct Client {
    conn: Connection,
}

impl Client {
    pub fn connect() -> Result<Client> {
        Ok(Client { conn: bus()? })
    }

    /// Whether the backend is on the bus now (asking never starts it).
    pub fn running(&self) -> bool {
        zbus::blocking::fdo::DBusProxy::new(&self.conn).is_ok_and(|d| d.name_has_owner(NAME.try_into().expect("a bus name")).unwrap_or(false))
    }

    fn call<B, R>(&self, method: &str, body: &B) -> Result<R>
    where
        B: serde::Serialize + zvariant::DynamicType,
        R: for<'d> serde::Deserialize<'d> + zvariant::Type,
    {
        self.conn.call_method(Some(NAME), PATH, Some(MESSAGES), method, body)?.body().deserialize()
    }

    pub fn status(&self) -> Result<Status> {
        self.call::<_, String>("GetStatus", &()).map(|j| Status::read(&j))
    }

    /// The conversations, the newest first, each with its messages (the
    /// backend keeps the answer under 8 MiB and 500 messages each).
    pub fn threads(&self, limit: u32) -> Result<Vec<Thread>> {
        self.call::<_, String>("ListThreads", &(limit,)).map(|j| Thread::read_all(&j))
    }

    /// A new message to a phone number or an email address.
    pub fn send(&self, to: &str, body: &str) -> Result<()> {
        self.call::<_, String>("Send", &(to, body)).map(|_| ())
    }

    /// An answer in a conversation; to a group, with the members it was
    /// shown with (`Thread::group_token`), which the person confirmed.
    pub fn reply(&self, thread: &Thread, body: &str) -> Result<()> {
        self.call::<_, String>("SendToThreadChecked", &(thread.key.as_str(), body, thread.is_group, thread.group_token().as_str())).map(|_| ())
    }

    /// A conversation's messages marked read (here, not on the phone).
    pub fn mark_read(&self, key: &str) -> Result<u32> {
        self.call("MarkThreadRead", &(key,))
    }

    pub fn set_starred(&self, key: &str, on: bool) -> Result<bool> {
        self.call("SetThreadStarred", &(key, on))
    }

    /// Conversations removed from this computer (the phone keeps them).
    pub fn delete(&self, keys: &[&str]) -> Result<u32> {
        self.call("DeleteThreads", &(keys, true))
    }

    /// The contacts whose names match.
    pub fn find_contacts(&self, query: &str) -> Result<Vec<Contact>> {
        let json: String = self.call("FindContacts", &(query,))?;
        let v: Value = serde_json::from_str(&json).unwrap_or(Value::Null);
        Ok(v.as_array().map(|a| a.iter().map(|c| Contact { name: text(c, "name"), address: text(c, "address") }).filter(|c| !c.address.is_empty()).collect()).unwrap_or_default())
    }

    /// The phone's contacts fetched again; how many are kept.
    pub fn sync_contacts(&self) -> Result<u32> {
        self.call("SyncContacts", &())
    }

    /// The backend's own popups: "messages", "all" or "none".
    pub fn set_notification_policy(&self, policy: &str) -> Result<String> {
        self.call("SetNotificationPolicy", &(policy,))
    }

    /// The backend's own copying of codes to the clipboard.
    pub fn set_otp_autocopy(&self, on: bool) -> Result<()> {
        self.call::<_, String>("SetOtpAutocopy", &(on,)).map(|_| ())
    }

    /// Try the phone again now: "started", "in-progress", "connected", ….
    pub fn reconnect(&self) -> Result<String> {
        self.call("ReconnectPhone", &())
    }

    /// The store of messages opened again (its key asked of the keyring).
    pub fn unlock_storage(&self) -> Result<()> {
        self.call::<_, String>("UnlockStorage", &()).map(|_| ())
    }

    /// Call `told` (on a thread of its own) with each thing the backend
    /// says and each time it comes or goes. Nothing is polled: the thread
    /// sleeps until the bus has something.
    pub fn watch(told: impl Fn(Event) + Send + 'static) -> Result<()> {
        use zbus::{MatchRule, message::Type};
        let conn = bus()?;
        let dbus = zbus::blocking::fdo::DBusProxy::new(&conn)?;
        let messages = zbus::blocking::MessageIterator::from(&conn);
        dbus.add_match_rule(MatchRule::builder().msg_type(Type::Signal).interface(EVENTS)?.path(PATH)?.build())?;
        dbus.add_match_rule(MatchRule::builder().msg_type(Type::Signal).sender("org.freedesktop.DBus")?.interface("org.freedesktop.DBus")?.member("NameOwnerChanged")?.arg(0, NAME)?.build())?;
        std::thread::Builder::new().name("phone-signals".into()).spawn(move || {
            let _keep = (conn, dbus);
            for m in messages.flatten() {
                if m.message_type() != Type::Signal {
                    continue;
                }
                let h = m.header();
                let event = match (h.interface().map(|i| i.as_str()), h.member().map(|m| m.as_str())) {
                    (Some(EVENTS), Some("HistoryChanged")) => Event::History,
                    (Some(EVENTS), Some("StatusChanged")) => Event::Status,
                    (Some(EVENTS), Some("OpenMessageRequested")) => match m.body().deserialize::<(String,)>() {
                        Ok((handle,)) => Event::Open(handle),
                        Err(_) => continue,
                    },
                    (Some("org.freedesktop.DBus"), Some("NameOwnerChanged")) => match m.body().deserialize::<(String, String, String)>() {
                        Ok((name, _, new)) if name == NAME => {
                            if new.is_empty() {
                                Event::Down
                            } else {
                                Event::Up
                            }
                        }
                        _ => continue,
                    },
                    _ => continue,
                };
                told(event);
            }
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_status_reads_leniently() {
        let s = Status::read(
            r#"{"daemon": true, "map": true, "pbap": false, "ancs": true, "api_version": 2, "backend_release": "0.9.1-1.fc45",
                "phone_profile_id": "0b7c1a52-6c0e-4c57-9d3f-2f1f0f1c7a11", "connectivity_state": "degraded",
                "connectivity_detail": "Some Bluetooth message profiles are unavailable", "phone_battery_level": 81,
                "contacts": 212, "events": 1840, "notification_policy": "messages", "otp_autocopy": false,
                "storage_policy": "encrypted", "storage_state": "ready", "verified_iphone_setup": ["contacts", 7],
                "something_new": {"a": 1}, "initializing": "no"}"#,
        );
        assert_eq!((s.api, s.release.as_str(), s.map, s.pbap, s.ancs), (2, "0.9.1-1.fc45", true, false, true));
        assert_eq!((s.connectivity.as_str(), s.battery, s.contacts), ("degraded", Some(81), 212));
        assert!(s.profile.is_some() && !s.initializing);
        assert_eq!(s.verified, ["contacts"]);
        // Nothing paired; and what isn't JSON at all.
        assert_eq!(Status::read(r#"{"phone_profile_id": null, "api_version": 2}"#).profile, None);
        assert_eq!(Status::read("not json"), Status::default());
    }

    #[test]
    fn conversations_read_and_a_groups_token_binds_its_members() {
        let threads = Thread::read_all(
            r#"[{"key": "t1", "name": "Acme", "is_group": false, "recipients": ["+15550100"], "reply_ready": true, "last_ts": "2026-08-12T11:00:00+00:00",
                 "messages": [{"handle": "h1", "body": "Your Acme code is 482913", "timestamp": "2026-08-12T10:00:00+00:00", "outgoing": false, "read": false, "sender": "+15550100", "body_markup": "…"},
                              {"handle": "h2", "body": "thanks", "timestamp": "2026-08-12T11:00:00+00:00", "outgoing": true, "read": true}, "stray"]},
                {"key": "g1", "is_group": true, "recipients": ["+15550102", "ann@example.com", "+15550102", ""], "roster_warning_id": "w7", "messages": null},
                42]"#,
        );
        assert_eq!(threads.len(), 2);
        assert_eq!((threads[0].name.as_str(), threads[0].messages.len(), threads[0].unread()), ("Acme", 2, 1));
        assert_eq!(threads[0].group_token(), "");
        assert_eq!((threads[1].name.as_str(), threads[1].messages.len()), ("Unknown", 0));
        assert_eq!(threads[1].group_token(), "w7\n+15550102\nann@example.com");
    }

    #[test]
    fn the_backends_errors_are_said_plainly() {
        let name = |n: &str| zbus::names::OwnedErrorName::try_from(n).unwrap();
        let message = zbus::message::Message::method_call("/", "M").unwrap().build(&()).unwrap();
        let e = zbus::Error::MethodError(name("io.weirdware.BlueFerry.Error.NotReady"), Some("contact cache is unavailable".into()), message.clone());
        assert_eq!((error_name(&e), said(&e).as_str()), (Some("NotReady"), "The phone isn't ready yet."));
        let e = zbus::Error::MethodError(name("io.weirdware.BlueFerry.Error.InvalidArgs"), Some("recipient is not a phone number or email address".into()), message.clone());
        assert_eq!(said(&e), "recipient is not a phone number or email address");
        let e = zbus::Error::MethodError(name("org.freedesktop.DBus.Error.ServiceUnknown"), None, message);
        assert_eq!((error_name(&e), said(&e).as_str()), (None, "The phone's backend isn't running."));
    }
}
