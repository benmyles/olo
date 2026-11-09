//! olo-wifi-heal: a root service that watches Wi-Fi and resets a chip
//! whose firmware has stopped joining, then joins the network again.
//!
//! On the Apple machines' Broadcom chips (brcmfmac, BCM4388 here) the
//! firmware can get into a state where every join fails without ever
//! associating (wpa_supplicant's "ASSOC-REJECT status_code=16", which
//! brcmfmac reports for any failed join, not something the access point
//! said) or a scan never finishes ("Reject scan trigger since one is
//! already pending"), and stays so until the driver starts it again: 38
//! hours on 2026-10-31, while NetworkManager told the person their
//! password was wrong. Resetting the device (the driver unbound and bound
//! again, which reloads the firmware) joins at once.
//!
//! Event-driven, nothing polled: NetworkManager's device states and
//! wpa_supplicant's interface states come as D-Bus signals; the only
//! timer is a stuck scan's, armed while a scan runs. A join that failed
//! without the supplicant ever getting past associating, after at least
//! two tries, may be the firmware's; but on this chip, which does the
//! handshake itself, a wrong password fails just the same (status 16,
//! never associated: measured 2026-11-01). So the reset is the test:
//! after it the same network is joined again, and if that fails the same
//! way the chip wasn't at fault (the password, most likely), which is
//! recorded, and that network isn't reset for again until it has joined.
//! Resets are rate limited (one in three minutes, four an hour) and only
//! for drivers known to recover this way. What it did goes to the journal and to
//! `/run/olo/wifi-heal.json`, which Olo's session tells the person of.
//!
//! Installed as a root-owned copy (`/usr/local/libexec/olo-wifi-heal`,
//! `olo-wifi-heal.service`) by Olo Doctor, never run from the person's
//! home: a program they can write to must not run as root.
//! `olo-wifi-heal --status` prints the report; `OLO_WIFI_HEAL_DEBUG=1`
//! logs every event it hears. Its source's fingerprint is in the file
//! (`SOURCE`), for Olo Doctor.

use std::{
    collections::HashMap,
    path::Path,
    sync::mpsc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

mod bluetooth;

use serde::{Deserialize, Serialize};
use zbus::blocking::Connection;
use zvariant::{OwnedObjectPath, OwnedValue};

const NM: &str = "org.freedesktop.NetworkManager";
const NM_PATH: &str = "/org/freedesktop/NetworkManager";
const NM_DEVICE: &str = "org.freedesktop.NetworkManager.Device";
const NM_ACTIVE: &str = "org.freedesktop.NetworkManager.Connection.Active";
const WPA: &str = "fi.w1.wpa_supplicant1";
const WPA_IFACE: &str = "fi.w1.wpa_supplicant1.Interface";
const PROPS: &str = "org.freedesktop.DBus.Properties";

/// Drivers whose firmware a reset brings back.
const DRIVERS: &[&str] = &["brcmfmac"];
/// A scan running this long has stuck.
const STUCK_SCAN: Duration = Duration::from_secs(120);
const MIN_GAP: Duration = Duration::from_secs(180);
const PER_HOUR: usize = 4;
const REPORT: &str = "/run/olo/wifi-heal.json";

// NM_DEVICE_STATE_*.
const PREPARE: u32 = 40;
const NEED_AUTH: u32 = 60;
const DISCONNECTED: u32 = 30;
const ACTIVATED: u32 = 100;
const FAILED: u32 = 120;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Heal {
    /// Seconds since the epoch.
    pub at: u64,
    pub interface: String,
    pub driver: String,
    /// Why it was reset.
    pub why: String,
    /// The network it joined again after the reset, once it has.
    pub rejoined: Option<String>,
    /// The reset failed, or was held back (rate limit, driver): why.
    pub error: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Report {
    pub heals: Vec<Heal>,
}

enum Ev {
    /// NetworkManager's device `path` went from `old` to `new`.
    Nm { path: String, new: u32, old: u32, reason: u32 },
    /// A supplicant interface's properties changed.
    Wpa { path: String, state: Option<String>, scanning: Option<bool> },
    /// A scan's deadline (by the scan's number, to ignore a finished one).
    ScanDeadline { interface: String, scan: u64 },
    /// A Bluetooth controller stopped answering (`bluetooth`), and why.
    Bluetooth { hci: String, why: String },
    /// A while after controller `hci` was reset (its heal's entry): back?
    BluetoothBack { hci: String, entry: usize },
}

/// What is followed of each Wi-Fi interface.
#[derive(Default)]
struct Iface {
    driver: String,
    /// The join under way: tries to associate, and whether one got
    /// further.
    joining: bool,
    associating: u32,
    associated: bool,
    /// The connection being joined (its settings' path) and network.
    connection: Option<OwnedObjectPath>,
    ssid: Option<String>,
    scan: u64,
    scanning: bool,
    /// Reset: rejoin this once the device is back.
    rejoin: Option<(OwnedObjectPath, String, usize)>,
}

struct Healer {
    conn: Connection,
    tx: mpsc::Sender<Ev>,
    ifaces: HashMap<String, Iface>,
    /// Supplicant interface path → interface name.
    names: HashMap<String, String>,
    resets: Vec<Instant>,
    /// Bluetooth controllers' resets, limited as Wi-Fi's are, apart.
    bt_resets: Vec<Instant>,
    report: Report,
    /// Connections a reset didn't help (their joins fail on their own:
    /// the password), until they join.
    not_the_chip: Vec<OwnedObjectPath>,
    /// The connection joined again after a reset, and its heal's entry:
    /// if this join fails the same way, the reset didn't help.
    retrying: Option<(OwnedObjectPath, usize, String)>,
}

fn log(msg: impl AsRef<str>) {
    eprintln!("olo-wifi-heal: {}", msg.as_ref());
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn prop<T: TryFrom<OwnedValue>>(conn: &Connection, dest: &str, path: &str, iface: &str, name: &str) -> Option<T> {
    let reply = conn.call_method(Some(dest), path, Some(PROPS), "Get", &(iface, name)).ok()?;
    let v: OwnedValue = reply.body().deserialize().ok()?;
    T::try_from(v).ok()
}

/// The driver behind network interface `name` ("brcmfmac").
fn driver_of(name: &str) -> String {
    std::fs::read_link(format!("/sys/class/net/{name}/device/driver")).ok().and_then(|p| p.file_name().map(|f| f.to_string_lossy().into_owned())).unwrap_or_default()
}

/// Unbind and bind interface `name`'s device from its driver (the
/// firmware loads again).
fn reset_device(name: &str) -> Result<(), String> {
    let device = std::fs::canonicalize(format!("/sys/class/net/{name}/device")).map_err(|e| format!("its device: {e}"))?;
    let driver = std::fs::canonicalize(device.join("driver")).map_err(|e| format!("its driver: {e}"))?;
    let id = device.file_name().ok_or("no device name")?.to_string_lossy().into_owned();
    std::fs::write(driver.join("unbind"), &id).map_err(|e| format!("unbinding {id}: {e}"))?;
    std::thread::sleep(Duration::from_secs(2));
    std::fs::write(driver.join("bind"), &id).map_err(|e| format!("binding {id}: {e}"))
}

fn write_report(report: &Report) {
    let path = Path::new(REPORT);
    let _ = std::fs::create_dir_all(path.parent().unwrap_or(Path::new("/run")));
    let tmp = path.with_extension("new");
    if std::fs::write(&tmp, serde_json::to_string_pretty(report).unwrap_or_default()).is_ok() {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o644));
        let _ = std::fs::rename(&tmp, path);
    }
}

impl Healer {
    /// Controller `hci` stopped answering: reset it (within the limits),
    /// say so, and look a while after whether it's back.
    fn heal_bluetooth(&mut self, hci: String, why: String) {
        let mut heal = Heal { at: now(), interface: hci.clone(), driver: bluetooth::driver_name(&hci), why: why.clone(), rejoined: None, error: None };
        self.bt_resets.retain(|t| t.elapsed() < Duration::from_secs(3600));
        if self.bt_resets.last().is_some_and(|t| t.elapsed() < MIN_GAP) {
            heal.error = Some("not reset: one was moments ago".into());
        } else if self.bt_resets.len() >= PER_HOUR {
            heal.error = Some(format!("not reset: {PER_HOUR} resets in the last hour already"));
        } else {
            log(format!("Bluetooth: {hci} ({}) doesn't answer ({why}): resetting it", heal.driver));
            self.bt_resets.push(Instant::now());
            if let Err(e) = bluetooth::reset(&hci) {
                heal.error = Some(format!("the reset failed: {e}"));
            }
        }
        if let Some(e) = &heal.error {
            log(format!("Bluetooth: {hci}: {e}"));
        }
        let held = heal.error.is_some();
        self.report.heals.push(heal);
        let entry = self.report.heals.len() - 1;
        write_report(&self.report);
        if !held {
            let tx = self.tx.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(15));
                let _ = tx.send(Ev::BluetoothBack { hci, entry });
            });
        }
    }

    /// The interface a NetworkManager device or supplicant object is.
    /// A device's is asked each time (rare: its states): a reset device
    /// comes back as wlan0 and is renamed.
    fn name_of(&mut self, path: &str, nm: bool) -> Option<String> {
        if nm {
            // Wi-Fi only (NM_DEVICE_TYPE_WIFI).
            let kind: u32 = prop(&self.conn, NM, path, NM_DEVICE, "DeviceType")?;
            return if kind == 2 { prop(&self.conn, NM, path, NM_DEVICE, "Interface") } else { None };
        }
        if let Some(n) = self.names.get(path) {
            return Some(n.clone());
        }
        let name: String = prop(&self.conn, WPA, path, WPA_IFACE, "Ifname")?;
        self.names.insert(path.to_owned(), name.clone());
        Some(name)
    }

    fn iface(&mut self, name: &str) -> &mut Iface {
        self.ifaces.entry(name.to_owned()).or_insert_with(|| Iface { driver: driver_of(name), ..Iface::default() })
    }

    fn handle(&mut self, ev: Ev) {
        if std::env::var_os("OLO_WIFI_HEAL_DEBUG").is_some() {
            match &ev {
                Ev::Nm { path, new, old, reason } => log(format!("(nm {path}: {old} -> {new}, reason {reason})")),
                Ev::Wpa { path, state, scanning } => log(format!("(wpa {path}: state {state:?} scanning {scanning:?})")),
                Ev::ScanDeadline { interface, scan } => log(format!("(scan deadline {interface} #{scan})")),
                Ev::Bluetooth { hci, why } => log(format!("(bluetooth {hci}: {why})")),
                Ev::BluetoothBack { hci, entry } => log(format!("(bluetooth {hci} back? #{entry})")),
            }
        }
        match ev {
            Ev::Nm { path, new, old, reason } => {
                let Some(name) = self.name_of(&path, true) else { return };
                // A join starts (not its own retry after asking for secrets).
                if new == PREPARE && old != NEED_AUTH {
                    let (connection, ssid) = self.joining(&path);
                    let i = self.iface(&name);
                    *i = Iface { driver: std::mem::take(&mut i.driver), scan: i.scan, scanning: i.scanning, rejoin: i.rejoin.take(), joining: true, connection, ssid, ..Iface::default() };
                } else if new == ACTIVATED {
                    let i = self.iface(&name);
                    i.joining = false;
                    i.rejoin = None;
                    let joined = i.connection.clone();
                    if let Some(c) = &joined {
                        self.not_the_chip.retain(|x| x != c);
                    }
                    // Joined again after a reset: healed.
                    if let Some((c, at, ssid)) = self.retrying.take()
                        && Some(&c) == joined.as_ref()
                        && let Some(h) = self.report.heals.get_mut(at)
                    {
                        log(format!("{name}: joined {ssid} after the reset"));
                        h.rejoined = Some(ssid);
                        write_report(&self.report);
                    }
                } else if new == FAILED && old != FAILED {
                    let i = self.iface(&name);
                    let (tries, further, ssid, connection) = (i.associating, i.associated, i.ssid.clone(), i.connection.clone());
                    i.joining = false;
                    let network = ssid.as_deref().unwrap_or("a network").to_owned();
                    log(format!("{name}: joining {network} failed (reason {reason}): {tries} tries to associate, {}", if further { "one got further" } else { "none got further" }));
                    if tries >= 2 && !further {
                        // Joined again after a reset and failed the same:
                        // not the chip.
                        if let Some((c, at, _)) = self.retrying.take()
                            && Some(&c) == connection.as_ref()
                        {
                            log(format!("{name}: {network} failed the same after the reset: not the chip (the password, most likely)"));
                            if let Some(h) = self.report.heals.get_mut(at) {
                                h.error = Some(format!("{network} still refused the join after the reset: its password is most likely wrong"));
                            }
                            write_report(&self.report);
                            self.not_the_chip.push(c);
                            return;
                        }
                        if connection.as_ref().is_some_and(|c| self.not_the_chip.contains(c)) {
                            log(format!("{name}: {network} failed as before; a reset didn't help it, so not again"));
                            return;
                        }
                        let why = format!("joining {network} failed {tries} times without associating");
                        self.heal(&name, &why);
                    }
                } else if new == DISCONNECTED {
                    self.rejoin(&name, &path);
                }
            }
            Ev::Wpa { path, state, scanning } => {
                let Some(name) = self.name_of(&path, false) else { return };
                let tx = self.tx.clone();
                let i = self.iface(&name);
                if let Some(state) = state {
                    match state.as_str() {
                        "associating" if i.joining => i.associating += 1,
                        "associated" | "4way_handshake" | "group_handshake" | "completed" if i.joining => i.associated = true,
                        _ => {}
                    }
                }
                if let Some(on) = scanning
                    && on != i.scanning
                {
                    i.scanning = on;
                    i.scan += 1;
                    if on {
                        let (interface, scan) = (name.clone(), i.scan);
                        std::thread::spawn(move || {
                            std::thread::sleep(STUCK_SCAN);
                            let _ = tx.send(Ev::ScanDeadline { interface, scan });
                        });
                    }
                }
            }
            Ev::Bluetooth { hci, why } => self.heal_bluetooth(hci, why),
            Ev::BluetoothBack { hci, entry } => {
                let back = bluetooth::answering(&self.conn, &hci);
                if let Some(h) = self.report.heals.get_mut(entry) {
                    if back {
                        h.rejoined = Some("Bluetooth is on again".into());
                    } else if h.error.is_none() {
                        h.error = Some("it still doesn't answer after the reset: restarting the computer brings it back".into());
                    }
                }
                log(format!("Bluetooth: {hci} {}", if back { "answers again" } else { "still doesn't answer after its reset" }));
                write_report(&self.report);
            }
            Ev::ScanDeadline { interface, scan } => {
                let i = self.iface(&interface);
                if i.scanning && i.scan == scan {
                    let why = format!("a scan didn't finish in {} s", STUCK_SCAN.as_secs());
                    self.heal(&interface, &why);
                }
            }
        }
    }

    /// The connection a device is joining, and its network's name.
    fn joining(&self, device: &str) -> (Option<OwnedObjectPath>, Option<String>) {
        let Some(active) = prop::<OwnedObjectPath>(&self.conn, NM, device, NM_DEVICE, "ActiveConnection").filter(|p| p.as_str() != "/") else { return (None, None) };
        let connection: Option<OwnedObjectPath> = prop(&self.conn, NM, active.as_str(), NM_ACTIVE, "Connection");
        let ssid: Option<String> = prop(&self.conn, NM, active.as_str(), NM_ACTIVE, "Id");
        (connection, ssid)
    }

    /// Reset `name`'s chip, if its driver recovers so and not too often.
    fn heal(&mut self, name: &str, why: &str) {
        let (driver, connection, ssid) = {
            let i = self.iface(name);
            (i.driver.clone(), i.connection.clone(), i.ssid.clone())
        };
        let mut heal = Heal { at: now(), interface: name.to_owned(), driver: driver.clone(), why: why.to_owned(), rejoined: None, error: None };
        self.resets.retain(|t| t.elapsed() < Duration::from_secs(3600));
        if !DRIVERS.contains(&driver.as_str()) {
            heal.error = Some(format!("not reset: {driver} isn't known to recover by a reset"));
        } else if self.resets.last().is_some_and(|t| t.elapsed() < MIN_GAP) {
            log(format!("{name}: {why}; reset {} s ago, not again yet", self.resets.last().map_or(0, |t| t.elapsed().as_secs())));
            return;
        } else if self.resets.len() >= PER_HOUR {
            heal.error = Some(format!("not reset: {PER_HOUR} resets in the last hour already"));
        } else {
            log(format!("{name}: {why}; resetting its {driver} device"));
            self.resets.push(Instant::now());
            match reset_device(name) {
                Ok(()) => {
                    let at = self.report.heals.len();
                    let i = self.iface(name);
                    i.scanning = false;
                    i.scan += 1;
                    i.rejoin = connection.map(|c| (c, ssid.clone().unwrap_or_default(), at));
                }
                Err(e) => heal.error = Some(e),
            }
        }
        if let Some(e) = &heal.error {
            log(format!("{name}: {why}; {e}"));
        }
        self.report.heals.push(heal);
        let excess = self.report.heals.len().saturating_sub(20);
        self.report.heals.drain(..excess);
        if let Some(i) = self.ifaces.get_mut(name)
            && let Some((_, _, at)) = &mut i.rejoin
        {
            *at = at.saturating_sub(excess);
        }
        write_report(&self.report);
    }

    /// A reset device is back and free: join what it was joining.
    fn rejoin(&mut self, name: &str, device: &str) {
        let Some((connection, ssid, at)) = self.iface(name).rejoin.take() else { return };
        // A moment for the supplicant to take the interface on.
        std::thread::sleep(Duration::from_secs(3));
        let Ok(dev) = zvariant::ObjectPath::try_from(device) else { return };
        let none = zvariant::ObjectPath::from_static_str_unchecked("/");
        let result = self.conn.call_method(Some(NM), NM_PATH, Some(NM), "ActivateConnection", &(connection.as_ref(), dev, none));
        match result {
            Ok(_) => {
                log(format!("{name}: back after the reset; joining {ssid}"));
                self.retrying = Some((connection, at, ssid));
            }
            Err(e) => log(format!("{name}: back after the reset; couldn't join {ssid} again: {e}")),
        }
        write_report(&self.report);
    }
}

/// Hear NetworkManager's and wpa_supplicant's signals, onto `tx`.
fn listen(conn: &Connection, tx: mpsc::Sender<Ev>) -> zbus::Result<()> {
    let rules = [
        format!("type='signal',sender='{NM}',interface='{NM_DEVICE}',member='StateChanged'"),
        format!("type='signal',sender='{WPA}',interface='{PROPS}',member='PropertiesChanged',arg0='{WPA_IFACE}'"),
    ];
    for rule in &rules {
        conn.call_method(Some("org.freedesktop.DBus"), "/org/freedesktop/DBus", Some("org.freedesktop.DBus"), "AddMatch", &(rule.as_str(),))?;
    }
    for msg in zbus::blocking::MessageIterator::from(conn) {
        let Ok(msg) = msg else { continue };
        let h = msg.header();
        if h.message_type() != zbus::message::Type::Signal {
            continue;
        }
        let (Some(path), Some(member)) = (h.path().map(|p| p.to_string()), h.member().map(|m| m.to_string())) else { continue };
        let ev = match member.as_str() {
            "StateChanged" => msg.body().deserialize::<(u32, u32, u32)>().ok().map(|(new, old, reason)| Ev::Nm { path, new, old, reason }),
            "PropertiesChanged" => msg.body().deserialize::<(String, HashMap<String, OwnedValue>, Vec<String>)>().ok().and_then(|(iface, changed, _)| {
                if iface != WPA_IFACE {
                    return None;
                }
                let state = changed.get("State").and_then(|v| String::try_from(v.try_clone().ok()?).ok());
                let scanning = changed.get("Scanning").and_then(|v| bool::try_from(v).ok());
                (state.is_some() || scanning.is_some()).then_some(Ev::Wpa { path, state, scanning })
            }),
            _ => None,
        };
        if let Some(ev) = ev
            && tx.send(ev).is_err()
        {
            break;
        }
    }
    Ok(())
}

/// Its source's fingerprint (build.rs), where Olo Doctor finds it in the
/// file without running it: the installed copy is out of date when its
/// differs from the new build's, not whenever their bytes do.
#[used]
pub static SOURCE: &str = concat!("olo-wifi-heal source ", env!("OLO_HEAL_SOURCE"), ";");

fn main() {
    olo_crash::install();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--status") {
        println!("{}", std::fs::read_to_string(REPORT).unwrap_or_else(|_| "{\"heals\":[]}".into()));
        return;
    }
    let conn = match Connection::system() {
        Ok(c) => c,
        Err(e) => {
            log(format!("no system bus: {e}"));
            std::process::exit(1);
        }
    };
    let report: Report = std::fs::read_to_string(REPORT).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let (tx, rx) = mpsc::channel();
    let listener = conn.clone();
    let ltx = tx.clone();
    std::thread::Builder::new()
        .name("listen".into())
        .spawn(move || {
            if let Err(e) = listen(&listener, ltx) {
                log(format!("listening: {e}"));
            }
            std::process::exit(1);
        })
        .expect("spawn");
    // Bluetooth: the kernel's messages, and BlueZ's adapters as they are.
    bluetooth::watch(tx.clone());
    bluetooth::check_at_start(conn.clone(), tx.clone());
    let mut healer = Healer { conn, tx, ifaces: HashMap::new(), names: HashMap::new(), resets: Vec::new(), bt_resets: Vec::new(), report, not_the_chip: Vec::new(), retrying: None };
    log(format!("watching Wi-Fi (resets {} when a join or scan sticks) and Bluetooth (resets a controller that stops answering)", DRIVERS.join(", ")));
    for ev in rx {
        healer.handle(ev);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heals_read_back() {
        let r = Report { heals: vec![Heal { at: 1, interface: "wld0".into(), driver: "brcmfmac".into(), why: "x".into(), rejoined: Some("Home Wi-Fi".into()), error: None }] };
        let back: Report = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        assert_eq!(back.heals, r.heals);
    }

    #[test]
    fn report_path_is_world_readable_runtime() {
        assert!(Path::new(REPORT).starts_with("/run"));
    }
}
