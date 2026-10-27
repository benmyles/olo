//! UDisks2: the disks, their partitions and what is on them; mounting,
//! formatting, partitioning, encryption (LUKS), naming, ejecting, SMART
//! health, writing an image and a read benchmark. Everything blocks, so
//! call it from a worker. UDisks asks polkit before privileged steps, so
//! those bring up the session's authentication agent.
//!
//! `TABULA_UDISKS_BUS` (a D-Bus address) talks to that bus instead of the
//! system bus: tests run `udisks-fake` there and never touch real disks.

use std::{
    collections::HashMap,
    os::fd::{AsRawFd, OwnedFd},
    sync::{
        OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use zbus::blocking::Connection;
use zvariant::{OwnedObjectPath, OwnedValue, Value};

use crate::{Result, bus};

const DEST: &str = "org.freedesktop.UDisks2";
const ROOT: &str = "/org/freedesktop/UDisks2";
const MANAGER: &str = "org.freedesktop.UDisks2.Manager";
const BLOCK: &str = "org.freedesktop.UDisks2.Block";
const PARTITION: &str = "org.freedesktop.UDisks2.Partition";
const TABLE: &str = "org.freedesktop.UDisks2.PartitionTable";
const FILESYSTEM: &str = "org.freedesktop.UDisks2.Filesystem";
const ENCRYPTED: &str = "org.freedesktop.UDisks2.Encrypted";
const LOOP: &str = "org.freedesktop.UDisks2.Loop";
const SWAP: &str = "org.freedesktop.UDisks2.Swapspace";
const DRIVE: &str = "org.freedesktop.UDisks2.Drive";
const ATA: &str = "org.freedesktop.UDisks2.Drive.Ata";
const NVME: &str = "org.freedesktop.UDisks2.NVMe.Controller";
const JOB: &str = "org.freedesktop.UDisks2.Job";

const MIB: u64 = 1 << 20;

/// The bus UDisks is on (the system bus, or `TABULA_UDISKS_BUS`).
pub fn conn() -> Result<&'static Connection> {
    static TEST: OnceLock<Connection> = OnceLock::new();
    match std::env::var("TABULA_UDISKS_BUS") {
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

/// A physical (or virtual) drive.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct Drive {
    pub path: String,
    pub vendor: String,
    pub model: String,
    pub serial: String,
    pub revision: String,
    pub size: u64,
    /// Removable media or a removable device (a USB stick, an SD card).
    pub removable: bool,
    pub ejectable: bool,
    pub can_power_off: bool,
    pub media_available: bool,
    /// "usb", "sdio", "ieee1394" or "" (internal).
    pub connection: String,
    /// rpm; 0 for solid state, -1 unknown.
    pub rotation: i32,
    pub optical: bool,
    pub health: Option<Health>,
}

impl Drive {
    /// Vendor and model, as people know it.
    pub fn name(&self) -> String {
        let model = self.model.trim();
        let vendor = self.vendor.trim();
        if vendor.is_empty() || model.to_lowercase().starts_with(&vendor.to_lowercase()) { model.to_owned() } else { format!("{vendor} {model}") }
    }

    /// What kind of drive, in words.
    pub fn kind(&self) -> &'static str {
        if self.optical {
            "Optical drive"
        } else if self.connection == "sdio" {
            "SD card"
        } else if self.connection == "usb" && self.removable {
            "USB drive"
        } else if self.removable {
            "Removable drive"
        } else if self.rotation > 0 {
            "Hard disk"
        } else {
            "Solid-state drive"
        }
    }
}

/// SMART health.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct Health {
    /// Degrees Celsius.
    pub temperature: Option<f64>,
    /// The drive predicts its own failure.
    pub failing: bool,
    pub power_on_hours: Option<u64>,
    /// NVMe critical warnings ("spare", "temperature", "degraded", …).
    pub warnings: Vec<String>,
}

impl Health {
    pub fn ok(&self) -> bool {
        !self.failing && self.warnings.is_empty()
    }
}

/// A partition, or what fills a disk without partitions, or an unlocked
/// encrypted volume's contents.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct Volume {
    pub path: String,
    /// "/dev/nvme0n1p2".
    pub device: String,
    pub size: u64,
    /// Where it starts on the disk.
    pub offset: u64,
    pub number: Option<u32>,
    /// The partition's own name (GPT).
    pub name: String,
    /// The partition type (a GPT GUID or an MBR number like "0x83").
    pub part_type: String,
    /// The file system or other content ("ext4", "crypto_LUKS", "swap").
    pub content: String,
    /// "filesystem", "crypto", "other" or "" (empty or unknown).
    pub usage: String,
    pub label: String,
    pub uuid: String,
    /// It has a file system UDisks can mount.
    pub mountable: bool,
    pub mount_points: Vec<String>,
    pub read_only: bool,
    /// Part of the running system (UDisks's hint): treat with care.
    pub system: bool,
    /// An extended partition (holds logical ones).
    pub container: bool,
    /// A logical partition inside an extended one.
    pub contained: bool,
    pub encrypted: bool,
    /// An unlocked encrypted volume's contents.
    pub cleartext: Option<Box<Volume>>,
    pub swap_active: bool,
    /// How full it is, when mounted (at its first mount point).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub space: Option<Space>,
}

/// How full a mounted file system is, in bytes (statvfs).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct Space {
    pub size: u64,
    pub used: u64,
    /// What can still be written (less the blocks kept for root).
    pub free: u64,
}

impl Space {
    /// How full, 0 to 1, as `df` counts it (the root reserve left out).
    pub fn fraction(&self) -> f32 {
        let total = self.used + self.free;
        if total == 0 { 0.0 } else { self.used as f32 / total as f32 }
    }
}

/// The file system mounted at `point`. Blocking: call it from a worker.
pub fn space(point: &str) -> Option<Space> {
    let path = std::ffi::CString::new(point).ok()?;
    // SAFETY: a NUL-terminated path, and a plain struct for statvfs to fill.
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(path.as_ptr(), &mut s) } != 0 {
        return None;
    }
    let unit = s.f_frsize;
    Some(Space { size: s.f_blocks * unit, used: s.f_blocks.saturating_sub(s.f_bfree) * unit, free: s.f_bavail * unit })
}

impl Volume {
    pub fn mounted(&self) -> bool {
        !self.mount_points.is_empty()
    }

    /// The content in words.
    pub fn content_name(&self) -> String {
        content_name(&self.content, &self.usage)
    }

    /// What to call it: its label, its partition name, or what it is.
    pub fn title(&self) -> String {
        if !self.label.is_empty() {
            return self.label.clone();
        }
        if let Some(c) = &self.cleartext
            && !c.label.is_empty()
        {
            return c.label.clone();
        }
        if !self.name.is_empty() {
            return self.name.clone();
        }
        self.content_name()
    }

    /// Busy: mounted, active swap, or unlocked (with whatever is inside).
    pub fn in_use(&self) -> bool {
        self.mounted() || self.swap_active || self.cleartext.is_some()
    }
}

/// A file system or other content, in words.
pub fn content_name(content: &str, usage: &str) -> String {
    match content {
        "" if usage.is_empty() => "Empty".into(),
        "ext2" | "ext3" | "ext4" => format!("Linux ({content})"),
        "btrfs" => "Linux (Btrfs)".into(),
        "xfs" => "Linux (XFS)".into(),
        "f2fs" => "Linux (F2FS)".into(),
        "vfat" => "FAT".into(),
        "exfat" => "exFAT".into(),
        "ntfs" => "NTFS".into(),
        "apfs" => "APFS".into(),
        "hfsplus" => "Mac OS (HFS+)".into(),
        "crypto_LUKS" => "Encrypted (LUKS)".into(),
        "swap" => "Swap".into(),
        "iso9660" => "CD/DVD (ISO 9660)".into(),
        "udf" => "UDF".into(),
        "LVM2_member" => "LVM".into(),
        "linux_raid_member" => "RAID member".into(),
        other => other.to_owned(),
    }
}

/// A disk: a whole block device.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct Disk {
    pub path: String,
    pub device: String,
    pub size: u64,
    pub read_only: bool,
    pub drive: Option<Drive>,
    /// "gpt" or "dos" when it has a partition table.
    pub table: Option<String>,
    /// Its partitions by offset, or, without a table, what fills it (one
    /// volume at offset 0, when there is anything).
    pub volumes: Vec<Volume>,
    /// A loop device's backing file.
    pub loop_file: Option<String>,
    /// The disk holds the running system.
    pub system: bool,
}

/// A stretch of a disk: a volume (by index) or free space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Segment {
    Volume(usize),
    Free { offset: u64, size: u64 },
}

impl Disk {
    pub fn name(&self) -> String {
        if let Some(d) = &self.drive {
            let n = d.name();
            if !n.is_empty() {
                return n;
            }
        }
        if let Some(f) = &self.loop_file {
            let name = std::path::Path::new(f).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            return format!("Disk image “{name}”");
        }
        self.device.clone()
    }

    pub fn kind(&self) -> &'static str {
        match &self.drive {
            Some(d) => d.kind(),
            None if self.loop_file.is_some() => "Disk image",
            None => "Disk",
        }
    }

    /// The disk from start to end: its top-level partitions and the free
    /// space between them (gaps under 1 MiB, alignment, are left out).
    /// Without a table it is one volume, or nothing (empty).
    pub fn segments(&self) -> Vec<Segment> {
        if self.table.is_none() {
            return (0..self.volumes.len()).map(Segment::Volume).collect();
        }
        let (start, end) = self.usable();
        let mut out = Vec::new();
        let mut at = start;
        let mut top: Vec<usize> = (0..self.volumes.len()).filter(|&i| !self.volumes[i].contained).collect();
        top.sort_by_key(|&i| self.volumes[i].offset);
        for i in top {
            let v = &self.volumes[i];
            if v.offset >= at + MIB {
                out.push(Segment::Free { offset: at, size: v.offset - at });
            }
            out.push(Segment::Volume(i));
            at = at.max(v.offset + v.size);
        }
        if end >= at + MIB {
            out.push(Segment::Free { offset: at, size: end - at });
        }
        out
    }

    /// Where partitions can go: after the first MiB (the table and
    /// alignment) and, on GPT, before the backup table at the end.
    pub fn usable(&self) -> (u64, u64) {
        let end = if self.table.as_deref() == Some("gpt") { self.size.saturating_sub(MIB) } else { self.size };
        (MIB.min(self.size), end)
    }

    /// Any volume on it is in use.
    pub fn in_use(&self) -> bool {
        self.volumes.iter().any(Volume::in_use)
    }
}

/// A job UDisks is doing (formatting, resizing, …).
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct Job {
    pub path: String,
    pub operation: String,
    /// 0–1, when it knows.
    pub progress: Option<f64>,
    /// The objects it works on.
    pub objects: Vec<String>,
    pub cancelable: bool,
    /// Bytes a second, when it knows.
    pub rate: u64,
    /// When it expects to end (µs since the epoch), when it knows.
    pub expected_end: u64,
}

impl Job {
    /// The operation in words.
    pub fn title(&self) -> String {
        let op = self.operation.as_str();
        match op {
            "format-erase" => "Erasing".into(),
            o if o.starts_with("format-") => "Formatting".into(),
            "partition-create" => "Creating a partition".into(),
            "partition-delete" => "Deleting a partition".into(),
            "partition-resize" | "filesystem-resize" => "Resizing".into(),
            "filesystem-mount" => "Mounting".into(),
            "filesystem-unmount" => "Unmounting".into(),
            "encrypted-unlock" => "Unlocking".into(),
            "encrypted-lock" => "Locking".into(),
            "filesystem-check" => "Checking".into(),
            "filesystem-repair" => "Repairing".into(),
            "drive-eject" => "Ejecting".into(),
            "ata-smart-selftest" | "nvme-selftest" => "Testing the drive".into(),
            other => other.replace('-', " "),
        }
    }
}

/// Everything UDisks knows.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct Status {
    /// Real drives first (internal before removable), then disk images.
    pub disks: Vec<Disk>,
    pub jobs: Vec<Job>,
    /// File systems UDisks can make here ("ext4", "exfat", …).
    pub filesystems: Vec<String>,
}

impl Status {
    pub fn disk(&self, path: &str) -> Option<&Disk> {
        self.disks.iter().find(|d| d.path == path)
    }

    /// The volume at `path` (or one unlocked inside it) and its disk.
    pub fn volume(&self, path: &str) -> Option<(&Disk, &Volume)> {
        self.disks.iter().find_map(|d| {
            d.volumes.iter().find_map(|v| {
                if v.path == path {
                    Some((d, v))
                } else {
                    v.cleartext.as_deref().filter(|c| c.path == path).map(|c| (d, c))
                }
            })
        })
    }

    /// Jobs working on `path`.
    pub fn jobs_on(&self, path: &str) -> Vec<&Job> {
        self.jobs.iter().filter(|j| j.objects.iter().any(|o| o == path)).collect()
    }
}

type Props = HashMap<String, OwnedValue>;
type Objects = HashMap<OwnedObjectPath, HashMap<String, Props>>;

fn get<T: TryFrom<OwnedValue>>(props: &Props, key: &str) -> Option<T> {
    props.get(key).and_then(|v| T::try_from(v.try_clone().ok()?).ok())
}

fn string(props: &Props, key: &str) -> String {
    get(props, key).unwrap_or_default()
}

fn path_of(props: &Props, key: &str) -> Option<String> {
    get::<OwnedObjectPath>(props, key).map(|p| p.as_str().to_owned()).filter(|p| p != "/")
}

/// A NUL-terminated byte string (UDisks's `ay` paths).
fn bytes(v: Vec<u8>) -> String {
    String::from_utf8_lossy(v.strip_suffix(&[0]).unwrap_or(&v)).into_owned()
}

fn bytes_of(props: &Props, key: &str) -> String {
    get::<Vec<u8>>(props, key).map(bytes).unwrap_or_default()
}

fn drive_of(path: &str, ifaces: &HashMap<String, Props>) -> Option<Drive> {
    let d = ifaces.get(DRIVE)?;
    let health = if let Some(a) = ifaces.get(ATA).filter(|a| get::<bool>(a, "SmartSupported").unwrap_or(false)) {
        let kelvin: f64 = get(a, "SmartTemperature").unwrap_or(0.0);
        Some(Health {
            temperature: (kelvin > 0.0).then_some(kelvin - 273.15),
            failing: get(a, "SmartFailing").unwrap_or(false),
            power_on_hours: get::<u64>(a, "SmartPowerOnSeconds").filter(|s| *s > 0).map(|s| s / 3600),
            warnings: Vec::new(),
        })
    } else {
        ifaces.get(NVME).filter(|n| get::<u64>(n, "SmartUpdated").unwrap_or(0) > 0).map(|n| {
            let kelvin: u16 = get(n, "SmartTemperature").unwrap_or(0);
            Health {
                temperature: (kelvin > 0).then_some(kelvin as f64 - 273.15),
                failing: false,
                power_on_hours: get(n, "SmartPowerOnHours"),
                warnings: get::<Vec<String>>(n, "SmartCriticalWarning").unwrap_or_default(),
            }
        })
    };
    Some(Drive {
        path: path.to_owned(),
        vendor: string(d, "Vendor"),
        model: string(d, "Model"),
        serial: string(d, "Serial"),
        revision: string(d, "Revision"),
        size: get(d, "Size").unwrap_or(0),
        removable: get(d, "Removable").unwrap_or(false) || get(d, "MediaRemovable").unwrap_or(false),
        ejectable: get(d, "Ejectable").unwrap_or(false),
        can_power_off: get(d, "CanPowerOff").unwrap_or(false),
        media_available: get(d, "MediaAvailable").unwrap_or(true),
        connection: string(d, "ConnectionBus"),
        rotation: get(d, "RotationRate").unwrap_or(-1),
        optical: get(d, "Optical").unwrap_or(false),
        health,
    })
}

fn volume_of(path: &str, ifaces: &HashMap<String, Props>, objects: &Objects) -> Volume {
    let empty = Props::new();
    let b = ifaces.get(BLOCK).unwrap_or(&empty);
    let p = ifaces.get(PARTITION);
    let fs = ifaces.get(FILESYSTEM);
    let encrypted = ifaces.get(ENCRYPTED);
    let cleartext = encrypted.and_then(|e| path_of(e, "CleartextDevice")).or_else(|| {
        // Older UDisks: find the block whose backing device is this one.
        encrypted?;
        objects.iter().find(|(_, i)| i.get(BLOCK).and_then(|b| path_of(b, "CryptoBackingDevice")).as_deref() == Some(path)).map(|(o, _)| o.as_str().to_owned())
    });
    let cleartext = cleartext.and_then(|c| {
        let (o, i) = objects.iter().find(|(o, _)| o.as_str() == c)?;
        Some(Box::new(volume_of(o.as_str(), i, objects)))
    });
    Volume {
        path: path.to_owned(),
        device: Some(bytes_of(b, "PreferredDevice")).filter(|d| !d.is_empty()).unwrap_or_else(|| bytes_of(b, "Device")),
        size: p.and_then(|p| get(p, "Size")).or_else(|| get(b, "Size")).unwrap_or(0),
        offset: p.and_then(|p| get(p, "Offset")).unwrap_or(0),
        number: p.and_then(|p| get(p, "Number")),
        name: p.map(|p| string(p, "Name")).unwrap_or_default(),
        part_type: p.map(|p| string(p, "Type")).unwrap_or_default(),
        content: string(b, "IdType"),
        usage: string(b, "IdUsage"),
        label: string(b, "IdLabel"),
        uuid: string(b, "IdUUID"),
        mountable: fs.is_some(),
        mount_points: fs.and_then(|f| get::<Vec<Vec<u8>>>(f, "MountPoints")).unwrap_or_default().into_iter().map(bytes).collect(),
        read_only: get(b, "ReadOnly").unwrap_or(false),
        system: get(b, "HintSystem").unwrap_or(false),
        container: p.and_then(|p| get(p, "IsContainer")).unwrap_or(false),
        contained: p.and_then(|p| get(p, "IsContained")).unwrap_or(false),
        encrypted: encrypted.is_some(),
        cleartext,
        swap_active: ifaces.get(SWAP).and_then(|s| get(s, "Active")).unwrap_or(false),
        space: None,
    }
}

/// Fill in how full each mounted volume is.
fn measure(v: &mut Volume) {
    v.space = v.mount_points.first().and_then(|p| space(p));
    if let Some(inner) = &mut v.cleartext {
        measure(inner);
    }
}

/// Read everything from GetManagedObjects' answer.
fn parse(objects: &Objects) -> (Vec<Disk>, Vec<Job>) {
    let mut disks = Vec::new();
    for (path, ifaces) in objects {
        let Some(b) = ifaces.get(BLOCK) else { continue };
        // Whole disks only: not partitions, not unlocked encrypted volumes.
        if ifaces.contains_key(PARTITION) || path_of(b, "CryptoBackingDevice").is_some() {
            continue;
        }
        let size: u64 = get(b, "Size").unwrap_or(0);
        let device = bytes_of(b, "Device");
        let drive = path_of(b, "Drive").and_then(|d| objects.iter().find(|(o, _)| o.as_str() == d)).and_then(|(o, i)| drive_of(o.as_str(), i));
        let loop_file = ifaces.get(LOOP).map(|l| bytes_of(l, "BackingFile"));
        // Unconnected loop devices, zram and the like: not disks to manage.
        if drive.is_none() && loop_file.as_deref().is_none_or(str::is_empty) {
            continue;
        }
        if size == 0 && drive.as_ref().is_none_or(|d| !d.removable) {
            continue;
        }
        let table = ifaces.get(TABLE).map(|t| string(t, "Type"));
        let mut volumes: Vec<Volume> = if table.is_some() {
            objects
                .iter()
                .filter(|(_, i)| i.get(PARTITION).and_then(|p| path_of(p, "Table")).as_deref() == Some(path.as_str()))
                .map(|(o, i)| volume_of(o.as_str(), i, objects))
                .collect()
        } else if !string(b, "IdUsage").is_empty() || !string(b, "IdType").is_empty() {
            vec![volume_of(path.as_str(), ifaces, objects)]
        } else {
            Vec::new()
        };
        volumes.sort_by_key(|v| v.offset);
        let system = get(b, "HintSystem").unwrap_or(false) && volumes.iter().any(|v| v.system && (v.mounted() || v.cleartext.as_ref().is_some_and(|c| c.mounted())));
        disks.push(Disk { path: path.as_str().to_owned(), device, size, read_only: get(b, "ReadOnly").unwrap_or(false), drive, table, volumes, loop_file, system });
    }
    disks.sort_by(|a, b| {
        let rank = |d: &Disk| (d.drive.is_none(), d.drive.as_ref().is_some_and(|d| d.removable), d.device.clone());
        rank(a).cmp(&rank(b))
    });
    let mut jobs: Vec<Job> = objects
        .iter()
        .filter_map(|(path, ifaces)| {
            let j = ifaces.get(JOB)?;
            Some(Job {
                path: path.as_str().to_owned(),
                operation: string(j, "Operation"),
                progress: get::<bool>(j, "ProgressValid").unwrap_or(false).then(|| get::<f64>(j, "Progress").unwrap_or(0.0)),
                objects: get::<Vec<OwnedObjectPath>>(j, "Objects").unwrap_or_default().into_iter().map(|o| o.as_str().to_owned()).collect(),
                cancelable: get(j, "Cancelable").unwrap_or(false),
                rate: get(j, "Rate").unwrap_or(0),
                expected_end: get(j, "ExpectedEndTime").unwrap_or(0),
            })
        })
        .collect();
    jobs.sort_by(|a, b| a.path.cmp(&b.path));
    (disks, jobs)
}

/// Everything UDisks knows now, and how full what is mounted is.
pub fn status() -> Result<Status> {
    let c = conn()?;
    let objects: Objects = bus::call(c, DEST, ROOT, "org.freedesktop.DBus.ObjectManager", "GetManagedObjects", &())?;
    let (mut disks, jobs) = parse(&objects);
    disks.iter_mut().flat_map(|d| d.volumes.iter_mut()).for_each(measure);
    let filesystems = bus::property::<Vec<String>>(c, DEST, &format!("{ROOT}/Manager"), MANAGER, "SupportedFilesystems").unwrap_or_default();
    Ok(Status { disks, jobs, filesystems })
}

/// Calls `changed` (on a thread of its own) whenever anything UDisks
/// knows changes; quick runs of changes come together.
pub fn watch(changed: impl Fn() + Send + 'static) -> Result<()> {
    let c = conn()?;
    let rule = zbus::MatchRule::builder().msg_type(zbus::message::Type::Signal).sender(DEST)?.build();
    let signals = zbus::blocking::MessageIterator::for_match_rule(rule, c, Some(256))?;
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    std::thread::Builder::new().name("udisks-signals".into()).spawn(move || {
        for _ in signals.flatten() {
            if tx.send(()).is_err() {
                return;
            }
        }
    })?;
    std::thread::Builder::new().name("udisks-changes".into()).spawn(move || {
        while rx.recv().is_ok() {
            // Let a burst finish (a format changes a dozen properties).
            std::thread::sleep(Duration::from_millis(80));
            while rx.try_recv().is_ok() {}
            changed();
        }
    })?;
    Ok(())
}

type Options<'a> = HashMap<&'a str, Value<'a>>;

fn call0<B: serde::Serialize + zvariant::DynamicType>(path: &str, iface: &str, method: &str, body: &B) -> Result<()> {
    conn()?.call_method(Some(DEST), path, Some(iface), method, body)?;
    Ok(())
}

/// Mount the file system on `block`; where it went.
pub fn mount(block: &str) -> Result<String> {
    bus::call(conn()?, DEST, block, FILESYSTEM, "Mount", &(Options::new(),))
}

pub fn unmount(block: &str) -> Result<()> {
    call0(block, FILESYSTEM, "Unmount", &(Options::new(),))
}

/// Unlock an encrypted volume; its contents' block.
pub fn unlock(block: &str, passphrase: &str) -> Result<String> {
    let o: OwnedObjectPath = bus::call(conn()?, DEST, block, ENCRYPTED, "Unlock", &(passphrase, Options::new()))?;
    Ok(o.as_str().to_owned())
}

pub fn lock(block: &str) -> Result<()> {
    call0(block, ENCRYPTED, "Lock", &(Options::new(),))
}

pub fn set_label(block: &str, label: &str) -> Result<()> {
    call0(block, FILESYSTEM, "SetLabel", &(label, Options::new()))
}

pub fn set_partition_name(block: &str, name: &str) -> Result<()> {
    call0(block, PARTITION, "SetName", &(name, Options::new()))
}

/// How to make a file system.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Format {
    /// "ext4", "exfat", … ("empty" leaves nothing on it).
    pub kind: String,
    pub label: String,
    /// Write zeros over it first (slow; makes old data unrecoverable).
    pub erase: bool,
    /// Encrypt it (LUKS) with this passphrase.
    pub passphrase: Option<String>,
}

impl Format {
    fn options(&self) -> Options<'_> {
        let mut o = Options::new();
        if !self.label.is_empty() {
            o.insert("label", Value::from(self.label.as_str()));
        }
        if self.erase {
            o.insert("erase", Value::from("zero"));
        }
        if let Some(p) = &self.passphrase {
            o.insert("encrypt.passphrase", Value::from(p.as_str()));
        }
        // The person formatting it owns it (not root), as for a stick.
        if matches!(self.kind.as_str(), "ext2" | "ext3" | "ext4" | "xfs" | "btrfs" | "f2fs" | "nilfs2") {
            o.insert("take-ownership", Value::from(true));
        }
        o.insert("update-partition-type", Value::from(true));
        o
    }
}

/// The file systems to offer, as (kind, name), best first, of those
/// UDisks can make here.
pub fn formats(supported: &[String]) -> Vec<(&'static str, &'static str)> {
    const ALL: [(&str, &str); 6] = [
        ("exfat", "exFAT (works everywhere)"),
        ("ext4", "Linux (ext4)"),
        ("btrfs", "Linux (Btrfs)"),
        ("xfs", "Linux (XFS)"),
        ("vfat", "FAT (older devices)"),
        ("ntfs", "NTFS (Windows)"),
    ];
    ALL.into_iter().filter(|(k, _)| supported.iter().any(|s| s == k)).collect()
}

/// Erase `block` and make a file system on it (a partition, or a whole
/// disk without partitions).
pub fn format(block: &str, f: &Format) -> Result<()> {
    call0(block, BLOCK, "Format", &(f.kind.as_str(), f.options()))
}

/// Erase a whole disk and give it a new, empty partition table ("gpt" or
/// "dos").
pub fn new_table(disk: &str, kind: &str, erase: bool) -> Result<()> {
    let mut o = Options::new();
    if erase {
        o.insert("erase", Value::from("zero"));
    }
    call0(disk, BLOCK, "Format", &(kind, o))
}

/// The partition type for a file system on a table of `table` type
/// (GPT's GUIDs, MBR's numbers).
pub fn partition_type(table: &str, kind: &str, encrypted: bool) -> &'static str {
    let windows = matches!(kind, "exfat" | "vfat" | "ntfs") && !encrypted;
    match (table, windows) {
        ("gpt", true) => "ebd0a0a2-b9e5-4433-87c0-68b6b72699c7",
        ("gpt", false) => "0fc63daf-8483-4772-8e79-3d69d8477de4",
        (_, true) if kind == "vfat" => "0x0c",
        (_, true) => "0x07",
        _ => "0x83",
    }
}

/// Make a partition on `disk` (offset and size in bytes, aligned to MiB)
/// with a file system; its block.
pub fn create_partition(disk: &str, table: &str, offset: u64, size: u64, name: &str, f: &Format) -> Result<String> {
    let offset = offset.div_ceil(MIB) * MIB;
    let size = size / MIB * MIB;
    let kind = if f.kind.is_empty() { "empty" } else { f.kind.as_str() };
    let part_type = partition_type(table, kind, f.passphrase.is_some());
    let name = if table == "gpt" { name } else { "" };
    let o: OwnedObjectPath = bus::call(
        conn()?,
        DEST,
        disk,
        TABLE,
        "CreatePartitionAndFormat",
        &(offset, size, part_type, name, Options::new(), kind, f.options()),
    )?;
    Ok(o.as_str().to_owned())
}

pub fn delete_partition(block: &str) -> Result<()> {
    call0(block, PARTITION, "Delete", &(Options::new(),))
}

/// Resize a partition and its file system to `size` bytes: a file system
/// shrinks before its partition and grows after it.
pub fn resize(block: &str, current: u64, size: u64, has_filesystem: bool) -> Result<()> {
    let size = size / MIB * MIB;
    if size < current {
        if has_filesystem {
            call0(block, FILESYSTEM, "Resize", &(size, Options::new()))?;
        }
        call0(block, PARTITION, "Resize", &(size, Options::new()))
    } else {
        call0(block, PARTITION, "Resize", &(size, Options::new()))?;
        if has_filesystem {
            // 0: fill the partition.
            call0(block, FILESYSTEM, "Resize", &(0u64, Options::new()))?;
        }
        Ok(())
    }
}

/// Whether a file system of `kind` can be resized here: (shrink, grow),
/// or why not.
pub fn can_resize(kind: &str) -> std::result::Result<(bool, bool), String> {
    let r: Result<(bool, u64, String)> = conn().and_then(|c| bus::call(c, DEST, &format!("{ROOT}/Manager"), MANAGER, "CanResize", &(kind,)));
    match r {
        Ok((true, mode, _)) => Ok((mode & (2 | 8) != 0, mode & (4 | 16) != 0)),
        Ok((false, _, missing)) if !missing.is_empty() => Err(format!("Resizing needs {missing}, which isn't installed.")),
        Ok(_) => Err("This file system can't be resized.".into()),
        Err(e) => Err(error_text(&e)),
    }
}

/// Stop using a volume: unmount it, or what is unlocked inside it and
/// lock it again (before formatting or deleting it).
pub fn release(v: &Volume) -> Result<()> {
    if let Some(c) = &v.cleartext {
        if c.mounted() {
            unmount(&c.path)?;
        }
        lock(&v.path)?;
    }
    if v.mounted() {
        unmount(&v.path)?;
    }
    Ok(())
}

/// Make a disk safe to unplug: unmount and lock everything on it, then
/// eject it (and power it off when it can be).
pub fn eject(disk: &Disk) -> Result<()> {
    for v in &disk.volumes {
        release(v)?;
    }
    let Some(d) = &disk.drive else { return Ok(()) };
    if d.ejectable {
        call0(&d.path, DRIVE, "Eject", &(Options::new(),))?;
    }
    if d.can_power_off {
        call0(&d.path, DRIVE, "PowerOff", &(Options::new(),))?;
    }
    Ok(())
}

/// Ask the drive for fresh SMART data.
pub fn smart_update(drive: &str, nvme: bool) -> Result<()> {
    call0(drive, if nvme { NVME } else { ATA }, "SmartUpdate", &(Options::new(),))
}

pub fn cancel(job: &str) -> Result<()> {
    call0(job, JOB, "Cancel", &(Options::new(),))
}

/// Words for an error from UDisks.
pub fn error_text(e: &zbus::Error) -> String {
    if let zbus::Error::MethodError(name, msg, _) = e {
        let msg = msg.clone().unwrap_or_default();
        let msg = msg.strip_prefix("GDBus.Error:").map(|m| m.split_once(' ').map_or(m, |(_, r)| r)).unwrap_or(&msg).trim().to_owned();
        return match name.as_str() {
            "org.freedesktop.UDisks2.Error.NotAuthorizedCanObtain" | "org.freedesktop.UDisks2.Error.NotAuthorized" | "org.freedesktop.UDisks2.Error.NotAuthorizedDismissed" => {
                "You weren't allowed to do that.".into()
            }
            "org.freedesktop.UDisks2.Error.Cancelled" => "It was stopped.".into(),
            "org.freedesktop.UDisks2.Error.DeviceBusy" => format!("It is in use. {msg}"),
            // What cryptsetup says to a wrong passphrase.
            _ if msg.contains("Failed to activate device: Operation not permitted") => "The passphrase isn't right.".into(),
            _ if msg.is_empty() => name.to_string(),
            _ => msg,
        };
    }
    e.to_string()
}

/// How far a long job has got.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Progress {
    pub done: u64,
    pub total: u64,
    /// Bytes a second.
    pub rate: f64,
}

/// Write the image file `image` onto `block`, all of it, block by block:
/// `progress` hears how far it got; `stop` cancels (the disk is then
/// partly written). Asks polkit for the device.
pub fn write_image(block: &str, image: &std::path::Path, stop: &AtomicBool, progress: &mut dyn FnMut(Progress)) -> std::result::Result<(), String> {
    use std::io::{Read, Write};
    let mut src = std::fs::File::open(image).map_err(|e| format!("Couldn't read the image: {e}"))?;
    let total = src.metadata().map(|m| m.len()).unwrap_or(0);
    let fd: zvariant::OwnedFd = conn()
        .and_then(|c| bus::call(c, DEST, block, BLOCK, "OpenForRestore", &(Options::new(),)))
        .map_err(|e| error_text(&e))?;
    let mut dst = std::fs::File::from(OwnedFd::from(fd));
    let mut buf = vec![0u8; 4 * MIB as usize];
    let (start, mut done, mut told) = (Instant::now(), 0u64, Instant::now());
    loop {
        if stop.load(Ordering::Relaxed) {
            return Err("Writing stopped part way, so the disk holds only some of the image: write it again or erase the disk before using it.".into());
        }
        let n = src.read(&mut buf).map_err(|e| format!("Couldn't read the image: {e}"))?;
        if n == 0 {
            break;
        }
        dst.write_all(&buf[..n]).map_err(|e| format!("Couldn't write to the disk: {e}"))?;
        done += n as u64;
        if told.elapsed() > Duration::from_millis(100) {
            told = Instant::now();
            progress(Progress { done, total, rate: done as f64 / start.elapsed().as_secs_f64().max(0.001) });
        }
    }
    match dst.sync_all() {
        // A pipe (udisks-fake hands one out) has nothing to sync.
        Err(e) if e.raw_os_error() == Some(libc::EINVAL) => {}
        r => r.map_err(|e| format!("Couldn't finish writing to the disk: {e}"))?,
    }
    progress(Progress { done, total, rate: done as f64 / start.elapsed().as_secs_f64().max(0.001) });
    Ok(())
}

/// What a read benchmark measured.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct Benchmark {
    /// MB/s of each sample, from the start of the disk to its end.
    pub samples: Vec<f64>,
    /// Average, slowest and fastest, MB/s.
    pub average: f64,
    pub min: f64,
    pub max: f64,
    /// Average time to read a random 4 KiB block, in milliseconds.
    pub access_ms: f64,
}

/// Read samples spread over the disk (never writes): `count` samples of
/// 8 MiB for the transfer rate, then 100 random 4 KiB reads for the
/// access time. `progress` hears each sample's rate as it is measured.
pub fn benchmark(block: &str, size: u64, count: usize, stop: &AtomicBool, progress: &mut dyn FnMut(usize, f64)) -> std::result::Result<Benchmark, String> {
    let mut o = Options::new();
    o.insert("writable", Value::from(false));
    let fd: zvariant::OwnedFd =
        conn().and_then(|c| bus::call(c, DEST, block, BLOCK, "OpenForBenchmark", &(o,))).map_err(|e| error_text(&e))?;
    let fd = OwnedFd::from(fd);
    const SAMPLE: usize = 8 << 20;
    // O_DIRECT (UDisks opens it so) wants aligned memory.
    let layout = std::alloc::Layout::from_size_align(SAMPLE, 4096).map_err(|e| e.to_string())?;
    // SAFETY: layout has a non-zero size.
    let ptr = unsafe { std::alloc::alloc(layout) };
    if ptr.is_null() {
        return Err("Out of memory.".into());
    }
    struct Buf(*mut u8, std::alloc::Layout);
    impl Drop for Buf {
        fn drop(&mut self) {
            // SAFETY: allocated with this layout above.
            unsafe { std::alloc::dealloc(self.0, self.1) }
        }
    }
    let buf = Buf(ptr, layout);
    let read_at = |offset: u64, len: usize| -> std::result::Result<(), String> {
        // SAFETY: the buffer holds SAMPLE bytes and len <= SAMPLE.
        let n = unsafe { libc::pread(fd.as_raw_fd(), buf.0.cast(), len, offset as libc::off_t) };
        if n < 0 { Err(format!("Couldn't read the disk: {}", std::io::Error::last_os_error())) } else { Ok(()) }
    };
    let span = size.saturating_sub(SAMPLE as u64) / 4096 * 4096;
    let mut out = Benchmark { min: f64::MAX, ..Default::default() };
    for i in 0..count {
        if stop.load(Ordering::Relaxed) {
            return Err("It was stopped.".into());
        }
        let offset = if count > 1 { span / (count as u64 - 1) * i as u64 / 4096 * 4096 } else { 0 };
        let t = Instant::now();
        read_at(offset, SAMPLE)?;
        let rate = SAMPLE as f64 / t.elapsed().as_secs_f64().max(1e-6) / 1e6;
        out.samples.push(rate);
        out.min = out.min.min(rate);
        out.max = out.max.max(rate);
        progress(i, rate);
    }
    out.average = out.samples.iter().sum::<f64>() / out.samples.len().max(1) as f64;
    if out.samples.is_empty() {
        out.min = 0.0;
    }
    // Random 4 KiB reads: a simple LCG is random enough here.
    let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
    let blocks = (size / 4096).max(1);
    let t = Instant::now();
    const READS: u32 = 100;
    for _ in 0..READS {
        if stop.load(Ordering::Relaxed) {
            return Err("It was stopped.".into());
        }
        x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        read_at((x >> 11) % blocks * 4096, 4096)?;
    }
    out.access_ms = t.elapsed().as_secs_f64() * 1000.0 / READS as f64;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn space_of_a_mounted_file_system() {
        let s = space("/").expect("statvfs /");
        assert!(s.size > 0 && s.used + s.free <= s.size);
        assert!((0.0..=1.0).contains(&s.fraction()));
        assert_eq!(space("/no/such/place"), None);
        assert_eq!(Space { size: 100, used: 75, free: 20 }.fraction(), 75.0 / 95.0);
        assert_eq!(Space::default().fraction(), 0.0);
    }

    fn v(offset: u64, size: u64) -> Volume {
        Volume { offset, size, ..Default::default() }
    }

    #[test]
    fn segments_find_free_space() {
        let disk = Disk { size: 100 * MIB, table: Some("gpt".into()), volumes: vec![v(MIB, 10 * MIB), v(20 * MIB, 30 * MIB)], ..Default::default() };
        assert_eq!(
            disk.segments(),
            vec![
                Segment::Volume(0),
                Segment::Free { offset: 11 * MIB, size: 9 * MIB },
                Segment::Volume(1),
                Segment::Free { offset: 50 * MIB, size: 49 * MIB },
            ]
        );
        let empty = Disk { size: 100 * MIB, table: Some("dos".into()), ..Default::default() };
        assert_eq!(empty.segments(), vec![Segment::Free { offset: MIB, size: 99 * MIB }]);
        // Alignment gaps don't count.
        let full = Disk { size: 12 * MIB, table: Some("dos".into()), volumes: vec![v(MIB, 11 * MIB - 4096)], ..Default::default() };
        assert_eq!(full.segments(), vec![Segment::Volume(0)]);
    }

    #[test]
    fn partition_types() {
        assert_eq!(partition_type("gpt", "exfat", false), "ebd0a0a2-b9e5-4433-87c0-68b6b72699c7");
        assert_eq!(partition_type("gpt", "exfat", true), "0fc63daf-8483-4772-8e79-3d69d8477de4");
        assert_eq!(partition_type("dos", "vfat", false), "0x0c");
        assert_eq!(partition_type("dos", "ext4", false), "0x83");
    }

    #[test]
    fn names() {
        let d = Drive { vendor: "SanDisk".into(), model: "SanDisk Ultra".into(), ..Default::default() };
        assert_eq!(d.name(), "SanDisk Ultra");
        let d = Drive { vendor: "Tabula".into(), model: "Test SSD".into(), ..Default::default() };
        assert_eq!(d.name(), "Tabula Test SSD");
        assert_eq!(content_name("crypto_LUKS", "crypto"), "Encrypted (LUKS)");
        assert_eq!(content_name("", ""), "Empty");
    }
}
