//! `blueferry-fake [--address ADDRESS] [--unpaired]`: a stand-in for
//! BlueFerry's backend (`io.weirdware.BlueFerry`, its `Messages1` and
//! `Events1`) for headless sessions and tests, so nothing touches
//! Bluetooth, a phone or anyone's messages. It takes the name on the bus
//! at ADDRESS (default: the session bus) and plays an iPhone that is
//! connected, at 81%, with a few conversations between made-up people:
//! messages can be sent and answered (they're added, and the history
//! said to have changed, as the real backend says it), conversations
//! marked read, starred and deleted, contacts found, the backend's own
//! popups and code copying switched. Point Olo at it with
//! `OLO_PHONE_BUS=ADDRESS`.
//!
//! What a test makes happen, it tells the running stand-in:
//! `blueferry-fake [--address ADDRESS] --tell receive NAME ADDRESS TEXT…`
//! (a message arrives, now) and `--tell set KEY VALUE` (`link`: ready,
//! degraded, reconnecting, authorization-required, …; `battery`: a
//! percent or `none`; `paired`: yes or no; `storage`: ready, locked, …).

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, mpsc},
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::{Value as Json, json};

const NAME: &str = "io.weirdware.BlueFerry";
const PATH: &str = "/io/weirdware/BlueFerry";
const EVENTS: &str = "io.weirdware.BlueFerry.Events1";
const FAKE: &str = "org.olo.BlueFerryFake";

#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "io.weirdware.BlueFerry.Error")]
enum Error {
    #[zbus(error)]
    ZBus(zbus::Error),
    NotReady(String),
    InvalidArgs(String),
    NotFound(String),
    ConfirmationRequired(String),
}

struct Message {
    handle: String,
    body: String,
    at: i64,
    outgoing: bool,
    read: bool,
    sender: String,
}

struct Thread {
    key: String,
    name: String,
    recipients: Vec<String>,
    messages: Vec<Message>,
    starred: bool,
}

impl Thread {
    fn is_group(&self) -> bool {
        self.recipients.len() > 1
    }

    fn last(&self) -> i64 {
        self.messages.last().map_or(0, |m| m.at)
    }

    fn json(&self) -> Json {
        json!({
            "key": self.key,
            "name": self.name,
            "is_group": self.is_group(),
            "recipients": self.recipients,
            "reply_ready": true,
            "messages": self.messages.iter().map(|m| json!({
                "handle": m.handle, "body": m.body, "timestamp": iso(m.at), "outgoing": m.outgoing, "read": m.read, "sender": m.sender,
            })).collect::<Vec<_>>(),
            "last_ts": iso(self.last()),
            "group_origin": if self.is_group() { "roster" } else { "" },
            "participants_required": false,
            "roster_changed": false,
            "unexpected_sender": "",
            "prompt_sender": "",
            "roster_warning_id": "",
            "starred": self.starred,
            "group_confirmed": self.is_group(),
        })
    }
}

struct Model {
    threads: Vec<Thread>,
    contacts: Vec<(String, String)>,
    paired: bool,
    link: String,
    battery: Option<u8>,
    storage: String,
    policy: String,
    otp_autocopy: bool,
    next: u64,
    /// What to say on the bus once the method has answered.
    signals: mpsc::Sender<&'static str>,
}

type Shared = Arc<Mutex<Model>>;

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// Unix seconds as ISO 8601 in UTC ("2026-08-12T10:00:00+00:00").
fn iso(t: i64) -> String {
    let (days, secs) = (t.div_euclid(86_400), t.rem_euclid(86_400));
    // (Howard Hinnant's days-to-civil.)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let (d, m) = (doy - (153 * mp + 2) / 5 + 1, if mp < 10 { mp + 3 } else { mp - 9 });
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}+00:00", secs / 3600, secs % 3600 / 60, secs % 60)
}

impl Model {
    fn sample(signals: mpsc::Sender<&'static str>) -> Model {
        let t = now();
        let (min, hour, day) = (60, 3600, 86_400);
        let mut next = 0u64;
        let mut msg = |body: &str, ago: i64, outgoing: bool, sender: &str| {
            next += 1;
            Message { handle: format!("h{next}"), body: body.into(), at: t - ago, outgoing, read: true, sender: if outgoing { String::new() } else { sender.into() } }
        };
        let threads = vec![
            Thread {
                key: "t-mira".into(),
                name: "Mira Kovač".into(),
                recipients: vec!["+15550142".into()],
                messages: vec![
                    msg("Are you still coming on Saturday?", 3 * hour, false, "+15550142"),
                    msg("Yes! I'll bring the seedlings.", 3 * hour - 4 * min, true, ""),
                    msg("Perfect. The gate code changed, I'll tell you when you're here.", 2 * hour, false, "+15550142"),
                ],
                starred: true,
            },
            Thread {
                key: "t-allotment".into(),
                name: "Allotment".into(),
                recipients: vec!["+15550142".into(), "+15550117".into(), "tomas@example.org".into()],
                messages: vec![
                    msg("Water is back on at the north tap.", day + 2 * hour, false, "+15550117"),
                    msg("Thanks Tomas!", day + hour, true, ""),
                    msg("Compost delivery is Thursday morning.", 5 * hour, false, "tomas@example.org"),
                ],
                starred: false,
            },
            Thread {
                key: "t-dad".into(),
                name: "Dad".into(),
                recipients: vec!["+15550108".into()],
                messages: vec![msg("Call me when you land.", 2 * day, false, "+15550108"), msg("Landed. All good.", 2 * day - 3 * hour, true, "")],
                starred: false,
            },
            Thread {
                key: "t-parcel".into(),
                name: "+15550190".into(),
                recipients: vec!["+15550190".into()],
                messages: vec![msg("Your parcel arrives tomorrow between 10:00 and 12:00.", 26 * hour, false, "+15550190")],
                starred: false,
            },
            Thread {
                key: "t-acme".into(),
                name: "22395".into(),
                recipients: vec!["22395".into()],
                messages: vec![msg("Your Acme Bank verification code is 305518. It expires in 10 minutes.", 9 * day, false, "22395")],
                starred: false,
            },
        ];
        let contacts = [("Mira Kovač", "+15550142"), ("Tomas Lind", "+15550117"), ("Tomas Lind", "tomas@example.org"), ("Dad", "+15550108"), ("Ines Duarte", "+15550163")];
        Model {
            threads,
            contacts: contacts.iter().map(|(n, a)| ((*n).to_owned(), (*a).to_owned())).collect(),
            paired: true,
            link: "ready".into(),
            battery: Some(81),
            storage: "ready".into(),
            policy: "messages".into(),
            otp_autocopy: false,
            next,
            signals,
        }
    }

    fn handle(&mut self) -> String {
        self.next += 1;
        format!("h{}", self.next)
    }

    fn say(&self, signal: &'static str) {
        let _ = self.signals.send(signal);
    }

    fn ready(&self) -> Result<(), Error> {
        if self.paired && matches!(self.link.as_str(), "ready" | "degraded") { Ok(()) } else { Err(Error::NotReady("the phone is not connected".into())) }
    }

    fn name_of(&self, address: &str) -> String {
        self.contacts.iter().find(|(_, a)| a == address).map_or_else(|| address.to_owned(), |(n, _)| n.clone())
    }

    fn status(&self) -> Json {
        let ready = self.paired && self.link == "ready";
        json!({
            "daemon": true,
            "api_version": 2,
            "backend_release": "0.9.1-1.fake",
            "_build_id": "0.9.1-1.fake",
            "phone_profile_id": if self.paired { Json::from("5a1f0c9e-7d2b-4c61-9b7e-0d3c2a1b4f68") } else { Json::Null },
            "initializing": false,
            "map": ready || (self.paired && self.link == "degraded"),
            "pbap": ready,
            "ancs": ready,
            "connectivity_state": if self.paired { self.link.as_str() } else { "connecting" },
            "connectivity_detail": "",
            "phone_battery_level": if ready { self.battery.map(Json::from).unwrap_or(Json::Null) } else { Json::Null },
            "contacts": self.contacts.len(),
            "events": self.threads.iter().map(|t| t.messages.len()).sum::<usize>(),
            "verified_iphone_setup": if self.paired { vec!["messages", "contacts"] } else { vec![] },
            "notification_policy": self.policy,
            "contacts_only_notifications": false,
            "notification_content_shown": false,
            "otp_autocopy": self.otp_autocopy,
            "saved_opt_ins": true,
            "storage_policy": "encrypted",
            "storage_state": self.storage,
            "storage_detail": "",
        })
    }

    /// A message arrives from `address` (shown as `name` if it has no
    /// conversation yet and isn't a contact).
    fn receive(&mut self, name: &str, address: &str, body: &str) -> String {
        let handle = self.handle();
        let message = Message { handle: handle.clone(), body: body.into(), at: now(), outgoing: false, read: false, sender: address.into() };
        match self.threads.iter_mut().find(|t| !t.is_group() && t.recipients.first().is_some_and(|r| r == address)) {
            Some(t) => t.messages.push(message),
            None => {
                let name = if name.is_empty() { self.name_of(address) } else { name.to_owned() };
                self.threads.push(Thread { key: format!("t-{}", self.next), name, recipients: vec![address.into()], messages: vec![message], starred: false });
            }
        }
        self.say("HistoryChanged");
        handle
    }

    fn sent(&mut self, key: &str, body: &str) -> Result<String, Error> {
        if body.trim().is_empty() {
            return Err(Error::InvalidArgs("message body is empty".into()));
        }
        self.ready()?;
        let handle = self.handle();
        let Some(t) = self.threads.iter_mut().find(|t| t.key == key) else { return Err(Error::NotFound("no such conversation".into())) };
        t.messages.push(Message { handle: handle.clone(), body: body.into(), at: now(), outgoing: true, read: true, sender: String::new() });
        self.say("HistoryChanged");
        Ok(format!("/org/bluez/obex/client/session0/transfer{}", &handle[1..]))
    }
}

struct Messages {
    model: Shared,
}

#[zbus::interface(name = "io.weirdware.BlueFerry.Messages1")]
impl Messages {
    fn get_status(&self) -> String {
        self.model.lock().expect("model").status().to_string()
    }

    fn is_healthy(&self) -> bool {
        self.model.lock().expect("model").ready().is_ok()
    }

    fn list_threads(&self, limit: u32) -> String {
        let model = self.model.lock().expect("model");
        let mut threads: Vec<&Thread> = model.threads.iter().collect();
        threads.sort_by_key(|t| std::cmp::Reverse(t.last()));
        Json::from(threads.iter().take(limit.max(1) as usize).map(|t| t.json()).collect::<Vec<_>>()).to_string()
    }

    fn send(&self, recipient: &str, body: &str) -> Result<String, Error> {
        let mut model = self.model.lock().expect("model");
        let to = recipient.trim();
        if !(to.contains('@') || to.trim_start_matches('+').chars().all(|c| c.is_ascii_digit()) && to.len() >= 3) {
            return Err(Error::InvalidArgs("recipient is not a phone number or email address".into()));
        }
        let key = match model.threads.iter().find(|t| !t.is_group() && t.recipients.first().is_some_and(|r| r == to)) {
            Some(t) => t.key.clone(),
            None => {
                model.ready()?;
                let key = format!("t-{}", model.next + 1);
                let name = model.name_of(to);
                model.threads.push(Thread { key: key.clone(), name, recipients: vec![to.into()], messages: vec![], starred: false });
                key
            }
        };
        model.sent(&key, body)
    }

    fn send_to_thread(&self, thread_key: &str, body: &str, _confirm_group: bool) -> Result<String, Error> {
        self.model.lock().expect("model").sent(thread_key, body)
    }

    fn send_to_thread_checked(&self, thread_key: &str, body: &str, confirm_group: bool, expected_group_token: &str) -> Result<String, Error> {
        let mut model = self.model.lock().expect("model");
        if let Some(t) = model.threads.iter().find(|t| t.key == thread_key)
            && t.is_group()
        {
            // The members it was shown with, a line each after the
            // (empty) warning id, in order.
            let mut who = t.recipients.clone();
            who.sort_unstable();
            who.dedup();
            let token = std::iter::once(String::new()).chain(who).collect::<Vec<_>>().join("\n");
            if !confirm_group || expected_group_token != token {
                return Err(Error::ConfirmationRequired("the group's members must be confirmed".into()));
            }
        }
        model.sent(thread_key, body)
    }

    fn mark_thread_read(&self, thread_key: &str) -> Result<u32, Error> {
        let mut model = self.model.lock().expect("model");
        let Some(t) = model.threads.iter_mut().find(|t| t.key == thread_key) else { return Err(Error::NotFound("no such conversation".into())) };
        let mut n = 0;
        for m in t.messages.iter_mut().filter(|m| !m.outgoing && !m.read) {
            m.read = true;
            n += 1;
        }
        if n > 0 {
            model.say("HistoryChanged");
        }
        Ok(n)
    }

    fn set_thread_starred(&self, thread_key: &str, starred: bool) -> Result<bool, Error> {
        let mut model = self.model.lock().expect("model");
        let Some(t) = model.threads.iter_mut().find(|t| t.key == thread_key) else { return Err(Error::NotFound("no such conversation".into())) };
        t.starred = starred;
        model.say("HistoryChanged");
        Ok(starred)
    }

    fn delete_threads(&self, thread_keys: Vec<String>, confirmed: bool) -> Result<u32, Error> {
        if !confirmed {
            return Err(Error::ConfirmationRequired("deleting must be confirmed".into()));
        }
        let mut model = self.model.lock().expect("model");
        let before = model.threads.len();
        model.threads.retain(|t| !thread_keys.contains(&t.key));
        let n = (before - model.threads.len()) as u32;
        if n > 0 {
            model.say("HistoryChanged");
        }
        Ok(n)
    }

    fn find_contacts(&self, query: &str) -> String {
        let q = query.trim().to_lowercase();
        let model = self.model.lock().expect("model");
        let found: Vec<Json> = model.contacts.iter().filter(|(n, _)| !q.is_empty() && n.to_lowercase().contains(&q)).map(|(n, a)| json!({"name": n, "address": a})).collect();
        Json::from(found).to_string()
    }

    fn sync_contacts(&self) -> Result<u32, Error> {
        let model = self.model.lock().expect("model");
        model.ready()?;
        Ok(model.contacts.len() as u32)
    }

    fn get_notification_policy(&self) -> String {
        self.model.lock().expect("model").policy.clone()
    }

    fn set_notification_policy(&self, policy: &str) -> Result<String, Error> {
        if !["messages", "all", "none"].contains(&policy) {
            return Err(Error::InvalidArgs("unknown notification policy".into()));
        }
        let mut model = self.model.lock().expect("model");
        model.policy = policy.into();
        model.say("StatusChanged");
        Ok(policy.into())
    }

    fn set_otp_autocopy(&self, enabled: bool) -> String {
        let mut model = self.model.lock().expect("model");
        model.otp_autocopy = enabled;
        model.say("StatusChanged");
        json!({"otp_autocopy": enabled}).to_string()
    }

    fn reconnect_phone(&self) -> String {
        let mut model = self.model.lock().expect("model");
        if !model.paired {
            return "started".into();
        }
        if model.link != "ready" {
            model.link = "ready".into();
            model.say("StatusChanged");
            return "started".into();
        }
        "connected".into()
    }

    fn unlock_storage(&self) -> String {
        let mut model = self.model.lock().expect("model");
        model.storage = "ready".into();
        model.say("StatusChanged");
        json!({"storage_state": "ready"}).to_string()
    }
}

/// What a test tells the stand-in to play.
struct Fake {
    model: Shared,
}

#[zbus::interface(name = "org.olo.BlueFerryFake")]
impl Fake {
    fn receive(&self, name: &str, address: &str, body: &str) -> String {
        self.model.lock().expect("model").receive(name, address, body)
    }

    fn set(&self, key: &str, value: &str) -> Result<(), Error> {
        let mut model = self.model.lock().expect("model");
        match key {
            "link" => model.link = value.into(),
            "battery" => model.battery = value.parse().ok(),
            "paired" => model.paired = value == "yes",
            "storage" => model.storage = value.into(),
            _ => return Err(Error::InvalidArgs(format!("nothing called {key}"))),
        }
        model.say("StatusChanged");
        Ok(())
    }

    /// A notification of the backend's clicked: the message wanted.
    fn open(&self, handle: &str) {
        let model = self.model.lock().expect("model");
        let _ = handle;
        model.say("OpenMessageRequested");
    }
}

const USAGE: &str = "usage: blueferry-fake [--address ADDRESS] [--unpaired]
       blueferry-fake [--address ADDRESS] --tell receive NAME ADDRESS TEXT…
       blueferry-fake [--address ADDRESS] --tell set link|battery|paired|storage VALUE";

fn main() {
    olo_crash::install();
    let mut address = std::env::var("OLO_PHONE_BUS").or_else(|_| std::env::var("DBUS_SESSION_BUS_ADDRESS")).unwrap_or_default();
    let mut unpaired = false;
    let mut tell: Option<Vec<String>> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--address" => address = args.next().expect("--address ADDRESS"),
            "--unpaired" => unpaired = true,
            "--tell" => tell = Some(args.by_ref().collect()),
            _ => {
                eprintln!("{USAGE}");
                std::process::exit(2);
            }
        }
    }
    if let Some(words) = tell {
        std::process::exit(told(&address, &words));
    }
    let (tx, rx) = mpsc::channel::<&'static str>();
    let mut sample = Model::sample(tx);
    sample.paired = !unpaired;
    let model: Shared = Arc::new(Mutex::new(sample));
    let built = zbus::blocking::connection::Builder::address(address.as_str())
        .and_then(|b| b.serve_at(PATH, Messages { model: model.clone() }))
        .and_then(|b| b.serve_at(PATH, Fake { model: model.clone() }))
        .and_then(|b| b.name(NAME))
        .and_then(|b| b.build());
    let conn = match built {
        Ok(c) => c,
        Err(e) => {
            eprintln!("blueferry-fake: can't take {NAME} on {address}: {e}");
            std::process::exit(1);
        }
    };
    // What the methods ask to be said, said once they've answered (a
    // method can't wait on the bus it is answering on).
    let mut revision = 0u64;
    for signal in rx {
        let sent = match signal {
            "HistoryChanged" => {
                revision += 1;
                let mut body: HashMap<&str, zvariant::Value> = HashMap::new();
                body.insert("revision", revision.into());
                conn.emit_signal(None::<&str>, PATH, EVENTS, signal, &(body,))
            }
            "OpenMessageRequested" => {
                let handle = model.lock().expect("model").threads.iter().flat_map(|t| t.messages.iter()).filter(|m| !m.outgoing).max_by_key(|m| m.at).map(|m| m.handle.clone()).unwrap_or_default();
                conn.emit_signal(None::<&str>, PATH, EVENTS, signal, &(handle,))
            }
            _ => conn.emit_signal(None::<&str>, PATH, EVENTS, signal, &()),
        };
        if let Err(e) = sent {
            eprintln!("blueferry-fake: {signal}: {e}");
        }
    }
}

/// Tell the stand-in running on `address` what to play.
fn told(address: &str, words: &[String]) -> i32 {
    let call = |method: &str, body: &dyn Fn(&zbus::blocking::Connection) -> zbus::Result<zbus::Message>| match zbus::blocking::connection::Builder::address(address)
        .and_then(|b| b.build())
        .and_then(|c| body(&c))
    {
        Ok(_) => 0,
        Err(e) => {
            eprintln!("blueferry-fake: {method}: {e}");
            1
        }
    };
    match words.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["receive", name, from, text @ ..] if !text.is_empty() => {
            let text = text.join(" ");
            call("receive", &|c| c.call_method(Some(NAME), PATH, Some(FAKE), "Receive", &(*name, *from, text.as_str())))
        }
        ["set", key, value] => call("set", &|c| c.call_method(Some(NAME), PATH, Some(FAKE), "Set", &(*key, *value))),
        ["open"] => call("open", &|c| c.call_method(Some(NAME), PATH, Some(FAKE), "Open", &("",))),
        _ => {
            eprintln!("{USAGE}");
            2
        }
    }
}
