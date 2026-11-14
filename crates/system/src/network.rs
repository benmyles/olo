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
const DHCP4: &str = "org.freedesktop.NetworkManager.DHCP4Config";

/// The bus NetworkManager is on: the system bus, or `OLO_NM_BUS` (a
/// D-Bus address: a stand-in's, `nm-fake`, for tests).
fn conn() -> Result<&'static zbus::blocking::Connection> {
    static TEST: std::sync::OnceLock<zbus::blocking::Connection> = std::sync::OnceLock::new();
    match std::env::var("OLO_NM_BUS") {
        Ok(address) if !address.is_empty() => {
            if let Some(c) = TEST.get() {
                return Ok(c);
            }
            let c = zbus::blocking::connection::Builder::address(address.as_str())?.build()?;
            Ok(TEST.get_or_init(|| c))
        }
        _ => bus::system(),
    }
}

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
    let conn = conn()?;
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

/// The links Deskbar's tray shows (Ben, 2026-11-05: "when ethernet is
/// connected it should also show an ethernet icon ... if both are
/// connected at once the one that is the default route/link should be
/// shown as "stronger""): Wi-Fi ([`wifi`]) and its adapter, the first
/// wired link that's up, and which kind carries the default route.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Links {
    pub wifi: Option<Wifi>,
    /// The Wi-Fi adapter: its addresses, gateway, DNS and rate.
    pub wifi_device: Option<Device>,
    /// The first wired link that's connected.
    pub wired: Option<Device>,
    /// What carries the default route (NetworkManager's primary
    /// connection): None for neither (a VPN, nothing).
    pub primary: Option<Kind>,
    /// A network that wants its sign-in page first ([`Portal`]).
    pub portal: Option<Portal>,
}

/// NM_CONNECTIVITY_PORTAL: what NetworkManager's connectivity check finds
/// behind a captive portal (its request answered by something else).
const CONNECTIVITY_PORTAL: u32 = 2;

/// The address NetworkManager's check asks on Fedora, when it doesn't
/// say its own.
const FEDORA_CHECK: &str = "http://fedoraproject.org/static/hotspot.txt";

/// A network that wants its sign-in page first: a captive portal (Ben,
/// 2026-11-13: "add captive portal recognition in network settings so
/// that when a connection requires portal signin the link is available
/// to click"). Seen by NetworkManager's own connectivity check, as its
/// signals say: nothing polled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Portal {
    /// The network's name (the connection's: a Wi-Fi network's is its
    /// name, "Airport Wi-Fi"; "Wired connection 1").
    pub network: String,
    /// Its link (the device's path).
    pub device: String,
    /// Where to sign in: the page the network names (DHCP option 114,
    /// RFC 8910), else NetworkManager's check address, a plain HTTP page
    /// the portal answers with its own (or sends on to it).
    pub url: String,
    /// `url` is the page the network names (worth showing: the check's
    /// address isn't the portal's).
    pub named: bool,
}

/// The network behind a captive portal among `devices`, if any. The
/// computer's connectivity (`Connectivity`, one call: what's asked on
/// every change of the links) is its default route's link's, so a portal
/// that holds the traffic back shows there first; then the link is the
/// one whose own check says so (`Ip4Connectivity` or `Ip6Connectivity`),
/// else (NetworkManager before 1.16 doesn't say) the one carrying the
/// default route.
fn portal_of(c: &zbus::blocking::Connection, devices: &[&Device]) -> Option<Portal> {
    let whole: u32 = bus::property(c, DEST, NM_PATH, NM, "Connectivity").ok()?;
    if whole != CONNECTIVITY_PORTAL {
        return None;
    }
    let behind = |d: &&&Device| ["Ip4Connectivity", "Ip6Connectivity"].iter().any(|p| bus::property::<u32>(c, DEST, &d.path, DEVICE, p).ok() == Some(CONNECTIVITY_PORTAL));
    let connected: Vec<&&Device> = devices.iter().filter(|d| d.connected()).collect();
    let device = connected.iter().copied().find(behind).copied().or_else(|| {
        let primary: OwnedObjectPath = bus::property(c, DEST, NM_PATH, NM, "PrimaryConnection").ok()?;
        let links: Vec<OwnedObjectPath> = bus::property(c, DEST, primary.as_str(), ACTIVE, "Devices").ok()?;
        let first = links.first()?;
        connected.iter().find(|d| d.path == first.as_str()).copied().copied()
    })?;
    // The page its DHCP names, if it does (an http or https address).
    let dhcp: OwnedObjectPath = bus::property(c, DEST, &device.path, DEVICE, "Dhcp4Config").unwrap_or_else(|_| OwnedObjectPath::try_from("/").expect("a path"));
    let named = (dhcp.as_str() != "/")
        .then(|| bus::property::<HashMap<String, OwnedValue>>(c, DEST, dhcp.as_str(), DHCP4, "Options").ok())
        .flatten()
        .and_then(|o| o.get("captive_portal").and_then(|v| String::try_from(v.clone()).ok()))
        .filter(|u| u.starts_with("https://") || u.starts_with("http://"));
    let check = || Some(str_prop(NM_PATH, NM, "ConnectivityCheckUri")).filter(|u| !u.is_empty()).unwrap_or_else(|| FEDORA_CHECK.to_owned());
    Some(Portal { network: device.connection.clone().unwrap_or_else(|| device.interface.clone()), device: device.path.clone(), named: named.is_some(), url: named.unwrap_or_else(check) })
}

/// NetworkManager's connectivity check made again now: whether the
/// computer is still behind a captive portal.
pub fn check_portal() -> Result<bool> {
    let found: u32 = bus::call(conn()?, DEST, NM_PATH, NM, "CheckConnectivity", &())?;
    Ok(found == CONNECTIVITY_PORTAL)
}

/// Opens `portal`'s sign-in page in the default browser, then has
/// NetworkManager check again every few seconds while it's open (a few
/// minutes at most, and only one such round at a time), so the warning
/// goes as soon as the network lets traffic through, not at its next
/// check of its own (every five minutes).
pub fn open_portal(portal: &Portal) {
    if let Err(e) = olo_xdg::open::open(&olo_xdg::open::Target::Url(portal.url.clone())) {
        tracing::warn!("network: the sign-in page {} couldn't be opened: {e}", portal.url);
    }
    static CHECKING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if CHECKING.swap(true, std::sync::atomic::Ordering::AcqRel) {
        return;
    }
    let spawned = std::thread::Builder::new().name("portal-check".into()).spawn(|| {
        for _ in 0..PORTAL_CHECKS {
            std::thread::sleep(PORTAL_CHECK_EVERY);
            if !check_portal().unwrap_or(false) {
                break;
            }
        }
        CHECKING.store(false, std::sync::atomic::Ordering::Release);
    });
    if spawned.is_err() {
        CHECKING.store(false, std::sync::atomic::Ordering::Release);
    }
}

/// How often, and how many times, NetworkManager is asked to check again
/// after a sign-in page is opened: every 5 s for 5 minutes.
const PORTAL_CHECK_EVERY: std::time::Duration = std::time::Duration::from_secs(5);
const PORTAL_CHECKS: u32 = 60;

/// [`Links`] now, and the objects whose changes change them besides
/// NetworkManager's own: each link connected and its addresses, the
/// access point joined.
fn links_and_paths() -> Result<(Links, Vec<String>)> {
    let c = conn()?;
    let mut links = Links::default();
    let mut paths = vec![NM_PATH.to_owned()];
    for p in devices_paths()? {
        let Ok(d) = device(&p) else { continue };
        let first_wifi = d.kind == Kind::Wifi && links.wifi_device.is_none();
        if !first_wifi && !(d.kind == Kind::Ethernet && links.wired.is_none() && d.connected()) {
            continue;
        }
        if d.connected() {
            paths.push(d.path.clone());
            let ip4: OwnedObjectPath = bus::property(c, DEST, p.as_str(), DEVICE, "Ip4Config")?;
            if ip4.as_str() != "/" {
                paths.push(ip4.as_str().to_owned());
            }
        }
        if !first_wifi {
            links.wired = Some(d);
            continue;
        }
        let ap: OwnedObjectPath = bus::property(c, DEST, p.as_str(), WIRELESS, "ActiveAccessPoint")?;
        links.wifi = Some(if ap.as_str() == "/" {
            Wifi { ssid: None, strength: 0 }
        } else {
            paths.push(ap.as_str().to_owned());
            let ssid: Vec<u8> = bus::property(c, DEST, ap.as_str(), AP, "Ssid")?;
            let strength: u8 = bus::property(c, DEST, ap.as_str(), AP, "Strength")?;
            Wifi { ssid: Some(String::from_utf8_lossy(&ssid).into_owned()), strength: strength.min(100) as u32 }
        });
        links.wifi_device = Some(d);
    }
    let primary: String = bus::property(c, DEST, NM_PATH, NM, "PrimaryConnectionType").unwrap_or_default();
    links.primary = match primary.as_str() {
        "802-11-wireless" => Some(Kind::Wifi),
        "802-3-ethernet" => Some(Kind::Ethernet),
        _ => None,
    };
    let shown: Vec<&Device> = links.wifi_device.iter().chain(links.wired.iter()).collect();
    links.portal = portal_of(c, &shown);
    // (A Wi-Fi network by the name it goes by, as the tray says it.)
    if let Some(p) = links.portal.as_mut()
        && links.wifi_device.as_ref().is_some_and(|d| d.path == p.device)
        && let Some(ssid) = links.wifi.as_ref().and_then(|w| w.ssid.clone())
    {
        p.network = ssid;
    }
    Ok((links, paths))
}

/// [`Links`] now.
pub fn links() -> Result<Links> {
    links_and_paths().map(|(l, _)| l)
}

/// A link's rate in words: "1 Gb/s", "866 Mb/s".
pub fn speed_text(mbps: u32) -> String {
    if mbps >= 1000 && mbps.is_multiple_of(1000) {
        format!("{} Gb/s", mbps / 1000)
    } else if mbps >= 1000 {
        format!("{:.1} Gb/s", mbps as f32 / 1000.0)
    } else {
        format!("{mbps} Mb/s")
    }
}

/// A link's bytes received and sent so far (its interface's counters).
pub fn traffic(interface: &str) -> Option<(u64, u64)> {
    let read = |what: &str| std::fs::read_to_string(format!("/sys/class/net/{interface}/statistics/{what}")).ok()?.trim().parse::<u64>().ok();
    Some((read("rx_bytes")?, read("tx_bytes")?))
}

/// [`watch_links`] for Wi-Fi alone.
pub fn watch_wifi(changed: impl Fn(Option<Wifi>) + Send + 'static) -> Result<()> {
    watch_links(move |l| changed(l.wifi))
}

/// Calls `changed` with the links ([`Links`]) now and whenever they
/// change, on threads of its own, following
/// NetworkManager's signals (nothing polled), on a bus connection of its
/// own that the bus sends only these: any device's state changing,
/// NetworkManager's own (devices coming and going, the default route
/// moving), and for each link connected, its properties (moving to
/// another access point), its addresses' and its access point's (the
/// strength). An adapter that isn't connected scans
/// often, its list of access points changing several times a second:
/// those aren't asked for. Quick runs of changes come together. Err if
/// NetworkManager can't be reached.
pub fn watch_links(changed: impl Fn(Links) + Send + 'static) -> Result<()> {
    use zbus::{MatchRule, message::Type};
    // A connection of its own (it hears every signal asked for).
    let conn = match std::env::var("OLO_NM_BUS") {
        Ok(address) if !address.is_empty() => zbus::blocking::connection::Builder::address(address.as_str())?.build()?,
        _ => zbus::blocking::Connection::system()?,
    };
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
    let (first, paths) = links_and_paths()?;
    let (tx, rx) = std::sync::mpsc::channel::<zbus::Message>();
    std::thread::Builder::new().name("network-signals".into()).spawn(move || {
        for m in messages.flatten() {
            if m.message_type() == Type::Signal && m.header().sender().is_some_and(|s| s.as_str() != "org.freedesktop.DBus") && tx.send(m).is_err() {
                return;
            }
        }
    })?;
    std::thread::Builder::new().name("network".into()).spawn(move || {
        // What's asked for besides: the links connected, their addresses
        // and the access point's properties (nothing while none is),
        // asked again only when those change.
        let (mut asked, mut followed): (Vec<MatchRule<'static>>, Vec<String>) = (Vec::new(), Vec::new());
        let mut follow = |paths: &[String]| {
            if paths == followed.as_slice() {
                return;
            }
            for rule in asked.drain(..) {
                let _ = dbus.remove_match_rule(rule);
            }
            for path in paths.iter().skip(1) {
                if let Ok(rule) = props(path)
                    && dbus.add_match_rule(rule.clone()).is_ok()
                {
                    asked.push(rule);
                }
            }
            followed = paths.to_vec();
        };
        follow(&paths);
        let mut last = first;
        changed(last.clone());
        while let Ok(m) = rx.recv() {
            // Let a burst finish (joining changes a dozen properties).
            std::thread::sleep(std::time::Duration::from_millis(80));
            let mut burst = vec![m];
            burst.extend(rx.try_iter());
            // The strength or the rate alone (every few seconds) is in
            // the signal; anything else is read again (two dozen calls).
            let now = match quick(&last, &burst) {
                Some(now) => now,
                None => {
                    let Ok((now, paths)) = links_and_paths() else { continue };
                    follow(&paths);
                    now
                }
            };
            if now != last {
                changed(now.clone());
                last = now;
            }
        }
    })?;
    Ok(())
}

/// `last` as `burst` changes it, if all it says is the access point's
/// strength or the adapter's rate (`PropertiesChanged` carries the new
/// values): None if anything else, to read them all again.
fn quick(last: &Links, burst: &[zbus::Message]) -> Option<Links> {
    let mut now = last.clone();
    for m in burst {
        let h = m.header();
        if h.member()?.as_str() != "PropertiesChanged" {
            return None;
        }
        let (iface, changed, invalidated): (String, HashMap<String, OwnedValue>, Vec<String>) = m.body().deserialize().ok()?;
        if !invalidated.is_empty() {
            return None;
        }
        for (name, v) in &changed {
            match (iface.as_str(), name.as_str()) {
                (AP, "Strength") => now.wifi.as_mut().filter(|w| w.ssid.is_some())?.strength = u8::try_from(v).ok()?.min(100) as u32,
                (WIRELESS, "Bitrate") => {
                    let d = now.wifi_device.as_mut().filter(|d| h.path().is_some_and(|p| p.as_str() == d.path))?;
                    d.speed = Some(u32::try_from(v).ok()? / 1000).filter(|s| *s > 0);
                }
                _ => return None,
            }
        }
    }
    Some(now)
}

fn devices_paths() -> Result<Vec<OwnedObjectPath>> {
    bus::property(conn()?, DEST, NM_PATH, NM, "Devices")
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
    /// A network that wants its sign-in page first ([`Portal`]).
    pub portal: Option<Portal>,
}

fn str_prop(dev: &str, iface: &str, name: &str) -> String {
    conn().ok().and_then(|c| bus::property::<String>(c, DEST, dev, iface, name).ok()).unwrap_or_default()
}

fn device(path: &OwnedObjectPath) -> Result<Device> {
    let c = conn()?;
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
    let c = conn()?;
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
    let c = conn()?;
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
    let portal = portal_of(c, &devices.iter().collect::<Vec<_>>());
    Ok(Status { wireless_enabled, wireless_hardware, devices, networks, portal })
}

/// Have NetworkManager read its configuration files again (after one
/// is written; as root, or whoever polkit lets).
pub fn reload() -> Result<()> {
    bus::call::<_, ()>(conn()?, DEST, NM_PATH, NM, "Reload", &(0u32,))
}

pub fn set_wireless_enabled(on: bool) -> Result<()> {
    bus::set_property(conn()?, DEST, NM_PATH, NM, "WirelessEnabled", on)
}

/// Ask a Wi-Fi device to look for networks (NM limits how often).
pub fn scan(device: &str) -> Result<()> {
    let opts: HashMap<&str, Value> = HashMap::new();
    bus::call::<_, ()>(conn()?, DEST, device, WIRELESS, "RequestScan", &(opts,))
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
    let c = conn()?;
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
    let c = conn()?;
    let dev = zvariant::ObjectPath::try_from(device)?;
    let ap = zvariant::ObjectPath::try_from(network.ap.as_str())?;
    let settings = enterprise_settings(e);
    let _: (OwnedObjectPath, OwnedObjectPath) = bus::call(c, DEST, NM_PATH, NM, "AddAndActivateConnection", &(settings, dev, ap))?;
    Ok(())
}

pub fn disconnect(device: &str) -> Result<()> {
    bus::call::<_, ()>(conn()?, DEST, device, DEVICE, "Disconnect", &())
}

/// A saved connection's settings, as NetworkManager gives them.
pub type Settings = HashMap<String, HashMap<String, OwnedValue>>;

fn owned(v: Value<'_>) -> Result<OwnedValue> {
    v.try_to_owned().map_err(zbus::Error::Variant)
}

/// The saved connection a link uses: the one it's connected with, else
/// the first it could use.
pub fn profile_of(device: &str) -> Result<Option<String>> {
    let c = conn()?;
    let active: OwnedObjectPath = bus::property(c, DEST, device, DEVICE, "ActiveConnection")?;
    if active.as_str() != "/" {
        let p: OwnedObjectPath = bus::property(c, DEST, active.as_str(), ACTIVE, "Connection")?;
        if p.as_str() != "/" {
            return Ok(Some(p.as_str().to_owned()));
        }
    }
    let available: Vec<OwnedObjectPath> = bus::property(c, DEST, device, DEVICE, "AvailableConnections").unwrap_or_default();
    Ok(available.first().map(|p| p.as_str().to_owned()))
}

pub fn profile_settings(profile: &str) -> Result<Settings> {
    bus::call(conn()?, DEST, profile, CONNECTION, "GetSettings", &())
}

/// `profile`'s settings as `change` makes them, saved (its secrets kept:
/// NetworkManager keeps them when none are given).
pub fn update_profile(profile: &str, change: impl FnOnce(&mut Settings) -> Result<()>) -> Result<()> {
    let mut s = profile_settings(profile)?;
    change(&mut s)?;
    // NM_SETTINGS_UPDATE2_FLAG_TO_DISK.
    let _: HashMap<String, OwnedValue> = bus::call(conn()?, DEST, profile, CONNECTION, "Update2", &(s, 1u32, HashMap::<String, Value>::new()))?;
    Ok(())
}

/// Every Ethernet and Wi-Fi connection saved.
pub fn link_profiles() -> Result<Vec<String>> {
    let paths: Vec<OwnedObjectPath> = bus::call(conn()?, DEST, SETTINGS_PATH, SETTINGS, "ListConnections", &())?;
    Ok(paths
        .into_iter()
        .filter(|p| {
            profile_settings(p.as_str())
                .ok()
                .is_some_and(|s| matches!(get(&s, "connection", "type").and_then(|v| String::try_from(v.clone()).ok()).as_deref(), Some("802-3-ethernet" | "802-11-wireless")))
        })
        .map(|p| p.as_str().to_owned())
        .collect())
}

/// A link's connection again, as saved now and with NetworkManager's
/// defaults as they are now (a route metric), without disconnecting.
pub fn reapply(device: &str) -> Result<()> {
    bus::call::<_, ()>(conn()?, DEST, device, DEVICE, "Reapply", &(HashMap::<String, HashMap<String, Value>>::new(), 0u64, 0u32))
}

/// [`reapply`] for every link connected.
pub fn reapply_all() -> Result<()> {
    for p in devices_paths()? {
        if device(&p).is_ok_and(|d| d.connected()) {
            reapply(p.as_str())?;
        }
    }
    Ok(())
}

/// Connect a wired link: with its saved connection, or a new one.
pub fn connect(device: &str) -> Result<()> {
    let c = conn()?;
    let dev = zvariant::ObjectPath::try_from(device)?;
    let root = zvariant::ObjectPath::try_from("/")?;
    match profile_of(device)? {
        Some(p) => {
            let p = zvariant::ObjectPath::try_from(p.as_str())?;
            let _: OwnedObjectPath = bus::call(c, DEST, NM_PATH, NM, "ActivateConnection", &(p, dev, root))?;
        }
        None => {
            let settings: HashMap<&str, HashMap<&str, Value>> = HashMap::from([("connection", HashMap::from([("type", Value::from("802-3-ethernet"))]))]);
            let _: (OwnedObjectPath, OwnedObjectPath) = bus::call(c, DEST, NM_PATH, NM, "AddAndActivateConnection", &(settings, dev, root))?;
        }
    }
    Ok(())
}

/// How a connection gets its IPv6 address.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Ipv6 {
    /// From the router's advertisements (and DHCPv6).
    #[default]
    Auto,
    /// Only the link's own (fe80::), to reach what's on it.
    LinkLocal,
    Off,
    /// Addresses of its own, set elsewhere: left as they are.
    Manual,
}

impl Ipv6 {
    fn method(self) -> &'static str {
        match self {
            Ipv6::Auto => "auto",
            Ipv6::LinkLocal => "link-local",
            Ipv6::Off => "disabled",
            Ipv6::Manual => "manual",
        }
    }
}

/// How a connection gets its addresses: IPv4 from DHCP or its own, DNS of
/// its own, IPv6, and the link's MTU.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Addressing {
    /// Its own IPv4 address, not DHCP's.
    pub manual: bool,
    /// "192.168.1.20/24" (manual).
    pub address: String,
    /// The router (manual).
    pub gateway: String,
    /// DNS servers of its own, asked instead of what DHCP says.
    pub dns: Vec<String>,
    #[serde(default)]
    pub ipv6: Ipv6,
    /// Bytes; 0 for the link's own.
    #[serde(default)]
    pub mtu: u32,
}

impl Addressing {
    pub fn check(&self) -> std::result::Result<(), String> {
        if self.mtu != 0 && !(576..=65535).contains(&self.mtu) {
            return Err(format!("an MTU of {} isn't one a link has (576 to 65535)", self.mtu));
        }
        if self.mtu != 0 && self.mtu < 1280 && !matches!(self.ipv6, Ipv6::Off) {
            return Err("IPv6 needs an MTU of 1280 or more".into());
        }
        if self.manual {
            let (a, prefix) = self.address.split_once('/').unwrap_or((self.address.as_str(), "24"));
            if a.parse::<std::net::Ipv4Addr>().is_err() || !prefix.parse::<u8>().is_ok_and(|p| (1..=32).contains(&p)) {
                return Err(format!("“{}” isn't an IPv4 address (192.168.1.20/24)", self.address));
            }
            if !self.gateway.is_empty() && self.gateway.parse::<std::net::Ipv4Addr>().is_err() {
                return Err(format!("“{}” isn't a router's address", self.gateway));
            }
        }
        if let Some(bad) = self.dns.iter().find(|d| d.parse::<std::net::Ipv4Addr>().is_err()) {
            return Err(format!("“{bad}” isn't an IPv4 address"));
        }
        Ok(())
    }
}

/// A connection's IPv4 settings.
pub fn profile_addressing(profile: &str) -> Result<Addressing> {
    Ok(addressing_of(&profile_settings(profile)?))
}

pub fn addressing_of(s: &Settings) -> Addressing {
    let text = |k: &str| get(s, "ipv4", k).and_then(|v| String::try_from(v.clone()).ok()).unwrap_or_default();
    let address = get(s, "ipv4", "address-data")
        .and_then(|v| Vec::<HashMap<String, OwnedValue>>::try_from(v.clone()).ok())
        .and_then(|l| l.into_iter().next())
        .and_then(|a| Some(format!("{}/{}", String::try_from(a.get("address")?.clone()).ok()?, u32::try_from(a.get("prefix")?).ok()?)))
        .unwrap_or_default();
    let dns = get(s, "ipv4", "dns").and_then(|v| Vec::<u32>::try_from(v.clone()).ok()).unwrap_or_default();
    let dns = dns.into_iter().map(|a| std::net::Ipv4Addr::from(a.to_ne_bytes()).to_string()).collect();
    let ipv6 = match get(s, "ipv6", "method").and_then(|v| String::try_from(v.clone()).ok()).as_deref() {
        Some("link-local") => Ipv6::LinkLocal,
        Some("disabled" | "ignore") => Ipv6::Off,
        Some("manual") => Ipv6::Manual,
        _ => Ipv6::Auto,
    };
    let mtu = ["802-3-ethernet", "802-11-wireless"].iter().find_map(|g| get(s, g, "mtu").and_then(|v| u32::try_from(v).ok())).unwrap_or(0);
    // (From DHCP, an address or router of its own besides isn't what's
    // edited here.)
    match text("method") == "manual" {
        true => Addressing { manual: true, address, gateway: text("gateway"), dns, ipv6, mtu },
        false => Addressing { manual: false, dns, ipv6, mtu, ..Addressing::default() },
    }
}

/// `v` written into `s`'s IPv4 settings.
pub fn set_addressing_in(s: &mut Settings, v: &Addressing) -> Result<()> {
    v.check().map_err(zbus::Error::Failure)?;
    let ip = s.entry("ipv4".into()).or_default();
    // (The old forms of the addresses go: the new win only alone.)
    for k in ["addresses", "address-data", "gateway", "dns", "dns-data", "ignore-auto-dns"] {
        ip.remove(k);
    }
    ip.insert("method".into(), owned(Value::from(if v.manual { "manual" } else { "auto" }))?);
    if v.manual {
        let (a, prefix) = v.address.split_once('/').unwrap_or((v.address.as_str(), "24"));
        let entry: HashMap<String, Value> = HashMap::from([("address".to_owned(), Value::from(a.to_owned())), ("prefix".to_owned(), Value::from(prefix.parse::<u32>().unwrap_or(24)))]);
        ip.insert("address-data".into(), owned(Value::from(vec![entry]))?);
        if !v.gateway.is_empty() {
            ip.insert("gateway".into(), owned(Value::from(v.gateway.as_str()))?);
        }
    }
    let dns: Vec<u32> = v.dns.iter().filter_map(|d| d.parse::<std::net::Ipv4Addr>().ok()).map(|a| u32::from_ne_bytes(a.octets())).collect();
    if !dns.is_empty() {
        ip.insert("dns".into(), owned(Value::from(dns))?);
        ip.insert("ignore-auto-dns".into(), owned(Value::from(true))?);
    }
    // IPv6, unless its own addresses were set elsewhere.
    if v.ipv6 != Ipv6::Manual {
        let ip6 = s.entry("ipv6".into()).or_default();
        for k in ["addresses", "address-data", "gateway"] {
            ip6.remove(k);
        }
        ip6.insert("method".into(), owned(Value::from(v.ipv6.method()))?);
    }
    let kind = get(s, "connection", "type").and_then(|v| String::try_from(v.clone()).ok()).unwrap_or_default();
    if matches!(kind.as_str(), "802-3-ethernet" | "802-11-wireless") {
        s.entry(kind).or_default().insert("mtu".into(), owned(Value::from(v.mtu))?);
    }
    Ok(())
}

/// Each Ethernet and Wi-Fi connection's search domains: `remove` taken
/// out (what Olo gave them before), `add` put in.
pub fn set_profile_searches(remove: &[String], add: &[String]) -> Result<()> {
    for p in link_profiles()? {
        let s = profile_settings(&p)?;
        let mut changed = false;
        let mut next = s.clone();
        for family in ["ipv4", "ipv6"] {
            let Some(group) = next.get_mut(family) else { continue };
            let now: Vec<String> = group.get("dns-search").and_then(|v| Vec::<String>::try_from(v.clone()).ok()).unwrap_or_default();
            let mut want: Vec<String> = now.iter().filter(|d| !remove.contains(d)).cloned().collect();
            for d in add {
                if !want.contains(d) {
                    want.push(d.clone());
                }
            }
            if want != now {
                group.insert("dns-search".into(), owned(Value::from(want))?);
                changed = true;
            }
        }
        if changed {
            let _: HashMap<String, OwnedValue> = bus::call(conn()?, DEST, p.as_str(), CONNECTION, "Update2", &(next, 1u32, HashMap::<String, Value>::new()))?;
        }
    }
    Ok(())
}

/// A saved Wi-Fi network as a backup carries it to another computer: its
/// name, how it's secured, and its password.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SavedWifi {
    /// The connection's name ("Home").
    pub id: String,
    /// The network's name as it's sent (bytes: not always UTF-8).
    pub ssid: Vec<u8>,
    pub hidden: bool,
    pub autoconnect: bool,
    /// "wpa-psk", "sae", "owe"; empty for an open network.
    pub key_mgmt: String,
    /// WPA's (or SAE's) password, if NetworkManager gave it.
    pub psk: Option<String>,
}

impl SavedWifi {
    pub fn name(&self) -> String {
        String::from_utf8_lossy(&self.ssid).into_owned()
    }
}

fn get<'a>(s: &'a HashMap<String, HashMap<String, OwnedValue>>, group: &str, key: &str) -> Option<&'a OwnedValue> {
    s.get(group).and_then(|g| g.get(key))
}

/// Every saved Wi-Fi network that can go to another computer, with its
/// password (NetworkManager asks polkit: a prompt, through Olo's agent,
/// if it wants one), and the names of those that can't: enterprise ones
/// (this computer's certificates and files) and WEP ones. Hotspots this
/// computer makes aren't networks to join, and are left out.
pub fn saved_wifi() -> Result<(Vec<SavedWifi>, Vec<String>)> {
    let c = conn()?;
    let paths: Vec<OwnedObjectPath> = bus::call(c, DEST, SETTINGS_PATH, SETTINGS, "ListConnections", &())?;
    let (mut out, mut left) = (Vec::new(), Vec::new());
    for p in paths {
        let Ok(settings) = bus::call::<_, HashMap<String, HashMap<String, OwnedValue>>>(c, DEST, p.as_str(), CONNECTION, "GetSettings", &()) else { continue };
        let Some(ssid) = get(&settings, "802-11-wireless", "ssid").and_then(|v| Vec::<u8>::try_from(v.clone()).ok()) else { continue };
        let text = |g: &str, k: &str| get(&settings, g, k).and_then(|v| String::try_from(v.clone()).ok()).unwrap_or_default();
        let flag = |g: &str, k: &str, or: bool| get(&settings, g, k).and_then(|v| bool::try_from(v.clone()).ok()).unwrap_or(or);
        let mode = text("802-11-wireless", "mode");
        if !mode.is_empty() && mode != "infrastructure" {
            continue;
        }
        let id = text("connection", "id");
        let key_mgmt = text("802-11-wireless-security", "key-mgmt");
        let name = String::from_utf8_lossy(&ssid).into_owned();
        if settings.contains_key("802-1x") || key_mgmt == "wpa-eap" || key_mgmt == "ieee8021x" || (key_mgmt == "none" && settings.contains_key("802-11-wireless-security")) {
            left.push(name);
            continue;
        }
        let psk = if matches!(key_mgmt.as_str(), "wpa-psk" | "sae") {
            match bus::call::<_, HashMap<String, HashMap<String, OwnedValue>>>(c, DEST, p.as_str(), CONNECTION, "GetSecrets", &("802-11-wireless-security",)) {
                Ok(secrets) => get(&secrets, "802-11-wireless-security", "psk").and_then(|v| String::try_from(v.clone()).ok()).filter(|p| !p.is_empty()),
                Err(e) => {
                    tracing::info!(network = name, "network: its password couldn't be read: {e}");
                    None
                }
            }
        } else {
            None
        };
        out.push(SavedWifi { id: if id.is_empty() { name } else { id }, ssid, hidden: flag("802-11-wireless", "hidden", false), autoconnect: flag("connection", "autoconnect", true), key_mgmt, psk });
    }
    Ok((out, left))
}

/// Whether a Wi-Fi network named `ssid` is saved here.
pub fn has_wifi(ssid: &[u8]) -> Result<bool> {
    Ok(saved()?.iter().any(|(s, _)| s.as_bytes() == ssid))
}

/// A new connection's UUID (version 4).
fn new_uuid() -> Result<String> {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).map_err(|e| zbus::Error::Failure(e.to_string()))?;
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    Ok(format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..]))
}

/// Save `w` here, as a network to join (not joined now).
pub fn add_wifi(w: &SavedWifi) -> Result<()> {
    let uuid = new_uuid()?;
    let mut settings: HashMap<&str, HashMap<&str, Value>> = HashMap::new();
    settings
        .insert("connection", HashMap::from([("id", Value::from(w.id.as_str())), ("type", Value::from("802-11-wireless")), ("uuid", Value::from(uuid)), ("autoconnect", Value::from(w.autoconnect))]));
    settings.insert("802-11-wireless", HashMap::from([("ssid", Value::from(w.ssid.clone())), ("mode", Value::from("infrastructure")), ("hidden", Value::from(w.hidden))]));
    if !w.key_mgmt.is_empty() {
        let mut security = HashMap::from([("key-mgmt", Value::from(w.key_mgmt.as_str()))]);
        if let Some(psk) = &w.psk {
            security.insert("psk", Value::from(psk.as_str()));
        }
        settings.insert("802-11-wireless-security", security);
    }
    let _: OwnedObjectPath = bus::call(conn()?, DEST, SETTINGS_PATH, SETTINGS, "AddConnection", &(settings,))?;
    Ok(())
}

/// Delete every saved connection for `ssid`.
pub fn forget(ssid: &str) -> Result<()> {
    for (_, path) in saved()?.into_iter().filter(|(s, _)| s == ssid) {
        bus::call::<_, ()>(conn()?, DEST, path.as_str(), CONNECTION, "Delete", &())?;
    }
    Ok(())
}

// ---- VPNs ----------------------------------------------------------------------

/// A VPN saved: WireGuard, or one of NetworkManager's VPN plugins'
/// (OpenVPN, IPsec…).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vpn {
    pub profile: String,
    pub name: String,
    /// "WireGuard", "OpenVPN", "IPsec"…
    pub kind: String,
    /// Its active connection, while it's up or coming up.
    pub active: Option<String>,
    /// NM_ACTIVE_CONNECTION_STATE_*: 1 connecting, 2 connected, 3
    /// disconnecting (0 while it's down).
    pub state: u32,
}

impl Vpn {
    pub fn state_text(&self) -> &'static str {
        match (self.active.is_some(), self.state) {
            (false, _) => "Not connected",
            (true, 1) => "Connecting…",
            (true, 2) => "Connected",
            (true, 3) => "Disconnecting…",
            _ => "Not connected",
        }
    }
}

/// A VPN plugin's name as people know it.
fn vpn_kind(service: &str) -> String {
    let last = service.rsplit('.').next().unwrap_or(service);
    match last {
        "openvpn" => "OpenVPN".into(),
        "vpnc" => "Cisco VPN".into(),
        "openconnect" => "OpenConnect".into(),
        "l2tp" => "L2TP".into(),
        "pptp" => "PPTP".into(),
        "strongswan" | "libreswan" | "openswan" => "IPsec".into(),
        "fortisslvpn" => "Fortinet SSL VPN".into(),
        "sstp" => "SSTP".into(),
        other => other.to_owned(),
    }
}

/// Every VPN saved, and whether each is up.
pub fn vpns() -> Result<Vec<Vpn>> {
    let c = conn()?;
    let actives: Vec<OwnedObjectPath> = bus::property(c, DEST, NM_PATH, NM, "ActiveConnections").unwrap_or_default();
    let up: Vec<(String, String, u32)> = actives
        .iter()
        .filter_map(|a| {
            let profile: OwnedObjectPath = bus::property(c, DEST, a.as_str(), ACTIVE, "Connection").ok()?;
            let state: u32 = bus::property(c, DEST, a.as_str(), ACTIVE, "State").unwrap_or(0);
            Some((profile.as_str().to_owned(), a.as_str().to_owned(), state))
        })
        .collect();
    let paths: Vec<OwnedObjectPath> = bus::call(c, DEST, SETTINGS_PATH, SETTINGS, "ListConnections", &())?;
    let mut out = Vec::new();
    for p in paths {
        let Ok(s) = profile_settings(p.as_str()) else { continue };
        let text = |g: &str, k: &str| get(&s, g, k).and_then(|v| String::try_from(v.clone()).ok()).unwrap_or_default();
        let kind = match text("connection", "type").as_str() {
            "wireguard" => "WireGuard".to_owned(),
            "vpn" => vpn_kind(&text("vpn", "service-type")),
            _ => continue,
        };
        let active = up.iter().find(|(prof, ..)| prof == p.as_str());
        out.push(Vpn { profile: p.as_str().to_owned(), name: text("connection", "id"), kind, active: active.map(|a| a.1.clone()), state: active.map_or(0, |a| a.2) });
    }
    out.sort_by_key(|v| v.name.to_lowercase());
    Ok(out)
}

/// The connections up now (their active connections' paths).
pub fn active_connections() -> Result<Vec<String>> {
    let actives: Vec<OwnedObjectPath> = bus::property(conn()?, DEST, NM_PATH, NM, "ActiveConnections")?;
    Ok(actives.into_iter().map(|a| a.as_str().to_owned()).collect())
}

/// Bring a VPN up (NetworkManager chooses the link it goes over).
pub fn vpn_up(profile: &str) -> Result<()> {
    let root = zvariant::ObjectPath::try_from("/")?;
    let p = zvariant::ObjectPath::try_from(profile)?;
    let _: OwnedObjectPath = bus::call(conn()?, DEST, NM_PATH, NM, "ActivateConnection", &(p, root.clone(), root))?;
    Ok(())
}

/// Take an active connection down (a VPN, a hotspot).
pub fn deactivate(active: &str) -> Result<()> {
    let a = zvariant::ObjectPath::try_from(active)?;
    bus::call::<_, ()>(conn()?, DEST, NM_PATH, NM, "DeactivateConnection", &(a,))
}

/// Delete a saved connection.
pub fn delete_profile(profile: &str) -> Result<()> {
    bus::call::<_, ()>(conn()?, DEST, profile, CONNECTION, "Delete", &())
}

/// A WireGuard key: base64 of 32 bytes.
fn wireguard_key(k: &str) -> bool {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.decode(k.trim()).is_ok_and(|b| b.len() == 32)
}

/// An interface name for a tunnel named `name`: its letters, digits, `-`
/// and `_`, at most 15 ("wg0" if none).
fn tunnel_name(name: &str) -> String {
    let n: String = name.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').take(15).collect();
    if n.is_empty() { "wg0".into() } else { n }
}

/// A WireGuard configuration as `wg-quick` reads it (`[Interface]`:
/// PrivateKey, Address, DNS, ListenPort, MTU; each `[Peer]`: PublicKey,
/// PresharedKey, AllowedIPs, Endpoint, PersistentKeepalive) as
/// NetworkManager's settings for a connection named `name`. Its DNS
/// servers are asked first and alone while it's up (a full tunnel's
/// names don't leak), its addresses' families only are on.
pub fn wireguard_settings(name: &str, text: &str) -> std::result::Result<HashMap<String, HashMap<String, Value<'static>>>, String> {
    let mut section = String::new();
    let mut iface: HashMap<String, String> = HashMap::new();
    let mut peers: Vec<HashMap<String, String>> = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.split('#').next().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }
        if let Some(s) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = s.trim().to_lowercase();
            if section == "peer" {
                peers.push(HashMap::new());
            }
            continue;
        }
        let (k, v) = line.split_once('=').ok_or_else(|| format!("line {}: “{line}” isn't a setting", n + 1))?;
        let (k, v) = (k.trim().to_lowercase(), v.trim().to_owned());
        let into = match section.as_str() {
            "interface" => &mut iface,
            "peer" => peers.last_mut().expect("a peer"),
            _ => return Err(format!("line {}: a setting outside [Interface] or [Peer]", n + 1)),
        };
        // (Repeated lines add to a list, as wg-quick reads them.)
        into.entry(k).and_modify(|e| *e = format!("{e}, {v}")).or_insert(v);
    }
    let list = |s: Option<&String>| s.map(|s| s.split(',').map(str::trim).filter(|x| !x.is_empty()).map(str::to_owned).collect::<Vec<_>>()).unwrap_or_default();
    let key = iface.get("privatekey").ok_or("there's no PrivateKey in [Interface]")?;
    if !wireguard_key(key) {
        return Err("the PrivateKey isn't a WireGuard key".into());
    }
    if peers.is_empty() {
        return Err("there's no [Peer]".into());
    }
    let number = |s: Option<&String>, what: &str| -> std::result::Result<Option<u32>, String> { s.map(|v| v.parse::<u32>().map_err(|_| format!("the {what} isn't a number"))).transpose() };
    let mut wg: HashMap<String, Value<'static>> = HashMap::from([("private-key".into(), Value::from(key.trim().to_owned())), ("private-key-flags".into(), Value::from(0u32))]);
    if let Some(port) = number(iface.get("listenport"), "ListenPort")? {
        wg.insert("listen-port".into(), Value::from(port));
    }
    if let Some(mtu) = number(iface.get("mtu"), "MTU")? {
        wg.insert("mtu".into(), Value::from(mtu));
    }
    let mut out_peers: Vec<HashMap<String, Value<'static>>> = Vec::new();
    for p in &peers {
        let public = p.get("publickey").ok_or("a [Peer] has no PublicKey")?;
        if !wireguard_key(public) {
            return Err("a peer's PublicKey isn't a WireGuard key".into());
        }
        let mut peer: HashMap<String, Value<'static>> = HashMap::from([("public-key".into(), Value::from(public.trim().to_owned()))]);
        let allowed = list(p.get("allowedips"));
        if let Some(bad) = allowed.iter().find(|a| a.split('/').next().is_none_or(|ip| ip.parse::<std::net::IpAddr>().is_err())) {
            return Err(format!("“{bad}” in AllowedIPs isn't an address"));
        }
        peer.insert("allowed-ips".into(), Value::from(allowed));
        if let Some(e) = p.get("endpoint") {
            peer.insert("endpoint".into(), Value::from(e.clone()));
        }
        if let Some(psk) = p.get("presharedkey") {
            if !wireguard_key(psk) {
                return Err("a peer's PresharedKey isn't a WireGuard key".into());
            }
            peer.insert("preshared-key".into(), Value::from(psk.trim().to_owned()));
            peer.insert("preshared-key-flags".into(), Value::from(0u32));
        }
        if let Some(k) = number(p.get("persistentkeepalive"), "PersistentKeepalive")? {
            peer.insert("persistent-keepalive".into(), Value::from(k));
        }
        out_peers.push(peer);
    }
    wg.insert("peers".into(), Value::from(out_peers));
    // Addresses, and DNS: servers (IPv4 and IPv6) or search domains.
    let (mut v4, mut v6) = (Vec::new(), Vec::new());
    for a in list(iface.get("address")) {
        let (ip, prefix) = a.split_once('/').map(|(i, p)| (i.to_owned(), p.parse::<u32>().ok())).unwrap_or((a.clone(), None));
        match ip.parse::<std::net::IpAddr>() {
            Ok(std::net::IpAddr::V4(_)) => v4.push(HashMap::from([("address".to_owned(), Value::from(ip)), ("prefix".to_owned(), Value::from(prefix.unwrap_or(32)))])),
            Ok(std::net::IpAddr::V6(_)) => v6.push(HashMap::from([("address".to_owned(), Value::from(ip)), ("prefix".to_owned(), Value::from(prefix.unwrap_or(128)))])),
            Err(_) => return Err(format!("“{a}” in Address isn't an address")),
        }
    }
    let (mut dns4, mut dns6, mut searches): (Vec<u32>, Vec<Vec<u8>>, Vec<String>) = (Vec::new(), Vec::new(), Vec::new());
    for d in list(iface.get("dns")) {
        match d.parse::<std::net::IpAddr>() {
            Ok(std::net::IpAddr::V4(a)) => dns4.push(u32::from_ne_bytes(a.octets())),
            Ok(std::net::IpAddr::V6(a)) => dns6.push(a.octets().to_vec()),
            Err(_) => searches.push(d),
        }
    }
    let has_dns = !dns4.is_empty() || !dns6.is_empty();
    let family = |addresses: Vec<HashMap<String, Value<'static>>>, v6: bool| -> HashMap<String, Value<'static>> {
        if addresses.is_empty() {
            return HashMap::from([("method".into(), Value::from("disabled"))]);
        }
        let mut f: HashMap<String, Value<'static>> = HashMap::from([("method".into(), Value::from("manual")), ("address-data".into(), Value::from(addresses))]);
        if v6 && !dns6.is_empty() {
            f.insert("dns".into(), Value::from(dns6.clone()));
        }
        if !v6 && !dns4.is_empty() {
            f.insert("dns".into(), Value::from(dns4.clone()));
        }
        if !searches.is_empty() {
            f.insert("dns-search".into(), Value::from(searches.clone()));
        }
        if has_dns {
            // Asked first and alone while it's up.
            f.insert("dns-priority".into(), Value::from(-50i32));
        }
        f
    };
    let id = name.trim();
    Ok(HashMap::from([
        (
            "connection".to_owned(),
            HashMap::from([
                ("id".to_owned(), Value::from(if id.is_empty() { "WireGuard".to_owned() } else { id.to_owned() })),
                ("type".to_owned(), Value::from("wireguard")),
                ("interface-name".to_owned(), Value::from(tunnel_name(id))),
                ("autoconnect".to_owned(), Value::from(false)),
            ]),
        ),
        ("wireguard".to_owned(), wg),
        ("ipv4".to_owned(), family(v4, false)),
        ("ipv6".to_owned(), family(v6, true)),
    ]))
}

// ---- A hotspot -------------------------------------------------------------------

/// The name Olo's hotspot connection is saved under.
const HOTSPOT: &str = "Olo Hotspot";

/// A hotspot this computer is making.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hotspot {
    pub device: String,
    pub ssid: String,
    /// Its active connection.
    pub active: String,
}

/// The hotspot on, if one is: a Wi-Fi connection up in access point mode.
pub fn hotspot() -> Result<Option<Hotspot>> {
    let c = conn()?;
    let actives: Vec<OwnedObjectPath> = bus::property(c, DEST, NM_PATH, NM, "ActiveConnections").unwrap_or_default();
    for a in actives {
        let Ok(profile) = bus::property::<OwnedObjectPath>(c, DEST, a.as_str(), ACTIVE, "Connection") else { continue };
        let Ok(s) = profile_settings(profile.as_str()) else { continue };
        if get(&s, "802-11-wireless", "mode").and_then(|v| String::try_from(v.clone()).ok()).as_deref() != Some("ap") {
            continue;
        }
        let ssid = get(&s, "802-11-wireless", "ssid").and_then(|v| Vec::<u8>::try_from(v.clone()).ok()).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
        let devices: Vec<OwnedObjectPath> = bus::property(c, DEST, a.as_str(), ACTIVE, "Devices").unwrap_or_default();
        return Ok(Some(Hotspot { device: devices.first().map(|d| d.as_str().to_owned()).unwrap_or_default(), ssid, active: a.as_str().to_owned() }));
    }
    Ok(None)
}

/// Make `device` a hotspot: a WPA2 network named `ssid` with `password`
/// (8 to 63 characters), sharing this computer's connection (the
/// adapter leaves the network it was on). Kept as Olo's hotspot
/// connection, so its name and password are there next time.
pub fn start_hotspot(device: &str, ssid: &str, password: &str) -> Result<()> {
    if ssid.is_empty() || ssid.len() > 32 {
        return Err(zbus::Error::Failure("a network's name is 1 to 32 bytes".into()));
    }
    if !(8..=63).contains(&password.len()) {
        return Err(zbus::Error::Failure("a WPA2 password is 8 to 63 characters".into()));
    }
    let c = conn()?;
    let mut s: HashMap<&str, HashMap<&str, Value>> = HashMap::new();
    s.insert("connection", HashMap::from([("id", Value::from(HOTSPOT)), ("type", Value::from("802-11-wireless")), ("autoconnect", Value::from(false)), ("uuid", Value::from(new_uuid()?))]));
    s.insert("802-11-wireless", HashMap::from([("ssid", Value::from(ssid.as_bytes().to_vec())), ("mode", Value::from("ap"))]));
    s.insert(
        "802-11-wireless-security",
        HashMap::from([
            ("key-mgmt", Value::from("wpa-psk")),
            ("psk", Value::from(password)),
            ("proto", Value::from(vec!["rsn"])),
            ("pairwise", Value::from(vec!["ccmp"])),
            ("group", Value::from(vec!["ccmp"])),
        ]),
    );
    s.insert("ipv4", HashMap::from([("method", Value::from("shared"))]));
    s.insert("ipv6", HashMap::from([("method", Value::from("ignore"))]));
    // The one saved before goes: this one takes its place.
    let paths: Vec<OwnedObjectPath> = bus::call(c, DEST, SETTINGS_PATH, SETTINGS, "ListConnections", &())?;
    for p in paths {
        if profile_settings(p.as_str()).ok().and_then(|x| get(&x, "connection", "id").and_then(|v| String::try_from(v.clone()).ok())).as_deref() == Some(HOTSPOT) {
            let _ = delete_profile(p.as_str());
        }
    }
    let dev = zvariant::ObjectPath::try_from(device)?;
    let root = zvariant::ObjectPath::try_from("/")?;
    let _: (OwnedObjectPath, OwnedObjectPath) = bus::call(c, DEST, NM_PATH, NM, "AddAndActivateConnection", &(s, dev, root))?;
    Ok(())
}

/// The name and password Olo's hotspot had last (the password asked
/// from NetworkManager, which may ask polkit).
pub fn saved_hotspot() -> Result<Option<(String, String)>> {
    let c = conn()?;
    let paths: Vec<OwnedObjectPath> = bus::call(c, DEST, SETTINGS_PATH, SETTINGS, "ListConnections", &())?;
    for p in paths {
        let Ok(s) = profile_settings(p.as_str()) else { continue };
        if get(&s, "connection", "id").and_then(|v| String::try_from(v.clone()).ok()).as_deref() != Some(HOTSPOT) {
            continue;
        }
        let ssid = get(&s, "802-11-wireless", "ssid").and_then(|v| Vec::<u8>::try_from(v.clone()).ok()).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
        let secrets: Settings = bus::call(c, DEST, p.as_str(), CONNECTION, "GetSecrets", &("802-11-wireless-security",)).unwrap_or_default();
        let psk = get(&secrets, "802-11-wireless-security", "psk").and_then(|v| String::try_from(v.clone()).ok()).unwrap_or_default();
        return Ok(Some((ssid, psk)));
    }
    Ok(None)
}

/// Save a WireGuard configuration (a `.conf` file's text) as a VPN named
/// `name` (not brought up).
pub fn import_wireguard(name: &str, text: &str) -> Result<()> {
    let mut s = wireguard_settings(name, text).map_err(zbus::Error::Failure)?;
    s.get_mut("connection").expect("connection").insert("uuid".into(), Value::from(new_uuid()?));
    let _: OwnedObjectPath = bus::call(conn()?, DEST, SETTINGS_PATH, SETTINGS, "AddConnection", &(s,))?;
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

#[cfg(test)]
mod links_tests {
    use super::*;

    fn changed(path: &str, iface: &str, name: &str, v: Value<'_>, invalidated: &[&str]) -> zbus::Message {
        let props: HashMap<&str, Value> = HashMap::from([(name, v)]);
        zbus::Message::signal(path, "org.freedesktop.DBus.Properties", "PropertiesChanged").unwrap().build(&(iface, props, invalidated.to_vec())).unwrap()
    }

    fn joined() -> Links {
        let wlan = Device {
            path: "/nm/Devices/2".into(),
            interface: "wlan0".into(),
            kind: Kind::Wifi,
            state: 100,
            reason: 0,
            hw_address: String::new(),
            connection: Some("Home".into()),
            addresses: vec!["10.0.0.4/24".into()],
            gateway: None,
            dns: Vec::new(),
            speed: Some(866),
        };
        Links { wifi: Some(Wifi { ssid: Some("Home".into()), strength: 80 }), wifi_device: Some(wlan), wired: None, primary: Some(Kind::Wifi), portal: None }
    }

    /// A strength or a rate is taken from the signal (every few seconds,
    /// and nothing read again for it); anything else is read again.
    #[test]
    fn strength_and_rate_from_the_signal() {
        let last = joined();
        let now = quick(&last, &[changed("/nm/AccessPoint/7", AP, "Strength", Value::U8(31), &[])]).expect("a strength alone");
        assert_eq!(now.wifi.as_ref().map(|w| w.strength), Some(31));
        let now = quick(&now, &[changed("/nm/Devices/2", WIRELESS, "Bitrate", Value::U32(144_000), &[])]).expect("a rate alone");
        assert_eq!(now.wifi_device.as_ref().and_then(|d| d.speed), Some(144));
        // Another adapter's rate, another property, a value only named,
        // a state change: read again.
        assert!(quick(&last, &[changed("/nm/Devices/9", WIRELESS, "Bitrate", Value::U32(1), &[])]).is_none());
        assert!(quick(&last, &[changed("/nm/Devices/2", WIRELESS, "ActiveAccessPoint", Value::from("/"), &[])]).is_none());
        assert!(quick(&last, &[changed("/nm/AccessPoint/7", AP, "Strength", Value::U8(5), &["Ssid"])]).is_none());
        let state = zbus::Message::signal("/nm/Devices/2", DEVICE, "StateChanged").unwrap().build(&(30u32, 100u32, 39u32)).unwrap();
        assert!(quick(&last, &[changed("/nm/AccessPoint/7", AP, "Strength", Value::U8(5), &[]), state]).is_none());
        // Not joined: no strength to change.
        let off = Links { wifi: Some(Wifi { ssid: None, strength: 0 }), ..joined() };
        assert!(quick(&off, &[changed("/nm/AccessPoint/7", AP, "Strength", Value::U8(5), &[])]).is_none());
    }

    #[test]
    fn rates_and_counters() {
        assert_eq!(speed_text(1000), "1 Gb/s");
        assert_eq!(speed_text(2500), "2.5 Gb/s");
        assert_eq!(speed_text(866), "866 Mb/s");
        assert!(traffic("lo").is_some());
        assert!(traffic("no-such-link").is_none());
    }
}

#[cfg(test)]
mod vpn_tests {
    use super::*;

    const CONF: &str = "[Interface]
PrivateKey = yAnz5TF+lXXJte14tji3zlMNq+hd2rYUIgJBgB3fBmk=
Address = 10.200.0.2/32, fd00:200::2/128
DNS = 10.200.0.1, corp.example
MTU = 1420

[Peer]
PublicKey = xTIBA5rboUvnH4htodjb6e697QjLERt1NAB4mZqp8Dg=
PresharedKey = /UwcSPg38hW/D9Y3tcS1FOV0K1wuURMbS0sesJEP5ak=
AllowedIPs = 0.0.0.0/0, ::/0
Endpoint = vpn.example.com:51820
PersistentKeepalive = 25
";

    #[test]
    fn a_wg_quick_file_as_networkmanagers() {
        let s = wireguard_settings("Office (wg)", CONF).unwrap();
        assert_eq!(s["connection"]["type"], Value::from("wireguard"));
        assert_eq!(s["connection"]["interface-name"], Value::from("Officewg"));
        assert_eq!(s["wireguard"]["mtu"], Value::from(1420u32));
        let Value::Array(peers) = &s["wireguard"]["peers"] else { panic!("peers") };
        assert_eq!(peers.len(), 1);
        assert_eq!(s["ipv4"]["method"], Value::from("manual"));
        assert_eq!(s["ipv4"]["dns"], Value::from(vec![u32::from_ne_bytes([10, 200, 0, 1])]));
        assert_eq!(s["ipv4"]["dns-search"], Value::from(vec!["corp.example".to_owned()]));
        assert_eq!(s["ipv4"]["dns-priority"], Value::from(-50i32));
        assert_eq!(s["ipv6"]["method"], Value::from("manual"));
        // IPv4 alone: IPv6 off.
        let v4 = CONF.replace(", fd00:200::2/128", "");
        assert_eq!(wireguard_settings("x", &v4).unwrap()["ipv6"]["method"], Value::from("disabled"));
    }

    #[test]
    fn what_isnt_a_wireguard_file_is_said() {
        assert!(wireguard_settings("x", "").unwrap_err().contains("PrivateKey"));
        assert!(wireguard_settings("x", &CONF.replace("yAnz5TF", "nope")).unwrap_err().contains("PrivateKey"));
        assert!(wireguard_settings("x", &CONF.replace("[Peer]", "[Pear]")).is_err());
        assert!(wireguard_settings("x", &CONF.replace("0.0.0.0/0", "everything")).unwrap_err().contains("AllowedIPs"));
        assert_eq!(tunnel_name("a very long name for a tunnel"), "averylongnamefo");
        assert_eq!(vpn_kind("org.freedesktop.NetworkManager.openvpn"), "OpenVPN");
    }
}
