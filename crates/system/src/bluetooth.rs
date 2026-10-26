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
const AGENT_PATH: &str = "/org/tabula/bluez/agent";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Adapter {
    pub path: String,
    /// The name others see ("aurora").
    pub name: String,
    pub address: String,
    pub powered: bool,
    pub discoverable: bool,
    pub discovering: bool,
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
        status()
            .ok()
            .and_then(|s| s.devices.into_iter().find(|d| d.path == device.as_str()))
            .map_or_else(|| "The device".to_owned(), |d| d.name)
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
