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

use super::{map, music, pbap};
use crate::Result;

const DEST: &str = "org.bluez";
const DEVICE: &str = "org.bluez.Device1";
const MANAGER: &str = "org.bluez.ProfileManager1";
const ADVERTS: &str = "org.bluez.LEAdvertisingManager1";
/// Where Olo's advertisement soliciting an iPhone is served.
const ADVERT: &str = "/org/olo/phone/solicit";
/// Apple's Notification Center Service, which an advertisement solicits:
/// an iPhone shows its switches for a computer (Show Notifications, Sync
/// Contacts) only while the computer asks for it so.
const ANCS: &str = "7905f431-b5ce-4e99-a40f-4b1e122d00d0";
/// A service every iPhone lists (Apple's own), known before its
/// Modalias is.
const APPLE: &str = "00000000-deca-fade-deca-deafdecacafe";

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

    /// The phone's service a channel is to (what `ConnectProfile` is
    /// asked for).
    pub fn uuid(self) -> &'static str {
        match self {
            Channel::Messages => map::MAS_UUID,
            Channel::Contacts => pbap::PSE_UUID,
            Channel::Notifications => map::MNS_UUID,
        }
    }

    /// What it's registered with BlueZ as. The contacts: as the phone
    /// book's client (BlueZ then knows the phone's service to connect to
    /// from its own table, and publishes the client's service record: a
    /// phone offers its contacts to a computer that has one; an iPhone's
    /// Sync Contacts switch appears only then). Registered under the
    /// phone's own service, as a client, it published none (Ben,
    /// 2026-11-17: Show Notifications appeared, "but no sync contacts").
    fn registered_as(self) -> &'static str {
        match self {
            Channel::Contacts => pbap::PCE_UUID,
            other => other.uuid(),
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
/// good, [`Stream::closer`] ends it from another thread, and
/// [`Stream::gone`] waits for the other side to end it.
///
/// BlueZ hands its sockets over as it uses them itself: **non-blocking**
/// (the flag is the socket's, not the descriptor's), so a read here
/// would fail at once with "Resource temporarily unavailable" (Ben's
/// iPhone, 2026-11-17: every session ended so as it began); it's made
/// blocking, with the patience. And a phone that offers OBEX over L2CAP
/// (an Android; an iPhone offers RFCOMM alone) is connected to that way
/// by BlueZ: a socket of **packets**, each read one whole OBEX packet
/// and no more. Read here a packet at a time and handed on as a stream;
/// what's written is a whole packet a write already (`obex`).
pub struct Stream {
    file: std::fs::File,
    packets: bool,
    /// A packet read and not all taken yet.
    pending: Vec<u8>,
    at: usize,
}

impl Stream {
    /// A socket BlueZ handed over (or a stand-in's end of one).
    pub fn adopt(fd: OwnedFd) -> Stream {
        let file = std::fs::File::from(fd);
        let raw = file.as_raw_fd();
        let mut kind: libc::c_int = 0;
        let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
        // SAFETY: our own descriptor; an int of the size given.
        unsafe {
            let flags = libc::fcntl(raw, libc::F_GETFL);
            if flags >= 0 && flags & libc::O_NONBLOCK != 0 {
                libc::fcntl(raw, libc::F_SETFL, flags & !libc::O_NONBLOCK);
            }
            libc::getsockopt(raw, libc::SOL_SOCKET, libc::SO_TYPE, (&raw mut kind).cast(), &raw mut len);
        }
        let s = Stream { file, packets: kind == libc::SOCK_SEQPACKET, pending: Vec::new(), at: 0 };
        s.patience(Duration::from_secs(30));
        s
    }

    /// A link of packets (L2CAP), not a stream of bytes (RFCOMM).
    pub fn packets(&self) -> bool {
        self.packets
    }

    /// How long a read or a write waits (0: for good).
    pub fn patience(&self, wait: Duration) {
        let tv = libc::timeval { tv_sec: wait.as_secs() as libc::time_t, tv_usec: wait.subsec_micros() as libc::suseconds_t };
        for option in [libc::SO_RCVTIMEO, libc::SO_SNDTIMEO] {
            // SAFETY: a timeval of the size given, on our own socket.
            unsafe { libc::setsockopt(self.file.as_raw_fd(), libc::SOL_SOCKET, option, (&raw const tv).cast(), std::mem::size_of::<libc::timeval>() as libc::socklen_t) };
        }
    }

    /// Something that ends the channel from elsewhere (a read waiting on
    /// it returns, and [`Gone::wait`]).
    pub fn closer(&self) -> Closer {
        Closer(self.file.try_clone().ok())
    }

    /// Something that waits for the channel's end from elsewhere.
    pub fn gone(&self) -> Gone {
        Gone(self.file.try_clone().ok())
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

/// A channel's end, waited for ([`Stream::gone`]).
pub struct Gone(Option<std::fs::File>);

impl Gone {
    /// Asleep until the other side has closed the channel, the link under
    /// it has gone, or it was ended here ([`Closer::close`]): nothing is
    /// read, and what arrives meanwhile doesn't wake it. A session that
    /// waits to be asked something would otherwise learn its phone had
    /// closed the channel only when next it tried to use it.
    pub fn wait(&self) {
        let Some(f) = &self.0 else { return };
        let mut fd = libc::pollfd { fd: f.as_raw_fd(), events: libc::POLLRDHUP, revents: 0 };
        loop {
            // SAFETY: one pollfd of our own descriptor.
            let n = unsafe { libc::poll(&raw mut fd, 1, -1) };
            if n > 0 || (n < 0 && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted) {
                return;
            }
        }
    }
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if !self.packets {
            return self.file.read(buf);
        }
        if self.at == self.pending.len() {
            // (The largest an L2CAP packet gets.)
            self.pending.resize(0x1_0000, 0);
            self.at = 0;
            let n = self.file.read(&mut self.pending);
            self.pending.truncate(*n.as_ref().unwrap_or(&0));
            if n? == 0 {
                return Ok(0);
            }
        }
        let n = buf.len().min(self.pending.len() - self.at);
        buf[..n].copy_from_slice(&self.pending[self.at..self.at + n]);
        self.at += n;
        Ok(n)
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.file.write(buf)
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

/// What [`Profiles::watch`] says changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// Which devices there are, paired, connected, what they offer, or
    /// BlueZ itself: a phone may be reachable now.
    Link,
    /// A phone's battery or what it's playing: nothing about reaching
    /// it.
    Detail,
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
pub fn kind_of(modalias: &str, uuids: &[String]) -> Kind {
    let apple = modalias.to_ascii_lowercase().starts_with("bluetooth:v004c") || uuids.iter().any(|u| u.eq_ignore_ascii_case(APPLE));
    if apple { Kind::IPhone } else { Kind::Other }
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
        (self.opened)(Opened { channel: self.channel, phone: device.as_str().to_owned(), stream: Stream::adopt(OwnedFd::from(fd)) });
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
    /// The BlueZ they were last registered with (its name on the bus):
    /// a BlueZ started again has forgotten them ([`Profiles::ensure`]).
    with: std::sync::Mutex<String>,
}

/// Olo's advertisement to an iPhone ([`Profiles::solicit`]).
struct Advert {
    name: String,
}

#[zbus::interface(name = "org.bluez.LEAdvertisement1")]
impl Advert {
    fn release(&self) {}

    #[zbus(property, name = "Type")]
    fn kind(&self) -> String {
        "peripheral".into()
    }

    #[zbus(property, name = "SolicitUUIDs")]
    fn solicits(&self) -> Vec<String> {
        vec![ANCS.into()]
    }

    /// (The company id kept for tests, 0xFFFF, and "Olo": it claims to be
    /// nobody's product. With the flags and the solicited service it
    /// fits a legacy advertisement's 31 bytes.)
    #[zbus(property)]
    fn manufacturer_data(&self) -> HashMap<u16, OwnedValue> {
        OwnedValue::try_from(Value::from(b"Olo".to_vec())).map(|v| HashMap::from([(0xFFFF, v)])).unwrap_or_default()
    }

    #[zbus(property)]
    fn discoverable(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn discoverable_timeout(&self) -> u16 {
        180
    }

    #[zbus(property)]
    fn local_name(&self) -> String {
        self.name.clone()
    }
}

/// The advertisement, broadcast for as long as this is kept.
pub struct Solicit {
    conn: Connection,
    adapter: String,
}

impl Drop for Solicit {
    fn drop(&mut self) {
        if let Ok(path) = zvariant::ObjectPath::try_from(ADVERT) {
            let _ = self.conn.call_method(Some(DEST), self.adapter.as_str(), Some(ADVERTS), "UnregisterAdvertisement", &(path,));
        }
        let _ = self.conn.object_server().remove::<Advert, _>(ADVERT);
    }
}

/// Tell BlueZ of the three profiles (their objects are served already).
fn announce(conn: &Connection) -> Result<()> {
    for channel in Channel::ALL {
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
            Channel::Messages => {
                options.insert("Name", "Olo Messages".into());
                options.insert("Role", "client".into());
            }
            // (No role: a client that also has its service record, which
            // listens on nothing.)
            Channel::Contacts => {
                options.insert("Name", "Olo Contacts".into());
            }
        }
        let path = zvariant::ObjectPath::try_from(channel.path())?;
        conn.call_method(Some(DEST), "/org/bluez", Some(MANAGER), "RegisterProfile", &(path, channel.registered_as(), options))?;
    }
    Ok(())
}

/// BlueZ's own name on the bus now (another after it's started again).
fn bluez(conn: &Connection) -> Result<String> {
    let dbus = zbus::blocking::fdo::DBusProxy::new(conn)?;
    Ok(dbus.get_name_owner(zbus::names::BusName::try_from(DEST)?)?.to_string())
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
        }
        announce(&conn)?;
        let with = std::sync::Mutex::new(bluez(&conn)?);
        Ok(Profiles { conn, with })
    }

    /// See that the BlueZ running now has them: one started again since
    /// (an update, a setting changed, a crash) has forgotten every
    /// profile, and says nothing of it. True: they were registered
    /// again (every channel that was open is gone). An error: BlueZ
    /// isn't there, or won't take them.
    pub fn ensure(&self) -> Result<bool> {
        let now = bluez(&self.conn)?;
        let mut with = self.with.lock().unwrap_or_else(|e| e.into_inner());
        if *with == now {
            return Ok(false);
        }
        announce(&self.conn)?;
        *with = now;
        Ok(true)
    }

    /// Advertise to `phone` (an iPhone, paired and connected) that this
    /// computer would like its notifications: what has an iPhone show its
    /// switches for this computer, Show Notifications and Sync Contacts
    /// (BlueFerry's notes: on iOS 26 they appeared only while this was
    /// broadcast, never for asking for its messages alone). A low-energy
    /// advertisement of the adapter's, which BlueZ makes discoverable
    /// for three minutes; nothing is connected to. Kept for as long as
    /// what's returned is.
    pub fn solicit(&self, phone: &str) -> Result<Solicit> {
        let adapter = phone.rsplit_once('/').map_or("/org/bluez/hci0", |(a, _)| a).to_owned();
        let name = self
            .conn
            .call_method(Some(DEST), adapter.as_str(), Some("org.freedesktop.DBus.Properties"), "Get", &("org.bluez.Adapter1", "Alias"))
            .ok()
            .and_then(|m| m.body().deserialize::<OwnedValue>().ok())
            .and_then(|v| String::try_from(v).ok())
            .unwrap_or_else(|| "Olo".into());
        let _ = self.conn.object_server().remove::<Advert, _>(ADVERT);
        self.conn.object_server().at(ADVERT, Advert { name })?;
        let options: HashMap<&str, Value> = HashMap::new();
        self.conn.call_method(Some(DEST), adapter.as_str(), Some(ADVERTS), "RegisterAdvertisement", &(zvariant::ObjectPath::try_from(ADVERT)?, options))?;
        Ok(Solicit { conn: self.conn.clone(), adapter })
    }

    /// Ask BlueZ for a channel to `phone`'s messages or contacts: it
    /// comes to `opened` when it's open. An error: the phone isn't in
    /// reach, doesn't offer it, or refused.
    pub fn open(&self, phone: &str, channel: Channel) -> Result<()> {
        self.conn.call_method(Some(DEST), phone, Some(DEVICE), "ConnectProfile", &(channel.uuid(),)).map(|_| ())
    }

    /// Have BlueZ let go of what it holds of a channel to `phone`: after
    /// it said yes to [`Profiles::open`] and no channel came (it took
    /// the profile for connected already, and answers so without opening
    /// anything), so the next asking starts afresh. An error (it held
    /// nothing) is nothing.
    pub fn let_go(&self, phone: &str, channel: Channel) {
        let _ = self.conn.call_method(Some(DEST), phone, Some(DEVICE), "DisconnectProfile", &(channel.uuid(),));
    }

    /// Whether BlueZ has a Bluetooth adapter now. (With none, its
    /// devices aren't listed either: a phone missing then hasn't been
    /// unpaired.)
    pub fn has_adapter(&self) -> bool {
        type Objects = HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>;
        self.conn
            .call_method(Some(DEST), "/", Some("org.freedesktop.DBus.ObjectManager"), "GetManagedObjects", &())
            .ok()
            .and_then(|m| m.body().deserialize::<Objects>().ok())
            .is_some_and(|objects| objects.values().any(|interfaces| interfaces.contains_key("org.bluez.Adapter1")))
    }

    /// What `phone` is playing ([`music::music`]).
    pub fn music(&self, phone: &str) -> Option<music::Music> {
        music::music(&self.conn, phone)
    }

    /// Have the phone's `player` play, pause or skip.
    pub fn music_act(&self, player: &str, action: music::Action) -> Result<()> {
        music::act(&self.conn, player, action)
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
                    kind: kind_of(&text("Modalias"), &uuids),
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
    /// goes, connects, disconnects, pairs or says what it offers, and
    /// when BlueZ itself goes or comes (started again: see
    /// [`Profiles::ensure`]): [`Change::Link`]; and whenever a phone's
    /// battery or what it's playing changes (the track, playing or not:
    /// never the position, which moves all the while):
    /// [`Change::Detail`], which says nothing of whether the phone can
    /// be reached. Nothing is polled.
    pub fn watch(&self, changed: impl Fn(Change) + Send + 'static) -> Result<()> {
        use zbus::{MatchRule, message::Type};
        const TOLD: &[&str] = &["Connected", "Paired", "UUIDs", "Alias", "ServicesResolved", "Modalias"];
        let conn = bus()?;
        let dbus = zbus::blocking::fdo::DBusProxy::new(&conn)?;
        let messages = zbus::blocking::MessageIterator::from(&conn);
        dbus.add_match_rule(MatchRule::builder().msg_type(Type::Signal).sender(DEST)?.interface("org.freedesktop.DBus.ObjectManager")?.build())?;
        dbus.add_match_rule(MatchRule::builder().msg_type(Type::Signal).sender(DEST)?.interface("org.freedesktop.DBus.Properties")?.member("PropertiesChanged")?.build())?;
        dbus.add_match_rule(MatchRule::builder().msg_type(Type::Signal).sender("org.freedesktop.DBus")?.interface("org.freedesktop.DBus")?.member("NameOwnerChanged")?.arg(0, DEST)?.build())?;
        std::thread::Builder::new().name("phone-bluez".into()).spawn(move || {
            let _keep = (conn, dbus);
            for m in messages.flatten() {
                if m.message_type() != Type::Signal {
                    continue;
                }
                let tell = match m.header().member().map(|m| m.as_str().to_owned()).as_deref() {
                    Some("InterfacesAdded" | "InterfacesRemoved") => Some(Change::Link),
                    Some("NameOwnerChanged") => m.body().deserialize::<(String, String, String)>().is_ok_and(|(name, _, _)| name == DEST).then_some(Change::Link),
                    Some("PropertiesChanged") => m.body().deserialize::<(String, HashMap<String, OwnedValue>, Vec<String>)>().ok().and_then(|(iface, props, gone)| {
                        let (told, change): (&[&str], Change) = match iface.as_str() {
                            DEVICE => (TOLD, Change::Link),
                            music::PLAYER => (&["Status", "Track"], Change::Detail),
                            "org.bluez.MediaControl1" => (&["Player", "Connected"], Change::Detail),
                            "org.bluez.Battery1" => (&["Percentage"], Change::Detail),
                            _ => return None,
                        };
                        props.keys().chain(gone.iter()).any(|k| told.contains(&k.as_str())).then_some(change)
                    }),
                    _ => None,
                };
                if let Some(change) = tell {
                    changed(change);
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
        assert_eq!(kind_of("bluetooth:v004Cp7A1Ad1A40", &[]), Kind::IPhone);
        assert_eq!(kind_of("bluetooth:v00E0p1200d1436", &[]), Kind::Other);
        assert_eq!(kind_of("", &[]), Kind::Other);
        // (Before its Modalias is read: Apple's own service among what
        // it offers.)
        assert_eq!(kind_of("", &["00000000-DECA-FADE-DECA-DEAFDECACAFE".into()]), Kind::IPhone);
        // The record: the Notification Server's class, RFCOMM on its
        // channel, OBEX, and the profile at 1.2.
        let r = mns_record();
        assert!(r.contains(r#"<uuid value="0x1133"/>"#) && r.contains(r#"<uuid value="0x0003"/><uint8 value="0x13"/>"#) && r.contains(r#"<uuid value="0x1134"/><uint16 value="0x0102"/>"#));
    }
}
