//! Bluetooth's half of the healer. A controller that has stopped answering
//! is reset: its driver unbound, the device itself reset where it can be
//! (a PCI function reset), and bound again; BlueZ takes the new controller
//! up as it was (Ben, 2026-11-09: AirPods "just hang on connecting…".
//! Intel's PCIe Bluetooth, `btintel_pcie`, had failed every command since
//! the morning: "Bluetooth: hci0: sending frame failed (-19)", ENODEV;
//! turning it off and on through rfkill didn't bring it back, as its
//! driver couldn't even send the reset).
//!
//! What says so, as it happens (nothing polled): the kernel's own messages
//! (`/dev/kmsg`, read as they come), a controller failing to send (ENODEV),
//! a command timing out, a hardware error: three within a minute. And, at
//! start, BlueZ's adapter powered while its power state says off (or the
//! other way round), as a dead controller looks to it, seen twice five
//! seconds apart (not a change under way). The kernel's messages from
//! before start count only if they're from the last ten minutes.

use std::{collections::HashMap, io::Read as _, path::PathBuf, sync::mpsc, time::Duration};

use zbus::blocking::Connection;

use crate::Ev;

/// Failures within this window, this many, and it has stopped answering.
const WINDOW_US: u64 = 60_000_000;
const FAILURES: usize = 3;
/// Messages from before the healer started that still count.
const RECENT_US: u64 = 600_000_000;

const BLUEZ: &str = "org.bluez";
const ADAPTER: &str = "org.bluez.Adapter1";

/// The controller a kernel message says failed, and how, if it says so:
/// "Bluetooth: hci0: sending frame failed (-19)".
pub fn failure(message: &str) -> Option<(String, &'static str)> {
    let rest = message.strip_prefix("Bluetooth: ")?;
    let (hci, what) = rest.split_once(": ")?;
    if !hci.starts_with("hci") {
        return None;
    }
    let how = if what.contains("(-19)") || what.contains("failed: -19") {
        "it failed to send to the controller (no such device)"
    } else if what.contains("tx timeout") {
        "a command to the controller timed out"
    } else if what.starts_with("Hardware error") {
        "the controller reported a hardware error"
    } else if what.contains("HCI reset during shutdown failed") {
        "the controller didn't take a reset"
    } else {
        return None;
    };
    Some((hci.to_owned(), how))
}

/// One kmsg record's time (µs since boot) and message.
fn record(text: &str) -> Option<(u64, &str)> {
    let (head, message) = text.split_once(';')?;
    let at = head.split(',').nth(2)?.parse().ok()?;
    Some((at, message.lines().next().unwrap_or_default()))
}

fn monotonic_us() -> u64 {
    let mut t = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: into our timespec.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut t) };
    t.tv_sec as u64 * 1_000_000 + t.tv_nsec as u64 / 1000
}

/// Follow the kernel's messages for controllers failing (on a thread of
/// its own): `Ev::Bluetooth` for one that failed often enough.
pub fn watch(tx: mpsc::Sender<Ev>) {
    std::thread::Builder::new()
        .name("kmsg".into())
        .spawn(move || {
            let mut kmsg = match std::fs::File::open("/dev/kmsg") {
                Ok(f) => f,
                Err(e) => {
                    crate::log(format!("Bluetooth: the kernel's messages can't be read ({e}): controllers aren't watched"));
                    return;
                }
            };
            let started = monotonic_us();
            let mut seen: HashMap<String, Vec<u64>> = HashMap::new();
            let mut buf = vec![0u8; 8192];
            loop {
                // (One record a read; EPIPE: some were overwritten, read on.)
                let n = match kmsg.read(&mut buf) {
                    Ok(0) => return,
                    Ok(n) => n,
                    Err(e) if e.raw_os_error() == Some(libc::EPIPE) => continue,
                    Err(e) => {
                        crate::log(format!("Bluetooth: reading the kernel's messages: {e}"));
                        return;
                    }
                };
                let text = String::from_utf8_lossy(&buf[..n]);
                let Some((at, message)) = record(&text) else { continue };
                if at + RECENT_US < started {
                    continue;
                }
                let Some((hci, how)) = failure(message) else { continue };
                let times = seen.entry(hci.clone()).or_default();
                times.retain(|t| at.saturating_sub(*t) < WINDOW_US);
                times.push(at);
                if times.len() >= FAILURES {
                    times.clear();
                    if tx.send(Ev::Bluetooth { hci, why: format!("{how}: \"{message}\"") }).is_err() {
                        return;
                    }
                }
            }
        })
        .expect("spawn");
}

/// BlueZ's adapters whose Powered and PowerState disagree (a controller
/// that doesn't answer), by their hci name.
pub fn disagreeing(conn: &Connection) -> Vec<String> {
    let Ok(reply) = conn.call_method(Some(BLUEZ), "/", Some("org.freedesktop.DBus.ObjectManager"), "GetManagedObjects", &()) else { return Vec::new() };
    type Objects = HashMap<zvariant::OwnedObjectPath, HashMap<String, HashMap<String, zvariant::OwnedValue>>>;
    let Ok(objects) = reply.body().deserialize::<Objects>() else { return Vec::new() };
    let mut out = Vec::new();
    for (path, ifaces) in objects {
        let Some(a) = ifaces.get(ADAPTER) else { continue };
        let powered = a.get("Powered").and_then(|v| bool::try_from(v.clone()).ok());
        let state = a.get("PowerState").and_then(|v| String::try_from(v.clone()).ok());
        if let (Some(p), Some(s)) = (powered, state)
            && ((p && s == "off") || (!p && s == "on"))
        {
            out.push(path.as_str().rsplit('/').next().unwrap_or_default().to_owned());
        }
    }
    out
}

/// After start: adapters that disagree now and still do five seconds on.
pub fn check_at_start(conn: Connection, tx: mpsc::Sender<Ev>) {
    std::thread::spawn(move || {
        let first = disagreeing(&conn);
        if first.is_empty() {
            return;
        }
        std::thread::sleep(Duration::from_secs(5));
        for hci in disagreeing(&conn).into_iter().filter(|h| first.contains(h)) {
            let _ = tx.send(Ev::Bluetooth { hci, why: "BlueZ has it powered and its power off (or the other way round): it doesn't answer".into() });
        }
    });
}

/// Controller `hci`'s device and driver (as sysfs has them).
pub fn device_of(hci: &str) -> Result<(PathBuf, PathBuf), String> {
    let device = std::fs::canonicalize(format!("/sys/class/bluetooth/{hci}/device")).map_err(|e| format!("its device: {e}"))?;
    let driver = std::fs::canonicalize(device.join("driver")).map_err(|e| format!("its driver: {e}"))?;
    Ok((device, driver))
}

/// The driver's name ("btintel_pcie").
pub fn driver_name(hci: &str) -> String {
    device_of(hci).ok().and_then(|(_, d)| d.file_name().map(|f| f.to_string_lossy().into_owned())).unwrap_or_default()
}

/// Reset controller `hci`: its driver unbound, the device reset if it can
/// be (PCI's function reset), and bound again.
pub fn reset(hci: &str) -> Result<(), String> {
    let (device, driver) = device_of(hci)?;
    let id = device.file_name().ok_or("no device name")?.to_string_lossy().into_owned();
    std::fs::write(driver.join("unbind"), &id).map_err(|e| format!("unbinding {id}: {e}"))?;
    std::thread::sleep(Duration::from_millis(500));
    if device.join("reset").exists()
        && let Err(e) = std::fs::write(device.join("reset"), "1")
    {
        crate::log(format!("Bluetooth: {id}'s reset: {e} (bound again as it is)"));
    }
    std::thread::sleep(Duration::from_millis(500));
    std::fs::write(driver.join("bind"), &id).map_err(|e| format!("binding {id}: {e}"))
}

/// Whether BlueZ has controller `hci` powered and on (after a reset).
pub fn answering(conn: &Connection, hci: &str) -> bool {
    let path = format!("/org/bluez/{hci}");
    let get = |name: &str| {
        conn.call_method(Some(BLUEZ), path.as_str(), Some("org.freedesktop.DBus.Properties"), "Get", &(ADAPTER, name)).ok().and_then(|r| r.body().deserialize::<zvariant::OwnedValue>().ok())
    };
    let powered = get("Powered").and_then(|v| bool::try_from(v).ok()).unwrap_or(false);
    let state = get("PowerState").and_then(|v| String::try_from(v).ok()).unwrap_or_default();
    powered && state == "on"
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The kernel's words for a controller that stopped answering (as on
    /// 2026-11-09), and others that aren't.
    #[test]
    fn failures_are_told_apart() {
        assert_eq!(failure("Bluetooth: hci0: sending frame failed (-19)").map(|f| f.0), Some("hci0".into()));
        assert!(failure("Bluetooth: hci0: Opcode 0x2042 failed: -19").is_some());
        assert!(failure("Bluetooth: hci1: command 0x0c03 tx timeout").is_some());
        assert!(failure("Bluetooth: hci0: Hardware error 0x0c").is_some());
        assert!(failure("Bluetooth: hci0: HCI reset during shutdown failed").is_some());
        assert!(failure("Bluetooth: hci0: Firmware loaded").is_none());
        assert!(failure("Bluetooth: BNEP (Ethernet Emulation) ver 1.3").is_none());
        assert!(failure("usb 3-10: new full-speed USB device").is_none());
        assert_eq!(record("6,1234,98765432,-;Bluetooth: hci0: sending frame failed (-19)\n SUBSYSTEM=pci"), Some((98765432, "Bluetooth: hci0: sending frame failed (-19)")));
    }
}
