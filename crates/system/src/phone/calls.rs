//! The phone's calls, taken on this computer: the hands-free profile,
//! which the sound server speaks. PipeWire, as the hands-free unit of a
//! phone connected that way, runs the profile's commands and carries
//! the call's sound (the far end to the output in use, the microphone
//! in use to the phone), and says what it knows on the session bus:
//! `org.pipewire.Telephony`, an object manager at
//! `/org/pipewire/Telephony` with
//!
//! - a gateway for each phone connected hands-free (`…/agN`,
//!   `org.pipewire.Telephony.AudioGateway1`: `Address`, `Dial(number)`,
//!   `HangupAll`), and
//! - under it each of its calls (`…/agN/callM`,
//!   `org.pipewire.Telephony.Call1`: `State`, `LineIdentification`,
//!   `Name`, `Answer`, `Hangup`).
//!
//! Read from the interfaces PipeWire 1.6 declares (its Bluetooth
//! plugin's own definitions). Connecting the phone's hands-free side is
//! BlueZ's ([`super::link::Profiles::hands_free`]); this is what's then
//! there. `OLO_PHONE_TELEPHONY=ADDRESS` has a stand-in's bus instead.

use std::collections::HashMap;

use zbus::blocking::Connection;
use zvariant::{OwnedObjectPath, OwnedValue};

use crate::Result;

const DEST: &str = "org.pipewire.Telephony";
const ROOT: &str = "/org/pipewire/Telephony";
const GATEWAY: &str = "org.pipewire.Telephony.AudioGateway1";
const CALL: &str = "org.pipewire.Telephony.Call1";

/// How a call stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Ringing here (waiting: while another call is on).
    Incoming,
    Waiting,
    /// Made from here: not yet ringing there, ringing there.
    Dialing,
    Alerting,
    Active,
    Held,
    Over,
}

impl State {
    pub fn parse(state: &str) -> Option<State> {
        Some(match state {
            "incoming" => State::Incoming,
            "waiting" => State::Waiting,
            "dialing" => State::Dialing,
            "alerting" => State::Alerting,
            "active" => State::Active,
            "held" => State::Held,
            "disconnected" => State::Over,
            _ => return None,
        })
    }

    /// Ringing here, to be answered or declined.
    pub fn ringing(self) -> bool {
        matches!(self, State::Incoming | State::Waiting)
    }
}

/// A phone connected hands-free.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gateway {
    pub path: String,
    /// The phone's Bluetooth address.
    pub address: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    pub path: String,
    /// The other end's number, and its name where the phone gave one.
    pub number: String,
    pub name: String,
    pub state: State,
}

impl Call {
    /// The gateway it's a call of.
    pub fn gateway(&self) -> &str {
        self.path.rsplit_once('/').map_or("", |(gateway, _)| gateway)
    }
}

/// The sound server's telephony service.
pub struct Telephony {
    conn: Connection,
}

fn bus() -> Result<Connection> {
    match std::env::var("OLO_PHONE_TELEPHONY") {
        Ok(address) if !address.is_empty() => zbus::blocking::connection::Builder::address(address.as_str())?.build(),
        _ => Connection::session(),
    }
}

type Objects = HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>;

/// The gateways and calls among a manager's objects.
fn read(objects: Objects) -> (Vec<Gateway>, Vec<Call>) {
    let text = |p: &HashMap<String, OwnedValue>, k: &str| p.get(k).and_then(|v| <&str>::try_from(v).ok()).unwrap_or_default().to_owned();
    let (mut gateways, mut calls) = (Vec::new(), Vec::new());
    for (path, interfaces) in objects {
        if let Some(g) = interfaces.get(GATEWAY) {
            gateways.push(Gateway { path: path.as_str().to_owned(), address: text(g, "Address") });
        }
        if let Some(c) = interfaces.get(CALL)
            && let Some(state) = State::parse(&text(c, "State")).filter(|s| *s != State::Over)
        {
            calls.push(Call { path: path.as_str().to_owned(), number: text(c, "LineIdentification"), name: text(c, "Name"), state });
        }
    }
    gateways.sort_by(|a, b| a.path.cmp(&b.path));
    calls.sort_by(|a, b| a.path.cmp(&b.path));
    (gateways, calls)
}

impl Telephony {
    pub fn open() -> Result<Telephony> {
        Ok(Telephony { conn: bus()? })
    }

    /// The phones connected hands-free and their calls (none of either
    /// where the sound server has no telephony service).
    pub fn survey(&self) -> (Vec<Gateway>, Vec<Call>) {
        self.conn
            .call_method(Some(DEST), ROOT, Some("org.freedesktop.DBus.ObjectManager"), "GetManagedObjects", &())
            .ok()
            .and_then(|m| m.body().deserialize::<Objects>().ok())
            .map(read)
            .unwrap_or_default()
    }

    /// Call `number` through the phone at `gateway`.
    pub fn dial(&self, gateway: &str, number: &str) -> Result<()> {
        self.conn.call_method(Some(DEST), gateway, Some(GATEWAY), "Dial", &(number,)).map(|_| ())
    }

    pub fn answer(&self, call: &str) -> Result<()> {
        self.conn.call_method(Some(DEST), call, Some(CALL), "Answer", &()).map(|_| ())
    }

    /// End a call, or decline one ringing.
    pub fn hang_up(&self, call: &str) -> Result<()> {
        self.conn.call_method(Some(DEST), call, Some(CALL), "Hangup", &()).map(|_| ())
    }

    /// Call `changed` (on a thread of its own) whenever a gateway or a
    /// call comes, goes or changes, and when the service itself does.
    /// Nothing is polled.
    pub fn watch(&self, changed: impl Fn() + Send + 'static) -> Result<()> {
        use zbus::{MatchRule, message::Type};
        let conn = bus()?;
        let dbus = zbus::blocking::fdo::DBusProxy::new(&conn)?;
        let messages = zbus::blocking::MessageIterator::from(&conn);
        dbus.add_match_rule(MatchRule::builder().msg_type(Type::Signal).sender(DEST)?.build())?;
        dbus.add_match_rule(MatchRule::builder().msg_type(Type::Signal).sender("org.freedesktop.DBus")?.interface("org.freedesktop.DBus")?.member("NameOwnerChanged")?.arg(0, DEST)?.build())?;
        std::thread::Builder::new().name("phone-calls".into()).spawn(move || {
            let _keep = (conn, dbus);
            for m in messages.flatten() {
                if m.message_type() == Type::Signal {
                    changed();
                }
            }
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_the_service_has() {
        let value = |s: &str| OwnedValue::try_from(zvariant::Value::from(s)).unwrap();
        let object = |path: &str, interface: &str, props: &[(&str, &str)]| {
            let props: HashMap<String, OwnedValue> = props.iter().map(|(k, v)| ((*k).to_owned(), value(v))).collect();
            (OwnedObjectPath::try_from(path).unwrap(), HashMap::from([(interface.to_owned(), props)]))
        };
        let objects: Objects = HashMap::from([
            object("/org/pipewire/Telephony/ag0", GATEWAY, &[("Address", "02:00:00:00:F0:01")]),
            object("/org/pipewire/Telephony/ag0/call2", CALL, &[("State", "incoming"), ("LineIdentification", "+15550100142"), ("Name", "")]),
            object("/org/pipewire/Telephony/ag0/call1", CALL, &[("State", "active"), ("LineIdentification", "+15550100190")]),
            object("/org/pipewire/Telephony/ag0/call3", CALL, &[("State", "disconnected")]),
        ]);
        let (gateways, calls) = read(objects);
        assert_eq!(gateways, [Gateway { path: "/org/pipewire/Telephony/ag0".into(), address: "02:00:00:00:F0:01".into() }]);
        assert_eq!(calls.iter().map(|c| (c.number.as_str(), c.state)).collect::<Vec<_>>(), [("+15550100190", State::Active), ("+15550100142", State::Incoming)]);
        assert_eq!(calls[1].gateway(), "/org/pipewire/Telephony/ag0");
        assert!(calls[1].state.ringing() && !calls[0].state.ringing());
        assert_eq!(State::parse("alerting"), Some(State::Alerting));
        assert_eq!(State::parse("ringing"), None);
    }
}
