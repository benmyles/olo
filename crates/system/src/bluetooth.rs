//! BlueZ: the adapter, devices nearby and paired, pairing (with an
//! agent for passkeys and PINs), connecting and forgetting.
//!
//! The agent is registered for this process only (never as the default
//! agent), so BlueZ uses it for pairings we start and leaves any other
//! desktop's agent alone.

use std::collections::HashMap;

use zbus::blocking::Connection;
use zvariant::{OwnedObjectPath, OwnedValue};

use crate::{Result, bus};

const DEST: &str = "org.bluez";
const ADAPTER: &str = "org.bluez.Adapter1";
const DEVICE: &str = "org.bluez.Device1";
const BATTERY: &str = "org.bluez.Battery1";
const AGENT_PATH: &str = "/com/olodesktop/bluez/agent";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Adapter {
    pub path: String,
    /// The name others see ("aurora").
    pub name: String,
    pub address: String,
    pub powered: bool,
    /// BlueZ's word for the controller's power ("on", "off",
    /// "off-enabling", "on-disabling", "off-blocked").
    pub power_state: String,
    pub discoverable: bool,
    pub discovering: bool,
}

impl Adapter {
    /// The controller doesn't answer: BlueZ has it powered and its power
    /// off, or the other way round, which isn't a change under way (those
    /// are "off-enabling" and "on-disabling"). So a controller looks whose
    /// driver fails every command (Intel's `btintel_pcie` with ENODEV,
    /// 2026-11-09): nothing connects, AirPods sit at Connecting….
    pub fn not_answering(&self) -> bool {
        (self.powered && self.power_state == "off") || (!self.powered && self.power_state == "on")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub path: String,
    pub name: String,
    pub address: String,
    /// freedesktop icon name BlueZ derives from the class ("audio-headset").
    pub icon: String,
    /// The device told us its name (unnamed ones are mostly anonymous
    /// beacons and phones nearby, not worth listing).
    pub named: bool,
    pub paired: bool,
    pub connected: bool,
    pub trusted: bool,
    /// Signal strength while discovering (dBm).
    pub rssi: Option<i16>,
    /// Battery level, for devices that report it.
    pub battery: Option<u8>,
    /// Its vendor and product, as BlueZ has them
    /// ("bluetooth:v004Cp2024d1D0A": Apple's AirPods Pro).
    pub modalias: Option<String>,
}

impl Device {
    /// The kind of device, in words.
    pub fn kind(&self) -> &'static str {
        match self.icon.as_str() {
            "audio-headset" | "audio-headphones" => "Headphones",
            "audio-card" => "Speaker",
            "input-keyboard" => "Keyboard",
            "input-mouse" => "Mouse",
            "input-tablet" => "Tablet",
            "input-gaming" => "Game controller",
            "phone" => "Phone",
            "computer" => "Computer",
            "printer" => "Printer",
            "camera-photo" | "camera-video" => "Camera",
            "network-wireless" => "Network",
            _ => "Device",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Status {
    pub adapter: Option<Adapter>,
    /// Paired first, then by name.
    pub devices: Vec<Device>,
}

type Objects = HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>;

fn get<T: TryFrom<OwnedValue>>(props: &HashMap<String, OwnedValue>, key: &str) -> Option<T> {
    props.get(key).and_then(|v| T::try_from(v.try_clone().ok()?).ok())
}

/// The first adapter and every device BlueZ knows about. Ok with no
/// adapter when there's no Bluetooth; Err when BlueZ isn't running.
pub fn status() -> Result<Status> {
    let objects: Objects = bus::call(bus::system()?, DEST, "/", "org.freedesktop.DBus.ObjectManager", "GetManagedObjects", &())?;
    let mut paths: Vec<&OwnedObjectPath> = objects.keys().collect();
    paths.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    let adapter = paths.iter().find_map(|p| {
        let a = objects[*p].get(ADAPTER)?;
        Some(Adapter {
            path: p.as_str().to_owned(),
            name: get::<String>(a, "Alias").or_else(|| get(a, "Name")).unwrap_or_default(),
            address: get(a, "Address").unwrap_or_default(),
            powered: get(a, "Powered").unwrap_or(false),
            power_state: get(a, "PowerState").unwrap_or_default(),
            discoverable: get(a, "Discoverable").unwrap_or(false),
            discovering: get(a, "Discovering").unwrap_or(false),
        })
    });
    let mut devices: Vec<Device> = paths
        .iter()
        .filter(|p| adapter.as_ref().is_some_and(|a| p.as_str().starts_with(&format!("{}/", a.path))))
        .filter_map(|p| {
            let ifaces = &objects[*p];
            let d = ifaces.get(DEVICE)?;
            let address: String = get(d, "Address").unwrap_or_default();
            Some(Device {
                path: p.as_str().to_owned(),
                name: get::<String>(d, "Alias").or_else(|| get(d, "Name")).unwrap_or_else(|| address.clone()),
                address,
                named: d.contains_key("Name"),
                icon: get(d, "Icon").unwrap_or_default(),
                paired: get(d, "Paired").unwrap_or(false),
                connected: get(d, "Connected").unwrap_or(false),
                trusted: get(d, "Trusted").unwrap_or(false),
                rssi: get(d, "RSSI"),
                battery: ifaces.get(BATTERY).and_then(|b| get(b, "Percentage")),
                modalias: get(d, "Modalias"),
            })
        })
        .collect();
    devices.sort_by(|a, b| b.paired.cmp(&a.paired).then(b.connected.cmp(&a.connected)).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    Ok(Status { adapter, devices })
}

pub fn set_powered(adapter: &str, on: bool) -> Result<()> {
    bus::set_property(bus::system()?, DEST, adapter, ADAPTER, "Powered", on)
}

pub fn set_discoverable(adapter: &str, on: bool) -> Result<()> {
    bus::set_property(bus::system()?, DEST, adapter, ADAPTER, "Discoverable", on)
}

/// Look for devices until [`stop_discovery`] (or this process ends).
pub fn start_discovery(adapter: &str) -> Result<()> {
    bus::call::<_, ()>(bus::system()?, DEST, adapter, ADAPTER, "StartDiscovery", &())
}

pub fn stop_discovery(adapter: &str) -> Result<()> {
    bus::call::<_, ()>(bus::system()?, DEST, adapter, ADAPTER, "StopDiscovery", &())
}

/// Pair with a device (asking through the agent if it wants a passkey),
/// then trust it so it may reconnect by itself. Blocks until done.
pub fn pair(device: &str) -> Result<()> {
    let c = bus::system()?;
    bus::call::<_, ()>(c, DEST, device, DEVICE, "Pair", &())?;
    bus::set_property(c, DEST, device, DEVICE, "Trusted", true)
}

pub fn connect(device: &str) -> Result<()> {
    bus::call::<_, ()>(bus::system()?, DEST, device, DEVICE, "Connect", &())
}

pub fn disconnect(device: &str) -> Result<()> {
    bus::call::<_, ()>(bus::system()?, DEST, device, DEVICE, "Disconnect", &())
}

pub fn cancel_pairing(device: &str) -> Result<()> {
    bus::call::<_, ()>(bus::system()?, DEST, device, DEVICE, "CancelPairing", &())
}

/// Forget a device (unpair and drop it).
pub fn remove(adapter: &str, device: &str) -> Result<()> {
    let path = zvariant::ObjectPath::try_from(device)?;
    bus::call::<_, ()>(bus::system()?, DEST, adapter, ADAPTER, "RemoveDevice", &(path,))
}

// ---- the agent -----------------------------------------------------------------

/// What a device being paired asks of the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    /// Does the device show this number too?
    Confirm(u32),
    /// Type this number on the device (a keyboard), then Return. Needs no
    /// answer: it stays up until the pairing ends.
    ShowPasskey(u32),
    /// Type this PIN on the device.
    ShowPin(String),
    /// The device needs a PIN typed here (old devices: often 0000).
    EnterPin,
    /// The device needs a passkey typed here.
    EnterPasskey,
    /// May the device pair (it asked, not us)?
    Authorize,
}

/// A question from BlueZ, answered with [`Question::answer`]. Dropping
/// it unanswered rejects.
pub struct Question {
    /// The device's name, and its path.
    pub device: String,
    pub path: String,
    pub ask: Ask,
    reply: Option<async_channel::Sender<Option<String>>>,
}

impl Question {
    /// `None` rejects; `Some(text)` accepts (the PIN or passkey typed, or
    /// anything for questions that only want a yes).
    pub fn answer(mut self, answer: Option<String>) {
        if let Some(r) = self.reply.take() {
            let _ = r.try_send(answer);
        }
    }
}

impl Drop for Question {
    fn drop(&mut self) {
        if let Some(r) = self.reply.take() {
            let _ = r.try_send(None);
        }
    }
}

pub enum AgentEvent {
    Ask(Question),
    /// BlueZ withdrew its question (the device gave up).
    Cancel,
}

#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.bluez.Error")]
enum AgentError {
    #[zbus(error)]
    ZBus(zbus::Error),
    Rejected(String),
    Canceled(String),
}

struct AgentObject {
    on: Box<dyn Fn(AgentEvent) + Send + Sync>,
}

impl AgentObject {
    fn name(device: &OwnedObjectPath) -> String {
        status().ok().and_then(|s| s.devices.into_iter().find(|d| d.path == device.as_str())).map_or_else(|| "The device".to_owned(), |d| d.name)
    }

    /// Show something that needs no answer (a code to type on the device).
    fn show(&self, device: &OwnedObjectPath, ask: Ask) {
        (self.on)(AgentEvent::Ask(Question { device: Self::name(device), path: device.as_str().to_owned(), ask, reply: None }));
    }

    async fn ask(&self, device: &OwnedObjectPath, ask: Ask) -> std::result::Result<Option<String>, AgentError> {
        let (tx, rx) = async_channel::bounded(1);
        (self.on)(AgentEvent::Ask(Question { device: Self::name(device), path: device.as_str().to_owned(), ask, reply: Some(tx) }));
        match rx.recv().await {
            Ok(Some(a)) => Ok(Some(a)),
            Ok(None) => Err(AgentError::Rejected("Rejected".into())),
            Err(_) => Err(AgentError::Canceled("Canceled".into())),
        }
    }
}

#[zbus::interface(name = "org.bluez.Agent1")]
impl AgentObject {
    fn release(&self) {}

    async fn request_pin_code(&self, device: OwnedObjectPath) -> std::result::Result<String, AgentError> {
        Ok(self.ask(&device, Ask::EnterPin).await?.unwrap_or_default())
    }

    fn display_pin_code(&self, device: OwnedObjectPath, pincode: String) {
        self.show(&device, Ask::ShowPin(pincode));
    }

    async fn request_passkey(&self, device: OwnedObjectPath) -> std::result::Result<u32, AgentError> {
        let a = self.ask(&device, Ask::EnterPasskey).await?.unwrap_or_default();
        a.trim().parse().map_err(|_| AgentError::Rejected("Not a number".into()))
    }

    fn display_passkey(&self, device: OwnedObjectPath, passkey: u32, entered: u16) {
        // Called again as each digit is typed; show it once.
        if entered == 0 {
            self.show(&device, Ask::ShowPasskey(passkey));
        }
    }

    async fn request_confirmation(&self, device: OwnedObjectPath, passkey: u32) -> std::result::Result<(), AgentError> {
        self.ask(&device, Ask::Confirm(passkey)).await.map(|_| ())
    }

    async fn request_authorization(&self, device: OwnedObjectPath) -> std::result::Result<(), AgentError> {
        self.ask(&device, Ask::Authorize).await.map(|_| ())
    }

    async fn authorize_service(&self, _device: OwnedObjectPath, _uuid: String) -> std::result::Result<(), AgentError> {
        // A paired, trusted device may use its services.
        Ok(())
    }

    fn cancel(&self) {
        (self.on)(AgentEvent::Cancel);
    }
}

/// A registered pairing agent; dropping it unregisters.
pub struct Agent {
    conn: &'static Connection,
}

impl Agent {
    /// Register an agent for pairings this process starts.
    pub fn register(on: impl Fn(AgentEvent) + Send + Sync + 'static) -> Result<Agent> {
        let conn = bus::system()?;
        conn.object_server().at(AGENT_PATH, AgentObject { on: Box::new(on) })?;
        let path = zvariant::ObjectPath::try_from(AGENT_PATH)?;
        bus::call::<_, ()>(conn, DEST, "/org/bluez", "org.bluez.AgentManager1", "RegisterAgent", &(path, "KeyboardDisplay"))?;
        Ok(Agent { conn })
    }
}

impl Drop for Agent {
    fn drop(&mut self) {
        if let Ok(path) = zvariant::ObjectPath::try_from(AGENT_PATH) {
            let _ = bus::call::<_, ()>(self.conn, DEST, "/org/bluez", "org.bluez.AgentManager1", "UnregisterAgent", &(path,));
        }
        let _ = self.conn.object_server().remove::<AgentObject, _>(AGENT_PATH);
    }
}

/// Rename a device as this computer knows it (BlueZ's alias).
pub fn set_alias(device: &str, name: &str) -> Result<()> {
    bus::set_property(bus::system()?, DEST, device, DEVICE, "Alias", name)
}

/// Call `changed` (on a thread of its own) whenever BlueZ's devices or
/// adapters come, go, connect, disconnect, pair or are renamed; a burst
/// is told once. Signal strengths and other chatter while searching
/// aren't told. Nothing is polled: the thread sleeps until BlueZ says
/// something.
pub fn watch(changed: impl Fn() + Send + 'static) -> Result<()> {
    use zbus::{MatchRule, message::Type};
    const TOLD: &[&str] = &["Connected", "Paired", "Alias", "Name", "Modalias", "Trusted", "Powered", "ServicesResolved"];
    let conn = Connection::system()?;
    let dbus = zbus::blocking::fdo::DBusProxy::new(&conn)?;
    let messages = zbus::blocking::MessageIterator::from(&conn);
    let objects = MatchRule::builder().msg_type(Type::Signal).sender(DEST)?.interface("org.freedesktop.DBus.ObjectManager")?.build();
    let props = MatchRule::builder().msg_type(Type::Signal).sender(DEST)?.interface("org.freedesktop.DBus.Properties")?.member("PropertiesChanged")?.build();
    dbus.add_match_rule(objects)?;
    dbus.add_match_rule(props)?;
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    std::thread::Builder::new().name("bluez-signals".into()).spawn(move || {
        let _keep = (conn, dbus);
        for m in messages.flatten() {
            if m.message_type() != Type::Signal {
                continue;
            }
            let h = m.header();
            let tell = match h.member().map(|m| m.as_str()) {
                Some("InterfacesAdded" | "InterfacesRemoved") => true,
                Some("PropertiesChanged") => m.body().deserialize::<(String, HashMap<String, OwnedValue>, Vec<String>)>().is_ok_and(|(iface, props, gone)| {
                    (iface == DEVICE || iface == ADAPTER || iface == BATTERY) && props.keys().chain(gone.iter()).any(|k| TOLD.contains(&k.as_str()) || iface == BATTERY)
                }),
                _ => false,
            };
            if tell && tx.send(()).is_err() {
                return;
            }
        }
    })?;
    std::thread::Builder::new().name("bluez".into()).spawn(move || {
        while rx.recv().is_ok() {
            // Let a burst finish (a connection changes several properties).
            std::thread::sleep(std::time::Duration::from_millis(60));
            while rx.try_recv().is_ok() {}
            changed();
        }
    })?;
    Ok(())
}

// ---- Apple's broadcasts --------------------------------------------------------

/// One of Apple's broadcasts heard: the address it came from (a private
/// one that changes, for AirPods), Apple's manufacturer data after the
/// company id, and how strong.
#[derive(Debug, Clone)]
pub struct Advert {
    pub address: String,
    pub data: Vec<u8>,
    pub rssi: Option<i16>,
}

/// What asks BlueZ to listen for one kind of Apple broadcast: its
/// AdvertisementMonitor1 (served at /org/olo/monitor/apple).
struct AppleMonitor {
    kind: u8,
}

#[zbus::interface(name = "org.bluez.AdvertisementMonitor1")]
impl AppleMonitor {
    fn release(&self) {}
    fn activate(&self) {}
    fn device_found(&self, _device: zvariant::ObjectPath<'_>) {}
    fn device_lost(&self, _device: zvariant::ObjectPath<'_>) {}
    #[zbus(property, name = "Type")]
    fn kind(&self) -> String {
        "or_patterns".into()
    }
    /// Manufacturer data (0xff) starting, at 0, with Apple's company id
    /// (0x004C, little-endian) and the message's kind.
    #[zbus(property)]
    fn patterns(&self) -> Vec<(u8, u8, Vec<u8>)> {
        vec![(0, 0xff, vec![0x4c, 0x00, self.kind])]
    }
}

/// Listen, passively, for Apple's broadcasts of `kind` (0x07: AirPods'
/// proximity message) and tell `heard` each one, from BlueZ's thread, for
/// as long as the returned connection is kept. The radio scans as BlueZ
/// does for any monitor: passively, with the controller filtering where it
/// can. Needs BlueZ's advertisement monitor (`bluetoothd --experimental`
/// on BlueZ 5.87): without it, an error that says so.
pub fn watch_apple(kind: u8, heard: impl Fn(Advert) + Send + 'static) -> Result<Connection> {
    use zbus::{MatchRule, message::Type};
    let adapter = status()?.adapter.ok_or_else(|| zbus::Error::Failure("no Bluetooth adapter".into()))?.path;
    let root = "/org/olo/monitor";
    let conn = zbus::blocking::connection::Builder::system()?.serve_at(root, zbus::fdo::ObjectManager)?.serve_at(format!("{root}/apple"), AppleMonitor { kind })?.build()?;
    let manager = conn.call_method(Some(DEST), adapter.as_str(), Some("org.bluez.AdvertisementMonitorManager1"), "RegisterMonitor", &(zvariant::ObjectPath::try_from(root)?,));
    if let Err(e) = manager {
        let unknown = matches!(&e, zbus::Error::MethodError(name, ..) if name.as_str().ends_with("UnknownMethod") || name.as_str().ends_with("UnknownInterface"));
        return Err(if unknown { zbus::Error::Failure("BlueZ's advertisement monitor is off (bluetoothd --experimental)".into()) } else { e });
    }
    // BlueZ makes and updates a device for each broadcast that matches:
    // follow its ManufacturerData.
    let listen = Connection::system()?;
    let dbus = zbus::blocking::fdo::DBusProxy::new(&listen)?;
    let messages = zbus::blocking::MessageIterator::from(&listen);
    dbus.add_match_rule(MatchRule::builder().msg_type(Type::Signal).sender(DEST)?.interface("org.freedesktop.DBus.ObjectManager")?.member("InterfacesAdded")?.build())?;
    dbus.add_match_rule(MatchRule::builder().msg_type(Type::Signal).sender(DEST)?.interface("org.freedesktop.DBus.Properties")?.member("PropertiesChanged")?.build())?;
    // Where each device object came from (its Address), looked up once.
    let mut addresses: HashMap<String, String> = HashMap::new();
    let apple = move |props: &HashMap<String, OwnedValue>| -> Option<Vec<u8>> {
        let m = props.get("ManufacturerData")?;
        let map = HashMap::<u16, OwnedValue>::try_from(m.try_clone().ok()?).ok()?;
        let data = Vec::<u8>::try_from(map.get(&0x004c)?.try_clone().ok()?).ok()?;
        (data.first() == Some(&kind)).then_some(data)
    };
    std::thread::Builder::new().name("bluez-adverts".into()).spawn(move || {
        let _keep = &dbus;
        for m in messages.flatten() {
            let h = m.header();
            let (path, props) = match h.member().map(|m| m.as_str()) {
                Some("InterfacesAdded") => {
                    let Ok((path, mut ifaces)) = m.body().deserialize::<(OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>)>() else { continue };
                    let Some(props) = ifaces.remove(DEVICE) else { continue };
                    if let Some(a) = props.get("Address").and_then(|v| String::try_from(v.try_clone().ok()?).ok()) {
                        addresses.insert(path.to_string(), a);
                    }
                    (path.to_string(), props)
                }
                Some("PropertiesChanged") => {
                    let Some(path) = h.path().map(|p| p.to_string()) else { continue };
                    let Ok((iface, props, _)) = m.body().deserialize::<(String, HashMap<String, OwnedValue>, Vec<String>)>() else { continue };
                    if iface != DEVICE {
                        continue;
                    }
                    (path, props)
                }
                _ => continue,
            };
            let Some(data) = apple(&props) else { continue };
            let address = match addresses.get(&path) {
                Some(a) => a.clone(),
                None => {
                    let got = listen
                        .call_method(Some(DEST), path.as_str(), Some("org.freedesktop.DBus.Properties"), "Get", &(DEVICE, "Address"))
                        .ok()
                        .and_then(|r| r.body().deserialize::<OwnedValue>().ok())
                        .and_then(|v| String::try_from(v).ok());
                    let Some(a) = got else { continue };
                    addresses.insert(path.clone(), a.clone());
                    a
                }
            };
            let rssi = props.get("RSSI").and_then(|v| i16::try_from(v).ok());
            heard(Advert { address, data, rssi });
        }
    })?;
    Ok(conn)
}
