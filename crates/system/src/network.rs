//! NetworkManager: devices and their addresses, Wi-Fi networks in range
//! and saved, and joining, leaving and forgetting them.

use std::collections::HashMap;

use zvariant::{OwnedObjectPath, OwnedValue, Value};

use crate::{Result, bus};

const DEST: &str = "org.freedesktop.NetworkManager";
const NM_PATH: &str = "/org/freedesktop/NetworkManager";
const NM: &str = "org.freedesktop.NetworkManager";
const DEVICE: &str = "org.freedesktop.NetworkManager.Device";
const WIRELESS: &str = "org.freedesktop.NetworkManager.Device.Wireless";
const WIRED: &str = "org.freedesktop.NetworkManager.Device.Wired";
const AP: &str = "org.freedesktop.NetworkManager.AccessPoint";
const IP4: &str = "org.freedesktop.NetworkManager.IP4Config";
const ACTIVE: &str = "org.freedesktop.NetworkManager.Connection.Active";
const SETTINGS_PATH: &str = "/org/freedesktop/NetworkManager/Settings";
const SETTINGS: &str = "org.freedesktop.NetworkManager.Settings";
const CONNECTION: &str = "org.freedesktop.NetworkManager.Settings.Connection";

/// NM_DEVICE_TYPE_*.
const TYPE_ETHERNET: u32 = 1;
const TYPE_WIFI: u32 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wifi {
    /// None when the adapter isn't associated with a network.
    pub ssid: Option<String>,
    /// 0–100.
    pub strength: u32,
}

/// The first Wi-Fi adapter's state, or None if there is no Wi-Fi adapter.
pub fn wifi() -> Result<Option<Wifi>> {
    let conn = bus::system()?;
    for dev in devices_paths()? {
        let kind: u32 = bus::property(conn, DEST, dev.as_str(), DEVICE, "DeviceType")?;
        if kind != TYPE_WIFI {
            continue;
        }
        let ap: OwnedObjectPath = bus::property(conn, DEST, dev.as_str(), WIRELESS, "ActiveAccessPoint")?;
        if ap.as_str() == "/" {
            return Ok(Some(Wifi { ssid: None, strength: 0 }));
        }
        let ssid: Vec<u8> = bus::property(conn, DEST, ap.as_str(), AP, "Ssid")?;
        let strength: u8 = bus::property(conn, DEST, ap.as_str(), AP, "Strength")?;
        return Ok(Some(Wifi { ssid: Some(String::from_utf8_lossy(&ssid).into_owned()), strength: strength.min(100) as u32 }));
    }
    Ok(None)
}

fn devices_paths() -> Result<Vec<OwnedObjectPath>> {
    bus::property(bus::system()?, DEST, NM_PATH, NM, "Devices")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Wifi,
    Ethernet,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub path: String,
    /// "wlan0".
    pub interface: String,
    pub kind: Kind,
    /// NM_DEVICE_STATE_* (100 is connected).
    pub state: u32,
    /// Why it last changed state (NM_DEVICE_STATE_REASON_*).
    pub reason: u32,
    pub hw_address: String,
    /// The connection in use ("Home Wi-Fi").
    pub connection: Option<String>,
    /// "192.168.1.20/24".
    pub addresses: Vec<String>,
    pub gateway: Option<String>,
    pub dns: Vec<String>,
    /// Mb/s, when known.
    pub speed: Option<u32>,
}

impl Device {
    pub fn connected(&self) -> bool {
        self.state == 100
    }

    /// The state in words.
    pub fn state_text(&self) -> &'static str {
        match self.state {
            10 => "Not managed",
            20 => "Unavailable",
            30 => "Not connected",
            40 | 50 => "Connecting…",
            60 => "Waiting for a password…",
            70 | 80 | 90 => "Getting an address…",
            100 => "Connected",
            110 => "Disconnecting…",
            120 => "Couldn't connect",
            _ => "Unknown",
        }
    }

    /// Why the last attempt failed, when that is worth saying.
    pub fn failure(&self) -> Option<&'static str> {
        match self.reason {
            7 | 8 => Some("The password wasn't accepted."),
            11 | 53 => Some("The network didn't answer."),
            5 | 6 | 17 => Some("No address was given out."),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Network {
    pub ssid: String,
    /// 0–100, the strongest access point seen for it.
    pub strength: u8,
    /// Needs a password (WPA/WPA2/WPA3/WEP).
    pub secure: bool,
    /// Enterprise (802.1X): needs more than a password.
    pub enterprise: bool,
    /// The access point to join (the strongest).
    pub ap: String,
    pub active: bool,
    /// There's a saved connection for it.
    pub known: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Status {
    pub wireless_enabled: bool,
    /// The radio's hardware switch allows it.
    pub wireless_hardware: bool,
    pub devices: Vec<Device>,
    /// Networks in range, strongest first; the one in use first of all.
    pub networks: Vec<Network>,
}

fn str_prop(dev: &str, iface: &str, name: &str) -> String {
    bus::system().ok().and_then(|c| bus::property::<String>(c, DEST, dev, iface, name).ok()).unwrap_or_default()
}

fn device(path: &OwnedObjectPath) -> Result<Device> {
    let c = bus::system()?;
    let p = path.as_str();
    let kind = match bus::property::<u32>(c, DEST, p, DEVICE, "DeviceType")? {
        TYPE_WIFI => Kind::Wifi,
        TYPE_ETHERNET => Kind::Ethernet,
        _ => Kind::Other,
    };
    let (state, reason): (u32, u32) = bus::property::<(u32, u32)>(c, DEST, p, DEVICE, "StateReason")?;
    let hw_iface = match kind {
        Kind::Wifi => WIRELESS,
        Kind::Ethernet => WIRED,
        Kind::Other => DEVICE,
    };
    let mut hw_address = str_prop(p, hw_iface, if kind == Kind::Other { "HwAddress" } else { "PermHwAddress" });
    if hw_address.is_empty() {
        hw_address = str_prop(p, hw_iface, "HwAddress");
    }
    let speed = match kind {
        Kind::Wifi => bus::property::<u32>(c, DEST, p, WIRELESS, "Bitrate").ok().map(|kbit| kbit / 1000),
        Kind::Ethernet => bus::property::<u32>(c, DEST, p, WIRED, "Speed").ok(),
        Kind::Other => None,
    }
    .filter(|s| *s > 0);
    let active: OwnedObjectPath = bus::property(c, DEST, p, DEVICE, "ActiveConnection")?;
    let connection = (active.as_str() != "/").then(|| str_prop(active.as_str(), ACTIVE, "Id")).filter(|s| !s.is_empty());
    let ip4: OwnedObjectPath = bus::property(c, DEST, p, DEVICE, "Ip4Config")?;
    let (mut addresses, mut gateway, mut dns) = (Vec::new(), None, Vec::new());
    if ip4.as_str() != "/" {
        let data: Vec<HashMap<String, OwnedValue>> = bus::property(c, DEST, ip4.as_str(), IP4, "AddressData").unwrap_or_default();
        for a in data {
            let addr = a.get("address").and_then(|v| String::try_from(v.clone()).ok());
            let prefix = a.get("prefix").and_then(|v| u32::try_from(v).ok());
            if let (Some(addr), Some(prefix)) = (addr, prefix) {
                addresses.push(format!("{addr}/{prefix}"));
            }
        }
        gateway = Some(str_prop(ip4.as_str(), IP4, "Gateway")).filter(|g| !g.is_empty());
        let ns: Vec<HashMap<String, OwnedValue>> = bus::property(c, DEST, ip4.as_str(), IP4, "NameserverData").unwrap_or_default();
        dns = ns.into_iter().filter_map(|n| n.get("address").and_then(|v| String::try_from(v.clone()).ok())).collect();
    }
    Ok(Device { path: p.to_owned(), interface: str_prop(p, DEVICE, "Interface"), kind, state, reason, hw_address, connection, addresses, gateway, dns, speed })
}

/// The SSIDs of saved Wi-Fi connections, with their settings paths.
fn saved() -> Result<Vec<(String, OwnedObjectPath)>> {
    let c = bus::system()?;
    let paths: Vec<OwnedObjectPath> = bus::call(c, DEST, SETTINGS_PATH, SETTINGS, "ListConnections", &())?;
    let mut out = Vec::new();
    for p in paths {
        let Ok(settings) = bus::call::<_, HashMap<String, HashMap<String, OwnedValue>>>(c, DEST, p.as_str(), CONNECTION, "GetSettings", &())
        else {
            continue;
        };
        if let Some(ssid) = settings.get("802-11-wireless").and_then(|w| w.get("ssid")).and_then(|v| Vec::<u8>::try_from(v.clone()).ok()) {
            out.push((String::from_utf8_lossy(&ssid).into_owned(), p));
        }
    }
    Ok(out)
}

/// Everything the Network panel shows.
pub fn status() -> Result<Status> {
    let c = bus::system()?;
    let wireless_enabled = bus::property(c, DEST, NM_PATH, NM, "WirelessEnabled")?;
    let wireless_hardware = bus::property(c, DEST, NM_PATH, NM, "WirelessHardwareEnabled")?;
    let devices: Vec<Device> = devices_paths()?.iter().filter_map(|p| device(p).ok()).filter(|d| d.kind != Kind::Other).collect();
    let known: Vec<String> = saved().unwrap_or_default().into_iter().map(|(s, _)| s).collect();
    let mut networks: Vec<Network> = Vec::new();
    for dev in devices.iter().filter(|d| d.kind == Kind::Wifi) {
        let active: OwnedObjectPath = bus::property(c, DEST, &dev.path, WIRELESS, "ActiveAccessPoint")?;
        let aps: Vec<OwnedObjectPath> = bus::call(c, DEST, &dev.path, WIRELESS, "GetAllAccessPoints", &())?;
        for ap in aps {
            let p = ap.as_str();
            let Ok(ssid) = bus::property::<Vec<u8>>(c, DEST, p, AP, "Ssid") else { continue };
            if ssid.is_empty() {
                continue; // hidden
            }
            let ssid = String::from_utf8_lossy(&ssid).into_owned();
            let strength: u8 = bus::property(c, DEST, p, AP, "Strength").unwrap_or(0);
            let flags: u32 = bus::property(c, DEST, p, AP, "Flags").unwrap_or(0);
            let wpa: u32 = bus::property(c, DEST, p, AP, "WpaFlags").unwrap_or(0);
            let rsn: u32 = bus::property(c, DEST, p, AP, "RsnFlags").unwrap_or(0);
            // NM_802_11_AP_SEC_KEY_MGMT_802_1X.
            let enterprise = (wpa | rsn) & 0x200 != 0;
            let is_active = active.as_str() == p;
            let n = Network { known: known.contains(&ssid), ssid, strength, secure: flags & 1 != 0 || wpa != 0 || rsn != 0, enterprise, ap: p.to_owned(), active: is_active };
            match networks.iter_mut().find(|x| x.ssid == n.ssid) {
                Some(x) => {
                    if n.active || (!x.active && n.strength > x.strength) {
                        *x = Network { known: x.known, ..n };
                    }
                }
                None => networks.push(n),
            }
        }
    }
    networks.sort_by(|a, b| b.active.cmp(&a.active).then(b.strength.cmp(&a.strength)).then(a.ssid.to_lowercase().cmp(&b.ssid.to_lowercase())));
    Ok(Status { wireless_enabled, wireless_hardware, devices, networks })
}

pub fn set_wireless_enabled(on: bool) -> Result<()> {
    bus::set_property(bus::system()?, DEST, NM_PATH, NM, "WirelessEnabled", on)
}

/// Ask a Wi-Fi device to look for networks (NM limits how often).
pub fn scan(device: &str) -> Result<()> {
    let opts: HashMap<&str, Value> = HashMap::new();
    bus::call::<_, ()>(bus::system()?, DEST, device, WIRELESS, "RequestScan", &(opts,))
}

/// Join `network` on `device`: its saved connection if there is one,
/// else a new one (with `password` for a secured network).
pub fn join(device: &str, network: &Network, password: Option<&str>) -> Result<()> {
    let c = bus::system()?;
    let dev = zvariant::ObjectPath::try_from(device)?;
    let ap = zvariant::ObjectPath::try_from(network.ap.as_str())?;
    if let Some((_, path)) = saved()?.into_iter().find(|(s, _)| *s == network.ssid)
        && password.is_none()
    {
        let _: OwnedObjectPath = bus::call(c, DEST, NM_PATH, NM, "ActivateConnection", &(path, dev, ap))?;
        return Ok(());
    }
    let mut settings: HashMap<&str, HashMap<&str, Value>> = HashMap::new();
    if network.secure {
        settings.insert(
            "802-11-wireless-security",
            HashMap::from([("key-mgmt", Value::from("wpa-psk")), ("psk", Value::from(password.unwrap_or_default()))]),
        );
    }
    let _: (OwnedObjectPath, OwnedObjectPath) = bus::call(c, DEST, NM_PATH, NM, "AddAndActivateConnection", &(settings, dev, ap))?;
    Ok(())
}

pub fn disconnect(device: &str) -> Result<()> {
    bus::call::<_, ()>(bus::system()?, DEST, device, DEVICE, "Disconnect", &())
}

/// Delete every saved connection for `ssid`.
pub fn forget(ssid: &str) -> Result<()> {
    for (_, path) in saved()?.into_iter().filter(|(s, _)| s == ssid) {
        bus::call::<_, ()>(bus::system()?, DEST, path.as_str(), CONNECTION, "Delete", &())?;
    }
    Ok(())
}
