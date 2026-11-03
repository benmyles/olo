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

/// [`wifi`], and the objects whose changes change it: NetworkManager,
/// the adapter, its access point.
fn wifi_and_paths() -> Result<(Option<Wifi>, Vec<String>)> {
    let conn = bus::system()?;
    let mut paths = vec![NM_PATH.to_owned()];
    for dev in devices_paths()? {
        let kind: u32 = bus::property(conn, DEST, dev.as_str(), DEVICE, "DeviceType")?;
        if kind != TYPE_WIFI {
            continue;
        }
        paths.push(dev.as_str().to_owned());
        let ap: OwnedObjectPath = bus::property(conn, DEST, dev.as_str(), WIRELESS, "ActiveAccessPoint")?;
        if ap.as_str() == "/" {
            return Ok((Some(Wifi { ssid: None, strength: 0 }), paths));
        }
        paths.push(ap.as_str().to_owned());
        let ssid: Vec<u8> = bus::property(conn, DEST, ap.as_str(), AP, "Ssid")?;
        let strength: u8 = bus::property(conn, DEST, ap.as_str(), AP, "Strength")?;
        return Ok((Some(Wifi { ssid: Some(String::from_utf8_lossy(&ssid).into_owned()), strength: strength.min(100) as u32 }), paths));
    }
    Ok((None, paths))
}

/// Calls `changed` with the first Wi-Fi adapter's state ([`wifi`]) now
/// and whenever it changes, on threads of its own, following
/// NetworkManager's signals (nothing polled), on a bus connection of its
/// own that the bus sends only these: any device's state changing,
/// NetworkManager's own (devices coming and going), and while connected,
/// the adapter's properties (moving to another access point) and its
/// access point's (the strength). An adapter that isn't connected scans
/// often, its list of access points changing several times a second:
/// those aren't asked for. Quick runs of changes come together. Err if
/// NetworkManager can't be reached.
pub fn watch_wifi(changed: impl Fn(Option<Wifi>) + Send + 'static) -> Result<()> {
    use zbus::{MatchRule, message::Type};
    let conn = zbus::blocking::Connection::system()?;
    let dbus = zbus::blocking::fdo::DBusProxy::new(&conn)?;
    let props = |path: &str| -> Result<MatchRule<'static>> {
        Ok(MatchRule::builder().msg_type(Type::Signal).sender(DEST)?.path(path.to_owned())?.interface("org.freedesktop.DBus.Properties")?.member("PropertiesChanged")?.build())
    };
    let states = MatchRule::builder().msg_type(Type::Signal).sender(DEST)?.interface(DEVICE)?.member("StateChanged")?.build();
    let own = MatchRule::builder().msg_type(Type::Signal).sender(DEST)?.path(NM_PATH)?.build();
    // (Before reading, so a change between is heard.)
    let messages = zbus::blocking::MessageIterator::from(&conn);
    dbus.add_match_rule(states)?;
    dbus.add_match_rule(own)?;
    let (first, paths) = wifi_and_paths()?;
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    std::thread::Builder::new().name("wifi-signals".into()).spawn(move || {
        for m in messages.flatten() {
            if m.message_type() == Type::Signal && m.header().sender().is_some_and(|s| s.as_str() != "org.freedesktop.DBus") && tx.send(()).is_err() {
                return;
            }
        }
    })?;
    std::thread::Builder::new().name("wifi".into()).spawn(move || {
        // What's asked for while connected: the adapter and its access
        // point's properties.
        let mut asked: Vec<MatchRule<'static>> = Vec::new();
        let mut follow = |now: &Option<Wifi>, paths: &[String]| {
            for rule in asked.drain(..) {
                let _ = dbus.remove_match_rule(rule);
            }
            if now.as_ref().is_some_and(|w| w.ssid.is_some()) {
                for path in paths.iter().skip(1) {
                    if let Ok(rule) = props(path)
                        && dbus.add_match_rule(rule.clone()).is_ok()
                    {
                        asked.push(rule);
                    }
                }
            }
        };
        follow(&first, &paths);
        let mut last = first;
        changed(last.clone());
        while rx.recv().is_ok() {
            // Let a burst finish (joining changes a dozen properties).
            std::thread::sleep(std::time::Duration::from_millis(80));
            while rx.try_recv().is_ok() {}
            let Ok((now, paths)) = wifi_and_paths() else { continue };
            follow(&now, &paths);
            if now != last {
                changed(now.clone());
                last = now;
            }
        }
    })?;
    Ok(())
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
    /// How a password joins it (`key_mgmt`): WPA3 alone ("sae"), WPA2
    /// or both ("wpa-psk"), Enhanced Open ("owe", no password).
    pub key_mgmt: &'static str,
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
        let Ok(settings) = bus::call::<_, HashMap<String, HashMap<String, OwnedValue>>>(c, DEST, p.as_str(), CONNECTION, "GetSettings", &()) else {
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
            let key_mgmt = key_mgmt(wpa, rsn);
            let n = Network {
                known: known.contains(&ssid),
                ssid,
                strength,
                secure: (flags & 1 != 0 || wpa != 0 || rsn != 0) && key_mgmt != "owe",
                enterprise,
                key_mgmt,
                ap: p.to_owned(),
                active: is_active,
            };
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

/// Have NetworkManager read its configuration files again (after one
/// is written; as root, or whoever polkit lets).
pub fn reload() -> Result<()> {
    bus::call::<_, ()>(bus::system()?, DEST, NM_PATH, NM, "Reload", &(0u32,))
}

pub fn set_wireless_enabled(on: bool) -> Result<()> {
    bus::set_property(bus::system()?, DEST, NM_PATH, NM, "WirelessEnabled", on)
}

/// Ask a Wi-Fi device to look for networks (NM limits how often).
pub fn scan(device: &str) -> Result<()> {
    let opts: HashMap<&str, Value> = HashMap::new();
    bus::call::<_, ()>(bus::system()?, DEST, device, WIRELESS, "RequestScan", &(opts,))
}

/// How a password joins an access point with these WPA and RSN flags
/// (NM_802_11_AP_SEC_*): "sae" for WPA3 alone, "owe" for Enhanced Open,
/// else "wpa-psk" (WPA2, and WPA2/WPA3 networks, which take either).
/// Asking WPA3-only "Home-WPA3" for WPA2 was refused before it began
/// ("Access point does not support PSK", 2026-10-31).
pub fn key_mgmt(wpa: u32, rsn: u32) -> &'static str {
    const PSK: u32 = 0x100;
    const SAE: u32 = 0x400;
    const OWE: u32 = 0x800 | 0x1000;
    let flags = wpa | rsn;
    if flags & SAE != 0 && flags & PSK == 0 {
        "sae"
    } else if flags & OWE != 0 && flags & PSK == 0 {
        "owe"
    } else {
        "wpa-psk"
    }
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
    if network.key_mgmt == "owe" {
        settings.insert("802-11-wireless-security", HashMap::from([("key-mgmt", Value::from("owe"))]));
    } else if network.secure {
        settings.insert("802-11-wireless-security", HashMap::from([("key-mgmt", Value::from(network.key_mgmt)), ("psk", Value::from(password.unwrap_or_default()))]));
    }
    let _: (OwnedObjectPath, OwnedObjectPath) = bus::call(c, DEST, NM_PATH, NM, "AddAndActivateConnection", &(settings, dev, ap))?;
    Ok(())
}

/// The outer EAP method of an 802.1X (WPA-Enterprise) network.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Eap {
    /// Protected EAP: a username and password inside TLS (most networks,
    /// eduroam included).
    #[default]
    Peap,
    /// Tunnelled TLS: likewise, with more inner methods.
    Ttls,
    /// A client certificate instead of a password.
    Tls,
}

impl Eap {
    pub const ALL: [Eap; 3] = [Eap::Peap, Eap::Ttls, Eap::Tls];

    pub fn name(self) -> &'static str {
        match self {
            Eap::Peap => "PEAP",
            Eap::Ttls => "TTLS",
            Eap::Tls => "TLS",
        }
    }

    /// The inner (phase 2) methods it can carry, preferred first; NM's names.
    pub fn inner(self) -> &'static [&'static str] {
        match self {
            Eap::Peap => &["mschapv2", "gtc", "md5"],
            Eap::Ttls => &["pap", "mschapv2", "mschap", "chap"],
            Eap::Tls => &[],
        }
    }
}

/// How the server's certificate is checked.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum CaCert {
    /// Against the system's trusted certificates.
    #[default]
    System,
    File(std::path::PathBuf),
    /// Not at all (some networks require it; it isn't safe).
    None,
}

/// What joining an 802.1X network takes.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Enterprise {
    pub eap: Eap,
    /// One of `eap.inner()`.
    pub inner: String,
    pub identity: String,
    pub password: String,
    /// Sent in the clear before the tunnel is up, instead of `identity`.
    pub anonymous_identity: String,
    pub ca: CaCert,
    /// The server certificate's domain must end with this.
    pub domain: String,
    /// TLS: the client certificate, its key and the key's password.
    pub client_cert: Option<std::path::PathBuf>,
    pub private_key: Option<std::path::PathBuf>,
    pub key_password: String,
}

impl Enterprise {
    /// Enough is filled in to try.
    pub fn ready(&self) -> bool {
        !self.identity.trim().is_empty()
            && match self.eap {
                Eap::Tls => self.client_cert.is_some() && self.private_key.is_some(),
                _ => !self.password.is_empty() && self.eap.inner().contains(&self.inner.as_str()),
            }
            && !matches!(&self.ca, CaCert::File(p) if p.as_os_str().is_empty())
    }
}

/// NM wants file references as `file://` URIs in NUL-terminated bytes.
fn file_blob(path: &std::path::Path) -> Value<'static> {
    let mut b = format!("file://{}", path.display()).into_bytes();
    b.push(0);
    Value::from(b)
}

/// The settings of a new connection to an 802.1X network.
pub fn enterprise_settings(e: &Enterprise) -> HashMap<&'static str, HashMap<&'static str, Value<'static>>> {
    let mut x: HashMap<&'static str, Value<'static>> = HashMap::new();
    let method = match e.eap {
        Eap::Peap => "peap",
        Eap::Ttls => "ttls",
        Eap::Tls => "tls",
    };
    x.insert("eap", Value::from(vec![method.to_owned()]));
    x.insert("identity", Value::from(e.identity.trim().to_owned()));
    if !e.anonymous_identity.trim().is_empty() {
        x.insert("anonymous-identity", Value::from(e.anonymous_identity.trim().to_owned()));
    }
    match e.eap {
        Eap::Tls => {
            if let Some(c) = &e.client_cert {
                x.insert("client-cert", file_blob(c));
            }
            if let Some(k) = &e.private_key {
                x.insert("private-key", file_blob(k));
            }
            if !e.key_password.is_empty() {
                x.insert("private-key-password", Value::from(e.key_password.clone()));
            }
        }
        _ => {
            x.insert("phase2-auth", Value::from(e.inner.clone()));
            x.insert("password", Value::from(e.password.clone()));
        }
    }
    match &e.ca {
        CaCert::System => {
            x.insert("system-ca-certs", Value::from(true));
        }
        CaCert::File(p) => {
            x.insert("ca-cert", file_blob(p));
        }
        CaCert::None => {}
    }
    if !e.domain.trim().is_empty() {
        x.insert("domain-suffix-match", Value::from(e.domain.trim().to_owned()));
    }
    HashMap::from([("802-11-wireless-security", HashMap::from([("key-mgmt", Value::from("wpa-eap"))])), ("802-1x", x)])
}

/// Join an 802.1X `network` on `device` as a new connection.
pub fn join_enterprise(device: &str, network: &Network, e: &Enterprise) -> Result<()> {
    let c = bus::system()?;
    let dev = zvariant::ObjectPath::try_from(device)?;
    let ap = zvariant::ObjectPath::try_from(network.ap.as_str())?;
    let settings = enterprise_settings(e);
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

#[cfg(test)]
mod enterprise_tests {
    use super::*;

    fn get<'a>(s: &'a HashMap<&str, HashMap<&str, Value<'static>>>, k: &str) -> Option<&'a Value<'static>> {
        s.get("802-1x").and_then(|x| x.get(k))
    }

    #[test]
    fn security_follows_the_access_point() {
        // Home-WPA3: RSN pair_ccmp group_ccmp sae.
        assert_eq!(key_mgmt(0, 0x400 | 0x8 | 0x80), "sae");
        // Home: psk and sae.
        assert_eq!(key_mgmt(0, 0x100 | 0x400 | 0x8 | 0x80), "wpa-psk");
        assert_eq!(key_mgmt(0, 0x100 | 0x8 | 0x80), "wpa-psk");
        assert_eq!(key_mgmt(0, 0x800), "owe");
    }

    #[test]
    fn peap_with_system_certificates() {
        let e = Enterprise { identity: " ann@uni.example ".into(), password: "pw".into(), inner: "mschapv2".into(), domain: "uni.example".into(), ..Default::default() };
        assert!(e.ready());
        let s = enterprise_settings(&e);
        assert_eq!(s["802-11-wireless-security"]["key-mgmt"], Value::from("wpa-eap"));
        assert_eq!(get(&s, "eap"), Some(&Value::from(vec!["peap".to_owned()])));
        assert_eq!(get(&s, "identity"), Some(&Value::from("ann@uni.example")));
        assert_eq!(get(&s, "phase2-auth"), Some(&Value::from("mschapv2")));
        assert_eq!(get(&s, "system-ca-certs"), Some(&Value::from(true)));
        assert_eq!(get(&s, "domain-suffix-match"), Some(&Value::from("uni.example")));
        assert!(get(&s, "anonymous-identity").is_none() && get(&s, "ca-cert").is_none());
    }

    #[test]
    fn tls_with_files() {
        let mut e = Enterprise { eap: Eap::Tls, identity: "host".into(), ca: CaCert::File("/etc/ca.pem".into()), ..Default::default() };
        assert!(!e.ready(), "needs a certificate and key");
        e.client_cert = Some("/home/a/me.pem".into());
        e.private_key = Some("/home/a/me.key".into());
        e.key_password = "k".into();
        assert!(e.ready());
        let s = enterprise_settings(&e);
        assert_eq!(get(&s, "ca-cert"), Some(&Value::from(b"file:///etc/ca.pem\0".to_vec())));
        assert_eq!(get(&s, "client-cert"), Some(&Value::from(b"file:///home/a/me.pem\0".to_vec())));
        assert_eq!(get(&s, "private-key-password"), Some(&Value::from("k")));
        assert!(get(&s, "password").is_none() && get(&s, "phase2-auth").is_none() && get(&s, "system-ca-certs").is_none());
    }

    #[test]
    fn inner_must_match() {
        let e = Enterprise { eap: Eap::Ttls, identity: "a".into(), password: "p".into(), inner: "gtc".into(), ..Default::default() };
        assert!(!e.ready());
        assert!(Enterprise { inner: "pap".into(), ..e }.ready());
    }
}
