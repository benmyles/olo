//! SATA SSDs' power: how deeply each one's link may sleep (Preferences ▸
//! Power ▸ Power Modes, beside the NVMe drives; Ben, 2026-11-15: "we
//! should be able to configure the settings independently for all
//! SSD/nvme drives").
//!
//! A SATA drive's power is mostly its link's: Aggressive Link Power
//! Management (ALPM) lets the link between the drive and its host
//! adapter drop into partial or slumber states between commands, saving
//! a watt or so and its heat, at a small cost in wake-up latency. The
//! kernel sets it per host adapter, `/sys/class/scsi_host/hostN/
//! link_power_management_policy`: `max_performance` (never sleeps),
//! `medium_power`, `med_power_with_dipm` (partial, the drive allowed to
//! ask too: Fedora's default on laptops), `min_power` (slumber), or
//! `keep_firmware_settings`. Each SATA SSD is on a port of its own, so
//! its host is its own.
//!
//! The presets are the NVMe drives' (`nvme_power::NvmePreset`), so every
//! SSD's row has the same choices: As It Is leaves the link as found,
//! Cooler is `med_power_with_dipm`, Coolest `min_power`. Written by the
//! follower (`olo-gpu-tune`, root's), which keeps the policy it found so
//! As It Is puts that back; read from /sys without root ([`drives`]).

use std::path::{Path, PathBuf};

use crate::nvme_power::{Drive, NvmePreset};

/// Where the kernel keeps SATA hosts' link policies.
pub const HOSTS: &str = "/sys/class/scsi_host";

/// A SATA SSD and its host adapter ("host2").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SataDrive {
    pub drive: Drive,
    pub host: String,
}

/// Every SATA SSD here: libata's disks (`/sys/block/sd*` under an
/// `ata` port: not USB's, not a card reader's), not spinning.
pub fn drives() -> Vec<SataDrive> {
    drives_under(Path::new("/sys/block"))
}

pub fn drives_under(block: &Path) -> Vec<SataDrive> {
    let read = |p: PathBuf| std::fs::read_to_string(p).map(|s| s.trim().to_owned()).unwrap_or_default();
    let mut v: Vec<SataDrive> = std::fs::read_dir(block)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let p = e.path();
            if !name.starts_with("sd") || read(p.join("queue/rotational")) != "0" {
                return None;
            }
            // Its device's place: …/ata3/host2/target2:0:0/2:0:0:0.
            let place = std::fs::canonicalize(p.join("device")).ok()?;
            let parts: Vec<String> = place.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
            if !parts.iter().any(|c| c.starts_with("ata") && c[3..].chars().all(|d| d.is_ascii_digit()) && c.len() > 3) {
                return None;
            }
            let host = parts.iter().find(|c| c.strip_prefix("host").is_some_and(|n| !n.is_empty() && n.chars().all(|d| d.is_ascii_digit())))?.clone();
            let device = p.join("device");
            // Its serial: the unit serial number page (its first four bytes
            // a header), else the kernel's id for it, else its name.
            let serial = std::fs::read(device.join("vpd_pg80"))
                .ok()
                .filter(|b| b.len() > 4)
                .map(|b| String::from_utf8_lossy(&b[4..]).trim_matches(|c: char| c.is_whitespace() || c == '\0').to_owned())
                .filter(|s| !s.is_empty())
                .or_else(|| Some(read(device.join("wwid"))).filter(|s| !s.is_empty()))
                .unwrap_or_else(|| name.clone());
            let size = read(p.join("size")).parse::<u64>().unwrap_or(0) * 512;
            Some(SataDrive { drive: Drive { name, model: read(device.join("model")), serial, size, sata: true }, host })
        })
        .collect();
    v.sort_by(|a, b| a.drive.name.len().cmp(&b.drive.name.len()).then(a.drive.name.cmp(&b.drive.name)));
    v
}

/// The link policy a preset puts a drive on (None: as it is).
pub fn policy(p: NvmePreset) -> Option<&'static str> {
    match p {
        NvmePreset::AsIs => None,
        NvmePreset::Cooler => Some("med_power_with_dipm"),
        NvmePreset::Coolest => Some("min_power"),
    }
}

/// A preset on a SATA drive in words: "Cooler (link naps)".
pub fn choice_name(p: NvmePreset) -> String {
    match p {
        NvmePreset::AsIs => p.name().into(),
        NvmePreset::Cooler => format!("{} (link naps)", p.name()),
        NvmePreset::Coolest => format!("{} (link sleeps)", p.name()),
    }
}

/// A policy in words, for what the follower says it found.
pub fn policy_words(policy: &str) -> &'static str {
    match policy {
        "max_performance" => "its link never sleeps",
        "medium_power" => "its link naps",
        "med_power_with_dipm" => "its link naps, the drive asking too",
        "min_power" => "its link sleeps",
        "keep_firmware_settings" => "its link as its firmware set it",
        _ => "its link's own",
    }
}

fn policy_path(hosts: &Path, host: &str) -> PathBuf {
    hosts.join(host).join("link_power_management_policy")
}

/// Host `host`'s link policy now.
pub fn current(host: &str) -> std::io::Result<String> {
    current_in(Path::new(HOSTS), host)
}

pub fn current_in(hosts: &Path, host: &str) -> std::io::Result<String> {
    std::fs::read_to_string(policy_path(hosts, host)).map(|s| s.trim().to_owned())
}

/// Host `host`'s link put on `policy` (root).
pub fn set(host: &str, policy: &str) -> std::io::Result<()> {
    set_in(Path::new(HOSTS), host, policy)
}

pub fn set_in(hosts: &Path, host: &str, policy: &str) -> std::io::Result<()> {
    std::fs::write(policy_path(hosts, host), policy)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A laptop's /sys with a SATA SSD (on ata3, host2), a spinning disk,
    /// a USB stick and an NVMe drive's block device: only the SSD.
    #[test]
    fn sata_ssds_found() {
        let root = PathBuf::from(format!("/var/tmp/olo-sata-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let w = |p: &str, c: &[u8]| {
            let path = root.join(p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, c).unwrap();
        };
        let disk = |name: &str, place: &str, rotational: &str, model: &str| {
            let dev = root.join("devices").join(place);
            std::fs::create_dir_all(&dev).unwrap();
            std::fs::write(dev.join("model"), format!("{model}\n")).unwrap();
            w(&format!("block/{name}/queue/rotational"), format!("{rotational}\n").as_bytes());
            w(&format!("block/{name}/size"), b"1953525168\n");
            std::os::unix::fs::symlink(&dev, root.join(format!("block/{name}/device"))).unwrap();
            dev
        };
        let ssd = disk("sda", "pci0000:00/0000:00:17.0/ata3/host2/target2:0:0/2:0:0:0", "0", "Samsung SSD 870 EVO 1TB");
        let mut pg80 = vec![0u8, 0x80, 0, 15];
        pg80.extend_from_slice(b"S6PUNX0R123456A");
        std::fs::write(ssd.join("vpd_pg80"), &pg80).unwrap();
        disk("sdb", "pci0000:00/0000:00:17.0/ata4/host3/target3:0:0/3:0:0:0", "1", "WDC WD40EFRX");
        disk("sdc", "pci0000:00/0000:00:14.0/usb2/2-1/2-1:1.0/host6/target6:0:0/6:0:0:0", "0", "Flash Drive");
        w("block/nvme0n1/queue/rotational", b"0\n");
        let found = drives_under(&root.join("block"));
        assert_eq!(found.len(), 1, "{found:?}");
        let d = &found[0];
        assert_eq!((d.drive.name.as_str(), d.host.as_str(), d.drive.serial.as_str(), d.drive.model.as_str()), ("sda", "host2", "S6PUNX0R123456A", "Samsung SSD 870 EVO 1TB"));
        assert_eq!(d.drive.size, 1_000_204_886_016);
        assert!(d.drive.sata);
        // Its host's policy read and set.
        w("hosts/host2/link_power_management_policy", b"med_power_with_dipm\n");
        assert_eq!(current_in(&root.join("hosts"), "host2").unwrap(), "med_power_with_dipm");
        set_in(&root.join("hosts"), "host2", policy(NvmePreset::Coolest).unwrap()).unwrap();
        assert_eq!(current_in(&root.join("hosts"), "host2").unwrap(), "min_power");
        assert_eq!(policy(NvmePreset::AsIs), None);
        let _ = std::fs::remove_dir_all(&root);
    }
}
