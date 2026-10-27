//! `udisks-fake [--address ADDRESS]`: a stand-in UDisks2 for headless
//! sessions and tests, so nothing touches real disks. It takes the name
//! org.freedesktop.UDisks2 on the bus at ADDRESS (default: the session
//! bus, `DBUS_SESSION_BUS_ADDRESS`) and serves four sample disks: a system
//! SSD, a USB stick, an encrypted backup disk (passphrase "secret") and an
//! empty SD card. It answers what `olo_system::disks` asks: the objects,
//! mounting, formatting (as a job with progress that can be cancelled),
//! partitioning, labels, unlocking, ejecting, SMART, and the restore and
//! benchmark file descriptors (onto sparse files in its directory). Point
//! apps at it with `OLO_UDISKS_BUS=ADDRESS`.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

const ROOT: &str = "/org/freedesktop/UDisks2";
const GIB: u64 = 1 << 30;
const MIB: u64 = 1 << 20;

#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.freedesktop.UDisks2.Error")]
enum Error {
    #[zbus(error)]
    ZBus(zbus::Error),
    Failed(String),
    Cancelled(String),
    DeviceBusy(String),
    NotSupported(String),
}

#[derive(Debug, Clone, Default)]
struct Content {
    /// "" when empty.
    kind: String,
    label: String,
    uuid: String,
    mounted: Option<String>,
    /// For crypto_LUKS: its passphrase, and what is inside once unlocked.
    passphrase: String,
    inner: Option<Box<Content>>,
    unlocked: bool,
}

impl Content {
    fn new(kind: &str, label: &str) -> Content {
        Content { kind: kind.into(), label: label.into(), uuid: uuid(), ..Default::default() }
    }
    fn usage(&self) -> &'static str {
        match self.kind.as_str() {
            "" => "",
            "crypto_LUKS" => "crypto",
            "swap" => "other",
            _ => "filesystem",
        }
    }
    fn mountable(&self) -> bool {
        self.usage() == "filesystem"
    }
    fn busy(&self) -> bool {
        self.mounted.is_some() || self.unlocked
    }
}

#[derive(Debug, Clone)]
struct Part {
    number: u32,
    offset: u64,
    size: u64,
    name: String,
    ptype: String,
    content: Content,
    system: bool,
}

#[derive(Debug, Clone)]
struct Disk {
    /// "sda".
    dev: String,
    vendor: String,
    model: String,
    serial: String,
    size: u64,
    removable: bool,
    bus: String,
    rotation: i32,
    temperature: f64,
    hours: u64,
    present: bool,
    table: Option<String>,
    parts: Vec<Part>,
    /// Without a table: what fills it.
    content: Content,
}

impl Disk {
    fn drive_path(&self) -> String {
        format!("{ROOT}/drives/{}_{}", self.model.replace(' ', "_"), self.serial)
    }
    fn block_path(&self) -> String {
        format!("{ROOT}/block_devices/{}", self.dev)
    }
    fn part_dev(&self, n: u32) -> String {
        part_name(&self.dev, n)
    }
}

#[derive(Debug, Clone)]
struct Job {
    id: u32,
    operation: String,
    progress: f64,
    objects: Vec<String>,
}

#[derive(Default)]
struct Model {
    disks: Vec<Disk>,
    jobs: Vec<Job>,
    next_job: u32,
    dir: PathBuf,
}

/// Where a block path points in the model.
enum At {
    Disk(usize),
    Part(usize, usize),
    /// An unlocked volume's contents (by its partition).
    Clear(usize, usize),
}

impl Model {
    fn find(&self, path: &str) -> Option<At> {
        let name = path.strip_prefix(&format!("{ROOT}/block_devices/"))?;
        for (i, d) in self.disks.iter().enumerate().filter(|(_, d)| d.present) {
            if name == d.dev {
                return Some(At::Disk(i));
            }
            for (j, p) in d.parts.iter().enumerate() {
                if name == d.part_dev(p.number) {
                    return Some(At::Part(i, j));
                }
                if name == format!("dm_2d{}", d.part_dev(p.number)) && p.content.unlocked {
                    return Some(At::Clear(i, j));
                }
            }
        }
        None
    }

    fn content(&mut self, at: &At) -> &mut Content {
        match *at {
            At::Disk(i) => &mut self.disks[i].content,
            At::Part(i, j) => &mut self.disks[i].parts[j].content,
            At::Clear(i, j) => self.disks[i].parts[j].content.inner.get_or_insert_with(Default::default),
        }
    }

    fn image(&self, dev: &str) -> PathBuf {
        self.dir.join(format!("{dev}.img"))
    }

    fn objects(&self) -> HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>> {
        let mut out = HashMap::new();
        let mut add = |path: &str, iface: &str, props: Vec<(&str, OwnedValue)>| {
            out.entry(op(path)).or_insert_with(HashMap::new).insert(format!("org.freedesktop.UDisks2.{iface}"), props.into_iter().map(|(k, v)| (k.to_owned(), v)).collect::<HashMap<_, _>>());
        };
        for d in self.disks.iter().filter(|d| d.present) {
            let drive = d.drive_path();
            add(
                &drive,
                "Drive",
                vec![
                    ("Vendor", ov(d.vendor.as_str())),
                    ("Model", ov(d.model.as_str())),
                    ("Serial", ov(d.serial.as_str())),
                    ("Revision", ov("1.0")),
                    ("Size", ov(d.size)),
                    ("Removable", ov(d.removable)),
                    ("MediaRemovable", ov(d.bus == "sdio")),
                    ("Ejectable", ov(d.removable)),
                    ("CanPowerOff", ov(d.removable)),
                    ("MediaAvailable", ov(true)),
                    ("ConnectionBus", ov(d.bus.as_str())),
                    ("RotationRate", ov(d.rotation)),
                    ("Optical", ov(false)),
                ],
            );
            if d.temperature > 0.0 {
                add(
                    &drive,
                    "Drive.Ata",
                    vec![
                        ("SmartSupported", ov(true)),
                        ("SmartEnabled", ov(true)),
                        ("SmartTemperature", ov(d.temperature + 273.15)),
                        ("SmartFailing", ov(false)),
                        ("SmartPowerOnSeconds", ov(d.hours * 3600)),
                    ],
                );
            }
            let block = d.block_path();
            let system = d.parts.iter().any(|p| p.system);
            let whole = if d.table.is_some() { Content::default() } else { d.content.clone() };
            add(&block, "Block", block_props(&format!("/dev/{}", d.dev), d.size, &drive, &whole, system, "/"));
            match &d.table {
                Some(t) => {
                    let parts: Vec<OwnedObjectPath> = d.parts.iter().map(|p| op(&format!("{ROOT}/block_devices/{}", d.part_dev(p.number)))).collect();
                    add(&block, "PartitionTable", vec![("Type", ov(t.as_str())), ("Partitions", ov(parts))]);
                }
                None => self.content_ifaces(&block, &whole, &d.dev, &mut add),
            }
            for p in &d.parts {
                let path = format!("{ROOT}/block_devices/{}", d.part_dev(p.number));
                add(&path, "Block", block_props(&format!("/dev/{}", d.part_dev(p.number)), p.size, &drive, &p.content, p.system, "/"));
                add(
                    &path,
                    "Partition",
                    vec![
                        ("Number", ov(p.number)),
                        ("Offset", ov(p.offset)),
                        ("Size", ov(p.size)),
                        ("Name", ov(p.name.as_str())),
                        ("Type", ov(p.ptype.as_str())),
                        ("Table", ov(ObjectPath::try_from(block.as_str()).unwrap())),
                        ("IsContainer", ov(false)),
                        ("IsContained", ov(false)),
                    ],
                );
                self.content_ifaces(&path, &p.content, &d.part_dev(p.number), &mut add);
                if let Some(inner) = p.content.inner.as_ref().filter(|_| p.content.unlocked) {
                    let clear = format!("{ROOT}/block_devices/dm_2d{}", d.part_dev(p.number));
                    add(&clear, "Block", block_props(&format!("/dev/mapper/luks-{}", d.part_dev(p.number)), p.size - 16 * MIB, "/", inner, p.system, &path));
                    if inner.mountable() {
                        add(&clear, "Filesystem", vec![("MountPoints", ov(mount_points(inner))), ("Size", ov(p.size - 16 * MIB))]);
                    }
                }
            }
        }
        for j in &self.jobs {
            let objects: Vec<OwnedObjectPath> = j.objects.iter().map(|o| op(o)).collect();
            add(
                &format!("{ROOT}/jobs/{}", j.id),
                "Job",
                vec![
                    ("Operation", ov(j.operation.as_str())),
                    ("Progress", ov(j.progress)),
                    ("ProgressValid", ov(true)),
                    ("Objects", ov(objects)),
                    ("Cancelable", ov(true)),
                    ("Rate", ov(0u64)),
                    ("ExpectedEndTime", ov(0u64)),
                ],
            );
        }
        out
    }

    fn content_ifaces(&self, path: &str, c: &Content, dev: &str, add: &mut impl FnMut(&str, &str, Vec<(&str, OwnedValue)>)) {
        if c.mountable() {
            add(path, "Filesystem", vec![("MountPoints", ov(mount_points(c))), ("Size", ov(0u64))]);
        }
        if c.kind == "crypto_LUKS" {
            let clear = if c.unlocked { format!("{ROOT}/block_devices/dm_2d{dev}") } else { "/".into() };
            add(path, "Encrypted", vec![("CleartextDevice", ov(ObjectPath::try_from(clear.as_str()).unwrap()))]);
        }
    }
}

fn block_props(device: &str, size: u64, drive: &str, c: &Content, system: bool, backing: &str) -> Vec<(&'static str, OwnedValue)> {
    vec![
        ("Device", ov(nul(device))),
        ("PreferredDevice", ov(nul(device))),
        ("Size", ov(size)),
        ("Drive", ov(ObjectPath::try_from(drive).unwrap())),
        ("CryptoBackingDevice", ov(ObjectPath::try_from(backing).unwrap())),
        ("IdType", ov(c.kind.as_str())),
        ("IdUsage", ov(c.usage())),
        ("IdLabel", ov(c.label.as_str())),
        ("IdUUID", ov(c.uuid.as_str())),
        ("ReadOnly", ov(false)),
        ("HintSystem", ov(system)),
    ]
}

/// "sda" 1 → "sda1"; "nvme0n1" 1 → "nvme0n1p1", as the kernel names them.
fn part_name(dev: &str, n: u32) -> String {
    if dev.ends_with(|c: char| c.is_ascii_digit()) { format!("{dev}p{n}") } else { format!("{dev}{n}") }
}

fn mount_points(c: &Content) -> Vec<Vec<u8>> {
    c.mounted.iter().map(|m| nul(m)).collect()
}

fn nul(s: &str) -> Vec<u8> {
    let mut v = s.as_bytes().to_vec();
    v.push(0);
    v
}

fn ov<'a>(v: impl Into<Value<'a>>) -> OwnedValue {
    v.into().try_to_owned().expect("plain value")
}

fn op(s: &str) -> OwnedObjectPath {
    OwnedObjectPath::try_from(s.to_owned()).expect("object path")
}

fn uuid() -> String {
    let mut b = [0u8; 16];
    let _ = getrandom::fill(&mut b);
    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        u32::from_be_bytes([b[0], b[1], b[2], b[3]]),
        u16::from_be_bytes([b[4], b[5]]),
        u16::from_be_bytes([b[6], b[7]]),
        u16::from_be_bytes([b[8], b[9]]),
        u64::from_be_bytes([0, 0, b[10], b[11], b[12], b[13], b[14], b[15]])
    )
}

fn sample(dir: PathBuf) -> Model {
    let mut efi = Content::new("vfat", "EFI");
    efi.mounted = Some("/boot/efi".into());
    let mut boot = Content::new("ext4", "boot");
    boot.mounted = Some("/boot".into());
    let mut root = Content::new("btrfs", "fedora");
    root.mounted = Some("/".into());
    let ssd = Disk {
        dev: "nvme0n1".into(),
        vendor: "Olo".into(),
        model: "Test SSD".into(),
        serial: "TBL0001".into(),
        size: 256 * GIB,
        removable: false,
        bus: String::new(),
        rotation: 0,
        temperature: 38.0,
        hours: 1234,
        present: true,
        table: Some("gpt".into()),
        parts: vec![
            Part { number: 1, offset: MIB, size: 600 * MIB, name: "EFI System Partition".into(), ptype: "c12a7328-f81f-11d2-ba4b-00a0c93ec93b".into(), content: efi, system: true },
            Part { number: 2, offset: 601 * MIB, size: GIB, name: String::new(), ptype: "0fc63daf-8483-4772-8e79-3d69d8477de4".into(), content: boot, system: true },
            Part { number: 3, offset: 1625 * MIB, size: 200 * GIB, name: String::new(), ptype: "0fc63daf-8483-4772-8e79-3d69d8477de4".into(), content: root, system: true },
        ],
        content: Content::default(),
    };
    let stick = Disk {
        dev: "sda".into(),
        vendor: "Olo".into(),
        model: "USB Stick".into(),
        serial: "TBL0002".into(),
        size: 32 * GIB,
        removable: true,
        bus: "usb".into(),
        rotation: -1,
        temperature: 0.0,
        hours: 0,
        present: true,
        table: Some("dos".into()),
        parts: vec![Part { number: 1, offset: MIB, size: 32 * GIB - MIB, name: String::new(), ptype: "0x07".into(), content: Content::new("exfat", "STICK"), system: false }],
        content: Content::default(),
    };
    let mut luks = Content::new("crypto_LUKS", "");
    luks.passphrase = "secret".into();
    luks.inner = Some(Box::new(Content::new("ext4", "Backups")));
    let backup = Disk {
        dev: "sdb".into(),
        vendor: "Olo".into(),
        model: "Backup Disk".into(),
        serial: "TBL0003".into(),
        size: 2000 * GIB,
        removable: true,
        bus: "usb".into(),
        rotation: 7200,
        temperature: 31.0,
        hours: 20_480,
        present: true,
        table: Some("gpt".into()),
        parts: vec![Part { number: 1, offset: MIB, size: 1500 * GIB, name: "Backups".into(), ptype: "ca7d7ccb-63ed-4c53-861c-1742536059cc".into(), content: luks, system: false }],
        content: Content::default(),
    };
    let card = Disk {
        dev: "mmcblk0".into(),
        vendor: String::new(),
        model: "SD Card".into(),
        serial: "TBL0004".into(),
        size: 64 * GIB,
        removable: true,
        bus: "sdio".into(),
        rotation: -1,
        temperature: 0.0,
        hours: 0,
        present: true,
        table: None,
        parts: Vec::new(),
        content: Content::default(),
    };
    Model { disks: vec![ssd, stick, backup, card], jobs: Vec::new(), next_job: 1, dir }
}

type Shared = Arc<Mutex<Model>>;

/// Tell clients something changed (any signal from us will do).
async fn changed(conn: &zbus::Connection) {
    let _ = conn.emit_signal(None::<()>, ROOT, "com.olodesktop.UDisksFake", "Changed", &()).await;
}

/// Run a job over `object` for `secs`, with progress, that can be
/// cancelled.
async fn job(model: &Shared, conn: &zbus::Connection, operation: &str, object: &str, secs: f64) -> std::result::Result<(), Error> {
    let cancel = Arc::new(AtomicBool::new(false));
    let id = {
        let mut m = model.lock().unwrap();
        let id = m.next_job;
        m.next_job += 1;
        m.jobs.push(Job { id, operation: operation.into(), progress: 0.0, objects: vec![object.into()] });
        id
    };
    let path = format!("{ROOT}/jobs/{id}");
    let _ = conn.object_server().at(path.as_str(), JobObject { cancel: cancel.clone() }).await;
    changed(conn).await;
    let steps = (secs * 10.0).max(1.0) as u32;
    let mut stopped = false;
    for i in 1..=steps {
        async_io::Timer::after(Duration::from_millis(100)).await;
        if cancel.load(Ordering::Relaxed) {
            stopped = true;
            break;
        }
        if let Some(j) = model.lock().unwrap().jobs.iter_mut().find(|j| j.id == id) {
            j.progress = i as f64 / steps as f64;
        }
        changed(conn).await;
    }
    model.lock().unwrap().jobs.retain(|j| j.id != id);
    let _ = conn.object_server().remove::<JobObject, _>(path.as_str()).await;
    changed(conn).await;
    if stopped { Err(Error::Cancelled("Job was cancelled".into())) } else { Ok(()) }
}

struct JobObject {
    cancel: Arc<AtomicBool>,
}

#[zbus::interface(name = "org.freedesktop.UDisks2.Job")]
impl JobObject {
    fn cancel(&self, _options: HashMap<String, OwnedValue>) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

struct Manager {
    model: Shared,
}

#[zbus::interface(name = "org.freedesktop.DBus.ObjectManager")]
impl Manager {
    fn get_managed_objects(&self) -> HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>> {
        self.model.lock().unwrap().objects()
    }
}

struct UDisksManager;

#[zbus::interface(name = "org.freedesktop.UDisks2.Manager")]
impl UDisksManager {
    #[zbus(property)]
    fn supported_filesystems(&self) -> Vec<String> {
        ["ext4", "xfs", "vfat", "ntfs", "exfat", "btrfs", "swap"].map(String::from).to_vec()
    }
    #[zbus(property)]
    fn version(&self) -> String {
        "2.11.2 (fake)".into()
    }
    fn can_resize(&self, kind: String) -> (bool, u64, String) {
        match kind.as_str() {
            "ext4" | "btrfs" => (true, 2 | 4 | 8 | 16, String::new()),
            "xfs" => (true, 4 | 16, String::new()),
            "ntfs" | "vfat" | "exfat" => (true, 2 | 4, String::new()),
            _ => (false, 0, String::new()),
        }
    }
}

/// Every interface with methods, for one block device path.
struct Block {
    model: Shared,
    path: String,
}

fn opts_str(o: &HashMap<String, OwnedValue>, key: &str) -> String {
    o.get(key).and_then(|v| String::try_from(v.try_clone().ok()?).ok()).unwrap_or_default()
}

fn label_of(kind: &str, label: &str) -> String {
    if matches!(kind, "vfat") { label.to_uppercase().chars().take(11).collect() } else { label.to_owned() }
}

fn new_content(kind: &str, o: &HashMap<String, OwnedValue>) -> Content {
    let label = label_of(kind, &opts_str(o, "label"));
    let passphrase = opts_str(o, "encrypt.passphrase");
    if kind == "empty" || kind.is_empty() {
        return Content::default();
    }
    if passphrase.is_empty() {
        return Content::new(kind, &label);
    }
    let mut c = Content::new("crypto_LUKS", "");
    c.passphrase = passphrase;
    c.inner = Some(Box::new(Content::new(kind, &label)));
    c
}

impl Block {
    fn at(&self) -> std::result::Result<At, Error> {
        self.model.lock().unwrap().find(&self.path).ok_or_else(|| Error::Failed(format!("No such device {}", self.path)))
    }

    fn not_busy(&self) -> std::result::Result<(), Error> {
        let at = self.at()?;
        let mut m = self.model.lock().unwrap();
        let busy = match at {
            At::Disk(i) => m.disks[i].content.busy() || m.disks[i].parts.iter().any(|p| p.content.busy()),
            ref a => m.content(a).busy(),
        };
        if busy { Err(Error::DeviceBusy(format!("{} is in use (mounted or unlocked)", self.path))) } else { Ok(()) }
    }

    fn file(&self) -> std::result::Result<zvariant::OwnedFd, Error> {
        let m = self.model.lock().unwrap();
        let (dev, size) = match m.find(&self.path) {
            Some(At::Disk(i)) => (m.disks[i].dev.clone(), m.disks[i].size),
            Some(At::Part(i, j)) => (m.disks[i].part_dev(m.disks[i].parts[j].number), m.disks[i].parts[j].size),
            _ => return Err(Error::Failed("No such device".into())),
        };
        let path = m.image(&dev);
        let f = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&path);
        let f = f.map_err(|e| Error::Failed(e.to_string()))?;
        if f.metadata().map(|md| md.len()).unwrap_or(0) < size {
            let w = std::fs::OpenOptions::new().write(true).open(&path).map_err(|e| Error::Failed(e.to_string()))?;
            w.set_len(size).map_err(|e| Error::Failed(e.to_string()))?;
        }
        Ok(zvariant::OwnedFd::from(std::os::fd::OwnedFd::from(f)))
    }
}

#[zbus::interface(name = "org.freedesktop.UDisks2.Block")]
impl Block {
    async fn format(&self, kind: String, options: HashMap<String, OwnedValue>, #[zbus(connection)] conn: &zbus::Connection) -> std::result::Result<(), Error> {
        self.not_busy()?;
        let erase = !opts_str(&options, "erase").is_empty();
        let operation = if erase { "format-erase".to_owned() } else { format!("format-{kind}") };
        job(&self.model, conn, &operation, &self.path, if erase { 4.0 } else { 1.5 }).await?;
        let at = self.at()?;
        {
            let mut m = self.model.lock().unwrap();
            match at {
                At::Disk(i) if kind == "gpt" || kind == "dos" => {
                    let d = &mut m.disks[i];
                    d.table = Some(kind);
                    d.parts.clear();
                    d.content = Content::default();
                }
                At::Disk(i) => {
                    let d = &mut m.disks[i];
                    d.table = None;
                    d.parts.clear();
                    d.content = new_content(&kind, &options);
                }
                ref a => *m.content(a) = new_content(&kind, &options),
            }
        }
        changed(conn).await;
        Ok(())
    }

    /// A pipe drained at a disk's pace (256 MB/s), so progress and
    /// stopping can be watched; what is written goes nowhere, so a large
    /// image doesn't fill /tmp.
    fn open_for_restore(&self, _options: HashMap<String, OwnedValue>) -> std::result::Result<zvariant::OwnedFd, Error> {
        use std::io::Read;
        self.not_busy()?;
        let (mut rx, tx) = std::io::pipe().map_err(|e| Error::Failed(e.to_string()))?;
        std::thread::spawn(move || {
            let mut buf = vec![0u8; 1 << 20];
            let start = std::time::Instant::now();
            let mut done = 0u64;
            while let Ok(n) = rx.read(&mut buf) {
                if n == 0 {
                    return;
                }
                done += n as u64;
                let due = std::time::Duration::from_secs_f64(done as f64 / (256 << 20) as f64);
                if let Some(wait) = due.checked_sub(start.elapsed()) {
                    std::thread::sleep(wait);
                }
            }
        });
        Ok(zvariant::OwnedFd::from(std::os::fd::OwnedFd::from(tx)))
    }

    fn open_for_benchmark(&self, _options: HashMap<String, OwnedValue>) -> std::result::Result<zvariant::OwnedFd, Error> {
        self.file()
    }
}

struct Filesystem {
    model: Shared,
    path: String,
}

#[zbus::interface(name = "org.freedesktop.UDisks2.Filesystem")]
impl Filesystem {
    async fn mount(&self, _options: HashMap<String, OwnedValue>, #[zbus(connection)] conn: &zbus::Connection) -> std::result::Result<String, Error> {
        let at = Block { model: self.model.clone(), path: self.path.clone() }.at()?;
        let point = {
            let mut m = self.model.lock().unwrap();
            let c = m.content(&at);
            if let Some(p) = &c.mounted {
                return Err(Error::Failed(format!("Already mounted at {p}")));
            }
            let name = if c.label.is_empty() { c.uuid.clone() } else { c.label.clone() };
            let user = std::env::var("USER").unwrap_or_else(|_| "user".into());
            let point = format!("/run/media/{user}/{name}");
            c.mounted = Some(point.clone());
            point
        };
        changed(conn).await;
        Ok(point)
    }

    async fn unmount(&self, _options: HashMap<String, OwnedValue>, #[zbus(connection)] conn: &zbus::Connection) -> std::result::Result<(), Error> {
        let at = Block { model: self.model.clone(), path: self.path.clone() }.at()?;
        {
            let mut m = self.model.lock().unwrap();
            let c = m.content(&at);
            if c.mounted.as_deref() == Some("/") {
                return Err(Error::DeviceBusy("target is busy".into()));
            }
            if c.mounted.take().is_none() {
                return Err(Error::Failed("Not mounted".into()));
            }
        }
        changed(conn).await;
        Ok(())
    }

    async fn set_label(&self, label: String, _options: HashMap<String, OwnedValue>, #[zbus(connection)] conn: &zbus::Connection) -> std::result::Result<(), Error> {
        let at = Block { model: self.model.clone(), path: self.path.clone() }.at()?;
        {
            let mut m = self.model.lock().unwrap();
            let c = m.content(&at);
            c.label = label_of(&c.kind.clone(), &label);
        }
        changed(conn).await;
        Ok(())
    }

    async fn resize(&self, _size: u64, _options: HashMap<String, OwnedValue>, #[zbus(connection)] conn: &zbus::Connection) -> std::result::Result<(), Error> {
        job(&self.model, conn, "filesystem-resize", &self.path, 1.0).await
    }
}

struct Partition {
    model: Shared,
    path: String,
}

#[zbus::interface(name = "org.freedesktop.UDisks2.Partition")]
impl Partition {
    async fn delete(&self, _options: HashMap<String, OwnedValue>, #[zbus(connection)] conn: &zbus::Connection) -> std::result::Result<(), Error> {
        let b = Block { model: self.model.clone(), path: self.path.clone() };
        b.not_busy()?;
        let At::Part(i, j) = b.at()? else { return Err(Error::Failed("Not a partition".into())) };
        self.model.lock().unwrap().disks[i].parts.remove(j);
        changed(conn).await;
        Ok(())
    }

    async fn resize(&self, size: u64, _options: HashMap<String, OwnedValue>, #[zbus(connection)] conn: &zbus::Connection) -> std::result::Result<(), Error> {
        let b = Block { model: self.model.clone(), path: self.path.clone() };
        let At::Part(i, j) = b.at()? else { return Err(Error::Failed("Not a partition".into())) };
        {
            let m = self.model.lock().unwrap();
            let d = &m.disks[i];
            let p = &d.parts[j];
            let next = d.parts.iter().filter(|q| q.offset > p.offset).map(|q| q.offset).min().unwrap_or(d.size - if d.table.as_deref() == Some("gpt") { MIB } else { 0 });
            if p.offset + size > next {
                return Err(Error::Failed("Not enough free space after the partition".into()));
            }
        }
        job(&self.model, conn, "partition-resize", &self.path, 1.0).await?;
        self.model.lock().unwrap().disks[i].parts[j].size = size;
        changed(conn).await;
        Ok(())
    }

    async fn set_name(&self, name: String, _options: HashMap<String, OwnedValue>, #[zbus(connection)] conn: &zbus::Connection) -> std::result::Result<(), Error> {
        let b = Block { model: self.model.clone(), path: self.path.clone() };
        let At::Part(i, j) = b.at()? else { return Err(Error::Failed("Not a partition".into())) };
        self.model.lock().unwrap().disks[i].parts[j].name = name;
        changed(conn).await;
        Ok(())
    }
}

struct Table {
    model: Shared,
    path: String,
}

#[zbus::interface(name = "org.freedesktop.UDisks2.PartitionTable")]
impl Table {
    #[allow(clippy::too_many_arguments)]
    async fn create_partition_and_format(
        &self,
        offset: u64,
        size: u64,
        ptype: String,
        name: String,
        _options: HashMap<String, OwnedValue>,
        kind: String,
        format_options: HashMap<String, OwnedValue>,
        #[zbus(connection)] conn: &zbus::Connection,
    ) -> std::result::Result<OwnedObjectPath, Error> {
        let b = Block { model: self.model.clone(), path: self.path.clone() };
        let At::Disk(i) = b.at()? else { return Err(Error::Failed("Not a disk".into())) };
        let number = {
            let m = self.model.lock().unwrap();
            let d = &m.disks[i];
            let end = d.size - if d.table.as_deref() == Some("gpt") { MIB } else { 0 };
            if offset < MIB || offset + size > end || d.parts.iter().any(|p| offset < p.offset + p.size && p.offset < offset + size) {
                return Err(Error::Failed("The requested space overlaps a partition or the end of the disk".into()));
            }
            if d.table.as_deref() == Some("dos") && d.parts.len() >= 4 {
                return Err(Error::NotSupported("An MBR disk holds four primary partitions".into()));
            }
            (1..).find(|n| !d.parts.iter().any(|p| p.number == *n)).unwrap_or(1)
        };
        let path = {
            let mut m = self.model.lock().unwrap();
            let d = &mut m.disks[i];
            d.parts.push(Part { number, offset, size, name, ptype, content: Content::default(), system: false });
            d.parts.sort_by_key(|p| p.offset);
            format!("{ROOT}/block_devices/{}", d.part_dev(number))
        };
        changed(conn).await;
        job(&self.model, conn, &format!("format-{kind}"), &path, 1.5).await?;
        {
            let mut m = self.model.lock().unwrap();
            if let Some(at) = m.find(&path) {
                *m.content(&at) = new_content(&kind, &format_options);
            }
        }
        changed(conn).await;
        Ok(op(&path))
    }
}

struct Encrypted {
    model: Shared,
    path: String,
}

#[zbus::interface(name = "org.freedesktop.UDisks2.Encrypted")]
impl Encrypted {
    async fn unlock(&self, passphrase: String, _options: HashMap<String, OwnedValue>, #[zbus(connection)] conn: &zbus::Connection) -> std::result::Result<OwnedObjectPath, Error> {
        let b = Block { model: self.model.clone(), path: self.path.clone() };
        let At::Part(i, j) = b.at()? else { return Err(Error::Failed("Not a partition".into())) };
        let clear = {
            let mut m = self.model.lock().unwrap();
            let d = &mut m.disks[i];
            let dev = d.part_dev(d.parts[j].number);
            let c = &mut d.parts[j].content;
            if c.passphrase != passphrase {
                return Err(Error::Failed("Error unlocking: Failed to activate device: Operation not permitted".into()));
            }
            c.unlocked = true;
            format!("{ROOT}/block_devices/dm_2d{dev}")
        };
        changed(conn).await;
        Ok(op(&clear))
    }

    async fn lock(&self, _options: HashMap<String, OwnedValue>, #[zbus(connection)] conn: &zbus::Connection) -> std::result::Result<(), Error> {
        let b = Block { model: self.model.clone(), path: self.path.clone() };
        let At::Part(i, j) = b.at()? else { return Err(Error::Failed("Not a partition".into())) };
        {
            let mut m = self.model.lock().unwrap();
            let c = &mut m.disks[i].parts[j].content;
            if c.inner.as_ref().is_some_and(|x| x.mounted.is_some()) {
                return Err(Error::DeviceBusy("The unlocked volume is mounted".into()));
            }
            c.unlocked = false;
        }
        changed(conn).await;
        Ok(())
    }
}

struct DriveObject {
    model: Shared,
    path: String,
}

impl DriveObject {
    fn index(&self) -> std::result::Result<usize, Error> {
        self.model.lock().unwrap().disks.iter().position(|d| d.present && d.drive_path() == self.path).ok_or_else(|| Error::Failed("No such drive".into()))
    }
}

#[zbus::interface(name = "org.freedesktop.UDisks2.Drive")]
impl DriveObject {
    async fn eject(&self, _options: HashMap<String, OwnedValue>) -> std::result::Result<(), Error> {
        let i = self.index()?;
        if self.model.lock().unwrap().disks[i].parts.iter().any(|p| p.content.busy()) {
            return Err(Error::DeviceBusy("A volume on the drive is in use".into()));
        }
        Ok(())
    }

    async fn power_off(&self, _options: HashMap<String, OwnedValue>, #[zbus(connection)] conn: &zbus::Connection) -> std::result::Result<(), Error> {
        let i = self.index()?;
        {
            let mut m = self.model.lock().unwrap();
            if m.disks[i].parts.iter().any(|p| p.content.busy()) || m.disks[i].content.busy() {
                return Err(Error::DeviceBusy("A volume on the drive is in use".into()));
            }
            m.disks[i].present = false;
        }
        changed(conn).await;
        Ok(())
    }
}

struct Ata;

#[zbus::interface(name = "org.freedesktop.UDisks2.Drive.Ata")]
impl Ata {
    fn smart_update(&self, _options: HashMap<String, OwnedValue>) {}
}

fn main() {
    let mut address = std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap_or_default();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--address" => address = args.next().expect("--address ADDRESS"),
            _ => {
                eprintln!("usage: udisks-fake [--address ADDRESS]");
                std::process::exit(2);
            }
        }
    }
    let dir = std::env::temp_dir().join(format!("udisks-fake-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let model: Shared = Arc::new(Mutex::new(sample(dir.clone())));
    let conn = match zbus::blocking::connection::Builder::address(address.as_str()).and_then(|b| b.name("org.freedesktop.UDisks2")).and_then(|b| b.build()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("udisks-fake: can't take org.freedesktop.UDisks2 on {address}: {e}");
            std::process::exit(1);
        }
    };
    let server = conn.object_server();
    server.at(ROOT, Manager { model: model.clone() }).expect("object manager");
    server.at(format!("{ROOT}/Manager"), UDisksManager).expect("manager");
    // Every object that has methods, by the paths it could ever have.
    let paths: Vec<(String, String, Vec<u32>, bool)> = {
        let m = model.lock().unwrap();
        m.disks.iter().map(|d| (d.dev.clone(), d.drive_path(), (1..=8).collect(), d.temperature > 0.0)).collect()
    };
    for (dev, drive, numbers, ata) in paths {
        let block = format!("{ROOT}/block_devices/{dev}");
        let _ = server.at(block.as_str(), Block { model: model.clone(), path: block.clone() });
        let _ = server.at(block.as_str(), Filesystem { model: model.clone(), path: block.clone() });
        let _ = server.at(block.as_str(), Table { model: model.clone(), path: block.clone() });
        let _ = server.at(drive.as_str(), DriveObject { model: model.clone(), path: drive.clone() });
        if ata {
            let _ = server.at(drive.as_str(), Ata);
        }
        for n in numbers {
            let part = format!("{ROOT}/block_devices/{}", part_name(&dev, n));
            let _ = server.at(part.as_str(), Block { model: model.clone(), path: part.clone() });
            let _ = server.at(part.as_str(), Filesystem { model: model.clone(), path: part.clone() });
            let _ = server.at(part.as_str(), Partition { model: model.clone(), path: part.clone() });
            let _ = server.at(part.as_str(), Encrypted { model: model.clone(), path: part.clone() });
            let clear = format!("{ROOT}/block_devices/dm_2d{}", part_name(&dev, n));
            let _ = server.at(clear.as_str(), Filesystem { model: model.clone(), path: clear.clone() });
        }
    }
    eprintln!("udisks-fake: serving on {address} (images in {})", dir.display());
    loop {
        std::thread::park();
    }
}
