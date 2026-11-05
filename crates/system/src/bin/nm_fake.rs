//! `nm-fake [--address ADDRESS] [--network SSID:PASSWORD]… [--enterprise
//! SSID]… [--ap SSID:STRENGTH]… [--wired] [--wifi SSID | --wifi-adapter]
//! [--dump FILE]`: a stand-in NetworkManager for tests, so nothing touches
//! the real one (its saved networks and their passwords, this computer's
//! links). It takes the name org.freedesktop.NetworkManager on the bus at
//! ADDRESS (default: the session bus, `DBUS_SESSION_BUS_ADDRESS`) and
//! answers what `olo_system::network` asks:
//!
//! - the saved connections (`ListConnections`, each one's `GetSettings`,
//!   `GetSecrets`, `Update`, `Update2` and `Delete`), and saving another
//!   (`AddConnection`). Each `--network` is a saved WPA one (an empty
//!   password: an open one), each `--enterprise` a WPA-Enterprise one;
//! - the links: `--wired` an Ethernet adapter (enp3s0) connected with
//!   "Wired connection 1", `--wifi SSID` a Wi-Fi adapter (wlan0) joined to
//!   that saved network, `--wifi-adapter` one joined to none. The saved
//!   Wi-Fi networks are in range (strongest first), and each `--ap` too.
//!   Each has its addresses, router and DNS (from "DHCP", or its
//!   connection's own), Activate, Deactivate, Disconnect, Reapply and
//!   `GetAppliedConnection`, and the default route goes where the lowest
//!   route metric is (NetworkManager's: Ethernet 100, Wi-Fi 600, unless a
//!   connection says), as `PrimaryConnection` and its type. What changes
//!   is signalled as NetworkManager does (`StateChanged`,
//!   `PropertiesChanged`).
//!
//! `--dump FILE` is rewritten with every saved connection (as JSON: id,
//! type, ssid, key-mgmt, psk, route metric, DNS, addresses) whenever one
//! changes. Point programs at it with `OLO_NM_BUS=ADDRESS`.
//!
//! `nm-fake --address ADDRESS --tell WHAT` tells a running one, as a
//! test would the world: `wired down` (the cable out), `wired up`,
//! `wifi down`, `wifi up SSID`, `strength N` (the joined network's),
//! `state` (prints the links and the default route as JSON).

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use zvariant::{OwnedObjectPath, OwnedValue, Value};

type Settings = HashMap<String, HashMap<String, OwnedValue>>;

const NM_PATH: &str = "/org/freedesktop/NetworkManager";
const SETTINGS_PATH: &str = "/org/freedesktop/NetworkManager/Settings";
const CONTROL_PATH: &str = "/org/olo/NmFake";
const NM: &str = "org.freedesktop.NetworkManager";
const DEVICE: &str = "org.freedesktop.NetworkManager.Device";
const WIRELESS: &str = "org.freedesktop.NetworkManager.Device.Wireless";
const AP: &str = "org.freedesktop.NetworkManager.AccessPoint";
const ACTIVE: &str = "org.freedesktop.NetworkManager.Connection.Active";
const IP4: &str = "org.freedesktop.NetworkManager.IP4Config";
const TYPE_ETHERNET: u32 = 1;
const TYPE_WIFI: u32 = 2;
/// NM_DEVICE_STATE_*.
const UNAVAILABLE: u32 = 20;
const DISCONNECTED: u32 = 30;
const ACTIVATED: u32 = 100;
/// NM_DEVICE_STATE_REASON_*.
const USER_REQUESTED: u32 = 39;
const CARRIER: u32 = 40;

fn value(v: Value<'_>) -> OwnedValue {
    v.try_to_owned().expect("a value")
}

fn opath(p: impl Into<String>) -> OwnedObjectPath {
    OwnedObjectPath::try_from(p.into()).expect("an object path")
}

fn root() -> OwnedObjectPath {
    opath("/")
}

fn uuid(n: usize) -> String {
    format!("00000000-0000-4000-8000-{n:012}")
}

fn fail(e: impl std::fmt::Display) -> zbus::fdo::Error {
    zbus::fdo::Error::Failed(e.to_string())
}

/// A saved connection: its settings (no secrets) and its password.
#[derive(Debug)]
struct Saved {
    /// Its number in its path (they stay put as others go).
    n: usize,
    settings: Settings,
    psk: String,
}

fn text_of(s: &Settings, g: &str, k: &str) -> String {
    s.get(g).and_then(|x| x.get(k)).and_then(|v| String::try_from(v.clone()).ok()).unwrap_or_default()
}

fn flag_of(s: &Settings, g: &str, k: &str, or: bool) -> bool {
    s.get(g).and_then(|x| x.get(k)).and_then(|v| bool::try_from(v.clone()).ok()).unwrap_or(or)
}

/// `ipv4.route-metric`: -1 (or none) for the device type's.
fn metric_of(s: &Settings) -> i64 {
    s.get("ipv4").and_then(|x| x.get("route-metric")).and_then(|v| i64::try_from(v.clone()).ok()).unwrap_or(-1)
}

/// `ipv4.dns`: addresses in network order, as NetworkManager has them.
fn dns_of(s: &Settings) -> Vec<String> {
    let raw: Vec<u32> = s.get("ipv4").and_then(|x| x.get("dns")).and_then(|v| Vec::<u32>::try_from(v.clone()).ok()).unwrap_or_default();
    raw.into_iter().map(|a| std::net::Ipv4Addr::from(a.to_ne_bytes()).to_string()).collect()
}

/// `ipv4.address-data` (a manual configuration's).
fn addresses_of(s: &Settings) -> Vec<(String, u32)> {
    let Some(v) = s.get("ipv4").and_then(|x| x.get("address-data")) else { return Vec::new() };
    let Ok(list) = Vec::<HashMap<String, OwnedValue>>::try_from(v.clone()) else { return Vec::new() };
    list.into_iter()
        .filter_map(|a| {
            let address = a.get("address").and_then(|v| String::try_from(v.clone()).ok())?;
            let prefix = a.get("prefix").and_then(|v| u32::try_from(v.clone()).ok())?;
            Some((address, prefix))
        })
        .collect()
}

impl Saved {
    fn text(&self, g: &str, k: &str) -> String {
        text_of(&self.settings, g, k)
    }

    fn kind(&self) -> u32 {
        match self.text("connection", "type").as_str() {
            "802-3-ethernet" => TYPE_ETHERNET,
            "802-11-wireless" => TYPE_WIFI,
            _ => 0,
        }
    }

    fn ssid(&self) -> Option<String> {
        let raw = self.settings.get("802-11-wireless").and_then(|w| w.get("ssid")).and_then(|v| Vec::<u8>::try_from(v.clone()).ok())?;
        Some(String::from_utf8_lossy(&raw).into_owned())
    }

    fn dump(&self) -> serde_json::Value {
        serde_json::json!({
            "id": self.text("connection", "id"),
            "type": self.text("connection", "type"),
            "ssid": self.ssid().unwrap_or_default(),
            "hidden": flag_of(&self.settings, "802-11-wireless", "hidden", false),
            "autoconnect": flag_of(&self.settings, "connection", "autoconnect", true),
            "key_mgmt": self.text("802-11-wireless-security", "key-mgmt"),
            "psk": self.psk,
            "enterprise": self.settings.contains_key("802-1x"),
            "route_metric": metric_of(&self.settings),
            "method": self.text("ipv4", "method"),
            "dns": dns_of(&self.settings),
            "ignore_auto_dns": flag_of(&self.settings, "ipv4", "ignore-auto-dns", false),
            "addresses": addresses_of(&self.settings).into_iter().map(|(a, p)| format!("{a}/{p}")).collect::<Vec<_>>(),
            "gateway": self.text("ipv4", "gateway"),
            "searches": self.settings.get("ipv4").and_then(|x| x.get("dns-search")).and_then(|v| Vec::<String>::try_from(v.clone()).ok()).unwrap_or_default(),
        })
    }
}

/// What "DHCP" gave a link.
#[derive(Debug, Clone)]
struct Lease {
    address: String,
    gateway: String,
    dns: Vec<String>,
}

#[derive(Debug)]
struct Dev {
    iface: String,
    kind: u32,
    hw: String,
    state: u32,
    reason: u32,
    /// The saved connection in use (its number).
    active: Option<usize>,
    /// Its settings as applied (Reapply changes them; a saved
    /// connection's changes don't until then).
    applied: Settings,
    /// The access point joined (Wi-Fi).
    ap: Option<usize>,
    lease: Lease,
    /// The route metric its kind had by default when it was connected
    /// or reapplied (NetworkManager's configuration, as last read).
    default_metric: i64,
}

#[derive(Debug)]
struct Ap {
    ssid: String,
    strength: u8,
    secure: bool,
}

struct Model {
    saved: Vec<Saved>,
    next: usize,
    devices: Vec<Dev>,
    aps: Vec<Ap>,
    wireless: bool,
    dump: Option<PathBuf>,
    /// Each kind's route metric by default, from Olo's conf.d file as
    /// last read (at start and `Reload`; under `OLO_NET_ROOT`).
    defaults: Vec<(u32, i64)>,
}

/// The route metrics Olo's conf.d file gives each kind of link.
fn conf_defaults() -> Vec<(u32, i64)> {
    use olo_system::netconf::{NetConf, Service};
    NetConf::read().metrics().into_iter().map(|(s, m)| (if s == Service::Ethernet { TYPE_ETHERNET } else { TYPE_WIFI }, m as i64)).collect()
}

type Shared = Arc<Mutex<Model>>;

fn conn_path(n: usize) -> String {
    format!("{SETTINGS_PATH}/{n}")
}
fn dev_path(i: usize) -> String {
    format!("{NM_PATH}/Devices/{}", i + 1)
}
fn ap_path(i: usize) -> String {
    format!("{NM_PATH}/AccessPoint/{}", i + 1)
}
fn active_path(i: usize) -> String {
    format!("{NM_PATH}/ActiveConnection/{}", i + 1)
}
fn ip4_path(i: usize) -> String {
    format!("{NM_PATH}/IP4Config/{}", i + 1)
}

/// A path's number at its end ("/…/Settings/3": 3).
fn number(path: &str) -> Option<usize> {
    path.rsplit('/').next()?.parse().ok()
}

impl Model {
    fn write_dump(&self) {
        if let Some(dump) = &self.dump {
            let all: Vec<serde_json::Value> = self.saved.iter().map(Saved::dump).collect();
            let _ = std::fs::write(dump, serde_json::to_vec_pretty(&all).unwrap_or_default());
        }
    }

    fn saved(&self, n: usize) -> Option<&Saved> {
        self.saved.iter().find(|s| s.n == n)
    }

    fn add(&mut self, settings: Settings, psk: String) -> usize {
        self.next += 1;
        let n = self.next;
        self.saved.push(Saved { n, settings, psk });
        self.write_dump();
        n
    }

    fn connected(&self, i: usize) -> bool {
        self.devices[i].state == ACTIVATED
    }

    /// The route metric a link's routes get: its connection's, else its
    /// kind's when it was applied.
    fn metric(&self, i: usize) -> i64 {
        let d = &self.devices[i];
        match metric_of(&d.applied) {
            m if m >= 0 => m,
            _ => d.default_metric,
        }
    }

    fn default_for(&self, kind: u32) -> i64 {
        self.defaults.iter().find(|(k, _)| *k == kind).map(|(_, m)| *m).unwrap_or(if kind == TYPE_ETHERNET { 100 } else { 600 })
    }

    /// Its addresses, router and DNS servers.
    fn ip(&self, i: usize) -> (Vec<(String, u32)>, String, Vec<String>) {
        let d = &self.devices[i];
        let manual = text_of(&d.applied, "ipv4", "method") == "manual";
        let (addresses, gateway) = if manual { (addresses_of(&d.applied), text_of(&d.applied, "ipv4", "gateway")) } else { (vec![(d.lease.address.clone(), 24)], d.lease.gateway.clone()) };
        let mut dns = dns_of(&d.applied);
        if !flag_of(&d.applied, "ipv4", "ignore-auto-dns", false) && !manual {
            dns.extend(d.lease.dns.iter().cloned());
        }
        (addresses, gateway, dns)
    }

    /// The link the default route goes through: the lowest metric of
    /// those connected with a router.
    fn primary(&self) -> Option<usize> {
        (0..self.devices.len()).filter(|&i| self.connected(i) && !self.ip(i).1.is_empty()).min_by_key(|&i| (self.metric(i), i))
    }

    fn activate(&mut self, n: usize, i: usize) -> zbus::fdo::Result<()> {
        let s = self.saved(n).ok_or_else(|| zbus::fdo::Error::UnknownObject(conn_path(n)))?;
        let d = self.devices.get(i).ok_or_else(|| fail("no such device"))?;
        if s.kind() != d.kind {
            return Err(fail("the connection isn't for this device"));
        }
        if d.kind == TYPE_WIFI && !self.wireless {
            return Err(fail("Wi-Fi is off"));
        }
        let (settings, ssid) = (s.settings.clone(), s.ssid());
        let ap = ssid.and_then(|ssid| self.aps.iter().position(|a| a.ssid == ssid));
        if self.devices[i].kind == TYPE_WIFI && ap.is_none() {
            return Err(fail("the network isn't in range"));
        }
        let default_metric = self.default_for(self.devices[i].kind);
        let d = &mut self.devices[i];
        (d.active, d.applied, d.ap, d.state, d.reason, d.default_metric) = (Some(n), settings, ap, ACTIVATED, 0, default_metric);
        Ok(())
    }

    fn deactivate(&mut self, i: usize, state: u32, reason: u32) {
        let d = &mut self.devices[i];
        (d.active, d.applied, d.ap, d.state, d.reason) = (None, Settings::new(), None, state, reason);
    }

    /// What clients see, to say what changed: (path, interface, property,
    /// value).
    fn seen(&self) -> Vec<(String, &'static str, &'static str, String)> {
        let mut out = Vec::new();
        let primary = self.primary();
        out.push((NM_PATH.to_owned(), NM, "PrimaryConnection", format!("{primary:?}")));
        out.push((NM_PATH.to_owned(), NM, "PrimaryConnectionType", format!("{:?}", primary.map(|i| self.devices[i].kind))));
        out.push((NM_PATH.to_owned(), NM, "ActiveConnections", format!("{:?}", (0..self.devices.len()).filter(|&i| self.connected(i)).collect::<Vec<_>>())));
        out.push((NM_PATH.to_owned(), NM, "WirelessEnabled", self.wireless.to_string()));
        for (i, d) in self.devices.iter().enumerate() {
            let p = dev_path(i);
            out.push((p.clone(), DEVICE, "State", d.state.to_string()));
            out.push((p.clone(), DEVICE, "StateReason", d.reason.to_string()));
            out.push((p.clone(), DEVICE, "ActiveConnection", format!("{:?}", d.active)));
            out.push((p.clone(), DEVICE, "Ip4Config", self.connected(i).to_string()));
            if d.kind == TYPE_WIFI {
                out.push((p.clone(), WIRELESS, "ActiveAccessPoint", format!("{:?}", d.ap)));
            }
            out.push((ip4_path(i), IP4, "AddressData", format!("{:?}", self.ip(i).0)));
            out.push((ip4_path(i), IP4, "Gateway", self.ip(i).1));
            out.push((ip4_path(i), IP4, "NameserverData", format!("{:?}", self.ip(i).2)));
            out.push((active_path(i), ACTIVE, "Default", (primary == Some(i)).to_string()));
        }
        for (i, a) in self.aps.iter().enumerate() {
            out.push((ap_path(i), AP, "Strength", a.strength.to_string()));
        }
        out
    }

    fn state_json(&self) -> String {
        let devices: Vec<serde_json::Value> = (0..self.devices.len())
            .map(|i| {
                let d = &self.devices[i];
                let (addresses, gateway, dns) = self.ip(i);
                serde_json::json!({
                    "interface": d.iface, "state": d.state, "metric": self.metric(i),
                    "connection": d.active.and_then(|n| self.saved(n)).map(|s| s.text("connection", "id")),
                    "addresses": addresses.iter().map(|(a, p)| format!("{a}/{p}")).collect::<Vec<_>>(), "gateway": gateway, "dns": dns,
                })
            })
            .collect();
        serde_json::json!({ "devices": devices, "primary": self.primary().map(|i| self.devices[i].iface.clone()), "wireless": self.wireless }).to_string()
    }
}

/// Signal what changed since `before` (`Model::seen`), as NetworkManager
/// would: `PropertiesChanged` for each object's (the values left for the
/// asking), `StateChanged` for a device's state.
async fn announce(conn: &zbus::Connection, model: &Shared, before: Vec<(String, &'static str, &'static str, String)>) {
    let after = model.lock().unwrap().seen();
    let mut groups: Vec<((String, &'static str), Vec<&'static str>)> = Vec::new();
    let mut states = Vec::new();
    for (path, iface, prop, now) in &after {
        let was = before.iter().find(|(p, i, n, _)| p == path && i == iface && n == prop).map(|x| &x.3);
        if was == Some(now) {
            continue;
        }
        if *iface == DEVICE && *prop == "State" {
            states.push((path.clone(), now.parse::<u32>().unwrap_or(0), was.and_then(|w| w.parse::<u32>().ok()).unwrap_or(0)));
        }
        match groups.iter_mut().find(|(k, _)| k.0 == *path && k.1 == *iface) {
            Some((_, props)) => props.push(prop),
            None => groups.push(((path.clone(), iface), vec![prop])),
        }
    }
    for ((path, iface), props) in groups {
        // An access point's strength with its value, as NetworkManager
        // sends them all (clients take it from there); the rest only
        // named.
        let mut changed: HashMap<&str, Value> = HashMap::new();
        let mut named = Vec::new();
        for prop in props {
            let now = after.iter().find(|(p, i, n, _)| *p == path && *i == iface && *n == prop).and_then(|x| x.3.parse::<u8>().ok());
            match now {
                Some(v) if iface == AP && prop == "Strength" => _ = changed.insert(prop, Value::from(v)),
                _ => named.push(prop),
            }
        }
        let _ = conn.emit_signal(None::<zbus::names::BusName>, path.as_str(), "org.freedesktop.DBus.Properties", "PropertiesChanged", &(iface, changed, named)).await;
    }
    for (path, new, old) in states {
        let reason = number(&path).and_then(|i| model.lock().unwrap().devices.get(i - 1).map(|d| d.reason)).unwrap_or(0);
        let _ = conn.emit_signal(None::<zbus::names::BusName>, path.as_str(), DEVICE, "StateChanged", &(new, old, reason)).await;
    }
}

// ---- NetworkManager ----------------------------------------------------------

struct Nm {
    model: Shared,
}

#[zbus::interface(name = "org.freedesktop.NetworkManager")]
impl Nm {
    #[zbus(property)]
    fn devices(&self) -> Vec<OwnedObjectPath> {
        (0..self.model.lock().unwrap().devices.len()).map(|i| opath(dev_path(i))).collect()
    }

    #[zbus(property)]
    fn all_devices(&self) -> Vec<OwnedObjectPath> {
        self.devices()
    }

    fn get_devices(&self) -> Vec<OwnedObjectPath> {
        self.devices()
    }

    fn get_all_devices(&self) -> Vec<OwnedObjectPath> {
        self.devices()
    }

    #[zbus(property)]
    fn active_connections(&self) -> Vec<OwnedObjectPath> {
        let m = self.model.lock().unwrap();
        (0..m.devices.len()).filter(|&i| m.connected(i)).map(|i| opath(active_path(i))).collect()
    }

    #[zbus(property)]
    fn primary_connection(&self) -> OwnedObjectPath {
        self.model.lock().unwrap().primary().map(|i| opath(active_path(i))).unwrap_or_else(root)
    }

    #[zbus(property)]
    fn primary_connection_type(&self) -> String {
        let m = self.model.lock().unwrap();
        match m.primary().map(|i| m.devices[i].kind) {
            Some(TYPE_ETHERNET) => "802-3-ethernet".into(),
            Some(TYPE_WIFI) => "802-11-wireless".into(),
            _ => String::new(),
        }
    }

    #[zbus(property)]
    fn wireless_enabled(&self) -> bool {
        self.model.lock().unwrap().wireless
    }

    #[zbus(property)]
    async fn set_wireless_enabled(&mut self, on: bool, #[zbus(connection)] conn: &zbus::Connection) {
        let before = {
            let mut m = self.model.lock().unwrap();
            let before = m.seen();
            m.wireless = on;
            if !on {
                for i in 0..m.devices.len() {
                    if m.devices[i].kind == TYPE_WIFI {
                        m.deactivate(i, UNAVAILABLE, 0);
                    }
                }
            }
            before
        };
        announce(conn, &self.model, before).await;
    }

    #[zbus(property)]
    fn wireless_hardware_enabled(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn networking_enabled(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn version(&self) -> String {
        "1.52.0-olo-fake".into()
    }

    /// NM_STATE_CONNECTED_GLOBAL, or DISCONNECTED.
    #[zbus(property)]
    fn state(&self) -> u32 {
        if self.model.lock().unwrap().primary().is_some() { 70 } else { 20 }
    }

    /// Its configuration read again: what links get by default when next
    /// connected or reapplied.
    fn reload(&self, _flags: u32) {
        self.model.lock().unwrap().defaults = conf_defaults();
    }

    async fn activate_connection(
        &self,
        connection: OwnedObjectPath,
        device: OwnedObjectPath,
        _specific: OwnedObjectPath,
        #[zbus(connection)] conn: &zbus::Connection,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        let n = number(connection.as_str()).ok_or_else(|| fail("no connection"))?;
        let before = {
            let mut m = self.model.lock().unwrap();
            let i = match number(device.as_str()) {
                Some(d) if device.as_str().starts_with(&format!("{NM_PATH}/Devices/")) => d - 1,
                // "/": the first device it fits.
                _ => {
                    let kind = m.saved(n).map(Saved::kind).unwrap_or(0);
                    m.devices.iter().position(|d| d.kind == kind).ok_or_else(|| fail("no device for it"))?
                }
            };
            let before = m.seen();
            m.activate(n, i)?;
            (before, i)
        };
        announce(conn, &self.model, before.0).await;
        Ok(opath(active_path(before.1)))
    }

    async fn add_and_activate_connection(
        &self,
        mut settings: Settings,
        device: OwnedObjectPath,
        specific: OwnedObjectPath,
        #[zbus(connection)] conn: &zbus::Connection,
        #[zbus(object_server)] server: &zbus::ObjectServer,
    ) -> zbus::fdo::Result<(OwnedObjectPath, OwnedObjectPath)> {
        let i = number(device.as_str()).map(|d| d - 1).ok_or_else(|| fail("no device"))?;
        let (n, before) = {
            let mut m = self.model.lock().unwrap();
            let d = m.devices.get(i).ok_or_else(|| fail("no such device"))?;
            let kind = d.kind;
            // Filled in from the access point, as NetworkManager does.
            let ssid = number(specific.as_str()).and_then(|a| m.aps.get(a - 1)).map(|a| a.ssid.clone());
            let id = ssid.clone().unwrap_or_else(|| "Wired connection".into());
            let next = m.next + 1;
            let c = settings.entry("connection".into()).or_default();
            c.entry("id".into()).or_insert_with(|| value(Value::from(id.as_str())));
            c.entry("uuid".into()).or_insert_with(|| value(Value::from(uuid(next))));
            c.entry("type".into()).or_insert_with(|| value(Value::from(if kind == TYPE_WIFI { "802-11-wireless" } else { "802-3-ethernet" })));
            if let Some(ssid) = ssid {
                settings.entry("802-11-wireless".into()).or_default().entry("ssid".into()).or_insert_with(|| value(Value::from(ssid.as_bytes().to_vec())));
            }
            let psk = settings.get_mut("802-11-wireless-security").and_then(|s| s.remove("psk")).and_then(|v| String::try_from(v).ok()).unwrap_or_default();
            let n = m.add(settings, psk);
            let before = m.seen();
            m.activate(n, i)?;
            (n, before)
        };
        server.at(conn_path(n).as_str(), Connection { model: self.model.clone(), n }).await?;
        announce(conn, &self.model, before).await;
        Ok((opath(conn_path(n)), opath(active_path(i))))
    }

    async fn deactivate_connection(&self, active: OwnedObjectPath, #[zbus(connection)] conn: &zbus::Connection) -> zbus::fdo::Result<()> {
        let i = number(active.as_str()).map(|d| d - 1).ok_or_else(|| fail("no active connection"))?;
        let before = {
            let mut m = self.model.lock().unwrap();
            if !m.devices.get(i).is_some_and(|d| d.active.is_some()) {
                return Err(fail("not active"));
            }
            let before = m.seen();
            m.deactivate(i, DISCONNECTED, USER_REQUESTED);
            before
        };
        announce(conn, &self.model, before).await;
        Ok(())
    }
}

// ---- Devices -------------------------------------------------------------------

struct DeviceObj {
    model: Shared,
    i: usize,
}

#[zbus::interface(name = "org.freedesktop.NetworkManager.Device")]
impl DeviceObj {
    #[zbus(property)]
    fn device_type(&self) -> u32 {
        self.model.lock().unwrap().devices[self.i].kind
    }

    #[zbus(property)]
    fn state(&self) -> u32 {
        self.model.lock().unwrap().devices[self.i].state
    }

    #[zbus(property)]
    fn state_reason(&self) -> (u32, u32) {
        let m = self.model.lock().unwrap();
        (m.devices[self.i].state, m.devices[self.i].reason)
    }

    #[zbus(property)]
    fn interface(&self) -> String {
        self.model.lock().unwrap().devices[self.i].iface.clone()
    }

    #[zbus(property)]
    fn ip_interface(&self) -> String {
        self.interface()
    }

    #[zbus(property)]
    fn hw_address(&self) -> String {
        self.model.lock().unwrap().devices[self.i].hw.clone()
    }

    #[zbus(property)]
    fn managed(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn active_connection(&self) -> OwnedObjectPath {
        if self.model.lock().unwrap().devices[self.i].active.is_some() { opath(active_path(self.i)) } else { root() }
    }

    #[zbus(property)]
    fn ip4_config(&self) -> OwnedObjectPath {
        if self.model.lock().unwrap().connected(self.i) { opath(ip4_path(self.i)) } else { root() }
    }

    #[zbus(property)]
    fn ip6_config(&self) -> OwnedObjectPath {
        root()
    }

    /// The saved connections it could use.
    #[zbus(property)]
    fn available_connections(&self) -> Vec<OwnedObjectPath> {
        let m = self.model.lock().unwrap();
        let kind = m.devices[self.i].kind;
        m.saved.iter().filter(|s| s.kind() == kind).map(|s| opath(conn_path(s.n))).collect()
    }

    async fn disconnect(&self, #[zbus(connection)] conn: &zbus::Connection) -> zbus::fdo::Result<()> {
        let before = {
            let mut m = self.model.lock().unwrap();
            if m.devices[self.i].active.is_none() {
                return Err(fail("not active"));
            }
            let before = m.seen();
            m.deactivate(self.i, DISCONNECTED, USER_REQUESTED);
            before
        };
        announce(conn, &self.model, before).await;
        Ok(())
    }

    /// The connection's settings again (empty: its saved ones as they are
    /// now), without disconnecting.
    async fn reapply(&self, connection: Settings, _version_id: u64, _flags: u32, #[zbus(connection)] conn: &zbus::Connection) -> zbus::fdo::Result<()> {
        let before = {
            let mut m = self.model.lock().unwrap();
            let Some(n) = m.devices[self.i].active else { return Err(fail("not active")) };
            let settings = if connection.is_empty() { m.saved(n).map(|s| s.settings.clone()).unwrap_or_default() } else { connection };
            let before = m.seen();
            let default_metric = m.default_for(m.devices[self.i].kind);
            m.devices[self.i].applied = settings;
            m.devices[self.i].default_metric = default_metric;
            before
        };
        announce(conn, &self.model, before).await;
        Ok(())
    }

    fn get_applied_connection(&self, _flags: u32) -> zbus::fdo::Result<(Settings, u64)> {
        let m = self.model.lock().unwrap();
        if m.devices[self.i].active.is_none() {
            return Err(fail("not active"));
        }
        Ok((m.devices[self.i].applied.clone(), 1))
    }
}

struct WiredObj {
    model: Shared,
    i: usize,
}

#[zbus::interface(name = "org.freedesktop.NetworkManager.Device.Wired")]
impl WiredObj {
    #[zbus(property)]
    fn hw_address(&self) -> String {
        self.model.lock().unwrap().devices[self.i].hw.clone()
    }

    #[zbus(property)]
    fn perm_hw_address(&self) -> String {
        self.hw_address()
    }

    #[zbus(property)]
    fn speed(&self) -> u32 {
        if self.model.lock().unwrap().devices[self.i].state == UNAVAILABLE { 0 } else { 1000 }
    }

    #[zbus(property)]
    fn carrier(&self) -> bool {
        self.model.lock().unwrap().devices[self.i].state != UNAVAILABLE
    }
}

struct WirelessObj {
    model: Shared,
    i: usize,
}

#[zbus::interface(name = "org.freedesktop.NetworkManager.Device.Wireless")]
impl WirelessObj {
    #[zbus(property)]
    fn hw_address(&self) -> String {
        self.model.lock().unwrap().devices[self.i].hw.clone()
    }

    #[zbus(property)]
    fn perm_hw_address(&self) -> String {
        self.hw_address()
    }

    /// Kb/s.
    #[zbus(property)]
    fn bitrate(&self) -> u32 {
        if self.model.lock().unwrap().connected(self.i) { 866_000 } else { 0 }
    }

    #[zbus(property)]
    fn active_access_point(&self) -> OwnedObjectPath {
        self.model.lock().unwrap().devices[self.i].ap.map(|a| opath(ap_path(a))).unwrap_or_else(root)
    }

    #[zbus(property)]
    fn access_points(&self) -> Vec<OwnedObjectPath> {
        self.get_all_access_points()
    }

    fn get_all_access_points(&self) -> Vec<OwnedObjectPath> {
        let m = self.model.lock().unwrap();
        if !m.wireless {
            return Vec::new();
        }
        (0..m.aps.len()).map(|a| opath(ap_path(a))).collect()
    }

    fn get_access_points(&self) -> Vec<OwnedObjectPath> {
        self.get_all_access_points()
    }

    fn request_scan(&self, _options: HashMap<String, OwnedValue>) {}
}

struct ApObj {
    model: Shared,
    i: usize,
}

#[zbus::interface(name = "org.freedesktop.NetworkManager.AccessPoint")]
impl ApObj {
    #[zbus(property)]
    fn ssid(&self) -> Vec<u8> {
        self.model.lock().unwrap().aps[self.i].ssid.as_bytes().to_vec()
    }

    #[zbus(property)]
    fn strength(&self) -> u8 {
        self.model.lock().unwrap().aps[self.i].strength
    }

    #[zbus(property)]
    fn flags(&self) -> u32 {
        u32::from(self.model.lock().unwrap().aps[self.i].secure)
    }

    #[zbus(property)]
    fn wpa_flags(&self) -> u32 {
        0
    }

    /// PSK with CCMP, when secured.
    #[zbus(property)]
    fn rsn_flags(&self) -> u32 {
        if self.model.lock().unwrap().aps[self.i].secure { 0x188 } else { 0 }
    }

    #[zbus(property)]
    fn frequency(&self) -> u32 {
        5180
    }
}

struct ActiveObj {
    model: Shared,
    i: usize,
}

#[zbus::interface(name = "org.freedesktop.NetworkManager.Connection.Active")]
impl ActiveObj {
    #[zbus(property)]
    fn id(&self) -> String {
        let m = self.model.lock().unwrap();
        m.devices[self.i].active.and_then(|n| m.saved(n)).map(|s| s.text("connection", "id")).unwrap_or_default()
    }

    #[zbus(property)]
    fn uuid(&self) -> String {
        let m = self.model.lock().unwrap();
        m.devices[self.i].active.and_then(|n| m.saved(n)).map(|s| s.text("connection", "uuid")).unwrap_or_default()
    }

    #[zbus(property, name = "Type")]
    fn kind(&self) -> String {
        let m = self.model.lock().unwrap();
        m.devices[self.i].active.and_then(|n| m.saved(n)).map(|s| s.text("connection", "type")).unwrap_or_default()
    }

    #[zbus(property)]
    fn connection(&self) -> OwnedObjectPath {
        self.model.lock().unwrap().devices[self.i].active.map(|n| opath(conn_path(n))).unwrap_or_else(root)
    }

    #[zbus(property)]
    fn devices(&self) -> Vec<OwnedObjectPath> {
        vec![opath(dev_path(self.i))]
    }

    /// NM_ACTIVE_CONNECTION_STATE_ACTIVATED.
    #[zbus(property)]
    fn state(&self) -> u32 {
        if self.model.lock().unwrap().connected(self.i) { 2 } else { 4 }
    }

    #[zbus(property)]
    fn default(&self) -> bool {
        self.model.lock().unwrap().primary() == Some(self.i)
    }
}

struct Ip4Obj {
    model: Shared,
    i: usize,
}

#[zbus::interface(name = "org.freedesktop.NetworkManager.IP4Config")]
impl Ip4Obj {
    #[zbus(property)]
    fn address_data(&self) -> Vec<HashMap<String, OwnedValue>> {
        let (addresses, ..) = self.model.lock().unwrap().ip(self.i);
        addresses.into_iter().map(|(a, p)| HashMap::from([("address".to_owned(), value(Value::from(a))), ("prefix".to_owned(), value(Value::from(p)))])).collect()
    }

    #[zbus(property)]
    fn gateway(&self) -> String {
        self.model.lock().unwrap().ip(self.i).1
    }

    #[zbus(property)]
    fn nameserver_data(&self) -> Vec<HashMap<String, OwnedValue>> {
        let (.., dns) = self.model.lock().unwrap().ip(self.i);
        dns.into_iter().map(|a| HashMap::from([("address".to_owned(), value(Value::from(a)))])).collect()
    }
}

// ---- Settings ------------------------------------------------------------------

struct SettingsObj {
    model: Shared,
}

#[zbus::interface(name = "org.freedesktop.NetworkManager.Settings")]
impl SettingsObj {
    fn list_connections(&self) -> Vec<OwnedObjectPath> {
        self.model.lock().unwrap().saved.iter().map(|s| opath(conn_path(s.n))).collect()
    }

    #[zbus(property)]
    fn connections(&self) -> Vec<OwnedObjectPath> {
        self.list_connections()
    }

    #[zbus(property)]
    fn can_modify(&self) -> bool {
        true
    }

    async fn add_connection(&self, mut settings: Settings, #[zbus(object_server)] server: &zbus::ObjectServer) -> zbus::fdo::Result<OwnedObjectPath> {
        let kind = text_of(&settings, "connection", "type");
        if !matches!(kind.as_str(), "802-11-wireless" | "802-3-ethernet") || text_of(&settings, "connection", "uuid").len() != 36 {
            return Err(zbus::fdo::Error::InvalidArgs("not a Wi-Fi or Ethernet connection with a uuid".into()));
        }
        if kind == "802-11-wireless" && !settings.get("802-11-wireless").is_some_and(|w| w.contains_key("ssid")) {
            return Err(zbus::fdo::Error::InvalidArgs("no ssid".into()));
        }
        let psk = settings.get_mut("802-11-wireless-security").and_then(|s| s.remove("psk")).and_then(|v| String::try_from(v).ok()).unwrap_or_default();
        let n = self.model.lock().unwrap().add(settings, psk);
        server.at(conn_path(n).as_str(), Connection { model: self.model.clone(), n }).await?;
        Ok(opath(conn_path(n)))
    }
}

struct Connection {
    model: Shared,
    n: usize,
}

impl Connection {
    fn replace(&self, mut settings: Settings) -> zbus::fdo::Result<()> {
        let mut m = self.model.lock().unwrap();
        let psk = settings.get_mut("802-11-wireless-security").and_then(|s| s.remove("psk")).and_then(|v| String::try_from(v).ok());
        let s = m.saved.iter_mut().find(|s| s.n == self.n).ok_or_else(|| zbus::fdo::Error::UnknownObject(conn_path(self.n)))?;
        // (Secrets not given are kept, as NetworkManager keeps them.)
        if let Some(psk) = psk {
            s.psk = psk;
        }
        s.settings = settings;
        m.write_dump();
        Ok(())
    }
}

#[zbus::interface(name = "org.freedesktop.NetworkManager.Settings.Connection")]
impl Connection {
    fn get_settings(&self) -> zbus::fdo::Result<Settings> {
        let m = self.model.lock().unwrap();
        m.saved(self.n).map(|s| s.settings.clone()).ok_or_else(|| zbus::fdo::Error::UnknownObject(conn_path(self.n)))
    }

    fn get_secrets(&self, setting: String) -> zbus::fdo::Result<Settings> {
        let m = self.model.lock().unwrap();
        let s = m.saved(self.n).ok_or_else(|| zbus::fdo::Error::UnknownObject(conn_path(self.n)))?;
        let mut out = HashMap::new();
        if setting == "802-11-wireless-security" && !s.psk.is_empty() {
            out.insert(setting, HashMap::from([("psk".into(), value(Value::from(s.psk.as_str())))]));
        }
        Ok(out)
    }

    fn update(&self, properties: Settings) -> zbus::fdo::Result<()> {
        self.replace(properties)
    }

    fn update2(&self, settings: Settings, _flags: u32, _args: HashMap<String, OwnedValue>) -> zbus::fdo::Result<HashMap<String, OwnedValue>> {
        if !settings.is_empty() {
            self.replace(settings)?;
        }
        Ok(HashMap::new())
    }

    async fn delete(&self, #[zbus(object_server)] server: &zbus::ObjectServer, #[zbus(connection)] conn: &zbus::Connection) -> zbus::fdo::Result<()> {
        let before = {
            let mut m = self.model.lock().unwrap();
            let before = m.seen();
            m.saved.retain(|s| s.n != self.n);
            for i in 0..m.devices.len() {
                if m.devices[i].active == Some(self.n) {
                    m.deactivate(i, DISCONNECTED, 0);
                }
            }
            m.write_dump();
            before
        };
        announce(conn, &self.model, before).await;
        server.remove::<Connection, _>(conn_path(self.n).as_str()).await?;
        Ok(())
    }
}

// ---- A test's say ------------------------------------------------------------

struct Control {
    model: Shared,
}

#[zbus::interface(name = "org.olo.NmFake")]
impl Control {
    async fn tell(&self, what: String, #[zbus(connection)] conn: &zbus::Connection) -> zbus::fdo::Result<String> {
        let words: Vec<&str> = what.split_whitespace().collect();
        let before = {
            let mut m = self.model.lock().unwrap();
            if words.first() == Some(&"state") {
                return Ok(m.state_json());
            }
            let before = m.seen();
            let kind = |w: &str| if w == "wired" { TYPE_ETHERNET } else { TYPE_WIFI };
            match words.as_slice() {
                [link @ ("wired" | "wifi"), "down"] => {
                    let i = m.devices.iter().position(|d| d.kind == kind(link)).ok_or_else(|| fail("no such link"))?;
                    let (state, reason) = if *link == "wired" { (UNAVAILABLE, CARRIER) } else { (DISCONNECTED, USER_REQUESTED) };
                    m.deactivate(i, state, reason);
                }
                ["wired", "up"] => {
                    let i = m.devices.iter().position(|d| d.kind == TYPE_ETHERNET).ok_or_else(|| fail("no wired link"))?;
                    let n = m.saved.iter().find(|s| s.kind() == TYPE_ETHERNET).map(|s| s.n).ok_or_else(|| fail("no wired connection"))?;
                    m.activate(n, i)?;
                }
                ["wifi", "up", ssid @ ..] => {
                    let ssid = ssid.join(" ");
                    let i = m.devices.iter().position(|d| d.kind == TYPE_WIFI).ok_or_else(|| fail("no Wi-Fi adapter"))?;
                    let n = m.saved.iter().find(|s| s.ssid().as_deref() == Some(ssid.as_str())).map(|s| s.n).ok_or_else(|| fail("no such network saved"))?;
                    m.activate(n, i)?;
                }
                ["strength", n] => {
                    let strength: u8 = n.parse().map_err(fail)?;
                    let i = m.devices.iter().position(|d| d.kind == TYPE_WIFI).ok_or_else(|| fail("no Wi-Fi adapter"))?;
                    let a = m.devices[i].ap.ok_or_else(|| fail("not joined"))?;
                    m.aps[a].strength = strength.min(100);
                }
                _ => return Err(fail(format!("can't: {what}"))),
            }
            before
        };
        announce(conn, &self.model, before).await;
        Ok(self.model.lock().unwrap().state_json())
    }
}

fn wifi_settings(n: usize, ssid: &str, key_mgmt: &str, enterprise: bool) -> Settings {
    let mut s: Settings = HashMap::new();
    s.insert(
        "connection".into(),
        HashMap::from([
            ("id".into(), value(Value::from(ssid))),
            ("uuid".into(), value(Value::from(uuid(n)))),
            ("type".into(), value(Value::from("802-11-wireless"))),
            ("autoconnect".into(), value(Value::from(true))),
        ]),
    );
    s.insert(
        "802-11-wireless".into(),
        HashMap::from([("ssid".into(), value(Value::from(ssid.as_bytes().to_vec()))), ("mode".into(), value(Value::from("infrastructure"))), ("hidden".into(), value(Value::from(false)))]),
    );
    if enterprise {
        s.insert("802-11-wireless-security".into(), HashMap::from([("key-mgmt".into(), value(Value::from("wpa-eap")))]));
        s.insert("802-1x".into(), HashMap::from([("eap".into(), value(Value::from(vec!["peap"])))]));
    } else if !key_mgmt.is_empty() {
        s.insert("802-11-wireless-security".into(), HashMap::from([("key-mgmt".into(), value(Value::from(key_mgmt)))]));
    }
    s.insert("ipv4".into(), HashMap::from([("method".into(), value(Value::from("auto")))]));
    s
}

fn wired_settings(n: usize) -> Settings {
    let mut s: Settings = HashMap::new();
    s.insert(
        "connection".into(),
        HashMap::from([
            ("id".into(), value(Value::from("Wired connection 1"))),
            ("uuid".into(), value(Value::from(uuid(n)))),
            ("type".into(), value(Value::from("802-3-ethernet"))),
            ("autoconnect".into(), value(Value::from(true))),
        ]),
    );
    s.insert("802-3-ethernet".into(), HashMap::new());
    s.insert("ipv4".into(), HashMap::from([("method".into(), value(Value::from("auto")))]));
    s
}

fn main() {
    olo_crash::install();
    let mut address = std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap_or_default();
    let mut model = Model { saved: Vec::new(), next: 0, devices: Vec::new(), aps: Vec::new(), wireless: true, dump: None, defaults: conf_defaults() };
    let (mut wired, mut wifi, mut adapter, mut tell, mut in_range) = (false, None::<String>, false, None::<String>, Vec::new());
    let mut args = std::env::args().skip(1);
    let usage = || -> ! {
        eprintln!("usage: nm-fake [--address ADDRESS] [--network SSID:PASSWORD]… [--enterprise SSID]… [--ap SSID:STRENGTH]… [--wired] [--wifi SSID | --wifi-adapter] [--dump FILE] | --tell WHAT");
        std::process::exit(2)
    };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--address" => address = args.next().unwrap_or_else(|| usage()),
            "--network" => {
                let net = args.next().unwrap_or_else(|| usage());
                let (ssid, psk) = net.split_once(':').unwrap_or((net.as_str(), ""));
                let key_mgmt = if psk.is_empty() { "" } else { "wpa-psk" };
                let n = model.next + 1;
                model.add(wifi_settings(n, ssid, key_mgmt, false), psk.into());
                in_range.push((ssid.to_owned(), None, !psk.is_empty()));
            }
            "--enterprise" => {
                let ssid = args.next().unwrap_or_else(|| usage());
                let n = model.next + 1;
                model.add(wifi_settings(n, &ssid, "wpa-eap", true), String::new());
                in_range.push((ssid, None, true));
            }
            "--ap" => {
                let a = args.next().unwrap_or_else(|| usage());
                let (ssid, strength) = a.rsplit_once(':').unwrap_or((a.as_str(), "50"));
                in_range.push((ssid.to_owned(), Some(strength.parse::<u8>().unwrap_or_else(|_| usage())), true));
            }
            "--wired" => wired = true,
            "--wifi" => wifi = Some(args.next().unwrap_or_else(|| usage())),
            "--wifi-adapter" => adapter = true,
            "--dump" => model.dump = Some(PathBuf::from(args.next().unwrap_or_else(|| usage()))),
            "--tell" => tell = Some(args.next().unwrap_or_else(|| usage())),
            _ => usage(),
        }
    }
    if let Some(what) = tell {
        let answer = zbus::blocking::connection::Builder::address(address.as_str())
            .and_then(|b| b.build())
            .and_then(|c| c.call_method(Some(NM), CONTROL_PATH, Some("org.olo.NmFake"), "Tell", &(what.as_str(),)).and_then(|r| r.body().deserialize::<String>()));
        match answer {
            Ok(a) => println!("{a}"),
            Err(e) => {
                eprintln!("nm-fake: {e}");
                std::process::exit(1);
            }
        }
        return;
    }
    for (k, (ssid, strength, secure)) in in_range.into_iter().enumerate() {
        if !model.aps.iter().any(|a| a.ssid == ssid) {
            model.aps.push(Ap { ssid, strength: strength.unwrap_or(80u8.saturating_sub(16 * k as u8).max(20)), secure });
        }
    }
    if wired {
        let n = model.next + 1;
        model.add(wired_settings(n), String::new());
        model.devices.push(Dev {
            iface: "enp3s0".into(),
            kind: TYPE_ETHERNET,
            hw: "02:00:00:00:01:01".into(),
            state: DISCONNECTED,
            reason: 0,
            active: None,
            applied: Settings::new(),
            ap: None,
            lease: Lease { address: "192.168.1.20".into(), gateway: "192.168.1.1".into(), dns: vec!["192.168.1.1".into()] },
            default_metric: 100,
        });
        let i = model.devices.len() - 1;
        model.activate(n, i).expect("the wired connection");
    }
    if wifi.is_some() || adapter {
        model.devices.push(Dev {
            iface: "wlan0".into(),
            kind: TYPE_WIFI,
            hw: "02:00:00:00:02:01".into(),
            state: DISCONNECTED,
            reason: 0,
            active: None,
            applied: Settings::new(),
            ap: None,
            lease: Lease { address: "10.0.0.42".into(), gateway: "10.0.0.1".into(), dns: vec!["10.0.0.1".into()] },
            default_metric: 600,
        });
        if let Some(ssid) = &wifi {
            if !model.aps.iter().any(|a| a.ssid == *ssid) {
                model.aps.insert(0, Ap { ssid: ssid.clone(), strength: 80, secure: false });
            }
            let n = match model.saved.iter().find(|s| s.ssid().as_deref() == Some(ssid.as_str())) {
                Some(s) => s.n,
                None => {
                    let n = model.next + 1;
                    model.add(wifi_settings(n, ssid, "", false), String::new())
                }
            };
            let i = model.devices.len() - 1;
            model.activate(n, i).expect("the Wi-Fi network");
        }
    }
    let (saved, devices, aps) = (model.saved.iter().map(|s| s.n).collect::<Vec<_>>(), model.devices.iter().map(|d| d.kind).collect::<Vec<_>>(), model.aps.len());
    model.write_dump();
    let model: Shared = Arc::new(Mutex::new(model));
    let conn = match zbus::blocking::connection::Builder::address(address.as_str()).and_then(|b| b.name(NM)).and_then(|b| b.build()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("nm-fake: can't take {NM} on {address}: {e}");
            std::process::exit(1);
        }
    };
    let server = conn.object_server();
    let m = || model.clone();
    server.at(NM_PATH, Nm { model: m() }).expect("NetworkManager");
    server.at(SETTINGS_PATH, SettingsObj { model: m() }).expect("settings");
    server.at(CONTROL_PATH, Control { model: m() }).expect("control");
    for n in &saved {
        server.at(conn_path(*n).as_str(), Connection { model: m(), n: *n }).expect("a connection");
    }
    for (i, kind) in devices.iter().enumerate() {
        let p = dev_path(i);
        server.at(p.as_str(), DeviceObj { model: m(), i }).expect("a device");
        if *kind == TYPE_ETHERNET {
            server.at(p.as_str(), WiredObj { model: m(), i }).expect("a wired device");
        } else {
            server.at(p.as_str(), WirelessObj { model: m(), i }).expect("a Wi-Fi device");
        }
        server.at(active_path(i).as_str(), ActiveObj { model: m(), i }).expect("an active connection");
        server.at(ip4_path(i).as_str(), Ip4Obj { model: m(), i }).expect("an IPv4 configuration");
    }
    for i in 0..aps {
        server.at(ap_path(i).as_str(), ApObj { model: m(), i }).expect("an access point");
    }
    eprintln!("nm-fake: serving {} saved connections and {} links on {address}", saved.len(), devices.len());
    loop {
        std::thread::park();
    }
}
