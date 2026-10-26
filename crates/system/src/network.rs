//! NetworkManager: the Wi-Fi network in use and its signal.

use zvariant::OwnedObjectPath;

use crate::{Result, bus};

const DEST: &str = "org.freedesktop.NetworkManager";
const DEVICE: &str = "org.freedesktop.NetworkManager.Device";
const WIRELESS: &str = "org.freedesktop.NetworkManager.Device.Wireless";
const AP: &str = "org.freedesktop.NetworkManager.AccessPoint";
/// NM_DEVICE_TYPE_WIFI.
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
    let devices: Vec<OwnedObjectPath> =
        bus::property(conn, DEST, "/org/freedesktop/NetworkManager", "org.freedesktop.NetworkManager", "Devices")?;
    for dev in devices {
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
