//! Which links are wired Ethernet ports (`olo_net::wired_port`), from a
//! stand-in for `/sys/class/net` as the kernel lays it out.

use std::{fs, path::Path};

/// A link `name` in `net`: its files, and its device's driver.
fn link(net: &Path, name: &str, kind: &str, devtype: Option<&str>, device: Option<&str>, wireless: bool) {
    let dir = net.join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("type"), format!("{kind}\n")).unwrap();
    fs::write(dir.join("uevent"), format!("INTERFACE={name}\nIFINDEX=2\n{}", devtype.map(|t| format!("DEVTYPE={t}\n")).unwrap_or_default())).unwrap();
    if let Some(driver) = device {
        let dev = net.join("devices").join(name);
        fs::create_dir_all(dev.join("drivers").join(driver)).unwrap();
        std::os::unix::fs::symlink(dev.join("drivers").join(driver), dev.join("driver")).unwrap();
        std::os::unix::fs::symlink(&dev, dir.join("device")).unwrap();
    }
    if wireless {
        fs::create_dir_all(dir.join("wireless")).unwrap();
    }
}

#[test]
fn ethernet_ports_are_told_from_the_rest() {
    let net = Path::new(env!("CARGO_TARGET_TMPDIR")).join("wired-sys-class-net");
    let _ = fs::remove_dir_all(&net);
    // A dock's port and a laptop's own: wired.
    link(&net, "enp172s0u1u2", "1", None, Some("r8152"), false);
    link(&net, "enp0s31f6", "1", None, Some("e1000e"), false);
    // Wi-Fi (its own DEVTYPE and directory), a mobile modem, a bridge
    // and a container's pair (no device), a VPN (no Ethernet), a phone
    // sharing its data over USB: not.
    link(&net, "wlp0s20f3", "1", Some("wlan"), Some("iwlwifi"), true);
    link(&net, "wwan0", "1", Some("wwan"), Some("cdc_mbim"), false);
    link(&net, "docker0", "1", Some("bridge"), None, false);
    link(&net, "veth1a2b", "1", None, None, false);
    link(&net, "wg0", "65534", None, None, false);
    link(&net, "usb0", "1", None, Some("rndis_host"), false);
    link(&net, "enx-iphone", "1", None, Some("ipheth"), false);
    let wired: Vec<&str> =
        ["enp172s0u1u2", "enp0s31f6", "wlp0s20f3", "wwan0", "docker0", "veth1a2b", "wg0", "usb0", "enx-iphone", "gone0"].into_iter().filter(|n| olo_net::wired_port(&net, n)).collect();
    assert_eq!(wired, ["enp172s0u1u2", "enp0s31f6"]);
    let _ = fs::remove_dir_all(&net);
}
