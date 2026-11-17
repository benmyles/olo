//! The link to the phone: BlueZ's. Olo registers three profiles with
//! BlueZ (`org.bluez.ProfileManager1`): the client's side of the phone's
//! messages and of its contacts, and a Message Notification Server of
//! its own for the phone to connect to. BlueZ finds the phone's channel
//! for a profile, opens it, and hands over the socket
//! (`Profile1.NewConnection`); what's spoken over it is Olo's
//! ([`super::obex`], [`super::map`], [`super::pbap`]). Nothing here needs
//! root, obexd or any other program.
//!
//! [`Profiles::register`] keeps the profiles registered for as long as
//! it's kept, and hears each channel as it opens ([`Opened`]);
//! [`Profiles::open`] asks for one to a phone; [`Profiles::phones`]
//! lists the phones paired (anything paired that offers its messages);
//! [`Profiles::watch`] tells when that changes. `OLO_PHONE_BLUEZ=ADDRESS`
//! points all of it at another bus (tests: `phone-standin`, a stand-in
//! BlueZ with a stand-in phone).

use std::{
    collections::HashMap,
    io::{Read, Write},
    os::fd::{AsRawFd, OwnedFd},
    sync::Arc,
    time::Duration,
};

use zbus::blocking::Connection;
use zvariant::{OwnedObjectPath, OwnedValue, Value};

use super::{map, pbap};
use crate::Result;

const DEST: &str = "org.bluez";
const DEVICE: &str = "org.bluez.Device1";
const MANAGER: &str = "org.bluez.ProfileManager1";

/// The channel the Notification Server listens on (RFCOMM; any free one).
const MNS_CHANNEL: u16 = 19;

/// Which of the phone's services a channel is to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Channel {
    /// Its Message Access Server (Olo is the client).
    Messages,
    /// Its Phone Book Server (Olo is the client).
    Contacts,
    /// Olo's own Notification Server (the phone is the client).
    Notifications,
}

impl Channel {
    const ALL: [Channel; 3] = [Channel::Messages, Channel::Contacts, Channel::Notifications];

    pub fn uuid(self) -> &'static str {
        match self {
            Channel::Messages => map::MAS_UUID,
            Channel::Contacts => pbap::PSE_UUID,
            Channel::Notifications => map::MNS_UUID,
        }
    }

    fn path(self) -> &'static str {
        match self {
            Channel::Messages => "/com/olodesktop/phone/messages",
            Channel::Contacts => "/com/olodesktop/phone/contacts",
            Channel::Notifications => "/com/olodesktop/phone/notifications",
        }
    }
}

/// A channel to a phone: read from and written to as any stream; a read
/// gives up after a while ([`Stream::patience`]) rather than wait for
/// good, and [`Stream::closer`] ends it from another thread.
pub struct Stream(std::fs::File);

impl Stream {
    fn new(fd: OwnedFd) -> Stream {
        let s = Stream(std::fs::File::from(fd));
        s.patience(Duration::from_secs(30));
        s
    }

    /// How long a read or a write waits (0: for good).
    pub fn patience(&self, wait: Duration) {
        let tv = libc::timeval { tv_sec: wait.as_secs() as libc::time_t, tv_usec: wait.subsec_micros() as libc::suseconds_t };
        for option in [libc::SO_RCVTIMEO, libc::SO_SNDTIMEO] {
            // SAFETY: a timeval of the size given, on our own socket.
            unsafe { libc::setsockopt(self.0.as_raw_fd(), libc::SOL_SOCKET, option, (&raw const tv).cast(), std::mem::size_of::<libc::timeval>() as libc::socklen_t) };
        }
    }

    /// Something that ends the channel from elsewhere (a read waiting on
    /// it returns).
    pub fn closer(&self) -> Closer {
        Closer(self.0.try_clone().ok())
    }
}

pub struct Closer(Option<std::fs::File>);

impl Closer {
    pub fn close(&self) {
        if let Some(f) = &self.0 {
            // SAFETY: shutting down our own descriptor.
            unsafe { libc::shutdown(f.as_raw_fd(), libc::SHUT_RDWR) };
        }
    }
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.0.read(buf)
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A channel BlueZ opened.
pub struct Opened {
    pub channel: Channel,
    /// The phone's object path with BlueZ.
    pub phone: String,
    pub stream: Stream,
}

/// What kind of phone it is, by its maker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    IPhone,
    /// Android, or anything else that speaks the profiles.
    Other,
}

/// A phone paired with this computer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Phone {
    pub path: String,
    pub name: String,
    pub address: String,
    pub kind: Kind,
    pub connected: bool,
    /// It offers its contacts too.
    pub contacts: bool,
    /// Its battery, in percent, when BlueZ has it.
    pub battery: Option<u8>,
}

/// Apple's Bluetooth vendor id, in a Modalias ("bluetooth:v004Cp…").
pub fn kind_of(modalias: &str) -> Kind {
    if modalias.to_ascii_lowercase().starts_with("bluetooth:v004c") { Kind::IPhone } else { Kind::Other }
}

fn bus() -> Result<Connection> {
    match std::env::var("OLO_PHONE_BLUEZ") {
        Ok(address) if !address.is_empty() => zbus::blocking::connection::Builder::address(address.as_str())?.build(),
        _ => Connection::system(),
    }
}

struct Profile {
    channel: Channel,
    opened: Arc<dyn Fn(Opened) + Send + Sync>,
}

#[zbus::interface(name = "org.bluez.Profile1")]
impl Profile {
    fn release(&self) {}

    fn new_connection(&self, device: OwnedObjectPath, fd: zvariant::OwnedFd, _properties: HashMap<String, OwnedValue>) {
        (self.opened)(Opened { channel: self.channel, phone: device.as_str().to_owned(), stream: Stream::new(OwnedFd::from(fd)) });
    }

    fn request_disconnection(&self, _device: OwnedObjectPath) {}
}

/// The Notification Server's service record: what a phone looks for to
/// know this computer wants to be told of messages (MAP's client is
/// known by it; an iPhone offers Show Notifications to what has it).
fn mns_record() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" ?>
<record>
  <attribute id="0x0001"><sequence><uuid value="0x1133"/></sequence></attribute>
  <attribute id="0x0004"><sequence>
    <sequence><uuid value="0x0100"/></sequence>
    <sequence><uuid value="0x0003"/><uint8 value="0x{MNS_CHANNEL:02x}"/></sequence>
    <sequence><uuid value="0x0008"/></sequence>
  </sequence></attribute>
  <attribute id="0x0005"><sequence><uuid value="0x1002"/></sequence></attribute>
  <attribute id="0x0009"><sequence><sequence><uuid value="0x1134"/><uint16 value="0x0102"/></sequence></sequence></attribute>
  <attribute id="0x0100"><text value="Olo Messages"/></attribute>
  <attribute id="0x0317"><uint32 value="0x0000001f"/></attribute>
</record>"#
    )
}

/// Olo's profiles, registered with BlueZ for as long as this is kept.
pub struct Profiles {
    conn: Connection,
}

impl Profiles {
    /// Register them; `opened` hears each channel as BlueZ opens it (on
    /// the bus's thread: hand it on, don't work there). An error whose
    /// name ends `AlreadyExists`: another program has them (obexd).
    pub fn register(opened: impl Fn(Opened) + Send + Sync + 'static) -> Result<Profiles> {
        let conn = bus()?;
        let opened: Arc<dyn Fn(Opened) + Send + Sync> = Arc::new(opened);
        for channel in Channel::ALL {
            conn.object_server().at(channel.path(), Profile { channel, opened: opened.clone() })?;
            let mut options: HashMap<&str, Value> = HashMap::new();
            options.insert("RequireAuthentication", true.into());
            options.insert("RequireAuthorization", false.into());
            options.insert("AutoConnect", false.into());
            let record = mns_record();
            match channel {
                Channel::Notifications => {
                    options.insert("Name", "Olo Messages".into());
                    options.insert("Role", "server".into());
                    options.insert("Channel", MNS_CHANNEL.into());
                    options.insert("ServiceRecord", record.as_str().into());
                }
                Channel::Messages | Channel::Contacts => {
                    options.insert("Name", if channel == Channel::Messages { "Olo Messages" } else { "Olo Contacts" }.into());
                    options.insert("Role", "client".into());
                }
            }
            let path = zvariant::ObjectPath::try_from(channel.path())?;
            conn.call_method(Some(DEST), "/org/bluez", Some(MANAGER), "RegisterProfile", &(path, channel.uuid(), options))?;
        }
        Ok(Profiles { conn })
    }

    /// Ask BlueZ for a channel to `phone`'s messages or contacts: it
    /// comes to `opened` when it's open. An error: the phone isn't in
    /// reach, doesn't offer it, or refused.
    pub fn open(&self, phone: &str, channel: Channel) -> Result<()> {
        self.conn.call_method(Some(DEST), phone, Some(DEVICE), "ConnectProfile", &(channel.uuid(),)).map(|_| ())
    }

    /// The phones paired with this computer: what's paired and offers
    /// its messages.
    pub fn phones(&self) -> Result<Vec<Phone>> {
        type Objects = HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>;
        let objects: Objects = self.conn.call_method(Some(DEST), "/", Some("org.freedesktop.DBus.ObjectManager"), "GetManagedObjects", &())?.body().deserialize()?;
        let mut phones: Vec<Phone> = objects
            .into_iter()
            .filter_map(|(path, interfaces)| {
                let d = interfaces.get(DEVICE)?;
                let flag = |k: &str| d.get(k).and_then(|v| bool::try_from(v).ok()).unwrap_or(false);
                let text = |k: &str| d.get(k).and_then(|v| <&str>::try_from(v).ok()).unwrap_or_default().to_owned();
                let uuids: Vec<String> = d.get("UUIDs").and_then(|v| <Vec<String>>::try_from(v.try_clone().ok()?).ok()).unwrap_or_default();
                let offers = |uuid: &str| uuids.iter().any(|u| u.eq_ignore_ascii_case(uuid));
                (flag("Paired") && offers(map::MAS_UUID)).then(|| Phone {
                    path: path.as_str().to_owned(),
                    name: Some(text("Alias")).filter(|n| !n.is_empty()).unwrap_or_else(|| text("Name")),
                    address: text("Address"),
                    kind: kind_of(&text("Modalias")),
                    connected: flag("Connected"),
                    contacts: offers(pbap::PSE_UUID),
                    battery: interfaces.get("org.bluez.Battery1").and_then(|b| b.get("Percentage")).and_then(|v| u8::try_from(v).ok()),
                })
            })
            .collect();
        phones.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(phones)
    }

    /// Call `changed` (on a thread of its own) whenever a device comes,
    /// goes, connects, disconnects, pairs or says what it offers. Nothing
    /// is polled.
    pub fn watch(&self, changed: impl Fn() + Send + 'static) -> Result<()> {
        use zbus::{MatchRule, message::Type};
        const TOLD: &[&str] = &["Connected", "Paired", "UUIDs", "Alias", "ServicesResolved"];
        let conn = bus()?;
        let dbus = zbus::blocking::fdo::DBusProxy::new(&conn)?;
        let messages = zbus::blocking::MessageIterator::from(&conn);
        dbus.add_match_rule(MatchRule::builder().msg_type(Type::Signal).sender(DEST)?.interface("org.freedesktop.DBus.ObjectManager")?.build())?;
        dbus.add_match_rule(MatchRule::builder().msg_type(Type::Signal).sender(DEST)?.interface("org.freedesktop.DBus.Properties")?.member("PropertiesChanged")?.build())?;
        std::thread::Builder::new().name("phone-bluez".into()).spawn(move || {
            let _keep = (conn, dbus);
            for m in messages.flatten() {
                if m.message_type() != Type::Signal {
                    continue;
                }
                let tell = match m.header().member().map(|m| m.as_str().to_owned()).as_deref() {
                    Some("InterfacesAdded" | "InterfacesRemoved") => true,
                    Some("PropertiesChanged") => m
                        .body()
                        .deserialize::<(String, HashMap<String, OwnedValue>, Vec<String>)>()
                        .is_ok_and(|(iface, props, gone)| iface == DEVICE && props.keys().chain(gone.iter()).any(|k| TOLD.contains(&k.as_str()))),
                    _ => false,
                };
                if tell {
                    changed();
                }
            }
        })?;
        Ok(())
    }
}

impl Drop for Profiles {
    fn drop(&mut self) {
        for channel in Channel::ALL {
            if let Ok(path) = zvariant::ObjectPath::try_from(channel.path()) {
                let _ = self.conn.call_method(Some(DEST), "/org/bluez", Some(MANAGER), "UnregisterProfile", &(path,));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_kind_of_phone_and_what_the_record_says() {
        assert_eq!(kind_of("bluetooth:v004Cp7A1Ad1A40"), Kind::IPhone);
        assert_eq!(kind_of("bluetooth:v00E0p1200d1436"), Kind::Other);
        assert_eq!(kind_of(""), Kind::Other);
        // The record: the Notification Server's class, RFCOMM on its
        // channel, OBEX, and the profile at 1.2.
        let r = mns_record();
        assert!(r.contains(r#"<uuid value="0x1133"/>"#) && r.contains(r#"<uuid value="0x0003"/><uint8 value="0x13"/>"#) && r.contains(r#"<uuid value="0x1134"/><uint16 value="0x0102"/>"#));
    }
}
